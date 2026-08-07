//! The compiled runtime manifest boundary.
//!
//! A `RuntimeManifest` is the deterministic, immutable description of one
//! attempt: agent identity and instructions, model protocol and reasoning
//! configuration, the capability set at a specific monotonic revision, and
//! context and execution limits. An attempt snapshots one manifest when it
//! starts, and that snapshot is immutable for the attempt's lifetime.
//!
//! M1 establishes the boundary with the smallest typed representation
//! needed; capability mutation, skill materialization, and MCP binding logic
//! are later milestones. No TypeScript/control-plane model is imported.

use serde::{Deserialize, Serialize};

use crate::model::types::{ModelProtocol, ReasoningEffort};
use crate::runtime::identity::{
    AgentId, AgentVersionId, CapabilityRevision, McpServerId, SkillId, SkillVersionId, ToolId,
};

/// The current schema version of [`RuntimeManifest`].
pub const MANIFEST_SCHEMA_VERSION: u16 = 1;

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

/// Model protocol and generation configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelManifest {
    /// Protocol the adapter must speak.
    pub protocol: ModelProtocol,
    /// Provider model identifier.
    pub model: String,
    /// Reasoning effort configuration.
    pub reasoning: ReasoningEffort,
}

/// The capability set observed by an attempt.
///
/// Skill and MCP binding data is not yet frozen by later milestones; these
/// bindings are the smallest typed runtime-owned representation that
/// establishes the manifest boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitiesManifest {
    /// Monotonic capability revision snapshot by the attempt.
    pub revision: CapabilityRevision,
    /// Bound skills.
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
}

/// An MCP server bound into the capability set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpBinding {
    /// Server identity.
    pub server_id: McpServerId,
}

/// Context token reservation defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextManifest {
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
    use crate::model::types::{ModelProtocol, ReasoningEffort};
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
                model: "gpt-5-mini".to_owned(),
                reasoning: ReasoningEffort::High,
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
                }],
                mcp: vec![McpBinding {
                    server_id: McpServerId::new("mcp-fs"),
                }],
            },
            context: ContextManifest {
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
        assert_eq!(value["schema_version"], 1);
    }
}
