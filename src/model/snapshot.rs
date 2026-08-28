//! Frozen provider-independent Request Snapshots (Issue #55).
//!
//! A Request Snapshot owns every non-history input needed to rebuild one
//! primary [`ModelRequest`]. The Conversation Surface revision is the only
//! canonical historical message reference; request-time derived values are
//! frozen by value. A terminally unresolved publication may contribute one
//! explicitly request-only carryover value, also frozen here by value and
//! without a canonical `MessageId`. Reconstruction never consults live model
//! configuration, Skills, contributors, filesystem state, pending carryover,
//! Publication Audit state, or runtime status.

use serde::{Deserialize, Serialize};

use crate::context::assembly::{AcceptedSystemSection, ContextGeneration};
use crate::conversation::{ConversationError, ConversationState, SurfaceRevision};
use crate::message::types::AgentStatusEmission;
use crate::model::input::{
    ModelInputMessage, RenderedUnresolvedOutputCarryover, RequestOnlyInsertionAnchor,
    RequestOnlyModelContext, canonical_input,
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

/// The Agent Status portion of one prepared model-turn start.
///
/// The canonical message identity and the semantic emission metadata travel
/// together inside the immutable Request Snapshot. This lets the durable
/// start transition validate that an emission belongs to the exact status
/// message and request that prepared it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStatusStart {
    /// The exact canonical Agent Status User message for the admitted
    /// generation. The initial request commits it; a transient retry carries
    /// the same metadata when the message remains active on the current
    /// Surface.
    pub message_id: MessageId,
    /// Emissions represented by that one status generation.
    #[serde(default)]
    pub emissions: Vec<AgentStatusEmission>,
}

/// A provider-independent frozen request boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestSnapshot {
    /// Stable durable identity of this actual request.
    pub request_id: RequestId,
    /// Request identity.
    pub identity: RequestIdentity,
    /// The provisional Assistant message identity frozen with this request.
    /// Publication opening and canonical acceptance must use this exact value.
    pub provisional_message_id: MessageId,
    /// The exact immutable Surface revision used by this request.
    pub surface_revision: SurfaceRevision,
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
    /// The selected reasoning profile identity, when any.
    pub reasoning_profile: Option<crate::model::catalog::ReasoningProfileId>,
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
    /// The exact Agent Status context and semantic emission metadata accepted
    /// for this request, when this request started a status generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<AgentStatusStart>,
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

impl RequestSnapshot {
    /// Builds the frozen snapshot from the exact effective values used to
    /// create a primary request.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: RequestIdentity,
        surface_revision: SurfaceRevision,
        effective_system_prompt: String,
        system_sections: Vec<AcceptedSystemSection>,
        runtime_resource_revision: crate::runtime::RuntimeResourceRevision,
        invocation: ModelInvocationConfig,
        context_window_tokens: u64,
        reasoning_profile: Option<crate::model::catalog::ReasoningProfileId>,
        reasoning_enabled: bool,
        tool_definitions: Vec<ModelToolDefinition>,
        capability_revision: CapabilityRevision,
        context_generation: ContextGeneration,
        continuation: Option<ProviderContinuationState>,
        request_context_ids: Vec<MessageId>,
    ) -> Self {
        let provisional_message_id = identity.provisional_message_id();
        Self {
            request_id: identity.request_id(),
            identity,
            provisional_message_id,
            surface_revision,
            effective_system_prompt,
            system_sections,
            runtime_resource_revision,
            invocation,
            context_window_tokens,
            reasoning_profile,
            reasoning_enabled,
            tool_definitions,
            capability_revision,
            context_generation,
            continuation,
            request_context_ids,
            unresolved_output_carryover_source: None,
            unresolved_output_carryover: None,
            unresolved_output_carryover_anchor: None,
            agent_status: None,
        }
    }

    /// Reconstructs the exact provider-neutral request from the referenced
    /// historical Surface revision and this frozen snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the referenced Surface revision, one of its
    /// canonical Ledger messages, or the frozen request-only insertion
    /// boundary cannot be reconstructed.
    pub fn reconstruct(
        &self,
        conversation: &ConversationState,
    ) -> Result<ModelRequest, RequestReconstructionError> {
        let canonical = conversation.reconstruct_messages(self.surface_revision)?;
        self.reconstruct_from_canonical(&canonical)
    }

    /// Reconstructs this snapshot from an already hydrated canonical Surface.
    ///
    /// Durable stores use this boundary after validating the Surface revision.
    /// The only request-only value considered is the immutable snapshot copy;
    /// no pending pointer, publication audit, or current runtime state is
    /// consulted.
    ///
    /// # Errors
    ///
    /// Returns an error when the frozen carryover source, insertion anchor, or
    /// canonical Surface does not match the snapshot.
    pub fn reconstruct_from_canonical(
        &self,
        canonical: &[crate::message::types::MessageBlock],
    ) -> Result<ModelRequest, RequestReconstructionError> {
        let mut messages = canonical_input(canonical);
        if self.unresolved_output_carryover.is_some()
            && self.unresolved_output_carryover_source.is_none()
        {
            return Err(RequestReconstructionError::Conversation(
                "carryover representation has no frozen source stream identity".to_owned(),
            ));
        }
        if self.unresolved_output_carryover_source.is_some()
            != self.unresolved_output_carryover_anchor.is_some()
        {
            return Err(RequestReconstructionError::Conversation(
                "frozen carryover source and insertion anchor must be present together".to_owned(),
            ));
        }
        let Some(source) = self.unresolved_output_carryover_source.as_ref() else {
            return Ok(ModelRequest {
                invocation: self.invocation.clone(),
                messages,
                tools: self.tool_definitions.clone(),
                effective_system_prompt: self.effective_system_prompt.clone(),
                continuation: self.continuation.clone(),
            });
        };
        let anchor = self
            .unresolved_output_carryover_anchor
            .as_ref()
            .ok_or_else(|| {
                RequestReconstructionError::Conversation(
                    "frozen carryover source has no insertion anchor".to_owned(),
                )
            })?;
        let context_len = self.request_context_ids.len();
        if context_len > messages.len() {
            return Err(RequestReconstructionError::Conversation(
                "frozen request context is longer than the reconstructed Surface".to_owned(),
            ));
        }
        let context_position = messages.len() - context_len;
        let suffix_ids: Vec<MessageId> = messages[context_position..]
            .iter()
            .filter_map(ModelInputMessage::canonical_id)
            .cloned()
            .collect();
        if suffix_ids != self.request_context_ids {
            return Err(RequestReconstructionError::Conversation(
                "frozen request context is not the reconstructed Surface suffix".to_owned(),
            ));
        }
        let position = match anchor {
            RequestOnlyInsertionAnchor::BeforeMessage(message_id) => messages
                .iter()
                .position(|message| message.canonical_id() == Some(message_id))
                .ok_or_else(|| {
                    RequestReconstructionError::Conversation(format!(
                        "carryover anchor message {message_id} is absent from the frozen Surface"
                    ))
                })?,
            RequestOnlyInsertionAnchor::AfterCanonical => context_position,
        };
        if let Some(context) = &self.unresolved_output_carryover {
            if context.source_stream_id != *source {
                return Err(RequestReconstructionError::Conversation(
                    "carryover representation disagrees with its frozen source stream identity"
                        .to_owned(),
                ));
            }
            messages.insert(
                position,
                ModelInputMessage::RequestOnly(RequestOnlyModelContext::UnresolvedOutputCarryover(
                    context.clone(),
                )),
            );
        }
        Ok(ModelRequest {
            invocation: self.invocation.clone(),
            messages,
            tools: self.tool_definitions.clone(),
            effective_system_prompt: self.effective_system_prompt.clone(),
            continuation: self.continuation.clone(),
        })
    }
}
