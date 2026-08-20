//! Shared deterministic helpers of the native tool implementations.

use std::path::{Path, PathBuf};

use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

/// The exact serialized byte length of one model-facing JSON value.
///
/// The bounded search tools budget their payload against the bytes the model
/// actually receives, so the only trustworthy measure is the serialization
/// itself: JSON string escaping (`"`, `\`, control characters), field names,
/// array and object punctuation, and numeric widths all change the size and
/// none of them are recoverable from a string's own length.
///
/// A value that cannot be serialized is reported as [`usize::MAX`], so a
/// caller budgeting against a cap always rejects it rather than admitting an
/// unmeasured item. Native tools only ever build serializable values.
#[must_use]
pub fn json_bytes(value: &serde_json::Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// The byte cost of appending one already-measured element to a JSON array
/// that currently holds `present` elements.
///
/// The array's own brackets belong to the enclosing envelope; each element
/// after the first additionally pays for its separating comma.
#[must_use]
pub fn json_array_element_cost(element_bytes: usize, present: usize) -> usize {
    element_bytes.saturating_add(usize::from(present > 0))
}

/// The one file-mutation commit of the native tool plane.
///
/// Write and Edit both replace a whole file with a whole new content
/// snapshot, so they share one commit: a uniquely named temporary file is
/// created inside the target's own directory, the complete content is
/// written to it, and it is renamed over the target. A reader of the target
/// therefore observes either the previous file or the complete new one, and
/// a failed commit leaves no temporary file behind.
///
/// # Errors
///
/// Returns an explicit diagnostic when the target has no parent directory,
/// when the temporary file cannot be created or written, or when the rename
/// fails.
pub fn atomic_commit(target: &Path, content: &[u8]) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", target.display()))?;
    let temp = create_temp_in(parent)?;
    if let Err(error) = std::fs::write(&temp, content) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("cannot write {}: {error}", temp.display()));
    }
    if let Err(error) = std::fs::rename(&temp, target) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("cannot persist {}: {error}", target.display()));
    }
    Ok(())
}

/// Creates a unique temporary file inside `parent` for the atomic commit.
fn create_temp_in(parent: &Path) -> Result<PathBuf, String> {
    for attempt in 0..100u32 {
        let candidate = parent.join(format!(".rustx-tool-tmp-{}-{attempt}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(error) if attempt >= 99 => {
                return Err(format!(
                    "cannot create a temporary file in {}: {error}",
                    parent.display()
                ));
            }
            Err(_) => {}
        }
    }
    Err("temporary file creation exhausted its attempts".to_owned())
}

/// A normalized failed tool result. A business-level failure of a native
/// tool is a normal failed tool result; it never becomes an attempt-level
/// runtime failure.
#[must_use]
pub fn failed_result(error: impl Into<String>) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Failed {
            error: error.into(),
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// A normalized successful structured result.
#[must_use]
pub fn success_json(value: serde_json::Value) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json { value }],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// A normalized successful structured result with truncation metadata and
/// artifact references.
#[must_use]
pub fn success_json_with(
    value: serde_json::Value,
    truncation: Option<crate::tools::types::TruncationState>,
    artifacts: Vec<crate::message::content::FileReference>,
) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json { value }],
        duration_ms: 0,
        exit_code: None,
        artifacts,
        truncation,
        managed_output: None,
    }
}
