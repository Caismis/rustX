//! Token accounting and provenance.
//!
//! Every projected input measurement carries explicit provenance
//! ([`TokenMeasurementSource`]): a provider-reported measurement is
//! authoritative only for the exact projection the completed provider
//! request measured; everything else is a deterministic runtime-owned
//! estimate. Estimates are never converted into provider usage
//! ([`ModelUsage`]), and cumulative provider usage snapshots are never
//! summed.
//!
//! [`ModelUsage`]: crate::model::types::ModelUsage
//! [`TokenMeasurementSource`]: crate::runtime::types::TokenMeasurementSource

use crate::context::projection::ContextProjection;
use crate::tools::types::ModelToolDefinition;

/// An observed provider-reported input measurement, tied to the exact
/// projection it measured.
///
/// The engine applies it only when the request context being measured is
/// fingerprint-identical to the observed one — the same Surface revision,
/// the same hydrated messages, and the same Effective System Prompt; otherwise the
/// measurement is dropped and a deterministic estimate is used instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderObservedInput {
    /// The fingerprint of the request context the provider request used.
    pub fingerprint: u64,
    /// The reported `ModelUsage.input_tokens` of that request.
    pub input_tokens: u64,
}

/// The deterministic input-token estimator boundary.
///
/// The engine never hard-codes a per-model token catalog; estimation is a
/// pluggable, deterministic runtime-owned concern so tests can supply exact
/// scripted token weights and production can use the default provider-neutral
/// fallback ([`DefaultTokenEstimator`]).
pub trait TokenEstimator: Send + Sync {
    /// The deterministic estimated input tokens of one request context,
    /// including
    /// non-compacted contributors such as tool definitions and the exact
    /// Effective System Prompt. This is the full request estimate: it feeds
    /// the soft-limit threshold and the hard fit.
    fn estimate_input(
        &self,
        projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64;

    /// The deterministic estimated input tokens of one projection's
    /// conversation content only, excluding non-conversation contributors
    /// such as tool definitions and the Effective System Prompt.
    ///
    /// This is the recent-conversation estimate: it measures how much
    /// literal conversation history a retained suffix contributes. Tool
    /// definitions and admitted Runtime context affect the full request estimate, the
    /// threshold, and the hard fit, but they must never count toward
    /// satisfying the `keep_recent_tokens` retention target.
    fn estimate_conversation_input(&self, projection: &ContextProjection) -> u64;
}

/// The deterministic function behind a [`ClosureTokenEstimator`].
pub type EstimatorFunction =
    dyn Fn(&ContextProjection, &[ModelToolDefinition]) -> u64 + Send + Sync;

/// The default provider-neutral fallback estimator.
///
/// The frozen formula is:
///
/// ```text
/// ceil(deterministic UTF-8 serialized bytes / 4)
/// ```
///
/// applied over the runtime-owned canonical serialization of the projected
/// canonical messages, the tool definitions, and the exact Effective System
/// Prompt. `ceil(x / 4)` is `(bytes + 3) / 4` over `u64`, so every byte counted
/// contributes at most 4 bytes to one token. The formula is intentionally an
/// estimate, never provider usage. The Effective System Prompt participates
/// in the full request estimate; the recent-conversation estimate
/// ([`TokenEstimator::estimate_conversation_input`]) excludes it.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultTokenEstimator;

impl DefaultTokenEstimator {
    /// The deterministic serialized bytes of the projected canonical
    /// messages, the tool definitions, and the exact Effective System Prompt.
    ///
    /// # Panics
    ///
    /// Panics only if the canonical projection, tool definitions, or system
    /// prompt fail to serialize, which is unreachable for the canonical
    /// runtime-owned types.
    #[must_use]
    pub fn serialized_bytes(
        projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        let items = serde_json::to_vec(&projection.messages)
            .expect("canonical projection messages serialize")
            .len();
        let tools = serde_json::to_vec(tool_definitions)
            .expect("canonical tool definitions serialize")
            .len();
        // An empty prompt means that no request-time prompt section exists;
        // do not charge the JSON representation's two quote bytes as model
        // input. Non-empty prompts remain part of the frozen deterministic
        // request estimate.
        let system_prompt = if projection.effective_system_prompt.is_empty() {
            0
        } else {
            serde_json::to_vec(&projection.effective_system_prompt)
                .expect("effective system prompt serializes")
                .len()
        };
        (items + tools + system_prompt) as u64
    }

    /// The deterministic serialized bytes of the projected canonical
    /// messages only, excluding tool definitions and the Effective System
    /// Prompt.
    ///
    /// # Panics
    ///
    /// Panics only if the canonical projection fails to serialize, which is
    /// unreachable for the canonical runtime-owned types.
    #[must_use]
    pub fn conversation_bytes(projection: &ContextProjection) -> u64 {
        serde_json::to_vec(&projection.messages)
            .expect("canonical projection messages serialize")
            .len() as u64
    }
}

impl TokenEstimator for DefaultTokenEstimator {
    fn estimate_input(
        &self,
        projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        bytes_to_tokens(Self::serialized_bytes(projection, tool_definitions))
    }

    fn estimate_conversation_input(&self, projection: &ContextProjection) -> u64 {
        bytes_to_tokens(Self::conversation_bytes(projection))
    }
}

/// A scripted estimator backed by an arbitrary deterministic function.
///
/// Tests use this to supply exact token weights and to prove that the
/// engine's decisions (threshold triggers, cut selection, retention) follow
/// the weights rather than raw message counts.
pub struct ClosureTokenEstimator {
    function: Box<EstimatorFunction>,
}

impl ClosureTokenEstimator {
    /// Creates a scripted estimator from a deterministic function.
    #[must_use]
    pub fn new(
        function: impl Fn(&ContextProjection, &[ModelToolDefinition]) -> u64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            function: Box::new(function),
        }
    }
}

impl TokenEstimator for ClosureTokenEstimator {
    fn estimate_input(
        &self,
        projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        (self.function)(projection, tool_definitions)
    }

    fn estimate_conversation_input(&self, projection: &ContextProjection) -> u64 {
        (self.function)(projection, &[])
    }
}

/// `ceil(bytes / 4)`: every four deterministic UTF-8 serialized bytes count
/// as one estimated token, with any remainder counting as one more.
#[must_use]
pub const fn bytes_to_tokens(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::{DefaultTokenEstimator, TokenEstimator, bytes_to_tokens};

    /// The frozen estimator formula: `ceil(bytes / 4)`.
    #[test]
    fn bytes_to_tokens_is_ceil_division_by_four() {
        assert_eq!(bytes_to_tokens(0), 0);
        assert_eq!(bytes_to_tokens(1), 1);
        assert_eq!(bytes_to_tokens(3), 1);
        assert_eq!(bytes_to_tokens(4), 1);
        assert_eq!(bytes_to_tokens(5), 2);
        assert_eq!(bytes_to_tokens(8), 2);
        assert_eq!(bytes_to_tokens(9), 3);
    }

    /// The default estimator maps the same input to the same estimate and
    /// counts tool definitions as part of the input.
    #[test]
    fn default_estimator_is_deterministic_and_includes_tools() {
        let projection = crate::context::projection::ContextProjection {
            surface_revision: crate::conversation::SurfaceRevision::INITIAL,
            messages: Vec::new(),
            effective_system_prompt: String::new(),
            estimated_input: crate::runtime::types::TokenMeasurement {
                input_tokens: 0,
                source: crate::runtime::types::TokenMeasurementSource::Estimated,
            },
        };
        let estimator = DefaultTokenEstimator;
        let without_tools = estimator.estimate_input(&projection, &[]);
        assert_eq!(estimator.estimate_input(&projection, &[]), without_tools);
        let with_tools = estimator.estimate_input(
            &projection,
            &[crate::tools::types::ModelToolDefinition {
                id: crate::runtime::identity::ToolId::new("tool-bash"),
                name: "bash".to_owned(),
                description: "Run a shell command".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
        );
        assert!(
            with_tools > without_tools,
            "tool definitions must contribute to the planned request estimate"
        );
        let with_prompt = crate::context::projection::ContextProjection {
            effective_system_prompt: "runtime status\n\nskill guidance".to_owned(),
            ..projection.clone()
        };
        assert!(
            estimator.estimate_input(&with_prompt, &[])
                > estimator.estimate_input(&projection, &[]),
            "the Effective System Prompt must contribute to the full request estimate"
        );
        assert_eq!(
            estimator.estimate_conversation_input(&with_prompt),
            estimator.estimate_conversation_input(&projection),
            "the Effective System Prompt must never satisfy keep_recent_tokens"
        );
    }
}
