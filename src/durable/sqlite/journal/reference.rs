//! Durable reference validation of Journal facts against their owners.

use super::{
    find_event_at_sequence, find_event_by_id, load_user_notification_tx,
    runtime_event_dependency_name,
};
use crate::durable::sqlite::auxiliary;
use crate::durable::sqlite::canonical::ledger::{
    ledger_message_exists, load_head, load_message_tx,
};
use crate::durable::sqlite::publication::proposal::{
    canonical_generation_owner, record_tool_proposal_dependency,
};
use crate::durable::sqlite::request::contribution::{
    read_logical_step_progress_tx, validate_contribution_emission_shape,
};
use crate::durable::sqlite::request::snapshot::{
    find_request_outcome_event, read_request_snapshot_tx, require_started_request_tx,
    validate_started_request_row_tx,
};
use crate::durable::sqlite::request::{
    reconstruct_request_start_reference, validate_request_start_metadata,
};
use crate::durable::sqlite::{decode, projection, storage};
use crate::durable::{ConversationStoreError, InboundDraft};
use crate::events::interaction::{
    InteractionSubject, interaction_arguments_digest, validate_interaction_settlement,
    validate_interaction_subject,
};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope, contribution_emission_event_id};
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, InboundKind, MessageBlock, UserContentBlock, UserSource,
};
use crate::model::ModelRequestOwner;
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, InteractionId, TurnId};
use rusqlite::OptionalExtension;
use rusqlite::Transaction;

/// The attempt and turn envelope one interaction audit fact is pinned to.
///
/// An interaction is always owned by exactly one model turn of exactly one
/// attempt, so an audit fact with no attempt or no turn could neither be
/// compared to its partner nor resolved to a durable generation — and an
/// unpinnable fact is not a pinned fact.
pub(in crate::durable::sqlite) fn require_interaction_envelope<'a>(
    envelope: &'a RuntimeEventEnvelope,
    interaction_id: &InteractionId,
) -> Result<(&'a AttemptId, &'a TurnId), ConversationStoreError> {
    let (Some(attempt_id), Some(turn_id)) = (&envelope.attempt_id, &envelope.turn_id) else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} audit facts must carry their owning attempt and turn"
        )));
    };
    Ok((attempt_id, turn_id))
}

pub(in crate::durable::sqlite) fn native_approval_key(
    id: &crate::tools::types::ToolInvocationId,
) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(serde_json::to_vec(id).expect("invocation identity"));
    format!("native-approval:{digest:x}")
}

/// Verifies that one Approval audit subject describes the canonical `ToolCall`
/// it names **in the generation its own envelope names** (Issue #109).
///
/// The subject is deliberately bounded: it names the call and tool identity by
/// value and pins the exact arguments by digest rather than copying them,
/// because the canonical `ToolCall` in the Message Ledger already holds those
/// arguments by value. That economy is only honest if the durable authority
/// checks the correspondence, and the correspondence is only meaningful if it
/// includes the generation. Content equality alone is not ownership: two turns
/// of one attempt, or two attempts of one conversation, can hold canonical
/// calls that compare equal in every field the subject carries.
///
/// The proof therefore composes the two ownerships the durable model already
/// retains, and never falls back to a conversation-global bare `call_id`:
///
/// ```text
/// retained canonical publication generation   (attempt, turn) -> message_id
///                     +
/// active canonical Surface ownership          message_id is still canonical
///                     =
/// the exact canonical Assistant ToolCall this approval may describe
/// ```
///
/// Every Agent Approval reaches this with a publication owner: the Agent Loop
/// commits an Assistant message through `commit_canonical_publication` and
/// refuses to commit one at all without an open stream, so a canonical
/// `ToolCall` without a frozen `(attempt, turn, message_id)` owner cannot occur
/// on the approval path. There is deliberately no lenient fallback for a state
/// the runtime cannot produce and no database can already contain.
/// Workflow subjects instead pin exact prepared invocation facts in this
/// interaction transaction, without consulting observational native events.
pub(in crate::durable::sqlite) fn validate_approval_subject_against_canonical(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    interaction_id: &InteractionId,
    attempt_id: &AttemptId,
    turn_id: &TurnId,
    subject: &InteractionSubject,
) -> Result<(), ConversationStoreError> {
    let InteractionSubject::Approval {
        invocation_id,
        tool_id,
        tool_name,
        arguments_digest,
        ..
    } = subject
    else {
        return Ok(());
    };
    let call_id = match invocation_id {
        crate::tools::types::ToolInvocationId::Agent { call_id } => call_id,
        crate::tools::types::ToolInvocationId::Workflow { node } => {
            if node.block.run.origin.attempt_id() != Some(attempt_id)
                || node.block.run.conversation_id != *conversation_id
            {
                return Err(ConversationStoreError::InvalidReference(
                    "approval attempt differs from native invocation owner".into(),
                ));
            }
            // The interaction-request transaction is the exact preparation audit
            // boundary. Native execution events are observations, not authority.
            // The coordinator supplies this immutable subject only after preflight.
            let key = native_approval_key(invocation_id);
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                    [&key],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            if exists {
                return Err(ConversationStoreError::InvalidReference(
                    "native invocation already has a pinned approval subject".into(),
                ));
            }
            return Ok(());
        }
    };
    // 1. The generation that is asking must be the generation that proposed
    //    the call. This is the check that makes "turn 2 approved turn 1's
    //    ToolCall" — a permanently false audit fact — unrepresentable.
    let Some(owner) = canonical_generation_owner(transaction, call_id, attempt_id, turn_id)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} approves tool call {call_id}, which attempt {attempt_id} turn {turn_id} never canonically proposed"
        )));
    };
    // 2. That owner must still be canonical: an Assistant message that left
    //    the active Surface is no longer a call the conversation is making.
    if !load_head(transaction)?.active_message_ids.contains(&owner) {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} approves tool call {call_id}, whose canonical owner {owner} is no longer on the active Surface"
        )));
    }
    let MessageBlock::Assistant(assistant) = load_message_tx(transaction, &owner)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "the canonical owner {owner} of tool call {call_id} is not an Assistant message"
        )));
    };
    let Some(call) = assistant.content.iter().find_map(|block| match block {
        AssistantContentBlock::ToolCall(call) if call.id == *call_id => Some(call),
        _ => None,
    }) else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "the canonical owner {owner} of tool call {call_id} does not contain that call"
        )));
    };
    // 3. And the frozen identity and arguments must match the subject exactly.
    if call.tool_id != *tool_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} names tool id {tool_id} for call {call_id}, but the canonical ToolCall froze {}",
            call.tool_id
        )));
    }
    if call.name != *tool_name {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} names tool {tool_name} for call {call_id}, but the canonical ToolCall froze {}",
            call.name
        )));
    }
    let canonical = interaction_arguments_digest(&call.arguments);
    if canonical != *arguments_digest {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} pins arguments digest {arguments_digest} for call {call_id}, but the canonical ToolCall arguments digest to {canonical}"
        )));
    }
    Ok(())
}

/// Resolves the authoritative child identity and workspace binding from the one durable
/// ownership opening fact of a subagent lifecycle.
///
/// Terminal callers restate `child_agent_id` so the compound event remains
/// self-describing, but that repeated field is not authority. The ownership
/// fact has one deterministic event identity
/// ([`subagent_ownership_event_id`](crate::runtime::subagent::subagent_ownership_event_id)),
/// so the authoritative child identity resolves through the unique
/// `event_id` index in bounded time instead of scanning the event journal.
/// The embedded `SubagentId` is revalidated defensively: the located fact
/// must actually belong to the requested child before its `child_agent_id`
/// is trusted as provenance authority. Duplicate ownership facts are
/// already rejected at commit time by the `lifecycle_state` uniqueness
/// probe, so a deterministic lookup cannot miss a second opening fact.
pub(in crate::durable::sqlite) fn find_subagent_ownership(
    transaction: &Transaction<'_>,
    subagent_id: &crate::runtime::identity::SubagentId,
) -> Result<
    (
        AgentId,
        std::path::PathBuf,
        crate::events::types::SubagentOwnershipKind,
        String,
        crate::runtime::subagent::AgentActivationOrigin,
    ),
    ConversationStoreError,
> {
    let event_id = crate::runtime::subagent::subagent_ownership_event_id(subagent_id);
    let envelope = find_event_by_id(transaction, &event_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "subagent terminal has no durable ownership fact for {subagent_id}"
        ))
    })?;
    match envelope.event {
        RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id: embedded,
            child_agent_id,
            ownership,
            workspace,
            agent,
            origin,
            ..
        } if embedded == *subagent_id => Ok((child_agent_id, workspace, ownership, agent, origin)),
        RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id: embedded,
            ..
        } => Err(ConversationStoreError::InvalidReference(format!(
            "subagent ownership event {event_id} belongs to {embedded}, not {subagent_id}"
        ))),
        _ => Err(ConversationStoreError::InvalidReference(format!(
            "the subagent ownership event {event_id} is not the typed ownership fact"
        ))),
    }
}

/// The complete legal-state validation of one normal subagent terminal
/// publication (Issue #192), run before anything is accepted.
///
/// The transition is semantically closed: it binds the
/// `SubagentTerminalPublished` fact, the terminal report draft, and — on
/// success — the runtime-authored terminal notice draft to each other and
/// to the committed ownership fact, so a caller cannot create durable
/// state no legal sequence of runtime decisions could reach:
///
/// - the event identity, the report message identity, and both producer
///   correlations are the canonical functions of the one `SubagentId`;
/// - the event names the committed child agent of a `Normal` ownership;
/// - a successful report is authored by that child agent, every other
///   terminal by the runtime;
/// - exactly a success carries the runtime-authored terminal notice, whose
///   identity, correlation, provenance, and exact canonical text are
///   recomputed from durable facts.
#[allow(clippy::too_many_lines)] // one semantically closed validation boundary
pub(in crate::durable::sqlite) fn validate_subagent_terminal_publication(
    transaction: &Transaction<'_>,
    notice: Option<&InboundDraft>,
    report: &InboundDraft,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let RuntimeEvent::SubagentTerminalPublished {
        subagent_id,
        child_agent_id,
        message_id,
        state,
        ..
    } = &event.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "subagent terminal acceptance requires a subagent terminal fact".to_owned(),
        ));
    };
    if event.attempt_id.is_some() || event.turn_id.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "subagent terminal publication must not carry an attempt or turn envelope".to_owned(),
        ));
    }
    let canonical_event_id = crate::runtime::subagent::terminal_event_id(subagent_id);
    if !event.event_id.as_str().is_empty() && event.event_id != canonical_event_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent terminal event identity {} does not match the canonical identity {canonical_event_id}",
            event.event_id
        )));
    }
    let (committed_child_agent_id, _workspace, ownership, agent, _) =
        find_subagent_ownership(transaction, subagent_id)?;
    if ownership != crate::events::types::SubagentOwnershipKind::Normal {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} Workflow ownership cannot use parent terminal publication"
        )));
    }
    if committed_child_agent_id != *child_agent_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal claims child agent {child_agent_id}, but durable ownership committed {committed_child_agent_id}"
        )));
    }
    let canonical_report_id = crate::runtime::subagent::terminal_message_id(subagent_id);
    if *message_id != canonical_report_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal references {message_id}, not its canonical report message {canonical_report_id}"
        )));
    }
    if report.message_id.as_ref() != Some(&canonical_report_id) {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal report identity is not the canonical {canonical_report_id}"
        )));
    }
    if report.correlation.as_deref()
        != Some(crate::runtime::subagent::terminal_correlation(subagent_id).as_str())
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal report does not carry the canonical producer correlation"
        )));
    }
    if report.kind != InboundKind::Message {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal report has an ineligible inbound kind"
        )));
    }
    let success = matches!(
        state,
        crate::events::types::SubagentTerminalState::Succeeded
    );
    let provenance_ok = if success {
        report.source
            == UserSource::Agent {
                agent_id: child_agent_id.clone(),
            }
    } else {
        report.source == UserSource::Runtime
    };
    if !provenance_ok {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal report provenance does not match its terminal state"
        )));
    }
    match (success, notice) {
        (true, Some(notice)) => {
            if notice.source != UserSource::Runtime || notice.kind != InboundKind::Message {
                return Err(ConversationStoreError::InvalidReference(
                    "a subagent terminal notice is runtime-authored only".to_owned(),
                ));
            }
            let canonical_notice_id =
                crate::runtime::subagent::terminal_notice_message_id(subagent_id);
            if notice.message_id.as_ref() != Some(&canonical_notice_id) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal notice identity is not the canonical {canonical_notice_id}"
                )));
            }
            if notice.correlation.as_deref()
                != Some(crate::runtime::subagent::terminal_notice_correlation(subagent_id).as_str())
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal notice does not carry the canonical producer correlation"
                )));
            }
            let expected =
                crate::runtime::subagent::terminal_notice_text(child_agent_id, subagent_id, &agent);
            if notice.content != [UserContentBlock::Text(TextBlock { text: expected })] {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal notice is not the canonical runtime-authored terminal metadata"
                )));
            }
            Ok(())
        }
        (true, None) => Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} successful terminal requires the runtime-authored terminal notice"
        ))),
        (false, Some(_)) => Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} non-success terminal must not carry a successful-terminal notice"
        ))),
        (false, None) => Ok(()),
    }
}

pub(in crate::durable::sqlite) fn validate_agent_admission_reference(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    use crate::events::types::AgentActivationAdmissionPhase;
    let (agent_id, activation_id, origin, phase) = match &envelope.event {
        RuntimeEvent::AgentActivationAdmission {
            agent_id,
            activation_id,
            origin,
            phase,
        } => (agent_id, activation_id, origin, Some(phase)),
        RuntimeEvent::SubagentOwnershipCommitted {
            child_agent_id,
            subagent_id,
            origin,
            admitted_authority: None,
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            ..
        } => (child_agent_id, subagent_id, origin, None),
        _ => return Ok(()),
    };
    let invalid = || {
        ConversationStoreError::InvalidReference(
            "Agent activation admission must reference its durable Agent and exact reservation"
                .to_owned(),
        )
    };
    if activation_id
        .conversation_ordinal(&envelope.conversation_id)
        .is_none()
    {
        return Err(invalid());
    }
    if let Some(phase) = phase
        && envelope.event_id != crate::runtime::subagent::admission_event_id(activation_id, phase)
    {
        return Err(invalid());
    }
    let admitted: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_authorities WHERE agent_id=?1)",
            [agent_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("Agent admission authority: {error}")))?;
    if !admitted {
        return Err(invalid());
    }
    if matches!(phase, Some(AgentActivationAdmissionPhase::Reserved)) {
        return Ok(());
    }
    let reserved_id = crate::runtime::subagent::admission_event_id(
        activation_id,
        &AgentActivationAdmissionPhase::Reserved,
    );
    let reserved = find_event_by_id(transaction, &reserved_id)?.ok_or_else(invalid)?;
    if !matches!(reserved.event, RuntimeEvent::AgentActivationAdmission {
        agent_id: owner, activation_id: target, origin: admitted_origin,
        phase: AgentActivationAdmissionPhase::Reserved,
    } if owner == *agent_id && target == *activation_id && admitted_origin == *origin)
    {
        return Err(invalid());
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Closed validation of every durable event family.
pub(in crate::durable::sqlite) fn validate_event_reference(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    match &envelope.event {
        RuntimeEvent::ContextContributionEmitted {
            producer,
            request_id,
            message_id,
            emission,
            logical_step_origin,
        } => {
            validate_contribution_emission_shape(emission)?;
            let Some((snapshot, Some(started_sequence), _)) =
                read_request_snapshot_tx(transaction, request_id)?
            else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "contribution emission references request {request_id}, which has not started"
                )));
            };
            let started =
                find_event_at_sequence(transaction, started_sequence)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "request {request_id} has no event at its recorded start sequence"
                    ))
                })?;
            validate_request_start_metadata(&snapshot, &started)?;
            let contribution = snapshot
                .contributions
                .iter()
                .find(|candidate| {
                    candidate.message_id == *message_id && candidate.producer == *producer
                })
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "emission for request {request_id} has no accepted contribution"
                    ))
                })?;
            if snapshot.identity.retry_number != 0 {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution emission must belong to the first request of its logical primary step"
                        .to_owned(),
                ));
            }
            let current_logical_step_progress = read_logical_step_progress_tx(transaction)?;
            if *logical_step_origin != current_logical_step_progress {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution emission logical-step origin is not the store-owned start boundary"
                        .to_owned(),
                ));
            }
            if envelope.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
                || envelope.turn_id.as_ref() != Some(&snapshot.identity.turn)
                || envelope.event_id
                    != contribution_emission_event_id(request_id, producer, emission)
                || envelope.timestamp != started.timestamp
                || !contribution
                    .emissions
                    .iter()
                    .any(|candidate| candidate == emission)
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "contribution emission for request {request_id} does not match its prepared start"
                )));
            }
            let message = load_message_tx(transaction, message_id)?;
            contribution.validate_message(&message)?;
        }
        // A retry schedule is correlated by the failed request identity, not
        // by event position. It is valid only after that exact actual request
        // started and durably failed, and its ordinal must be the next shared
        // actual-request generation of the same logical turn.
        RuntimeEvent::ModelRetryScheduled {
            failed_request_id,
            retry_number,
            ..
        } => {
            let (snapshot, _, _) = require_started_request_tx(
                transaction,
                &envelope.conversation_id,
                failed_request_id,
            )?;
            if envelope.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
                || envelope.turn_id.as_ref() != Some(&snapshot.identity.turn)
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "model retry schedule for {failed_request_id} has a foreign attempt or turn envelope"
                )));
            }
            let expected_retry_number =
                snapshot
                    .identity
                    .retry_number
                    .checked_add(1)
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "model request retry ordinal is exhausted".to_owned(),
                        )
                    })?;
            if *retry_number != expected_retry_number {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "retry schedule for {failed_request_id} names ordinal {retry_number}, expected {expected_retry_number}"
                )));
            }
            let Some(outcome) = find_request_outcome_event(transaction, failed_request_id)? else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "retry schedule for {failed_request_id} has no provider outcome"
                )));
            };
            if !matches!(
                outcome.event,
                RuntimeEvent::ModelRequestFailed {
                    request_id,
                    ..
                } if request_id == *failed_request_id
            ) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "retry schedule for {failed_request_id} does not follow a failed provider outcome"
                )));
            }
        }
        // Provider terminal ownership is checked on the closed owner branch.
        // Agent terminals retain every existing Snapshot/P rule; auxiliary
        // terminals require a Call prerequisite and exactly one settlement.
        RuntimeEvent::ModelRequestCompleted {
            owner, request_id, ..
        }
        | RuntimeEvent::ModelRequestFailed {
            owner, request_id, ..
        } => match owner {
            ModelRequestOwner::Agent { .. } => {
                let Some(row) = read_request_snapshot_tx(transaction, request_id)? else {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "model request outcome references request {request_id}, which never started"
                    )));
                };
                let (snapshot, _, _) = validate_started_request_row_tx(
                    transaction,
                    &envelope.conversation_id,
                    request_id,
                    row,
                )?;
                if envelope.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
                    || envelope.turn_id.as_ref() != Some(&snapshot.identity.turn)
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "model request outcome for {request_id} has a foreign attempt or turn envelope"
                    )));
                }
                if find_request_outcome_event(transaction, request_id)?.is_some() {
                    return Err(ConversationStoreError::TerminalViolation(format!(
                        "request {request_id} already has a durable provider outcome"
                    )));
                }
            }
            ModelRequestOwner::Auxiliary { .. } => {
                auxiliary::validate_request_terminal_event_tx(transaction, envelope)?;
            }
        },
        // Every Tool Plane transition that names a proposal uses the same
        // semantic owner. This includes execution starts/progress/outcomes
        // and detached background authorization.
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id,
            tool_id,
        }
        | RuntimeEvent::ToolExecutionProgress {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionDeadlineFired {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionCancellationRequested {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionSettlementObserved {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionSettlementControlFailed {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionCompleted {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionFailed {
            tool_call_id,
            tool_id,
            ..
        } => {
            record_tool_proposal_dependency(
                transaction,
                tool_call_id,
                envelope.attempt_id.as_ref(),
                envelope.turn_id.as_ref(),
                Some(tool_id),
                runtime_event_dependency_name(&envelope.event),
                false,
                None,
            )?;
        }
        RuntimeEvent::BackgroundExecutionCommitted {
            tool_call_id,
            tool_id,
            ..
        } => {
            record_tool_proposal_dependency(
                transaction,
                tool_call_id,
                envelope.attempt_id.as_ref(),
                envelope.turn_id.as_ref(),
                Some(tool_id),
                runtime_event_dependency_name(&envelope.event),
                true,
                None,
            )?;
        }
        RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id,
            origin,
            workspace,
            ownership,
            ..
        } => {
            match (ownership, origin) {
                (crate::events::types::SubagentOwnershipKind::Workflow,
                 crate::runtime::subagent::AgentActivationOrigin::Workflow { node_id })
                    if node_id.block.run.conversation_id == envelope.conversation_id => {}
                (crate::events::types::SubagentOwnershipKind::Normal,
                 crate::runtime::subagent::AgentActivationOrigin::CreationTool { .. }
                 | crate::runtime::subagent::AgentActivationOrigin::MessageTool { .. }
                 | crate::runtime::subagent::AgentActivationOrigin::ClientControl) => {}
                _ => return Err(ConversationStoreError::InvalidReference(
                    "subagent ownership kind and origin must agree and name the admitting Workflow Conversation".into(),
                )),
            }
            if !workspace.is_absolute() {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} has a non-absolute execution workspace binding"
                )));
            }
            if let Some(tool_call_id) = origin.tool_call_id() {
                record_tool_proposal_dependency(
                    transaction,
                    tool_call_id,
                    envelope.attempt_id.as_ref(),
                    envelope.turn_id.as_ref(),
                    None,
                    runtime_event_dependency_name(&envelope.event),
                    true,
                    None,
                )?;
            }
            // The durable identity of an ownership fact is canonical: the
            // EventId must be the deterministic `subagent-committed-event:{id}`
            // derived from the very SubagentId embedded in the payload. A
            // mismatched pair is malformed and must never enter durable
            // authority; the authority rejects it rather than silently
            // rewriting or accepting it.
            let canonical = crate::runtime::subagent::subagent_ownership_event_id(subagent_id);
            if envelope.event_id != canonical {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent ownership event identity {} does not match the canonical identity {canonical} for {subagent_id}",
                    envelope.event_id
                )));
            }
            let key = format!("subagent:{subagent_id}");
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                    [&key],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    storage(format!("subagent ownership uniqueness probe: {error}"))
                })?;
            if exists {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "subagent {subagent_id} already has a durable ownership fact"
                )));
            }
        }
        // The interaction audit plane (Issue #109). Both facts carry the
        // canonical event identity of their interaction, so the durable
        // authority resolves the pair through the unique `event_id` index in
        // bounded time instead of scanning the Journal.
        RuntimeEvent::InteractionRequested {
            interaction_id,
            subject,
        } => {
            let canonical =
                crate::runtime::interaction::interaction_requested_event_id(interaction_id);
            if envelope.event_id != canonical {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "interaction requested event identity {} does not match the canonical identity {canonical} for {interaction_id}",
                    envelope.event_id
                )));
            }
            // Bounded payloads are a durable invariant, not a coordinator
            // convention: an interaction fact constructed or deserialized
            // outside the live coordinator is refused here by the very same
            // contract the coordinator validates against.
            validate_interaction_subject(subject).map_err(|message| {
                ConversationStoreError::InvalidReference(format!(
                    "interaction {interaction_id} requested an unbounded subject: {message}"
                ))
            })?;
            if envelope.attempt_id.is_some() || envelope.turn_id.is_some() {
                let (attempt_id, turn_id) = require_interaction_envelope(envelope, interaction_id)?;
                let workflow_node = match subject {
                    InteractionSubject::Review { review } => Some(review.instance.as_ref()),
                    InteractionSubject::Questionnaire {
                        invocation_id: crate::tools::types::ToolInvocationId::Workflow { node },
                        ..
                    } => Some(node.as_ref()),
                    _ => None,
                };
                if workflow_node.is_some_and(|node| {
                    node.block.run.conversation_id != envelope.conversation_id
                        || node.block.run.origin.attempt_id() != Some(attempt_id)
                }) {
                    return Err(ConversationStoreError::InvalidReference(
                        "interaction Workflow correlation differs from its owning envelope".into(),
                    ));
                }
                validate_approval_subject_against_canonical(
                    transaction,
                    &envelope.conversation_id,
                    interaction_id,
                    attempt_id,
                    turn_id,
                    subject,
                )?;
            } else {
                super::super::scheduled_workflow::validate_interaction_owner(
                    transaction,
                    &envelope.conversation_id,
                    interaction_id,
                    subject,
                    false,
                )?;
            }
            let key = format!("interaction:{interaction_id}");
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                    [&key],
                    |row| row.get(0),
                )
                .map_err(|error| storage(format!("interaction uniqueness probe: {error}")))?;
            if exists {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "interaction {interaction_id} already has a durable requested fact"
                )));
            }
        }
        RuntimeEvent::InteractionSettled {
            interaction_id,
            settlement,
        } => {
            let canonical =
                crate::runtime::interaction::interaction_settled_event_id(interaction_id);
            if envelope.event_id != canonical {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "interaction settled event identity {} does not match the canonical identity {canonical} for {interaction_id}",
                    envelope.event_id
                )));
            }
            // A settlement is meaningless without the request it settles: a
            // settled-without-requested fact would assert that a decision was
            // made about a prompt that never durably existed.
            let requested_id =
                crate::runtime::interaction::interaction_requested_event_id(interaction_id);
            let requested = find_event_by_id(transaction, &requested_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "interaction settlement has no durable requested fact for {interaction_id}"
                ))
            })?;
            let RuntimeEvent::InteractionRequested {
                interaction_id: embedded,
                subject,
            } = &requested.event
            else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "the interaction requested event {requested_id} is not the typed requested fact"
                )));
            };
            if embedded != interaction_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "interaction requested event {requested_id} belongs to {embedded}, not {interaction_id}"
                )));
            }
            if envelope.attempt_id.is_some() || envelope.turn_id.is_some() {
                let _ = require_interaction_envelope(envelope, interaction_id)?;
            } else {
                super::super::scheduled_workflow::validate_interaction_owner(
                    transaction,
                    &envelope.conversation_id,
                    interaction_id,
                    subject,
                    true,
                )?;
            }
            // Both facts of one interaction belong to the exact same
            // conversation + attempt + turn envelope. The conversation is
            // already guaranteed by `persist_event_tx`, which refuses a
            // foreign conversation outright; the attempt and the turn are
            // pinned here. Checking only the attempt would durably admit a
            // settlement committed under a later turn of the same attempt,
            // which the contract of a pinned audit pair forbids — and the
            // durable authority must not rely on the coordinator happening to
            // rebuild the same turn.
            if requested.attempt_id != envelope.attempt_id || requested.turn_id != envelope.turn_id
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "interaction {interaction_id} settled under a foreign attempt or turn envelope"
                )));
            }
            validate_interaction_settlement(subject, settlement).map_err(|message| {
                ConversationStoreError::InvalidReference(format!(
                    "interaction {interaction_id} settlement is not one its requested subject could produce: {message}"
                ))
            })?;
        }
        RuntimeEvent::AssistantMessageCommitted { message_id } => {
            if !ledger_message_exists(transaction, message_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "committed-message event references missing message {message_id}"
                )));
            }
            if !matches!(
                load_message_tx(transaction, message_id)?,
                MessageBlock::Assistant(_)
            ) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Assistant committed-message event references non-Assistant message {message_id}"
                )));
            }
        }
        RuntimeEvent::ToolMessageCommitted {
            message_id,
            tool_call_id,
        } => {
            if !ledger_message_exists(transaction, message_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "committed-message event references missing message {message_id}"
                )));
            }
            let message = load_message_tx(transaction, message_id)?;
            let MessageBlock::Tool(tool) = message else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Tool committed-message event references non-Tool message {message_id}"
                )));
            };
            if tool.tool_call_id != *tool_call_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Tool committed-message event for {message_id} references tool call {}, body answers {}",
                    tool_call_id, tool.tool_call_id
                )));
            }
            record_tool_proposal_dependency(
                transaction,
                tool_call_id,
                envelope.attempt_id.as_ref(),
                envelope.turn_id.as_ref(),
                Some(&tool.tool_id),
                runtime_event_dependency_name(&envelope.event),
                false,
                Some(&tool.occurrence.assistant_message_id),
            )?;
        }
        RuntimeEvent::CompactionCompleted {
            checkpoint_id,
            activation_id,
            generation,
            ..
        } => {
            let activation = projection::completion_activation(transaction, checkpoint_id)?;
            if activation.activation != *activation_id || *generation != activation_id.0 {
                return Err(ConversationStoreError::InvalidReference(
                    "compaction completion differs from atomic projection activation".into(),
                ));
            }
        }
        RuntimeEvent::ModelRequestStarted { owner, .. } => match owner {
            ModelRequestOwner::Agent { .. } => {
                let _ = reconstruct_request_start_reference(transaction, envelope, None)?;
            }
            ModelRequestOwner::Auxiliary { .. } => {
                auxiliary::validate_request_start_event_tx(transaction, envelope)?;
            }
        },
        RuntimeEvent::AuxiliaryRetryScheduled { .. } => {
            auxiliary::validate_retry_event_tx(transaction, envelope)?;
        }
        RuntimeEvent::AuxiliaryCallStarted { .. } | RuntimeEvent::AuxiliaryCallSettled { .. } => {
            auxiliary::validate_call_event_tx(transaction, envelope)?;
        }
        RuntimeEvent::BackgroundTerminalPublished { message_id, .. } => {
            let notification = load_user_notification_tx(transaction, message_id, "background")?;
            if notification.source != UserSource::Runtime
                || notification.kind != InboundKind::Message
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "background terminal fact references an ineligible notification {message_id}"
                )));
            }
        }
        RuntimeEvent::SubagentTerminalPublished {
            subagent_id,
            child_agent_id,
            message_id,
            state,
            ..
        } => {
            let expected_event_id = crate::runtime::subagent::terminal_event_id(subagent_id);
            if envelope.event_id != expected_event_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent terminal event identity {} does not match the canonical identity {expected_event_id}",
                    envelope.event_id
                )));
            }
            if envelope.attempt_id.is_some() || envelope.turn_id.is_some() {
                return Err(ConversationStoreError::InvalidReference(
                    "subagent terminal publication must not carry an attempt or turn envelope"
                        .to_owned(),
                ));
            }
            if *message_id != crate::runtime::subagent::terminal_message_id(subagent_id) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal references {message_id}, not its canonical report message {}",
                    crate::runtime::subagent::terminal_message_id(subagent_id)
                )));
            }
            let (committed_child_agent_id, _workspace, ownership, agent, _) =
                find_subagent_ownership(transaction, subagent_id)?;
            if ownership != crate::events::types::SubagentOwnershipKind::Normal {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} Workflow ownership cannot use parent terminal publication"
                )));
            }
            if committed_child_agent_id != *child_agent_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal claims child agent {child_agent_id}, but durable ownership committed {committed_child_agent_id}"
                )));
            }
            let publication = load_user_notification_tx(transaction, message_id, "subagent")?;
            let provenance_ok = match state {
                // A successful child answer is authored by the child agent;
                // every other terminal is a runtime-authored notice.
                crate::events::types::SubagentTerminalState::Succeeded => {
                    publication.source
                        == UserSource::Agent {
                            agent_id: child_agent_id.clone(),
                        }
                }
                crate::events::types::SubagentTerminalState::Failed
                | crate::events::types::SubagentTerminalState::Cancelled
                | crate::events::types::SubagentTerminalState::Interrupted => {
                    publication.source == UserSource::Runtime
                }
            };
            if !provenance_ok || publication.kind != InboundKind::Message {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent terminal fact references an ineligible publication {message_id}"
                )));
            }
            // A successful publication commits with its runtime-authored
            // terminal notice (Issue #192): the exact canonical notice of
            // this subagent and this named agent must already be durable in
            // the same transaction.
            if *state == crate::events::types::SubagentTerminalState::Succeeded {
                let notice = load_user_notification_tx(
                    transaction,
                    &crate::runtime::subagent::terminal_notice_message_id(subagent_id),
                    "subagent terminal",
                )?;
                let expected_notice = crate::runtime::subagent::terminal_notice_text(
                    child_agent_id,
                    subagent_id,
                    &agent,
                );
                let notice_text_ok = notice.source == UserSource::Runtime
                    && notice.kind == InboundKind::Message
                    && notice.content
                        == vec![UserContentBlock::Text(TextBlock {
                            text: expected_notice,
                        })];
                if !notice_text_ok {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "subagent {subagent_id} successful terminal lacks its canonical runtime-authored terminal notice"
                    )));
                }
            }
        }
        RuntimeEvent::SubagentPhysicalSettlementProven {
            subagent_id,
            child_agent_id,
        } => {
            if envelope.event_id
                != crate::runtime::subagent::physical_settlement_event_id(subagent_id)
                || envelope.attempt_id.is_some()
                || envelope.turn_id.is_some()
            {
                return Err(ConversationStoreError::InvalidReference(
                    "invalid physical settlement proof identity".into(),
                ));
            }
            let (owner, _, ownership, _, _) = find_subagent_ownership(transaction, subagent_id)?;
            let terminal_id = match ownership {
                crate::events::types::SubagentOwnershipKind::Normal => {
                    crate::runtime::subagent::terminal_event_id(subagent_id)
                }
                crate::events::types::SubagentOwnershipKind::Workflow => {
                    crate::runtime::subagent::terminal_settlement_event_id(subagent_id)
                }
            };
            let terminal: Option<String> = transaction
                .query_row(
                    "SELECT event_json FROM events WHERE event_id=?1",
                    [terminal_id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| {
                    storage(format!("physical settlement terminal lookup: {error}"))
                })?;
            let terminal = terminal.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "physical settlement without terminal publication".into(),
                )
            })?;
            let terminal: RuntimeEventEnvelope = decode(&terminal, "physical settlement terminal")?;
            if owner != *child_agent_id
                || !matches!(
                    terminal.event,
                    RuntimeEvent::SubagentTerminalPublished {
                        physical_settlement_proven: false,
                        ..
                    } | RuntimeEvent::SubagentTerminalSettled {
                        physical_settlement_proven: false,
                        ..
                    }
                )
            {
                return Err(ConversationStoreError::InvalidReference(
                    "physical settlement does not name an unresolved terminal".into(),
                ));
            }
        }
        RuntimeEvent::SubagentTerminalSettled {
            subagent_id,
            child_agent_id,
            ..
        } => {
            let expected_event_id =
                crate::runtime::subagent::terminal_settlement_event_id(subagent_id);
            if envelope.event_id != expected_event_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Workflow subagent terminal event identity {} does not match the canonical identity {expected_event_id}",
                    envelope.event_id
                )));
            }
            let (committed_child_agent_id, _workspace, ownership, _agent, _) =
                find_subagent_ownership(transaction, subagent_id)?;
            if ownership != crate::events::types::SubagentOwnershipKind::Workflow {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} normal ownership cannot use Workflow terminal settlement"
                )));
            }
            if committed_child_agent_id != *child_agent_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Workflow subagent {subagent_id} terminal claims child agent {child_agent_id}, but durable ownership committed {committed_child_agent_id}"
                )));
            }
        }
        RuntimeEvent::WorkflowAgentOutputCommitted {
            subagent_id,
            workflow_id,
            run_id,
            node_id,
            output,
            ..
        } => {
            let expected_event_id = crate::runtime::subagent::workflow_output_event_id(subagent_id);
            if envelope.event_id != expected_event_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Workflow output event identity {} does not match the canonical identity {expected_event_id}",
                    envelope.event_id
                )));
            }
            if node_id.node.is_empty()
                || node_id.node.len() > 64
                || node_id.block.definition.blocks.len()
                    > 2 * crate::runtime::workflow::MAX_BLOCK_DEPTH
                || node_id
                    .block
                    .definition
                    .blocks
                    .iter()
                    .any(|key| key.is_empty() || key.len() > 64)
                || node_id.block.invocations.len() != node_id.block.definition.blocks.len() / 2 + 1
            {
                return Err(ConversationStoreError::InvalidReference(
                    "Workflow output event has an invalid node identity".to_owned(),
                ));
            }
            if &node_id.block.run != run_id || &node_id.block.definition.workflow_id != workflow_id
            {
                return Err(ConversationStoreError::InvalidReference(
                    "Workflow output run and definition disagree with its node identity".to_owned(),
                ));
            }
            let encoded = serde_json::to_vec(output)
                .map_err(|error| storage(format!("Workflow output event encoding: {error}")))?;
            if encoded.len() > crate::runtime::subagent::MAX_RESULT_CONTENT_BYTES {
                return Err(ConversationStoreError::InvalidReference(
                    "Workflow output event exceeds the bounded value size".to_owned(),
                ));
            }
            let (_, _, ownership, _, origin) = find_subagent_ownership(transaction, subagent_id)?;
            if ownership != crate::events::types::SubagentOwnershipKind::Workflow {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} normal ownership cannot commit a Workflow output"
                )));
            }
            if !matches!(origin, crate::runtime::subagent::AgentActivationOrigin::Workflow { node_id: owned } if owned.as_ref() == node_id)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "Workflow output node differs from the committed child ownership origin"
                        .to_owned(),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}
