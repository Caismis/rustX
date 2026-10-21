//! Bounded summary joins over finite own facts, canonical headers and request summaries.
use super::TraceProjection;
use super::anchor::AnchorFacts;
use crate::durable::ConversationStoreError;
use crate::durable::observation::{TraceFactData as D, TraceFactHeader};
#[cfg(test)]
use crate::events::types::RuntimeEvent as E;
use crate::observation::trace::bounds::{
    TRACE_RECORD_BYTES, TracePreview, encoded_len, identity_fits,
};
#[cfg(test)]
use crate::observation::trace::types::TraceState;
use crate::observation::trace::types::{
    TraceFieldKind, TraceFieldSource, TraceRecord, TraceRequestSummary,
};
#[cfg(test)]
use crate::tools::types::ToolExecutionStatus;

impl TraceProjection<'_> {
    pub(super) fn record(
        &self,
        anchor: &TraceFactHeader,
    ) -> Result<TraceRecord, ConversationStoreError> {
        let mut facts = self.anchor_facts(anchor)?;
        let summary = if let Some(request) = &facts.request {
            let summary = self.request_summary(request, &facts.fields)?;
            if let Some((_, preview)) = &summary {
                facts.preview = Some(preview.clone());
            }
            summary.map(|(summary, _)| summary)
        } else {
            None
        };
        self.enrich_record(anchor, &mut facts)?;
        let AnchorFacts {
            id,
            position,
            location,
            kind,
            state,
            timing,
            preview,
            calls,
            native_id,
            agent_id,
            activation_id,
            activation_origin,
            originating_tool_call_id,
            message_id,
            attachments,
            tool,
            has_detail,
            truncated,
            fields,
            collections,
            ..
        } = facts;
        Ok(TraceRecord {
            id,
            position,
            location,
            kind,
            state,
            timing,
            preview,
            calls,
            native_id,
            agent_id,
            activation_id,
            activation_origin,
            originating_tool_call_id,
            message_id,
            attachments,
            tool,
            has_detail,
            truncated,
            fields,
            collections,
            request: summary,
        })
    }
    fn enrich_record(
        &self,
        anchor: &TraceFactHeader,
        facts: &mut AnchorFacts,
    ) -> Result<(), ConversationStoreError> {
        match &anchor.fact {
            D::Adoption {
                first_ledger_positions,
                ..
            } => {
                for position in first_ledger_positions {
                    let canonical = self
                        .store
                        .observation_trace_canonical_summary(self.cut()?, *position)?;
                    if facts.preview.is_none() {
                        facts.preview = canonical.preview;
                    }
                    if facts.message_id.is_none() {
                        facts.message_id = canonical.identity;
                    }
                    for artifact in canonical.attachments {
                        if facts
                            .attachments
                            .iter()
                            .any(|known| known.artifact_id == artifact.artifact_id)
                        {
                            continue;
                        }
                        if facts.attachments.len() < 32 {
                            facts.attachments.push(artifact);
                        } else {
                            facts.truncated = true;
                        }
                    }
                    facts.fields.extend(canonical.fields);
                    facts.collections.extend(canonical.collections);
                    facts.truncated |= canonical.truncated;
                }
            }
            D::ToolStart {
                occurrence: Some(occurrence),
                ..
            } => {
                super::summary::record_tool_proposal();
                let proposal = self
                    .store
                    .observation_trace_tool_proposal(self.cut()?, occurrence)?;
                if let Some(tool) = &mut facts.tool {
                    tool.name = proposal.name;
                    tool.arguments = proposal.arguments;
                    tool.fields.extend(proposal.fields);
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn request_summary(
        &self,
        request: &super::anchor::AnchorRequest,
        fields: &[TraceFieldSource],
    ) -> Result<Option<(TraceRequestSummary, TracePreview)>, ConversationStoreError> {
        let Some(summary) = self
            .store
            .observation_request_summary_at(request.start_sequence, self.through)?
        else {
            return Ok(None);
        };
        if request
            .retry_number
            .is_some_and(|number| number != summary.retry_number)
        {
            return Err(ConversationStoreError::Storage(
                "request summary ordinal differs from native core".into(),
            ));
        }
        let preview = summary.model_preview.clone();
        let previous = self.previous_request(request.start_sequence)?;
        let (system_prompt, tool_catalog) = previous.presentation(&summary);
        let context = self.context_presentation(request.start_sequence)?;
        Ok(Some((
            TraceRequestSummary {
                accepted_message_locator: self
                    .store
                    .observation_request_accepted_at(self.cut()?, request.start_sequence)?,
                request_id: summary.request_id,
                request_id_source: field(fields, TraceFieldKind::RequestId),
                assistant_message_id: summary.assistant_message_id,
                assistant_message_id_source: field(fields, TraceFieldKind::ProvisionalMessageId),
                model: summary.model,
                model_source: field(fields, TraceFieldKind::Model),
                retry_number: summary.retry_number,
                previous_failure_kind: self
                    .store
                    .observation_request_retry_failure_at(request.start_sequence, self.through)?,
                failure_kind: request.failure_kind.clone(),
                usage: request.usage.clone(),
                generation: request.generation,
                system_prompt,
                predecessor: previous.identity(),
                tool_catalog,
                context_truncated: context.next_offset.is_some()
                    || context
                        .items
                        .iter()
                        .any(|item| item.truncated || item.message_id.is_none()),
                context_additions: context.items,
                context_total: context.total,
                context_domain: context.domain,
                context_next_offset: context.next_offset,
            },
            preview,
        )))
    }
}
pub(super) fn field(fields: &[TraceFieldSource], kind: TraceFieldKind) -> Option<TraceFieldSource> {
    fields
        .iter()
        .find(|source| match source {
            TraceFieldSource::Value { field, .. } | TraceFieldSource::Content { field, .. } => {
                *field == kind
            }
        })
        .cloned()
}

/// The typed state proven by one terminal fact.
#[cfg(test)]
pub(super) fn terminal(event: &E) -> TraceState {
    match event {
        E::AttemptCompleted { .. }
        | E::TurnCompleted
        | E::ModelRequestCompleted { .. }
        | E::CompactionCompleted { .. }
        | E::WorkflowCompleted { .. } => TraceState::Completed,
        E::InteractionSettled { settlement, .. } => {
            use crate::events::interaction::InteractionSettlement as S;
            match settlement {
                S::Approved | S::Reviewed { .. } | S::QuestionnaireSubmitted { .. } => {
                    TraceState::Completed
                }
                S::Denied { .. } | S::QuestionnaireDeclined => TraceState::Denied,
                S::Cancelled { .. } => TraceState::Cancelled,
                S::DeadlineExpired { .. } => TraceState::TimedOut,
            }
        }
        E::AttemptCancelled { .. } | E::WorkflowCancelled { .. } => TraceState::Cancelled,
        E::AttemptTimedOut { .. } => TraceState::TimedOut,
        E::AttemptLimitExceeded { .. } => TraceState::Limited,
        E::ModelRequestFailed { error, .. } => match error.kind {
            crate::model::error::ModelErrorKind::Cancelled => TraceState::Cancelled,
            crate::model::error::ModelErrorKind::Timeout => TraceState::TimedOut,
            _ => TraceState::Failed,
        },
        E::AttemptFailed { .. } | E::CompactionFailed { .. } | E::ToolExecutionFailed { .. } => {
            TraceState::Failed
        }
        E::ToolExecutionCompleted { result, .. } => tool_state(&result.status),
        E::WorkflowFailed { status, .. } => tool_state(status),
        E::SubagentTerminalPublished { state, .. } | E::SubagentTerminalSettled { state, .. } => {
            use crate::events::types::SubagentTerminalState as S;
            match state {
                S::Succeeded => TraceState::Completed,
                S::Failed => TraceState::Failed,
                S::Cancelled => TraceState::Cancelled,
                S::Interrupted => TraceState::Interrupted,
            }
        }
        E::BackgroundTerminalPublished { state, .. } => {
            use crate::events::types::BackgroundTerminalState as S;
            match state {
                S::Succeeded => TraceState::Completed,
                S::Failed => TraceState::Failed,
                S::Cancelled => TraceState::Cancelled,
                S::Denied => TraceState::Denied,
                S::TimedOut => TraceState::TimedOut,
                S::OutcomeUnknown => TraceState::OutcomeUnknown,
            }
        }
        _ => TraceState::Incomplete,
    }
}

#[cfg(test)]
pub(super) fn tool_state(state: &ToolExecutionStatus) -> TraceState {
    match state {
        ToolExecutionStatus::Success => TraceState::Completed,
        ToolExecutionStatus::Failed { .. } => TraceState::Failed,
        ToolExecutionStatus::Denied { .. } => TraceState::Denied,
        ToolExecutionStatus::Cancelled { .. } => TraceState::Cancelled,
        ToolExecutionStatus::TimedOut => TraceState::TimedOut,
        ToolExecutionStatus::OutcomeUnknown { .. } => TraceState::OutcomeUnknown,
    }
}

/// Enforces the per-record identity and byte bounds.
///
/// An oversized native identity is omitted whole rather than shortened,
/// because a shortened identity is a different identity that refers to
/// nothing. The record keeps its own stable Trace id either way.
pub(super) fn bound_record(record: &mut TraceRecord) {
    if record
        .location
        .attempt_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id.as_str()))
    {
        record.location.attempt_id = None;
        record.truncated = true;
    }
    if record
        .location
        .step_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id.as_str()))
    {
        record.location.step_id = None;
        record.truncated = true;
    }
    if record
        .native_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id))
    {
        record.native_id = None;
        record.truncated = true;
    }
    if record
        .message_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id.as_str()))
    {
        record.message_id = None;
        record.truncated = true;
    }
    if record
        .activation_origin
        .as_ref()
        .and_then(crate::runtime::subagent::AgentActivationOrigin::tool_call_id)
        .is_some_and(|id| !identity_fits(id.as_str()))
    {
        record.activation_origin = None;
        record.truncated = true;
    }
    if record
        .originating_tool_call_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id.as_str()))
    {
        record.originating_tool_call_id = None;
        record.truncated = true;
    }
    let identities = record.attachments.len() + record.calls.len();
    record
        .attachments
        .retain(|reference| identity_fits(reference.artifact_id.as_str()));
    record.calls.retain(|call| {
        call.call_id
            .as_ref()
            .is_none_or(|id| identity_fits(id.as_str()))
            && call
                .tool_id
                .as_ref()
                .is_none_or(|id| identity_fits(id.as_str()))
    });
    record.truncated |= identities != record.attachments.len() + record.calls.len();
    if encoded_len(record) <= TRACE_RECORD_BYTES {
        return;
    }
    record.preview = None;
    while let Some(call) = record.calls.pop() {
        let domain = format!("messages.{}.tools", call.occurrence.locator.key());
        if let Some(frontier) = record
            .collections
            .iter_mut()
            .find(|frontier| frontier.domain == domain)
        {
            frontier.loaded = record.calls.len() as u64;
            frontier.next_offset = Some(u64::from(call.occurrence.block_index.get()));
        }
    }
    record.truncated = true;
    // Context presentation is the largest optional block a request row can
    // carry. Releasing it keeps the request's own identity, classification
    // and outcome visible rather than losing the whole summary for it.
    if encoded_len(record) > TRACE_RECORD_BYTES
        && let Some(request) = record.request.as_mut()
    {
        request.context_additions.clear();
        request.context_truncated = true;
        request.context_next_offset = (request.context_total > 0).then_some(0);
        request.system_prompt.preview = None;
    }
}
