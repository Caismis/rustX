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

use crate::message::types::MessageBlock;
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
///
/// Estimation sees only the exact provider-visible request input: the ordered
/// canonical messages, the exact Effective System Prompt, and the tool
/// definitions. `SurfaceRevision`, token-measurement provenance, and any other
/// runtime or durable store state are deliberately outside this boundary, so
/// a custom estimator can never make token cost depend on them — a
/// hypothetical compaction candidate and the actual post-compaction request
/// therefore estimate identically whenever their provider-visible inputs are
/// identical.
pub trait TokenEstimator: Send + Sync {
    /// The deterministic estimated input tokens of one request's
    /// provider-visible input, including non-compacted contributors such as
    /// tool definitions and the exact Effective System Prompt. This is the
    /// full request estimate: it feeds the soft-limit threshold and the hard
    /// fit.
    fn estimate_input(
        &self,
        messages: &[MessageBlock],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64;

    /// The deterministic estimated input tokens of the conversation content
    /// only, excluding non-conversation contributors such as tool definitions
    /// and the Effective System Prompt.
    ///
    /// This is the recent-conversation estimate: it measures how much
    /// literal conversation history a retained suffix contributes. Tool
    /// definitions and admitted Runtime context affect the full request estimate, the
    /// threshold, and the hard fit, but they must never count toward
    /// satisfying the `keep_recent_tokens` retention target.
    fn estimate_conversation_input(&self, messages: &[MessageBlock]) -> u64;
}

/// The deterministic function behind a [`ClosureTokenEstimator`].
pub type EstimatorFunction =
    dyn Fn(&[MessageBlock], &str, &[ModelToolDefinition]) -> u64 + Send + Sync;

/// The default provider-neutral fallback estimator.
///
/// The frozen formula is:
///
/// ```text
/// ceil(deterministic UTF-8 serialized bytes / 4)
/// ```
///
/// applied over the runtime-owned canonical serialization of the canonical
/// messages, the tool definitions, and the exact Effective System Prompt.
/// `ceil(x / 4)` is `(bytes + 3) / 4` over `u64`, so every byte counted
/// contributes at most 4 bytes to one token. The formula is intentionally an
/// estimate, never provider usage. The Effective System Prompt participates
/// in the full request estimate; the recent-conversation estimate
/// ([`TokenEstimator::estimate_conversation_input`]) excludes it.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultTokenEstimator;

impl DefaultTokenEstimator {
    /// The deterministic serialized bytes of the canonical messages, the
    /// tool definitions, and the exact Effective System Prompt.
    ///
    /// # Panics
    ///
    /// Panics only if the canonical messages, tool definitions, or system
    /// prompt fail to serialize, which is unreachable for the canonical
    /// runtime-owned types.
    #[must_use]
    pub fn serialized_bytes(
        messages: &[MessageBlock],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        let items = serde_json::to_vec(messages)
            .expect("canonical messages serialize")
            .len();
        let tools = serde_json::to_vec(tool_definitions)
            .expect("canonical tool definitions serialize")
            .len();
        // An empty prompt means that no request-time prompt section exists;
        // do not charge the JSON representation's two quote bytes as model
        // input. Non-empty prompts remain part of the frozen deterministic
        // request estimate.
        let system_prompt = if effective_system_prompt.is_empty() {
            0
        } else {
            serde_json::to_vec(effective_system_prompt)
                .expect("effective system prompt serializes")
                .len()
        };
        (items + tools + system_prompt) as u64
    }

    /// The deterministic serialized bytes of the canonical messages only,
    /// excluding tool definitions and the Effective System Prompt.
    ///
    /// # Panics
    ///
    /// Panics only if the canonical messages fail to serialize, which is
    /// unreachable for the canonical runtime-owned types.
    #[must_use]
    pub fn conversation_bytes(messages: &[MessageBlock]) -> u64 {
        serde_json::to_vec(messages)
            .expect("canonical messages serialize")
            .len() as u64
    }
}

impl TokenEstimator for DefaultTokenEstimator {
    fn estimate_input(
        &self,
        messages: &[MessageBlock],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        bytes_to_tokens(Self::serialized_bytes(
            messages,
            effective_system_prompt,
            tool_definitions,
        ))
    }

    fn estimate_conversation_input(&self, messages: &[MessageBlock]) -> u64 {
        bytes_to_tokens(Self::conversation_bytes(messages))
    }
}

/// A scripted estimator backed by an arbitrary deterministic function.
///
/// Tests use this to supply exact token weights and to prove that the
/// engine's decisions (threshold triggers, cut selection, retention) follow
/// the weights rather than raw message counts. The function receives only the
/// provider-visible request input — messages, Effective System Prompt, and
/// tools — so scripted estimation can never depend on `SurfaceRevision` or
/// token-measurement provenance.
pub struct ClosureTokenEstimator {
    function: Box<EstimatorFunction>,
}

impl ClosureTokenEstimator {
    /// Creates a scripted estimator from a deterministic function over the
    /// exact provider-visible request input.
    #[must_use]
    pub fn new(
        function: impl Fn(&[MessageBlock], &str, &[ModelToolDefinition]) -> u64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            function: Box::new(function),
        }
    }
}

impl TokenEstimator for ClosureTokenEstimator {
    fn estimate_input(
        &self,
        messages: &[MessageBlock],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        (self.function)(messages, effective_system_prompt, tool_definitions)
    }

    fn estimate_conversation_input(&self, messages: &[MessageBlock]) -> u64 {
        (self.function)(messages, "", &[])
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
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{MessageId, ToolId};
    use crate::tools::types::ModelToolDefinition;

    fn user_message(id: &str, text: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    fn tool_definition() -> ModelToolDefinition {
        ModelToolDefinition {
            id: ToolId::new("tool-bash"),
            name: "bash".to_owned(),
            description: "Run a shell command".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

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
    /// counts messages, tool definitions, and the Effective System Prompt as
    /// full-request input while never counting the Effective System Prompt
    /// toward the conversation-only estimate.
    #[test]
    fn default_estimator_sees_only_provider_visible_input() {
        let estimator = DefaultTokenEstimator;
        let messages = vec![user_message("msg-1", "hello")];

        // Messages affect the estimate.
        assert!(estimator.estimate_input(&messages, "", &[]) > 0);
        assert!(
            estimator.estimate_input(&[], "", &[]) < estimator.estimate_input(&messages, "", &[]),
            "messages must contribute to the request estimate"
        );

        // Tool definitions affect the full input estimate.
        let without_tools = estimator.estimate_input(&messages, "", &[]);
        let with_tools = estimator.estimate_input(&messages, "", &[tool_definition()]);
        assert!(
            with_tools > without_tools,
            "tool definitions must contribute to the planned request estimate"
        );

        // The Effective System Prompt affects the full input estimate...
        let without_prompt = estimator.estimate_input(&messages, "", &[]);
        let with_prompt =
            estimator.estimate_input(&messages, "runtime status\n\nskill guidance", &[]);
        assert!(
            with_prompt > without_prompt,
            "the Effective System Prompt must contribute to the full request estimate"
        );

        // ...but it is not an input to conversation-only estimation at all:
        // the conversation estimate is a pure function of the ordered
        // messages, so the Effective System Prompt can never satisfy
        // `keep_recent_tokens`.
        assert_eq!(
            estimator.estimate_conversation_input(&messages),
            bytes_to_tokens(DefaultTokenEstimator::conversation_bytes(&messages)),
            "conversation-only estimation depends only on the ordered messages"
        );
    }
}
