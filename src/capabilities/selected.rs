//! A child's finite, parent-frozen source Tool and Resource materialization plan.
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
    /// Source-wide Resources authority, frozen independently of ordinary Tools.
    pub resource_sources: std::collections::BTreeSet<ToolSourceId>,
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
            .chain(self.resource_sources.iter().cloned())
            .collect()
    }

    /// Whether this plan needs any externally sourced execution plane.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source_tools.is_empty() && self.resource_sources.is_empty()
    }
}

/// A selected-only materialization failure.
///
/// Every variant is decided during child preparation, before the child
/// answers `Ready` and therefore long before any durable ownership commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedMaterializationError {
    /// The connected source no longer advertises the frozen Resources capability.
    SourceResourcesUnsupported {
        /// The source whose Resources authority the parent froze.
        source_id: ToolSourceId,
    },
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
            Self::SourceResourcesUnsupported { source_id } => write!(
                formatter,
                "ToolSource {source_id} no longer advertises the frozen Resources capability; \
                 the child refuses to start weaker than it was authorized"
            ),
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

#[cfg(test)]
mod tests {
    use super::{SelectedCapabilityPlan, SelectedSourceTool};
    use crate::capabilities::ToolSourceId;
    use crate::runtime::identity::{McpServerId, SourceToolIdentity};

    #[test]
    fn resource_only_authority_requires_a_source_even_without_tools() {
        let source = ToolSourceId::Mcp(McpServerId::new("documents"));
        let plan = SelectedCapabilityPlan {
            resource_sources: [source.clone()].into(),
            ..Default::default()
        };
        assert!(!plan.is_empty());
        assert_eq!(plan.required_sources(), [source].into());
    }

    #[test]
    fn exact_tools_and_source_resources_remain_distinct_authority() {
        let source = ToolSourceId::Mcp(McpServerId::new("documents"));
        let python = ToolSourceId::ManagedPython("reports".into());
        let mut plan = SelectedCapabilityPlan {
            source_tools: vec![SelectedSourceTool {
                source_id: source.clone(),
                name: "find".into(),
                identity: SourceToolIdentity::new("sha256:frozen"),
            }],
            ..Default::default()
        };
        assert!(plan.resource_sources.is_empty());
        assert_eq!(plan.required_sources(), [source.clone()].into());
        plan.resource_sources
            .extend([source.clone(), python.clone()]);
        assert_eq!(plan.required_sources(), [source, python].into());
        assert_eq!(plan.source_tools.len(), 1);
    }
}
