//! Tool invocation fact vocabulary shared across layers.

use crate::tools::deadline::{ToolCancellationCause, ToolDeadlineKind, ToolSettlementCertainty};
use crate::tools::types::{ToolExecutionStatus, ToolProgress};

/// Facts selected by the invocation owner, independently of caller framing.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InvocationFact {
    Deadline { kind: ToolDeadlineKind },
    CancellationRequested { cause: ToolCancellationCause },
    SettlementObserved { certainty: ToolSettlementCertainty },
    SettlementControlFailed { reason: String },
}

/// Native execution facts for callers without canonical `ToolCall` framing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeInvocationFact {
    Prepared {
        tool_name: String,
        arguments_digest: String,
    },
    Started,
    Progress {
        progress: ToolProgress,
    },
    Lifecycle {
        fact: InvocationFact,
    },
    Completed {
        status: ToolExecutionStatus,
    },
}
