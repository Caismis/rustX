//! Shared deterministic output limits of the tool plane.
//!
//! Named runtime/tool constants replace scattered magic numbers. Changing a
//! value because of a concrete implementation constraint is acceptable but
//! must be reported explicitly; tests assert limits at their boundaries.

use std::time::Duration;

/// The maximum model-facing bytes of one tool result payload.
pub const MAX_MODEL_TOOL_RESULT_BYTES: usize = 64 * 1024;

/// The maximum number of Glob results returned to the model.
pub const MAX_GLOB_RESULTS: usize = 2_000;

/// The maximum number of Grep matches returned to the model.
pub const MAX_GREP_MATCHES: usize = 2_000;

/// The maximum length of one progress message text.
pub const MAX_PROGRESS_MESSAGE_BYTES: usize = 512;

/// The per-stream bounded preview retained for Bash stdout/stderr/combined.
pub const BASH_STREAM_PREVIEW_BYTES: usize = 16 * 1024;

/// The grace period between TERM and KILL when terminating a Bash process
/// group.
pub const BASH_TERM_GRACE: Duration = Duration::from_secs(2);

/// The default foreground Bash timeout when the model omits `timeout_ms`.
pub const DEFAULT_FOREGROUND_BASH_TIMEOUT: Duration = Duration::from_secs(120);

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
    let head = limit / 2;
    let tail = limit - head;
    let marker = format!("\n...[truncated {} bytes]...\n", data.len() - limit);
    let mut out = Vec::with_capacity(limit + marker.len());
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
