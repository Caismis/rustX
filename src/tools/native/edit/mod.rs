//! Native Edit tool (M5).
//!
//! Applies an exact text replacement to a UTF-8 file inside the workspace.
//! The default `replace_all = false` requires exactly one exact match; zero
//! matches fails, and more than one match with `replace_all = false` fails
//! explicitly. `replace_all = true` replaces every exact match but still
//! fails on zero matches. The writeback is atomic (temporary file +
//! rename). No fuzzy/LLM edit matching exists.
//!
//! The model-facing argument contract is the typed [`EditInput`]; the
//! canonical schema is generated from it.

mod input;

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{failed_result, success_json};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation};

use input::EditInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "edit";

/// The tool-owned registration of the native Edit tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<EditInput>(
            "tool-edit",
            NAME,
            "Replace exact text in a UTF-8 file inside the workspace (atomic writeback).",
            policy,
        ),
        std::sync::Arc::new(EditTool),
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
    let input = match EditInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let (old_text, new_text, replace_all) = (
        input.old_text.as_str(),
        input.new_text.as_str(),
        input.replace_all,
    );
    let target = match context.workspace.resolve(&input.path) {
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
