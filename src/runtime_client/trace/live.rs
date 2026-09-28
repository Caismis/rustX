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
//!
//! There is exactly one state machine here. It reads a narrow borrowed view
//! of the correlation identities it needs, so an already projected
//! [`TraceRecord`] page and a freshly resolved [`AnchorFacts`] are repaired
//! by the same code rather than by two copies that could drift.

use super::anchor::AnchorFacts;
use super::types::{TraceKind, TraceRecord, TraceState};
use crate::runtime::identity::{AttemptId, MessageId, ToolCallId, ToolId};
use crate::runtime_client::snapshot::{ForegroundToolState, RuntimeClientAttemptPhase};

/// The exact facts current lifecycle repair reads, and the one it writes.
///
/// Every correlation below is an exact identity match. Nothing here reads a
/// name, a timestamp, a row position or the loaded window.
pub(super) struct LiveTarget<'a> {
    kind: TraceKind,
    state: &'a mut TraceState,
    native_id: Option<&'a str>,
    attempt_id: Option<&'a AttemptId>,
    /// The provisional Assistant identity this request itself froze.
    assistant_message_id: Option<&'a MessageId>,
    tool_call: Option<(&'a ToolCallId, &'a ToolId)>,
}

impl TraceRecord {
    fn live_target(&mut self) -> LiveTarget<'_> {
        LiveTarget {
            kind: self.kind,
            state: &mut self.state,
            native_id: self.native_id.as_deref(),
            attempt_id: self.location.attempt_id.as_ref(),
            assistant_message_id: self
                .request
                .as_ref()
                .map(|request| &request.assistant_message_id),
            tool_call: self
                .tool
                .as_ref()
                .map(|tool| (&tool.call_id, &tool.tool_id)),
        }
    }
}

impl AnchorFacts {
    fn live_target(&mut self) -> LiveTarget<'_> {
        LiveTarget {
            kind: self.kind,
            state: &mut self.state,
            native_id: self.native_id.as_deref(),
            attempt_id: self.location.attempt_id.as_ref(),
            assistant_message_id: self
                .request
                .as_ref()
                .map(|request| &request.frozen.provisional_message_id),
            tool_call: self
                .tool
                .as_ref()
                .map(|tool| (&tool.call_id, &tool.tool_id)),
        }
    }
}

/// Applies current lifecycle evidence to the snapshot's own Trace window.
pub(crate) fn repair_live(snapshot: &mut crate::runtime_client::snapshot::RuntimeClientSnapshot) {
    let mut page = std::mem::take(&mut snapshot.trace);
    repair_records(&mut page.records, snapshot);
    snapshot.trace = page;
}

/// Applies current lifecycle evidence to already projected summary records.
pub(crate) fn repair_records(
    records: &mut [TraceRecord],
    snapshot: &crate::runtime_client::snapshot::RuntimeClientSnapshot,
) {
    repair(records.iter_mut().map(TraceRecord::live_target), snapshot);
}

/// Applies current lifecycle evidence to one refreshed record's own facts.
pub(super) fn repair_anchor(
    facts: &mut AnchorFacts,
    snapshot: &crate::runtime_client::snapshot::RuntimeClientSnapshot,
) {
    repair(std::iter::once(facts.live_target()), snapshot);
}

fn repair<'a>(
    targets: impl IntoIterator<Item = LiveTarget<'a>>,
    snapshot: &crate::runtime_client::snapshot::RuntimeClientSnapshot,
) {
    use crate::runtime::subagent::SubagentState as S;
    use crate::runtime::workflow::read_model::WorkflowState as W;
    use crate::tools::background::BackgroundLifecycle as B;
    for record in targets {
        if *record.state != TraceState::Incomplete {
            continue;
        }
        let live = match record.kind {
            TraceKind::Background => snapshot
                .jobs
                .iter()
                .find(|current| record.native_id == Some(current.job_id.as_str()))
                .and_then(|current| match current.state {
                    B::Starting => Some(TraceState::Pending),
                    B::Running => Some(TraceState::Running),
                    B::Cancelling => Some(TraceState::Cancelling),
                    B::PublishingTerminal => Some(TraceState::Settling),
                    _ => None,
                }),
            TraceKind::Subagent => snapshot
                .agents
                .iter()
                .find(|current| record.native_id == Some(current.activation_id.as_str()))
                .and_then(|current| match current.activation_state {
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
                    serde_json::to_string(&current.id).ok().as_deref() == record.native_id
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
                        && record.native_id == Some(current.request.id.as_str())
                })
                .then_some(TraceState::Waiting),
            _ => None,
        };
        if let Some(state) = live {
            *record.state = state;
            continue;
        }
        let Some(attempt) = &snapshot.attempt else {
            continue;
        };
        if !matches!(attempt.phase, RuntimeClientAttemptPhase::Running)
            || record.attempt_id != Some(&attempt.attempt_id)
        {
            continue;
        }
        match record.kind {
            TraceKind::Attempt => *record.state = TraceState::Running,
            TraceKind::Request => {
                // The in-flight generation is named by the provisional
                // Assistant identity the request itself froze, so a retry of
                // the same step cannot claim an earlier request's row.
                if let (Some(assistant), Some(in_flight)) =
                    (record.assistant_message_id, &attempt.in_flight)
                    && *assistant == in_flight.message_id
                {
                    *record.state = TraceState::Running;
                }
            }
            TraceKind::Tool => {
                if let Some((call_id, tool_id)) = record.tool_call
                    && attempt.foreground.iter().any(|current| {
                        current.call_id == *call_id
                            && current.tool_id == *tool_id
                            && matches!(current.state, ForegroundToolState::Running { .. })
                    })
                {
                    *record.state = TraceState::Running;
                }
            }
            _ => {}
        }
    }
}
