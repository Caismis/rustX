//! Native file-operation facts decoded from recorded tool calls.
//!
//! Classification is by canonical [`ToolCall::tool_id`], never by the
//! model-facing name — the argument-schema contract shared by the tools that
//! execute and the compaction metadata that aggregates the facts.

use crate::tools::types::ToolCall;

/// The canonical tool id of the native `read` tool.
pub const READ_TOOL_ID: &str = "tool-read";
/// The canonical tool id of the native `edit` tool.
pub const EDIT_TOOL_ID: &str = "tool-edit";
/// The canonical tool id of the native `write` tool.
pub const WRITE_TOOL_ID: &str = "tool-write";

/// cancelled does not rewrite what the retired conversation asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeFileOperation {
    /// A native `read(path)` call.
    Read {
        /// The path argument of the call.
        path: String,
    },
    /// A native `edit(path)` or `write(path)` call.
    Modified {
        /// The path argument of the call.
        path: String,
    },
}

/// Classifies one canonical tool call as a native file operation, decoding
/// the path through the owning tool module.
///
/// Returns `None` for any non-file native tool, any non-native tool, and any
/// native file call whose recorded arguments do not identify a path.
pub fn native_file_operation(call: &ToolCall) -> Option<NativeFileOperation> {
    let tool_id = call.tool_id.as_str();
    if tool_id == READ_TOOL_ID {
        read_operation_path(&call.arguments).map(|path| NativeFileOperation::Read { path })
    } else if tool_id == EDIT_TOOL_ID {
        edit_operation_path(&call.arguments).map(|path| NativeFileOperation::Modified { path })
    } else if tool_id == WRITE_TOOL_ID {
        write_operation_path(&call.arguments).map(|path| NativeFileOperation::Modified { path })
    } else {
        None
    }
}
/// argument contract.
pub fn file_path_argument(arguments: &serde_json::Value) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct PathOnly {
        path: String,
    }
    serde_json::from_value::<PathOnly>(arguments.clone())
        .ok()
        .map(|decoded| decoded.path)
}
/// The path argument of the native `read` tool's recorded arguments.
pub fn read_operation_path(arguments: &serde_json::Value) -> Option<String> {
    file_path_argument(arguments)
}
/// The path argument of the native `edit` tool's recorded arguments.
pub fn edit_operation_path(arguments: &serde_json::Value) -> Option<String> {
    file_path_argument(arguments)
}
/// The path argument of the native `write` tool's recorded arguments.
pub fn write_operation_path(arguments: &serde_json::Value) -> Option<String> {
    file_path_argument(arguments)
}
