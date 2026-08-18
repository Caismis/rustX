//! Capability preparation and commit errors (M6).

use crate::runtime::identity::CapabilityRevision;
use crate::skills::{DependencyConflict, EnvironmentPreparationError, SkillPackageError};

/// A candidate capability preparation failure.
///
/// Preparation failure never mutates the active capability state: the
/// current active revision remains authoritative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityPreparationError {
    /// Skill discovery/parsing/validation failed (one malformed Skill
    /// fails the whole candidate transaction).
    SkillDiscovery(SkillPackageError),
    /// The merged dependency declarations conflict across active Skills.
    DependencyConflict(DependencyConflict),
    /// The environment store is not disjoint from the model Workspace.
    EnvironmentStoreOverlapsWorkspace { store_root: String },
    /// A shared environment identity/materialization failure.
    Environment(EnvironmentPreparationError),
    /// An MCP server could not be discovered or its catalog was unstable.
    Mcp(String),
    /// A custom Python `ToolVersion` could not be discovered, published, or prepared.
    Python(String),
    /// The composed canonical registry was rejected as invalid or colliding.
    ToolRegistry(String),
    /// Preparation was requested after the owning conversation runtime
    /// closed new capability admission.
    ConversationInactive,
}

impl core::fmt::Display for CapabilityPreparationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SkillDiscovery(error) => write!(f, "skill discovery failed: {error}"),
            Self::DependencyConflict(conflict) => write!(f, "{conflict}"),
            Self::EnvironmentStoreOverlapsWorkspace { store_root } => write!(
                f,
                "the environment store root {store_root:?} must be disjoint from the model \
                 Workspace"
            ),
            Self::Environment(error) => write!(f, "{error}"),
            Self::Mcp(error) => write!(f, "MCP preparation failed: {error}"),
            Self::Python(error) => write!(f, "Python tool preparation failed: {error}"),
            Self::ToolRegistry(error) => write!(f, "tool registry composition failed: {error}"),
            Self::ConversationInactive => write!(
                f,
                "capability preparation is closed because the conversation runtime is draining"
            ),
        }
    }
}

impl std::error::Error for CapabilityPreparationError {}

impl From<SkillPackageError> for CapabilityPreparationError {
    fn from(error: SkillPackageError) -> Self {
        Self::SkillDiscovery(error)
    }
}

impl From<DependencyConflict> for CapabilityPreparationError {
    fn from(conflict: DependencyConflict) -> Self {
        Self::DependencyConflict(conflict)
    }
}

impl From<EnvironmentPreparationError> for CapabilityPreparationError {
    fn from(error: EnvironmentPreparationError) -> Self {
        Self::Environment(error)
    }
}

/// A candidate capability activation (commit) failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityCommitError {
    /// A candidate prepared from an obsolete base revision cannot overwrite
    /// newer capability state.
    StaleCandidate {
        prepared_from: CapabilityRevision,
        current: CapabilityRevision,
    },
    /// A capability commit while an attempt lease is active is rejected.
    Busy,
    /// A capability commit on a coordinator already claimed by an inactive
    /// `ConversationRuntime` is rejected (Issue #61).
    ///
    /// Once a conversation runtime owns this coordinator, active capability
    /// mutation follows the runtime lifecycle: the startup commit that
    /// happens *before* the conversation runtime is constructed remains
    /// allowed, and after `ConversationRuntime::activate` commits follow
    /// the normal quiescence rules.
    ConversationInactive,
    /// A tools/list candidate was invalidated before the swap.
    StaleMcpCandidate {
        server_id: crate::runtime::identity::McpServerId,
    },
}

impl core::fmt::Display for CapabilityCommitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::StaleCandidate {
                prepared_from,
                current,
            } => write!(
                f,
                "stale candidate: prepared from revision {prepared_from:?} but the active \
                 revision is now {current:?}"
            ),
            Self::Busy => write!(
                f,
                "a capability commit is rejected while an attempt capability lease is active"
            ),
            Self::ConversationInactive => write!(
                f,
                "a capability commit is rejected while the owning conversation runtime is inactive"
            ),
            Self::StaleMcpCandidate { server_id } => write!(
                f,
                "MCP capability candidate for server {server_id} was invalidated before commit"
            ),
        }
    }
}

impl std::error::Error for CapabilityCommitError {}
