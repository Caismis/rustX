//! Subagent admission vocabulary shared across layers.

use crate::runtime::identity::ToolCallId;
use serde::{Deserialize, Serialize};

/// Truthful admission source. Client controls have no model `ToolCall` identity.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentActivationOrigin {
    CreationTool {
        tool_call_id: ToolCallId,
    },
    MessageTool {
        tool_call_id: ToolCallId,
    },
    ClientControl,
    Workflow {
        node_id: Box<crate::runtime::workflow::WorkflowNodeInstance>,
    },
}

impl AgentActivationOrigin {
    #[must_use]
    pub fn tool_call_id(&self) -> Option<&ToolCallId> {
        match self {
            Self::CreationTool { tool_call_id } | Self::MessageTool { tool_call_id } => {
                Some(tool_call_id)
            }
            Self::ClientControl | Self::Workflow { .. } => None,
        }
    }
}

/// The canonical typed name of one admitted subagent definition.
///
/// The keyspace is deliberately narrow: lowercase ASCII letters, digits,
/// `-`, and `_`, starting with a letter. A name is the model-facing routing
/// token, the durable ownership identity, and the Runtime Client projection
/// identity, so an ambiguous or shell-shaped spelling is rejected at the
/// configuration boundary rather than normalized later.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]
pub struct SubagentName(String);

impl SubagentName {
    /// The maximum byte length of a canonical name.
    pub const MAX_BYTES: usize = 64;

    /// Parses one canonical subagent name.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection detail of the first violated
    /// rule.
    pub fn parse(value: &str) -> Result<Self, SubagentNameError> {
        if value.is_empty() {
            return Err(SubagentNameError::Empty);
        }
        if value.len() > Self::MAX_BYTES {
            return Err(SubagentNameError::TooLong { bytes: value.len() });
        }
        match value.chars().next() {
            None => return Err(SubagentNameError::Empty),
            Some(first) if !first.is_ascii_lowercase() => {
                return Err(SubagentNameError::InvalidStart { found: first });
            }
            Some(_) => {}
        }
        if let Some(found) = value
            .chars()
            .find(|character| !matches!(character, 'a'..='z' | '0'..='9' | '-' | '_'))
        {
            return Err(SubagentNameError::InvalidCharacter { found });
        }
        Ok(Self(value.to_owned()))
    }

    /// The canonical name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for SubagentName {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SubagentName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A canonical subagent-name violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentNameError {
    /// The name is empty.
    Empty,
    /// The name exceeds [`SubagentName::MAX_BYTES`].
    TooLong {
        /// The offending byte length.
        bytes: usize,
    },
    /// The name does not start with a lowercase ASCII letter.
    InvalidStart {
        /// The offending first character.
        found: char,
    },
    /// The name contains a character outside `[a-z0-9_-]`.
    InvalidCharacter {
        /// The offending character.
        found: char,
    },
}

impl core::fmt::Display for SubagentNameError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a subagent name must not be empty"),
            Self::TooLong { bytes } => write!(
                formatter,
                "a subagent name exceeds the {}-byte bound ({bytes} bytes)",
                SubagentName::MAX_BYTES
            ),
            Self::InvalidStart { found } => write!(
                formatter,
                "a subagent name must start with a lowercase ASCII letter, found {found:?}"
            ),
            Self::InvalidCharacter { found } => write!(
                formatter,
                "a subagent name accepts only [a-z0-9_-], found {found:?}"
            ),
        }
    }
}

impl std::error::Error for SubagentNameError {}
