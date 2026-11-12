//! Shared deterministic helpers of the native tool implementations.

use std::path::{Path, PathBuf};

use crate::runtime::types::CancellationReason;
use crate::tools::managed_output::ManagedToolOutput;
use crate::tools::types::{
    ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
};

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

/// The native mutation target prepared once and bound to an authorized
/// Write/Edit invocation.
///
/// Classification, execute, and commit all use this same held target. If the
/// filesystem facts behind the interpreted path change after binding, the
/// old authorization cannot continue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundMutationTarget {
    /// Absolute, lexically normalized request path before symlink following.
    interpreted: PathBuf,
    /// Effective host path authorized for mutation.
    effective: PathBuf,
}

impl BoundMutationTarget {
    /// The effective absolute host path authorized for mutation.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.effective
    }

    /// Re-resolves the interpreted path and refuses when the effective target
    /// or managed-output ownership no longer matches the bound authorization.
    pub fn ensure_still_authorized(&self, tool_output: &ManagedToolOutput) -> Result<(), String> {
        let current = resolve_effective_path(&self.interpreted)?;
        if current != self.effective {
            return Err(format!(
                "mutation target changed since authorization (authorized {}, now {})",
                self.effective.display(),
                current.display()
            ));
        }
        tool_output
            .ensure_model_mutation_allowed(&self.effective)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// The commit handle for the still-authorized effective path.
    #[must_use]
    pub fn commit_target(&self) -> MutationTarget {
        MutationTarget {
            path: self.effective.clone(),
        }
    }
}

/// Resolves and authorizes one native mutation target before side effects.
///
/// This is deliberately not a workspace sandbox. `ManagedToolOutput` owns
/// the only model-mutation exception: its runtime-owned namespace is
/// read-only even though Read/Grep/Glob can inspect it.
/// `interpreted` is the already absolute, lexically normalized result of
/// [`interpret_path`].
pub fn bind_mutation_target(
    interpreted: &Path,
    tool_output: &ManagedToolOutput,
) -> Result<BoundMutationTarget, String> {
    let effective = resolve_effective_path(interpreted)?;
    tool_output
        .ensure_model_mutation_allowed(&effective)
        .map_err(|error| error.to_string())?;
    Ok(BoundMutationTarget {
        interpreted: interpreted.to_path_buf(),
        effective,
    })
}

/// Binds the Write/Edit mutation target for one prepared invocation, or
/// returns `None` for non-mutation tools.
pub fn bind_invocation_mutation_target(
    origin: &crate::tools::types::ToolOrigin,
    tool_name: &str,
    arguments: &serde_json::Value,
    workspace_root: &Path,
    tool_output: &ManagedToolOutput,
) -> Result<Option<BoundMutationTarget>, String> {
    if !matches!(origin, crate::tools::types::ToolOrigin::Builtin)
        || !matches!(tool_name, "write" | "edit")
    {
        return Ok(None);
    }
    let requested = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "write/edit invocation is missing a path".to_owned())?;
    let interpreted = interpret_path(workspace_root, requested);
    Ok(Some(bind_mutation_target(&interpreted, tool_output)?))
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
/// The caller must pass the still-authorized target produced from a
/// [`BoundMutationTarget`] so the path that passed ownership checks is the
/// path committed here; ordinary write-through semantics are retained
/// without replacing a link.
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
///
/// Permission classification for Write/Edit must use this same resolver so a
/// dangling `PLAN.md` symlink cannot authorize a different host target than
/// the mutation commit will write.
pub(crate) fn resolve_effective_path(target: &Path) -> Result<PathBuf, String> {
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
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Failed {
            error: error.into(),
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

/// A normalized cancelled tool result for an executor that has already been
/// invoked. The Agent Loop remains authoritative and normalizes the final
/// result, but this direct executor result cannot claim `BeforeStart`.
#[must_use]
pub fn cancelled_result(reason: CancellationReason) -> ToolExecutionResult {
    ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Cancelled {
            reason,
            phase: ToolCancellationPhase::DuringExecution,
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

/// A normalized successful structured result used by tools whose contract is
/// intentionally JSON (for example the subagent intrinsic).
#[must_use]
pub fn success_json(value: serde_json::Value) -> ToolExecutionResult {
    ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json { value }],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
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
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Text(
            crate::message::content::TextBlock { text: text.into() },
        )],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation,
        workflow: None,
        managed_output: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::managed_output::ManagedToolOutput;

    #[test]
    fn external_write_and_edit_names_never_bind_local_mutation_targets() {
        use crate::tools::types::ToolOrigin;
        let workspace = tempfile::tempdir().unwrap();
        let output = ManagedToolOutput::new(
            crate::runtime::identity::ConversationId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
            ),
            workspace.path().join("tool-output"),
        )
        .unwrap();
        for origin in [
            ToolOrigin::Mcp {
                server_id: crate::runtime::identity::McpServerId::new("remote"),
            },
            ToolOrigin::ManagedPython {
                package: "remote".into(),
            },
            ToolOrigin::Host {
                namespace: None,
                name: "remote".into(),
            },
        ] {
            for name in ["write", "edit"] {
                assert!(
                    bind_invocation_mutation_target(
                        &origin,
                        name,
                        &serde_json::json!({}),
                        workspace.path(),
                        &output
                    )
                    .unwrap()
                    .is_none()
                );
                assert!(
                    bind_invocation_mutation_target(
                        &origin,
                        name,
                        &serde_json::json!({"path": output.root().unwrap()}),
                        workspace.path(),
                        &output
                    )
                    .unwrap()
                    .is_none()
                );
            }
        }
        assert!(
            bind_invocation_mutation_target(
                &ToolOrigin::Builtin,
                "write",
                &serde_json::json!({}),
                workspace.path(),
                &output
            )
            .is_err()
        );
    }

    #[test]
    fn bound_target_refuses_when_plan_file_is_replaced_with_external_symlink() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let plan = workspace.path().join("PLAN.md");
        std::fs::write(&plan, "plan").expect("seed plan");
        let tool_output = ManagedToolOutput::new(
            crate::runtime::identity::ConversationId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
            ),
            workspace.path().join("tool-output"),
        )
        .expect("managed tool output");
        let interpreted = interpret_path(workspace.path(), "PLAN.md");
        let bound = bind_mutation_target(&interpreted, &tool_output).expect("bind");
        assert_eq!(bound.path(), &plan.canonicalize().expect("canonical plan"));

        std::fs::remove_file(&plan).expect("remove plan");
        let dangling = outside.path().join("secret.txt");
        std::os::unix::fs::symlink(&dangling, &plan).expect("replace with symlink");

        let error = bound
            .ensure_still_authorized(&tool_output)
            .expect_err("symlink replacement invalidates authorization");
        assert!(
            error.contains("mutation target changed since authorization"),
            "{error}"
        );
        assert!(!dangling.exists(), "outside target must stay untouched");
    }
}
