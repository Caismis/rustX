//! Finite, read-only packaging of native Session history. No runtime is loaded.
mod prepare_error;
mod projection;
use crate::durable::SqliteConversationStore;
use crate::durable::sqlite::archive::{Authority, error};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::local_runtime::session::{SessionCatalog, SessionNode, SessionSnapshot};
use crate::message::types::{AssistantContentBlock, MessageBlock, UserContentBlock};
use crate::runtime::identity::{ArtifactId, ConversationId, SessionId};
use crate::runtime::local_storage::{ConversationAccess, ProductRoot};
use crate::tools::artifacts::ArtifactStore;
use crate::tools::types::{ToolExecutionResult, ToolResultContent};
pub use prepare_error::SessionArchivePrepareError;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use crate::durable::sqlite::archive::ConversationArchiveFrontiers;

/// Logical archive version, independent of durable and transport schemas.
pub const FORMAT: &str = "rustx-session-archive/v1";
/// Maximum queued byte chunks. ZIP metadata scales with entries, not bytes.
pub const STREAM_CAPACITY: usize = 2;
pub const CHUNK_BYTES: usize = 64 * 1024;

#[derive(Serialize)]
struct ConversationManifest {
    conversation_id: ConversationId,
    parent_conversation: Option<ConversationId>,
    frontiers: ConversationArchiveFrontiers,
}
struct ConversationCut {
    manifest: ConversationManifest,
    store: SqliteConversationStore,
    root: PathBuf,
    _access: ConversationAccess,
    artifact_lengths: BTreeMap<ArtifactId, Option<u64>>,
}
#[derive(Serialize)]
struct ArtifactManifest {
    conversation_id: ConversationId,
    artifact_id: ArtifactId,
    metadata: Value,
    bytes: u64,
    entry: String,
}
struct ArtifactCut {
    manifest: ArtifactManifest,
    file: crate::tools::artifacts::ArtifactReadHandle,
}

/// One finite domain. All members share the instant when the final included
/// database prefix and settled artifact lengths have been captured, before
/// releasing the first database/ownership barrier.
/// Only immutable prefixes and allocation lifetime guards survive preparation.
pub struct SessionArchiveCut {
    session: SessionSnapshot,
    cwd: PathBuf,
    nodes: Vec<SessionNode>,
    conversations: Vec<ConversationCut>,
    artifacts: Vec<ArtifactCut>,
}

/// Reusable native producer: catalog/lineage + durable history + artifact bytes.
pub struct SessionArchiveProducer;
impl SessionArchiveProducer {
    /// Capture a finite Session and its durable children, then preflight all
    /// logical records and required artifact handles. No agent composition.
    /// # Errors
    /// Missing/ambiguous lineage, damaged authorities or required bytes fail.
    pub fn prepare(
        root: &Path,
        session: &SessionId,
        cancel: &CancellationToken,
    ) -> Result<SessionArchiveCut, SessionArchivePrepareError> {
        Self::prepare_inner(
            root,
            session,
            cancel,
            #[cfg(test)]
            || {},
        )
    }

    pub(crate) fn prepare_inner(
        root: &Path,
        session: &SessionId,
        cancel: &CancellationToken,
        #[cfg(test)] before_capture: impl FnOnce(),
    ) -> Result<SessionArchiveCut, SessionArchivePrepareError> {
        check_cancel(cancel)?;
        let root = ProductRoot::existing(root)?;
        let ownership = root.freeze_ownership()?;
        let catalog =
            SessionCatalog::read_under_guard(&root)?.ok_or(SessionArchivePrepareError::Storage)?;
        let snapshot = catalog.snapshot(session)?;
        let cwd = catalog.lineage(session, None)?.1.cwd;
        let selected = crate::local_runtime::session_ownership::SessionOwnership::inspect(
            &root,
            &ownership,
            &catalog,
            || check_cancel(cancel),
        )?
        .select(session)?;
        let nodes = selected.nodes;
        let mut conversations = Vec::new();
        // Global ownership is fully validated before acquiring any archive barrier.
        // The archive consumes these facts; it never rediscovers descendants.
        for owned in selected.conversations {
            check_cancel(cancel)?;
            let unavailable = if owned.parent_conversation.is_some() {
                SessionArchivePrepareError::DescendantUnavailable
            } else {
                SessionArchivePrepareError::ConversationUnavailable
            };
            let access = ConversationAccess::existing(&root, &owned.private_root)
                .map_err(|_| unavailable)?;
            let store = SqliteConversationStore::open_existing(
                owned.conversation_id.clone(),
                &owned.database,
            )
            .map_err(|_| unavailable)?;
            conversations.push(ConversationCut {
                manifest: ConversationManifest {
                    conversation_id: owned.conversation_id,
                    parent_conversation: owned.parent_conversation,
                    frontiers: ConversationArchiveFrontiers::default(),
                },
                root: owned.private_root,
                store,
                _access: access,
                artifact_lengths: BTreeMap::new(),
            });
        }
        conversations.sort_by(|a, b| a.manifest.conversation_id.cmp(&b.manifest.conversation_id));
        // Ownership is already frozen, so lineage traversal above needed no
        // execution read barriers. Only bounded frontier/identity reads occur
        // while all included databases are simultaneously read-locked.
        #[cfg(test)]
        before_capture();
        for conversation in &mut conversations {
            conversation.store.archive_barrier()?;
            conversation.manifest.frontiers = conversation.store.archive_frontiers()?;
            conversation.artifact_lengths =
                ArtifactStore::archive_lengths(&conversation.root, || check_cancel(cancel))?;
        }
        // LINEARIZATION: all database prefixes still equal these frontiers;
        // captured settled artifact bytes are immutable native facts. The
        // reference set is a function of the frozen prefixes, resolved below.
        for conversation in &conversations {
            conversation.store.archive_release()?;
        }
        drop(ownership);
        let mut cut = SessionArchiveCut {
            session: snapshot,
            cwd,
            nodes,
            conversations,
            artifacts: Vec::new(),
        };
        cut.preflight(cancel)?;
        Ok(cut)
    }
}
impl SessionArchiveCut {
    #[must_use]
    pub fn filename(&self) -> String {
        format!("rustx-session-{}.zip", self.session.id)
    }

    fn preflight(&mut self, cancel: &CancellationToken) -> Result<(), SessionArchivePrepareError> {
        for conversation in &self.conversations {
            let mut refs = BTreeMap::<ArtifactId, Value>::new();
            conversation
                .records(cancel, |_, record| {
                    collect_record_artifacts(record, &mut refs);
                    Ok(())
                })
                .map_err(|_| {
                    if cancel.is_cancelled() {
                        SessionArchivePrepareError::Cancelled
                    } else {
                        SessionArchivePrepareError::CorruptAuthority
                    }
                })?;
            for (id, metadata) in refs {
                check_cancel(cancel)?;
                let bytes = conversation
                    .artifact_lengths
                    .get(&id)
                    .copied()
                    .flatten()
                    .ok_or(SessionArchivePrepareError::ArtifactUnavailable)?;
                let file = ArtifactStore::open_archive_reader(&conversation.root, &id)
                    .map_err(|_| SessionArchivePrepareError::ArtifactUnavailable)?;
                if file.len != bytes {
                    return Err(SessionArchivePrepareError::ArtifactUnavailable);
                }
                self.artifacts.push(ArtifactCut {
                    manifest: ArtifactManifest {
                        entry: format!(
                            "artifacts/{}/{}/content",
                            conversation.manifest.conversation_id, id
                        ),
                        conversation_id: conversation.manifest.conversation_id.clone(),
                        artifact_id: id,
                        metadata,
                        bytes,
                    },
                    file,
                });
            }
        }
        Ok(())
    }

    /// Start ZIP generation behind a fixed-capacity byte channel. Dropping the
    /// stream cancels production, including a producer waiting for capacity.
    #[must_use]
    pub fn stream(self) -> SessionArchiveStream {
        let (sender, receiver) = mpsc::channel(STREAM_CAPACITY);
        let cancel = CancellationToken::new();
        let producer_cancel = cancel.clone();
        let producer = tokio::task::spawn_blocking(move || {
            let writer = ChannelWriter {
                sender: sender.clone(),
                cancel: producer_cancel.clone(),
            };
            let result = self
                .write_zip(writer, &producer_cancel)
                .map(|()| ArchiveChunk::Complete);
            let _ = sender.blocking_send(result);
        });
        SessionArchiveStream {
            receiver,
            cancel,
            complete: false,
            producer: Some(producer),
        }
    }

    #[cfg(test)]
    pub(crate) fn write_test(self, writer: impl Write) -> io::Result<()> {
        self.write_zip(writer, &CancellationToken::new())
    }

    fn write_zip(mut self, writer: impl Write, cancel: &CancellationToken) -> io::Result<()> {
        let mut zip = zip::ZipWriter::new_stream(writer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);
        zip.start_file("manifest.json", options).map_err(error)?;
        serde_json::to_writer(&mut zip, &json!({
            "format": FORMAT, "rustx_version": env!("CARGO_PKG_VERSION"),
            "durable_schema": crate::durable::sqlite::SQLITE_SCHEMA_VERSION,
            "session": self.session, "cwd": self.cwd, "nodes": self.nodes,
            "conversations": self.conversations.iter().map(|c| &c.manifest).collect::<Vec<_>>(),
            "artifacts": self.artifacts.iter().map(|a| &a.manifest).collect::<Vec<_>>(),
            "schemas": {"journal":1,"messages":1,"surface":1,"requests":1,"generations":1,"publication_audits":1,"inherited_responses":1},
            "integrity": "ZIP CRC32 per entry",
            "excluded": ["provider-private continuation state", "infrastructure configuration and credentials", "opaque request parameters outside the inspection allowlist", "provider and runtime diagnostic prose/codes", "workflow recovery comparison guards"],
            "unavailable": ["historical workspace-upload bytes are not immutable durable artifacts; recorded references remain in history"]
        })).map_err(error)?;
        for conversation in &self.conversations {
            for (authority, name, through) in conversation.authorities() {
                check_cancel(cancel)?;
                zip.start_file(
                    format!(
                        "sessions/{}/{name}.jsonl",
                        conversation.manifest.conversation_id
                    ),
                    options,
                )
                .map_err(error)?;
                conversation.each(authority, through, cancel, |record| {
                    serde_json::to_writer(&mut zip, &record.value()).map_err(error)?;
                    zip.write_all(b"\n")
                })?;
            }
            zip.start_file(
                format!(
                    "sessions/{}/generations.jsonl",
                    conversation.manifest.conversation_id
                ),
                options,
            )
            .map_err(error)?;
            conversation.each(Authority::Journal, conversation.manifest.frontiers.journal, cancel, |record| {
                if let Record::Journal(envelope) = record {
                    match envelope.event {
                        RuntimeEvent::ModelRequestCompleted { request_id, generation, .. } | RuntimeEvent::ModelRequestFailed { request_id, generation, .. } => {
                            serde_json::to_writer(&mut zip, &json!({"request_id":request_id,"journal_sequence":envelope.sequence,"evidence":generation})).map_err(error)?;
                            zip.write_all(b"\n")?;
                        }
                        _ => {}
                    }
                }
                Ok(())
            })?;
        }
        let mut buffer = vec![0; CHUNK_BYTES];
        for artifact in &mut self.artifacts {
            check_cancel(cancel)?;
            zip.start_file(&artifact.manifest.entry, options)
                .map_err(error)?;
            let mut remaining = artifact.manifest.bytes;
            while remaining > 0 {
                check_cancel(cancel)?;
                let size = usize::try_from(remaining.min(CHUNK_BYTES as u64)).map_err(error)?;
                artifact.file.read_exact(&mut buffer[..size])?;
                zip.write_all(&buffer[..size])?;
                remaining -= size as u64;
            }
        }
        check_cancel(cancel)?;
        zip.finish().map_err(error)?;
        Ok(())
    }
}

impl ConversationCut {
    fn authorities(&self) -> [(Authority, &'static str, i64); 6] {
        let f = &self.manifest.frontiers;
        [
            (Authority::Journal, "journal", f.journal),
            (Authority::Messages, "messages", f.messages),
            (Authority::Surface, "surface", f.surface),
            (Authority::Requests, "requests", f.requests),
            (
                Authority::InheritedResponses,
                "inherited_responses",
                f.inherited_responses,
            ),
            (
                Authority::PublicationAudits,
                "publication_audits",
                f.publication_audits,
            ),
        ]
    }
    fn records(
        &self,
        cancel: &CancellationToken,
        mut visit: impl FnMut(Authority, &Record) -> io::Result<()>,
    ) -> io::Result<()> {
        for (authority, _, through) in self.authorities() {
            self.each(authority, through, cancel, |r| visit(authority, &r))?;
        }
        Ok(())
    }
    fn each(
        &self,
        authority: Authority,
        through: i64,
        cancel: &CancellationToken,
        mut visit: impl FnMut(Record) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut after = -1;
        loop {
            check_cancel(cancel)?;
            let Some((position, body)) = self.store.archive_record(authority, after, through)?
            else {
                break;
            };
            after = position;
            visit(Record::decode(authority, &body)?)?;
        }
        Ok(())
    }
}

enum Record {
    Journal(Box<RuntimeEventEnvelope>),
    Messages(MessageBlock),
    Surface(crate::conversation::SurfaceOp),
    Requests(Box<crate::model::snapshot::RequestSnapshot>),
    Audits(crate::publication::PublicationAudit),
    InheritedResponse(crate::durable::response::CompletedResponseProvenance),
}
impl Record {
    fn decode(authority: Authority, body: &str) -> io::Result<Self> {
        Ok(match authority {
            Authority::Journal => Self::Journal(serde_json::from_str(body).map_err(error)?),
            Authority::Messages => Self::Messages(serde_json::from_str(body).map_err(error)?),
            Authority::Surface => Self::Surface(serde_json::from_str(body).map_err(error)?),
            Authority::Requests => {
                Self::Requests(Box::new(serde_json::from_str(body).map_err(error)?))
            }
            Authority::InheritedResponses => {
                Self::InheritedResponse(serde_json::from_str(body).map_err(error)?)
            }
            Authority::PublicationAudits => {
                Self::Audits(serde_json::from_str(body).map_err(error)?)
            }
        })
    }
    fn value(&self) -> Value {
        match self {
            Self::Journal(v) => projection::journal(v),
            Self::Messages(v) => projection::message(v),
            Self::Surface(v) => json!(v),
            Self::Requests(v) => projection::request(v),
            Self::Audits(v) => json!(v),
            Self::InheritedResponse(v) => json!(v),
        }
    }
}
fn collect_record_artifacts(record: &Record, refs: &mut BTreeMap<ArtifactId, Value>) {
    match record {
        Record::Messages(MessageBlock::User(user)) => {
            for block in &user.content {
                match block {
                    UserContentBlock::File(r) => insert_ref(refs, &r.artifact_id, json!(r)),
                    UserContentBlock::Image(r) => insert_ref(refs, &r.artifact_id, json!(r)),
                    _ => {}
                }
            }
        }
        Record::Messages(MessageBlock::Assistant(a)) => {
            for block in &a.content {
                if let AssistantContentBlock::Image(r) = block {
                    insert_ref(refs, &r.artifact_id, json!(r));
                }
            }
        }
        Record::Messages(MessageBlock::Tool(t)) => collect_tool(&t.result, refs),
        Record::Journal(envelope) => {
            if let RuntimeEvent::ToolExecutionCompleted { result, .. } = &envelope.event {
                collect_tool(result, refs);
            }
        }
        _ => {}
    }
}
fn collect_tool(tool: &ToolExecutionResult, refs: &mut BTreeMap<ArtifactId, Value>) {
    for r in &tool.artifacts {
        insert_ref(refs, &r.artifact_id, json!(r));
    }
    for block in &tool.content {
        match block {
            ToolResultContent::File(r) => insert_ref(refs, &r.artifact_id, json!(r)),
            ToolResultContent::Image(r) => insert_ref(refs, &r.artifact_id, json!(r)),
            _ => {}
        }
    }
}
fn insert_ref(refs: &mut BTreeMap<ArtifactId, Value>, id: &ArtifactId, metadata: Value) {
    // One recorded display descriptor per identity; all occurrence metadata
    // remains losslessly available in the native logical records.
    refs.entry(id.clone()).or_insert(metadata);
}
fn check_cancel(cancel: &CancellationToken) -> io::Result<()> {
    if cancel.is_cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "archive cancelled",
        ))
    } else {
        Ok(())
    }
}
struct ChannelWriter {
    sender: mpsc::Sender<io::Result<ArchiveChunk>>,
    cancel: CancellationToken,
}
impl Write for ChannelWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        check_cancel(&self.cancel)?;
        let size = bytes.len().min(CHUNK_BYTES);
        if size == 0 {
            return Ok(0);
        }
        self.sender
            .blocking_send(Ok(ArchiveChunk::Data(bytes[..size].to_vec())))
            .map_err(|_| error("archive consumer closed"))?;
        Ok(size)
    }
    fn flush(&mut self) -> io::Result<()> {
        check_cancel(&self.cancel)
    }
}
/// Bounded native archive bytes. EOF means successful ZIP completion; failures
/// are delivered as errors, and dropping the receiver wakes blocked writers.
pub struct SessionArchiveStream {
    receiver: mpsc::Receiver<io::Result<ArchiveChunk>>,
    cancel: CancellationToken,
    complete: bool,
    producer: Option<tokio::task::JoinHandle<()>>,
}
impl SessionArchiveStream {
    /// Cancel and wait for the active native reader/container worker to exit.
    pub async fn cancel(mut self) {
        self.cancel.cancel();
        self.receiver.close();
        if let Some(producer) = self.producer.take() {
            let _ = producer.await;
        }
    }
    pub async fn recv(&mut self) -> Option<io::Result<Vec<u8>>> {
        if self.complete {
            return None;
        }
        match self.receiver.recv().await {
            Some(Ok(ArchiveChunk::Data(bytes))) => Some(Ok(bytes)),
            Some(Ok(ArchiveChunk::Complete)) => {
                self.complete = true;
                None
            }
            Some(Err(e)) => {
                self.complete = true;
                Some(Err(e))
            }
            None => {
                self.complete = true;
                Some(Err(error("archive producer ended without completion")))
            }
        }
    }
}
impl Drop for SessionArchiveStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

enum ArchiveChunk {
    Data(Vec<u8>),
    Complete,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn archive_backpressure_bounds_producer_and_drop_wakes_writer() {
        let (sender, mut receiver) = mpsc::channel(STREAM_CAPACITY);
        let (filled, wait_filled) = tokio::sync::oneshot::channel();
        let (attempting, wait_attempting) = tokio::sync::oneshot::channel();
        let task = tokio::task::spawn_blocking(move || {
            let mut writer = ChannelWriter {
                sender,
                cancel: CancellationToken::new(),
            };
            for _ in 0..STREAM_CAPACITY {
                writer.write_all(&vec![1; CHUNK_BYTES]).unwrap();
            }
            filled.send(()).unwrap();
            attempting.send(()).unwrap();
            // Third write cannot finish until the consumer frees capacity.
            writer.write_all(&vec![2; CHUNK_BYTES])
        });
        wait_filled.await.unwrap();
        wait_attempting.await.unwrap();
        assert_eq!(receiver.len(), STREAM_CAPACITY);
        assert!(!task.is_finished());
        let chunk = receiver.recv().await.unwrap().unwrap();
        assert!(matches!(chunk,ArchiveChunk::Data(bytes) if bytes.len()==CHUNK_BYTES));
        task.await.unwrap().unwrap();
        assert_eq!(receiver.len(), STREAM_CAPACITY);
        let (sender, receiver) = mpsc::channel(1);
        let (filled, wait_filled) = tokio::sync::oneshot::channel();
        let task = tokio::task::spawn_blocking(move || {
            let mut writer = ChannelWriter {
                sender,
                cancel: CancellationToken::new(),
            };
            writer.write_all(&vec![1; CHUNK_BYTES]).unwrap();
            filled.send(()).unwrap();
            writer.write_all(&vec![2; CHUNK_BYTES])
        });
        wait_filled.await.unwrap();
        drop(receiver);
        assert!(task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn archive_abrupt_producer_exit_is_not_success() {
        let (sender, receiver) = mpsc::channel(1);
        drop(sender);
        let mut stream = SessionArchiveStream {
            receiver,
            cancel: CancellationToken::new(),
            complete: false,
            producer: None,
        };
        assert!(stream.recv().await.unwrap().is_err());
    }

    #[test]
    fn archive_typed_safety_preserves_authored_text_and_removes_private_continuation() {
        use crate::message::types::{AssistantMessageBlock, ReasoningBlock};
        let message = MessageBlock::Assistant(AssistantMessageBlock {
            id: crate::runtime::identity::MessageId::new("safe"),
            content: vec![AssistantContentBlock::Reasoning(ReasoningBlock {
                text: Some("authored Authorization: Bearer sk-literal".into()),
                provider_state: Some(
                    crate::runtime::continuation::ProviderContinuationState::Anthropic(
                        crate::runtime::continuation::AnthropicContinuation {
                            opaque: json!({"private":"PROVIDER_PRIVATE_SECRET"}),
                        },
                    ),
                ),
            })],
        });
        let record = Record::decode(
            Authority::Messages,
            &serde_json::to_string(&message).unwrap(),
        )
        .unwrap();
        let encoded = record.value().to_string();
        assert!(encoded.contains("authored Authorization: Bearer sk-literal"));
        assert!(!encoded.contains("PROVIDER_PRIVATE_SECRET"));
    }
}
