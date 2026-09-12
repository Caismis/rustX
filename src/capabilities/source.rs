//! Canonical source identity above the two native materialization owners.
use crate::runtime::identity::McpServerId;
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
impl TryFrom<String> for ToolSourceId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let (python, name) = value
            .strip_prefix("python:")
            .map_or((false, value.as_str()), |name| (true, name));
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            || name.starts_with('.')
        {
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

/// Finite source demand admitted by the composition owner for one candidate.
/// The Python catalog carries discovery/path authority; selecting a missing
/// identity cannot manufacture a package or a source enablement grant.
#[derive(Debug, Clone, Default)]
pub struct ToolSourceDemand {
    pub sources: std::collections::BTreeSet<ToolSourceId>,
    pub managed_python: crate::runtime::resources::ManagedPythonCatalog,
}
impl ToolSourceDemand {
    #[must_use]
    pub fn new(
        sources: impl IntoIterator<Item = ToolSourceId>,
        managed_python: crate::runtime::resources::ManagedPythonCatalog,
    ) -> Self {
        Self {
            sources: sources.into_iter().collect(),
            managed_python,
        }
    }
}
