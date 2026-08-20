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
        /// The bounded failure diagnostic (at most
        /// [`CAPABILITY_FAILURE_REASON_MAX_BYTES`] bytes; see
        /// [`capability_failure_reason`]).
        reason: String,
    },
}

impl CapabilitySourceState {
    /// The one construction boundary of an unavailable state: the
    /// diagnostic is normalized (bounded, deterministic, valid UTF-8)
    /// *here*, before the state can enter the authoritative availability
    /// map, so no external peer or package-manager payload can make the
    /// committed state unbounded.
    pub fn unavailable(diagnostic: impl core::fmt::Display) -> Self {
        Self::Unavailable {
            reason: capability_failure_reason(diagnostic),
        }
    }
}

/// The maximum byte length of one stored capability availability
/// diagnostic (`CapabilitySourceState::Unavailable::reason`), including
/// the truncation marker.
///
/// An optional capability source is an external peer (an MCP server) or an
/// external toolchain (the Python plane): its error payloads are
/// attacker-influenceable and arbitrarily large. Every diagnostic is
/// normalized through [`capability_failure_reason`] at the
/// capability-owning boundary *before* it is committed into the
/// authoritative availability state, so the Runtime Client snapshot/event
/// stream and the TUI project an already-bounded value and never
/// re-truncate.
pub const CAPABILITY_FAILURE_REASON_MAX_BYTES: usize = 1024;

/// The marker appended when a diagnostic exceeds the bound.
const TRUNCATION_MARKER: &str = "\u{2026}[truncated]";

/// Normalizes one external failure diagnostic into the bounded capability
/// availability reason.
///
/// The result is always valid UTF-8 (it is a `String`) and at most
/// [`CAPABILITY_FAILURE_REASON_MAX_BYTES`] bytes: an over-long diagnostic
/// is cut at the largest UTF-8 character boundary that leaves room for the
/// truncation marker, then the marker is appended. The transformation is
/// deterministic — the same diagnostic always yields the same stored
/// reason — and it carries no redaction promise beyond what the owning
/// adapter's own `Display` already guarantees (MCP transport `Display`
/// shapes never serialize header values or environment maps).
pub fn capability_failure_reason(diagnostic: impl core::fmt::Display) -> String {
    let full = diagnostic.to_string();
    if full.len() <= CAPABILITY_FAILURE_REASON_MAX_BYTES {
        return full;
    }
    let mut end = CAPABILITY_FAILURE_REASON_MAX_BYTES - TRUNCATION_MARKER.len();
    while !full.is_char_boundary(end) {
        end -= 1;
    }
    let mut reason = full[..end].to_owned();
    reason.push_str(TRUNCATION_MARKER);
    debug_assert!(reason.len() <= CAPABILITY_FAILURE_REASON_MAX_BYTES);
    reason
}

/// The authoritative availability of every optional capability source the
/// coordinator evaluated, keyed by stable source identity.
///
/// Deterministic by construction (`BTreeMap` identity order). A source
/// absent from the map was never evaluated (not configured).
pub type CapabilityAvailability = BTreeMap<CapabilitySourceId, CapabilitySourceState>;

#[cfg(test)]
mod tests {
    use super::{
        CAPABILITY_FAILURE_REASON_MAX_BYTES, CapabilitySourceState, TRUNCATION_MARKER,
        capability_failure_reason,
    };

    /// A short diagnostic is stored verbatim.
    #[test]
    fn a_short_diagnostic_is_unchanged() {
        let reason = capability_failure_reason("the server closed the transport");
        assert_eq!(reason, "the server closed the transport");
    }

    /// An arbitrarily large external payload is bounded deterministically:
    /// the stored reason never exceeds the documented bound, is valid
    /// UTF-8, ends with the truncation marker, and repeats exactly.
    #[test]
    fn an_oversized_diagnostic_is_bounded_deterministically() {
        let huge = "x".repeat(CAPABILITY_FAILURE_REASON_MAX_BYTES * 8);
        let reason = capability_failure_reason(&huge);
        assert!(
            reason.len() <= CAPABILITY_FAILURE_REASON_MAX_BYTES,
            "the stored reason respects the documented bound: {}",
            reason.len()
        );
        assert!(reason.ends_with(TRUNCATION_MARKER));
        assert!(reason.starts_with("xxx"));
        assert_eq!(
            reason,
            capability_failure_reason(&huge),
            "truncation is deterministic"
        );
    }

    /// Truncation never splits a multi-byte character: the cut retreats to
    /// a UTF-8 character boundary.
    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        // The emoji straddles the truncation budget: the cut must
        // retreat to the character boundary just before it.
        let budget = CAPABILITY_FAILURE_REASON_MAX_BYTES - TRUNCATION_MARKER.len();
        let prefix = "a".repeat(budget - 1);
        let diagnostic = format!("{prefix}\u{1F980}{}", "z".repeat(32));
        assert!(diagnostic.len() > CAPABILITY_FAILURE_REASON_MAX_BYTES);
        let reason = capability_failure_reason(diagnostic);
        assert!(reason.len() <= CAPABILITY_FAILURE_REASON_MAX_BYTES);
        assert!(reason.ends_with(TRUNCATION_MARKER));
        assert!(
            !reason.contains('\u{1F980}'),
            "the multi-byte character is never split into the stored reason"
        );
        assert!(reason.starts_with(&prefix[..8]));
    }

    /// The unavailable constructor normalizes at the state-construction
    /// boundary, so a state built through it is already bounded.
    #[test]
    fn the_unavailable_constructor_normalizes_before_storage() {
        let huge = "y".repeat(CAPABILITY_FAILURE_REASON_MAX_BYTES * 4);
        let CapabilitySourceState::Unavailable { reason } =
            CapabilitySourceState::unavailable(&huge)
        else {
            panic!("the constructor builds the unavailable state");
        };
        assert!(reason.len() <= CAPABILITY_FAILURE_REASON_MAX_BYTES);
        assert!(reason.ends_with(TRUNCATION_MARKER));
    }
}
