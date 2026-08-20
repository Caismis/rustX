//! The Bash-local output capture half of one invocation.
//!
//! The supervised process-ownership half of a Bash invocation lives in the
//! shared runner (`crate::runtime::process_runner`); this module owns the
//! capture half: the per-stream incremental UTF-8 decoding, the bounded
//! head/tail previews, the execution-mode-dependent output storage, the
//! drain of the reader tasks, and the bounded capture settlement failure.
//!
//! # Text decoding
//!
//! Every byte stream is decoded with its own incremental UTF-8 decoder
//! ([`super::text`]) before its text is multiplexed, previewed, or stored,
//! so every advertised output path contains valid UTF-8 text that Read and
//! Grep can actually inspect. Invalid sequences decode to U+FFFD; a
//! sequence split across read boundaries is completed by decoder state,
//! never corrupted by interleaving.
//!
//! # Two output-storage lifecycles (Issue #86)
//!
//! Text overflow is not an artifact, and the two execution modes have
//! deliberately different storage lifecycles over the same managed
//! tool-output store ([`crate::tools::managed_output`]):
//!
//! - **Foreground: context-overflow storage.** The combined text is
//!   retained completely in memory until it crosses its preview bound;
//!   only the crossing allocates one lazy result spill, writes the
//!   retained prefix, and streams every later fragment into it. Output at
//!   or below the bound creates no file at all.
//! - **Background: the live observation channel.** The live-output file
//!   was allocated at the dispatch commit point — before the accepted
//!   result advertised its absolute path — and every decoded text fragment
//!   is appended from the first byte on, so the model can Read/Grep the
//!   output while the execution runs. At settlement the same file is the
//!   complete textual output; no second file is created for the same
//!   payload.
//!
//! Neither file is ever a [`FileReference`], a semantic artifact, or a
//! model `File` modality: the absolute path appears inside ordinary
//! textual tool output.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncReadExt;

use super::text::IncrementalUtf8Decoder;
use crate::tools::managed_output::{BackgroundOutput, ManagedToolOutput, ResultSpill};

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

/// The test-only observation seam of background output appends: after every
/// committed append to the live-output file, the cumulative appended byte
/// count is published, so a test can synchronize on "output fragment X is
/// observable through the path" without polling or sleeps.
#[cfg(test)]
pub(super) type AppendWatch = Option<tokio::sync::watch::Sender<u64>>;
/// See [`AppendWatch`]: the non-test seam is uninhabited.
#[cfg(not(test))]
pub(super) type AppendWatch = Option<std::convert::Infallible>;

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
/// state plus one in-flight fragment are ever retained. The retained
/// content is decoded text, so every truncation boundary lands on a
/// character boundary.
#[derive(Clone, Debug)]
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

    pub(super) fn push(&mut self, text: &str) {
        let bytes = text.as_bytes();
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
            let text = String::from_utf8(self.head).expect("the retained preview is decoded text");
            return (text, false);
        }
        let marker = format!(
            "\n...[truncated {} bytes]...\n",
            self.total - self.limit as u64
        );
        let content = self.limit.saturating_sub(marker.len());
        let head = &self.head[..floor_char_boundary(&self.head, self.head.len().min(content / 2))];
        let tail_keep = content - head.len();
        let tail_start = ceil_char_boundary(&self.tail, self.tail.len().saturating_sub(tail_keep));
        let mut out = Vec::with_capacity(self.limit);
        out.extend_from_slice(head);
        out.extend_from_slice(marker.as_bytes());
        out.extend_from_slice(&self.tail[tail_start..]);
        (
            String::from_utf8(out).expect("the bounded preview is decoded text"),
            true,
        )
    }
}

/// The largest index `<= index` that lands on a UTF-8 character boundary
/// of `text`. `text` holds decoded text, so the walk terminates.
fn floor_char_boundary(text: &[u8], mut index: usize) -> usize {
    while index > 0 && std::str::from_utf8(&text[..index]).is_err() {
        index -= 1;
    }
    index
}

/// The smallest index `>= index` that lands on a UTF-8 character boundary
/// of `text`. `text` holds decoded text, so the walk terminates.
fn ceil_char_boundary(text: &[u8], mut index: usize) -> usize {
    while index < text.len() && std::str::from_utf8(&text[index..]).is_err() {
        index += 1;
    }
    index
}

/// The foreground combined-multiplex capture: a bounded preview plus the
/// lazy complete result spill into the managed tool-output store.
///
/// Before the combined output crosses `limit`, the *complete* output is
/// retained in memory (bounded by `limit` plus one in-flight fragment) and
/// no file exists. The first push that crosses the bound allocates one
/// result spill, writes the retained prefix verbatim, and streams every
/// later fragment directly into it; the retained prefix is then dropped,
/// so memory use returns to the bounded preview state. The complete file
/// therefore always contains the full output from the first character on,
/// with no lost prefix and no duplicated fragment.
#[derive(Debug)]
pub(super) struct SpillCapture {
    preview: PreviewCapture,
    /// The complete retained prefix; `Some` until the spill starts.
    complete: Option<String>,
    /// The open spill file once the bound has been crossed.
    spill: Option<ResultSpill>,
    /// The test-only spill-transition observation seam: signaled the
    /// moment the lazy spill is allocated (before the retained prefix is
    /// written), so a test can synchronize on the exact overflow boundary
    /// without polling the filesystem. Uninhabited in non-test builds.
    #[cfg(test)]
    spill_started: Option<tokio::sync::watch::Sender<bool>>,
}

/// The settled state of one combined capture.
pub(super) struct CapturedOutput {
    /// The deterministic bounded UTF-8 preview.
    pub preview: String,
    /// Whether the preview is truncated. For a foreground capture this is
    /// equivalent to "a result spill exists"; for a background capture the
    /// live-output file exists regardless of truncation.
    pub truncated: bool,
    /// The complete output size in bytes.
    pub total_bytes: u64,
    /// Whether the capture settled completely: every observed fragment was
    /// captured and the output file, when one exists, holds the complete
    /// output. An incomplete capture (a failed stream read, a failed output
    /// write, a force-finalized reader) never advertises a locator as the
    /// complete output.
    pub complete: bool,
    /// The absolute managed output locator.
    ///
    /// Foreground: present only when the capture is complete and the
    /// output crossed the bound. Background: present whenever the
    /// live-output file exists — it was advertised at dispatch — but it
    /// represents the *complete* output only when `complete` holds; an
    /// incomplete background capture leaves the partial file in place as
    /// honestly-labelled partial running output.
    pub output_locator: Option<PathBuf>,
}

impl SpillCapture {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            preview: PreviewCapture::new(limit),
            complete: Some(String::new()),
            spill: None,
            #[cfg(test)]
            spill_started: None,
        }
    }

    /// Installs the test-only spill-transition observation seam.
    #[cfg(test)]
    pub(super) fn with_spill_started_watch(
        mut self,
        watch: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        self.spill_started = Some(watch);
        self
    }

    /// Pushes one observed text fragment: bounds the preview, retains the
    /// complete prefix until the bound is crossed, and spills from the
    /// crossing on.
    ///
    /// # Errors
    ///
    /// Returns an explicit failure when the spill file cannot be allocated
    /// or written: the capture never reports successful retention while
    /// silently losing full output.
    pub(super) fn push(&mut self, text: &str, store: &ManagedToolOutput) -> Result<(), String> {
        self.preview.push(text);
        if let Some(spill) = &mut self.spill {
            return spill
                .write_all(text)
                .map_err(|error| format!("cannot write the combined output spill: {error}"));
        }
        let complete = self.complete.as_mut().expect("prefix retained pre-spill");
        complete.push_str(text);
        if self.preview.total > self.preview.limit as u64 {
            let spill = store
                .open_spill()
                .map_err(|error| format!("cannot allocate the combined output spill: {error}"))?;
            // The capture retains the spill handle before the prefix write:
            // a failed prefix write leaves a partial file, and the settled
            // capture must own it so it is never advertised as complete.
            self.spill = Some(spill);
            #[cfg(test)]
            if let Some(watch) = &self.spill_started {
                watch.send_replace(true);
            }
            let prefix = std::mem::take(complete);
            self.spill
                .as_mut()
                .expect("spill retained")
                .write_all(&prefix)
                .map_err(|error| format!("cannot write the combined output spill: {error}"))?;
            self.complete = None;
        }
        Ok(())
    }

    /// The settled foreground capture: bounded preview, truncation state,
    /// complete byte count, and the absolute spill locator when one exists.
    ///
    /// `complete` is the executor's settlement fact — whether every output
    /// fragment provably reached the capture (all readers and the combined
    /// multiplex drained without error). A locator is published only for a
    /// complete capture: an incomplete capture discards the partial spill
    /// (best-effort removal of the file) and never advertises it as the
    /// complete output.
    pub(super) fn finish(self, complete: bool) -> CapturedOutput {
        let Self { preview, spill, .. } = self;
        let total_bytes = preview.total;
        let (preview, truncated) = preview.finish();
        let output_locator = match (complete, spill) {
            (true, Some(spill)) => Some(spill.path().to_path_buf()),
            (false, Some(spill)) => {
                // A partial spill is auxiliary residue, never an advertised
                // complete result: drop the handle and remove the file
                // best-effort. The bounded preview remains the canonical
                // record either way.
                let path = spill.path().to_path_buf();
                drop(spill);
                let _ = std::fs::remove_file(&path);
                None
            }
            (_, None) => None,
        };
        CapturedOutput {
            preview,
            truncated,
            total_bytes,
            complete,
            output_locator,
        }
    }
}

/// The background combined-multiplex capture: a bounded preview plus the
/// continuous append of every decoded text fragment into the execution's
/// live-output file.
///
/// Unlike the foreground [`SpillCapture`], the file is not overflow
/// storage: it was allocated at the dispatch commit point and advertised
/// to the model immediately, so output streams into it from the first
/// fragment on, regardless of size. At settlement the same path is the
/// complete textual output — no second file is created for the same
/// payload.
#[derive(Debug)]
pub(super) struct BackgroundOutputCapture {
    preview: PreviewCapture,
    /// The append sink of the execution's live-output file.
    sink: BackgroundOutput,
    /// Cumulative bytes appended, for the test-only append observation
    /// seam.
    #[cfg(test)]
    appended: u64,
    /// The test-only append observation seam; uninhabited in non-test
    /// builds.
    #[cfg(test)]
    watch: AppendWatch,
}

impl BackgroundOutputCapture {
    pub(super) fn new(limit: usize, sink: BackgroundOutput, watch: AppendWatch) -> Self {
        #[cfg(not(test))]
        let _ = watch;
        Self {
            preview: PreviewCapture::new(limit),
            sink,
            #[cfg(test)]
            appended: 0,
            #[cfg(test)]
            watch,
        }
    }

    /// Pushes one observed text fragment: bounds the preview and appends
    /// the fragment to the live-output file. A successful append is the
    /// linearization point after which the fragment is observable through
    /// the advertised path while the execution runs.
    ///
    /// # Errors
    ///
    /// Returns an explicit failure when the append fails: an output-storage
    /// failure of an already-advertised path is represented at settlement,
    /// never silently lost.
    pub(super) fn push(&mut self, text: &str) -> Result<(), String> {
        self.preview.push(text);
        self.sink
            .append(text)
            .map_err(|error| format!("cannot append to the background output file: {error}"))?;
        #[cfg(test)]
        {
            self.appended += text.len() as u64;
            if let Some(watch) = &self.watch {
                watch.send_replace(self.appended);
            }
        }
        Ok(())
    }

    /// The settled background capture. The live-output path is always
    /// reported — it was advertised at dispatch — and `complete` states
    /// whether it holds the complete output or the honestly partial one.
    pub(super) fn finish(self, complete: bool) -> CapturedOutput {
        let path = self.sink.path().to_path_buf();
        let total_bytes = self.preview.total;
        let (preview, truncated) = self.preview.finish();
        CapturedOutput {
            preview,
            truncated,
            total_bytes,
            complete,
            output_locator: Some(path),
        }
    }
}

/// One output reader task handle.
pub(super) type StreamHandle = tokio::task::JoinHandle<Result<(), String>>;

/// Streams one child pipe through its incremental UTF-8 decoder into its
/// preview capture and the combined multiplex.
///
/// The decoder state is per stream and survives across reads, so a
/// multi-byte sequence split over read boundaries decodes exactly once and
/// interleaving with the other stream can never fabricate invalid UTF-8.
/// Any capture failure (a pipe read or a lost multiplex) is returned
/// explicitly; it is never silently discarded.
pub(super) async fn capture_stream<R>(
    mut pipe: R,
    capture: Arc<Mutex<PreviewCapture>>,
    combined_tx: tokio::sync::mpsc::Sender<(u8, String)>,
    stream_id: u8,
    name: &'static str,
    park: CapturePark,
) -> Result<(), String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    #[cfg(not(test))]
    let _ = park;
    let mut decoder = IncrementalUtf8Decoder::default();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match pipe.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                return Err(format!("cannot read the {name} stream: {error}"));
            }
        };
        let text = decoder.push(&buffer[..read]);
        if text.is_empty() {
            continue;
        }
        capture.lock().expect("preview lock").push(&text);
        combined_tx
            .send((stream_id, text))
            .await
            .map_err(|_| format!("the combined {name} capture is unavailable"))?;
    }
    // End of stream: flush the decoder deterministically (an incomplete
    // trailing sequence becomes U+FFFD) and capture the final fragment.
    let tail = decoder.finish();
    if !tail.is_empty() {
        capture.lock().expect("preview lock").push(&tail);
        combined_tx
            .send((stream_id, tail))
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

/// Consumes the runtime-observed combined stdout/stderr text multiplex of
/// one **foreground** invocation: bounds its preview and lazily spills the
/// complete combined output into the managed tool-output store once the
/// preview bound is crossed.
pub(super) async fn consume_combined(
    mut rx: tokio::sync::mpsc::Receiver<(u8, String)>,
    store: ManagedToolOutput,
    capture: Arc<Mutex<SpillCapture>>,
) -> Result<(), String> {
    while let Some((_stream_id, text)) = rx.recv().await {
        capture
            .lock()
            .expect("combined capture lock")
            .push(&text, &store)?;
    }
    Ok(())
}

/// Consumes the runtime-observed combined stdout/stderr text multiplex of
/// one **background** invocation: bounds its preview and appends every
/// fragment to the execution's live-output file, so the advertised path is
/// readable while the execution runs.
pub(super) async fn consume_background(
    mut rx: tokio::sync::mpsc::Receiver<(u8, String)>,
    capture: Arc<Mutex<BackgroundOutputCapture>>,
) -> Result<(), String> {
    while let Some((_stream_id, text)) = rx.recv().await {
        capture
            .lock()
            .expect("background capture lock")
            .push(&text)?;
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
    /// file at all; one fragment past the bound allocates exactly one
    /// spill that carries the complete content from the first character
    /// on, and bounded streaming continues after the transition without
    /// duplication or a lost prefix.
    #[test]
    fn the_lazy_spill_transition_is_exact() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("tool-output");
        let store =
            || ManagedToolOutput::new(ConversationId::new("conv"), &root).expect("managed store");
        let results = || root.join("results");

        // Below the bound: complete preview, no spill.
        let mut capture = SpillCapture::new(16);
        capture.push("first\n", &store()).expect("push");
        capture.push("second\n", &store()).expect("push");
        let settled = capture.finish(true);
        assert_eq!(settled.preview, "first\nsecond\n");
        assert!(!settled.truncated);
        assert_eq!(settled.total_bytes, 13);
        assert!(settled.output_locator.is_none());
        let no_files = |root: &std::path::Path| {
            std::fs::read_dir(root).is_ok_and(|mut entries| entries.next().is_none())
        };
        assert!(no_files(&results()), "no spill file was allocated");

        // Exactly at the bound: still no spill.
        let mut capture = SpillCapture::new(64);
        let exact: String = "x".repeat(64);
        capture.push(&exact, &store()).expect("push");
        let settled = capture.finish(true);
        assert_eq!(settled.preview, exact);
        assert!(!settled.truncated);
        assert!(settled.output_locator.is_none());
        assert!(no_files(&results()));

        // One character past the bound, delivered across several chunks:
        // exactly one spill file with the complete content from the first
        // character on — the retained prefix, the crossing chunk, and every
        // later chunk appear exactly once and in order. The limit
        // comfortably exceeds the truncation marker, so the bounded preview
        // still shows real head and tail content.
        let mut capture = SpillCapture::new(64);
        capture.push(&"a".repeat(60), &store()).expect("push");
        assert!(no_files(&results()), "still below the bound");
        capture.push("cross", &store()).expect("crossing push");
        assert!(
            results().join("result_1.txt").exists(),
            "the crossing allocated the spill"
        );
        capture.push("-after", &store()).expect("push after spill");
        let settled = capture.finish(true);
        assert!(settled.truncated);
        assert_eq!(settled.total_bytes, 71);
        let spill = settled.output_locator.expect("the spill locator");
        // The store canonicalizes its root once at construction, so compare
        // against the canonical root (on macOS /var is a /private/var link).
        let canonical_root = root.canonicalize().expect("canonical root");
        assert_eq!(spill, canonical_root.join("results/result_1.txt"));
        assert!(
            spill.starts_with(store().root()),
            "the spill locator lives under the canonical managed root"
        );
        let mut expected = "a".repeat(60);
        expected.push_str("cross-after");
        assert_eq!(
            std::fs::read_to_string(&spill).expect("spill text"),
            expected,
            "the spill holds the complete text from the first character on"
        );
        assert_eq!(
            std::fs::read_dir(results()).expect("results root").count(),
            1,
            "exactly one spill file was allocated"
        );
        // The bounded preview is the deterministic head + tail of the same
        // complete text.
        assert!(settled.preview.starts_with("aaaa"));
        assert!(settled.preview.ends_with("cross-after"));
        assert!(settled.preview.len() <= 64);
    }

    /// An incomplete capture never advertises its partial spill as the
    /// complete output: the locator is discarded and the partial file is
    /// removed best-effort, while the bounded preview remains the
    /// canonical record.
    #[test]
    fn an_incomplete_capture_never_advertises_a_partial_spill() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("tool-output");
        let store =
            ManagedToolOutput::new(ConversationId::new("conv"), &root).expect("managed store");
        let mut capture = SpillCapture::new(16);
        capture
            .push(&"x".repeat(32), &store)
            .expect("crossing push");
        let partial = root.join("results/result_1.txt");
        assert!(partial.exists(), "the spill was allocated");
        // The capture settles incompletely (a reader failed after the
        // spill was allocated): no locator is published and the partial
        // file is removed.
        let settled = capture.finish(false);
        assert!(!settled.complete);
        assert!(settled.output_locator.is_none());
        assert!(
            !partial.exists(),
            "the partial spill was removed best-effort"
        );
    }
}
