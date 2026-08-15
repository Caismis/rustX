//! Strongly typed runtime-owned identifiers.
//!
//! Every identifier is a transparent string-backed newtype. The types are
//! distinct so that unrelated identifier domains cannot be mixed
//! accidentally, and they serialize deterministically as plain JSON strings
//! for persistence. Identifiers are externally assigned in M1; this module
//! does not generate identifiers.

use serde::{Deserialize, Serialize};

/// Defines a transparent string-backed identifier type with standard traits.
macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a new identifier from a string value.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns the identifier value as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the identifier and returns its string value.
            #[must_use]
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

id_type! {
    /// Identifies a durable conversation.
    ConversationId
}

id_type! {
    /// Identifies a committed canonical message block.
    MessageId
}

id_type! {
    /// Identifies an agent.
    AgentId
}

id_type! {
    /// Identifies an immutable agent version.
    AgentVersionId
}

id_type! {
    /// Identifies one attempt to execute an agent manifest.
    AttemptId
}

id_type! {
    /// Identifies one turn within an attempt.
    TurnId
}

id_type! {
    /// Identifies one durable runtime event.
    EventId
}

id_type! {
    /// Identifies a tool definition in the capability set.
    ToolId
}

id_type! {
    /// Identifies one tool call issued by the current agent.
    ToolCallId
}

id_type! {
    /// Identifies one detached runtime execution instance of a background
    /// tool.
    ///
    /// `ToolExecutionId` is distinct from `ToolCallId`: a `ToolCallId`
    /// identifies the logical model-issued call, while a `ToolExecutionId`
    /// identifies the runtime execution instance and may outlive the
    /// attempt that created it. Allocation is conversation-owned and
    /// monotonic (`exec_1`, `exec_2`, ...); cross-conversation uniqueness is
    /// not required because the background registry is conversation-scoped.
    ToolExecutionId
}

id_type! {
    /// Identifies an immutable version of a custom Python tool.
    ToolVersionId
}

id_type! {
    /// Identifies an MCP server bound to the runtime.
    McpServerId
}

id_type! {
    /// Identifies a skill bound to the runtime.
    ///
    /// M6 makes the standard Agent Skills `name` the logical skill identity:
    /// a `SkillId` is the validated standard skill name, not an externally
    /// assigned opaque string.
    SkillId
}

id_type! {
    /// Identifies an immutable skill version.
    ///
    /// M6 derives `SkillVersionId` deterministically from the complete
    /// accepted Skill package content (SHA-256 over the stable textual form
    /// `sha256:<64 lowercase hex characters>`). Any package-content change
    /// yields a new version id.
    SkillVersionId
}

id_type! {
    /// Identifies the immutable shared Python environment of one active
    /// Skill capability set.
    ///
    /// M6 derives `PythonEnvironmentDigest` deterministically from the
    /// environment-relevant inputs: format/version domain, OS, architecture,
    /// resolved Python runtime identity, resolved pip identity, and the
    /// sorted normalized direct dependency map. It is distinct from
    /// [`SkillVersionId`]: a description-only Skill change can produce a new
    /// Skill version without changing the Python environment identity.
    PythonEnvironmentDigest
}

id_type! {
    /// Identifies an immutable environment owned by custom Python tools.
    ///
    /// This identity is intentionally separate from the M6 shared Skill
    /// environment identity even when both happen to contain Python.
    PythonToolEnvironmentDigest
}

id_type! {
    /// Identifies the immutable shared Node environment of one active Skill
    /// capability set.
    ///
    /// M6 derives `NodeEnvironmentDigest` deterministically from the
    /// environment-relevant inputs: format/version domain, OS, architecture,
    /// resolved Node runtime identity, resolved npm identity, and the sorted
    /// normalized direct dependency map. It is distinct from
    /// [`SkillVersionId`]: a description-only Skill change can produce a new
    /// Skill version without changing the Node environment identity.
    NodeEnvironmentDigest
}

id_type! {
    /// Identifies a durable artifact produced or referenced by the runtime.
    ///
    /// An artifact is identified by an opaque runtime-owned id, never by a
    /// local filesystem path: paths are executor concerns and are not a
    /// universal durable artifact identity.
    ArtifactId
}

/// The rustX-owned logical identity of a context contributor.
///
/// The identity is independent from registration order, process-local object
/// identity, and package/content generation.  The Context Assembly boundary
/// derives trusted native provenance itself; contributors never provide it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ContextContributorIdentity {
    /// A rustX-native semantic owner.
    Native(NativeContextContributor),
    /// A certified extension with a stable logical key.
    CertifiedExtension(CertifiedExtensionIdentity),
}

/// Native semantic owners that may publish model-visible context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeContextContributor {
    /// Workspace/project instructions.
    WorkspaceInstructions,
    /// The native capability/Skill guidance owner.
    SkillGuidance,
    /// The native runtime/Agent Status owner.
    AgentStatus,
    /// The core runtime/system identity owner.
    CoreSystemIdentity,
    /// The native agent profile/persona owner.
    AgentProfile,
}

impl NativeContextContributor {
    /// Every native semantic owner, in contract order. This is the source
    /// used by the compatibility manifest and reserved-identity validation;
    /// callers must not maintain a second list of native slots.
    pub const ALL: [Self; 5] = [
        Self::WorkspaceInstructions,
        Self::SkillGuidance,
        Self::AgentStatus,
        Self::CoreSystemIdentity,
        Self::AgentProfile,
    ];

    /// The canonical extension-key spelling reserved for this native owner.
    #[must_use]
    pub const fn logical_key(self) -> &'static str {
        match self {
            Self::WorkspaceInstructions => "workspace-instructions",
            Self::SkillGuidance => "skill-guidance",
            Self::AgentStatus => "agent-status",
            Self::CoreSystemIdentity => "core-runtime-identity",
            Self::AgentProfile => "agent-profile",
        }
    }

    /// The machine-readable manifest spelling of this native slot.
    #[must_use]
    pub const fn manifest_name(self) -> &'static str {
        match self {
            Self::WorkspaceInstructions => "workspace_instructions",
            Self::SkillGuidance => "skill_guidance",
            Self::AgentStatus => "agent_status",
            Self::CoreSystemIdentity => "core_runtime_identity",
            Self::AgentProfile => "agent_profile",
        }
    }
}

/// The stable logical key of one certified extension.
///
/// Package/content attestation is intentionally not part of this type.  A
/// package may be upgraded while preserving its logical ordering identity;
/// the assembly generation records the attestation separately.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CertifiedExtensionIdentity(String);

impl CertifiedExtensionIdentity {
    /// Validates and canonicalizes a configured logical extension key.
    ///
    /// # Errors
    ///
    /// Returns a validation message when the key is empty, too long, or uses
    /// a character or boundary that is not part of the stable identity
    /// grammar.
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into().trim().to_ascii_lowercase();
        if value.is_empty() {
            return Err("extension logical identity must not be empty".to_owned());
        }
        if value.len() > 128 {
            return Err("extension logical identity must be at most 128 bytes".to_owned());
        }
        if !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._/-".contains(&byte)
        }) {
            return Err(format!(
                "extension logical identity {value:?} contains an unsupported character"
            ));
        }
        if value.starts_with('.') || value.ends_with('.') || value.contains("..") {
            return Err(format!(
                "extension logical identity {value:?} has an invalid dot boundary"
            ));
        }
        Ok(Self(value))
    }

    /// The canonical logical key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for CertifiedExtensionIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A monotonic revision counter for the capability set observed by an attempt.
///
/// A running attempt snapshots one immutable `CapabilityRevision` when it
/// starts and keeps it for its entire lifetime. The revision is a counter,
/// not a provider-specific string: every capability mutation atomically swaps
/// the whole capability set and increments the revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityRevision(u64);

impl CapabilityRevision {
    /// Creates a revision from a raw counter value.
    #[must_use]
    pub const fn new(revision: u64) -> Self {
        Self(revision)
    }

    /// Returns the raw counter value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Default for CapabilityRevision {
    /// The zero revision, meaning "no capabilities have been established".
    fn default() -> Self {
        Self(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentId, AgentVersionId, ArtifactId, AttemptId, CapabilityRevision, ConversationId,
        EventId, McpServerId, MessageId, NodeEnvironmentDigest, PythonEnvironmentDigest,
        PythonToolEnvironmentDigest, SkillId, SkillVersionId, ToolCallId, ToolExecutionId, ToolId,
        ToolVersionId, TurnId,
    };

    /// Strong identifiers serialize as plain strings, not as structs.
    #[test]
    fn strong_ids_serialize_as_plain_strings() {
        let id = ConversationId::new("conv-1");
        let json = serde_json::to_string(&id).expect("serialize conversation id");
        assert_eq!(json, "\"conv-1\"");
    }

    /// Every strong identifier type round-trips against its own type; IDs
    /// from different domains must never deserialize interchangeably.
    #[test]
    fn strong_ids_round_trip_against_their_own_type() {
        fn round_trip<T>(value: &T) -> T
        where
            T: Clone + PartialEq + std::fmt::Debug + serde::Serialize + serde::de::DeserializeOwned,
        {
            let json = serde_json::to_string(value).expect("serialize id");
            let decoded: T = serde_json::from_str(&json).expect("deserialize id");
            assert_eq!(&decoded, value, "id must round-trip as its own type");
            decoded
        }

        let _ = round_trip(&ConversationId::new("conv-1"));
        let _ = round_trip(&MessageId::new("msg-1"));
        let _ = round_trip(&AgentId::new("agent-a"));
        let _ = round_trip(&AgentVersionId::new("agent-v1"));
        let _ = round_trip(&AttemptId::new("attempt-1"));
        let _ = round_trip(&TurnId::new("turn-1"));
        let _ = round_trip(&EventId::new("evt-1"));
        let _ = round_trip(&ToolId::new("tool-bash"));
        let _ = round_trip(&ToolCallId::new("call_01"));
        let _ = round_trip(&ToolExecutionId::new("exec_1"));
        let _ = round_trip(&ToolVersionId::new("tool-v2"));
        let _ = round_trip(&McpServerId::new("mcp-fs"));
        let _ = round_trip(&SkillId::new("skill-readme"));
        let _ = round_trip(&SkillVersionId::new(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ));
        let _ = round_trip(&PythonEnvironmentDigest::new(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ));
        let _ = round_trip(&PythonToolEnvironmentDigest::new(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ));
        let _ = round_trip(&NodeEnvironmentDigest::new(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ));
        let _ = round_trip(&ArtifactId::new("artifact-1"));
        let _ = round_trip(&CapabilityRevision::new(42));
    }

    /// A typed id remains distinct from a raw string representation.
    #[test]
    fn id_accessors_expose_the_string_value() {
        let id = ConversationId::new("conv-1");
        assert_eq!(id.as_str(), "conv-1");
        assert_eq!(id.to_string(), "conv-1");
        assert_eq!(id.into_string(), "conv-1");
    }

    /// Capability revisions round-trip as plain JSON numbers, not strings.
    #[test]
    fn capability_revision_round_trips_as_number() {
        let revision = CapabilityRevision::new(42);
        let json = serde_json::to_string(&revision).expect("serialize revision");
        assert_eq!(json, "42");
        let decoded: CapabilityRevision =
            serde_json::from_str(&json).expect("deserialize revision");
        assert_eq!(decoded, revision);
        assert_eq!(decoded.get(), 42);
    }

    /// The default revision is zero, meaning no capability set is established.
    #[test]
    fn capability_revision_default_is_zero() {
        assert_eq!(CapabilityRevision::default().get(), 0);
    }
}
