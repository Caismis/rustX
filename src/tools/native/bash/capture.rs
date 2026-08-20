//! The Bash-local output capture half of one invocation.
//!
//! The supervised process-ownership half of a Bash invocation lives in the
//! shared runner (`crate::runtime::process_runner`); this module owns the
//! capture half: the bounded head/tail previews, the lazy spill of the
//! runtime-observed combined multiplex into the conversation's managed
//! tool-output store, the drain of the reader tasks, and the bounded
//! capture settlement failure.
//!
//! # Text overflow is not an artifact
//!
//! Bash output is *text*. The bounded preview is the canonical replayable
//! record; only when the combined output crosses its preview bound does the
//! capture allocate one managed spill file, write the retained complete
//! prefix, and stream every subsequent byte into it. The spill file is
//! auxiliary runtime-owned storage addressed by its absolute path inside
//! ordinary textual tool output — never a [`FileReference`], never a
//! semantic artifact, and never a model `File` modality. Small output
//! creates no file at all.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncReadExt;

use crate::tools::managed_output::{ManagedToolOutput, ToolOutputSpill};

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

    /// The reader-side handle handed to the stdout capture task.
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

/// The reader-side capture-hold handle: parks the stdout capture task after
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

/// The bounded streaming preview capture of one output stream.
///
/// The capture retains a deterministic head/tail preview without holding
/// unbounded output in memory: at most `limit * 3 / 2` bytes of preview
/// state plus one in-flight chunk are ever retained.
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
        // The head keeps up to `limit` bytes: while the output stays within
        // the bound the head *is* the complete output, and `finish` must
        // return it verbatim. Capping at half here would silently truncate
        // every complete output larger than half without a marker.
        if self.head.len() < self.limit {
            let take = (self.limit - self.head.len()).min(bytes.len());
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

/// The combined-multiplex capture: a bounded preview plus the lazy complete
/// spill into the managed tool-output store.
///
/// Before the combined output crosses `limit`, the *complete* output is
/// retained in memory (bounded by `limit` plus one in-flight chunk) and no
/// file exists. The first push that crosses the bound allocates one managed
/// spill file, writes the retained prefix verbatim, and streams every later
/// chunk directly into it; the retained prefix is then dropped, so memory
/// use returns to the bounded preview state. The complete file therefore
/// always contains the full output from byte zero, with no lost prefix and
/// no duplicated chunk.
pub(super) struct SpillCapture {
    preview: PreviewCapture,
    /// The complete retained prefix; `Some` until the spill starts.
    complete: Option<Vec<u8>>,
    /// The open spill file once the bound has been crossed.
    spill: Option<ToolOutputSpill>,
}

/// The settled state of one combined capture.
pub(super) struct CapturedOutput {
    /// The deterministic bounded UTF-8 preview.
    pub preview: String,
    /// Whether the preview is truncated (equivalently: a spill exists).
    pub truncated: bool,
    /// The complete output size in bytes.
    pub total_bytes: u64,
    /// The absolute managed spill locator, when the output crossed the
    /// bound.
    pub spill_path: Option<PathBuf>,
}

impl SpillCapture {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            preview: PreviewCapture::new(limit),
            complete: Some(Vec::new()),
            spill: None,
        }
    }

    /// Pushes one observed chunk: bounds the preview, retains the complete
    /// prefix until the bound is crossed, and spills from the crossing on.
    ///
    /// # Errors
    ///
    /// Returns an explicit failure when the spill file cannot be allocated
    /// or written: the capture never reports successful retention while
    /// silently losing full output.
    pub(super) fn push(&mut self, bytes: &[u8], store: &ManagedToolOutput) -> Result<(), String> {
        self.preview.push(bytes);
        if let Some(spill) = &mut self.spill {
            return spill
                .write_all(bytes)
                .map_err(|error| format!("cannot write the combined output spill: {error}"));
        }
        let complete = self.complete.as_mut().expect("prefix retained pre-spill");
        complete.extend_from_slice(bytes);
        if self.preview.total > self.preview.limit as u64 {
            let mut spill = store
                .open_spill()
                .map_err(|error| format!("cannot allocate the combined output spill: {error}"))?;
            spill
                .write_all(complete)
                .map_err(|error| format!("cannot write the combined output spill: {error}"))?;
            self.spill = Some(spill);
            self.complete = None;
        }
        Ok(())
    }

    /// The settled capture: bounded preview, truncation state, complete
    /// byte count, and the absolute spill locator when one exists.
    pub(super) fn finish(self) -> CapturedOutput {
        let Self { preview, spill, .. } = self;
        let total_bytes = preview.total;
        let (preview, truncated) = preview.finish();
        CapturedOutput {
            preview,
            truncated,
            total_bytes,
            spill_path: spill.map(|spill| spill.path().to_path_buf()),
        }
    }
}

/// One output reader task handle.
pub(super) type StreamHandle = tokio::task::JoinHandle<Result<(), String>>;

/// Streams one child pipe into its preview capture and the combined
/// multiplex.
///
/// Any capture failure (a pipe read or a lost multiplex) is returned
/// explicitly; it is never silently discarded.
pub(super) async fn capture_stream<R>(
    mut pipe: R,
    capture: Arc<Mutex<PreviewCapture>>,
    combined_tx: tokio::sync::mpsc::Sender<(u8, Vec<u8>)>,
    stream_id: u8,
    name: &'static str,
    park: CapturePark,
) -> Result<(), String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    #[cfg(not(test))]
    let _ = park;
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
            .send((stream_id, chunk))
            .await
            .map_err(|_| format!("the combined {name} capture is unavailable"))?;
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
    Ok(())
}

/// Consumes the runtime-observed combined stdout/stderr multiplex: bounds
/// its preview and lazily spills the complete combined output into the
/// managed tool-output store once the preview bound is crossed.
pub(super) async fn consume_combined(
    mut rx: tokio::sync::mpsc::Receiver<(u8, Vec<u8>)>,
    store: ManagedToolOutput,
    capture: Arc<Mutex<SpillCapture>>,
) -> Result<(), String> {
    while let Some((_stream_id, chunk)) = rx.recv().await {
        capture
            .lock()
            .expect("combined capture lock")
            .push(&chunk, &store)?;
    }
    Ok(())
}

/// Awaits every output reader task; the handles stay usable after a dropped
/// drain future, so a terminated tree can be re-drained exactly once more.
pub(super) async fn await_drain(
    stdout_task: &mut Option<StreamHandle>,
    stderr_task: &mut Option<StreamHandle>,
    combined_task: &mut StreamHandle,
) -> Result<(), String> {
    await_handle(stdout_task).await?;
    await_handle(stderr_task).await?;
    combined_task
        .await
        .map_err(|join| format!("the combined output reader task failed: {join}"))?
}

async fn await_handle(handle: &mut Option<StreamHandle>) -> Result<(), String> {
    match handle {
        Some(handle) => handle
            .await
            .map_err(|join| format!("the output reader task failed: {join}"))?,
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::identity::ConversationId;
    use crate::tools::managed_output::ManagedToolOutput;

    use super::SpillCapture;

    /// The exact lazy-spill transition (Issue #86 acceptance 18.2): output
    /// at or below the preview bound stays fully in memory with no spill
    /// file at all; one byte past the bound allocates exactly one spill
    /// that carries the complete content from byte zero, and bounded
    /// streaming continues after the transition without duplication or a
    /// lost prefix.
    #[test]
    fn the_lazy_spill_transition_is_exact() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("tool-output");
        let store =
            || ManagedToolOutput::new(ConversationId::new("conv"), &root).expect("managed store");

        // Below the bound: complete preview, no spill.
        let mut capture = SpillCapture::new(16);
        capture.push(b"first\n", &store()).expect("push");
        capture.push(b"second\n", &store()).expect("push");
        let settled = capture.finish();
        assert_eq!(settled.preview, "first\nsecond\n");
        assert!(!settled.truncated);
        assert_eq!(settled.total_bytes, 13);
        assert!(settled.spill_path.is_none());
        let no_files = |root: &std::path::Path| {
            std::fs::read_dir(root).is_ok_and(|mut entries| entries.next().is_none())
        };
        assert!(no_files(&root), "no spill file was allocated");

        // Exactly at the bound: still no spill.
        let mut capture = SpillCapture::new(64);
        let exact: Vec<u8> = (0u8..64).collect();
        capture.push(&exact, &store()).expect("push");
        let settled = capture.finish();
        assert_eq!(settled.preview.as_bytes(), exact.as_slice());
        assert!(!settled.truncated);
        assert!(settled.spill_path.is_none());
        assert!(no_files(&root));

        // One byte past the bound, delivered across several chunks: exactly
        // one spill file with the complete content from byte zero — the
        // retained prefix, the crossing chunk, and every later chunk appear
        // exactly once and in order. The limit comfortably exceeds the
        // truncation marker, so the bounded preview still shows real head
        // and tail content.
        let mut capture = SpillCapture::new(64);
        capture.push(&[b'a'; 60], &store()).expect("push");
        assert!(no_files(&root), "still below the bound");
        capture.push(b"cross", &store()).expect("crossing push");
        assert!(
            root.join("output_1.log").exists(),
            "the crossing allocated the spill"
        );
        capture.push(b"-after", &store()).expect("push after spill");
        let settled = capture.finish();
        assert!(settled.truncated);
        assert_eq!(settled.total_bytes, 71);
        let spill = settled.spill_path.expect("the spill locator");
        assert_eq!(spill, root.join("output_1.log"));
        let mut expected = vec![b'a'; 60];
        expected.extend_from_slice(b"cross-after");
        assert_eq!(
            std::fs::read(&spill).expect("spill bytes"),
            expected,
            "the spill holds the complete bytes from byte zero"
        );
        assert_eq!(
            std::fs::read_dir(&root).expect("spill root").count(),
            1,
            "exactly one spill file was allocated"
        );
        // The bounded preview is the deterministic head + tail of the same
        // complete text.
        assert!(settled.preview.starts_with("aaaa"));
        assert!(settled.preview.ends_with("cross-after"));
        assert!(settled.preview.len() <= 64);
    }
}
