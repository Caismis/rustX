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

use rustx_conversation::{ConversationError, ConversationState};
use rustx_types::context::assembly::{AcceptedSystemSection, ContextGeneration};
use rustx_types::model::input::{
    ModelInputMessage, RenderedUnresolvedOutputCarryover, RequestOnlyInsertionAnchor,
};
use rustx_types::model::invocation_config::ModelInvocationConfig;
pub use rustx_types::model::snapshot::RequestIdentity;
use rustx_types::model::types::ModelRequest;
use rustx_types::runtime::continuation::ProviderContinuationState;
use rustx_types::runtime::identity::{
    CapabilityRevision, MessageId, PublicationStreamId, RequestId,
};
use rustx_types::tools::types::ModelToolDefinition;

pub use rustx_types::model::snapshot::ModelRequestOwner;

pub use rustx_types::context::contribution::ContributionStart;

/// A provider-independent frozen request boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestSnapshot {
    /// Native execution mode frozen at this committed request start. Together
    /// with `history_cut.activation` this reconstructs sparse reminder consumption.
    pub permission_mode: rustx_types::runtime::types::ApprovalMode,
    /// Exact canonical task input retained across continuation/reconstruction.
    /// Its original source is loaded by identity, never inferred from history.
    pub permission_task_message: Option<MessageId>,
    /// Exact prepared structured output, omitted from routine observations.
    pub response_schema: Option<rustx_types::model::structured::PreparedResponseSchema>,
    /// Associations in actual-request coordinates, distinct from checkpoint and physical call coordinates.
    pub tool_associations: crate::model::occurrence::FrozenToolAssociations,
    pub history_cut: crate::projection::RequestHistoryCut,
    /// Ordered frozen input. Reconstruction never selects history or transforms it.
    pub input: rustx_types::model::selected_input::SelectedInputManifest,
    /// Image bytes selected before start, retained for exact reconstruction.
    pub images: rustx_types::model::images::ResolvedImages,
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
    pub canonical_frontier: rustx_types::context::resources::LedgerFrontier,
    /// The exact request-time rendered Effective System Prompt.
    pub effective_system_prompt: String,
    /// Exact ordered System-section result from which the prompt was
    /// rendered. No historical discovery or extension logic is rerun.
    pub system_sections: Vec<AcceptedSystemSection>,
    /// Process-local resource generation observed by the admitted attempt.
    pub runtime_resource_revision: rustx_types::runtime::identity::RuntimeResourceRevision,
    /// The effective provider-neutral model invocation values.
    pub invocation: ModelInvocationConfig,
    /// The model context limit frozen for this request.
    pub context_window_tokens: u64,
    /// The selected Model Profile identity, when any.
    pub profile: Option<rustx_types::model::catalog::ModelProfileId>,
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
    pub continuation_source:
        Option<rustx_types::runtime::continuation::AcceptedOutputContinuationSource>,
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

pub const MAX_FROZEN_INPUT_ITEMS: usize = 8_192;
pub const MAX_FROZEN_DERIVED_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_FROZEN_PROVENANCE_REFS: usize = 1_024;
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
#[derive(Default)]
struct SnapshotInput {
    manifest: rustx_types::model::selected_input::SelectedInputManifest,
    images: rustx_types::model::images::ResolvedImages,
    fingerprint: String,
    response_schema: Option<rustx_types::model::structured::PreparedResponseSchema>,
    continuation_source:
        Option<rustx_types::runtime::continuation::AcceptedOutputContinuationSource>,
}

impl SnapshotInput {
    fn capture(
        request: &ModelRequest,
        selected: &rustx_types::model::selected_input::SelectedInputManifest,
    ) -> Result<Self, String> {
        #[cfg(test)]
        let _reads = rustx_types::model::selected_input::PreparationReads::new();
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
        Ok(Self {
            manifest: selected.clone(),
            images: request.images.clone(),
            fingerprint: selected.semantic_digest().map_or_else(
                || request.semantic_input_fingerprint(),
                |messages| request.semantic_input_fingerprint_with_messages(&messages),
            ),
            response_schema: request.response_schema.clone(),
            continuation_source: request.continuation_source.clone(),
        })
    }
}

impl RequestSnapshot {
    /// Builds the frozen snapshot from the exact effective values used to
    /// create a primary request.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        permission_mode: rustx_types::runtime::types::ApprovalMode,
        identity: RequestIdentity,
        canonical_frontier: rustx_types::context::resources::LedgerFrontier,
        effective_system_prompt: String,
        system_sections: Vec<AcceptedSystemSection>,
        runtime_resource_revision: rustx_types::runtime::identity::RuntimeResourceRevision,
        invocation: ModelInvocationConfig,
        context_window_tokens: u64,
        profile: Option<rustx_types::model::catalog::ModelProfileId>,
        tool_definitions: Vec<ModelToolDefinition>,
        capability_revision: CapabilityRevision,
        context_generation: ContextGeneration,
        continuation: Option<ProviderContinuationState>,
        request_context_ids: Vec<MessageId>,
    ) -> Self {
        let messages = rustx_types::model::selected_input::SelectedModelInput::default()
            .semantic_fingerprint();
        let images = rustx_types::model::images::ResolvedImages::new();
        let fingerprint = rustx_types::model::types::fingerprint_metadata(&(
            messages,
            &images,
            &effective_system_prompt,
            &tool_definitions,
            &invocation,
            &continuation,
            None::<rustx_types::model::structured::PreparedResponseSchema>,
        ));
        Self::with_input(
            permission_mode,
            identity,
            canonical_frontier,
            effective_system_prompt,
            system_sections,
            runtime_resource_revision,
            invocation,
            context_window_tokens,
            profile,
            tool_definitions,
            capability_revision,
            context_generation,
            continuation,
            request_context_ids,
            SnapshotInput {
                images,
                fingerprint,
                ..SnapshotInput::default()
            },
        )
    }

    #[allow(clippy::too_many_arguments)] // Freeze the admitted request and the exact staged context metadata.
    pub fn from_selected(
        permission_mode: rustx_types::runtime::types::ApprovalMode,
        identity: RequestIdentity,
        canonical_frontier: rustx_types::context::resources::LedgerFrontier,
        system_sections: Vec<AcceptedSystemSection>,
        runtime_resource_revision: rustx_types::runtime::identity::RuntimeResourceRevision,
        context_window_tokens: u64,
        profile: Option<rustx_types::model::catalog::ModelProfileId>,
        capability_revision: CapabilityRevision,
        context_generation: ContextGeneration,
        request_context_ids: Vec<MessageId>,
        request: &ModelRequest,
        selected: &rustx_types::model::selected_input::SelectedInputManifest,
    ) -> Result<Self, String> {
        let input = SnapshotInput::capture(request, selected)?;
        Ok(Self::with_input(
            permission_mode,
            identity,
            canonical_frontier,
            request.effective_system_prompt.clone(),
            system_sections,
            runtime_resource_revision,
            request.invocation.clone(),
            context_window_tokens,
            profile,
            request.tools.clone(),
            capability_revision,
            context_generation,
            request.continuation.clone(),
            request_context_ids,
            input,
        ))
    }

    #[allow(clippy::too_many_arguments)] // One constructor for the complete snapshot representation.
    fn with_input(
        permission_mode: rustx_types::runtime::types::ApprovalMode,
        identity: RequestIdentity,
        canonical_frontier: rustx_types::context::resources::LedgerFrontier,
        effective_system_prompt: String,
        system_sections: Vec<AcceptedSystemSection>,
        runtime_resource_revision: rustx_types::runtime::identity::RuntimeResourceRevision,
        invocation: ModelInvocationConfig,
        context_window_tokens: u64,
        profile: Option<rustx_types::model::catalog::ModelProfileId>,
        tool_definitions: Vec<ModelToolDefinition>,
        capability_revision: CapabilityRevision,
        context_generation: ContextGeneration,
        continuation: Option<ProviderContinuationState>,
        request_context_ids: Vec<MessageId>,
        input: SnapshotInput,
    ) -> Self {
        let provisional_message_id = identity.provisional_message_id();
        Self {
            permission_mode,
            permission_task_message: None,
            response_schema: input.response_schema,
            tool_associations: Vec::new().into(),
            history_cut: crate::projection::RequestHistoryCut {
                ledger: rustx_types::context::resources::LedgerFrontier(
                    canonical_frontier
                        .0
                        .saturating_sub(request_context_ids.len() as u64),
                ),
                checkpoint_id: None,
                activation: rustx_types::context::checkpoint::ProjectionActivationId(0),
                base_frontier: rustx_types::context::resources::LedgerFrontier(0),
            },
            input: input.manifest,
            images: input.images,
            semantic_input_fingerprint: input.fingerprint,
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
            tool_definitions,
            capability_revision,
            context_generation,
            continuation,
            continuation_source: input.continuation_source,
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
        conversation: &rustx_types::runtime::identity::ConversationId,
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
    pub fn freeze_selected_input(
        &mut self,
        _conversation: &rustx_types::runtime::identity::ConversationId,
        request: &ModelRequest,
        selected: &rustx_types::model::selected_input::SelectedInputManifest,
    ) -> Result<(), String> {
        let input = SnapshotInput::capture(request, selected)?;
        self.input = input.manifest;
        self.images = input.images;
        self.response_schema = input.response_schema;
        self.continuation_source = input.continuation_source;
        self.semantic_input_fingerprint = input.fingerprint;
        Ok(())
    }

    /// Resolve exactly the frozen immutable references. No current-state reads.
    /// # Errors
    /// Missing, corrupted, or incorrectly scoped input is explicitly non-reconstructible.
    pub fn reconstruct_with(
        &self,
        mut load: impl FnMut(
            &rustx_types::model::projected::CanonicalMessageRef,
        ) -> Result<
            rustx_types::message::types::MessageBlock,
            RequestReconstructionError,
        >,
    ) -> Result<ModelRequest, RequestReconstructionError> {
        use rustx_types::model::projected::FrozenModelInput;
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
    pub fn selected_fingerprint_matches(&self) -> bool {
        let Some(messages) = self.input.semantic_digest() else {
            return false;
        };
        rustx_types::model::types::fingerprint_metadata(&(
            messages,
            &self.images,
            &self.effective_system_prompt,
            &self.tool_definitions,
            &self.invocation,
            &self.continuation,
            &self.response_schema,
        )) == self.semantic_input_fingerprint
    }

    /// Reconstruct with an already retained finite set of immutable dependencies.
    /// # Errors
    /// Reject missing dependency bytes rather than selecting another history.
    pub fn reconstruct_from_canonical(
        &self,
        canonical: &[rustx_types::message::types::MessageBlock],
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
