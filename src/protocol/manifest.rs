//! The compiled runtime manifest boundary.
//!
//! A `RuntimeManifest` is the deterministic, immutable description of one
//! attempt: agent identity and instructions, the catalog model reference and
//! its selected reasoning profile, the capability set at a specific
//! monotonic revision, and context and execution limits. An attempt
//! snapshots one manifest when it starts, and that snapshot is immutable for
//! the attempt's lifetime.
//!
//! M1 establishes the boundary with the smallest typed representation
//! needed; capability mutation, Skill materialization, and external MCP/Python
//! binding provenance are runtime-owned. No TypeScript/control-plane model is
//! imported.

use serde::{Deserialize, Serialize};

use crate::model::catalog::{ModelRef, ReasoningProfileId};
use crate::model::types::ModelProtocol;
use crate::runtime::identity::{
    AgentId, AgentVersionId, CapabilityRevision, McpServerId, SkillId, SkillVersionId, ToolId,
};
use crate::tools::types::ToolOrigin;

/// The current schema version of [`RuntimeManifest`]. M7 version 2 adds
/// provider-independent `ToolOrigin` provenance to every tool binding.
pub const MANIFEST_SCHEMA_VERSION: u16 = 2;

/// The immutable execution description of one attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeManifest {
    /// Explicit manifest schema version; never inferred from the crate
    /// version.
    pub schema_version: u16,
    /// Runtime version that compiled this manifest.
    pub runtime_version: String,
    /// Agent identity, version, and instructions.
    pub agent: AgentManifest,
    /// Model protocol and configuration.
    pub model: ModelManifest,
    /// The capability set at a monotonic revision.
    pub capabilities: CapabilitiesManifest,
    /// Context token reservation defaults.
    pub context: ContextManifest,
    /// Attempt execution limits.
    pub limits: AttemptLimitsManifest,
}

/// Agent identity, immutable version, and instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentManifest {
    /// Agent identity.
    pub id: AgentId,
    /// Immutable agent version identity.
    pub version_id: AgentVersionId,
    /// Agent instructions.
    pub instructions: String,
}

/// The model binding and reasoning selection of one attempt.
///
/// Reasoning is a model-declared *named profile*, never a universal effort
/// enum: the runtime assigns no meaning to the profile name, and the
/// profile's wire behaviour is exactly its configured provider request
/// parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelManifest {
    /// Protocol the adapter must speak.
    pub protocol: ModelProtocol,
    /// The fully qualified catalog model reference.
    pub model: ModelRef,
    /// The selected reasoning profile, when the model declares any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_profile: Option<ReasoningProfileId>,
    /// Whether the selected profile semantically enables reasoning.
    pub reasoning_enabled: bool,
}

/// The capability set observed by an attempt.
///
/// Skill, MCP, and Python binding data is the typed runtime-owned provenance
/// projection of one immutable capability revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitiesManifest {
    /// Monotonic capability revision snapshot by the attempt.
    pub revision: CapabilityRevision,
    /// All Skills bound into the immutable attempt capability set, including
    /// Skills hidden from the model-visible catalog.
    #[serde(default)]
    pub skills: Vec<SkillBinding>,
    /// Bound tools.
    #[serde(default)]
    pub tools: Vec<ToolBinding>,
    /// Bound MCP servers.
    #[serde(default)]
    pub mcp: Vec<McpBinding>,
}

/// A skill bound into the capability set at an immutable version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBinding {
    /// Skill identity.
    pub skill_id: SkillId,
    /// Immutable skill version identity.
    pub version_id: SkillVersionId,
}

/// A tool bound into the capability set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBinding {
    /// Tool identity.
    pub tool_id: ToolId,
    /// Stable tool name.
    pub name: String,
    /// Provider-independent origin and immutable provenance.
    pub origin: ToolOrigin,
}

/// An MCP server bound into the capability set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpBinding {
    /// Server identity.
    pub server_id: McpServerId,
}

/// Context token configuration.
///
/// The model context window is runtime-owned configuration, never a
/// hard-coded per-model catalog in the context engine. The effective soft
/// input limit of a request is
/// `context_window_tokens - reserve_tokens - max_output_tokens` (checked,
/// impossible configurations are rejected), where `max_output_tokens` is the
/// runtime-resolved generation budget of that request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextManifest {
    /// The model context window in tokens.
    pub context_window_tokens: u64,
    /// Tokens permanently reserved out of the model context window.
    pub reserve_tokens: u64,
    /// Tokens of recent conversation history kept uncompressed.
    pub keep_recent_tokens: u64,
}

/// Attempt execution limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptLimitsManifest {
    /// Maximum number of turns in one attempt.
    pub max_turns: u32,
    /// Maximum number of tool calls in one attempt.
    pub max_tool_calls: u32,
    /// Maximum attempt runtime in seconds.
    pub max_runtime_seconds: u64,
}

#[cfg(test)]
mod tests {
    use super::{
        AgentManifest, AttemptLimitsManifest, CapabilitiesManifest, ContextManifest,
        MANIFEST_SCHEMA_VERSION, McpBinding, ModelManifest, RuntimeManifest, SkillBinding,
        ToolBinding,
    };
    use crate::model::catalog::{ModelRef, ReasoningProfileId};
    use crate::model::types::ModelProtocol;
    use crate::runtime::identity::{
        AgentId, AgentVersionId, CapabilityRevision, McpServerId, SkillId, SkillVersionId, ToolId,
    };

    /// An example manifest for tests.
    fn example_manifest() -> RuntimeManifest {
        RuntimeManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            runtime_version: "0.1.0".to_owned(),
            agent: AgentManifest {
                id: AgentId::new("agent-a"),
                version_id: AgentVersionId::new("agent-v1"),
                instructions: "You are a helpful agent.".to_owned(),
            },
            model: ModelManifest {
                protocol: ModelProtocol::OpenAiResponses,
                model: ModelRef::parse("provider-a/gpt-5-mini").expect("valid reference"),
                reasoning_profile: Some(ReasoningProfileId::new("high")),
                reasoning_enabled: true,
            },
            capabilities: CapabilitiesManifest {
                revision: CapabilityRevision::new(42),
                skills: vec![SkillBinding {
                    skill_id: SkillId::new("skill-readme"),
                    version_id: SkillVersionId::new("skill-v3"),
                }],
                tools: vec![ToolBinding {
                    tool_id: ToolId::new("tool-bash"),
                    name: "bash".to_owned(),
                    origin: crate::tools::types::ToolOrigin::Builtin,
                }],
                mcp: vec![McpBinding {
                    server_id: McpServerId::new("mcp-fs"),
                }],
            },
            context: ContextManifest {
                context_window_tokens: 131_072,
                reserve_tokens: 16_384,
                keep_recent_tokens: 20_000,
            },
            limits: AttemptLimitsManifest {
                max_turns: 64,
                max_tool_calls: 128,
                max_runtime_seconds: 1_800,
            },
        }
    }

    /// The manifest round-trips deterministically with its explicit revision.
    #[test]
    fn manifest_round_trip() {
        let manifest = example_manifest();
        let first = serde_json::to_string(&manifest).expect("serialize manifest");
        let second = serde_json::to_string(&manifest).expect("serialize manifest again");
        assert_eq!(first, second, "serialization must be deterministic");
        let decoded: RuntimeManifest = serde_json::from_str(&first).expect("deserialize manifest");
        assert_eq!(decoded, manifest);
        assert_eq!(decoded.capabilities.revision.get(), 42);
    }

    /// The schema version is an explicit value, not derived from the crate.
    #[test]
    fn manifest_schema_version_is_explicit() {
        let value = serde_json::to_value(example_manifest()).expect("serialize manifest");
        assert_eq!(value["schema_version"], 2);
    }
}
