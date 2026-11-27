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

/// The independent attestation/content generation recorded beside a logical
/// contributor identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContributorGeneration {
    /// Stable logical identity used for deterministic ordering.
    pub identity: crate::runtime::identity::ContextContributorIdentity,
    /// Optional attestation/package/content generation. It does not affect
    /// ordering.
    pub attestation: Option<String>,
}

/// One frozen context assembly generation accepted for a primary step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextGeneration {
    /// Monotonic per-attempt generation assigned by the Agent Loop.
    pub id: u64,
    /// Owners of final accepted User contributions or System sections, sorted
    /// and deduplicated by identity, with their authoritative attestations.
    pub contributors: Vec<ContributorGeneration>,
}

/// One accepted system section after lane/identity assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedSystemSection {
    /// The semantic system lane.
    pub lane: SystemSectionLane,
    /// The contributor that owns the section.
    pub contributor: crate::runtime::identity::ContextContributorIdentity,
    /// The exact section content.
    pub content: String,
}

impl ContextGeneration {
    /// Sets the Agent Loop-owned generation number.
    #[must_use]
    pub const fn with_id(mut self, id: u64) -> Self {
        self.id = id;
        self
    }
}

/// The finite request-time system-section lanes owned by rustX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemSectionLane {
    /// Core runtime/system identity. Single-owner and native-reserved.
    CoreRuntimeIdentity,
    /// Agent profile/persona. Single-owner and native-reserved.
    AgentProfile,
    /// Runtime-loaded workspace/project instructions. Single-owner and
    /// native-reserved; never canonical conversation history.
    WorkspaceInstructions,
    /// Certified-extension sections, ordered by logical contributor identity.
    CertifiedExtension,
    /// Native capability guidance sections, including the request-time Skill
    /// catalog, rather than conversational User context facts.
    NativeCapabilityGuidance,
    /// Permission-mode classification in immutable request snapshots.
    PermissionReminder,
}

impl SystemSectionLane {
    /// The contract's deterministic total order.
    pub const ALL: [Self; 5] = [
        Self::CoreRuntimeIdentity,
        Self::AgentProfile,
        Self::WorkspaceInstructions,
        Self::CertifiedExtension,
        Self::NativeCapabilityGuidance,
    ];

    /// Stable manifest spelling of one system-section lane.
    #[must_use]
    pub const fn manifest_name(self) -> &'static str {
        match self {
            Self::CoreRuntimeIdentity => "core_runtime_identity",
            Self::AgentProfile => "agent_profile",
            Self::WorkspaceInstructions => "workspace_instructions",
            Self::CertifiedExtension => "certified_extension",
            Self::NativeCapabilityGuidance => "native_capability_guidance",
            Self::PermissionReminder => "permission_reminder",
        }
    }
}

/// Canonical native wrapper shared by admission and durable retry validation.
pub fn permission_reminder_content(text: &str) -> Vec<crate::message::types::UserContentBlock> {
    vec![crate::message::types::UserContentBlock::Text(
        crate::message::content::TextBlock {
            text: format!("<system-reminder>\n{text}\n</system-reminder>"),
        },
    )]
}
