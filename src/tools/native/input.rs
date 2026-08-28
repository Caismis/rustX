//! The typed input boundary of the native tool plane.
//!
//! The canonical executor ABI is unchanged: a [`ToolExecutor`] receives a
//! [`ToolInvocation`] whose `arguments` are canonical JSON, already
//! validated by the registry against the tool's generated schema. A native
//! executor immediately decodes those validated arguments into its
//! tool-owned typed input through this boundary, before any tool-specific
//! filesystem, process, or other business work begins:
//!
//! ```text
//! model JSON arguments
//!         |
//!         v
//! canonical schema validation (registry preflight)
//!         |
//!         v
//! validated ToolInvocation           <- the canonical executor ABI
//!         |
//!         v
//! typed input deserialization        <- this module
//!         |
//!         v
//! tool-specific semantic validation  <- the tool's own input module
//!         |
//!         v
//! actual tool work
//! ```
//!
//! [`ToolExecutor`]: crate::tools::executor::ToolExecutor
//! [`ToolInvocation`]: crate::tools::types::ToolInvocation
//!
//! The typed input is the model-facing *input contract*: required fields,
//! type correctness, and schema constraints belong to it. Workspace
//! permission, filesystem existence, and process lifecycle rules remain
//! tool execution concerns and are never expressed as deserialization
//! rules.
//!
//! A rejected input is a normal failed tool result, exactly like any other
//! business-level tool failure: it is never an attempt-level runtime
//! failure, and the executor's real work never starts.

use serde::de::DeserializeOwned;

/// Deserializes model-issued arguments into a tool's typed input contract.
///
/// # Errors
///
/// Returns the deterministic rejection message of the first contract
/// violation (unknown field, missing required field, or wrong JSON type).
pub(crate) fn decode<I: DeserializeOwned>(
    tool: &str,
    arguments: &serde_json::Value,
) -> Result<I, String> {
    serde_json::from_value(arguments.clone())
        .map_err(|error| format!("invalid {tool} arguments: {error}"))
}

/// Decodes only the `path` field of one native file tool's recorded
/// arguments.
///
/// Compaction file-operation metadata (Issue #140) extracts the path a
/// historical canonical tool call named without re-running the full input
/// contract: the call may predate a current validation rule or have been
/// rejected by it, yet the path remains a conversation fact. Each native file
/// tool owns its own wrapper around this shared field decode, so the answer
/// to "which argument names the file" stays in the tool module that owns the
/// argument contract.
pub(crate) fn file_path_argument(arguments: &serde_json::Value) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct PathOnly {
        path: String,
    }
    serde_json::from_value::<PathOnly>(arguments.clone())
        .ok()
        .map(|decoded| decoded.path)
}
