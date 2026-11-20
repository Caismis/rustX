//! Physical capture for Agent Bash. Only the Client ever opens these files.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rustx_confined_fs::PackageDirectory;
use rustx_execution_protocol::process::{
    BASH_PREVIEW_BYTES, BashCaptureResult, BashCaptureState, MAX_BASH_CAPTURE_BYTES,
    MAX_BASH_CAPTURES, OperationId, ProcessStream,
};
use rustx_process_supervisor::output_text::{IncrementalUtf8Decoder, TextPreviewCapture};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) struct Captures {
    directory: Mutex<Option<Arc<Directory>>>,
    slots: Arc<Semaphore>,
}

struct Directory {
    root: PackageDirectory,
    temporary: tempfile::TempDir,
}

pub(crate) struct CaptureFile {
    file: tempfile::NamedTempFile,
    directory: Arc<Directory>,
    _slot: OwnedSemaphorePermit,
    /// Readers and appends observe whole decoded fragments, including the rollback
    /// of a failed write. This lock is independent of process cancellation.
    io: tokio::sync::Mutex<()>,
    readable: AtomicBool,
}

impl Captures {
    pub(crate) fn new() -> Self {
        Self {
            directory: Mutex::new(None),
            slots: Arc::new(Semaphore::new(MAX_BASH_CAPTURES)),
        }
    }

    pub(crate) fn create(&self, operation: OperationId) -> io::Result<Arc<CaptureFile>> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| io::Error::other("Bash capture file capacity exhausted"))?;
        let mut directory = self.directory.lock().expect("capture directory");
        let directory = if let Some(directory) = directory.as_ref() {
            directory.clone()
        } else {
            let temporary = tempfile::tempdir()?;
            let directory_value = Arc::new(Directory {
                root: PackageDirectory::open(&temporary.path().canonicalize()?)?,
                temporary,
            });
            *directory = Some(directory_value.clone());
            directory_value
        };
        let file = tempfile::Builder::new()
            .prefix(&format!("{:032x}", operation.as_u128()))
            .rand_bytes(0)
            .suffix(".output")
            .tempfile_in(directory.temporary.path())?;
        Ok(Arc::new(CaptureFile {
            file,
            directory,
            _slot: slot,
            io: tokio::sync::Mutex::new(()),
            readable: AtomicBool::new(true),
        }))
    }
}

impl CaptureFile {
    pub(crate) fn tail(&self) -> io::Result<(String, bool)> {
        use std::os::unix::fs::FileExt as _;
        self.read(|root, name| {
            let file = root.file(name)?;
            let end = file.metadata()?.len();
            let start = end.saturating_sub(BASH_PREVIEW_BYTES as u64);
            let mut bytes = vec![0; usize::try_from(end - start).expect("bounded Bash tail")];
            file.read_exact_at(&mut bytes, start)?;
            let first = bytes
                .iter()
                .position(|byte| byte & 0xc0 != 0x80)
                .unwrap_or(bytes.len());
            let text = std::str::from_utf8(&bytes[first..]).map_err(io::Error::other)?;
            Ok((text.to_owned(), start > 0))
        })
        .ok_or_else(|| io::Error::other("Bash capture is unavailable"))?
    }

    pub(crate) fn read<T>(&self, read: impl FnOnce(&PackageDirectory, &Path) -> T) -> Option<T> {
        let _guard = self.io.blocking_lock();
        if !self.readable.load(Ordering::Acquire) {
            return None;
        }
        let name = self.file.path().file_name().expect("capture filename");
        Some(read(&self.directory.root, Path::new(name)))
    }
}

pub(crate) struct Capture {
    stdout: Stream,
    stderr: Stream,
    combined: TextPreviewCapture,
    prefix: Option<String>,
    file: Option<Arc<CaptureFile>>,
    writer: Option<tokio::fs::File>,
    stored: u64,
    failed: bool,
}

struct Stream {
    decoder: IncrementalUtf8Decoder,
    preview: TextPreviewCapture,
}

impl Stream {
    fn new() -> Self {
        Self {
            decoder: IncrementalUtf8Decoder::default(),
            preview: TextPreviewCapture::new(BASH_PREVIEW_BYTES),
        }
    }
}

impl Capture {
    pub(crate) fn new(file: Option<Arc<CaptureFile>>) -> Self {
        Self {
            stdout: Stream::new(),
            stderr: Stream::new(),
            combined: TextPreviewCapture::new(BASH_PREVIEW_BYTES),
            prefix: file.is_none().then(String::new),
            file,
            writer: None,
            stored: 0,
            failed: false,
        }
    }

    pub(crate) async fn push(
        &mut self,
        stream: ProcessStream,
        bytes: Option<&[u8]>,
        store: &Captures,
        operation: OperationId,
    ) {
        let lane = match stream {
            ProcessStream::Stdout | ProcessStream::Pty => &mut self.stdout,
            ProcessStream::Stderr => &mut self.stderr,
        };
        let text = match bytes {
            Some(bytes) => lane.decoder.push(bytes),
            None => lane.decoder.finish(),
        };
        if text.is_empty() {
            return;
        }
        lane.preview.push(&text);
        self.combined.push(&text);
        if self.failed {
            return;
        }
        let mut previous = String::new();
        if let Some(prefix) = self.prefix.as_mut() {
            if prefix.len() + text.len() <= BASH_PREVIEW_BYTES {
                prefix.push_str(&text);
                return;
            }
            previous = self.prefix.take().expect("retained prefix");
            self.file = store.create(operation).ok();
        }
        if self.append(&previous, &text).await.is_err() {
            self.failed = true;
        }
    }

    async fn append(&mut self, prefix: &str, text: &str) -> io::Result<()> {
        let file = self
            .file
            .as_ref()
            .ok_or_else(|| io::Error::other("missing capture file"))?;
        let size = (prefix.len() + text.len()) as u64;
        if self.stored + size > MAX_BASH_CAPTURE_BYTES {
            return Err(io::Error::other("Bash capture byte limit reached"));
        }
        let _guard = file.io.lock().await;
        let writer = if let Some(writer) = self.writer.as_mut() {
            writer
        } else {
            self.writer
                .insert(tokio::fs::File::from_std(file.file.reopen()?))
        };
        let result = async {
            writer.write_all(prefix.as_bytes()).await?;
            writer.write_all(text.as_bytes()).await?;
            writer.flush().await
        }
        .await;
        if result.is_err() {
            file.readable
                .store(writer.set_len(self.stored).await.is_ok(), Ordering::Release);
        } else {
            self.stored += size;
        }
        result
    }

    pub(crate) fn finish(
        self,
        streams_complete: bool,
        started: bool,
        signal: Option<i32>,
    ) -> (BashCaptureResult, Option<Arc<CaptureFile>>, bool) {
        let complete = !self.failed && streams_complete;
        let total = self.combined.total();
        let (combined, combined_truncated) = self.combined.finish();
        let (stdout, stdout_truncated) = self.stdout.preview.finish();
        let (stderr, stderr_truncated) = self.stderr.preview.finish();
        let file = self
            .file
            .filter(|file| file.readable.load(Ordering::Acquire));
        let output = match (complete, file.is_some()) {
            (true, true) => BashCaptureState::Complete,
            (true, false) => BashCaptureState::None,
            (false, true) => BashCaptureState::Partial,
            (false, false) => BashCaptureState::Unavailable,
        };
        (
            BashCaptureResult {
                stdout,
                stderr,
                combined,
                truncated: combined_truncated || stdout_truncated || stderr_truncated || !complete,
                total: complete.then(|| total.into()),
                output,
                started,
                signal,
            },
            file,
            complete,
        )
    }
}
