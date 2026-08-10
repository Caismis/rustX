//! Native Write tool (M5).
//!
//! Creates or replaces a file inside the workspace. The parent directory
//! must already exist — there is no implicit recursive directory creation —
//! and the write is atomic: a temporary file in the target directory is
//! written and then renamed over the target. No shell invocation is used.

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::support::{failed_result, success_json};
use crate::tools::native::{NativeToolPolicy, native_definition};
use crate::tools::types::{ToolDefinition, ToolExecutionResult, ToolInvocation};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "write";

/// The canonical business schema of the tool.
#[must_use]
pub fn definition(policy: NativeToolPolicy) -> ToolDefinition {
    native_definition(
        "tool-write",
        NAME,
        "Create or replace a file inside the workspace (parent directory must already exist).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }),
        policy,
    )
}

/// The native Write executor.
pub struct WriteTool;

impl ToolExecutor for WriteTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_write(&invocation, &context) })
    }
}

fn run_write(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let Some(object) = invocation.arguments.as_object() else {
        return failed_result("write arguments must be an object");
    };
    let Some(path_text) = object.get("path").and_then(serde_json::Value::as_str) else {
        return failed_result("write requires a string path");
    };
    let Some(file_content) = object.get("content").and_then(serde_json::Value::as_str) else {
        return failed_result("write requires a string content");
    };
    let target = match context.workspace.resolve(path_text) {
        Ok(target) => target,
        Err(error) => return failed_result(error.to_string()),
    };
    let parent = match target.parent() {
        Some(parent) if parent.exists() && parent.is_dir() => parent,
        _ => {
            return failed_result(format!(
                "the parent directory of {} does not exist; Write never creates directories \
                 implicitly",
                context.workspace.relative(&target).unwrap_or_default()
            ));
        }
    };
    let temp = match create_temp_in(parent) {
        Ok(temp) => temp,
        Err(error) => return failed_result(error),
    };
    if let Err(error) = std::fs::write(&temp, file_content.as_bytes()) {
        let _ = std::fs::remove_file(&temp);
        return failed_result(format!("cannot write {}: {error}", temp.display()));
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return failed_result(format!("cannot persist {}: {error}", target.display()));
    }
    success_json(serde_json::json!({
        "path": context.workspace.relative(&target).unwrap_or_default(),
        "bytes_written": file_content.len(),
    }))
}

/// Creates a unique temporary file inside `parent` for atomic writeback.
fn create_temp_in(parent: &std::path::Path) -> Result<std::path::PathBuf, String> {
    for attempt in 0..100u32 {
        let candidate = parent.join(format!(".rustx-write-tmp-{}-{attempt}", std::process::id()));
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
