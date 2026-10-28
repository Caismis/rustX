//! Borrowed original-event capture, independent of optional body/summary layouts.
use super::{
    Connection, ConversationStoreError, OptionalExtension, TraceAdoptionClass, TraceBootstrapFact,
    TraceCanonicalSummary, TraceFactData, TraceFactHeader, TraceFactKind, TraceFieldKind,
    TraceFieldSource, TraceTerminalClass, TraceTerminalHeader, TraceToolProposalSummary, decode,
    encode, header, params, storage, value_source,
};
use crate::events::types::{RuntimeEvent as E, RuntimeEventEnvelope};
use crate::observation::trace::{TraceLocation, TracePreview, TraceToolOutcome};
use crate::runtime::types::CancellationReason as R;
use crate::tools::types::ToolExecutionStatus as Status;

fn find(
    connection: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Option<u64>, ConversationStoreError> {
    connection
        .query_row(sql, args, |row| row.get(0))
        .optional()
        .map_err(|error| storage(error.to_string()))
}
fn attempt_start(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<Option<u64>, ConversationStoreError> {
    let Some(attempt) = &event.attempt_id else {
        return Ok(None);
    };
    find(
        connection,
        "SELECT sequence FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_id=?1 AND json_extract(event_json,'$.event.type')='attempt_started' AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",
        params![attempt.as_str(), event.sequence],
    )
}
fn step_start(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<Option<u64>, ConversationStoreError> {
    let (Some(attempt), Some(step)) = (&event.attempt_id, &event.turn_id) else {
        return Ok(None);
    };
    find(
        connection,
        "SELECT sequence FROM events INDEXED BY events_step_kind_idx WHERE attempt_id=?1 AND turn_id=?2 AND json_extract(event_json,'$.event.type')='turn_started' AND sequence<=?3 ORDER BY sequence DESC LIMIT 1",
        params![attempt.as_str(), step.as_str(), event.sequence],
    )
}
fn location_attempt_source(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<Option<u64>, ConversationStoreError> {
    let Some(attempt) = &event.attempt_id else {
        return Ok(None);
    };
    let started = attempt_start(connection, event)?;
    // Native diagnostic requests may exist without a distinct AttemptStarted.
    // Their actual J envelope owns the same original field; it does not prove
    // that an AttemptStarted fact occurred. Keep the earliest real selector.
    let request = find(
        connection,
        "SELECT sequence FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_id=?1 AND json_extract(event_json,'$.event.type')='model_request_started' AND sequence<=?2 ORDER BY sequence LIMIT 1",
        params![attempt.as_str(), event.sequence],
    )?;
    Ok(started.into_iter().chain(request).min())
}
fn location_step_source(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<Option<u64>, ConversationStoreError> {
    let (Some(attempt), Some(step)) = (&event.attempt_id, &event.turn_id) else {
        return Ok(None);
    };
    let started = step_start(connection, event)?;
    let request = find(
        connection,
        "SELECT sequence FROM events INDEXED BY events_step_kind_idx WHERE attempt_id=?1 AND turn_id=?2 AND json_extract(event_json,'$.event.type')='model_request_started' AND sequence<=?3 ORDER BY sequence LIMIT 1",
        params![attempt.as_str(), step.as_str(), event.sequence],
    )?;
    Ok(started.into_iter().chain(request).min())
}
fn location(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<TraceLocation, ConversationStoreError> {
    let attempt_id = event
        .attempt_id
        .as_ref()
        .filter(|id| id.as_str().len() <= 512)
        .cloned();
    let attempt_id_source = if attempt_id.is_none() {
        event
            .attempt_id
            .as_ref()
            .map(|id| {
                let start = location_attempt_source(connection, event)?
                    .ok_or_else(|| storage("original Trace Attempt field source unavailable"))?;
                super::super::observation_identity::journal(start, id)
            })
            .transpose()?
    } else {
        None
    };
    let step_id = event
        .turn_id
        .as_ref()
        .filter(|id| id.as_str().len() <= 512)
        .cloned();
    let step_id_source = if step_id.is_none() {
        event
            .turn_id
            .as_ref()
            .map(|id| {
                let start = location_step_source(connection, event)?
                    .ok_or_else(|| storage("original Trace Step field source unavailable"))?;
                value_source(
                    TraceFieldKind::StepId,
                    start,
                    vec!["turn_id".into()],
                    &id.as_str(),
                )
            })
            .transpose()?
    } else {
        None
    };
    Ok(TraceLocation {
        attempt_id,
        attempt_id_source,
        step_id,
        step_id_source,
    })
}
fn alias(
    fields: &mut Vec<TraceFieldSource>,
    event: &RuntimeEventEnvelope,
    kind: TraceFieldKind,
    name: &str,
    value: &str,
) -> Result<(), ConversationStoreError> {
    if value.len() > 512 {
        fields.push(value_source(
            kind,
            event.sequence,
            vec!["event".into(), name.into()],
            &value,
        )?);
    }
    Ok(())
}
fn terminal(class: TraceTerminalClass) -> TraceTerminalHeader {
    TraceTerminalHeader {
        class,
        tool_outcome: None,
        detail: None,
        failure_kind: None,
        usage: None,
        generation: None,
    }
}
fn cancel_detail(status: &Status) -> Option<String> {
    let Status::Cancelled { reason, phase } = status else {
        return None;
    };
    let label = match reason {
        R::UserRequested => "cancelled by user request",
        R::RuntimeShutdown => "cancelled by runtime shutdown",
        R::ParentCancelled => "cancelled because its parent was cancelled",
        R::SubagentExecutionDeadlineExceeded => "cancelled by the Subagent execution deadline",
    };
    let phase = match phase {
        crate::tools::types::ToolCancellationPhase::BeforeStart => "before execution started",
        crate::tools::types::ToolCancellationPhase::DuringExecution => "proven stopped after start",
    };
    Some(format!("{label} ({phase})"))
}
fn status(status: &Status) -> (TraceTerminalClass, TraceToolOutcome, Option<TracePreview>) {
    let (class, outcome, text) = match status {
        Status::Success => (
            TraceTerminalClass::Completed,
            TraceToolOutcome::Success,
            None,
        ),
        Status::Failed { error } => (
            TraceTerminalClass::Failed,
            TraceToolOutcome::Failed,
            Some(error.as_str()),
        ),
        Status::Denied { reason } => (
            TraceTerminalClass::Denied,
            TraceToolOutcome::Denied,
            Some(reason.as_str()),
        ),
        Status::Cancelled { .. } => {
            // The closed cancellation labels are finite; this helper never
            // formats a free-form original result or diagnostic.
            let text = cancel_detail(status);
            return (
                TraceTerminalClass::Cancelled,
                TraceToolOutcome::Cancelled,
                text.as_deref().map(TracePreview::of),
            );
        }
        Status::TimedOut => (
            TraceTerminalClass::TimedOut,
            TraceToolOutcome::TimedOut,
            None,
        ),
        Status::OutcomeUnknown { detail } => (
            TraceTerminalClass::OutcomeUnknown,
            TraceToolOutcome::OutcomeUnknown,
            Some(detail.as_str()),
        ),
    };
    (class, outcome, text.map(TracePreview::of))
}
fn position(
    connection: &Connection,
    id: &crate::runtime::identity::MessageId,
) -> Result<Option<u64>, ConversationStoreError> {
    find(
        connection,
        "SELECT position FROM message_ledger WHERE message_id=?1",
        [id.as_str()],
    )
}
fn terminal_owner(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<Option<u64>, ConversationStoreError> {
    let before = event.sequence;
    match &event.event{
        E::AttemptCompleted{..}|E::AttemptFailed{..}|E::AttemptCancelled{..}|E::AttemptTimedOut{..}|E::AttemptLimitExceeded{..}=>attempt_start(connection,event),
        E::TurnCompleted=>step_start(connection,event),
        E::ModelRequestCompleted{request_id,..}|E::ModelRequestFailed{request_id,..}=>find(connection,"SELECT sequence FROM events INDEXED BY events_request_id_kind_idx WHERE json_extract(event_json,'$.event.request_id')=?1 AND json_extract(event_json,'$.event.type')='model_request_started' AND sequence<?2 ORDER BY sequence DESC LIMIT 1",params![request_id.as_str(),before]),
        E::ToolExecutionCompleted{tool_call_id,tool_id,..}|E::ToolExecutionFailed{tool_call_id,tool_id,..}=>{
            if let Some(occurrence)=&event.tool_occurrence{
                find(connection,"SELECT sequence FROM events INDEXED BY events_observation_tool_lifecycle_idx WHERE observation_tool_ledger=?1 AND observation_tool_block=?2 AND observation_tool_phase=1 AND sequence<?3 ORDER BY sequence DESC LIMIT 1",params![occurrence.ledger_position,occurrence.block_index.get(),before])
            }else{
                find(connection,"SELECT sequence FROM events INDEXED BY events_tool_call_scope_kind_idx WHERE json_extract(event_json,'$.event.tool_call_id')=?1 AND attempt_id IS ?2 AND turn_id IS ?3 AND json_extract(event_json,'$.event.type')='tool_execution_started' AND json_extract(event_json,'$.event.tool_id')=?4 AND sequence<?5 ORDER BY sequence DESC LIMIT 1",params![tool_call_id.as_str(),event.attempt_id.as_ref().map(crate::runtime::identity::AttemptId::as_str),event.turn_id.as_ref().map(crate::runtime::identity::TurnId::as_str),tool_id.as_str(),before])
            }
        },
        E::ToolMessageCommitted{..}=>{
            event.tool_occurrence.as_ref().map(|occurrence|find(connection,"SELECT sequence FROM events INDEXED BY events_observation_tool_lifecycle_idx WHERE observation_tool_ledger=?1 AND observation_tool_block=?2 AND observation_tool_phase=1 AND sequence<?3 ORDER BY sequence DESC LIMIT 1",params![occurrence.ledger_position,occurrence.block_index.get(),before])).transpose().map(Option::flatten)
        },
        E::CompactionCompleted{..}|E::CompactionFailed{..}=>find(connection,"SELECT sequence FROM events INDEXED BY events_kind_idx WHERE json_extract(event_json,'$.event.type')='compaction_started' AND sequence<?1 ORDER BY sequence DESC LIMIT 1",[before]),
        E::BackgroundTerminalPublished{execution_id,..}=>find(connection,"SELECT sequence FROM events INDEXED BY events_execution_id_kind_idx WHERE json_extract(event_json,'$.event.execution_id')=?1 AND json_extract(event_json,'$.event.type')='background_execution_committed' AND sequence<?2 ORDER BY sequence DESC LIMIT 1",params![execution_id.as_str(),before]),
        E::SubagentTerminalPublished{subagent_id,..}|E::SubagentTerminalSettled{subagent_id,..}=>find(connection,"SELECT sequence FROM events INDEXED BY events_subagent_id_kind_idx WHERE json_extract(event_json,'$.event.subagent_id')=?1 AND json_extract(event_json,'$.event.type')='subagent_ownership_committed' AND sequence<?2 ORDER BY sequence DESC LIMIT 1",params![subagent_id.as_str(),before]),
        E::WorkflowCompleted{run_id,..}|E::WorkflowFailed{run_id,..}|E::WorkflowCancelled{run_id,..}=>{
            let (kind,identity)=match &run_id.origin {
                crate::runtime::workflow::WorkflowRunOrigin::Tool{attempt_id}=>("tool",attempt_id.as_str()),
                crate::runtime::workflow::WorkflowRunOrigin::Scheduled{start_id}=>("scheduled",start_id.as_str()),
            };
            find(connection,"SELECT sequence FROM events INDEXED BY events_workflow_coordinates_idx WHERE json_extract(event_json,'$.event.run_id.conversation_id')=?1 AND json_extract(event_json,'$.event.run_id.origin.type')=?2 AND coalesce(json_extract(event_json,'$.event.run_id.origin.attempt_id'),json_extract(event_json,'$.event.run_id.origin.start_id'))=?3 AND json_extract(event_json,'$.event.run_id.invocation')=?4 AND json_extract(event_json,'$.event.type')='workflow_started' AND sequence<?5 ORDER BY sequence DESC LIMIT 1",params![run_id.conversation_id.as_str(),kind,identity,run_id.invocation,before])
        },
        E::InteractionSettled{interaction_id,..}=>find(connection,"SELECT sequence FROM events INDEXED BY events_interaction_id_kind_idx WHERE json_extract(event_json,'$.event.interaction_id')=?1 AND json_extract(event_json,'$.event.type')='interaction_requested' AND sequence<?2 ORDER BY sequence DESC LIMIT 1",params![interaction_id.as_str(),before]),
        _=>Ok(Some(before)),
    }
}
fn adoption(
    connection: &Connection,
    sequence: u64,
    ids: &[crate::runtime::identity::MessageId],
) -> Result<TraceFactData, ConversationStoreError> {
    let mut first = Vec::with_capacity(ids.len().min(8));
    let mut human = true;
    let mut sender = None;
    let mut unanimous = true;
    for (ordinal, id) in ids.iter().enumerate() {
        let position =
            position(connection, id)?.ok_or_else(|| storage("original adopted C absent"))?;
        if first.len() < 8 {
            first.push(position);
        }
        // Numeric membership is captured from the original borrowed adopted IDs.
        // Its digest binds each selected coordinate to that same native C identity.
        let inserted = connection.execute(
            "INSERT INTO trace_adoption_members(source_sequence,ordinal,ledger_position,identity_digest) SELECT ?1,?2,position,observation_identity_digest FROM message_ledger WHERE position=?3 AND typeof(observation_identity_digest)='blob' AND length(observation_identity_digest)=32",
            params![sequence,ordinal as u64,position],
        ).map_err(|error|storage(error.to_string()))?;
        if inserted != 1 {
            return Err(storage("original adopted C identity evidence unavailable"));
        }
        let bytes: Option<Vec<u8>> = connection
            .query_row(
                "SELECT observation_trace_summary FROM message_ledger WHERE position=?1",
                [position],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        let summary: TraceCanonicalSummary = super::decode(
            &bytes.ok_or_else(|| storage("native adopted classification unavailable"))?,
        )?;
        human &= summary.ordinary_input
            && matches!(
                summary.source,
                Some(crate::message::types::UserSource::Human)
            );
        if let Some(crate::message::types::UserSource::Agent { agent_id }) = summary.source {
            if let Some(previous) = &sender {
                unanimous &= previous == &agent_id;
            } else {
                sender = Some(agent_id);
            }
        } else {
            unanimous = false;
        }
    }
    Ok(TraceFactData::Adoption {
        total: ids.len() as u64,
        first_ledger_positions: first,
        classification: if human {
            TraceAdoptionClass::Human
        } else {
            TraceAdoptionClass::Context
        },
        unanimous_agent: unanimous.then_some(sender).flatten(),
    })
}
fn core_data(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    fields: &mut Vec<TraceFieldSource>,
) -> Result<TraceFactData, ConversationStoreError> {
    match &event.event {
        E::AttemptStarted { .. } => Ok(TraceFactData::AttemptStart),
        E::TurnStarted => Ok(TraceFactData::StepStart),
        E::InboundTurnAdopted { message_ids } => adoption(connection, event.sequence, message_ids),
        E::ModelRequestStarted { request_id, .. } => {
            alias(
                fields,
                event,
                TraceFieldKind::RequestId,
                "request_id",
                request_id.as_str(),
            )?;
            Ok(TraceFactData::RequestStart {
                start_sequence: event.sequence,
                retry_number: None,
                request_id: (request_id.as_str().len() <= 512).then(|| request_id.clone()),
                assistant_message_id: None,
            })
        }
        E::AssistantMessageCommitted { message_id } => {
            alias(
                fields,
                event,
                TraceFieldKind::MessageId,
                "message_id",
                message_id.as_str(),
            )?;
            Ok(TraceFactData::AssistantCommit {
                ledger_position: position(connection, message_id)?
                    .ok_or_else(|| storage("native Assistant C absent"))?,
            })
        }
        E::ToolExecutionStarted {
            tool_call_id,
            tool_id,
        } => {
            alias(
                fields,
                event,
                TraceFieldKind::CallId,
                "tool_call_id",
                tool_call_id.as_str(),
            )?;
            alias(
                fields,
                event,
                TraceFieldKind::ToolId,
                "tool_id",
                tool_id.as_str(),
            )?;
            Ok(TraceFactData::ToolStart {
                occurrence: event.tool_occurrence.clone(),
                call_id: (tool_call_id.as_str().len() <= 512).then(|| tool_call_id.clone()),
                tool_id: (tool_id.as_str().len() <= 512).then(|| tool_id.clone()),
            })
        }
        E::ToolMessageCommitted { message_id, .. } => Ok(TraceFactData::CanonicalToolCommit {
            occurrence: event.tool_occurrence.clone(),
            ledger_position: position(connection, message_id)?
                .ok_or_else(|| storage("native Tool C absent"))?,
        }),
        E::CompactionStarted { operation_id } => {
            alias(
                fields,
                event,
                TraceFieldKind::NativeId,
                "operation_id",
                operation_id.as_str(),
            )?;
            Ok(TraceFactData::CompactionStart {
                native_id: (operation_id.as_str().len() <= 512).then(|| operation_id.to_string()),
            })
        }
        _ => Err(storage("not a typed native Trace start")),
    }
}
fn actor_data(
    event: &RuntimeEventEnvelope,
    fields: &mut Vec<TraceFieldSource>,
) -> Result<TraceFactData, ConversationStoreError> {
    match &event.event {
        E::BackgroundExecutionCommitted {
            execution_id,
            tool_call_id,
            ..
        } => {
            alias(
                fields,
                event,
                TraceFieldKind::CallId,
                "tool_call_id",
                tool_call_id.as_str(),
            )?;
            Ok(TraceFactData::BackgroundStart {
                native_id: Some(execution_id.to_string()),
                originating_tool_call_id: (tool_call_id.as_str().len() <= 512)
                    .then(|| tool_call_id.clone()),
            })
        }
        E::SubagentOwnershipCommitted {
            subagent_id,
            child_agent_id,
            origin,
            agent,
            ..
        } => {
            alias(
                fields,
                event,
                TraceFieldKind::NativeId,
                "subagent_id",
                subagent_id.as_str(),
            )?;
            Ok(TraceFactData::SubagentStart {
                activation_id: (subagent_id.as_str().len() <= 512).then(|| subagent_id.clone()),
                agent_id: child_agent_id.clone(),
                origin: crate::observation::bounded::bounded_clone(origin, 1024),
                agent_preview: TracePreview::of(agent),
            })
        }
        E::WorkflowStarted {
            run_id,
            tool_call_id,
            ..
        } => {
            if let Some(tool_call_id) = tool_call_id {
                alias(
                    fields,
                    event,
                    TraceFieldKind::CallId,
                    "tool_call_id",
                    tool_call_id.as_str(),
                )?;
            }
            let native_id = crate::observation::bounded::bounded_bytes(run_id, 512)
                .map(|bytes| String::from_utf8(bytes).expect("native JSON UTF8"));
            if native_id.is_none() {
                fields.push(value_source(
                    TraceFieldKind::NativeId,
                    event.sequence,
                    vec!["event".into(), "run_id".into()],
                    run_id,
                )?);
            }
            Ok(TraceFactData::WorkflowStart {
                native_id,
                originating_tool_call_id: tool_call_id
                    .as_ref()
                    .filter(|id| id.as_str().len() <= 512)
                    .cloned(),
            })
        }
        E::InteractionRequested { interaction_id, .. } => {
            alias(
                fields,
                event,
                TraceFieldKind::NativeId,
                "interaction_id",
                interaction_id.as_str(),
            )?;
            Ok(TraceFactData::InteractionStart {
                native_id: (interaction_id.as_str().len() <= 512)
                    .then(|| interaction_id.to_string()),
            })
        }
        _ => Err(storage("not a typed native Trace start")),
    }
}
fn request_tool_data(
    event: &RuntimeEventEnvelope,
    fields: &mut Vec<TraceFieldSource>,
) -> Result<TraceFactData, ConversationStoreError> {
    use TraceTerminalClass as C;
    let ending = match &event.event {
        E::ModelRequestCompleted {
            usage, generation, ..
        } => {
            let mut end = terminal(C::Completed);
            end.usage.clone_from(usage);
            end.generation = *generation;
            end
        }
        E::ModelRequestFailed {
            error,
            usage,
            generation,
            ..
        } => {
            fields.push(value_source(
                TraceFieldKind::Failure,
                event.sequence,
                vec!["event".into(), "error".into()],
                error,
            )?);
            let mut end = terminal(match error.kind {
                crate::model::error::ModelErrorKind::Cancelled => C::Cancelled,
                crate::model::error::ModelErrorKind::Timeout => C::TimedOut,
                _ => C::Failed,
            });
            end.failure_kind = Some(error.kind.clone());
            end.usage.clone_from(usage);
            end.generation = *generation;
            end
        }
        E::ToolExecutionCompleted { result, .. } => {
            fields.push(value_source(
                TraceFieldKind::Result,
                event.sequence,
                vec!["event".into(), "result".into()],
                result,
            )?);
            let (class, outcome, detail) = status(&result.status);
            let mut end = terminal(class);
            end.tool_outcome = Some(outcome);
            end.detail = detail;
            end
        }
        E::ToolExecutionFailed { error, .. } => {
            fields.push(value_source(
                TraceFieldKind::Failure,
                event.sequence,
                vec!["event".into(), "error".into()],
                error,
            )?);
            let mut end = terminal(C::Failed);
            end.tool_outcome = Some(TraceToolOutcome::Failed);
            end.detail = Some(TracePreview::of(error));
            end
        }
        E::WorkflowFailed {
            status: original, ..
        } => {
            let (class, outcome, detail) = status(original);
            let mut end = terminal(class);
            end.tool_outcome = Some(outcome);
            end.detail = detail;
            end
        }
        _ => return Err(storage("not a typed native Trace terminal")),
    };
    Ok(TraceFactData::Terminal(ending))
}
fn terminal_data(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    fields: &mut Vec<TraceFieldSource>,
) -> Result<TraceFactData, ConversationStoreError> {
    use TraceTerminalClass as C;
    let ending = match &event.event {
        E::AttemptCompleted { finish_reason, .. } => {
            fields.push(value_source(
                TraceFieldKind::Result,
                event.sequence,
                vec!["event".into(), "finish_reason".into()],
                finish_reason,
            )?);
            terminal(C::Completed)
        }
        E::TurnCompleted | E::WorkflowCompleted { .. } | E::CompactionCompleted { .. } => {
            terminal(C::Completed)
        }
        E::AttemptFailed { .. } => {
            if let Some(summary) = super::super::observation_execution::summary_at_sequence(
                connection,
                event.sequence,
            )? && summary.failure_readable_end > 0
            {
                fields.push(TraceFieldSource::Value {
                    field: TraceFieldKind::Failure,
                    domain: format!("journal.{}", event.sequence),
                    item_offset: 0,
                    path: vec!["normalized_failure".into()],
                    encoded_bytes: summary.failure_readable_end,
                });
            }
            terminal(C::Failed)
        }
        E::CompactionFailed { error, .. } => {
            fields.push(value_source(
                TraceFieldKind::Failure,
                event.sequence,
                vec!["event".into(), "error".into()],
                error,
            )?);
            terminal(C::Failed)
        }
        E::AttemptCancelled { .. } | E::WorkflowCancelled { .. } => terminal(C::Cancelled),
        E::AttemptTimedOut { .. } => terminal(C::TimedOut),
        E::AttemptLimitExceeded { .. } => terminal(C::Limited),
        _ => return Err(storage("not a typed native Trace terminal")),
    };
    Ok(TraceFactData::Terminal(ending))
}
fn foreign_terminal_data(
    event: &RuntimeEventEnvelope,
) -> Result<TraceFactData, ConversationStoreError> {
    use TraceTerminalClass as C;
    let ending = match &event.event {
        E::BackgroundTerminalPublished { state, .. } => {
            use crate::events::types::BackgroundTerminalState as S;
            terminal(match state {
                S::Succeeded => C::Completed,
                S::Failed => C::Failed,
                S::Cancelled => C::Cancelled,
                S::Denied => C::Denied,
                S::TimedOut => C::TimedOut,
                S::OutcomeUnknown => C::OutcomeUnknown,
            })
        }
        E::SubagentTerminalPublished { state, .. } | E::SubagentTerminalSettled { state, .. } => {
            use crate::events::types::SubagentTerminalState as S;
            terminal(match state {
                S::Succeeded => C::Completed,
                S::Failed => C::Failed,
                S::Cancelled => C::Cancelled,
                S::Interrupted => C::Interrupted,
            })
        }
        E::InteractionSettled { settlement, .. } => {
            use crate::events::interaction::InteractionSettlement as S;
            terminal(match settlement {
                S::Approved | S::Reviewed { .. } | S::QuestionnaireSubmitted { .. } => C::Completed,
                S::Denied { .. } | S::QuestionnaireDeclined => C::Denied,
                S::Cancelled { .. } => C::Cancelled,
                S::DeadlineExpired { .. } => C::TimedOut,
            })
        }
        _ => return Err(storage("not a typed native Trace terminal")),
    };
    Ok(TraceFactData::Terminal(ending))
}
fn data(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    fields: &mut Vec<TraceFieldSource>,
) -> Result<TraceFactData, ConversationStoreError> {
    match &event.event {
        E::AttemptStarted { .. }
        | E::TurnStarted
        | E::InboundTurnAdopted { .. }
        | E::ModelRequestStarted { .. }
        | E::AssistantMessageCommitted { .. }
        | E::ToolExecutionStarted { .. }
        | E::ToolMessageCommitted { .. }
        | E::CompactionStarted { .. } => core_data(connection, event, fields),
        E::BackgroundExecutionCommitted { .. }
        | E::SubagentOwnershipCommitted { .. }
        | E::WorkflowStarted { .. }
        | E::InteractionRequested { .. } => actor_data(event, fields),
        E::ModelRequestCompleted { .. }
        | E::ModelRequestFailed { .. }
        | E::ToolExecutionCompleted { .. }
        | E::ToolExecutionFailed { .. }
        | E::WorkflowFailed { .. } => request_tool_data(event, fields),
        E::BackgroundTerminalPublished { .. }
        | E::SubagentTerminalPublished { .. }
        | E::SubagentTerminalSettled { .. }
        | E::InteractionSettled { .. } => foreign_terminal_data(event),
        _ => terminal_data(connection, event, fields),
    }
}
pub(in crate::durable::sqlite) fn index_event(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    if event.event.is_auxiliary_model_request()
        || matches!(
            event.event,
            E::AuxiliaryCallStarted { .. } | E::AuxiliaryCallSettled { .. }
        )
    {
        return Ok(());
    }
    let Some(kind) =
        TraceFactKind::parse(&super::super::observation_event::native_kind(&event.event)?)
    else {
        return Ok(());
    };
    if let E::ModelRequestStarted { request_id, .. } = &event.event {
        let retained: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM request_snapshots WHERE request_id=?1)",
                [request_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        if retained {
            return Ok(());
        } // The native caller captures core after actual J binding.
    }
    let mut canonical_fields_ready = None;
    if let E::AssistantMessageCommitted { message_id } = &event.event {
        // Presentation refresh is separate from this J row's own identity,
        // location and lifecycle proof. Reads still prefer retained raw spans.
        let staged: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM publication_streams INDEXED BY publication_streams_message_settlement_idx WHERE message_id=?1 AND settlement IS NULL)",
            [message_id.as_str()], |row| row.get(0),
        ).map_err(|error| storage(error.to_string()))?;
        canonical_fields_ready = Some(
            !staged
                && super::super::optional_display_layout(connection, || {
                    refresh_raw_bindings(
                        connection,
                        &event.conversation_id,
                        position(connection, message_id)?
                            .ok_or_else(|| storage("original Assistant C absent"))?,
                    )
                })?
                .is_some(),
        );
        let _ = super::super::optional_display_layout(connection, || {
            super::capture_definitions(
                connection,
                event,
                position(connection, message_id)?
                    .ok_or_else(|| storage("original Assistant C absent"))?,
                message_id,
            )
        })?;
        let _ = super::super::optional_display_layout(connection, || {
            capture_accepted_relation(connection, event, message_id)
        })?;
    }
    let mut fields = Vec::new();
    let header = TraceFactHeader {
        sequence: event.sequence,
        timestamp: event.timestamp,
        kind,
        owner: event
            .event
            .model_request_owner()
            .map(crate::durable::observation::RequestOwnerKind::from),
        owner_anchor: terminal_owner(connection, event)?,
        location: location(connection, event)?,
        fact: data(connection, event, &mut fields)?,
        bootstrap: bootstrap(connection, event)?,
        canonical_fields_ready,
        fields,
    };
    save(connection, &header)
}
fn save(connection: &Connection, header: &TraceFactHeader) -> Result<(), ConversationStoreError> {
    connection.execute("UPDATE events SET observation_trace_fact=?2,observation_trace_owner=?3 WHERE sequence=?1",params![header.sequence,encode(header)?,header.owner_anchor]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}
pub(in crate::durable::sqlite) fn request_core(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    snapshot: &crate::model::RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let mut fields = Vec::new();
    alias(
        &mut fields,
        event,
        TraceFieldKind::RequestId,
        "request_id",
        snapshot.request_id.as_str(),
    )?;
    if snapshot.provisional_message_id.as_str().len() > 512 {
        fields.push(TraceFieldSource::Value {
            field: TraceFieldKind::ProvisionalMessageId,
            domain: format!(
                "trace.records.{}.detail.request.assistant_message_id",
                event.sequence
            ),
            item_offset: 0,
            path: Vec::new(),
            encoded_bytes: super::super::observation_request_summary::serialized_bytes(
                &snapshot.provisional_message_id.as_str(),
            )?,
        });
    }
    let header = TraceFactHeader {
        sequence: event.sequence,
        timestamp: event.timestamp,
        kind: TraceFactKind::ModelRequestStarted,
        owner: event
            .event
            .model_request_owner()
            .map(crate::durable::observation::RequestOwnerKind::from),
        owner_anchor: Some(event.sequence),
        location: location(connection, event)?,
        bootstrap: None,
        canonical_fields_ready: None,
        fields,
        fact: TraceFactData::RequestStart {
            start_sequence: event.sequence,
            retry_number: Some(snapshot.identity.retry_number),
            request_id: (snapshot.request_id.as_str().len() <= 512)
                .then(|| snapshot.request_id.clone()),
            assistant_message_id: (snapshot.provisional_message_id.as_str().len() <= 512)
                .then(|| snapshot.provisional_message_id.clone()),
        },
    };
    let step = step_start(connection, event)?;
    connection.execute("UPDATE request_snapshots SET observation_step_anchor=?2,observation_retry_number=?3 WHERE started_sequence=?1",params![event.sequence,step,snapshot.identity.retry_number]).map_err(|error|storage(error.to_string()))?;
    save(connection, &header)
}
pub(super) fn tool_status(status: &Status) -> (TraceToolOutcome, Option<TracePreview>) {
    let (_, outcome, detail) = self::status(status);
    (outcome, detail)
}

fn bootstrap(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<Option<TraceBootstrapFact>, ConversationStoreError> {
    Ok(match &event.event {
        E::AttemptCancelled { reason, .. } => {
            Some(TraceBootstrapFact::AttemptCancelled { reason: *reason })
        }
        E::AttemptLimitExceeded { limit, .. } => {
            Some(TraceBootstrapFact::AttemptLimited { limit: *limit })
        }
        E::CompactionFailed { error, .. } => Some(TraceBootstrapFact::CompactionFailed {
            detail: TracePreview::of(error),
        }),
        E::CompactionCompleted { .. } => {
            super::super::observation_occupancy::witness(connection, event).map(|reading| {
                TraceBootstrapFact::CompactionCompleted {
                    reading: Box::new(reading),
                }
            })
        }
        E::TurnStarted => {
            let attempt = event
                .attempt_id
                .as_ref()
                .ok_or_else(|| storage("original Turn has no Attempt"))?;
            let previous = find(
                connection,
                "SELECT sequence FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_id=?1 AND json_extract(event_json,'$.event.type')='turn_started' AND sequence<?2 ORDER BY sequence DESC LIMIT 1",
                params![attempt.as_str(), event.sequence],
            )?;
            let number = if let Some(previous) = previous {
                match header(connection, previous, event.sequence)?.bootstrap {
                    Some(TraceBootstrapFact::Turn { number }) => number
                        .checked_add(1)
                        .ok_or_else(|| storage("native Turn count overflow"))?,
                    _ => return Err(storage("native Turn count evidence unavailable")),
                }
            } else {
                1
            };
            Some(TraceBootstrapFact::Turn { number })
        }
        _ => None,
    })
}

#[cfg(test)]
thread_local! {static COMPARE_WORK:std::cell::Cell<(usize,usize,usize,usize)>=const {std::cell::Cell::new((0,0,0,0))};}
#[cfg(test)]
pub(in crate::durable::sqlite) fn accepted_compare_work() -> (usize, usize, usize, usize) {
    COMPARE_WORK.with(|work| work.replace((0, 0, 0, 0)))
}
/// Compare the frozen original field, without parsing the growing request object.
pub(in crate::durable::sqlite) fn accepted_identity_equal(
    connection: &Connection,
    start: u64,
    through: u64,
    identity: &str,
) -> Result<bool, ConversationStoreError> {
    struct Compare<'a> {
        connection: &'a Connection,
        start: u64,
        through: u64,
        bytes: Vec<u8>,
        index: usize,
        offset: u64,
        end: u64,
        same: bool,
        failure: Option<ConversationStoreError>,
    }
    impl std::io::Write for Compare<'_> {
        fn write(&mut self, expected: &[u8]) -> std::io::Result<usize> {
            if self.index == self.bytes.len() {
                match super::super::observation_request::read_at(
                    self.connection,
                    self.start,
                    self.through,
                    &["assistant_message_id".into()],
                    self.offset,
                    65_536,
                ) {
                    Ok(chunk) => {
                        #[cfg(test)]
                        {
                            let (rows, bytes) = super::super::content::last_read_work();
                            COMPARE_WORK.with(|work| {
                                let (calls, max_rows, max_bytes, total) = work.get();
                                work.set((
                                    calls + 1,
                                    max_rows.max(rows),
                                    max_bytes.max(bytes),
                                    total + chunk.bytes.len(),
                                ));
                            });
                        }

                        self.bytes = chunk.bytes;
                        self.index = 0;
                        self.end = chunk.readable_end;
                    }
                    Err(error) => {
                        self.failure = Some(error);
                        return Err(std::io::Error::other("original identity field unavailable"));
                    }
                }
            }
            let count = expected.len().min(self.bytes.len() - self.index);
            if count == 0 || expected[..count] != self.bytes[self.index..self.index + count] {
                self.same = false;
                return Err(std::io::Error::other("original identity differs"));
            }
            self.index += count;
            self.offset += count as u64;
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut compare = Compare {
        connection,
        start,
        through,
        bytes: Vec::new(),
        index: 0,
        offset: 0,
        end: 0,
        same: true,
        failure: None,
    };
    let result = serde_json::to_writer(&mut compare, &identity);
    if let Some(error) = compare.failure {
        return Err(error);
    }
    Ok(result.is_ok() && compare.same && compare.offset == compare.end)
}

fn refresh_raw_bindings(
    connection: &Connection,
    conversation: &crate::runtime::identity::ConversationId,
    position: u64,
) -> Result<(), ConversationStoreError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT observation_trace_summary FROM message_ledger WHERE position=?1",
            [position],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let mut summary: TraceCanonicalSummary =
        decode(&bytes.ok_or_else(|| storage("native canonical Trace summary unavailable"))?)?;
    for call in &mut summary.calls {
        for field in &mut call.fields {
            prefer_original_raw(connection, conversation, field)?;
        }
    }
    connection
        .execute(
            "UPDATE message_ledger SET observation_trace_summary=?2 WHERE position=?1",
            params![position, encode(&summary)?],
        )
        .map_err(|error| storage(error.to_string()))?;
    let mut statement=connection.prepare_cached("SELECT block_index,observation_trace_proposal FROM canonical_tool_calls INDEXED BY canonical_tool_calls_observation_idx WHERE observation_ledger=?1 ORDER BY block_index").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map([position], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
        })
        .map_err(|error| storage(error.to_string()))?;
    for row in rows {
        let (block, bytes) = row.map_err(|error| storage(error.to_string()))?;
        let mut proposal: TraceToolProposalSummary =
            decode(&bytes.ok_or_else(|| storage("native proposal header unavailable"))?)?;
        for field in &mut proposal.fields {
            prefer_original_raw(connection, conversation, field)?;
        }
        connection.execute("UPDATE canonical_tool_calls SET observation_trace_proposal=?3 WHERE observation_ledger=?1 AND block_index=?2",params![position,block,encode(&proposal)?]).map_err(|error|storage(error.to_string()))?;
    }
    Ok(())
}
/// Mandatory original raw retention precedes this optional presentation update.
/// A failed update leaves the original J header's false readiness marker intact.
pub(in crate::durable::sqlite) fn settle_fields(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let E::AssistantMessageCommitted { message_id } = &event.event else {
        return Ok(());
    };
    refresh_raw_bindings(
        connection,
        &event.conversation_id,
        position(connection, message_id)?.ok_or_else(|| storage("original Assistant C absent"))?,
    )?;
    let mut fact = super::header(connection, event.sequence, event.sequence)?;
    fact.canonical_fields_ready = Some(true);
    save(connection, &fact)
}
pub(in crate::durable::sqlite) fn prefer_original_raw(
    connection: &Connection,
    conversation: &crate::runtime::identity::ConversationId,
    field: &mut TraceFieldSource,
) -> Result<(), ConversationStoreError> {
    if let TraceFieldSource::Content {
        field: TraceFieldKind::Arguments,
        binding,
    } = field
    {
        let raw = super::super::content::descriptor(
            connection,
            conversation,
            &binding.content_ref.source,
            crate::observation::content::ContentRepresentation::RawJsonUtf8,
        )?;
        if let Some(raw) = raw {
            binding.content_ref = raw;
        } else if binding.content_ref.representation
            == crate::observation::content::ContentRepresentation::RawJsonUtf8
        {
            return Err(storage("required original raw Tool arguments unavailable"));
        }
    }
    Ok(())
}

// Optional accepted-input presentation cannot suppress this Assistant's own
// original C/J fact when the request body read layout is missing.
fn capture_accepted_relation(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    message_id: &crate::runtime::identity::MessageId,
) -> Result<(), ConversationStoreError> {
    if let (Some(attempt), Some(step)) = (&event.attempt_id, &event.turn_id) {
        let start = find(
            connection,
            "SELECT sequence FROM events INDEXED BY events_step_kind_idx WHERE attempt_id=?1 AND turn_id=?2 AND json_extract(event_json,'$.event.type')='model_request_started' AND sequence<?3 ORDER BY sequence DESC LIMIT 1",
            params![attempt.as_str(), step.as_str(), event.sequence],
        )?;
        if let Some(start) = start {
            // Coordinates select the candidate; exact original frozen
            // provisional equality alone proves this accepted relation.
            let equal =
                accepted_identity_equal(connection, start, event.sequence, message_id.as_str())?;
            if equal {
                connection.execute("UPDATE request_snapshots SET observation_accepted_ledger=?2 WHERE started_sequence=?1",params![start,position(connection,message_id)?]).map_err(|error|storage(error.to_string()))?;
            }
        }
    }
    Ok(())
}
