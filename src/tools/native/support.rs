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

pub use super::mutation::{BoundMutationTarget, atomic_commit, bind_mutation_target};

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
