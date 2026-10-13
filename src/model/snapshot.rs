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
    AttemptId, CapabilityRevision, MessageId, PublicationStreamId, RequestId, TurnId,
};
use crate::tools::types::ModelToolDefinition;

/// The identity of one actual primary request attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Associations in actual-request coordinates, distinct from checkpoint and physical call coordinates.
    pub tool_associations: Vec<crate::model::occurrence::ActualRequestToolAssociation>,
    pub history_cut: crate::durable::projection::RequestHistoryCut,
    /// Ordered frozen input. Reconstruction never selects history or transforms it.
    pub input: Vec<crate::model::projected::FrozenModelInput>,
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
            images: crate::model::images::ResolvedImages::new(),
            invocation: invocation.clone(),
            messages: Vec::new(),
            tools: tool_definitions.clone(),
            effective_system_prompt: effective_system_prompt.clone(),
            continuation: continuation.clone(),
            continuation_source: None,
        }
        .semantic_input_fingerprint();
        Self {
            tool_associations: Vec::new(),
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
            input: Vec::new(),
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
        use crate::model::projected::{CanonicalMessageRef, FrozenModelInput};
        request.continuation_boundary()?;
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
        self.input = request
            .messages
            .iter()
            .map(|message| match message {
                ModelInputMessage::Canonical(m) => {
                    FrozenModelInput::Original(CanonicalMessageRef::of(conversation, m))
                }
                ModelInputMessage::Derived(m) => FrozenModelInput::Derived(m.clone()),
                ModelInputMessage::RequestOnly(m) => FrozenModelInput::RequestOnly(m.clone()),
            })
            .collect();
        self.images.clone_from(&request.images);
        self.continuation_source
            .clone_from(&request.continuation_source);
        self.tool_associations = crate::model::occurrence::canonical_associations(
            conversation,
            &self.request_id,
            &request.messages,
        )
        .unwrap_or_default();
        self.semantic_input_fingerprint = request.semantic_input_fingerprint();
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
        crate::model::occurrence::validate_associations(
            &self.request_id,
            &messages,
            &self.tool_associations,
        )
        .map_err(RequestReconstructionError::Conversation)?;
        let request = ModelRequest {
            images: self.images.clone(),
            invocation: self.invocation.clone(),
            messages,
            tools: self.tool_definitions.clone(),
            effective_system_prompt: self.effective_system_prompt.clone(),
            continuation: self.continuation.clone(),
            continuation_source: self.continuation_source.clone(),
        };
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
