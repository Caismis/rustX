//! Frozen provider-independent Request Snapshots (Issue #55).
//!
//! A Request Snapshot owns the ordered frozen input of one actually started
//! primary [`ModelRequest`]. Original input uses immutable scoped Ledger references;
//! request-time Derived values and selected image bytes are frozen by value. A terminally unresolved publication may contribute one
//! explicitly request-only carryover value, also frozen here by value and
//! without a canonical `MessageId`. Reconstruction never consults live model
//! configuration, Skills, contributors, filesystem state, pending carryover,
//! Publication Audit state, or runtime status.

use serde::{Deserialize, Serialize};

use crate::context::assembly::{AcceptedSystemSection, ContextGeneration};
#[cfg(test)]
use crate::conversation::SurfaceRevision;
use crate::conversation::{ConversationError, ConversationState};
use crate::model::input::{
    ModelInputMessage, RenderedUnresolvedOutputCarryover, RequestOnlyInsertionAnchor,
};
use crate::model::invocation::ModelInvocationConfig;
use crate::model::types::ModelRequest;
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::{
    AttemptId, AuxiliaryCallId, CapabilityRevision, MessageId, PublicationStreamId, RequestId,
    TurnId,
};
use crate::tools::types::ModelToolDefinition;

/// The identity of one actual primary request attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestIdentity {
    /// The attempt that owns this request.
    pub attempt_id: AttemptId,
    /// The logical turn/step within that attempt.
    pub turn: TurnId,
    /// The shared monotonically increasing actual-request ordinal within the
    /// logical model step. Zero is the initial request; transient and
    /// context-overflow recovery requests share the same sequence.
    pub retry_number: u32,
}

/// Ownership of one actual model request; identity grants no execution permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelRequestOwner {
    /// A primary request owned by an Agent attempt.
    Agent { identity: RequestIdentity },
    /// One physical request within a logical auxiliary call.
    Auxiliary {
        call_id: AuxiliaryCallId,
        /// Zero is the initial request. Physical retries have distinct request
        /// identities and advance this ordinal within the same auxiliary call.
        ordinal: u32,
    },
}

impl RequestIdentity {
    /// Derives the stable durable identity of this actual request.
    #[must_use]
    pub fn request_id(&self) -> RequestId {
        RequestId::new(format!(
            "request:{}:{}:{}:{}:{}",
            self.attempt_id.as_str().len(),
            self.attempt_id,
            self.turn.as_str().len(),
            self.turn,
            self.retry_number
        ))
    }

    /// Derives the provisional Assistant message identity frozen for this
    /// request generation.
    ///
    /// The Request Snapshot is the authority for this mapping. Publication
    /// streams and canonical acceptance must use the value frozen here rather
    /// than deriving a second identity from live Agent Loop state.
    #[must_use]
    pub fn provisional_message_id(&self) -> MessageId {
        if self.retry_number == 0 {
            MessageId::new(format!("{}-agent-{}", self.attempt_id, self.turn))
        } else {
            MessageId::new(format!(
                "{}-agent-{}-retry-{}",
                self.attempt_id, self.turn, self.retry_number
            ))
        }
    }
}

pub use crate::context::contribution::ContributionStart;

/// A provider-independent frozen request boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestSnapshot {
    /// Exact prepared structured output, omitted from routine observations.
    pub response_schema: Option<super::structured::PreparedResponseSchema>,
    /// Associations in actual-request coordinates, distinct from checkpoint and physical call coordinates.
    pub tool_associations: crate::model::occurrence::FrozenToolAssociations,
    pub history_cut: crate::durable::projection::RequestHistoryCut,
    /// Ordered frozen input. Reconstruction never selects history or transforms it.
    pub input: crate::model::SelectedInputManifest,
    /// Image bytes selected before start, retained for exact reconstruction.
    pub images: crate::model::images::ResolvedImages,
    /// Semantic input identity, independent of actual-request identities.
    pub semantic_input_fingerprint: String,
    /// Stable durable identity of this actual request.
    pub request_id: RequestId,
    /// Request identity.
    pub identity: RequestIdentity,
    /// The provisional Assistant message identity frozen with this request.
    /// Publication opening and canonical acceptance must use this exact value.
    pub provisional_message_id: MessageId,
    /// Canonical dependency frontier captured with request-start Context admission.
    /// Historical selection is independently frozen by `history_cut`.
    pub canonical_frontier: crate::context::resources::LedgerFrontier,
    /// The exact request-time rendered Effective System Prompt.
    pub effective_system_prompt: String,
    /// Exact ordered System-section result from which the prompt was
    /// rendered. No historical discovery or extension logic is rerun.
    pub system_sections: Vec<AcceptedSystemSection>,
    /// Process-local resource generation observed by the admitted attempt.
    pub runtime_resource_revision: crate::runtime::RuntimeResourceRevision,
    /// The effective provider-neutral model invocation values.
    pub invocation: ModelInvocationConfig,
    /// The model context limit frozen for this request.
    pub context_window_tokens: u64,
    /// The selected Model Profile identity, when any.
    pub profile: Option<crate::model::catalog::ModelProfileId>,
    /// The effective semantic reasoning state.
    pub reasoning_enabled: bool,
    /// The exact effective tool definitions/capability view used by this
    /// request. The capability revision is retained for audit, while the
    /// definitions are frozen by value because capability revision metadata
    /// is not a separate historical definition lookup authority.
    pub tool_definitions: Vec<ModelToolDefinition>,
    /// The immutable capability generation observed at admission.
    pub capability_revision: CapabilityRevision,
    /// The accepted context contributor generation that explains assembly.
    pub context_generation: ContextGeneration,
    /// Provider continuation state, if this request used one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ProviderContinuationState>,
    pub continuation_source: Option<crate::runtime::continuation::AcceptedOutputContinuationSource>,
    /// The exact ordered request-scoped context `MessageId`s this request
    /// commits atomically with its start (Issue #12, M9b). Frozen so an
    /// idempotent retry of `commit_model_turn_start` proves exact ordered
    /// context equality — the complete ordered set, never just "every
    /// supplied message exists and matches".
    pub request_context_ids: Vec<MessageId>,
    /// The source identity frozen when this logical step was prepared. This
    /// remains present even when fit degradation omits the request-only body.
    /// It is provenance, not a canonical message identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_output_carryover_source: Option<PublicationStreamId>,
    /// The exact admitted request-only carryover representation, if the
    /// degradation ladder retained one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_output_carryover: Option<RenderedUnresolvedOutputCarryover>,
    /// The exact canonical insertion anchor of the frozen carryover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_output_carryover_anchor: Option<RequestOnlyInsertionAnchor>,
    /// Frozen accepted contributions visible in this actual request.
    pub contributions: Vec<ContributionStart>,
}

/// A historical reconstruction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestReconstructionError {
    /// Surface or Ledger reconstruction failed.
    Conversation(String),
}

impl core::fmt::Display for RequestReconstructionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Conversation(detail) => {
                write!(
                    f,
                    "historical request conversation reconstruction failed: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for RequestReconstructionError {}

impl From<ConversationError> for RequestReconstructionError {
    fn from(error: ConversationError) -> Self {
        Self::Conversation(error.to_string())
    }
}

const MAX_FROZEN_INPUT_ITEMS: usize = 8_192;
const MAX_FROZEN_DERIVED_BYTES: usize = 8 * 1024 * 1024;
const MAX_FROZEN_PROVENANCE_REFS: usize = 1_024;
const MAX_FROZEN_IMAGE_BYTES: usize = 16 * 256 * 1024;

/// Counts serializer output without allocating a serialized copy of a body.
struct InputByteBudget {
    remaining: usize,
}
impl std::io::Write for InputByteBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("frozen Derived input exceeds byte budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn validate_frozen_budget(request: &ModelRequest) -> Result<(), String> {
    if request.messages.len() > MAX_FROZEN_INPUT_ITEMS {
        return Err("frozen input exceeds item budget".into());
    }
    if request
        .images
        .values()
        .try_fold(0usize, |total, bytes| total.checked_add(bytes.len()))
        .is_none_or(|total| total > MAX_FROZEN_IMAGE_BYTES)
    {
        return Err("frozen input exceeds image byte budget".into());
    }
    let mut bytes = InputByteBudget {
        remaining: MAX_FROZEN_DERIVED_BYTES,
    };
    let mut provenance = MAX_FROZEN_PROVENANCE_REFS;
    for message in &request.messages {
        if let ModelInputMessage::Derived(derived) = message {
            provenance = provenance
                .checked_sub(
                    derived
                        .sources
                        .len()
                        .saturating_add(derived.checkpoint_sources.len()),
                )
                .ok_or("frozen input exceeds provenance budget")?;
            serde_json::to_writer(&mut bytes, derived)
                .map_err(|_| "frozen Derived input exceeds byte budget")?;
        }
    }
    Ok(())
}
impl RequestSnapshot {
    /// Builds the frozen snapshot from the exact effective values used to
    /// create a primary request.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: RequestIdentity,
        canonical_frontier: crate::context::resources::LedgerFrontier,
        effective_system_prompt: String,
        system_sections: Vec<AcceptedSystemSection>,
        runtime_resource_revision: crate::runtime::RuntimeResourceRevision,
        invocation: ModelInvocationConfig,
        context_window_tokens: u64,
        profile: Option<crate::model::catalog::ModelProfileId>,
        reasoning_enabled: bool,
        tool_definitions: Vec<ModelToolDefinition>,
        capability_revision: CapabilityRevision,
        context_generation: ContextGeneration,
        continuation: Option<ProviderContinuationState>,
        request_context_ids: Vec<MessageId>,
    ) -> Self {
        let provisional_message_id = identity.provisional_message_id();
        let semantic_input_fingerprint = ModelRequest {
            response_schema: None,
            response_byte_limit: None,
            images: crate::model::images::ResolvedImages::new(),
            invocation: invocation.clone(),
            messages: crate::model::SelectedModelInput::default(),
            tools: tool_definitions.clone(),
            effective_system_prompt: effective_system_prompt.clone(),
            continuation: continuation.clone(),
            continuation_source: None,
        }
        .semantic_input_fingerprint();
        Self {
            response_schema: None,
            tool_associations: Vec::new().into(),
            history_cut: crate::durable::projection::RequestHistoryCut {
                ledger: crate::context::resources::LedgerFrontier(
                    canonical_frontier
                        .0
                        .saturating_sub(request_context_ids.len() as u64),
                ),
                checkpoint_id: None,
                activation: crate::context::checkpoint::ProjectionActivationId(0),
                base_frontier: crate::context::resources::LedgerFrontier(0),
            },
            input: crate::model::SelectedInputManifest::default(),
            images: crate::model::images::ResolvedImages::new(),
            semantic_input_fingerprint,
            request_id: identity.request_id(),
            identity,
            provisional_message_id,
            canonical_frontier,
            effective_system_prompt,
            system_sections,
            runtime_resource_revision,
            invocation,
            context_window_tokens,
            profile,
            reasoning_enabled,
            tool_definitions,
            capability_revision,
            context_generation,
            continuation,
            continuation_source: None,
            request_context_ids,
            unresolved_output_carryover_source: None,
            unresolved_output_carryover: None,
            unresolved_output_carryover_anchor: None,
            contributions: Vec::new(),
        }
    }

    /// Reconstruct from the immutable dependency set retained by the hot conversation.
    /// # Errors
    /// Missing Original bytes fail explicitly.
    pub fn reconstruct(
        &self,
        conversation: &ConversationState,
    ) -> Result<ModelRequest, RequestReconstructionError> {
        self.reconstruct_with(|source| {
            conversation
                .ledger()
                .get(&source.message_id)
                .cloned()
                .ok_or_else(|| {
                    RequestReconstructionError::Conversation(format!(
                        "missing frozen input {}",
                        source.message_id
                    ))
                })
        })
    }

    /// Freeze the final provider-neutral request, after all semantic transformations.
    /// # Errors
    /// Rejects item, cumulative Derived byte, image and inline provenance budget violations
    /// before cloning retained bodies.
    pub fn freeze_input(
        &mut self,
        conversation: &crate::runtime::identity::ConversationId,
        request: &ModelRequest,
    ) -> Result<(), String> {
        validate_frozen_budget(request)?;
        let selected = request.messages.freeze(conversation);
        self.freeze_selected_input(conversation, request, &selected)?;
        self.tool_associations = crate::model::occurrence::canonical_associations(
            conversation,
            &self.request_id,
            request,
        )
        .unwrap_or_default()
        .into();
        Ok(())
    }

    /// Freeze the exact immutable selection carried by native request assembly.
    /// The native producer updates this handle at the same insertion/media edit
    /// boundary as the request. External callers use `freeze_input`.
    pub(crate) fn freeze_selected_input(
        &mut self,
        _conversation: &crate::runtime::identity::ConversationId,
        request: &ModelRequest,
        selected: &crate::model::SelectedInputManifest,
    ) -> Result<(), String> {
        #[cfg(test)]
        let _reads = super::selected_input::PreparationReads::new();
        if !selected.messages().same_input(&request.messages) {
            return Err("native selected input count disagrees with request".into());
        }
        request.continuation_boundary()?;
        selected.validate_budget()?;
        if request
            .images
            .values()
            .try_fold(0usize, |total, bytes| total.checked_add(bytes.len()))
            .is_none_or(|total| total > MAX_FROZEN_IMAGE_BYTES)
        {
            return Err("frozen input exceeds image byte budget".into());
        }
        self.input = selected.clone();
        self.images.clone_from(&request.images);
        self.response_schema.clone_from(&request.response_schema);
        self.continuation_source
            .clone_from(&request.continuation_source);
        self.semantic_input_fingerprint = selected.semantic_digest().map_or_else(
            || request.semantic_input_fingerprint(),
            |messages| request.semantic_input_fingerprint_with_messages(&messages),
        );
        Ok(())
    }

    /// Resolve exactly the frozen immutable references. No current-state reads.
    /// # Errors
    /// Missing, corrupted, or incorrectly scoped input is explicitly non-reconstructible.
    pub fn reconstruct_with(
        &self,
        mut load: impl FnMut(
            &crate::model::projected::CanonicalMessageRef,
        )
            -> Result<crate::message::types::MessageBlock, RequestReconstructionError>,
    ) -> Result<ModelRequest, RequestReconstructionError> {
        use crate::model::projected::FrozenModelInput;
        let messages = self
            .input
            .iter()
            .map(|item| match item {
                FrozenModelInput::Original(source) => {
                    let message = load(source)?;
                    if !source.matches(&message) {
                        return Err(RequestReconstructionError::Conversation(
                            "frozen Original identity or content integrity mismatch".into(),
                        ));
                    }
                    Ok(ModelInputMessage::Canonical(message))
                }
                FrozenModelInput::Derived(message) => {
                    Ok(ModelInputMessage::Derived(message.clone()))
                }
                FrozenModelInput::RequestOnly(message) => {
                    Ok(ModelInputMessage::RequestOnly(message.clone()))
                }
            })
            .collect::<Result<Vec<_>, RequestReconstructionError>>()?;
        let request = ModelRequest {
            response_schema: self.response_schema.clone(),
            response_byte_limit: None,
            images: self.images.clone(),
            invocation: self.invocation.clone(),
            messages: messages.into(),
            tools: self.tool_definitions.clone(),
            effective_system_prompt: self.effective_system_prompt.clone(),
            continuation: self.continuation.clone(),
            continuation_source: self.continuation_source.clone(),
        };
        crate::model::occurrence::validate_associations(
            &self.request_id,
            &request,
            &self.tool_associations,
        )
        .map_err(RequestReconstructionError::Conversation)?;
        if request.semantic_input_fingerprint() != self.semantic_input_fingerprint {
            return Err(RequestReconstructionError::Conversation(
                "frozen input fingerprint mismatch or missing input bytes".into(),
            ));
        }
        request
            .continuation_boundary()
            .map_err(RequestReconstructionError::Conversation)?;
        Ok(request)
    }

    /// Checks native scalar metadata against the certified unfinished flat SHA.
    pub(crate) fn selected_fingerprint_matches(&self) -> bool {
        use sha2::{Digest, Sha256};
        let Some(messages) = self.input.semantic_digest() else {
            return false;
        };
        let bytes = serde_json::to_vec(&(
            messages,
            &self.images,
            &self.effective_system_prompt,
            &self.tool_definitions,
            &self.invocation,
            &self.continuation,
            &self.response_schema,
        ))
        .expect("native request metadata serializes");
        format!("{:x}", Sha256::digest(bytes)) == self.semantic_input_fingerprint
    }

    /// Reconstruct with an already retained finite set of immutable dependencies.
    /// # Errors
    /// Reject missing dependency bytes rather than selecting another history.
    pub fn reconstruct_from_canonical(
        &self,
        canonical: &[crate::message::types::MessageBlock],
    ) -> Result<ModelRequest, RequestReconstructionError> {
        self.reconstruct_with(|source| {
            canonical
                .iter()
                .find(|m| m.id() == &source.message_id)
                .cloned()
                .ok_or_else(|| {
                    RequestReconstructionError::Conversation(format!(
                        "missing frozen input {}",
                        source.message_id
                    ))
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::projected::{ProjectedMessage, SemanticTransformation};
    use crate::runtime::identity::ConversationId;

    fn frozen(request: &ModelRequest) -> RequestSnapshot {
        let mut snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("attempt-frozen"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            crate::context::resources::LedgerFrontier((SurfaceRevision::new(1)).get()),
            request.effective_system_prompt.clone(),
            vec![],
            crate::runtime::RuntimeResourceRevision::new(1),
            request.invocation.clone(),
            128_000,
            None,
            false,
            request.tools.clone(),
            CapabilityRevision::new(1),
            ContextGeneration {
                id: 1,
                contributors: vec![],
            },
            request.continuation.clone(),
            vec![],
        );
        snapshot
            .freeze_input(
                &ConversationId::new("conv_5cdad274-a06f-7a35-8b6c-e0beb9470e1f"),
                request,
            )
            .unwrap();
        snapshot
    }

    #[test]
    fn structured_contract_survives_snapshot_reconstruction() {
        use crate::model::structured::{
            PreparedResponseSchema, ResponseSchema, StructuredOutputStrategy,
        };
        let mut request = crate::scripted_suites::common::simple_request(
            crate::model::ModelProtocol::OpenAiChatCompletions,
            "test",
            "source",
        );
        request.response_schema = Some(PreparedResponseSchema {
            schema: ResponseSchema::new("answer".into(), serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false}), 128).unwrap(),
            strategy: StructuredOutputStrategy::Native,
        });
        request.invocation.capabilities.json_schema_output = true;
        let snapshot = frozen(&request);
        let decoded: RequestSnapshot =
            serde_json::from_value(serde_json::to_value(&snapshot).unwrap()).unwrap();
        let reconstructed = decoded
            .reconstruct_with(|source| {
                request
                    .messages
                    .iter()
                    .find_map(|input| match input {
                        ModelInputMessage::Canonical(message) if source.matches(message) => {
                            Some(Ok(message.clone()))
                        }
                        _ => None,
                    })
                    .unwrap()
            })
            .unwrap();
        assert_eq!(reconstructed, request);
        assert_eq!(
            reconstructed.semantic_input_fingerprint(),
            snapshot.semantic_input_fingerprint
        );
    }

    #[test]
    fn primary_snapshot_retains_request_identity_serialization() {
        let request = crate::scripted_suites::common::simple_request(
            crate::model::ModelProtocol::OpenAiChatCompletions,
            "test",
            "input",
        );
        let snapshot = frozen(&request);
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            value["identity"],
            serde_json::json!({"attempt_id": "attempt-frozen", "turn": "1", "retry_number": 0})
        );
        assert!(value.get("owner").is_none());
        assert_eq!(value["request_id"], "request:14:attempt-frozen:1:1:0");
        let decoded: RequestSnapshot = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, snapshot);
        assert_eq!(decoded.request_id, decoded.identity.request_id());
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One captured cross-protocol/native/Original-Derived request matrix.
    fn frozen_tool_associations_equal_captured_adapter_wire_and_reconstruction() {
        use crate::message::types::{
            AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
            ReasoningBlock, ToolCallOccurrenceRef, ToolMessageBlock,
        };
        use crate::model::{ModelProtocol, projected::CanonicalMessageRef};
        use crate::runtime::continuation::{
            AcceptedOutputContinuationSource, AnthropicContinuation, OpenAiResponsesContinuation,
            ProviderContinuationState,
        };
        use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
        use crate::tools::types::ToolCall;
        let scope = ConversationId::new("conv_5cdad274-a06f-7a35-8b6c-e0beb9470e1f");
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            for mode in 0..5 {
                if mode > 0 && protocol != ModelProtocol::OpenAiResponses {
                    continue;
                }
                for derived_call in [false, true] {
                    for derived_result in [false, true] {
                        let mut request = crate::scripted_suites::common::simple_request(
                            protocol, "test", "history",
                        );
                        if mode >= 2 {
                            request.invocation.compat.responses_storage =
                                crate::model::catalog::ResponsesStorageMode::Stateless;
                        }
                        for batch in 0..2 {
                            let assistant_id = MessageId::new(format!("assistant-{batch}"));
                            let mut content = Vec::new();
                            let native = match protocol {
                                ModelProtocol::AnthropicMessages => Some(
                                    ProviderContinuationState::Anthropic(AnthropicContinuation {
                                        opaque: serde_json::json!({"type":"thinking","thinking":"native", "signature":"original-signature"}),
                                    }),
                                ),
                                ModelProtocol::OpenAiResponses if mode == 1 && batch == 1 => {
                                    Some(ProviderContinuationState::OpenAiResponses(
                                        OpenAiResponsesContinuation::Stored {
                                            previous_response_id: "accepted-response".into(),
                                        },
                                    ))
                                }
                                ModelProtocol::OpenAiResponses
                                    if ((mode == 2 || mode == 3) && batch == 1
                                        || mode == 4 && batch == 0) =>
                                {
                                    Some(ProviderContinuationState::OpenAiResponses(
                                        OpenAiResponsesContinuation::Stateless {
                                            items: vec![
                                                serde_json::json!({"type":"reasoning","id":"reasoning-native","encrypted_content":"original-encrypted"}),
                                                serde_json::json!({"type":"function_call","id":"fc-a","call_id":"repeat-a","name":"tool","arguments":"{}"}),
                                                serde_json::json!({"type":"function_call","id":"fc-b","call_id":"repeat-b","name":"tool","arguments":"{}"}),
                                            ],
                                        },
                                    ))
                                }
                                _ => None,
                            };
                            if let Some(state) = native.clone() {
                                content.push(AssistantContentBlock::Reasoning(ReasoningBlock {
                                    text: (protocol == ModelProtocol::AnthropicMessages)
                                        .then(|| "native".into()),
                                    provider_state: Some(state),
                                }));
                            }
                            let start = u32::try_from(content.len()).unwrap();
                            for id in ["repeat-a", "repeat-b"] {
                                content.push(AssistantContentBlock::ToolCall(ToolCall {
                                    id: ToolCallId::new(id),
                                    tool_id: ToolId::new("tool"),
                                    name: "tool".into(),
                                    arguments: serde_json::json!({}),
                                }));
                            }
                            let assistant = MessageBlock::Assistant(AssistantMessageBlock {
                                id: assistant_id.clone(),
                                content,
                            });
                            if (mode == 1 || mode == 2) && batch == 1 {
                                request.continuation_source = Some(
                                    AcceptedOutputContinuationSource::from_accepted_output(
                                        RequestId::new("accepted-request"),
                                        &scope,
                                        &request.messages,
                                        &assistant,
                                    )
                                    .unwrap(),
                                );
                                request.continuation = native;
                            }
                            request.messages.push(assistant.into());
                            // Results deliberately reverse the call order within a complete batch.
                            for (block, id) in [(start + 1, "repeat-b"), (start, "repeat-a")] {
                                request.messages.push(
                                    MessageBlock::Tool(ToolMessageBlock {
                                        id: MessageId::new(format!("result-{batch}-{block}")),
                                        tool_call_id: ToolCallId::new(id),
                                        occurrence: ToolCallOccurrenceRef::new(
                                            assistant_id.clone(),
                                            ContentBlockIndex::new(block),
                                        ),
                                        tool_id: ToolId::new("tool"),
                                        result:
                                            crate::scripted_suites::support::fake::success_result(
                                                "result",
                                            ),
                                    })
                                    .into(),
                                );
                            }
                        }
                        let canonical: Vec<_> = request
                            .messages
                            .iter()
                            .filter_map(|m| m.as_canonical().cloned())
                            .collect();
                        let mut snapshot = frozen(&request);
                        if mode == 1 || mode == 2 {
                            let covered =
                                serde_json::to_value(&snapshot.tool_associations[..2]).unwrap();
                            assert!(
                                covered
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .all(|a| a["wire_id"].is_null()),
                                "covered associations must not claim fields absent from the actual wire request"
                            );
                        }
                        let associations = snapshot.tool_associations.clone();
                        for message in &mut request.messages {
                            let convert = matches!(
                                message.view(),
                                crate::model::projected::ModelMessageView::Assistant(_)
                            ) && derived_call
                                || matches!(
                                    message.view(),
                                    crate::model::projected::ModelMessageView::Tool(_, _)
                                ) && derived_result;
                            if convert {
                                let original = message.as_canonical().unwrap();
                                *message = ModelInputMessage::Derived(
                                    crate::model::projected::DerivedModelMessage {
                                        message: ProjectedMessage::from(original),
                                        sources: vec![CanonicalMessageRef::of(&scope, original)],
                                        checkpoint_sources: vec![],
                                        transformations: vec![],
                                    },
                                );
                            }
                        }
                        snapshot.freeze_input(&scope, &request).unwrap();
                        snapshot.tool_associations = associations;
                        if mode == 4 {
                            assert!(
                                crate::model::adapter::validation::validate_prepared_request(
                                    &request
                                )
                                .is_err(),
                                "opaque older output cannot receive fabricated correlation labels"
                            );
                            continue;
                        }
                        let translate = |input: &ModelRequest| {
                            let tools = crate::model::adapter::validation::validate_request(
                                input, protocol,
                            )
                            .unwrap();
                            match protocol {
                                ModelProtocol::OpenAiChatCompletions => crate::model::adapter::openai::chat_completions::translate_request(input).unwrap(),
                                ModelProtocol::OpenAiResponses => crate::model::adapter::openai::responses::translate_request(input,&tools,input.invocation.compat.responses_storage).unwrap(),
                                ModelProtocol::AnthropicMessages => crate::model::adapter::anthropic::mapping::translate_request(input,&tools).unwrap(),
                            }
                        };
                        let mut captured = translate(&request);
                        let actual = crate::model::adapter::correlation::observed_correlations(
                            &mut captured,
                            protocol,
                        );
                        let mut ordered = snapshot.tool_associations.clone();
                        ordered.sort_by_key(|a| (a.call.message.item_index, a.call.block.0));
                        let calls: Vec<_> = ordered
                            .iter()
                            .filter_map(|a| a.wire_id.as_ref().map(|id| id.0.clone()))
                            .collect();
                        ordered.sort_by_key(|a| a.result.item_index);
                        let results: Vec<_> = ordered
                            .iter()
                            .filter_map(|a| a.wire_id.as_ref().map(|id| id.0.clone()))
                            .collect();
                        let actual_calls: Vec<_> = actual
                            .iter()
                            .filter(|(call, _)| *call)
                            .map(|(_, id)| id.clone())
                            .collect();
                        let actual_results: Vec<_> = actual
                            .iter()
                            .filter(|(call, _)| !*call)
                            .map(|(_, id)| id.clone())
                            .collect();
                        if mode == 1 {
                            assert!(actual_calls.is_empty());
                        } else {
                            assert_eq!(actual_calls, calls);
                        }
                        assert_eq!(actual_results, results);
                        let reconstructed =
                            snapshot.reconstruct_from_canonical(&canonical).unwrap();
                        assert_eq!(translate(&request), translate(&reconstructed));
                    }
                }
            }
        }
    }

    #[test]
    fn frozen_derived_input_reconstructs_without_dynamic_selection() {
        let mut request = crate::scripted_suites::common::simple_request(
            crate::model::ModelProtocol::OpenAiChatCompletions,
            "test",
            "original",
        );
        let source = request.messages[0].as_canonical().unwrap().clone();
        if let ModelInputMessage::Canonical(crate::message::types::MessageBlock::User(user)) =
            &mut request.messages[0]
        {
            user.content = vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "frozen upload path /workspace/input.csv".into(),
                },
            )];
        }
        request.messages[0].mark_transformed(
            &source,
            &ConversationId::new("conv_5cdad274-a06f-7a35-8b6c-e0beb9470e1f"),
            SemanticTransformation::UploadRendering,
        );
        assert_eq!(request.messages[0].canonical_id(), None);
        let snapshot = frozen(&request);
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        let decoded: RequestSnapshot = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            decoded
                .reconstruct_with(|_| panic!("Derived input must not resolve dynamic sources"))
                .unwrap(),
            request
        );
        assert!(
            matches!(&decoded.input[0], crate::model::projected::FrozenModelInput::Derived(m) if matches!(m.message, ProjectedMessage::User { .. }))
        );
    }

    #[test]
    fn missing_original_or_redacted_derived_bytes_are_non_reconstructible() {
        let request = crate::scripted_suites::common::simple_request(
            crate::model::ModelProtocol::OpenAiChatCompletions,
            "test",
            "required bytes",
        );
        let snapshot = frozen(&request);
        assert!(snapshot.reconstruct_from_canonical(&[]).is_err());
        let mut redacted = snapshot.clone();
        redacted.input.clear();
        assert!(
            redacted
                .reconstruct_from_canonical(&[])
                .unwrap_err()
                .to_string()
                .contains("missing input bytes")
        );
        assert_eq!(
            snapshot
                .reconstruct_from_canonical(&[request.messages[0].as_canonical().unwrap().clone()])
                .unwrap(),
            request
        );
    }

    #[test]
    fn semantic_fingerprint_ignores_occurrence_identity_but_includes_materialized_input() {
        let request = crate::scripted_suites::common::simple_request(
            crate::model::ModelProtocol::OpenAiChatCompletions,
            "test",
            "unchanged",
        );
        let mut snapshot = frozen(&request);
        let fingerprint = snapshot.semantic_input_fingerprint.clone();
        snapshot.identity.retry_number = 1;
        snapshot.request_id = snapshot.identity.request_id();
        snapshot.provisional_message_id = snapshot.identity.provisional_message_id();
        snapshot.request_context_ids.clear();
        assert_eq!(snapshot.semantic_input_fingerprint, fingerprint);
        let mut other = request.clone();
        other
            .effective_system_prompt
            .push_str("authorized correction");
        assert_ne!(other.semantic_input_fingerprint(), fingerprint);
        other = request.clone();
        other.images.insert(
            crate::runtime::identity::ArtifactId::new("image"),
            vec![1, 2, 3],
        );
        assert_ne!(other.semantic_input_fingerprint(), fingerprint);
    }
    #[test]
    fn frozen_original_integrity_includes_provenance_metadata() {
        let request = crate::scripted_suites::common::simple_request(
            crate::model::ModelProtocol::OpenAiChatCompletions,
            "test",
            "same provider text",
        );
        let snapshot = frozen(&request);
        let mut changed = request.messages[0].as_canonical().unwrap().clone();
        if let crate::message::types::MessageBlock::User(user) = &mut changed {
            user.source = crate::message::types::UserSource::Runtime;
        }
        assert!(
            snapshot
                .reconstruct_from_canonical(&[changed])
                .unwrap_err()
                .to_string()
                .contains("integrity mismatch")
        );
    }

    #[test]
    fn frozen_body_and_provenance_budgets_do_not_install_partial_input() {
        let request = crate::scripted_suites::common::simple_request(
            crate::model::ModelProtocol::OpenAiChatCompletions,
            "test",
            "original",
        );
        let source = request.messages[0].as_canonical().unwrap().clone();
        let mut oversized = request.clone();
        if let ModelInputMessage::Canonical(crate::message::types::MessageBlock::User(user)) =
            &mut oversized.messages[0]
        {
            user.content = vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "x".repeat(MAX_FROZEN_DERIVED_BYTES),
                },
            )];
        }
        let scope = ConversationId::new("conv_5cdad274-a06f-7a35-8b6c-e0beb9470e1f");
        oversized.messages[0].mark_transformed(
            &source,
            &scope,
            SemanticTransformation::UploadRendering,
        );
        let mut snapshot = frozen(&request);
        let before = snapshot.clone();
        assert!(
            snapshot
                .freeze_input(&scope, &oversized)
                .unwrap_err()
                .contains("byte budget")
        );
        assert_eq!(snapshot, before);
        let mut provenance = request.clone();
        provenance.messages[0] =
            ModelInputMessage::Derived(crate::model::projected::DerivedModelMessage {
                message: ProjectedMessage::from(&source),
                sources: vec![
                    crate::model::projected::CanonicalMessageRef::of(&scope, &source);
                    MAX_FROZEN_PROVENANCE_REFS + 1
                ],
                transformations: vec![SemanticTransformation::UploadRendering],
                checkpoint_sources: Vec::new(),
            });
        assert!(
            snapshot
                .freeze_input(&scope, &provenance)
                .unwrap_err()
                .contains("provenance budget")
        );
        assert_eq!(snapshot, before);
    }
}
