//! Positive current-lifecycle evidence for records with no durable terminal.
//!
//! Only lifecycle *labels* cross this boundary. No registry payload,
//! workspace path, executor environment or guessed timing does, and no
//! duration is ever synthesized from current time: a record that is running
//! right now still has one endpoint, so it still has no duration.
//!
//! Durable terminal facts always win. This pass only refines `Incomplete`,
//! which is the state a durable start with no durable terminal truthfully
//! has, and only when a current runtime projection positively names that
//! exact identity.

use super::types::{TraceKind, TraceRecord, TraceState};
use crate::runtime_client::snapshot::{ForegroundToolState, RuntimeClientAttemptPhase};

/// Applies current lifecycle evidence to the snapshot's own Trace window.
pub(crate) fn repair_live(snapshot: &mut crate::runtime_client::snapshot::RuntimeClientSnapshot) {
    let mut page = std::mem::take(&mut snapshot.trace);
    repair_records(&mut page.records, snapshot);
    snapshot.trace = page;
}

/// Applies current lifecycle evidence to already projected records.
pub(crate) fn repair_records(
    records: &mut [TraceRecord],
    snapshot: &crate::runtime_client::snapshot::RuntimeClientSnapshot,
) {
    use crate::runtime::subagent::SubagentState as S;
    use crate::runtime::workflow::read_model::WorkflowState as W;
    use crate::tools::background::BackgroundLifecycle as B;
    for record in records {
        if record.state != TraceState::Incomplete {
            continue;
        }
        let live = match record.kind {
            TraceKind::Background => snapshot
                .background
                .iter()
                .find(|current| record.native_id.as_deref() == Some(current.execution_id.as_str()))
                .and_then(|current| match current.state {
                    B::Starting => Some(TraceState::Pending),
                    B::Running => Some(TraceState::Running),
                    B::Cancelling => Some(TraceState::Cancelling),
                    B::PublishingTerminal => Some(TraceState::Settling),
                    _ => None,
                }),
            TraceKind::Subagent => snapshot
                .subagents
                .iter()
                .find(|current| record.native_id.as_deref() == Some(current.subagent_id.as_str()))
                .and_then(|current| match current.state {
                    S::Running => Some(TraceState::Running),
                    S::Cancelling => Some(TraceState::Cancelling),
                    S::PublishingTerminal => Some(TraceState::Settling),
                    _ => None,
                }),
            TraceKind::Workflow => snapshot
                .workflows
                .runs
                .iter()
                .find(|current| {
                    serde_json::to_string(&current.id).ok().as_ref() == record.native_id.as_ref()
                })
                .and_then(|current| match current.state {
                    W::Pending => Some(TraceState::Pending),
                    W::Running => Some(TraceState::Running),
                    W::Waiting { .. } => Some(TraceState::Waiting),
                    W::Draining => Some(TraceState::Settling),
                    W::Settled { .. } => None,
                }),
            TraceKind::Interaction => snapshot
                .pending_interactions
                .iter()
                .any(|current| {
                    current.request.conversation_id == snapshot.conversation_id
                        && record.native_id.as_deref() == Some(current.request.id.as_str())
                })
                .then_some(TraceState::Waiting),
            _ => None,
        };
        if let Some(state) = live {
            record.state = state;
            continue;
        }
        let Some(attempt) = &snapshot.attempt else {
            continue;
        };
        if !matches!(attempt.phase, RuntimeClientAttemptPhase::Running)
            || record.location.attempt_id.as_ref() != Some(&attempt.attempt_id)
        {
            continue;
        }
        match record.kind {
            TraceKind::Attempt => record.state = TraceState::Running,
            TraceKind::Request => {
                // The in-flight generation is named by the provisional
                // Assistant identity the request itself froze, so a retry of
                // the same step cannot claim an earlier request's row.
                if let (Some(request), Some(in_flight)) = (&record.request, &attempt.in_flight)
                    && request.assistant_message_id == in_flight.message_id
                {
                    record.state = TraceState::Running;
                }
            }
            TraceKind::Tool => {
                if let Some(tool) = &record.tool
                    && attempt.foreground.iter().any(|current| {
                        current.call_id == tool.call_id
                            && current.tool_id == tool.tool_id
                            && matches!(current.state, ForegroundToolState::Running { .. })
                    })
                {
                    record.state = TraceState::Running;
                }
            }
            _ => {}
        }
    }
}
