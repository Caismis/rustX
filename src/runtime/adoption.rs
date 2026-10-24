//! Native configuration-adoption contracts.
//!
//! Configuration authoring (source capture, merge, provenance, redaction and
//! unit composition) belongs to the application configuration owner. The
//! runtime owns what happens after that: an off-side prepared candidate, the
//! advisory adoption eligibility, the adoption refusal vocabulary, the frozen
//! configuration facts of an admitted Attempt, and the one pure cache-impact
//! rule that decides whether a candidate may be adopted implicitly.
//!
//! None of these values interprets authored documents; a candidate carries
//! the application-composed [`RuntimeConfiguration`](crate::runtime::resources::RuntimeConfiguration)
//! inside its preview generation unchanged.

use std::sync::Arc;

use crate::model::request_shape::CacheImpact;
use crate::model::session::SessionModelState;
use crate::runtime::resources::RuntimeResourceSnapshot;

/// Compose a live catalog observation over one exact adopted resource binding.
/// This synchronous seam cannot read authored sources or start discovery.
pub(crate) type RuntimeCatalogPreparer = dyn Fn(
        crate::capabilities::PreparedCapabilityCandidate,
        &RuntimeResourceSnapshot,
    ) -> Result<crate::runtime::resources::PreparedRuntimeResources, String>
    + Send
    + Sync;

// Why the runtime refused to adopt a prepared configuration. (A plain
// comment: this wire type's schema carries no description.)
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AdoptionError {
    Busy,
    NotReady,
    Conflict,
    Failed { diagnostic: String },
}

/// Whether a live runtime could adopt a prepared configuration now.
///
/// Runtime-domain advisory state, published by the runtime through its Runtime
/// Client projection; it is never part of a `ConfigurationApplication`.
/// Adoption always revalidates the native admission gate.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AdoptionEligibility {
    Eligible,
    Busy,
    Unavailable,
}

// The configuration facts an admitted Attempt froze at its admission
// linearization point; later adoption never mutates them. (A plain comment:
// this wire type's schema carries no description.)
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AdmittedConfiguration {
    pub approval_mode: crate::runtime::ApprovalMode,
    pub model_timeout: Option<crate::local_runtime::config::ModelTimeoutPolicyDocument>,
    pub tool_deadline: Option<crate::local_runtime::config::ToolDeadlinePolicyDocument>,
    pub attempt: crate::runtime::identity::AttemptId,
    pub generation: crate::runtime::identity::RuntimeResourceRevision,
    pub model: crate::model::session::SessionModelView,
    pub resources: crate::runtime::capability_inspection::CapabilityInspection,
}

/// One off-side configuration candidate prepared by a specific runtime
/// allocation against one exact Session binding revision.
///
/// Nothing in this value is visible to an admitted Attempt until
/// [`ConversationRuntime::adopt_configuration`](crate::runtime::ConversationRuntime)
/// commits it under the coordinator lock.
pub(crate) struct PreparedConfiguration {
    /// The runtime allocation that prepared this candidate. A candidate from
    /// a replaced runtime can never adopt into its successor.
    pub(crate) owner: Arc<()>,
    /// The Session binding revision this candidate was prepared against.
    pub(crate) baseline: u64,
    /// The complete candidate generation used for comparison and, without a
    /// capability candidate, for adoption itself.
    pub(crate) preview: Arc<RuntimeResourceSnapshot>,
    pub(crate) model: SessionModelState,
    pub(crate) capability: Option<crate::capabilities::PreparedCapabilityCandidate>,
    pub(crate) resources: Option<crate::runtime::resources::PreparedRuntimeResourceData>,
    pub(crate) impact: CacheImpact,
}

impl PreparedConfiguration {
    /// Implicit (non-explicit) adoption is allowed only for a candidate whose
    /// configuration contribution is proven cache-preserving.
    #[must_use]
    pub(crate) fn adopts_implicitly(&self) -> bool {
        self.impact == CacheImpact::Preserved
    }
}

/// The one cache-impact rule of a configuration transition.
///
/// Compares the configuration-induced request shape of the current
/// generation and model with the candidate's. An unavailable shape on either
/// side is never proof of preservation, and a changed context policy is
/// never preserved even when the provider request bytes are identical,
/// because it changes future history projection.
#[must_use]
pub(crate) fn configuration_cache_impact(
    current: &RuntimeResourceSnapshot,
    current_model: &SessionModelState,
    candidate: &RuntimeResourceSnapshot,
    candidate_model: &SessionModelState,
) -> CacheImpact {
    let impact = match (
        current.request_shape(current_model),
        candidate.request_shape(candidate_model),
    ) {
        (Ok(current), Ok(candidate)) => current.compare(&candidate),
        (Err(error), _) | (_, Err(error)) => {
            #[cfg(test)]
            eprintln!("configuration shape unavailable: {error}");
            let _ = error;
            CacheImpact::Unproven
        }
    };
    let context = |snapshot: &RuntimeResourceSnapshot| {
        snapshot
            .configuration()
            .map(|configuration| configuration.config.context)
    };
    if impact == CacheImpact::Preserved && context(current) != context(candidate) {
        CacheImpact::Unproven
    } else {
        impact
    }
}

/// One coherent cut of a runtime's adopted configuration: the generation
/// future Attempts acquire, the Session binding and model intent, the
/// effective approval mode, and the configuration the current Attempt froze.
pub(crate) struct ConfigurationCut {
    pub(crate) binding_revision: u64,
    pub(crate) resources: Arc<RuntimeResourceSnapshot>,
    pub(crate) approval_mode: crate::runtime::ApprovalMode,
    pub(crate) session_model: Option<crate::model::session::SessionModelConfig>,
    pub(crate) effective_model: crate::model::session::SessionModelView,
    pub(crate) admitted_attempt: Option<AdmittedConfiguration>,
}
