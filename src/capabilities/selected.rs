//! A child's finite, parent-frozen ordinary source Tool materialization plan.
//! Selection never rediscovers or expands All in the child. The native source
//! materializer must reproduce each canonical identity before constructing an
//! executor; a same-name replacement is a preparation failure.

use super::ToolSourceId;
use crate::runtime::identity::SourceToolIdentity;

/// One source Tool the child must materialize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedSourceTool {
    /// The server that publishes it.
    pub source_id: ToolSourceId,
    /// The canonical tool name as the server publishes it.
    pub name: String,
    /// The parent-frozen expected canonical identity.
    pub identity: SourceToolIdentity,
}

/// The complete selected-only materialization plan of one child.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectedCapabilityPlan {
    /// Exactly the source Tools to expose, in the frozen canonical order.
    pub source_tools: Vec<SelectedSourceTool>,
}

impl SelectedCapabilityPlan {
    /// The distinct sources this plan requires, in identity order.
    ///
    /// This is the set a child connects — never the configured set.
    #[must_use]
    pub fn required_sources(&self) -> std::collections::BTreeSet<ToolSourceId> {
        self.source_tools
            .iter()
            .map(|tool| tool.source_id.clone())
            .collect()
    }

    /// Whether this plan needs any externally sourced execution plane.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source_tools.is_empty()
    }
}

/// A selected-only materialization failure.
///
/// Every variant is decided during child preparation, before the child
/// answers `Ready` and therefore long before any durable ownership commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedMaterializationError {
    /// The server no longer publishes a tool of that name.
    SourceToolMissing {
        /// The server that was connected.
        source_id: ToolSourceId,
        /// The missing tool name.
        name: String,
    },
    /// The server publishes that tool, but its canonical semantic identity
    /// is not the one the parent generation froze.
    SourceIdentityMismatch {
        /// The server that was connected.
        source_id: ToolSourceId,
        /// The tool name.
        name: String,
        /// The parent-frozen expected identity.
        expected: SourceToolIdentity,
        /// The identity the child derived from its own catalog read.
        observed: SourceToolIdentity,
    },
}

impl core::fmt::Display for SelectedMaterializationError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SourceToolMissing { source_id, name } => write!(
                formatter,
                "ToolSource {source_id} no longer publishes the frozen tool {name:?}; the \
                 child refuses to start weaker than it was authorized"
            ),
            Self::SourceIdentityMismatch {
                source_id,
                name,
                expected,
                observed,
            } => write!(
                formatter,
                "ToolSource {source_id} publishes {name:?} with canonical identity {observed} \
                 but the invoking generation authorized {expected}; the child refuses to \
                 execute a definition its parent never authorized"
            ),
        }
    }
}

impl std::error::Error for SelectedMaterializationError {}
