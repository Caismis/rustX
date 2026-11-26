//! Interaction request/settlement vocabulary shared by the runtime
//! interaction domain and the durable journal record plane.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::events::interaction::{
    InteractionRequester, QuestionnaireResponse, QuestionnaireSpecification,
};
use crate::runtime::cancellation::ExecutionCancellation;
use crate::runtime::identity::{AttemptId, ConversationId, EventId, InteractionId, ToolId};
use crate::runtime::subagent::SubagentName;
use crate::runtime::types::CancellationReason;
use crate::tools::types::{ToolInvocationMode, ToolOrigin};

/// The bounded native interaction vocabulary of the 0.1 protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum InteractionKind {
    /// Business acceptance of an immutable subject, never Tool permission.
    Review {
        review: crate::events::review::ReviewSpecification,
        subject_digest: String,
    },
    /// Ask a client whether the already-resolved tool invocation may start.
    Approval {
        /// Caller-neutral invocation correlation.
        invocation_id: crate::tools::types::ToolInvocationId,
        /// The canonical registry-resolved tool identity.
        tool_id: ToolId,
        /// The safe model-facing tool name.
        tool_name: String,
        /// The registry-resolved tool origin.
        origin: ToolOrigin,
        /// The registry-resolved execution mode.
        mode: ToolInvocationMode,
        /// The already validated business arguments.  This is descriptive
        /// data only; it is never accepted back as replacement input.
        arguments: serde_json::Value,
        /// The native policy's bounded explanation for asking.
        reason: String,
    },
    /// Ask the user one bounded questionnaire. This is one coordinator
    /// interaction, not a sequence of old single-question interactions.
    Questionnaire {
        invocation_id: crate::tools::types::ToolInvocationId,
        /// The canonical identity of the tool that asked — a native tool, or
        /// an MCP-served tool whose `ToolOrigin` names its server. Every
        /// Runtime Client renders the requester from these facts and never
        /// infers the source from the prompt text.
        requester: InteractionRequester,
        /// The complete immutable facts shown to the Runtime Client.
        questionnaire: QuestionnaireSpecification,
    },
}

/// One truthful native interaction owner. Scheduled work has no model turn.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionExecution {
    Attempt {
        attempt_id: AttemptId,
        turn: u32,
    },
    Scheduled {
        run: crate::runtime::workflow::WorkflowRunId,
    },
}
impl InteractionExecution {
    #[must_use]
    pub fn attempt_id(&self) -> Option<&AttemptId> {
        match self {
            Self::Attempt { attempt_id, .. } => Some(attempt_id),
            Self::Scheduled { .. } => None,
        }
    }
    #[must_use]
    pub const fn turn(&self) -> Option<u32> {
        match self {
            Self::Attempt { turn, .. } => Some(*turn),
            Self::Scheduled { .. } => None,
        }
    }
    #[must_use]
    pub fn namespace(&self) -> String {
        match self {
            Self::Attempt { attempt_id, .. } => format!("attempt:{attempt_id}"),
            Self::Scheduled { run } => match &run.origin {
                crate::runtime::workflow::WorkflowRunOrigin::Scheduled { start_id } => {
                    format!("scheduled:{start_id}")
                }
                crate::runtime::workflow::WorkflowRunOrigin::Tool { .. } => {
                    "invalid-scheduled-origin".into()
                }
            },
        }
    }
    #[must_use]
    pub fn interaction_id(&self, ordinal: u64) -> InteractionId {
        match self {
            Self::Attempt { attempt_id, .. } => InteractionId::for_attempt(attempt_id, ordinal),
            Self::Scheduled { .. } => InteractionId::new(format!("{}:{ordinal}", self.namespace())),
        }
    }
}

/// One live interaction request projected to a Runtime Client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct InteractionRequest {
    /// The non-reused runtime-owned interaction identity.
    pub id: InteractionId,
    /// The conversation that owns the interaction.
    pub conversation_id: ConversationId,
    /// Exact native owner, including Scheduled identity without an Attempt.
    pub execution: InteractionExecution,
    /// The bounded interaction facts.
    pub kind: InteractionKind,
}

impl InteractionRequest {
    /// The address of this request at the root human-facing surface.
    #[must_use]
    pub fn interaction_ref(&self) -> InteractionRef {
        InteractionRef {
            conversation_id: self.conversation_id.clone(),
            interaction_id: self.id.clone(),
        }
    }
}

/// The root-facing address of a conversation-local interaction.
///
/// `InteractionId` is allocated inside one conversation/attempt domain and is
/// intentionally not globally unique. The pair is the only identity that
/// crosses a Runtime Client or parent/child routing boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct InteractionRef {
    /// The conversation-owned semantic interaction domain.
    pub conversation_id: ConversationId,
    /// The interaction identity allocated by that conversation's coordinator.
    pub interaction_id: InteractionId,
}

impl InteractionRef {
    /// Creates one routed address from the canonical identity pair.
    #[must_use]
    pub fn new(conversation_id: ConversationId, interaction_id: InteractionId) -> Self {
        Self {
            conversation_id,
            interaction_id,
        }
    }
}

impl core::fmt::Display for InteractionRef {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "{}::{}",
            self.conversation_id, self.interaction_id
        )
    }
}

/// Presentation metadata for the root interaction surface.
///
/// This is projection data only. It grants no authority to the root router,
/// and it is never copied into either conversation's canonical history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum InteractionSource {
    /// The root runtime's primary conversation.
    Primary,
    /// A live supervised child conversation.
    Subagent {
        /// The parent-owned child identity.
        subagent_id: crate::runtime::identity::SubagentId,
        /// The child conversation that owns the interaction.
        child_conversation_id: ConversationId,
        /// The frozen named definition used by the child.
        agent_name: SubagentName,
    },
}

/// One pending interaction projected to the root Runtime Client.
///
/// The request remains the originating conversation's immutable request. The
/// routed address and source only make that request understandable and
/// answerable at the shared human-facing surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RoutedInteraction {
    /// The stable address the human response must use.
    pub interaction: InteractionRef,
    /// Root-facing source metadata.
    pub source: InteractionSource,
    /// The originating conversation's request facts.
    pub request: Arc<InteractionRequest>,
}

impl RoutedInteraction {
    /// Projects a primary-conversation request.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn primary(request: impl Into<Arc<InteractionRequest>>) -> Self {
        let request = request.into();
        Self {
            interaction: request.interaction_ref(),
            source: InteractionSource::Primary,
            request,
        }
    }

    /// Projects a child-conversation request with parent-owned source data.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn subagent(
        subagent_id: crate::runtime::identity::SubagentId,
        child_conversation_id: ConversationId,
        agent_name: SubagentName,
        request: impl Into<Arc<InteractionRequest>>,
    ) -> Self {
        let request = request.into();
        let interaction = request.interaction_ref();
        debug_assert_eq!(interaction.conversation_id, child_conversation_id);
        Self {
            interaction,
            source: InteractionSource::Subagent {
                subagent_id,
                child_conversation_id,
                agent_name,
            },
            request,
        }
    }
}

/// The finite approval decision accepted from a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum ApprovalDecision {
    /// Continue with the exact `PreparedInvocation` that was already
    /// resolved by the Tool Registry.
    Allow,
    /// Do not start the invocation and settle its canonical result slot as
    /// policy-denied.
    Deny {
        /// A bounded client-facing reason.
        reason: String,
    },
}

/// A typed response to one native interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum InteractionResponse {
    /// Only an exact subject/instance decision; no replacement content.
    Review {
        response: crate::events::review::ReviewResponse,
    },
    /// The response to an approval request.
    Approval {
        /// The finite approval decision.  It has no tool arguments.
        decision: ApprovalDecision,
    },
    /// The response to a Questionnaire request. It carries only indices and
    /// answer decisions; the request facts remain authoritative in the
    /// pending interaction.
    Questionnaire {
        /// A submitted answer set or explicit decline.
        response: QuestionnaireResponse,
    },
}

/// Internal reliable control route. Cancellation is distinct from a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionControl {
    Respond { response: InteractionResponse },
    Cancel,
}

/// The terminal outcome delivered to the semantic waiter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum InteractionOutcome {
    /// A native ancestor deadline won the rendezvous.
    DeadlineExpired {
        kind: crate::tools::deadline::ToolDeadlineKind,
    },
    /// A client supplied the terminal typed response, including an explicit
    /// questionnaire decline.
    Responded {
        /// The accepted response.
        response: InteractionResponse,
    },
    /// The owning attempt cancellation won the rendezvous.
    Cancelled {
        /// The first-winner cancellation cause from the owning attempt.
        reason: CancellationReason,
    },
}

impl InteractionOutcome {
    #[must_use]
    pub fn interrupted(cancellation: &ExecutionCancellation) -> Self {
        match cancellation.native_cause() {
            crate::tools::deadline::ToolCancellationCause::Attempt(reason) => {
                Self::Cancelled { reason }
            }
            crate::tools::deadline::ToolCancellationCause::Deadline(kind) => {
                Self::DeadlineExpired { kind }
            }
        }
    }
}

/// An interaction-operation failure that is not itself a human decision.
///
/// `Unavailable` is reserved for refusal before the publication frontier:
/// no capable provider admitted the request, so no `InteractionRequested`
/// fact exists. Once a request has crossed that frontier, failures are
/// runtime/control failures and must stop the owning supervised execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionFailure {
    /// Publication was refused before an `InteractionRequested` fact could
    /// commit because no capable human provider was admitted.
    Unavailable,
    /// The request was rejected before the publication frontier for an
    /// internal reason: the subject facts failed bounded validation or the
    /// per-attempt interaction ordinal space was exhausted. This is never
    /// human provider absence.
    Invalid,
    /// The coordinator could not commit the requested fact or its lifecycle
    /// admission after the root publication frontier was crossed.
    PublicationFailed,
    /// The reliable interaction-control path failed after publication.
    ControlLost,
    /// A required durable interaction fact could not commit.
    AuditFailed,
}

impl InteractionFailure {
    #[must_use]
    pub fn is_unavailable(self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

impl From<InteractionRouteError> for InteractionFailure {
    fn from(error: InteractionRouteError) -> Self {
        match error {
            InteractionRouteError::ControlLost => Self::ControlLost,
        }
    }
}

/// The result of the root-side publication admission handshake.
///
/// Provider refusal and reliable control loss are deliberately distinct even
/// though both happen before the requested fact can commit. Only the former
/// is the product-level `Unavailable` interaction contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionAdmissionError {
    /// The root authority refused because no capable human surface was
    /// attached at the admission frontier.
    Unavailable,
    /// The reliable child/parent control path failed before admission could
    /// produce a permit.
    ControlLost,
}

impl From<InteractionAdmissionError> for InteractionFailure {
    fn from(error: InteractionAdmissionError) -> Self {
        match error {
            InteractionAdmissionError::Unavailable => Self::Unavailable,
            InteractionAdmissionError::ControlLost => Self::ControlLost,
        }
    }
}

impl core::fmt::Display for InteractionFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("no capable human interaction provider was admitted"),
            Self::Invalid => f.write_str("the interaction request was rejected before publication"),
            Self::PublicationFailed => {
                f.write_str("the interaction request could not be durably published")
            }
            Self::ControlLost => f.write_str("the reliable interaction control path was lost"),
            Self::AuditFailed => f.write_str("the interaction audit could not be committed"),
        }
    }
}

/// The canonical durable identity of one interaction's requested fact.
///
/// The identity is derived from the interaction identity alone, so the
/// durable authority can resolve the requested/settled pair through its unique
/// `event_id` index instead of scanning the Event Journal.
#[must_use]
pub fn interaction_requested_event_id(interaction_id: &InteractionId) -> EventId {
    EventId::new(format!("interaction-requested-event:{interaction_id}"))
}

/// The canonical durable identity of one interaction's settled fact.
#[must_use]
pub fn interaction_settled_event_id(interaction_id: &InteractionId) -> EventId {
    EventId::new(format!("interaction-settled-event:{interaction_id}"))
}

/// A reliable interaction route can fail only as a control-path failure.
/// Provider absence is decided by the separate root admission operation
/// before the originating coordinator commits its requested fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionRouteError {
    /// The reliable child/parent control path no longer accepts semantic
    /// interaction traffic.
    ControlLost,
}
