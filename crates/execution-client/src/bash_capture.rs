//! Physical capture for Agent Bash. Only the Client ever opens these files.

use std::io;
use std::os::unix::fs::PermissionsExt as _;
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

#[cfg(test)]
type AllocationHold = (
    OperationId,
    std::sync::mpsc::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);

pub(crate) struct Captures {
    prefix: &'static str,
    directory: Mutex<Option<Arc<Directory>>>,
    slots: Arc<Semaphore>,
    #[cfg(test)]
    allocation_hold: Mutex<Option<AllocationHold>>,
    #[cfg(test)]
    append_failure: Mutex<Option<OperationId>>,
}

struct Directory {
    root: PackageDirectory,
    temporary: tempfile::TempDir,
}

pub(crate) struct CaptureFile {
    file: std::fs::File,
    name: std::ffi::OsString,
    directory: Arc<Directory>,
    _slot: OwnedSemaphorePermit,
    /// Readers and appends observe whole decoded fragments, including the rollback
    /// of a failed write. This lock is independent of process cancellation.
    removed: tokio::sync::Mutex<bool>,
    readable: AtomicBool,
    #[cfg(test)]
    pub(crate) fail_next_unlink: AtomicBool,
    #[cfg(test)]
    fail_append_and_rollback: AtomicBool,
}

impl Captures {
    pub(crate) fn new() -> Self {
        Self::with_capacity("rustx-bash-", MAX_BASH_CAPTURES)
    }

    pub(crate) fn with_capacity(prefix: &'static str, slots: usize) -> Self {
        Self {
            prefix,
            directory: Mutex::new(None),
            slots: Arc::new(Semaphore::new(slots)),
            #[cfg(test)]
            allocation_hold: Mutex::new(None),
            #[cfg(test)]
            append_failure: Mutex::new(None),
        }
    }

    pub(crate) fn close(&self) -> io::Result<()> {
        let mut owned = self.directory.lock().expect("capture directory");
        if let Some(directory) = owned.as_ref() {
            if Arc::strong_count(directory) != 1 {
                return Err(io::Error::other("Bash capture files still have an owner"));
            }
            directory.root.verify()?;
            std::fs::remove_dir(directory.temporary.path())?;
            *owned = None;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> Option<std::path::PathBuf> {
        self.directory
            .lock()
            .unwrap()
            .as_ref()
            .map(|directory| directory.temporary.path().to_path_buf())
    }

    #[cfg(test)]
    pub(crate) fn available(&self) -> usize {
        self.slots.available_permits()
    }

    #[cfg(test)]
    pub(crate) fn hold_allocation(
        &self,
        operation: OperationId,
        entered: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    ) {
        *self.allocation_hold.lock().unwrap() = Some((operation, entered, resume));
    }

    #[cfg(test)]
    pub(crate) fn fail_append_and_rollback_for(&self, operation: OperationId) {
        *self.append_failure.lock().unwrap() = Some(operation);
    }

    pub(crate) fn create(&self, operation: OperationId) -> io::Result<Arc<CaptureFile>> {
        let slot = self.slots.clone().try_acquire_owned().map_err(|_| {
            io::Error::new(io::ErrorKind::WouldBlock, "capture file capacity exhausted")
        })?;
        #[cfg(test)]
        {
            let hold = {
                let mut armed = self.allocation_hold.lock().unwrap();
                if armed.as_ref().is_some_and(|(key, ..)| *key == operation) {
                    armed.take()
                } else {
                    None
                }
            };
            if let Some((_, entered, resume)) = hold {
                entered.send(()).unwrap();
                resume.recv().unwrap();
            }
        }
        let retained = self.directory.lock().expect("capture directory").clone();
        let directory = if let Some(retained) = retained {
            retained
        } else {
            let mut temporary = tempfile::Builder::new()
                .prefix(self.prefix)
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()?;
            let root = PackageDirectory::open(&temporary.path().canonicalize()?)?;
            let mut directory = self.directory.lock().expect("capture directory");
            if let Some(retained) = directory.as_ref() {
                retained.clone()
            } else {
                temporary.disable_cleanup(true);
                directory
                    .insert(Arc::new(Directory { root, temporary }))
                    .clone()
            }
        };
        let file = tempfile::Builder::new()
            .prefix(&format!("{:032x}", operation.as_u128()))
            .rand_bytes(0)
            .suffix(".output")
            .tempfile_in(directory.temporary.path())?;
        let (file, mut path) = file.into_parts();
        let name = path.file_name().expect("capture filename").to_owned();
        // The operation retains the handle through a confirmed unlink.
        // Automatic cleanup could discard an unverified pathname.
        path.disable_cleanup(true);
        Ok(Arc::new(CaptureFile {
            file,
            name,
            directory,
            _slot: slot,
            removed: tokio::sync::Mutex::new(false),
            readable: AtomicBool::new(true),
            #[cfg(test)]
            fail_next_unlink: AtomicBool::new(false),
            #[cfg(test)]
            fail_append_and_rollback: AtomicBool::new({
                let mut armed = self.append_failure.lock().unwrap();
                if *armed == Some(operation) {
                    *armed = None;
                    true
                } else {
                    false
                }
            }),
        }))
    }
}

impl CaptureFile {
    pub(crate) fn append_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        use std::os::unix::fs::FileExt as _;
        let removed = self.removed.blocking_lock();
        if *removed || !self.readable.load(Ordering::Acquire) {
            return Err(io::ErrorKind::NotFound.into());
        }
        if self.file.metadata()?.len() != offset {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if let Err(error) = self.file.write_all_at(bytes, offset) {
            self.readable
                .store(self.file.set_len(offset).is_ok(), Ordering::Release);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn sync_for_publication(&self, length: u64) -> io::Result<()> {
        self.read_verified(|_, _, file| {
            if file.metadata()?.len() != length {
                return Err(io::ErrorKind::InvalidData.into());
            }
            self.file.sync_all()
        })?
    }

    pub(crate) fn read_verified<T>(
        &self,
        read: impl FnOnce(&PackageDirectory, &Path, std::fs::File) -> T,
    ) -> io::Result<T> {
        use std::os::unix::fs::MetadataExt as _;
        self.read(|root, name| {
            let named_file = root.file(name)?;
            let named = named_file.metadata()?;
            let owned = self.file.metadata()?;
            if (named.dev(), named.ino()) != (owned.dev(), owned.ino()) {
                return Err(io::ErrorKind::InvalidData.into());
            }
            Ok(read(root, name, named_file))
        })
        .unwrap_or_else(|| Err(io::ErrorKind::NotFound.into()))
    }

    pub(crate) fn proves_empty(&self) -> bool {
        use std::os::unix::fs::MetadataExt as _;

        self.read(|root, name| {
            let owned = self.file.metadata()?;
            let named = root.file(name)?.metadata()?;
            Ok::<_, io::Error>(
                named.len() == 0 && owned.dev() == named.dev() && owned.ino() == named.ino(),
            )
        })
        .is_some_and(|result| matches!(result, Ok(true)))
    }

    pub(crate) fn discard(&self) -> io::Result<()> {
        use rustix::fs::{AtFlags, fstat, statat, unlinkat};

        let mut removed = self.removed.blocking_lock();
        if *removed {
            return Ok(());
        }
        let retained = fstat(&self.file)?;
        if retained.st_nlink == 0 {
            *removed = true;
            return Ok(());
        }
        let name = self.name.as_os_str();
        let root = &self.directory.root;
        root.verify()?;
        match statat(root.descriptor(), name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(named) => {
                if (named.st_dev, named.st_ino) != (retained.st_dev, retained.st_ino) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                #[cfg(test)]
                if self.fail_next_unlink.swap(false, Ordering::AcqRel) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                match unlinkat(root.descriptor(), name, AtFlags::empty()) {
                    Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => return Err(error.into()),
        }
        if fstat(&self.file)?.st_nlink != 0 {
            return Err(io::Error::other("Bash capture still has directory links"));
        }
        *removed = true;
        Ok(())
    }

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
        let removed = self.removed.blocking_lock();
        if *removed || !self.readable.load(Ordering::Acquire) {
            return None;
        }
        Some(read(&self.directory.root, Path::new(&self.name)))
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
        allocate: impl FnOnce() -> tokio::task::JoinHandle<io::Result<Arc<CaptureFile>>>,
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
            self.file = allocate().await.ok().and_then(Result::ok);
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
        let removed = file.removed.lock().await;
        if *removed {
            return Err(io::ErrorKind::NotFound.into());
        }
        let writer = if let Some(writer) = self.writer.as_mut() {
            writer
        } else {
            self.writer
                .insert(tokio::fs::File::from_std(file.file.try_clone()?))
        };
        #[cfg(test)]
        let fail = file.fail_append_and_rollback.swap(false, Ordering::AcqRel);
        #[cfg(not(test))]
        let fail = false;
        let result = async {
            if fail {
                let first = prefix.as_bytes().first().unwrap_or(&text.as_bytes()[0]);
                writer.write_all(std::slice::from_ref(first)).await?;
                writer.flush().await?;
                return Err(io::Error::other("injected Bash capture append failure"));
            }
            writer.write_all(prefix.as_bytes()).await?;
            writer.write_all(text.as_bytes()).await?;
            writer.flush().await
        }
        .await;
        if result.is_err() {
            let rolled_back = !fail && writer.set_len(self.stored).await.is_ok();
            file.readable.store(rolled_back, Ordering::Release);
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
    ) -> (BashCaptureResult, bool) {
        let readable = self
            .file
            .as_ref()
            .is_some_and(|file| file.readable.load(Ordering::Acquire));
        let complete = !self.failed && streams_complete && (self.file.is_none() || readable);
        let total = self.combined.total();
        let (combined, combined_truncated) = self.combined.finish();
        let (stdout, stdout_truncated) = self.stdout.preview.finish();
        let (stderr, stderr_truncated) = self.stderr.preview.finish();
        let output = match (complete, readable) {
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
            complete,
        )
    }
}
