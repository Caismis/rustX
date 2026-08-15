//! Frozen provider-independent Request Snapshots (Issue #55).
//!
//! A Request Snapshot owns every non-history input needed to rebuild one
//! primary [`ModelRequest`]. The Conversation Surface revision is the only
//! historical message reference; request-time derived values are frozen by
//! value. Reconstruction never consults live model configuration, Skills,
//! contributors, filesystem state, or runtime status.

use serde::{Deserialize, Serialize};

use crate::context::assembly::ContextGeneration;
use crate::conversation::{ConversationError, ConversationState, SurfaceRevision};
use crate::model::invocation::ModelInvocationConfig;
use crate::model::types::ModelRequest;
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::{AttemptId, CapabilityRevision, TurnId};
use crate::tools::types::ModelToolDefinition;

/// The identity of one actual primary request attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestIdentity {
    /// The attempt that owns this request.
    pub attempt_id: AttemptId,
    /// The logical turn/step within that attempt.
    pub turn: TurnId,
    /// Zero for the first request of the step; one for the bounded overflow
    /// retry.
    pub retry_number: u32,
}

/// A provider-independent frozen request boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestSnapshot {
    /// Request identity.
    pub identity: RequestIdentity,
    /// The exact immutable Surface revision used by this request.
    pub surface_revision: SurfaceRevision,
    /// The exact request-time rendered Effective System Prompt.
    pub effective_system_prompt: String,
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
    /// definitions are frozen by value because pre-M8 capability storage is
    /// not a durable historical lookup authority.
    pub tool_definitions: Vec<ModelToolDefinition>,
    /// The immutable capability generation observed at admission.
    pub capability_revision: CapabilityRevision,
    /// The accepted context contributor generation that explains assembly.
    pub context_generation: ContextGeneration,
    /// Provider continuation state, if this request used one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ProviderContinuationState>,
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
        invocation: ModelInvocationConfig,
        context_window_tokens: u64,
        reasoning_profile: Option<crate::model::catalog::ReasoningProfileId>,
        reasoning_enabled: bool,
        tool_definitions: Vec<ModelToolDefinition>,
        capability_revision: CapabilityRevision,
        context_generation: ContextGeneration,
        continuation: Option<ProviderContinuationState>,
    ) -> Self {
        Self {
            identity,
            surface_revision,
            effective_system_prompt,
            invocation,
            context_window_tokens,
            reasoning_profile,
            reasoning_enabled,
            tool_definitions,
            capability_revision,
            context_generation,
            continuation,
        }
    }

    /// Reconstructs the exact provider-neutral request from the referenced
    /// historical Surface revision and this frozen snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the referenced Surface revision or one of its
    /// canonical Ledger messages cannot be reconstructed.
    pub fn reconstruct(
        &self,
        conversation: &ConversationState,
    ) -> Result<ModelRequest, RequestReconstructionError> {
        Ok(ModelRequest {
            invocation: self.invocation.clone(),
            messages: conversation.reconstruct_messages(self.surface_revision)?,
            tools: self.tool_definitions.clone(),
            effective_system_prompt: self.effective_system_prompt.clone(),
            continuation: self.continuation.clone(),
        })
    }
}
