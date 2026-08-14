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
/// The engine applies it only when the projection being measured is
/// fingerprint-identical to the observed one; otherwise the measurement is
/// dropped and a deterministic estimate is used instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderObservedInput {
    /// The fingerprint of the projection the provider request used.
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
    /// The deterministic estimated input tokens of one projection, including
    /// non-compacted contributors such as tool definitions and the exact
    /// Agent Status attachment. This is the full request estimate: it feeds
    /// the soft-limit threshold and the hard fit, so the Agent Status
    /// snapshot can itself change the compaction decision.
    fn estimate_input(
        &self,
        projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64;

    /// The deterministic estimated input tokens of one projection's
    /// conversation content only, excluding non-conversation contributors
    /// such as tool definitions and the Agent Status attachment.
    ///
    /// This is the recent-conversation estimate: it measures how much
    /// literal conversation history a retained suffix contributes. Tool
    /// definitions and Agent Status affect the full request estimate, the
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
/// applied over the runtime-owned canonical serialization of the projection
/// items, the tool definitions, and the exact Agent Status attachment, plus
/// the configured per-request contributors. `ceil(x / 4)` is `(bytes + 3) /
/// 4` over `u64`, so every byte counted contributes at most 4 bytes to one
/// token. The formula is intentionally an estimate, never provider usage.
/// Agent Status is real model input, so it participates in the full request
/// estimate; the recent-conversation estimate ([`TokenEstimator::estimate_conversation_input`])
/// excludes it.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultTokenEstimator;

impl DefaultTokenEstimator {
    /// The deterministic serialized bytes of the projection items, the tool
    /// definitions, and the exact Agent Status attachment.
    ///
    /// # Panics
    ///
    /// Panics only if the canonical projection, tool definitions, or status
    /// attachment fail to serialize, which is unreachable for the canonical
    /// runtime-owned types.
    #[must_use]
    pub fn serialized_bytes(
        projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        let items = serde_json::to_vec(&projection.items)
            .expect("canonical projection items serialize")
            .len();
        let tools = serde_json::to_vec(tool_definitions)
            .expect("canonical tool definitions serialize")
            .len();
        let status = projection.agent_status.as_ref().map_or(0, |status| {
            serde_json::to_vec(status)
                .expect("canonical agent status attachment serializes")
                .len()
        });
        let catalog = projection.skill_catalog.as_ref().map_or(0, |catalog| {
            serde_json::to_vec(catalog)
                .expect("canonical skill catalog attachment serializes")
                .len()
        });
        (items + tools + status + catalog) as u64
    }

    /// The deterministic serialized bytes of the projection items only,
    /// excluding tool definitions and the Agent Status attachment.
    ///
    /// # Panics
    ///
    /// Panics only if the canonical projection fails to serialize, which is
    /// unreachable for the canonical runtime-owned types.
    #[must_use]
    pub fn conversation_bytes(projection: &ContextProjection) -> u64 {
        serde_json::to_vec(&projection.items)
            .expect("canonical projection items serialize")
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
        use crate::model::types::{AgentStatusAttachment, SkillCatalogAttachment};
        let projection = crate::context::projection::ContextProjection {
            items: Vec::new(),
            agent_status: None,
            skill_catalog: None,
            estimated_input: crate::runtime::types::TokenMeasurement {
                input_tokens: 0,
                source: crate::runtime::types::TokenMeasurementSource::Estimated,
            },
            checkpoint_generation: None,
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
        // The exact Agent Status attachment is actual model input: it
        // contributes to the full request estimate but never to the
        // recent-conversation estimate.
        let with_status = crate::context::projection::ContextProjection {
            agent_status: Some(AgentStatusAttachment {
                target_message_id: crate::runtime::identity::MessageId::new("msg-inbound-1"),
                rendered:
                    "<system-reminder>\nCurrent time: 2026-08-08T16:31:00+08:00\n</system-reminder>"
                        .to_owned(),
            }),
            ..projection.clone()
        };
        assert!(
            estimator.estimate_input(&with_status, &[])
                > estimator.estimate_input(&projection, &[]),
            "the Agent Status attachment must contribute to the full request estimate"
        );
        assert_eq!(
            estimator.estimate_conversation_input(&with_status),
            estimator.estimate_conversation_input(&projection),
            "the Agent Status attachment must never satisfy keep_recent_tokens"
        );
        // The exact Skill catalog attachment is actual model input: it
        // contributes to the full request estimate but never to the
        // recent-conversation estimate.
        let with_catalog = crate::context::projection::ContextProjection {
            skill_catalog: Some(SkillCatalogAttachment {
                rendered: "## Skills\n\n- pdf: Create PDF documents.\n".to_owned(),
            }),
            ..projection.clone()
        };
        assert!(
            estimator.estimate_input(&with_catalog, &[])
                > estimator.estimate_input(&projection, &[]),
            "the Skill catalog attachment must contribute to the full request estimate"
        );
        assert_eq!(
            estimator.estimate_conversation_input(&with_catalog),
            estimator.estimate_conversation_input(&projection),
            "the Skill catalog attachment must never satisfy keep_recent_tokens"
        );
    }
}
