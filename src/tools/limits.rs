//! Shared deterministic output limits of the tool plane.
//!
//! Named runtime/tool constants replace scattered magic numbers. Changing a
//! value because of a concrete implementation constraint is acceptable but
//! must be reported explicitly; tests assert limits at their boundaries.

use std::time::Duration;

/// The maximum bytes of one structured-document source that native Read
/// hands to its document decoder (Issue #48). This is an extraction-resource
/// bound, not a model-facing output bound: decoding happens before Read's
/// 2000-line/50KB projection, so a huge archive-backed document must be
/// rejected before unbounded parse work starts, not after.
pub const MAX_DOCUMENT_SOURCE_BYTES: usize = 32 * 1024 * 1024;

/// The maximum model-facing bytes of one tool result payload.
pub const MAX_MODEL_TOOL_RESULT_BYTES: usize = 64 * 1024;

/// The maximum bytes of one managed-output continuation diagnostic.
///
/// The absolute managed-output locator and the fixed continuation guidance
/// are essential continuation state and are retained structurally; an
/// output-storage diagnostic is advisory and is always bounded to this cap
/// when the continuation is rendered, so an arbitrary diagnostic can never
/// make the canonical terminal record unbounded.
pub const MAX_OUTPUT_CONTINUATION_DIAGNOSTIC_BYTES: usize = 1024;

/// The tool-owned content budget of the native sequential/search projections.
pub const NATIVE_FILE_TOOL_MAX_BYTES: usize = 50 * 1024;

/// The maximum number of complete lines returned by Read when its own byte
/// budget is not reached first.
pub const MAX_READ_LINES: usize = 2_000;

/// The canonical bounded default number of Grep matches, used when the model
/// omits `limit`.
pub const DEFAULT_GREP_MATCHES: u64 = 100;

/// The canonical bounded default number of Glob results, used when the model
/// omits `limit`.
pub const DEFAULT_GLOB_RESULTS: u64 = 1_000;

/// The maximum number of context lines Grep returns on each side of a
/// matching line.
pub const MAX_GREP_CONTEXT_LINES: u32 = 20;

/// The maximum Unicode scalar values of one line Grep reports. A longer line
/// is shortened with an explicit marker.
pub const MAX_GREP_LINE_CHARS: usize = 500;

/// The maximum length of one progress message text.
pub const MAX_PROGRESS_MESSAGE_BYTES: usize = 512;

/// The maximum number of progress observations one active foreground tool
/// call retains before structural settlement.
///
/// Each retained observation is already payload-bounded by
/// [`bound_tool_progress`]; this constant bounds their count. Once the bound
/// is reached, the first `MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1`
/// observations are pinned and the final slot tracks the newest observation,
/// so retained progress always ends with the most recent executor state and
/// the retained count never exceeds this bound, even while a misbehaving
/// executor reports without pause.
pub const MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL: usize = 128;

/// The shared foreground Tool Plane preview threshold.
///
/// Results at or below this bound remain direct model-facing results. Once a
/// logical textual representation crosses it, the Tool Plane allocates one
/// lazy complete-result spill. This is a projection threshold, not the
/// complete managed-output storage limit.
pub const FOREGROUND_TOOL_RESULT_PREVIEW_BYTES: usize = 16 * 1024;

/// The grace period between TERM and KILL when terminating a Bash process
/// group.
pub const BASH_TERM_GRACE: Duration = Duration::from_secs(2);

/// The bounded window after a Bash TERMINATE request (cancellation or
/// timeout) after which missing process terminality becomes failure intent.
/// It never authorizes settlement without the terminal child-set event. The
/// same duration separately bounds capture only after process terminality.
pub const BASH_TERMINATION_CONFIRMATION: Duration = Duration::from_secs(6);

/// The default foreground Bash timeout when the model omits `timeout`.
pub const DEFAULT_FOREGROUND_BASH_TIMEOUT: Duration = Duration::from_mins(2);

/// Produces a deterministic bounded preview of `data`.
///
/// When the data exceeds `limit` bytes, a head/tail scheme is used: the
/// first half of the limit keeps the head, a truncation marker identifies
/// the discarded span, and the second half keeps the tail. The returned
/// boolean reports whether the output was truncated.
#[must_use]
pub fn bounded_preview(data: &[u8], limit: usize) -> (Vec<u8>, bool) {
    if data.len() <= limit {
        return (data.to_vec(), false);
    }
    let marker = format!("\n...[truncated {} bytes]...\n", data.len() - limit);
    if marker.len() >= limit {
        let mut marker = marker.into_bytes();
        marker.truncate(limit);
        return (marker, true);
    }
    let content = limit.saturating_sub(marker.len());
    let head = content / 2;
    let tail = content - head;
    let mut out = Vec::with_capacity(limit);
    out.extend_from_slice(&data[..head]);
    out.extend_from_slice(marker.as_bytes());
    out.extend_from_slice(&data[data.len() - tail..]);
    (out, true)
}

/// Produces a deterministic bounded UTF-8 text preview of `data`.
///
/// Head and tail are converted with UTF-8-lossy conversion independently,
/// so subprocess bytes that are not UTF-8 never corrupt the model-facing
/// preview (raw artifact bytes are preserved separately). The returned
/// boolean reports whether the output was truncated.
#[must_use]
pub fn bounded_text_preview(data: &[u8], limit: usize) -> (String, bool) {
    if data.len() <= limit {
        return (String::from_utf8_lossy(data).into_owned(), false);
    }
    let (bytes, truncated) = bounded_preview(data, limit);
    (String::from_utf8_lossy(&bytes).into_owned(), truncated)
}

/// Bounds one progress notification through the canonical normalization
/// shared by the foreground, background, and every origin executor path.
///
/// This is the canonical finite-value invariant boundary: `completed` and
/// `total` that are not finite (`NaN`, `+inf`, `-inf`) are dropped, so a
/// canonical serialization can never fail because an executor produced a
/// non-finite value. Finite fractional values are preserved exactly.
/// `message` is bounded to [`MAX_PROGRESS_MESSAGE_BYTES`] and never panics
/// on UTF-8: the bound is cut at a character boundary, so the result is
/// always valid UTF-8 and the output is deterministic for a given input.
#[must_use]
pub fn bound_tool_progress(
    progress: crate::tools::types::ToolProgress,
) -> crate::tools::types::ToolProgress {
    let crate::tools::types::ToolProgress {
        message,
        completed,
        total,
    } = progress;
    crate::tools::types::ToolProgress {
        message: message.map(|text| bound_utf8_text(text, MAX_PROGRESS_MESSAGE_BYTES)),
        completed: completed.filter(|value| value.is_finite()),
        total: total.filter(|value| value.is_finite()),
    }
}

/// Truncates `text` to at most `max_bytes` bytes at a UTF-8 character
/// boundary. Never panics and never splits a code point.
#[must_use]
pub fn bound_utf8_text(mut text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

#[cfg(test)]
mod tests {
    use super::{MAX_PROGRESS_MESSAGE_BYTES, bound_tool_progress};
    use crate::tools::types::ToolProgress;

    /// ASCII exactly at the boundary is preserved unchanged.
    #[test]
    fn ascii_at_the_boundary_is_preserved() {
        let progress = ToolProgress {
            message: Some("x".repeat(MAX_PROGRESS_MESSAGE_BYTES)),
            completed: Some(1.0),
            total: Some(2.0),
        };
        let bounded = bound_tool_progress(progress.clone());
        assert_eq!(
            bounded.message.as_deref().expect("message").len(),
            MAX_PROGRESS_MESSAGE_BYTES
        );
        assert_eq!(bounded.message, progress.message);
        assert_eq!(bounded.completed, Some(1.0));
        assert_eq!(bounded.total, Some(2.0));
    }

    /// ASCII crossing the boundary is truncated to the bound.
    #[test]
    fn ascii_crossing_the_boundary_is_truncated() {
        let bounded = bound_tool_progress(ToolProgress {
            message: Some("x".repeat(MAX_PROGRESS_MESSAGE_BYTES + 10)),
            completed: None,
            total: None,
        });
        assert_eq!(
            bounded.message.as_deref().expect("message").len(),
            MAX_PROGRESS_MESSAGE_BYTES
        );
    }

    /// A multibyte code point crossing the boundary is never split: the
    /// truncation index walks back to a character boundary and the result
    /// is valid UTF-8.
    #[test]
    fn multibyte_code_point_crossing_the_boundary_is_never_split() {
        // MAX-1 ASCII bytes plus one 4-byte emoji: the 512-byte bound lands
        // inside the emoji, so the truncation keeps the 511 ASCII bytes.
        let message = format!("{}😀", "x".repeat(MAX_PROGRESS_MESSAGE_BYTES - 1));
        let bounded = bound_tool_progress(ToolProgress {
            message: Some(message),
            completed: None,
            total: None,
        });
        let text = bounded.message.expect("message");
        assert_eq!(text, "x".repeat(MAX_PROGRESS_MESSAGE_BYTES - 1));
        assert_eq!(text.len(), MAX_PROGRESS_MESSAGE_BYTES - 1);
    }

    /// Emoji-only messages are truncated to a whole number of code points.
    #[test]
    fn emoji_only_messages_are_truncated_at_a_code_point_boundary() {
        let bounded = bound_tool_progress(ToolProgress {
            message: Some("😀".repeat(200)),
            completed: None,
            total: None,
        });
        let text = bounded.message.expect("message");
        assert_eq!(text, "😀".repeat(MAX_PROGRESS_MESSAGE_BYTES / 4));
        assert!(text.len() <= MAX_PROGRESS_MESSAGE_BYTES);
    }

    /// The normalization is deterministic and preserves the numeric fields.
    #[test]
    fn normalization_is_deterministic_and_preserves_counts() {
        let progress = ToolProgress {
            message: Some("y".repeat(MAX_PROGRESS_MESSAGE_BYTES + 5)),
            completed: Some(7.0),
            total: Some(9.0),
        };
        let first = bound_tool_progress(progress.clone());
        let second = bound_tool_progress(progress);
        assert_eq!(first, second);
        assert_eq!(first.completed, Some(7.0));
        assert_eq!(first.total, Some(9.0));
        let none_message = bound_tool_progress(ToolProgress {
            message: None,
            completed: Some(3.0),
            total: Some(4.0),
        });
        assert_eq!(none_message.message, None);
        assert_eq!(none_message.completed, Some(3.0));
        assert_eq!(none_message.total, Some(4.0));
    }

    /// The canonical finite-value invariant: non-finite `completed`/`total`
    /// values are dropped at the shared normalization boundary, so no later
    /// canonical serialization can fail on them.
    #[test]
    fn non_finite_values_are_dropped_at_the_canonical_boundary() {
        for non_finite in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let bounded = bound_tool_progress(ToolProgress {
                message: Some("bad".to_owned()),
                completed: Some(non_finite),
                total: Some(non_finite),
            });
            assert_eq!(
                bounded.completed, None,
                "non-finite completed must never reach canonical progress"
            );
            assert_eq!(
                bounded.total, None,
                "non-finite total must never reach canonical progress"
            );
            assert_eq!(bounded.message.as_deref(), Some("bad"));
        }
    }

    /// A non-finite value in one field does not drop the other, finite field.
    #[test]
    fn a_non_finite_value_only_drops_its_own_field() {
        let bounded = bound_tool_progress(ToolProgress {
            message: None,
            completed: Some(f64::NAN),
            total: Some(2.5),
        });
        assert_eq!(bounded.completed, None);
        assert_eq!(bounded.total, Some(2.5));
    }

    /// Fractional finite progress survives the canonical boundary.
    #[test]
    fn fractional_progress_survives_the_canonical_boundary() {
        let bounded = bound_tool_progress(ToolProgress {
            message: Some("fractional".to_owned()),
            completed: Some(0.5),
            total: Some(3.5),
        });
        assert_eq!(bounded.completed, Some(0.5));
        assert_eq!(bounded.total, Some(3.5));
    }
}
