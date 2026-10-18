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
use crate::tools::types::ModelToolDefinition;

/// Provider-reported input measured for one exact immutable selected request.
/// The anchor retains its actual input root; it never rebuilds a history-sized
/// identity list or substitutes a hash for exact native input equivalence.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderObservedInput {
    /// Adapter-normalized effective input, including cache input where reported.
    pub input_tokens: u64,
    pub anchor: ObservedAnchor,
}

/// One immutable measured input and the exact non-conversation context it used.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedAnchor {
    surface_revision: SurfaceRevision,
    input: crate::model::SelectedModelInput,
    effective_system_prompt: String,
    tool_definitions: std::sync::Arc<[ModelToolDefinition]>,
}

impl ObservedAnchor {
    /// Retains the producer's exact final selected root, after media projection.
    #[must_use]
    pub fn of_selected(
        surface_revision: SurfaceRevision,
        input: &crate::model::SelectedModelInput,
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> Self {
        #[cfg(test)]
        let _timer = crate::agent::execution::attribution::Timer::new(
            crate::agent::execution::attribution::Phase::ContextObserve,
        );
        #[cfg(test)]
        crate::agent::execution::attribution::record_context(
            crate::agent::execution::attribution::ContextWork {
                anchor_roots: 1,
                ..Default::default()
            },
        );
        Self {
            surface_revision,
            input: input.clone(),
            effective_system_prompt: effective_system_prompt.to_owned(),
            tool_definitions: tool_definitions.into(),
        }
    }

    fn same_context(&self, prompt: &str, tools: &[ModelToolDefinition]) -> bool {
        self.effective_system_prompt == prompt && self.tool_definitions.as_ref() == tools
    }

    /// Exact content and scalar identity, without a collision-based shortcut.
    #[must_use]
    pub fn matches_selected(
        &self,
        revision: SurfaceRevision,
        input: &crate::model::SelectedModelInput,
        prompt: &str,
        tools: &[ModelToolDefinition],
    ) -> bool {
        self.surface_revision == revision
            && self.same_context(prompt, tools)
            && self.input.same_input(input)
    }

    /// The original measured input must remain an exact ordered prefix.
    /// Shared native subtrees are compared by their immutable ownership; distinct
    /// roots use the native exact comparison rather than accepting a digest.
    #[must_use]
    pub fn covered_prefix_model_input(
        &self,
        input: &crate::model::SelectedModelInput,
        prompt: &str,
        tools: &[ModelToolDefinition],
    ) -> Option<usize> {
        (self.same_context(prompt, tools) && input.covers_prefix(&self.input))
            .then_some(self.input.len())
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
/// mutable runtime or durable control state stay outside this boundary.
/// Immutable managed image bytes are part of the visible input: the Attempt's
/// image estimator resolves their validated measurements through its frozen
/// Conversation artifact authority before adding a modality charge. Hypothetical
/// compaction and actual requests therefore estimate identically for identical
/// visible input. Conversation-retention estimates remain canonical-only.
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

    /// Prices the native immutable selection. Arbitrary estimators receive the
    /// same complete ordered input as their slice API; only an implementation
    /// with an exact aggregate rule may override this boundary.
    fn estimate_selected(
        &self,
        messages: &crate::model::SelectedModelInput,
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        #[cfg(test)]
        crate::agent::execution::attribution::record_context(
            crate::agent::execution::attribution::ContextWork {
                estimator_calls: 1,
                materialized_items: messages.len() as u64,
                ..Default::default()
            },
        );
        self.estimate_input(
            &messages.to_vec(),
            effective_system_prompt,
            tool_definitions,
        )
    }

    /// Whether compaction's whole-request prices are monotone for this input.
    ///
    /// Returning true promises that growing the rendered summary prefix cannot
    /// reduce its estimate, and deleting a historical prefix from a request
    /// with fixed core inputs cannot increase either primary or recent prices.
    /// The promise must hold for all complete boundaries, without resource
    /// resampling. Arbitrary estimators conservatively return false.
    fn supports_monotone_compaction(&self, _messages: &[ModelInputMessage]) -> bool {
        false
    }

    /// The same monotonicity question for a shared native selection.
    fn supports_monotone_selected(&self, messages: &crate::model::SelectedModelInput) -> bool {
        self.supports_monotone_compaction(&messages.to_vec())
    }

    /// Price final materialized input without selecting runtime resources again.
    fn estimate_frozen_request(&self, request: &crate::model::ModelRequest) -> u64 {
        self.estimate_selected(
            &request.messages,
            &request.effective_system_prompt,
            &request.tools,
        )
    }

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
/// ([`TokenEstimator::estimate_conversation_input`]) excludes it. This is the
/// text/structure component; the Attempt-bound image estimator adds the
/// conservative dimension-based charge after applying request projection.
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
        let Ok(visible) = messages
            .iter()
            .map(|m| m.view().model_visible())
            .collect::<Result<Vec<_>, _>>()
        else {
            // Unsupported Tool content must never be priced as an empty input.
            return u64::MAX;
        };
        let items = encoded_bytes(&visible);
        let tools = encoded_bytes(tool_definitions);
        // An empty prompt means that no request-time prompt section exists;
        // do not charge the JSON representation's two quote bytes as model
        // input. Non-empty prompts remain part of the frozen deterministic
        // request estimate.
        let system_prompt = if effective_system_prompt.is_empty() {
            0
        } else {
            encoded_bytes(effective_system_prompt)
        };
        items + tools + system_prompt
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
        let Ok(visible) = messages
            .iter()
            .map(|m| crate::model::projected::ModelMessageView::from(m).model_visible())
            .collect::<Result<Vec<_>, _>>()
        else {
            return u64::MAX;
        };
        encoded_bytes(&visible)
    }
}

/// One immutable leaf's exact model-visible JSON. Called only when the native
/// node aggregate is first computed; unsupported Tool projections remain errors.
pub(crate) fn model_visible_item_bytes(message: &ModelInputMessage) -> Option<u64> {
    let visible = message.view().model_visible().ok()?;
    Some(encoded_bytes(&visible))
}

/// Price semantic role/content directly into a counting sink. Message IDs,
/// source digests and projection provenance never enter model token accounting.
fn encoded_bytes(value: &(impl serde::Serialize + ?Sized)) -> u64 {
    struct Count(u64);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len() as u64);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).expect("semantic model content serializes");
    #[cfg(test)]
    ACCOUNTED_BYTES.with(|total| total.set(total.get().saturating_add(count.0)));
    #[cfg(test)]
    crate::agent::execution::attribution::record_context(
        crate::agent::execution::attribution::ContextWork {
            token_bytes: count.0,
            ..Default::default()
        },
    );
    count.0
}
#[cfg(test)]
thread_local! {
    static ACCOUNTED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static ESTIMATOR_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
pub(crate) fn take_accounted_bytes() -> u64 {
    ACCOUNTED_BYTES.with(|total| total.replace(0))
}

#[cfg(test)]
pub(crate) fn take_estimator_calls() -> u64 {
    ESTIMATOR_CALLS.with(|total| total.replace(0))
}

impl TokenEstimator for DefaultTokenEstimator {
    fn supports_monotone_compaction(&self, _messages: &[ModelInputMessage]) -> bool {
        // JSON arrays contribute brackets plus independent item bytes and
        // separators. Prefix deletion only removes nonnegative bytes. Summary
        // text appends only escaped UTF-8 bytes; ceil(bytes/4) is monotone.
        true
    }

    fn supports_monotone_selected(&self, _messages: &crate::model::SelectedModelInput) -> bool {
        true
    }

    fn estimate_input(
        &self,
        messages: &[ModelInputMessage],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        #[cfg(test)]
        ESTIMATOR_CALLS.with(|total| total.set(total.get() + 1));
        bytes_to_tokens(Self::serialized_bytes(
            messages,
            effective_system_prompt,
            tool_definitions,
        ))
    }

    fn estimate_selected(
        &self,
        messages: &crate::model::SelectedModelInput,
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        #[cfg(test)]
        ESTIMATOR_CALLS.with(|total| total.set(total.get() + 1));
        #[cfg(test)]
        crate::agent::execution::attribution::record_context(
            crate::agent::execution::attribution::ContextWork {
                estimator_calls: 1,
                ..Default::default()
            },
        );
        let Some(items) = messages.visible_bytes(0..messages.len()) else {
            return u64::MAX;
        };
        let prompt = if effective_system_prompt.is_empty() {
            0
        } else {
            encoded_bytes(effective_system_prompt)
        };
        // Array brackets and commas are already included. Round once over the
        // complete exact byte total, never once per leaf or subtree.
        bytes_to_tokens(items + encoded_bytes(tool_definitions) + prompt)
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
mod selected_tests;

#[cfg(test)]
mod tests {
    use super::{DefaultTokenEstimator, TokenEstimator, bytes_to_tokens};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::ModelInputMessage;
    use crate::model::input::canonical_input;
    use crate::runtime::identity::{MessageId, ToolId};
    use crate::tools::types::ModelToolDefinition;

    #[test]
    #[allow(clippy::too_many_lines)] // All hidden metadata variants share the same bounded visible fixture.
    fn tool_accounting_ignores_runtime_metadata_and_truncated_tail() {
        use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef, ToolMessageBlock};
        use crate::runtime::identity::ToolCallId;
        let body = |result| {
            ModelInputMessage::Canonical(MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new("result"),
                tool_call_id: ToolCallId::new("call"),
                occurrence: ToolCallOccurrenceRef::new(
                    MessageId::new("assistant"),
                    ContentBlockIndex::new(0),
                ),
                tool_id: ToolId::new("tool"),
                result,
            }))
        };
        let original = crate::scripted_suites::support::fake::success_result(&"x".repeat(100_000));
        let input = body(original.clone());
        let expected =
            DefaultTokenEstimator::serialized_bytes(std::slice::from_ref(&input), "", &[]);
        for index in 0..7 {
            let mut result = original.clone();
            match index {
                0 => result.duration_ms = 9_000_000,
                1 => result.exit_code = Some(-999),
                2 => {
                    result.truncation = Some(crate::tools::types::TruncationState {
                        truncated: true,
                        original_bytes: Some(900_000),
                    });
                }
                3 => {
                    result.workflow =
                        Some(Box::new(crate::runtime::workflow::WorkflowToolIdentity {
                            workflow_id: crate::runtime::workflow::WorkflowId::parse("workflow")
                                .unwrap(),
                            program_digest: "runtime-only".repeat(1000),
                        }));
                }
                4 => result
                    .artifacts
                    .push(crate::message::content::FileReference {
                        artifact_id: crate::runtime::identity::ArtifactId::new("artifact-meta"),
                        name: Some("hidden".repeat(1000)),
                        mime_type: None,
                        description: None,
                    }),
                5 => result
                    .deliveries
                    .push(crate::tools::session_files::SessionFileReference {
                        scope: crate::tools::session_files::SessionFileScope {
                            conversation_id: crate::runtime::identity::ConversationId::generate(),
                            device: "1".into(),
                            inode: "2".into(),
                        },
                        path: "file.txt".into(),
                        name: "hidden".repeat(1000),
                        description: None,
                        mime_type: "text/plain".into(),
                    }),
                _ => result
                    .content
                    .push(crate::tools::types::ToolResultContent::Text(TextBlock {
                        text: "invisible tail".repeat(1000),
                    })),
            }
            assert_eq!(
                original.model_facing_projection(),
                result.model_facing_projection()
            );
            let other = body(result);
            assert_eq!(
                expected,
                DefaultTokenEstimator::serialized_bytes(std::slice::from_ref(&other), "", &[])
            );
            assert_eq!(
                crate::model::types::semantic_messages_fingerprint(std::slice::from_ref(&input)),
                crate::model::types::semantic_messages_fingerprint(&[other])
            );
        }
        let mut diagnostic = original.clone();
        diagnostic.status = crate::tools::types::ToolExecutionStatus::Failed {
            error: "diagnostic".repeat(100_000),
        };
        let mut changed_diagnostic = diagnostic.clone();
        if let crate::tools::types::ToolExecutionStatus::Failed { error } =
            &mut changed_diagnostic.status
        {
            error.push_str("hidden diagnostic tail");
        }
        assert_eq!(
            diagnostic.model_facing_projection(),
            changed_diagnostic.model_facing_projection()
        );
        assert_eq!(
            DefaultTokenEstimator::serialized_bytes(&[body(diagnostic)], "", &[]),
            DefaultTokenEstimator::serialized_bytes(&[body(changed_diagnostic)], "", &[])
        );
        let changed = body(crate::scripted_suites::support::fake::success_result(
            "different visible text",
        ));
        assert_ne!(
            expected,
            DefaultTokenEstimator::serialized_bytes(&[changed], "", &[])
        );
    }

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

    #[test]
    fn original_and_derived_equivalent_content_have_equal_token_cost_without_provenance() {
        let original = user_message("physical", "same semantic body");
        let input = ModelInputMessage::Canonical(original.clone());
        let derived = ModelInputMessage::Derived(crate::model::projected::DerivedModelMessage {
            message: crate::model::projected::ProjectedMessage::from(&original),
            sources: vec![
                crate::model::projected::CanonicalMessageRef::of(
                    &crate::runtime::identity::ConversationId::generate(),
                    &original
                );
                1024
            ],
            checkpoint_sources: Vec::new(),
            transformations: vec![crate::model::projected::SemanticTransformation::Summary],
        });
        assert_eq!(
            DefaultTokenEstimator::serialized_bytes(&[input], "", &[]),
            DefaultTokenEstimator::serialized_bytes(&[derived], "", &[])
        );
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
