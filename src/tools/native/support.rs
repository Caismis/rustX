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
/// paths are interpreted verbatim; containment against the admitted Client
/// workspace is applied by the Execution Client, never locally.
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

pub use super::mutation::{BoundMutationTarget, PreparedRemoteMutation};

/// Binds the Write/Edit mutation claim for one prepared invocation, or
/// returns `None` for non-mutation tools and for arguments the tool's
/// own parser will reject — the parser's canonical wording is
/// authoritative.
///
/// For a builtin `write`/`edit` the binder resolves the logical path
/// with [`interpret_path`], applies the lexical managed-output guard,
/// and sends the full planned operation — the write payload or the edit
/// replacements — to the Session's admitted Client as a `Current`
/// expectation prepare *before* authorization: the Client retains the
/// target and the bounded payload in memory, nothing is staged or
/// published, and the later commit or abort consumes the ticket exactly
/// once. A prepare failure is deferred into the bound target so the
/// tool renders it with its own stable wording.
///
/// # Errors
///
/// Returns `Err` only for a missing `path` argument or a managed-output
/// guard denial — the failures the caller reports verbatim.
pub async fn bind_invocation_mutation_target(
    origin: &crate::tools::types::ToolOrigin,
    tool_name: &str,
    arguments: &serde_json::Value,
    workspace: &crate::tools::workspace::Workspace,
    tool_output: &ManagedToolOutput,
    cancellation: &crate::runtime::cancellation::ExecutionCancellation,
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
    let interpreted = interpret_path(workspace.root(), requested);
    tool_output
        .ensure_model_mutation_allowed(&interpreted)
        .map_err(|error| error.to_string())?;
    // No admitted Client means no mutation claim exists at all — the
    // failure short-circuits here, never a target bound on nothing.
    let Some(scope) = workspace.execution_scope().cloned() else {
        return Err(format!(
            "{} {requested}: Session execution unavailable: no admitted Execution Client",
            verb(tool_name),
        ));
    };
    let prepared = match tool_name {
        "write" => {
            let Some(content) = arguments.get("content").and_then(serde_json::Value::as_str) else {
                // The tool's parser owns the canonical argument
                // error — prepare nothing.
                return Ok(None);
            };
            // Bounded before the payload is copied: the wire cap is the
            // same constant the Server would refuse at.
            if content.len() > rustx_execution_protocol::MAX_WRITE_BYTES {
                return Err(format!(
                    "cannot persist {requested}: file write exceeds the write cap"
                ));
            }
            scope
                .prepare_write(&interpreted, content.as_bytes().to_vec(), cancellation)
                .await
                .map_err(|error| format!("cannot persist {requested}: {error}"))
        }
        "edit" => {
            // Strict shape: every element must carry both texts — a
            // partial or malformed array prepares nothing and the tool's
            // parser produces the canonical rejection.
            let Some(edits) = arguments.get("edits").and_then(serde_json::Value::as_array) else {
                return Ok(None);
            };
            // The request caps gate the copy itself: an over-cap array
            // is refused before more old/new texts are cloned, not after.
            if edits.is_empty() || edits.len() > rustx_execution_protocol::MAX_EDIT_REPLACEMENTS {
                return Err(format!(
                    "cannot read {requested}: edit request outside the bounded replacement policy"
                ));
            }
            let mut request_bytes = 0_usize;
            let mut replacements = Vec::with_capacity(edits.len());
            for edit in edits {
                let (Some(old), Some(new)) = (
                    edit.get("oldText").and_then(serde_json::Value::as_str),
                    edit.get("newText").and_then(serde_json::Value::as_str),
                ) else {
                    return Ok(None);
                };
                request_bytes = request_bytes
                    .saturating_add(old.len())
                    .saturating_add(new.len());
                if request_bytes > rustx_execution_protocol::MAX_EDIT_REQUEST_BYTES {
                    return Err(format!(
                        "cannot read {requested}: edit request outside the bounded replacement policy"
                    ));
                }
                replacements.push(rustx_execution_protocol::TextReplacement {
                    old_text: old.to_owned(),
                    new_text: new.to_owned(),
                });
            }
            scope
                .prepare_edit(&interpreted, replacements, cancellation)
                .await
                .map_err(|error| edit_prepare_error(&error, requested, edits.len()))
        }
        _ => unreachable!("tool name gated above"),
    }?;
    Ok(Some(BoundMutationTarget::ready(
        interpreted,
        std::sync::Arc::new(PreparedRemoteMutation::new(scope, prepared)),
    )))
}

/// The historical `cannot persist`/`cannot read` opener each tool uses.
fn verb(tool_name: &str) -> &'static str {
    if tool_name == "edit" {
        "cannot read"
    } else {
        "cannot persist"
    }
}

/// Renders one edit prepare failure with the tool's historical
/// diagnostics: a `RefusedEdit`-downcastable error renders through the
/// edit kernel's exact wording — denied edits never expose source
/// content — and every other failure keeps the `cannot read` prefix.
fn edit_prepare_error(error: &std::io::Error, requested: &str, total: usize) -> String {
    if let Some(refused) = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustx_execution_server::RefusedEdit>())
    {
        return crate::tools::native::edit::edit_failure(requested, total, refused.0);
    }
    format!("cannot read {requested}: {error}")
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
    use crate::tools::workspace::Workspace;

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        (dir, workspace)
    }

    fn cancellation() -> crate::runtime::cancellation::ExecutionCancellation {
        crate::runtime::cancellation::ExecutionCancellation::detached(
            crate::runtime::cancellation::CancellationSignal::new(),
            crate::runtime::types::CancellationReason::UserRequested,
        )
    }

    #[tokio::test]
    async fn external_write_and_edit_names_never_bind_mutation_targets() {
        use crate::tools::types::ToolOrigin;
        let (dir, workspace) = workspace();
        let output = ManagedToolOutput::new(
            crate::runtime::identity::ConversationId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
            ),
            dir.path().join("tool-output"),
        )
        .unwrap();
        let cancellation = cancellation();
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
                        &workspace,
                        &output,
                        &cancellation,
                    )
                    .await
                    .unwrap()
                    .is_none()
                );
                assert!(
                    bind_invocation_mutation_target(
                        &origin,
                        name,
                        &serde_json::json!({"path": output.root().unwrap()}),
                        &workspace,
                        &output,
                        &cancellation,
                    )
                    .await
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
                &workspace,
                &output,
                &cancellation,
            )
            .await
            .is_err()
        );
    }
}
