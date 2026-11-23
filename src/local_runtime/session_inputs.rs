//! Persisted Session authoring is content, with publication owned by the
//! configuration application. It never describes process or private storage.
use crate::runtime::identity::McpServerId;
use crate::runtime::subagent::SubagentName;
use crate::runtime::workflow::WorkflowId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const CONFIG_DOCUMENT: &str = "session:config";
pub(super) const RESOURCES_DOCUMENT: &str = "session:resources";
const MAX_SESSION_CONFIG_BYTES: usize = 128 * 1024;
const MAX_SESSION_RESOURCE_BYTES: usize = 512 * 1024;
const MAX_SESSION_RESOURCE_DEFINITIONS: usize = 128;

/// Complete Session-authored definitions keyed by their resource identity.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SessionResources {
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::runtime::workflow::deserialize_unique_map"
    )]
    pub agents: BTreeMap<SubagentName, String>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::runtime::workflow::deserialize_unique_map"
    )]
    pub workflows: BTreeMap<WorkflowId, String>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::runtime::workflow::deserialize_unique_map"
    )]
    pub mcp: BTreeMap<McpServerId, String>,
}

impl SessionResources {
    pub(super) fn check_bounds(&self) -> Result<(), String> {
        if self.agents.len() + self.workflows.len() + self.mcp.len()
            > MAX_SESSION_RESOURCE_DEFINITIONS
        {
            return Err("SessionResources exceeds 128 definitions".into());
        }
        let bytes = self
            .agents
            .values()
            .chain(self.workflows.values())
            .chain(self.mcp.values())
            .map(String::len)
            .fold(0usize, usize::saturating_add);
        if bytes > MAX_SESSION_RESOURCE_BYTES {
            return Err("SessionResources exceeds its total byte limit".into());
        }
        Ok(())
    }
}

/// An exact revision of existing Session-owned input for the same Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SessionInputReference {
    /// Existing Session supplying the saved input.
    pub session_id: crate::runtime::identity::SessionId,
    /// Exact revision observed by the caller.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub revision: u64,
}

/// Reference either input independently when creating a Session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SessionInputReferences {
    /// Source of the initial Session config.
    pub session_config: Option<SessionInputReference>,
    /// Source of the initial Session resource declarations.
    pub session_resources: Option<SessionInputReference>,
}

/// An omitted field makes no edit. Clearing reveals the current lower layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    content = "content",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SessionInputChange<T> {
    Clear,
    Replace(T),
    Reference(SessionInputReference),
}

pub(super) fn check_config_bounds(content: &str) -> Result<(), String> {
    if content.len() > MAX_SESSION_CONFIG_BYTES {
        return Err("SessionConfig exceeds its byte limit".into());
    }
    Ok(())
}

/// Strict TOML source for one Session, with diagnostics that omit its value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SessionConfigContent(pub String);

impl std::fmt::Debug for SessionConfigContent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionConfig")
            .field("bytes", &self.0.len())
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionAuthoredInputs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_config: Option<SessionConfigContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_resources: Option<SessionResources>,
}

impl SessionAuthoredInputs {
    pub(crate) fn is_empty(&self) -> bool {
        self.session_config.is_none() && self.session_resources.is_none()
    }

    pub(crate) fn validate_bounds(&self) -> Result<(), String> {
        if let Some(config) = &self.session_config {
            check_config_bounds(&config.0)?;
        }
        if let Some(resources) = &self.session_resources {
            resources.check_bounds()?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for SessionResources {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionResources")
            .field("agents", &self.agents.len())
            .field("workflows", &self.workflows.len())
            .field("mcp", &self.mcp.len())
            .finish()
    }
}
