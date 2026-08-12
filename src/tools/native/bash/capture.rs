//! The Bash-local output capture half of one invocation.
//!
//! The supervised process-ownership half of a Bash invocation lives in the
//! shared runner (`crate::runtime::process_runner`); this module owns the
//! capture half: the bounded head/tail previews, the artifact spooling of
//! stdout, stderr, and the runtime-observed combined multiplex, the drain
//! of the reader tasks, and the bounded capture settlement failure.

use std::io::Write;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncReadExt;

use crate::message::content::FileReference;

/// The test-only seam that holds one output reader task open: the stdout
/// reader parks after EOF until the invocation's bounded settlement path
/// force-finalizes it. This is how the regressions prove that a wedged
/// capture can never turn the bounded confirmation contract into an
/// unbounded wait.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct CaptureHold {
    parked_tx: tokio::sync::watch::Sender<bool>,
    parked_rx: tokio::sync::watch::Receiver<bool>,
}

#[cfg(test)]
impl CaptureHold {
    pub(super) fn new() -> Self {
        let (parked_tx, parked_rx) = tokio::sync::watch::channel(false);
        Self {
            parked_tx,
            parked_rx,
        }
    }

    /// The reader-side handle handed to the stdout spool task.
    pub(super) fn reader(&self) -> CaptureHoldReader {
        CaptureHoldReader {
            parked: self.parked_tx.clone(),
        }
    }

    /// Test side: waits until the stdout reader provably parked after EOF.
    pub(super) async fn await_parked(&self) {
        let mut rx = self.parked_rx.clone();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }
}

/// The reader-side capture-hold handle: parks the stdout spool task after
/// EOF until the bounded settlement path aborts it.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct CaptureHoldReader {
    parked: tokio::sync::watch::Sender<bool>,
}

/// The capture-park handle passed to the output readers: `Some` only in
/// test builds. In non-test builds the seam type is uninhabited, so no
/// reader can ever park.
#[cfg(test)]
pub(super) type CapturePark = Option<CaptureHoldReader>;
/// See [`CapturePark`]: the non-test seam is uninhabited.
#[cfg(not(test))]
pub(super) type CapturePark = Option<std::convert::Infallible>;

/// A process-control failure of the owned Bash invocation.
///
/// The ownership-half failure kinds are owned by the shared supervised
/// command runner (`crate::runtime::process_runner::ProcessControlError`);
/// the Bash-local half is the bounded capture settlement failure.
///
/// Supervisor setup, signaling, waiting, and IPC failures never silently
/// fail: a failure that undermines ownership or settlement surfaces as an
/// explicit failed tool result, never as an ordinary `Success`,
/// `Cancelled`, or `TimedOut`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BashProcessControlError {
    /// The output capture did not settle within the bounded confirmation
    /// window after the owned process tree reached its terminal state.
    /// This is the bounded settlement escape hatch for a wedged capture:
    /// the reader tasks are force-finalized and the invocation settles as
    /// an explicit bounded failure — the confirmation contract is a real
    /// bound, never an unbounded wait.
    CaptureTimeout,
}

impl core::fmt::Display for BashProcessControlError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CaptureTimeout => write!(
                f,
                "the bash output capture did not settle within the bounded confirmation window"
            ),
        }
    }
}

/// The three captured stream references of one invocation.
pub(super) type StreamReferences = (
    Option<FileReference>,
    Option<FileReference>,
    Option<FileReference>,
);

/// The bounded streaming preview capture of one output stream.
///
/// The capture retains a deterministic head/tail preview without holding
/// unbounded output in memory: the full stream is spooled to the artifact
/// store by the reader, and the preview is bounded by
/// [`BASH_STREAM_PREVIEW_BYTES`].
#[derive(Clone)]
pub(super) struct PreviewCapture {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: u64,
    limit: usize,
}

impl PreviewCapture {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            head: Vec::new(),
            tail: Vec::new(),
            total: 0,
            limit,
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        let half = self.limit / 2;
        if self.head.len() < half {
            let take = (half - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
        }
        if bytes.len() >= half {
            self.tail.clear();
            self.tail.extend_from_slice(&bytes[bytes.len() - half..]);
        } else {
            self.tail.extend_from_slice(bytes);
            let overflow = self.tail.len().saturating_sub(half);
            if overflow > 0 {
                self.tail.drain(..overflow);
            }
        }
    }

    /// The deterministic bounded UTF-8 preview and its truncation state.
    pub(super) fn finish(self) -> (String, bool) {
        if self.total <= self.limit as u64 {
            return (String::from_utf8_lossy(&self.head).into_owned(), false);
        }
        let marker = format!(
            "\n...[truncated {} bytes]...\n",
            self.total - self.limit as u64
        );
        let content = self.limit.saturating_sub(marker.len());
        let head = &self.head[..self.head.len().min(content / 2)];
        let tail_keep = content - head.len();
        let mut out = Vec::with_capacity(self.limit);
        out.extend_from_slice(head);
        out.extend_from_slice(marker.as_bytes());
        out.extend_from_slice(&self.tail[self.tail.len().saturating_sub(tail_keep)..]);
        (String::from_utf8_lossy(&out).into_owned(), true)
    }
}

/// One output reader task handle.
pub(super) type StreamHandle = tokio::task::JoinHandle<Result<Option<FileReference>, String>>;

/// Streams one child pipe into its preview capture and the combined
/// multiplex, spooling the full raw bytes into the artifact store.
///
/// Any capture failure (pipe read, artifact allocation, artifact open, or
/// write) is returned explicitly; it is never silently discarded.
pub(super) async fn spool_stream<R>(
    mut pipe: R,
    store: crate::tools::artifacts::ArtifactStore,
    capture: Arc<Mutex<PreviewCapture>>,
    combined_tx: tokio::sync::mpsc::Sender<(u8, Vec<u8>)>,
    stream_id: u8,
    name: &'static str,
    park: CapturePark,
) -> Result<Option<FileReference>, String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    #[cfg(not(test))]
    let _ = park;
    let mut artifact: Option<(
        crate::runtime::identity::ArtifactId,
        crate::tools::artifacts::ArtifactWriter,
    )> = None;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match pipe.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                return Err(format!("cannot read the {name} stream: {error}"));
            }
        };
        let chunk = buffer[..read].to_vec();
        capture.lock().expect("preview lock").push(&chunk);
        combined_tx
            .send((stream_id, chunk.clone()))
            .await
            .map_err(|_| format!("the combined {name} capture is unavailable"))?;
        let writer = if let Some(handle) = &mut artifact {
            handle
        } else {
            let id = store
                .create_artifact()
                .map_err(|error| format!("cannot allocate the {name} artifact: {error}"))?;
            let writer = store
                .open_writer(&id)
                .map_err(|error| format!("cannot open the {name} artifact: {error}"))?;
            artifact = Some((id, writer));
            artifact.as_mut().expect("inserted above")
        };
        writer
            .1
            .write_all(&chunk)
            .map_err(|error| format!("cannot write the {name} artifact: {error}"))?;
    }
    drop(combined_tx);
    #[cfg(test)]
    if let Some(park) = park {
        // The deterministic stuck-capture seam: park provably after EOF and
        // stay open until the bounded settlement path force-finalizes
        // (aborts) this task.
        park.parked.send(true).ok();
        std::future::pending::<()>().await;
    }
    Ok(artifact.map(|(id, _)| FileReference {
        artifact_id: id,
        name: Some(format!("{name}.log")),
        mime_type: Some("application/octet-stream".to_owned()),
        description: Some(format!("full {name} output of the bash execution")),
    }))
}

/// Consumes the runtime-observed combined stdout/stderr multiplex and spools
/// it as one artifact while retaining a bounded preview.
pub(super) async fn consume_combined(
    mut rx: tokio::sync::mpsc::Receiver<(u8, Vec<u8>)>,
    store: crate::tools::artifacts::ArtifactStore,
    capture: Arc<Mutex<PreviewCapture>>,
) -> Result<Option<FileReference>, String> {
    let mut artifact: Option<(
        crate::runtime::identity::ArtifactId,
        crate::tools::artifacts::ArtifactWriter,
    )> = None;
    while let Some((_stream_id, chunk)) = rx.recv().await {
        capture.lock().expect("preview lock").push(&chunk);
        let writer = if let Some(handle) = &mut artifact {
            handle
        } else {
            let id = store
                .create_artifact()
                .map_err(|error| format!("cannot allocate the combined artifact: {error}"))?;
            let writer = store
                .open_writer(&id)
                .map_err(|error| format!("cannot open the combined artifact: {error}"))?;
            artifact = Some((id, writer));
            artifact.as_mut().expect("inserted above")
        };
        writer
            .1
            .write_all(&chunk)
            .map_err(|error| format!("cannot write the combined artifact: {error}"))?;
    }
    Ok(artifact.map(|(id, _)| FileReference {
        artifact_id: id,
        name: Some("combined.log".to_owned()),
        mime_type: Some("application/octet-stream".to_owned()),
        description: Some("combined stdout/stderr output of the bash execution".to_owned()),
    }))
}

/// Awaits every output reader task; the handles stay usable after a dropped
/// drain future, so a terminated tree can be re-drained exactly once more.
pub(super) async fn await_drain(
    stdout_task: &mut Option<StreamHandle>,
    stderr_task: &mut Option<StreamHandle>,
    combined_task: &mut StreamHandle,
) -> Result<StreamReferences, String> {
    let stdout = await_handle(stdout_task).await?;
    let stderr = await_handle(stderr_task).await?;
    let combined = combined_task
        .await
        .map_err(|join| format!("the combined output reader task failed: {join}"))??;
    Ok((stdout, stderr, combined))
}

async fn await_handle(handle: &mut Option<StreamHandle>) -> Result<Option<FileReference>, String> {
    match handle {
        Some(handle) => handle
            .await
            .map_err(|join| format!("the output reader task failed: {join}"))?,
        None => Ok(None),
    }
}
