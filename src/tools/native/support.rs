//! Shared deterministic helpers of the native tool implementations.

use std::path::{Path, PathBuf};

use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

/// Resolves one model-facing file path against the authoritative execution
/// directory. Absolute paths remain host filesystem paths; relative paths are
/// interpreted from `cwd` without applying a containment policy.
#[must_use]
pub fn resolve_path(cwd: &Path, requested: &str) -> PathBuf {
    let path = Path::new(requested);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
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
/// When the final path component is a symlink, the commit target is the
/// symlink's destination rather than the link itself. This preserves ordinary
/// write-through filesystem semantics while retaining atomic replacement of
/// the destination file.
pub fn atomic_commit(target: &Path, content: &[u8]) -> Result<(), String> {
    let commit_target = follow_final_symlink(target)?;
    let parent = commit_target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", commit_target.display()))?;
    let temp = create_temp_in(parent)?;
    if let Err(error) = std::fs::write(&temp, content) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("cannot write {}: {error}", temp.display()));
    }
    if let Err(error) = std::fs::rename(&temp, &commit_target) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "cannot persist {}: {error}",
            commit_target.display()
        ));
    }
    Ok(())
}

/// Resolves an existing final-component symlink without changing the path
/// when the target is an ordinary file or a not-yet-created file.
fn follow_final_symlink(target: &Path) -> Result<PathBuf, String> {
    let metadata = match std::fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(target.to_path_buf());
        }
        Err(error) => return Err(format!("cannot inspect {}: {error}", target.display())),
    };
    if !metadata.file_type().is_symlink() {
        return Ok(target.to_path_buf());
    }
    let link = std::fs::read_link(target)
        .map_err(|error| format!("cannot resolve {}: {error}", target.display()))?;
    let destination = if link.is_absolute() {
        link
    } else {
        target
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", target.display()))?
            .join(link)
    };
    if destination.exists() {
        std::fs::canonicalize(&destination)
            .map_err(|error| format!("cannot resolve {}: {error}", target.display()))
    } else {
        Ok(destination)
    }
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

/// A normalized successful structured result used by tools whose contract is
/// intentionally JSON (for example the subagent intrinsic).
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

/// A normalized successful plain-text result.
#[must_use]
pub fn success_text(
    text: impl Into<String>,
    truncation: Option<crate::tools::types::TruncationState>,
) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Text(
            crate::message::content::TextBlock { text: text.into() },
        )],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation,
        managed_output: None,
    }
}
