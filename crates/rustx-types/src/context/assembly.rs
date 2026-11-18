//! User-context semantic lanes.
use serde::{Deserialize, Serialize};

/// The finite user-context semantic lanes owned by rustX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserContextLane {
    /// Claimed inbound input. This is not extension-controlled.
    ClaimedInbound,
    /// The semantic lane of the **native runtime observation owner**
    /// (`NativeContextContributor::RuntimeToolObservation` in `rustx`).
    ///
    /// This lane describes *who owns the fact*, not *when the fact became
    /// eligible*. Deferred proposals produced by a certified extension keep
    /// that extension's own semantics and land in
    /// [`UserContextLane::ExtensionEnvironment`]; only the native runtime
    /// observation owner's facts belong here. The lane is native-reserved:
    /// no extension can claim it.
    ///
    /// The lane sits immediately after claimed inbound because a native
    /// runtime observation describes what the environment just did for the
    /// tool batch that precedes this step, while the request-time
    /// environment and task-data lanes describe the
    /// *current* step.
    RuntimeToolObservation,
    /// Generic certified-extension/environment context.
    ExtensionEnvironment,
    /// Compiled-in domain task data, never system instructions.
    TaskData,
}

impl UserContextLane {
    /// The contract's deterministic total order.
    pub const ALL: [Self; 4] = [
        Self::ClaimedInbound,
        Self::RuntimeToolObservation,
        Self::ExtensionEnvironment,
        Self::TaskData,
    ];

    /// Stable manifest spelling of one user-context lane.
    #[must_use]
    pub const fn manifest_name(self) -> &'static str {
        match self {
            Self::TaskData => "task_data",
            Self::ClaimedInbound => "claimed_inbound",
            Self::RuntimeToolObservation => "runtime_tool_observation",
            Self::ExtensionEnvironment => "extension_environment",
        }
    }
}
