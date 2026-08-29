//! The capability composition and coordination layer (M6).
//!
//! This layer owns the operational capability contract that M6 needs:
//!
//! - the immutable active [`CapabilitySnapshot`] (`snapshot` module);
//! - the RAII-style attempt capability lease ([`AttemptCapabilityLease`]);
//! - candidate preparation and the quiescent atomic commit boundary
//!   ([`CapabilityCoordinator`]);
//! - the exact M6 quiescence linearization point.
//!
//! # Ownership
//!
//! The layer depends on the runtime identity types, the Skill plane
//! (`crate::skills`), and the Tool plane (`crate::tools`); lower-level
//! runtime identity modules never depend upward on Skills.
//!
//! # M6 quiescence
//!
//! A capability activation/commit is legal only when there are zero active
//! [`AttemptCapabilityLease`] leases for that conversation.
//! Conversation-owned detached background executions do not count as
//! active attempt leases. Attempt lease acquisition and candidate
//! capability commit are serialized through the same synchronization
//! boundary (the coordinator state mutex):
//!
//! - attempt acquisition wins first → commit observes busy and cannot
//!   activate a new revision;
//! - commit wins first → the next attempt snapshots the new revision.
//!
//! There is no unchecked window between the deciding quiescence
//! observation and the active-snapshot swap, and no sleep-based
//! coordination. A candidate may be prepared independently; only its
//! activation respects the commit boundary. If candidate preparation or
//! commit fails, the current active revision remains authoritative, and a
//! candidate prepared from an obsolete base revision is rejected as stale.
//!
//! # Optional-source availability (Issue #81)
//!
//! Failure of an optional capability source (the custom Python tool plane,
//! or any one configured MCP server) is not a preparation error: it is
//! recorded as typed [`CapabilitySourceState::Unavailable`] state keyed by
//! stable [`CapabilitySourceId`], and preparation continues with every
//! other source. Only successfully prepared capability objects enter the
//! committed active snapshot, and `CapabilityRevision` advances only when
//! the effective committed executable set changes — never because a
//! diagnostic reason changed.

mod availability;
mod coordinator;
mod error;
pub mod selected;
mod snapshot;
mod tools;

pub use availability::{
    CAPABILITY_FAILURE_REASON_MAX_BYTES, CapabilityAvailability, CapabilitySourceId,
    CapabilitySourceState, capability_failure_reason,
};
pub(crate) use coordinator::CommittedCapability;
pub use coordinator::{
    AttemptCapabilityLease, CapabilityCoordinator, CapabilityCoordinatorConfig, CapabilityObserver,
    CapabilityResourceInputs, PreparedCapabilityCandidate,
};
pub use error::{CapabilityCommitError, CapabilityPreparationError};
pub use selected::{
    SelectedCapabilityPlan, SelectedMaterializationError, SelectedMcpTool, SelectedPythonTool,
};
pub use snapshot::CapabilitySnapshot;
pub use tools::{AvailableTool, AvailableToolCatalog, ToolActivationPolicy};

/// The commit-boundary synchronization hook, used by the Runtime Client
/// lock-order tests to park a commit with the coordinator lock held.
#[cfg(test)]
pub(crate) use coordinator::test_sync;

pub(crate) use coordinator::RuntimeCapabilityPublication;
