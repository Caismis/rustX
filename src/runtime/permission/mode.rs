//! The closed set of built-in execution-permission modes.

use serde::{Deserialize, Serialize};

/// The runtime-wide execution-permission mode.
///
/// Modes are built into Rust and are not extensible through TOML presets.
/// Changing mode never changes Tool Registry membership, model-facing names,
/// tool Schema, MCP discovery, or capability selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ExecutionPermissionMode {
    /// Native reads and approved bookkeeping are allowed; Write/Edit may
    /// automatically change only workspace-root `PLAN.md`; Bash requires a
    /// proven read-only classification. Other defaults refuse without a
    /// nonblank `elevation_reason`.
    #[default]
    Plan,
    /// Retain Plan's deterministic fast paths. Remaining defaults go through
    /// the shared Auxiliary permission reviewer. Denied calls may escalate
    /// with `elevation_reason`.
    Automatic,
    /// Default calls are allowed without Auxiliary or ordinary human review.
    /// Effective explicit `deny` and `ask` resource settings still apply.
    FullAccess,
}

impl ExecutionPermissionMode {
    /// Stable `snake_case` spelling used in TOML, protocol, and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Automatic => "automatic",
            Self::FullAccess => "full_access",
        }
    }
}

impl std::fmt::Display for ExecutionPermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_through_json() {
        for mode in [
            ExecutionPermissionMode::Plan,
            ExecutionPermissionMode::Automatic,
            ExecutionPermissionMode::FullAccess,
        ] {
            let encoded = serde_json::to_value(mode).expect("encode");
            assert_eq!(encoded, serde_json::Value::String(mode.as_str().into()));
            let decoded: ExecutionPermissionMode = serde_json::from_value(encoded).expect("decode");
            assert_eq!(decoded, mode);
        }
    }

    #[test]
    fn default_mode_is_plan() {
        assert_eq!(
            ExecutionPermissionMode::default(),
            ExecutionPermissionMode::Plan
        );
    }
}
