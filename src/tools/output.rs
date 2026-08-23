//! Tool Plane result normalization and managed-output capture.
//!
//! Origin adapters translate provider/runtime protocols into logical textual
//! result fragments. This module owns the mode-independent representation
//! policy and the mode-dependent storage lifecycle:
//!
//! - foreground output stays in a bounded preview until it crosses the shared
//!   preview threshold, then allocates one complete result spill lazily;
//! - background output is written to the sink that the background registry
//!   allocated and advertised at dispatch time.
//!
//! The complete managed output is auxiliary execution output. The bounded
//! preview and typed continuation are the only model-facing projection.

use std::io::{Read, Write};
use std::path::PathBuf;

use crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
use crate::tools::managed_output::{BackgroundOutput, ManagedToolOutput, ResultSpill};
use crate::tools::types::{ManagedOutputContinuation, ToolResultContent, TruncationState};

/// Test-only observation seam for committed background appends.
#[cfg(test)]
type AppendWatch = Option<tokio::sync::watch::Sender<u64>>;
#[cfg(not(test))]
type AppendWatch = Option<std::convert::Infallible>;

/// A bounded deterministic UTF-8 preview of one logical textual result.
///
/// The preview keeps the complete prefix while the result is within the
/// bound. Once the bound is crossed it retains a deterministic head/tail
/// projection with an explicit byte-count marker. The capture never stores an
/// arbitrary complete result in memory after the foreground spill starts.
#[derive(Clone, Debug)]
pub(crate) struct TextPreviewCapture {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: u64,
    limit: usize,
}

impl TextPreviewCapture {
    /// Creates a preview with the supplied byte bound.
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            head: Vec::new(),
            tail: Vec::new(),
            total: 0,
            limit,
        }
    }

    /// Adds one already UTF-8-decoded fragment.
    pub(crate) fn push(&mut self, text: &str) {
        let bytes = text.as_bytes();
        self.total = self.total.saturating_add(bytes.len() as u64);
        let half = self.limit / 2;
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

    /// Returns the deterministic preview and whether it crossed the bound.
    pub(crate) fn finish(self) -> (String, bool) {
        if self.total <= self.limit as u64 {
            let text = String::from_utf8(self.head).expect("the retained preview is decoded text");
            return (text, false);
        }
        let marker = format!(
            "\n...[truncated {} bytes]...\n",
            self.total - self.limit as u64
        );
        let content = self.limit.saturating_sub(marker.len());
        if content == 0 {
            let mut marker = marker;
            marker.truncate(self.limit);
            return (marker, true);
        }
        let head_end = floor_char_boundary(&self.head, self.head.len().min(content / 2));
        let head = &self.head[..head_end];
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

    /// The complete logical byte count observed by the capture.
    pub(crate) fn total(&self) -> u64 {
        self.total
    }
}

/// The settled state shared by foreground and background textual capture.
#[derive(Debug)]
pub(crate) struct CapturedOutput {
    /// The deterministic bounded UTF-8 preview.
    pub(crate) preview: String,
    /// Whether the complete logical representation crossed the preview bound.
    pub(crate) truncated: bool,
    /// Complete logical representation size in bytes.
    pub(crate) total_bytes: u64,
    /// Whether every observed fragment was retained in managed output when a
    /// managed-output file exists.
    pub(crate) complete: bool,
    /// The managed-output locator, when a spill/sink was allocated.
    pub(crate) output_locator: Option<PathBuf>,
}

/// One Tool Plane capture with the invocation-mode-selected storage
/// lifecycle. Origin adapters use this same seam; they do not select their
/// own overflow policy.
pub(crate) enum ToolOutputCapture {
    Foreground(ForegroundOutputCapture),
    Background(BackgroundOutputCapture),
}

impl ToolOutputCapture {
    /// Creates the lazy foreground capture.
    pub(crate) fn foreground() -> Self {
        Self::Foreground(ForegroundOutputCapture::new())
    }

    /// Creates a capture over an already dispatch-owned background sink.
    pub(crate) fn background(sink: BackgroundOutput, watch: AppendWatch) -> Self {
        Self::Background(BackgroundOutputCapture::new(
            FOREGROUND_TOOL_RESULT_PREVIEW_BYTES,
            sink,
            watch,
        ))
    }

    /// Whether this capture owns the background lifecycle.
    pub(crate) fn is_background(&self) -> bool {
        matches!(self, Self::Background(_))
    }

    /// Adds one logical UTF-8 fragment through the mode-specific storage
    /// implementation.
    pub(crate) fn push(
        &mut self,
        text: &str,
        foreground_store: Option<&ManagedToolOutput>,
    ) -> Result<(), String> {
        match self {
            Self::Foreground(capture) => capture.push(
                text,
                foreground_store.expect("foreground capture requires the managed-output store"),
            ),
            Self::Background(capture) => capture.push(text),
        }
    }

    /// Streams a logical result transport through the same mode-specific
    /// bounded preview and storage seam.
    pub(crate) fn push_reader<R: Read>(
        &mut self,
        reader: R,
        foreground_store: Option<&ManagedToolOutput>,
    ) -> Result<(), String> {
        match self {
            Self::Foreground(capture) => capture.push_reader(
                reader,
                foreground_store.expect("foreground capture requires the managed-output store"),
            ),
            Self::Background(capture) => capture.push_reader(reader),
        }
    }

    /// Settles the capture without changing the storage identity.
    pub(crate) fn finish(self, complete: bool) -> CapturedOutput {
        match self {
            Self::Foreground(capture) => capture.finish(complete),
            Self::Background(capture) => capture.finish(complete),
        }
    }
}

/// An incremental JSON/text writer backed by the shared Tool Plane capture.
///
/// JSON serializers may split a UTF-8 scalar across `Write` calls. This
/// adapter retains only an incomplete scalar (at most three bytes) between
/// calls, so structured MCP content can be serialized directly into the
/// bounded preview/spill seam without first materializing an arbitrary-sized
/// `String`.
pub(crate) struct ToolOutputWriter<'a> {
    capture: &'a mut ToolOutputCapture,
    foreground_store: Option<&'a ManagedToolOutput>,
    pending: Vec<u8>,
}

impl<'a> ToolOutputWriter<'a> {
    /// Creates a writer over one mode-selected Tool Plane capture.
    pub(crate) fn new(
        capture: &'a mut ToolOutputCapture,
        foreground_store: Option<&'a ManagedToolOutput>,
    ) -> Self {
        Self {
            capture,
            foreground_store,
            pending: Vec::new(),
        }
    }

    /// Flushes the final UTF-8 scalar and returns the capture diagnostic, if
    /// the logical representation could not be retained.
    pub(crate) fn finish(mut self) -> Result<(), String> {
        self.drain(true)
    }

    fn drain(&mut self, eof: bool) -> Result<(), String> {
        match std::str::from_utf8(&self.pending) {
            Ok(text) => {
                if !text.is_empty() {
                    self.capture.push(text, self.foreground_store)?;
                }
                self.pending.clear();
                Ok(())
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let text =
                        std::str::from_utf8(&self.pending[..valid]).expect("valid UTF-8 prefix");
                    self.capture.push(text, self.foreground_store)?;
                    self.pending.drain(..valid);
                }
                if error.error_len().is_some() {
                    return Err("the logical result transport is not valid UTF-8".to_owned());
                }
                if eof {
                    return Err(
                        "the logical result transport ends with incomplete UTF-8".to_owned()
                    );
                }
                Ok(())
            }
        }
    }
}

impl Write for ToolOutputWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        self.drain(false)
            .map_err(std::io::Error::other)
            .map(|()| bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Foreground Tool Plane capture with a lazy complete-result spill.
#[derive(Debug)]
pub(crate) struct ForegroundOutputCapture {
    preview: TextPreviewCapture,
    complete_prefix: Option<String>,
    spill: Option<ResultSpill>,
    #[cfg(test)]
    spill_started: Option<tokio::sync::watch::Sender<bool>>,
}

impl ForegroundOutputCapture {
    /// Creates a capture using the shared foreground preview policy.
    pub(crate) fn new() -> Self {
        Self::with_limit(FOREGROUND_TOOL_RESULT_PREVIEW_BYTES)
    }

    /// Creates a capture with an explicit bound for focused unit tests.
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            preview: TextPreviewCapture::new(limit),
            complete_prefix: Some(String::new()),
            spill: None,
            #[cfg(test)]
            spill_started: None,
        }
    }

    /// Installs the narrow test-only lazy-spill observation seam.
    #[cfg(test)]
    pub(crate) fn with_spill_started_watch(
        mut self,
        watch: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        self.spill_started = Some(watch);
        self
    }

    /// Adds one already UTF-8-decoded logical fragment.
    ///
    /// The crossing decision is made before copying the fragment into the
    /// retained prefix. A provider/runtime adapter can therefore pass a
    /// large fragment without first materializing an additional unbounded
    /// complete-result buffer.
    pub(crate) fn push(&mut self, text: &str, store: &ManagedToolOutput) -> Result<(), String> {
        let crosses = self.spill.is_none()
            && self.preview.total().saturating_add(text.len() as u64) > self.preview.limit as u64;
        self.preview.push(text);
        if let Some(spill) = &mut self.spill {
            return spill
                .write_all(text)
                .map_err(|error| format!("cannot write the foreground result spill: {error}"));
        }
        if !crosses {
            self.complete_prefix
                .as_mut()
                .expect("the complete prefix is retained before the spill")
                .push_str(text);
            return Ok(());
        }

        let spill = store
            .open_spill()
            .map_err(|error| format!("cannot allocate the foreground result spill: {error}"))?;
        self.spill = Some(spill);
        #[cfg(test)]
        if let Some(watch) = &self.spill_started {
            watch.send_replace(true);
        }
        let prefix = self
            .complete_prefix
            .take()
            .expect("the complete prefix is retained before the spill");
        let spill = self.spill.as_mut().expect("spill retained");
        spill
            .write_all(&prefix)
            .map_err(|error| format!("cannot write the foreground result spill: {error}"))?;
        spill
            .write_all(text)
            .map_err(|error| format!("cannot write the foreground result spill: {error}"))?;
        Ok(())
    }

    /// Streams a UTF-8 result transport without materializing it in memory.
    pub(crate) fn push_reader<R: Read>(
        &mut self,
        reader: R,
        store: &ManagedToolOutput,
    ) -> Result<(), String> {
        stream_utf8(reader, |text| self.push(text, store))
    }

    /// Settles the capture. A partial foreground spill remains available as a
    /// typed `Partial` locator; it is never falsely promoted to `Complete`.
    pub(crate) fn finish(self, complete: bool) -> CapturedOutput {
        let total_bytes = self.preview.total();
        let (preview, truncated) = self.preview.finish();
        let output_locator = self.spill.map(|spill| spill.path().to_path_buf());
        CapturedOutput {
            preview,
            truncated,
            total_bytes,
            complete,
            output_locator,
        }
    }
}

/// Background capture over an already dispatch-allocated output sink.
#[derive(Debug)]
pub(crate) struct BackgroundOutputCapture {
    preview: TextPreviewCapture,
    sink: BackgroundOutput,
    #[cfg(test)]
    appended: u64,
    #[cfg(test)]
    watch: Option<tokio::sync::watch::Sender<u64>>,
}

impl BackgroundOutputCapture {
    /// Creates a capture over the single dispatch-owned sink.
    pub(crate) fn new(limit: usize, sink: BackgroundOutput, watch: AppendWatch) -> Self {
        #[cfg(not(test))]
        let _ = watch;
        Self {
            preview: TextPreviewCapture::new(limit),
            sink,
            #[cfg(test)]
            appended: 0,
            #[cfg(test)]
            watch,
        }
    }

    /// Adds one fragment and appends it to the already advertised path.
    pub(crate) fn push(&mut self, text: &str) -> Result<(), String> {
        self.preview.push(text);
        self.sink
            .append(text)
            .map_err(|error| format!("cannot write the background result output: {error}"))?;
        #[cfg(test)]
        {
            self.appended = self.appended.saturating_add(text.len() as u64);
            if let Some(watch) = &self.watch {
                watch.send_replace(self.appended);
            }
        }
        Ok(())
    }

    /// Streams a UTF-8 result transport directly into the background sink.
    pub(crate) fn push_reader<R: Read>(&mut self, reader: R) -> Result<(), String> {
        stream_utf8(reader, |text| self.push(text))
    }

    /// Settles the capture while retaining the same dispatch-owned locator.
    pub(crate) fn finish(self, complete: bool) -> CapturedOutput {
        let total_bytes = self.preview.total();
        let (preview, truncated) = self.preview.finish();
        CapturedOutput {
            preview,
            truncated,
            total_bytes,
            complete,
            output_locator: Some(self.sink.path().to_path_buf()),
        }
    }
}

/// Derives typed continuation metadata from one settled capture.
pub(crate) fn continuation_for_capture(
    captured: &CapturedOutput,
    background: bool,
    diagnostic: Option<&str>,
) -> Option<ManagedOutputContinuation> {
    match (background, captured.complete, &captured.output_locator) {
        (_, true, Some(locator)) => Some(ManagedOutputContinuation::Complete {
            locator: locator.clone(),
        }),
        (_, false, Some(locator)) => Some(ManagedOutputContinuation::Partial {
            locator: locator.clone(),
            diagnostic: diagnostic
                .unwrap_or("output storage did not complete")
                .to_owned(),
        }),
        (_, false, None) => diagnostic.map(|diagnostic| ManagedOutputContinuation::Unavailable {
            diagnostic: diagnostic.to_owned(),
        }),
        (false, true, None) => None,
        (true, true, None) => Some(ManagedOutputContinuation::Unavailable {
            diagnostic: "the dispatch-owned background output locator is unavailable".to_owned(),
        }),
    }
}

/// Derives truthful truncation metadata from one settled capture.
pub(crate) fn truncation_for_capture(captured: &CapturedOutput) -> Option<TruncationState> {
    let truncated = captured.truncated || !captured.complete;
    truncated.then_some(TruncationState {
        truncated: true,
        original_bytes: captured.complete.then_some(captured.total_bytes),
    })
}

/// Renders a foreground continuation through the one typed renderer.
pub(crate) fn foreground_continuation_block(
    continuation: Option<&ManagedOutputContinuation>,
) -> Option<ToolResultContent> {
    continuation.map(|continuation| {
        ToolResultContent::Text(crate::message::content::TextBlock {
            text: continuation.render(),
        })
    })
}

/// Streams bytes as valid UTF-8 fragments, preserving code points split over
/// reader boundaries. Invalid transport bytes are an explicit protocol/storage
/// failure rather than malformed canonical text.
fn stream_utf8<R: Read, F>(mut reader: R, mut push: F) -> Result<(), String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let mut buffer = [0u8; 8192];
    let mut pending = Vec::new();
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("cannot read the logical result transport: {error}"))?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&buffer[..read]);
        match std::str::from_utf8(&pending) {
            Ok(text) => {
                if !text.is_empty() {
                    push(text)?;
                }
                pending.clear();
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let text = std::str::from_utf8(&pending[..valid]).expect("valid UTF-8 prefix");
                    push(text)?;
                    pending.drain(..valid);
                }
                if error.error_len().is_some() {
                    return Err("the logical result transport is not valid UTF-8".to_owned());
                }
            }
        }
    }
    if pending.is_empty() {
        Ok(())
    } else {
        Err("the logical result transport ends with incomplete UTF-8".to_owned())
    }
}

fn floor_char_boundary(text: &[u8], mut index: usize) -> usize {
    while index > 0 && std::str::from_utf8(&text[..index]).is_err() {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &[u8], mut index: usize) -> usize {
    while index < text.len() && std::str::from_utf8(&text[index..]).is_err() {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::{
        ForegroundOutputCapture, TextPreviewCapture, ToolOutputCapture, ToolOutputWriter,
        stream_utf8,
    };
    use crate::runtime::identity::ConversationId;
    use crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
    use crate::tools::managed_output::ManagedToolOutput;

    fn store(root: &std::path::Path) -> ManagedToolOutput {
        ManagedToolOutput::new(ConversationId::new("conv-output"), root).expect("store")
    }

    #[test]
    fn foreground_capture_has_no_spill_at_or_below_the_boundary() {
        let directory = tempfile::tempdir().expect("directory");
        let root = directory.path().join("tool-output");
        let managed = store(&root);
        let mut capture = ForegroundOutputCapture::new();
        capture
            .push(&"x".repeat(FOREGROUND_TOOL_RESULT_PREVIEW_BYTES), &managed)
            .expect("push");
        let settled = capture.finish(true);
        assert!(!settled.truncated);
        assert!(settled.output_locator.is_none());
        assert_eq!(
            std::fs::read_dir(root.join("results"))
                .expect("results")
                .count(),
            0
        );
    }

    #[test]
    fn foreground_capture_spills_complete_text_once_at_boundary_plus_one() {
        let directory = tempfile::tempdir().expect("directory");
        let root = directory.path().join("tool-output");
        let managed = store(&root);
        let mut capture = ForegroundOutputCapture::new();
        let complete = format!("{}😀", "a".repeat(FOREGROUND_TOOL_RESULT_PREVIEW_BYTES - 1));
        capture.push(&complete, &managed).expect("push");
        let settled = capture.finish(true);
        let locator = settled.output_locator.expect("spill locator");
        assert!(settled.truncated);
        assert_eq!(std::fs::read_to_string(locator).expect("spill"), complete);
        assert_eq!(
            std::fs::read_dir(root.join("results"))
                .expect("results")
                .count(),
            1
        );
    }

    #[test]
    fn reader_streaming_preserves_split_utf8_without_materializing_the_tail() {
        let mut fragments = Vec::new();
        stream_utf8(Cursor::new("prefix😀suffix".as_bytes()), |text| {
            fragments.push(text.to_owned());
            Ok(())
        })
        .expect("valid UTF-8");
        assert_eq!(fragments.concat(), "prefix😀suffix");
    }

    #[test]
    fn preview_is_valid_utf8_when_the_bound_lands_inside_a_code_point() {
        let mut preview = TextPreviewCapture::new(7);
        preview.push("aaaa😀bbbb");
        let (text, truncated) = preview.finish();
        assert!(truncated);
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
    }

    #[test]
    fn structured_writer_streams_split_utf8_into_the_shared_capture() {
        let directory = tempfile::tempdir().expect("directory");
        let root = directory.path().join("tool-output");
        let managed = store(&root);
        let mut capture = ToolOutputCapture::Foreground(ForegroundOutputCapture::with_limit(8));
        let mut writer = ToolOutputWriter::new(&mut capture, Some(&managed));
        writer.write_all(b"\"a\\n").expect("JSON prefix");
        writer.write_all(&[0xf0]).expect("split UTF-8 prefix");
        writer
            .write_all(&[0x9f, 0x98, 0x80])
            .expect("split UTF-8 suffix");
        writer.write_all(b"\"").expect("JSON close");
        writer.finish().expect("writer finish");
        let captured = capture.finish(true);
        let locator = captured.output_locator.expect("oversized writer spill");
        let bytes = std::fs::read(locator).expect("writer spill");
        assert_eq!(bytes, b"\"a\\n\xf0\x9f\x98\x80\"");
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_ok());
        assert!(std::str::from_utf8(captured.preview.as_bytes()).is_ok());
    }
}
