//! Bounded summary-record projection: one anchor, one pageable row.
//!
//! A summary row is the *immutable historical presentation* of one record.
//! It starts from the facts the anchor owns by itself ([`super::anchor`])
//! and adds the two things only a reader of history needs:
//!
//! ```text
//! bounded preview / detail summary   content this row stands for
//! immutable relationships           System Prompt state, Context, Tool name
//! ```
//!
//! Those additions are historical joins. They are performed here, when a
//! page is built, and nowhere else: a lifecycle refresh projects
//! [`super::TraceLifecycle`], which carries none of them, so it must not pay
//! for them or fail because of them.
//!
//! A record never borrows a fact from a neighbouring anchor: a Tool's
//! outcome comes from that Tool's own terminal, a request's usage from that
//! request's own terminal fact, and a step's end from that step's own
//! completion.

use super::anchor::AnchorFacts;
use super::bounds::{TRACE_RECORD_BYTES, TracePreview, encoded_len, identity_fits};
use super::content::message_preview;
use super::types::{
    TraceGeneration, TraceGenerationTimeline, TraceRecord, TraceRequestSummary, TraceState,
};
use super::{ADOPTED_MESSAGE_LIMIT, TraceProjection};
use crate::durable::ConversationStoreError;
use crate::durable::presentation::FactScope;
use crate::events::types::{RuntimeEvent as E, RuntimeEventEnvelope};
use crate::model::generation_evidence::GenerationEvidence;
use crate::model::types::ModelUsage;
use crate::tools::types::ToolExecutionStatus;

impl TraceProjection<'_> {
    /// Projects one anchor into its bounded summary record.
    ///
    /// # Errors
    ///
    /// Propagates the failed durable read of any authority this row joins.
    pub(super) fn record(
        &self,
        anchor: &RuntimeEventEnvelope,
    ) -> Result<TraceRecord, ConversationStoreError> {
        let facts = self.anchor_facts(anchor)?;
        let AnchorFacts {
            id,
            position,
            location,
            kind,
            state,
            timing,
            mut preview,
            calls,
            native_id,
            agent_id,
            activation_id,
            originating_tool_call_id,
            message_id,
            attachments,
            mut tool,
            request,
            has_detail,
            truncated,
        } = facts;
        // Everything below is presentation-only: the previews that need a
        // Ledger read of their own, the recorded Tool name, and the two
        // request-relative relationships. None of it reaches a lifecycle
        // update, and a lifecycle refresh performs none of these reads.
        //
        // The request anchor is the one that froze a snapshot, so the
        // resolved facts drive this rather than a second match on the event.
        // The immutable snapshot is also this request's identity authority.
        let summary = match request {
            Some(request) => {
                let frozen = &request.frozen;
                // Both relationships below are resolved here, from native
                // authority, so no client has to compare request details or
                // diff messages to discover them.
                let previous = self.previous_request(anchor.sequence)?;
                let (system_prompt, tool_catalog) = previous.presentation(frozen);
                let (context_additions, context_truncated) = self.context_presentation(frozen)?;
                Some(TraceRequestSummary {
                    previous_failure_kind: self.previous_request_failure(frozen)?,
                    assistant_message_id: frozen.provisional_message_id.clone(),
                    request_id: frozen.request_id.clone(),
                    retry_number: frozen.identity.retry_number,
                    model: frozen.invocation.model.clone(),
                    failure_kind: request.failure_kind,
                    usage: request.usage,
                    generation: request.generation,
                    system_prompt,
                    predecessor: previous.identity(),
                    tool_catalog,
                    context_additions,
                    context_truncated,
                })
            }
            None => None,
        };
        match &anchor.event {
            E::InboundTurnAdopted { message_ids } => {
                for message in self
                    .store
                    .load_messages(&message_ids[..message_ids.len().min(ADOPTED_MESSAGE_LIMIT)])?
                {
                    if preview.is_none() {
                        preview = message_preview(&message);
                    }
                }
            }
            E::CompactionStarted => {
                if let Some(id) = message_id.as_ref() {
                    for message in self.store.load_messages(std::slice::from_ref(id))? {
                        preview = message_preview(&message);
                    }
                }
            }
            E::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                if let Some(tool) = tool.as_mut() {
                    tool.name
                        .clone_from(&self.tool_call_name(anchor, tool_call_id, tool_id)?);
                    if preview.is_none()
                        && let Some(name) = tool.name.as_deref()
                    {
                        preview = Some(TracePreview::of(name));
                    }
                }
            }
            _ => {}
        }
        Ok(TraceRecord {
            id,
            position,
            location,
            kind,
            state,
            timing,
            preview,
            request: summary,
            tool,
            calls,
            native_id,
            agent_id,
            activation_id,
            originating_tool_call_id,
            message_id,
            attachments,
            has_detail,
            truncated,
        })
    }

    /// The exact preceding actual request's failure class, when recorded.
    ///
    /// The previous request is named by the native ordinal, not by scanning
    /// timestamps: retry evidence is the frozen `retry_number`, and nothing
    /// else is treated as proof that one request followed another.
    fn previous_request_failure(
        &self,
        frozen: &crate::model::snapshot::RequestSnapshot,
    ) -> Result<Option<crate::model::error::ModelErrorKind>, ConversationStoreError> {
        if frozen.identity.retry_number == 0 {
            return Ok(None);
        }
        let mut identity = frozen.identity.clone();
        identity.retry_number -= 1;
        Ok(self
            .ending(
                FactScope::Request(identity.request_id().to_string()),
                &["model_request_failed"],
            )?
            .and_then(|event| match event.event {
                E::ModelRequestFailed { error, .. } => Some(error.kind),
                _ => None,
            }))
    }

    /// The recorded model-facing name of one started Tool call.
    ///
    /// Read from the canonical Assistant proposal in the same logical Step,
    /// matched on both the call identity and the Tool identity, so a reused
    /// provider call id in another step cannot supply a name here.
    pub(super) fn tool_call_name(
        &self,
        anchor: &RuntimeEventEnvelope,
        call_id: &crate::runtime::identity::ToolCallId,
        tool_id: &crate::runtime::identity::ToolId,
    ) -> Result<Option<String>, ConversationStoreError> {
        Ok(self
            .step_tool_call(anchor, call_id, tool_id)?
            .map(|(_, call)| call.name))
    }
}

/// Derives display metrics from settled evidence and this request's usage.
pub(super) fn generation_metrics(
    evidence: GenerationEvidence,
    usage: Option<&ModelUsage>,
) -> TraceGeneration {
    TraceGeneration {
        timeline: generation_timeline(evidence),
        ttft_ms: evidence.time_to_first_output_ms(),
        generation_ms: evidence.generation_ms(),
        terminal_ms: evidence.terminal_ms,
        // Throughput needs both a measured decode span and reported output
        // tokens. Missing usage leaves it unavailable rather than zero.
        output_tokens_per_second: usage
            .and_then(|usage| evidence.throughput_tokens_per_second(usage.output_tokens)),
    }
}

/// Positions require the runtime's bridge; numeric TTFT alone is insufficient.
fn generation_timeline(evidence: GenerationEvidence) -> Option<TraceGenerationTimeline> {
    let dispatch_ms = evidence.dispatch_after_start_ms?;
    let first_output_ms = match evidence.first_output_ms {
        Some(v) => Some(dispatch_ms.checked_add(v)?),
        None => None,
    };
    let last_output_ms = match evidence.last_output_ms {
        Some(v) => Some(dispatch_ms.checked_add(v)?),
        None => None,
    };
    Some(TraceGenerationTimeline {
        dispatch_ms,
        first_output_ms,
        last_output_ms,
        terminal_ms: dispatch_ms.checked_add(evidence.terminal_ms)?,
    })
}

/// The typed state proven by one terminal fact.
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
                S::ReviewInvalidated => TraceState::Interrupted,
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
        .originating_tool_call_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id.as_str()))
    {
        record.originating_tool_call_id = None;
        record.truncated = true;
    }
    if record.request.as_ref().is_some_and(|request| {
        !identity_fits(request.request_id.as_str())
            || !identity_fits(request.assistant_message_id.as_str())
    }) {
        record.request = None;
        record.truncated = true;
    }
    if record.tool.as_ref().is_some_and(|tool| {
        !identity_fits(tool.call_id.as_str()) || !identity_fits(tool.tool_id.as_str())
    }) {
        record.tool = None;
        record.truncated = true;
    }
    let identities = record.attachments.len() + record.calls.len();
    record
        .attachments
        .retain(|reference| identity_fits(reference.artifact_id.as_str()));
    record.calls.retain(|call| {
        identity_fits(call.call_id.as_str()) && identity_fits(call.tool_id.as_str())
    });
    record.truncated |= identities != record.attachments.len() + record.calls.len();
    if encoded_len(record) > TRACE_RECORD_BYTES {
        record.preview = None;
        record.calls.clear();
        record.truncated = true;
    }
    // Context presentation is the largest optional block a request row can
    // carry. Releasing it keeps the request's own identity, classification
    // and outcome visible rather than losing the whole summary for it.
    if encoded_len(record) > TRACE_RECORD_BYTES
        && let Some(request) = record.request.as_mut()
    {
        request.context_additions.clear();
        request.context_truncated = true;
        request.system_prompt.preview = None;
    }
    // JSON escaping can expand even bounded identity and model strings
    // several times over, so the bound is re-checked after each release.
    if encoded_len(record) > TRACE_RECORD_BYTES {
        record.request = None;
        record.tool = None;
        record.attachments.clear();
    }
}
