//! Canonical source identity above the two native materialization owners.
use rustx_identity::McpServerId;
use serde::{Deserialize, Serialize};

/// A configured MCP source or a canonical Managed Python package.
/// Namespace parsing happens only at the authoring boundary; runtime dispatch
/// matches these variants, never display strings.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub enum ToolSourceId {
    Mcp(McpServerId),
    ManagedPython(String),
}
impl ToolSourceId {
    /// The Managed Python package identity, without the `python:` namespace
    /// that only the source-selection domain carries.
    #[must_use]
    pub fn managed_python(&self) -> Option<&str> {
        match self {
            Self::ManagedPython(name) => Some(name),
            Self::Mcp(_) => None,
        }
    }
}
impl TryFrom<String> for ToolSourceId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let (python, name) = value
            .strip_prefix("python:")
            .map_or((false, value.as_str()), |name| (true, name));
        if name.is_empty() {
            return Err(format!("invalid ToolSource identity {value:?}"));
        }
        if python {
            crate::tools::python::validate_identifier(name).map_err(|error| error.to_string())?;
        }
        Ok(if python {
            Self::ManagedPython(name.to_owned())
        } else {
            Self::Mcp(McpServerId::new(name))
        })
    }
}
impl From<ToolSourceId> for String {
    fn from(value: ToolSourceId) -> Self {
        value.to_string()
    }
}
impl std::fmt::Display for ToolSourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mcp(id) => id.fmt(f),
            Self::ManagedPython(package) => write!(f, "python:{package}"),
        }
    }
}
