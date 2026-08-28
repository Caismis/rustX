//! Token accounting and provenance.
//!
//! Every projected input measurement carries explicit provenance
//! ([`TokenMeasurementSource`]). A provider-reported measurement is
//! authoritative for the exact projection the completed provider request
//! measured, and remains authoritative for the prefix it covers of any
//! request context that extends it: the measured part keeps the provider's
//! number and only the canonical messages appended since are estimated.
//! Everything else is a whole-projection deterministic runtime-owned
//! estimate. Estimates are never converted into provider usage
//! ([`ModelUsage`]), and cumulative provider usage snapshots are never
//! summed.
//!
//! [`ModelUsage`]: crate::model::types::ModelUsage
//! [`TokenMeasurementSource`]: crate::runtime::types::TokenMeasurementSource

use crate::conversation::SurfaceRevision;
use crate::message::types::MessageBlock;
use crate::model::input::ModelInputMessage;
use crate::runtime::identity::MessageId;
use crate::tools::types::ModelToolDefinition;

/// An observed provider-reported input measurement, tied to the request
/// context it measured.
///
/// The measurement is authoritative for exactly the request it describes.
/// The engine reuses it in two ways, and never in any other:
///
/// - **exactly**, when the request context being measured is
///   fingerprint-identical to the observed one — the same Surface revision,
///   the same hydrated messages, and the same Effective System Prompt;
/// - **as an anchor**, when the measured context is a *prefix* of the one
///   being measured and the non-conversation input is unchanged. The
///   measured prefix keeps its provider-reported value and only the
///   canonical messages appended since are estimated.
///
/// The second case is what keeps token accounting honest over a long
/// conversation. A whole-conversation estimate compounds estimator error
/// across every message ever sent, so a provider-neutral `bytes / 4`
/// approximation drifts further from the truth the longer the conversation
/// runs — which is exactly when the soft-limit decision matters most.
/// Anchoring confines that error to the messages added since the last
/// completed request. Otherwise the measurement is dropped and a full
/// deterministic estimate is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderObservedInput {
    /// The fingerprint of the request context the provider request used.
    pub fingerprint: u64,
    /// The reported `ModelUsage.input_tokens` of that request.
    ///
    /// This is the adapter-normalized *effective* input of the request,
    /// including provider cache-read and cache-creation input where the
    /// provider reports those categories separately.
    pub input_tokens: u64,
    /// The structural identity of the measured request context.
    ///
    /// Absent for a measurement recorded without one, which can then only be
    /// reused on an exact fingerprint match.
    pub anchor: Option<ObservedAnchor>,
}

/// The structural identity of one measured request context.
///
/// A hash cannot answer "is this a prefix of that", so the anchor keeps the
/// ordered canonical identities the measured request projected, alongside a
/// fingerprint of the non-conversation input the measurement already covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedAnchor {
    /// The ordered canonical message identities of the measured request.
    pub message_ids: Vec<MessageId>,
    /// The ordered identities of all provider-neutral input items. A
    /// request-only item has a private fingerprint, never a canonical id.
    input_items: Vec<ObservedInputIdentity>,
    /// The fingerprint of the non-conversation request input the measurement
    /// already accounts for: the exact Effective System Prompt and the
    /// compiled tool definitions.
    ///
    /// Their cost is inside `input_tokens`, so anchoring is sound only while
    /// they are unchanged. When either changes the anchor is refused rather
    /// than patched up with a guessed delta.
    pub non_conversation_fingerprint: u64,
}

impl ObservedAnchor {
    /// The anchor of one exact request context.
    #[must_use]
    pub fn of(
        messages: &[MessageBlock],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> Self {
        Self {
            message_ids: messages
                .iter()
                .map(|message| message.id().clone())
                .collect(),
            input_items: messages
                .iter()
                .map(|message| ObservedInputIdentity::Canonical(message.id().clone()))
                .collect(),
            non_conversation_fingerprint: non_conversation_fingerprint(
                effective_system_prompt,
                tool_definitions,
            ),
        }
    }

    /// Creates an anchor for an already ordered mixed request.
    #[must_use]
    pub fn of_model_input(
        messages: &[ModelInputMessage],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> Self {
        Self {
            message_ids: messages
                .iter()
                .filter_map(ModelInputMessage::canonical_id)
                .cloned()
                .collect(),
            input_items: messages.iter().map(input_identity).collect(),
            non_conversation_fingerprint: non_conversation_fingerprint(
                effective_system_prompt,
                tool_definitions,
            ),
        }
    }

    /// Whether the measured request contained request-only context.
    #[must_use]
    pub fn has_request_only(&self) -> bool {
        self.input_items
            .iter()
            .any(|item| matches!(item, ObservedInputIdentity::RequestOnly(_)))
    }

    /// Whether this anchor is a prefix of `messages` under the same
    /// non-conversation input, and if so how many messages it covers.
    ///
    /// Equal length is a prefix: a context identical to the measured one
    /// except for its Surface revision is still fully measured.
    #[must_use]
    pub fn covered_prefix(
        &self,
        messages: &[MessageBlock],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> Option<usize> {
        if self.has_request_only() {
            return None;
        }
        if self.non_conversation_fingerprint
            != non_conversation_fingerprint(effective_system_prompt, tool_definitions)
        {
            return None;
        }
        if self.message_ids.len() > messages.len() {
            return None;
        }
        self.message_ids
            .iter()
            .zip(messages)
            .all(|(measured, current)| measured == current.id())
            .then_some(self.message_ids.len())
    }

    /// Whether this anchor is an ordered prefix of a mixed request.
    #[must_use]
    pub fn covered_prefix_model_input(
        &self,
        messages: &[ModelInputMessage],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> Option<usize> {
        if self.non_conversation_fingerprint
            != non_conversation_fingerprint(effective_system_prompt, tool_definitions)
        {
            return None;
        }
        if self.input_items.len() > messages.len() {
            return None;
        }
        self.input_items
            .iter()
            .zip(messages)
            .all(|(measured, current)| measured == &input_identity(current))
            .then_some(self.input_items.len())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ObservedInputIdentity {
    Canonical(MessageId),
    RequestOnly(u64),
}

fn input_identity(message: &ModelInputMessage) -> ObservedInputIdentity {
    match message {
        ModelInputMessage::Canonical(message) => {
            ObservedInputIdentity::Canonical(message.id().clone())
        }
        ModelInputMessage::RequestOnly(_) => {
            let bytes = serde_json::to_vec(message).expect("model input serializes");
            let mut hash = 0xcbf2_9ce4_8422_2325_u64;
            for byte in bytes {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            ObservedInputIdentity::RequestOnly(hash)
        }
    }
}

/// The deterministic fingerprint of the non-conversation input of one
/// request: the exact Effective System Prompt and the compiled tool
/// definitions.
///
/// # Panics
///
/// Panics only if the canonical tool definitions fail to serialize, which is
/// unreachable for the runtime-owned types.
#[must_use]
pub fn non_conversation_fingerprint(
    effective_system_prompt: &str,
    tool_definitions: &[ModelToolDefinition],
) -> u64 {
    let bytes =
        effective_system_prompt.as_bytes().iter().copied().chain(
            serde_json::to_vec(tool_definitions).expect("canonical tool definitions serialize"),
        );
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The deterministic identity of the non-conversation, non-prompt part of
/// one provider request: its resolved invocation configuration and the
/// opaque provider continuation state it carries.
///
/// A provider-reported input measurement is a measurement of *that* request,
/// and both of these change what the provider counted without changing a
/// single canonical message. A continuation is the sharp case: under a
/// stored-continuation protocol the provider bills the whole referenced
/// prior generation as input, so a request whose visible message list is a
/// prefix of another can still have been measured against a completely
/// different body of context. Reusing the earlier number as a prefix anchor
/// would then under-count by exactly the amount that matters.
///
/// Binding every measurement to this identity is what keeps the anchor
/// honest: when the identity changes the measurement is dropped, never
/// scaled or patched with a guessed delta.
///
/// # Panics
///
/// Panics only if the runtime-owned invocation configuration or
/// continuation state fails to serialize, which is unreachable for these
/// types.
#[must_use]
pub fn request_identity_fingerprint(
    invocation: &crate::model::ModelInvocationConfig,
    continuation: Option<&crate::runtime::continuation::ProviderContinuationState>,
) -> u64 {
    let invocation =
        serde_json::to_vec(invocation).expect("the resolved invocation configuration serializes");
    let continuation =
        serde_json::to_vec(&continuation).expect("the provider continuation state serializes");
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in invocation.into_iter().chain(continuation) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Fingerprints one exact ordered request input while retaining the Surface
/// revision component used by the existing provider-measurement contract.
/// Request-only items participate by value; they never need or receive a
/// canonical message identity.
///
/// # Panics
///
/// Panics only if the provider-neutral input or system prompt cannot be
/// serialized, which cannot occur for their serde-backed UTF-8 value types.
#[must_use]
pub fn model_input_fingerprint(
    surface_revision: SurfaceRevision,
    messages: &[ModelInputMessage],
    effective_system_prompt: &str,
) -> u64 {
    let bytes = serde_json::to_vec(&surface_revision)
        .expect("surface revision serializes")
        .into_iter()
        .chain(serde_json::to_vec(messages).expect("provider-neutral model input serializes"))
        .chain(serde_json::to_vec(effective_system_prompt).expect("system prompt serializes"));
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The deterministic input-token estimator boundary.
///
/// The engine never hard-codes a per-model token catalog; estimation is a
/// pluggable, deterministic runtime-owned concern so tests can supply exact
/// scripted token weights and production can use the default provider-neutral
/// fallback ([`DefaultTokenEstimator`]).
///
/// Estimation sees only the exact provider-visible request input: the ordered
/// canonical/request-only messages, the exact Effective System Prompt, and
/// the tool definitions. `SurfaceRevision`, token-measurement provenance, and
/// any other runtime or durable store state are deliberately outside this
/// boundary, so a custom estimator can never make token cost depend on them —
/// a hypothetical compaction candidate and the actual post-compaction request
/// therefore estimate identically whenever their provider-visible inputs are
/// identical. Conversation-retention estimates remain canonical-only.
pub trait TokenEstimator: Send + Sync {
    /// The deterministic estimated input tokens of one request's
    /// provider-visible input, including non-compacted contributors such as
    /// tool definitions and the exact Effective System Prompt. This is the
    /// full request estimate: it feeds the soft-limit threshold and the hard
    /// fit.
    fn estimate_input(
        &self,
        messages: &[ModelInputMessage],
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
    dyn Fn(&[ModelInputMessage], &str, &[ModelToolDefinition]) -> u64 + Send + Sync;

/// The default provider-neutral fallback estimator.
///
/// The frozen formula is:
///
/// ```text
/// ceil(deterministic UTF-8 serialized bytes / 4)
/// ```
///
/// applied over the runtime-owned serialization of the ordered
/// canonical/request-only messages, the tool definitions, and the exact
/// Effective System Prompt.
/// `ceil(x / 4)` is `(bytes + 3) / 4` over `u64`, so every byte counted
/// contributes at most 4 bytes to one token. The formula is intentionally an
/// estimate, never provider usage. The Effective System Prompt participates
/// in the full request estimate; the recent-conversation estimate
/// ([`TokenEstimator::estimate_conversation_input`]) excludes it.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultTokenEstimator;

impl DefaultTokenEstimator {
    /// The deterministic serialized bytes of the provider-neutral input, the
    /// tool definitions, and the exact Effective System Prompt.
    ///
    /// # Panics
    ///
    /// Panics only if the canonical messages, tool definitions, or system
    /// prompt fail to serialize, which is unreachable for the canonical
    /// runtime-owned types.
    #[must_use]
    pub fn serialized_bytes(
        messages: &[ModelInputMessage],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        let items = serde_json::to_vec(messages)
            .expect("provider-neutral model input serializes")
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
        messages: &[ModelInputMessage],
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
/// provider-visible request input — canonical/request-only messages, Effective
/// System Prompt, and tools — so scripted estimation can never depend on
/// `SurfaceRevision` or token-measurement provenance.
pub struct ClosureTokenEstimator {
    function: Box<EstimatorFunction>,
}

impl ClosureTokenEstimator {
    /// Creates a scripted estimator from a deterministic function over the
    /// exact provider-visible request input.
    #[must_use]
    pub fn new(
        function: impl Fn(&[ModelInputMessage], &str, &[ModelToolDefinition]) -> u64
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            function: Box::new(function),
        }
    }
}

impl TokenEstimator for ClosureTokenEstimator {
    fn estimate_input(
        &self,
        messages: &[ModelInputMessage],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        (self.function)(messages, effective_system_prompt, tool_definitions)
    }

    fn estimate_conversation_input(&self, messages: &[MessageBlock]) -> u64 {
        let messages = crate::model::input::canonical_input(messages);
        (self.function)(&messages, "", &[])
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
    use crate::model::input::canonical_input;
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
        let canonical_messages = vec![user_message("msg-1", "hello")];
        let messages = canonical_input(&canonical_messages);

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
            estimator.estimate_conversation_input(&canonical_messages),
            bytes_to_tokens(DefaultTokenEstimator::conversation_bytes(
                &canonical_messages
            )),
            "conversation-only estimation depends only on the ordered messages"
        );
    }
}
