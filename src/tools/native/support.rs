//! Shared deterministic helpers of the native tool implementations.

use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

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
    }
}

/// A normalized successful structured result.
#[must_use]
pub fn success_json(value: serde_json::Value) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json { value }],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}

/// A normalized successful structured result with truncation metadata and
/// artifact references.
#[must_use]
pub fn success_json_with(
    value: serde_json::Value,
    truncation: Option<crate::tools::types::TruncationState>,
    artifacts: Vec<crate::message::content::FileReference>,
) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json { value }],
        duration_ms: 0,
        exit_code: None,
        artifacts,
        truncation,
    }
}
