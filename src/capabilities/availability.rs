//! Typed capability-source availability state (Issue #81).
//!
//! The governing invariant:
//!
//! > Failure of an optional capability source changes that source's
//! > availability state; it must never terminate the core conversation
//! > runtime, and it must never erase another source that initialized
//! > successfully.
//!
//! The capability coordinator owns this state authoritatively. It is
//! control-plane health, deliberately separate from the execution
//! identity: [`crate::runtime::identity::CapabilityRevision`] advances
//! only when the effective committed executable capability set changes,
//! never because a diagnostic reason string changed, and the
//! provider/model-facing `CapabilitiesManifest` never carries
//! availability. The Runtime Client projects this state outward
//! read-only, so a client observes *why* a source is unavailable instead
//! of inferring failure from a dead transport.

use std::collections::BTreeMap;

use crate::runtime::identity::McpServerId;

/// The stable identity of one optional capability source.
///
/// Native tools are the base registry of the core runtime: their
/// construction failure is a fatal composition error, so they never
/// appear here. Only optional external capability sources have
/// availability state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapabilitySourceId {
    /// The custom Python tool plane of the Workspace
    /// (`.agents/tools/`).
    Python,
    /// One configured MCP server, keyed by its authoritative identity.
    Mcp(McpServerId),
}

impl core::fmt::Display for CapabilitySourceId {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Python => formatter.write_str("python"),
            Self::Mcp(server_id) => write!(formatter, "mcp:{server_id}"),
        }
    }
}

/// The availability of one optional capability source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilitySourceState {
    /// The source initialized; its capabilities are part of the committed
    /// executable set.
    Ready,
    /// The source failed to initialize (or failed a later refresh); it
    /// contributes nothing to the committed executable set. `reason` is a
    /// bounded diagnostic, never a credential or a secret.
    Unavailable {
        /// The bounded failure diagnostic.
        reason: String,
    },
}

/// The authoritative availability of every optional capability source the
/// coordinator evaluated, keyed by stable source identity.
///
/// Deterministic by construction (`BTreeMap` identity order). A source
/// absent from the map was never evaluated (not configured).
pub type CapabilityAvailability = BTreeMap<CapabilitySourceId, CapabilitySourceState>;
