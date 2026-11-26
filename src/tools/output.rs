//! Tool Plane result normalization and managed-output capture.
//!
//! Origin adapters translate provider/runtime protocols into logical textual
//! result fragments. This module owns the mode-independent representation
//! policy and the mode-dependent storage lifecycle:
//!
//! - MCP keeps a bounded model preview and can stream normalized text to
//!   its separately owned Execution Client import;
//! - the explicit supervisor regression fixture uses the standalone lazy
//!   spill capture ([`ForegroundOutputCapture`]). Native Bash capture lives
//!   in the Execution Client;
//! - background output is written to the sink that the background registry
//!   allocated and advertised at dispatch time.
//!
//! The complete managed output is auxiliary execution output. Capture owns
//! previews and typed continuation metadata;
//! [`crate::tools::types::ToolExecutionResult::model_facing_projection`] is the
//! single aggregate model-facing projection that combines those facts with
//! status and tool-owned content.
//! A native runtime consumer may additionally request a complete, separately
//! bounded stdout channel; it never consumes a preview or multiplexed spill
//! as the original structured return value.

use std::io::Write;
#[cfg(any(test, feature = "test-support"))]
use std::path::PathBuf;

use crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
#[cfg(any(test, feature = "test-support"))]
use crate::tools::managed_output::{BackgroundOutput, ManagedToolOutput, ResultSpill};
#[cfg(test)]
use crate::tools::types::ManagedOutputContinuation;
use crate::tools::types::TruncationState;

pub(crate) use rustx_types::tools::output::stdout;
pub(crate) use stdout::BoundedStdoutCapture;

/// Test-only observation seam for committed background appends.
#[cfg(test)]
type AppendWatch = Option<tokio::sync::watch::Sender<u64>>;
#[cfg(all(not(test), feature = "test-support"))]
type AppendWatch = Option<std::convert::Infallible>;

pub(crate) use rustx_process_supervisor::output_text::TextPreviewCapture;

/// The settled state shared by foreground and background textual capture.
#[derive(Debug)]
pub(crate) struct CapturedOutput {
    /// The deterministic bounded UTF-8 preview.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) preview: String,
    /// Whether the complete logical representation crossed the preview bound.
    pub(crate) truncated: bool,
    /// Complete logical representation size in bytes.
    pub(crate) total_bytes: u64,
    /// Whether every observed fragment was retained in managed output when a
    /// managed-output file exists.
    pub(crate) complete: bool,
    /// The managed-output locator, when a spill/sink was allocated.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) output_locator: Option<PathBuf>,
}

/// A bounded Tool Plane preview with no file storage behind it.
pub(crate) struct ToolOutputCapture(TextPreviewCapture);

impl ToolOutputCapture {
    pub(crate) fn preview_only() -> Self {
        Self(TextPreviewCapture::new(
            FOREGROUND_TOOL_RESULT_PREVIEW_BYTES,
        ))
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        self.0.total()
    }

    pub(crate) fn push(&mut self, text: &str) {
        self.0.push(text);
    }

    /// `complete` records that every observed fragment was normalized.
    pub(crate) fn finish(self, complete: bool) -> CapturedOutput {
        let total_bytes = self.0.total();
        let (preview, truncated) = self.0.finish();
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = preview;
        CapturedOutput {
            #[cfg(any(test, feature = "test-support"))]
            preview,
            truncated,
            total_bytes,
            complete,
            #[cfg(any(test, feature = "test-support"))]
            output_locator: None,
        }
    }
}

/// An incremental JSON/text writer backed by the shared Tool Plane capture.
///
/// JSON serializers may split a UTF-8 scalar across `Write` calls. Each
/// `write` is processed in bounded slices and this adapter retains only an
/// incomplete scalar (at most three bytes) between calls, so structured MCP
/// content can be serialized directly into the bounded preview seam without
/// first materializing or parking an arbitrary-sized buffer.
/// The bounded text sink a [`ToolOutputWriter`] drains into.
type OutputSink<'a> = Box<dyn FnMut(&str) -> Result<(), String> + 'a>;

pub(crate) struct ToolOutputWriter<'a> {
    push: OutputSink<'a>,
    pending: Vec<u8>,
}

/// The bounded slice size of one writer drain step.
const WRITER_SLICE_BYTES: usize = 8192;

impl<'a> ToolOutputWriter<'a> {
    /// Creates a writer over an arbitrary bounded text sink — for example a
    /// closure tapping one serialization into both the aggregate capture
    /// and a bounded display preview.
    pub(crate) fn over_sink(push: impl FnMut(&str) -> Result<(), String> + 'a) -> Self {
        Self::with_sink(push)
    }

    fn with_sink(push: impl FnMut(&str) -> Result<(), String> + 'a) -> Self {
        Self {
            push: Box::new(push),
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
                    (self.push)(text)?;
                }
                self.pending.clear();
                Ok(())
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let text =
                        std::str::from_utf8(&self.pending[..valid]).expect("valid UTF-8 prefix");
                    (self.push)(text)?;
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
        for chunk in bytes.chunks(WRITER_SLICE_BYTES) {
            self.pending.extend_from_slice(chunk);
            self.drain(false).map_err(std::io::Error::other)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Standalone foreground capture with a lazy complete-result spill.
///
/// The direct supervisor fixture uses this path to assert spill ownership
/// under injected I/O faults. Product Bash uses the Execution Client.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub(crate) struct ForegroundOutputCapture {
    preview: TextPreviewCapture,
    complete_prefix: Option<String>,
    spill: Option<ResultSpill>,
    #[cfg(test)]
    spill_started: Option<tokio::sync::watch::Sender<bool>>,
}

#[cfg(any(test, feature = "test-support"))]
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
            && self.preview.total().saturating_add(text.len() as u64) > self.preview.limit() as u64;
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
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub(crate) struct BackgroundOutputCapture {
    preview: TextPreviewCapture,
    sink: BackgroundOutput,
    #[cfg(test)]
    appended: u64,
    #[cfg(test)]
    watch: Option<tokio::sync::watch::Sender<u64>>,
}

#[cfg(any(test, feature = "test-support"))]
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

/// Derives typed continuation metadata for the trace projection fixture.
#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{ForegroundOutputCapture, TextPreviewCapture, ToolOutputCapture, ToolOutputWriter};
    use crate::runtime::identity::ConversationId;
    use crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
    use crate::tools::managed_output::ManagedToolOutput;

    fn store(root: &std::path::Path) -> ManagedToolOutput {
        ManagedToolOutput::new(
            ConversationId::new("conv_b4cee425-72b5-7705-895a-08669869c060"),
            root,
        )
        .expect("store")
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
            std::fs::read_dir(managed.root().unwrap())
                .expect("results")
                .filter(|entry| entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("r-")))
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
            std::fs::read_dir(managed.root().unwrap())
                .expect("results")
                .filter(|entry| entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("r-")))
                .count(),
            1
        );
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
    fn preview_only_capture_never_allocates_a_spill_on_overflow() {
        let directory = tempfile::tempdir().expect("directory");
        let root = directory.path().join("tool-output");
        let managed = store(&root);
        let mut capture = ToolOutputCapture::preview_only();
        capture.push(&"x".repeat(FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1));
        let settled = capture.finish(true);
        assert!(settled.complete, "input was fully normalized");
        assert!(settled.truncated);
        assert!(settled.output_locator.is_none());
        assert!(
            settled.preview.contains("truncated"),
            "bounded marker retained: {}",
            settled.preview
        );
        assert_eq!(
            std::fs::read_dir(managed.root().unwrap())
                .expect("results")
                .filter(|entry| entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("r-")))
                .count(),
            0,
            "preview-only foreground never allocates a result spill"
        );
    }

    #[test]
    fn structured_writer_streams_split_utf8_into_the_shared_capture() {
        let mut capture = ToolOutputCapture(TextPreviewCapture::new(
            FOREGROUND_TOOL_RESULT_PREVIEW_BYTES,
        ));
        let mut writer = ToolOutputWriter::over_sink(|text| {
            capture.push(text);
            Ok(())
        });
        writer.write_all(b"\"a\\n").expect("JSON prefix");
        writer.write_all(&[0xf0]).expect("split UTF-8 prefix");
        writer
            .write_all(&[0x9f, 0x98, 0x80])
            .expect("split UTF-8 suffix");
        writer.write_all(b"\"").expect("JSON close");
        writer.finish().expect("writer finish");
        let captured = capture.finish(true);
        assert!(captured.output_locator.is_none());
        assert!(!captured.truncated);
        assert_eq!(
            captured.preview, "\"a\\n\u{1f600}\"",
            "split scalars assemble exactly"
        );
        assert!(serde_json::from_str::<serde_json::Value>(&captured.preview).is_ok());
    }

    #[test]
    fn a_multi_mib_single_write_never_parks_unbounded_pending_bytes() {
        let mut capture = ToolOutputCapture::preview_only();
        let mut writer = ToolOutputWriter::over_sink(|text| {
            capture.push(text);
            Ok(())
        });
        writer
            .write_all(&vec![b'x'; 4 * 1024 * 1024])
            .expect("multi-MiB write");
        assert!(
            writer.pending.len() <= 3,
            "only an incomplete scalar may remain: {}",
            writer.pending.len()
        );
        assert!(
            writer.pending.capacity() < 64 * 1024,
            "pending must stay slice-bounded: {}",
            writer.pending.capacity()
        );
        writer.finish().expect("writer finish");
        let captured = capture.finish(true);
        assert!(captured.truncated);
        assert_eq!(captured.total_bytes, 4 * 1024 * 1024);
    }
}
