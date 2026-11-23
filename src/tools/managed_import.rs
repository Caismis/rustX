//! Bounded Server-side producer for Client-owned normalized output.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use base64::Engine as _;
use rustx_execution_protocol::{
    MAX_RAW_CHUNK_BYTES, OPTIONAL_SAVE_DEADLINE,
    save::{ContentFormat, Locator, SaveExtent, SaveKey, SaveReply, SaveRequest, Sha256Hex},
};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    local_runtime::session_runtime_manager::{OutputBinding, SessionExecutionScope},
    runtime::{
        cancellation::{CancellationSignal, ExecutionCancellation},
        identity::ConversationId,
        types::CancellationReason,
    },
};

use super::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;

const OUTPUT_ROOT: &str = "/.rustx-output/import";
const SAVE_WINDOW: usize = 4;
const MAX_ACTIVE_SAVES: usize = 16;
const CLEANUP_BUDGET: Duration = Duration::from_secs(2);

pub(crate) fn locator_at(path: &Path) -> Option<Locator> {
    let token = path.strip_prefix(OUTPUT_ROOT).ok()?.to_str()?;
    Locator::parse(&format!("save:{token}"))
}

fn locator_path(locator: Locator) -> Option<PathBuf> {
    let name = String::from(locator);
    Some(Path::new(OUTPUT_ROOT).join(name.strip_prefix("save:")?))
}

pub(crate) enum Import {
    Prefix {
        scope: SessionExecutionScope,
        conversation: ConversationId,
        cancellation: ExecutionCancellation,
        prefix: Vec<u8>,
    },
    Active(Writer),
    Unavailable {
        selected: bool,
    },
}

pub(crate) struct Writer {
    messages: mpsc::Sender<Message>,
    abort: CancellationToken,
    settled: oneshot::Receiver<Option<Locator>>,
    deadline: Instant,
}

enum Message {
    Text(Vec<u8>),
    Publish,
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.abort.cancel();
    }
}

impl Import {
    pub(crate) fn new(
        scope: SessionExecutionScope,
        conversation: ConversationId,
        cancellation: ExecutionCancellation,
    ) -> Self {
        Self::Prefix {
            scope,
            conversation,
            cancellation,
            prefix: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, fragment: &str) {
        self.feed(fragment, Writer::push);
    }

    fn feed(&mut self, fragment: &str, send: fn(&Writer, Vec<u8>) -> bool) {
        if fragment.is_empty() {
            return;
        }
        match self {
            Self::Prefix {
                scope,
                conversation,
                cancellation,
                prefix,
            } => {
                if prefix.len() + fragment.len() <= FOREGROUND_TOOL_RESULT_PREVIEW_BYTES {
                    prefix.extend_from_slice(fragment.as_bytes());
                    return;
                }
                let binding = scope.bind_output(conversation.as_str());
                let selected = binding.is_some();
                let writer = binding.and_then(|binding| {
                    if cancellation.is_cancelled() {
                        return None;
                    }
                    Writer::start(binding, cancellation)
                });
                if let Some(writer) = writer {
                    let mut first = std::mem::take(prefix);
                    first.extend_from_slice(fragment.as_bytes());
                    if send(&writer, first) {
                        *self = Self::Active(writer);
                        return;
                    }
                }
                *self = Self::Unavailable { selected };
            }
            Self::Active(writer) => {
                if !send(writer, fragment.as_bytes().to_vec()) {
                    *self = Self::Unavailable { selected: true };
                }
            }
            Self::Unavailable { .. } => {}
        }
    }

    pub(crate) fn selected(&self) -> bool {
        matches!(self, Self::Active(_) | Self::Unavailable { selected: true })
    }

    pub(crate) async fn publish(self) -> Option<PathBuf> {
        let Self::Active(mut writer) = self else {
            return None;
        };
        tokio::time::timeout_at(tokio::time::Instant::from_std(writer.deadline), async {
            writer.messages.send(Message::Publish).await.ok()?;
            (&mut writer.settled)
                .await
                .ok()
                .flatten()
                .and_then(locator_path)
        })
        .await
        .ok()
        .flatten()
    }
}

impl Writer {
    fn start(binding: OutputBinding, cancellation: &ExecutionCancellation) -> Option<Self> {
        static SAVES: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let permit = SAVES
            .get_or_init(|| Arc::new(Semaphore::new(MAX_ACTIVE_SAVES)))
            .clone()
            .try_acquire_owned()
            .ok()?;
        let deadline = Instant::now() + OPTIONAL_SAVE_DEADLINE;
        let abort = CancellationToken::new();
        let discarded = abort.clone();
        let signal = cancellation.child_signal();
        let cancellation = ExecutionCancellation::detached(signal.clone(), cancellation.reason());
        let (messages, input) = mpsc::channel(SAVE_WINDOW);
        let (reply, settled) = oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let save = receive(binding, input, cancellation, deadline);
            tokio::pin!(save);
            let locator = tokio::select! {
                biased;
                () = discarded.cancelled() => {
                    // Dropping a losing candidate may happen under the native
                    // claim gate. Only the receiver touches that gate here,
                    // and it still collects any reply already in flight.
                    signal.cancel();
                    save.await
                }
                locator = &mut save => locator,
            };
            let _ = reply.send(locator);
        });
        Some(Self {
            messages,
            abort,
            settled,
            deadline,
        })
    }

    fn push(&self, bytes: Vec<u8>) -> bool {
        Instant::now() < self.deadline && self.messages.try_send(Message::Text(bytes)).is_ok()
    }

    fn push_blocking(&self, bytes: Vec<u8>) -> bool {
        // The managed receiver stops reading at the same absolute deadline
        // and drops the receive side before cleanup.
        Instant::now() < self.deadline && self.messages.blocking_send(Message::Text(bytes)).is_ok()
    }
}

/// Coalesces the MCP serializer's tiny fragments in one raw-chunk window.
/// It is used on the blocking pool after the complete remote result arrives.
pub(crate) struct BufferedImport {
    import: Import,
    buffer: String,
}

impl BufferedImport {
    pub(crate) fn new(import: Import) -> Self {
        Self {
            import,
            buffer: String::new(),
        }
    }

    pub(crate) fn push(&mut self, mut text: &str) {
        while !text.is_empty() {
            match self.import {
                Import::Unavailable { .. } => return,
                Import::Prefix { .. } => {
                    let split = text.floor_char_boundary(MAX_RAW_CHUNK_BYTES.min(text.len()));
                    self.import.feed(&text[..split], Writer::push_blocking);
                    text = &text[split..];
                }
                Import::Active(_) => {
                    let remaining = MAX_RAW_CHUNK_BYTES - self.buffer.len();
                    let split = text.floor_char_boundary(remaining.min(text.len()));
                    self.buffer.push_str(&text[..split]);
                    text = &text[split..];
                    if !text.is_empty() {
                        self.flush();
                    }
                }
            }
        }
    }

    fn flush(&mut self) {
        self.import.feed(&self.buffer, Writer::push_blocking);
        self.buffer.clear();
    }

    pub(crate) fn finish(mut self) -> Import {
        self.flush();
        self.import
    }
}

async fn receive(
    binding: OutputBinding,
    mut input: mpsc::Receiver<Message>,
    cancellation: ExecutionCancellation,
    deadline: Instant,
) -> Option<Locator> {
    let SaveReply::Begun { key } = binding
        .exchange(
            SaveRequest::Begin {
                owner: binding.owner.clone(),
                content_format: ContentFormat::NormalizedUtf8V1,
            },
            deadline,
            &cancellation,
        )
        .await
        .ok()?
    else {
        return None;
    };
    let mut digest = Sha256::new();
    let mut offset = 0_u64;
    let mut sequence = 1_u64;
    let mut commit = false;
    let expires = tokio::time::Instant::from_std(deadline);
    'receive: loop {
        let message = tokio::select! {
            biased;
            () = cancellation.cancelled() => break,
            result = tokio::time::timeout_at(expires, input.recv()) => result.ok().flatten(),
        };
        match message {
            Some(Message::Text(data)) => {
                for chunk in data.chunks(MAX_RAW_CHUNK_BYTES) {
                    if !matches!(
                        binding
                            .exchange(
                                SaveRequest::Chunk {
                                    key: key.clone(),
                                    sequence: sequence.into(),
                                    offset: offset.into(),
                                    sha256: Sha256Hex::of(chunk),
                                    data_base64: base64::engine::general_purpose::STANDARD
                                        .encode(chunk),
                                },
                                deadline,
                                &cancellation
                            )
                            .await,
                        Ok(SaveReply::Chunked { .. })
                    ) {
                        break 'receive;
                    }
                    offset += chunk.len() as u64;
                    sequence += 1;
                    digest.update(chunk);
                }
            }
            Some(Message::Publish) => {
                commit = true;
                break;
            }
            None => break,
        }
    }
    drop(input);
    if commit {
        let sha256 = Sha256Hex::from_digest(digest.finalize().into());
        match publish(&binding, &key, offset, sha256, deadline, &cancellation).await {
            // The exact receipt confirms a locator. A transport failure
            // may have published one, so only the binding may dispose it.
            Ok(Some(locator)) => return Some(locator),
            Err(_) => return None,
            Ok(None) => {}
        }
    }
    let cleanup = ExecutionCancellation::detached(
        CancellationSignal::new(),
        CancellationReason::UserRequested,
    );
    let _ = binding
        .exchange(
            SaveRequest::Abort { key },
            Instant::now() + CLEANUP_BUDGET,
            &cleanup,
        )
        .await;
    None
}

async fn publish(
    binding: &OutputBinding,
    key: &SaveKey,
    offset: u64,
    sha256: Sha256Hex,
    deadline: Instant,
    cancellation: &ExecutionCancellation,
) -> std::io::Result<Option<Locator>> {
    let length = offset.into();
    let extent = SaveExtent::Complete;
    if !matches!(
        binding
            .exchange(
                SaveRequest::Seal {
                    key: key.clone(),
                    length,
                    sha256,
                    extent
                },
                deadline,
                cancellation
            )
            .await,
        Ok(SaveReply::Sealed { .. })
    ) || cancellation.is_cancelled()
    {
        return Ok(None);
    }
    match binding
        .exchange(
            SaveRequest::Publish {
                key: key.clone(),
                length,
                sha256,
                extent,
            },
            deadline,
            cancellation,
        )
        .await?
    {
        SaveReply::Published { locator, .. } => Ok(Some(locator)),
        _ => Ok(None),
    }
}
