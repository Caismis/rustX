//! Exact executable tool identities shared by Workflow Tool leaves and
//! allowlists.

use serde::{Deserialize, Serialize};

use super::source::ToolSourceId;

/// One exact executable identity, used by Workflow Tool leaves and allowlists.
/// Source-wide Agent capability selection cannot be authored in this type.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExactToolSelector {
    Builtin {
        name: String,
    },
    Source {
        source_id: ToolSourceId,
        name: String,
    },
}
impl ExactToolSelector {
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Builtin { name } => format!("builtin:{name}"),
            Self::Source { source_id, name } => format!("source:{source_id}/{name}"),
        }
    }
    #[must_use]
    pub const fn source(&self) -> Option<&ToolSourceId> {
        match self {
            Self::Builtin { .. } => None,
            Self::Source { source_id, .. } => Some(source_id),
        }
    }
}
impl std::fmt::Display for ExactToolSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical())
    }
}
