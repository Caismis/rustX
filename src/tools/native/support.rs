//! Shared deterministic helpers of the native tool implementations.

use std::path::{Path, PathBuf};

use crate::tools::managed_output::ManagedToolOutput;
use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

/// Interprets one model-facing file path against the authoritative execution
/// directory and lexically normalizes it.
///
/// This is a syntax-only boundary: it resolves relative paths against `cwd`,
/// removes `.` and `..` components without consulting the filesystem, and
/// leaves symlink/effective-target resolution to the mutation layer. Absolute
/// paths remain ordinary host filesystem paths and no containment policy is
/// applied.
#[must_use]
pub fn interpret_path(cwd: &Path, requested: &str) -> PathBuf {
    let path = Path::new(requested);
    let interpreted = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    lexical_normalize(&interpreted)
}

/// Removes `.` and `..` components without traversing the filesystem.
///
/// The native execution cwd is absolute, so the result of
/// [`interpret_path`] is absolute. The component handling nevertheless keeps
/// relative `..` components when called with a relative path, which preserves
/// the standard `Path` semantics and avoids string-based path rewriting.
#[must_use]
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let can_pop = normalized
                    .components()
                    .next_back()
                    .is_some_and(|last| matches!(last, std::path::Component::Normal(_)));
                if can_pop {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            std::path::Component::Normal(name) => normalized.push(name),
        }
    }
    normalized
}

/// One filesystem target prepared for a native whole-file mutation.
///
/// The path is resolved once, before any parent directory or temporary-file
/// side effect. Write/Edit pass this same value through to `atomic_commit`,
/// so the ownership decision and the committed target cannot diverge merely
/// because a final or ancestor symlink is involved.
#[derive(Debug)]
pub struct MutationTarget {
    path: PathBuf,
}

impl MutationTarget {
    /// The effective absolute host path that will be replaced.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Resolves and authorizes one native mutation target before side effects.
///
/// This is deliberately not a workspace sandbox. `ManagedToolOutput` owns
/// the only model-mutation exception: its runtime-owned namespace is
/// read-only even though Read/Grep/Glob can inspect it.
/// `interpreted` is the already absolute, lexically normalized result of
/// [`interpret_path`].
pub fn prepare_mutation_target(
    interpreted: &Path,
    tool_output: &ManagedToolOutput,
) -> Result<MutationTarget, String> {
    let effective = resolve_effective_path(interpreted)?;
    tool_output
        .ensure_model_mutation_allowed(&effective)
        .map_err(|error| error.to_string())?;
    Ok(MutationTarget { path: effective })
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
/// `prepare_mutation_target` resolves final and ancestor symlinks before this
/// function is called. The prepared target is therefore both the path that
/// passed the managed-output ownership check and the path committed here;
/// ordinary write-through semantics are retained without replacing a link.
pub fn atomic_commit(target: &MutationTarget, content: &[u8]) -> Result<(), String> {
    let parent = target
        .path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", target.path.display()))?;
    let temp = create_temp_in(parent)?;
    if let Err(error) = std::fs::write(&temp, content) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("cannot write {}: {error}", temp.display()));
    }
    if let Err(error) = std::fs::rename(&temp, &target.path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("cannot persist {}: {error}", target.path.display()));
    }
    Ok(())
}

/// Resolves all existing symlink components and preserves the effective
/// destination of a dangling final symlink or a not-yet-created descendant.
fn resolve_effective_path(target: &Path) -> Result<PathBuf, String> {
    let mut candidate = lexical_normalize(target);
    for _ in 0..40 {
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link = std::fs::read_link(&candidate)
                    .map_err(|error| format!("cannot resolve {}: {error}", target.display()))?;
                let destination = if link.is_absolute() {
                    link
                } else {
                    candidate
                        .parent()
                        .ok_or_else(|| format!("{} has no parent directory", target.display()))?
                        .join(link)
                };
                // Symlink targets are still path syntax. Normalize their
                // lexical `.`/`..` components before checking existence so a
                // dangling destination has the same deterministic effective
                // target regardless of whether an intermediate component
                // happens to exist.
                candidate = lexical_normalize(&destination);
            }
            Ok(_) => {
                return std::fs::canonicalize(&candidate)
                    .map_err(|error| format!("cannot resolve {}: {error}", target.display()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return canonicalize_deepest_existing(&candidate)
                    .map_err(|error| format!("cannot resolve {}: {error}", target.display()));
            }
            Err(error) => return Err(format!("cannot inspect {}: {error}", target.display())),
        }
    }
    Err(format!(
        "cannot resolve {}: too many symlink hops",
        target.display()
    ))
}

/// Canonicalizes the deepest existing ancestor and appends the remaining
/// components. This follows symlinked ancestors without requiring the final
/// mutation target to exist yet.
fn canonicalize_deepest_existing(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path);
    }
    let Some(parent) = path.parent() else {
        return std::fs::canonicalize(path);
    };
    if parent == path {
        return std::fs::canonicalize(path);
    }
    let file_name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    Ok(canonicalize_deepest_existing(parent)?.join(file_name))
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
