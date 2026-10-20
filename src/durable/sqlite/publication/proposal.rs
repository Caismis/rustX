//! Publication proposal ownership and tool-proposal dependencies.

use crate::durable::ConversationStoreError;
use crate::durable::sqlite::canonical::ledger::{load_head, load_message_tx};
use crate::durable::sqlite::storage;
use crate::message::types::{AssistantContentBlock, ContentBlockIndex, MessageBlock};
use crate::publication::{
    PublicationAuditBlock, PublicationFrame, PublicationPayload, PublicationSettlement,
};
use crate::runtime::identity::{
    AttemptId, MessageId, PublicationStreamId, ToolCallId, ToolId, TurnId,
};
use rusqlite::OptionalExtension;
use rusqlite::{Transaction, params};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::durable::sqlite) enum PublicationProposalState {
    Started,
    Completed,
}

impl PublicationProposalState {
    pub(in crate::durable::sqlite) fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
        }
    }

    pub(in crate::durable::sqlite) fn parse(value: &str) -> Result<Self, ConversationStoreError> {
        match value {
            "started" => Ok(Self::Started),
            "completed" => Ok(Self::Completed),
            other => Err(ConversationStoreError::InvalidReference(format!(
                "publication proposal has unknown state {other}"
            ))),
        }
    }
}

/// The store-owned identity and assembly state of one staged proposal.
///
/// `persisted_state` is `None` for a proposal first created by the current
/// transaction. Keeping it alongside the working state lets one transaction
/// stage `Started` and `Completed` together without inserting an intermediate
/// row or losing the exact state transition.
#[derive(Debug, Clone)]
pub(in crate::durable::sqlite) struct PublicationProposalOwner {
    pub(in crate::durable::sqlite) call_id: ToolCallId,
    pub(in crate::durable::sqlite) block_index: ContentBlockIndex,
    pub(in crate::durable::sqlite) tool_id: ToolId,
    pub(in crate::durable::sqlite) name: String,
    pub(in crate::durable::sqlite) state: PublicationProposalState,
    pub(in crate::durable::sqlite) persisted_state: Option<PublicationProposalState>,
}

pub(in crate::durable::sqlite) fn load_publication_proposal_owners(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
) -> Result<Vec<PublicationProposalOwner>, ConversationStoreError> {
    let mut statement = transaction
        .prepare(
            "SELECT call_id,block_index,tool_id,tool_name,state
             FROM publication_proposals WHERE stream_id=?1 ORDER BY call_id",
        )
        .map_err(|error| storage(format!("publication proposal owner query: {error}")))?;
    let rows = statement
        .query_map([stream_id.as_str()], |row| {
            let block_index: i64 = row.get(1)?;
            let state: String = row.get(4)?;
            Ok((
                row.get::<_, String>(0)?,
                block_index,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                state,
            ))
        })
        .map_err(|error| storage(format!("publication proposal owner rows: {error}")))?;
    rows.map(|row| {
        let (call_id, block_index, tool_id, name, state) =
            row.map_err(|error| storage(format!("publication proposal owner row: {error}")))?;
        let block_index = u32::try_from(block_index).map_err(|_| {
            ConversationStoreError::InvalidReference(
                "publication proposal block index is invalid".to_owned(),
            )
        })?;
        let state = PublicationProposalState::parse(&state)?;
        Ok(PublicationProposalOwner {
            call_id: ToolCallId::new(call_id),
            block_index: ContentBlockIndex::new(block_index),
            tool_id: ToolId::new(tool_id),
            name,
            state,
            persisted_state: Some(state),
        })
    })
    .collect()
}

pub(in crate::durable::sqlite) fn proposal_owner<'a>(
    owners: &'a mut [PublicationProposalOwner],
    call_id: &ToolCallId,
) -> Option<&'a mut PublicationProposalOwner> {
    owners.iter_mut().find(|owner| owner.call_id == *call_id)
}

pub(in crate::durable::sqlite) fn proposal_violation(
    stream_id: &PublicationStreamId,
    detail: impl Into<String>,
) -> ConversationStoreError {
    ConversationStoreError::PublicationViolation(format!(
        "publication stream {stream_id} proposal violation: {}",
        detail.into()
    ))
}

pub(in crate::durable::sqlite) fn require_started_proposal<'a>(
    owners: &'a mut [PublicationProposalOwner],
    stream_id: &PublicationStreamId,
    call_id: &ToolCallId,
) -> Result<&'a mut PublicationProposalOwner, ConversationStoreError> {
    let owner = proposal_owner(owners, call_id).ok_or_else(|| {
        proposal_violation(
            stream_id,
            format!("tool proposal {call_id} has no Started frame"),
        )
    })?;
    if owner.state == PublicationProposalState::Completed {
        return Err(proposal_violation(
            stream_id,
            format!("tool proposal {call_id} is already completed"),
        ));
    }
    Ok(owner)
}

pub(in crate::durable::sqlite) fn validate_proposal_frames(
    stream_id: &PublicationStreamId,
    frames: &[PublicationFrame],
    owners: &mut Vec<PublicationProposalOwner>,
) -> Result<(), ConversationStoreError> {
    for frame in frames {
        match &frame.payload {
            PublicationPayload::ProposedToolCallStarted { block_index, call } => {
                if proposal_owner(owners, &call.id).is_some() {
                    return Err(proposal_violation(
                        stream_id,
                        format!("tool proposal {} already has a Started frame", call.id),
                    ));
                }
                owners.push(PublicationProposalOwner {
                    call_id: call.id.clone(),
                    block_index: *block_index,
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                    state: PublicationProposalState::Started,
                    persisted_state: None,
                });
            }
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index,
                call_id,
                ..
            } => {
                let owner = require_started_proposal(owners, stream_id, call_id)?;
                if owner.block_index != *block_index {
                    return Err(proposal_violation(
                        stream_id,
                        format!(
                            "tool proposal {call_id} arguments use block {block_index}, but block {} is frozen",
                            owner.block_index
                        ),
                    ));
                }
            }
            PublicationPayload::ProposedToolCallCompleted { block_index, call } => {
                let owner = require_started_proposal(owners, stream_id, &call.id)?;
                if owner.block_index != *block_index {
                    return Err(proposal_violation(
                        stream_id,
                        format!(
                            "tool proposal {} completes block {block_index}, but block {} is frozen",
                            call.id, owner.block_index
                        ),
                    ));
                }
                if owner.tool_id != call.tool_id {
                    return Err(proposal_violation(
                        stream_id,
                        format!("tool proposal {} completion changes tool id", call.id),
                    ));
                }
                if owner.name != call.name {
                    return Err(proposal_violation(
                        stream_id,
                        format!("tool proposal {} completion changes tool name", call.id),
                    ));
                }
                owner.state = PublicationProposalState::Completed;
            }
            PublicationPayload::TextSuffix { .. }
            | PublicationPayload::ReasoningSuffix { .. }
            | PublicationPayload::RefusalSuffix { .. }
            | PublicationPayload::TerminalOnly => {}
        }
    }
    Ok(())
}

/// Proves that every proposal materialized into an audit is backed by the
/// stream-local durable owner created by the staging state machine.
pub(in crate::durable::sqlite) fn validate_audit_proposal_ownership(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
    content: &[PublicationAuditBlock],
) -> Result<(), ConversationStoreError> {
    for block in content {
        let PublicationAuditBlock::ProposedToolCall {
            block_index,
            call_id,
            tool_id,
            name,
            complete,
            ..
        } = block
        else {
            continue;
        };
        let owner: Option<(i64, String, String, String)> = transaction
            .query_row(
                "SELECT block_index,tool_id,tool_name,state
                 FROM publication_proposals WHERE stream_id=?1 AND call_id=?2",
                params![stream_id.as_str(), call_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| storage(format!("audit proposal owner probe: {error}")))?;
        let Some((stored_block_index, stored_tool_id, stored_name, stored_state)) = owner else {
            return Err(proposal_violation(
                stream_id,
                format!("audit proposal {call_id} has no durable Started owner"),
            ));
        };
        let stored_block_index = u32::try_from(stored_block_index).map_err(|_| {
            ConversationStoreError::InvalidReference(
                "publication proposal block index is invalid".to_owned(),
            )
        })?;
        let stored_state = PublicationProposalState::parse(&stored_state)?;
        if ContentBlockIndex::new(stored_block_index) != *block_index
            || stored_tool_id != tool_id.as_str()
            || stored_name != *name
        {
            return Err(proposal_violation(
                stream_id,
                format!("audit proposal {call_id} disagrees with its frozen owner identity"),
            ));
        }
        let expected_state = if *complete {
            PublicationProposalState::Completed
        } else {
            PublicationProposalState::Started
        };
        if stored_state != expected_state {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "audit proposal {call_id} has state {}, expected {}",
                    stored_state.as_str(),
                    expected_state.as_str()
                ),
            ));
        }
    }
    Ok(())
}

/// C must accept exactly the completed proposal set that the publication
/// stream durably assembled. This is the C-side owner of the final
/// proposal-state boundary; Tool Plane events can then resolve the retained
/// canonical owner without reconstructing frame JSON.
pub(in crate::durable::sqlite) fn validate_canonical_tool_proposals(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    let MessageBlock::Assistant(assistant) = message else {
        return Ok(());
    };
    let owners = load_publication_proposal_owners(transaction, stream_id)?;
    let mut accepted_call_ids = BTreeSet::new();
    for (index, block) in assistant.content.iter().enumerate() {
        let AssistantContentBlock::ToolCall(call) = block else {
            continue;
        };
        if !accepted_call_ids.insert(call.id.as_str().to_owned()) {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "canonical Assistant contains duplicate ToolCall {}",
                    call.id
                ),
            ));
        }
        let block_index = u32::try_from(index)
            .map(ContentBlockIndex::new)
            .map_err(|_| storage("canonical Assistant block index overflow"))?;
        let Some(owner) = owners.iter().find(|owner| owner.call_id == call.id) else {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "canonical ToolCall {} has no stream-local Started owner",
                    call.id
                ),
            ));
        };
        if owner.block_index != block_index
            || owner.tool_id != call.tool_id
            || owner.name != call.name
        {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "canonical ToolCall {} disagrees with its frozen owner",
                    call.id
                ),
            ));
        }
        if owner.state != PublicationProposalState::Completed {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "canonical ToolCall {} has no durable Completed frame",
                    call.id
                ),
            ));
        }
    }
    for owner in owners {
        match owner.state {
            PublicationProposalState::Started => {
                return Err(proposal_violation(
                    stream_id,
                    format!(
                        "canonical acceptance leaves Started-only proposal {}",
                        owner.call_id
                    ),
                ));
            }
            PublicationProposalState::Completed
                if !accepted_call_ids.contains(owner.call_id.as_str()) =>
            {
                return Err(proposal_violation(
                    stream_id,
                    format!(
                        "canonical Assistant omits Completed proposal {}",
                        owner.call_id
                    ),
                ));
            }
            PublicationProposalState::Completed => {}
        }
    }
    Ok(())
}

/// Rejects every dependent Tool Plane transition for a proposal that belongs
/// to a settled publication audit, and records the dependency otherwise.
///
/// This is the durable half of the hard Issue #108 invariant: no tool
/// proposal from an Incomplete or Unaccepted publication may have a dependent
/// `ToolExecutionStarted`, `ToolResult`, or side-effect authorization.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One owner validates execution envelopes and exact canonical references.
pub(in crate::durable::sqlite) fn record_tool_proposal_dependency(
    transaction: &Transaction<'_>,
    call_id: &ToolCallId,
    attempt_id: Option<&AttemptId>,
    turn_id: Option<&TurnId>,
    expected_tool_id: Option<&ToolId>,
    dependency: &str,
    allow_unowned_canonical: bool,
    canonical_owner: Option<&MessageId>,
) -> Result<(), ConversationStoreError> {
    // Detached authorization facts intentionally outlive an attempt and carry
    // no envelope generation. Resolve those facts through the current durable
    // Surface: an active canonical Assistant is the only valid owner, and
    // ambiguous publication owners are rejected. This keeps a historical
    // audited/canonical reuse from becoming a bare-call-id alias.
    let detached_active_messages =
        if canonical_owner.is_none() && attempt_id.is_none() && turn_id.is_none() {
            Some(load_head(transaction)?.active_message_ids)
        } else {
            None
        };
    let mut statement = transaction
        .prepare(
            "SELECT p.stream_id,s.attempt_id,s.turn_id,s.message_id,p.tool_id,p.settlement,p.state
             FROM publication_proposals p
             JOIN publication_streams s ON s.stream_id=p.stream_id
             WHERE p.call_id=?1
             ORDER BY p.stream_id",
        )
        .map_err(|error| storage(format!("tool proposal probe: {error}")))?;
    let rows = statement
        .query_map([call_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| storage(format!("tool proposal rows: {error}")))?;
    let candidates = rows
        .map(|row| row.map_err(|error| storage(format!("tool proposal row: {error}"))))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if candidates.is_empty() {
        // A direct canonical Assistant commit predates the publication plane
        // in a few durable recovery prefixes. It is a valid owner only when
        // the active Surface still contains that exact ToolCall. An arbitrary
        // call id with no publication owner and no canonical Assistant owner
        // is a malformed foreground dependency, not an idempotent no-op. A
        // direct canonical ToolMessage or detached lifecycle opening may
        // still be admitted without a model proposal: neither is a
        // ToolResult or execution fact for a publication proposal.
        let owner_tool_id = if let Some(owner) = canonical_owner {
            let message = load_message_tx(transaction, owner)?;
            match message {
                MessageBlock::Assistant(assistant) => {
                    assistant.content.iter().find_map(|block| match block {
                        AssistantContentBlock::ToolCall(call) if call.id == *call_id => {
                            Some(call.tool_id.clone())
                        }
                        _ => None,
                    })
                }
                _ => None,
            }
        } else {
            canonical_surface_tool_id(transaction, call_id)?
        };
        if let Some(owner_tool_id) = owner_tool_id {
            if let Some(expected_tool_id) = expected_tool_id
                && *expected_tool_id != owner_tool_id
            {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "{dependency} for tool call {call_id} uses tool id {expected_tool_id}, but the canonical Assistant owner freezes {owner_tool_id}"
                )));
            }
            return Ok(());
        }
        if allow_unowned_canonical {
            return Ok(());
        }
        return Err(ConversationStoreError::PublicationViolation(format!(
            "{dependency} for tool call {call_id} has no durable proposal or canonical Assistant owner"
        )));
    }
    let matching: Vec<_> = candidates
        .iter()
        .filter(
            |(_, candidate_attempt, candidate_turn, candidate_message, _, _, _)| {
                canonical_owner.is_none_or(|owner| candidate_message == owner.as_str())
                    && attempt_id.is_none_or(|attempt| candidate_attempt == attempt.as_str())
                    && turn_id.is_none_or(|turn| candidate_turn == turn.as_str())
                    && detached_active_messages.as_ref().is_none_or(|active| {
                        active
                            .iter()
                            .any(|message_id| message_id.as_str() == candidate_message)
                    })
            },
        )
        .collect();
    if matching.is_empty() {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "{dependency} for tool call {call_id} does not match any exact publication generation"
        )));
    }
    let canonical: Vec<_> = matching
        .iter()
        .filter(|(_, _, _, _, _, settlement, _)| {
            settlement.as_deref() == Some(PublicationSettlement::Canonical.as_str())
        })
        .collect();
    let selected = match canonical.len().cmp(&1) {
        std::cmp::Ordering::Equal => canonical[0],
        std::cmp::Ordering::Greater => {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "{dependency} for tool call {call_id} has ambiguous canonical proposal ownership"
            )));
        }
        std::cmp::Ordering::Less => {
            let audited = matching.iter().find_map(|(_, _, _, _, _, settlement, _)| {
                settlement.as_deref().filter(|settlement| {
                    *settlement == PublicationSettlement::Incomplete.as_str()
                        || *settlement == PublicationSettlement::Unaccepted.as_str()
                })
            });
            if let Some(settlement) = audited {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "tool call {call_id} is a model proposal of an {settlement} publication and may never execute or acquire {dependency}"
                )));
            }
            if matching.len() != 1 {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "{dependency} for tool call {call_id} has ambiguous unsettled proposal ownership"
                )));
            }
            matching[0]
        }
    };
    if let Some(expected_tool_id) = expected_tool_id
        && selected.4.as_str() != expected_tool_id.as_str()
    {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "{dependency} for tool call {call_id} uses tool id {expected_tool_id}, but the frozen proposal owner uses {}",
            selected.4
        )));
    }
    let state = &selected.6;
    let state = PublicationProposalState::parse(state)?;
    if state != PublicationProposalState::Completed {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "{dependency} for tool call {call_id} has no durable Completed proposal frame"
        )));
    }
    transaction
        .execute(
            "UPDATE publication_proposals SET executed=1 WHERE stream_id=?1 AND call_id=?2",
            params![selected.0.as_str(), call_id.as_str()],
        )
        .map_err(|error| storage(format!("record tool proposal dependency: {error}")))?;
    Ok(())
}

/// Resolves the non-publication canonical path used by older recovery
/// prefixes without turning a missing publication owner into a bare call-id
/// authority. The active canonical Assistant is the only durable fact that
/// can authorize a Tool Plane transition when no publication proposal row
/// exists, and it freezes the tool ID used for that authorization.
pub(in crate::durable::sqlite) fn canonical_surface_tool_id(
    transaction: &Transaction<'_>,
    call_id: &ToolCallId,
) -> Result<Option<ToolId>, ConversationStoreError> {
    let head = load_head(transaction)?;
    for message_id in head.active_message_ids {
        let message = load_message_tx(transaction, &message_id)?;
        let MessageBlock::Assistant(assistant) = message else {
            continue;
        };
        if let Some(tool_id) = assistant.content.iter().find_map(|block| match block {
            AssistantContentBlock::ToolCall(call) if call.id == *call_id => {
                Some(call.tool_id.clone())
            }
            _ => None,
        }) {
            return Ok(Some(tool_id));
        }
    }
    Ok(None)
}

/// The canonical Assistant message that owns `call_id` inside the exact
/// durable generation `(attempt_id, turn_id)` names.
///
/// A `ToolCallId` is request/publication-scoped, never conversation-global, so
/// "a canonical `ToolCall` with this id exists somewhere" is not ownership.
/// The retained FND-03 publication owner is: the proposal row is keyed by
/// `(stream_id, call_id)`, its stream froze the attempt, turn, and Assistant
/// message identity when it opened, and canonical acceptance stamped both rows
/// `canonical`. Resolving through that join is what makes the generation part
/// of the identity rather than an afterthought.
///
/// Returns `None` when this generation proposed no such canonical call. An
/// ambiguous canonical owner is a durable contradiction and is reported as
/// one, matching the guard [`record_tool_proposal_dependency`] already applies.
pub(in crate::durable::sqlite) fn canonical_generation_owner(
    transaction: &Transaction<'_>,
    call_id: &ToolCallId,
    attempt_id: &AttemptId,
    turn_id: &TurnId,
) -> Result<Option<MessageId>, ConversationStoreError> {
    let mut statement = transaction
        .prepare(
            "SELECT s.message_id
             FROM publication_proposals p
             JOIN publication_streams s ON s.stream_id=p.stream_id
             WHERE p.call_id=?1 AND p.settlement=?2
               AND s.attempt_id=?3 AND s.turn_id=?4
             ORDER BY s.message_id",
        )
        .map_err(|error| storage(format!("canonical generation owner probe: {error}")))?;
    let rows = statement
        .query_map(
            params![
                call_id.as_str(),
                PublicationSettlement::Canonical.as_str(),
                attempt_id.as_str(),
                turn_id.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| storage(format!("canonical generation owner rows: {error}")))?;
    let owners = rows
        .map(|row| row.map_err(|error| storage(format!("canonical generation owner row: {error}"))))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    match owners.len() {
        0 => Ok(None),
        1 => Ok(Some(MessageId::new(owners[0].clone()))),
        _ => Err(ConversationStoreError::PublicationViolation(format!(
            "tool call {call_id} has ambiguous canonical proposal ownership in attempt {attempt_id} turn {turn_id}"
        ))),
    }
}
