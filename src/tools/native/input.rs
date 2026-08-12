//! The typed input boundary of the native tool plane.
//!
//! Model-issued arguments reach a native executor only through this
//! boundary:
//!
//! ```text
//! model JSON arguments
//!         |
//!         v
//! canonical schema validation (registry preflight)
//!         |
//!         v
//! typed input deserialization        <- this module
//!         |
//!         v
//! tool-specific semantic validation  <- the tool's own input module
//!         |
//!         v
//! execution
//! ```
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
