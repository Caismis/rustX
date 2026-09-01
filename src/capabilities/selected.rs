//! The **selected-only** capability materialization plan of a subagent
//! child (Issue #145).
//!
//! A child runtime does not discover capabilities; it realizes a frozen
//! selection. This module is the typed input of that realization, and its
//! shape is what makes "selected only" structural rather than a rule
//! somebody has to remember:
//!
//! ```text
//! discovery pipeline (parent)     selected realization (child)
//!   walk every Skill root           materialize the frozen Skill set
//!   connect every MCP server        connect only the servers named here
//!   prepare every workspace         (managed Python packages cross as
//!     Python package                  ordinary frozen MCP bindings, #174)
//!   activate by policy              expose exactly these definitions
//! ```
//!
//! The plan carries **expected identities**, not just names: an MCP
//! selection carries the parent-frozen
//! [`McpToolIdentity`](crate::runtime::identity::McpToolIdentity) the child
//! must recompute from its own catalog read.
//! A child that cannot reproduce an identity fails preparation — it never
//! substitutes a same-named replacement.

use crate::runtime::identity::{McpServerId, McpToolIdentity};

/// One MCP tool the child must materialize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedMcpTool {
    /// The server that publishes it.
    pub server_id: McpServerId,
    /// The canonical tool name as the server publishes it.
    pub name: String,
    /// The parent-frozen expected canonical identity.
    pub identity: McpToolIdentity,
}

/// The complete selected-only materialization plan of one child.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectedCapabilityPlan {
    /// Exactly the MCP tools to expose, in the frozen canonical order.
    pub mcp_tools: Vec<SelectedMcpTool>,
}

impl SelectedCapabilityPlan {
    /// The distinct MCP servers this plan requires, in identity order.
    ///
    /// This is the set a child connects — never the configured set.
    #[must_use]
    pub fn required_mcp_servers(&self) -> std::collections::BTreeSet<McpServerId> {
        self.mcp_tools
            .iter()
            .map(|tool| tool.server_id.clone())
            .collect()
    }

    /// Whether this plan needs any externally sourced execution plane.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mcp_tools.is_empty()
    }
}

/// A selected-only materialization failure.
///
/// Every variant is decided during child preparation, before the child
/// answers `Ready` and therefore long before any durable ownership commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedMaterializationError {
    /// The server no longer publishes a tool of that name.
    McpToolMissing {
        /// The server that was connected.
        server_id: McpServerId,
        /// The missing tool name.
        name: String,
    },
    /// The server publishes that tool, but its canonical semantic identity
    /// is not the one the parent generation froze.
    McpIdentityMismatch {
        /// The server that was connected.
        server_id: McpServerId,
        /// The tool name.
        name: String,
        /// The parent-frozen expected identity.
        expected: McpToolIdentity,
        /// The identity the child derived from its own catalog read.
        observed: McpToolIdentity,
    },
}

impl core::fmt::Display for SelectedMaterializationError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::McpToolMissing { server_id, name } => write!(
                formatter,
                "MCP server {server_id} no longer publishes the frozen tool {name:?}; the \
                 child refuses to start weaker than it was authorized"
            ),
            Self::McpIdentityMismatch {
                server_id,
                name,
                expected,
                observed,
            } => write!(
                formatter,
                "MCP server {server_id} publishes {name:?} with canonical identity {observed} \
                 but the invoking generation authorized {expected}; the child refuses to \
                 execute a definition its parent never authorized"
            ),
        }
    }
}

impl std::error::Error for SelectedMaterializationError {}
