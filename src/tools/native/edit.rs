//! Native Edit tool (M5).
//!
//! Applies an exact text replacement to a UTF-8 file inside the workspace.
//! The default `replace_all = false` requires exactly one exact match; zero
//! matches fails, and more than one match with `replace_all = false` fails
//! explicitly. `replace_all = true` replaces every exact match but still
//! fails on zero matches. The writeback is atomic (temporary file +
//! rename). No fuzzy/LLM edit matching exists.

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::native_definition;
use crate::tools::native::support::{failed_result, success_json};
use crate::tools::types::{ToolDefinition, ToolExecutionResult, ToolInvocation};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "edit";

/// The canonical business schema of the tool.
#[must_use]
pub fn definition() -> ToolDefinition {
    native_definition(
        "tool-edit",
        NAME,
        "Replace exact text in a UTF-8 file inside the workspace (atomic writeback).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_text": {"type": "string"},
                "new_text": {"type": "string"},
                "replace_all": {"type": "boolean", "default": false}
            },
            "required": ["path", "old_text", "new_text"],
            "additionalProperties": false
        }),
    )
}

/// The native Edit executor.
pub struct EditTool;

impl ToolExecutor for EditTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_edit(&invocation, &context) })
    }
}

fn run_edit(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let Some(object) = invocation.arguments.as_object() else {
        return failed_result("edit arguments must be an object");
    };
    let Some(path) = object.get("path").and_then(serde_json::Value::as_str) else {
        return failed_result("edit requires a string path");
    };
    let Some(old_text) = object.get("old_text").and_then(serde_json::Value::as_str) else {
        return failed_result("edit requires a string old_text");
    };
    let Some(new_text) = object.get("new_text").and_then(serde_json::Value::as_str) else {
        return failed_result("edit requires a string new_text");
    };
    let replace_all = object
        .get("replace_all")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if old_text.is_empty() {
        return failed_result("edit requires a non-empty old_text");
    }
    let target = match context.workspace.resolve(path) {
        Ok(target) => target,
        Err(error) => return failed_result(error.to_string()),
    };
    let bytes = match std::fs::read(&target) {
        Ok(bytes) => bytes,
        Err(error) => {
            return failed_result(format!("cannot read {}: {error}", target.display()));
        }
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return failed_result(format!(
            "{} is not a UTF-8 text file; Edit never operates on binary content",
            context.workspace.relative(&target).unwrap_or_default()
        ));
    };
    let match_count = text.matches(old_text).count();
    if match_count == 0 {
        return failed_result(format!(
            "old_text not found in {}",
            context.workspace.relative(&target).unwrap_or_default()
        ));
    }
    if !replace_all && match_count > 1 {
        return failed_result(format!(
            "old_text occurs {match_count} times in {}; replace_all = false requires exactly \
             one exact match",
            context.workspace.relative(&target).unwrap_or_default()
        ));
    }
    let replaced = if replace_all {
        text.replace(old_text, new_text)
    } else {
        text.replacen(old_text, new_text, 1)
    };
    if let Err(error) = atomic_writeback(&target, replaced.as_bytes()) {
        return failed_result(error);
    }
    success_json(serde_json::json!({
        "path": context.workspace.relative(&target).unwrap_or_default(),
        "replacements": match_count,
    }))
}

/// Atomic writeback: a temporary file in the target directory is written
/// and renamed over the target.
fn atomic_writeback(target: &std::path::Path, content: &[u8]) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", target.display()))?;
    let temp = parent.join(format!(".rustx-edit-tmp-{}", std::process::id()));
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
