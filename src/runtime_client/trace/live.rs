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
    request_id: Option<&'a crate::runtime::identity::RequestId>,
    state: &'a mut TraceState,
    native_id: Option<&'a str>,
    attempt_id: Option<&'a AttemptId>,
    /// The provisional Assistant identity this request itself froze.
    assistant_message_id: Option<&'a MessageId>,
    tool_occurrence: Option<&'a crate::runtime_client::snapshot::RuntimeClientToolOccurrence>,
    tool_call: Option<(&'a ToolCallId, &'a ToolId)>,
}

impl TraceRecord {
    fn live_target(&mut self) -> LiveTarget<'_> {
        LiveTarget {
            kind: self.kind,
            request_id: self
                .request
                .as_ref()
                .and_then(|request| request.request_id.as_ref()),
            state: &mut self.state,
            native_id: self.native_id.as_deref(),
            attempt_id: self.location.attempt_id.as_ref(),
            assistant_message_id: self
                .request
                .as_ref()
                .and_then(|request| request.assistant_message_id.as_ref()),
            tool_occurrence: self.tool.as_ref().and_then(|tool| tool.occurrence.as_ref()),
            tool_call: self
                .tool
                .as_ref()
                .and_then(|tool| Some((tool.call_id.as_ref()?, tool.tool_id.as_ref()?))),
        }
    }
}

impl AnchorFacts {
    fn live_target(&mut self) -> LiveTarget<'_> {
        LiveTarget {
            kind: self.kind,
            request_id: self
                .request
                .as_ref()
                .and_then(|request| request.request_id.as_ref()),
            state: &mut self.state,
            native_id: self.native_id.as_deref(),
            attempt_id: self.location.attempt_id.as_ref(),
            assistant_message_id: self
                .request
                .as_ref()
                .and_then(|request| request.assistant_message_id.as_ref()),
            tool_occurrence: self.tool.as_ref().and_then(|tool| tool.occurrence.as_ref()),
            tool_call: self
                .tool
                .as_ref()
                .and_then(|tool| Some((tool.call_id.as_ref()?, tool.tool_id.as_ref()?))),
        }
    }
}

/// The fields lifecycle repair borrows from either an owned public snapshot or
/// an internal shared cut. Repair never materializes another whole snapshot.
#[derive(Clone, Copy)]
pub(crate) struct LiveSnapshot<'a> {
    conversation_id: &'a crate::runtime::identity::ConversationId,
    jobs: &'a [crate::runtime_client::snapshot::RuntimeClientJob],
    agents: &'a [crate::runtime_client::snapshot::RuntimeClientAgent],
    workflows: &'a crate::runtime::workflow::read_model::WorkflowSnapshot,
    pending_interactions: &'a [crate::runtime_client::snapshot::RuntimeClientInteractionHeader],
    attempt: &'a Option<crate::runtime_client::snapshot::RuntimeClientAttempt>,
}
macro_rules! live_snapshot {
    ($snapshot:ty) => {
        impl<'a> From<&'a $snapshot> for LiveSnapshot<'a> {
            fn from(snapshot: &'a $snapshot) -> Self {
                Self {
                    conversation_id: &snapshot.conversation_id,
                    jobs: &snapshot.jobs,
                    agents: &snapshot.agents,
                    workflows: &snapshot.workflows,
                    pending_interactions: &snapshot.pending_interactions,
                    attempt: &snapshot.attempt,
                }
            }
        }
    };
}
live_snapshot!(crate::runtime_client::snapshot::RuntimeClientSnapshot);
live_snapshot!(crate::runtime_client::projection::shared_snapshot::SharedSnapshot);

/// Applies current lifecycle evidence to the snapshot's own Trace window.
#[cfg(test)]
pub(crate) fn repair_live(snapshot: &mut crate::runtime_client::snapshot::RuntimeClientSnapshot) {
    let mut page = std::mem::take(&mut snapshot.trace);
    repair_records(&mut page.records, &*snapshot);
    snapshot.trace = page;
}

/// Applies current lifecycle evidence to already projected summary records.
pub(crate) fn repair_records<'a>(
    records: &mut [TraceRecord],
    snapshot: impl Into<LiveSnapshot<'a>>,
) {
    repair(
        records.iter_mut().map(TraceRecord::live_target),
        snapshot.into(),
    );
}

/// Applies current lifecycle evidence to one refreshed record's own facts.
pub(super) fn repair_anchor(
    facts: &mut AnchorFacts,
    snapshot: &crate::runtime_client::snapshot::RuntimeClientSnapshot,
) {
    repair(std::iter::once(facts.live_target()), snapshot.into());
}

fn repair<'a>(targets: impl IntoIterator<Item = LiveTarget<'a>>, snapshot: LiveSnapshot<'_>) {
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
                    &current.conversation_id == snapshot.conversation_id
                        && record.native_id == Some(current.request_id.as_str())
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
            || record
                .attempt_id
                .is_none_or(|id| attempt.attempt_id.as_ref() != Some(id))
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
            TraceKind::Tool
                if attempt.foreground.iter().any(|current| {
                    let same = record.tool_occurrence.map_or_else(
                        || {
                            record.tool_call.is_some_and(|(call_id, tool_id)| {
                                current.call_id.as_ref() == Some(call_id)
                                    && current.tool_id.as_ref() == Some(tool_id)
                            })
                        },
                        |occurrence| &current.occurrence == occurrence,
                    );
                    same && matches!(current.state, ForegroundToolState::Running { .. })
                }) =>
            {
                *record.state = TraceState::Running;
            }
            _ => {}
        }
    }
}

/// A child Trace uses only child activity identities, never the parent's snapshot.
pub(crate) fn repair_agent_records(
    records: &mut [TraceRecord],
    agent: &crate::runtime_client::snapshot::RuntimeClientAgent,
) {
    repair_agent(records.iter_mut().map(TraceRecord::live_target), agent);
}
pub(super) fn repair_agent_anchor(
    facts: &mut AnchorFacts,
    agent: &crate::runtime_client::snapshot::RuntimeClientAgent,
) {
    repair_agent(std::iter::once(facts.live_target()), agent);
}
fn repair_agent<'a>(
    targets: impl IntoIterator<Item = LiveTarget<'a>>,
    agent: &crate::runtime_client::snapshot::RuntimeClientAgent,
) {
    use crate::runtime::subagent::{AgentState, SubagentActivity};
    if !matches!(agent.state, AgentState::Active | AgentState::Stopping) {
        return;
    }
    for record in targets {
        if *record.state != TraceState::Incomplete
            || record.attempt_id != agent.observation.attempt_id.as_ref()
            || record.attempt_id.is_none()
        {
            continue;
        }
        let live = match (record.kind, &agent.observation.activity) {
            (TraceKind::Attempt, _) => Some(if agent.state == AgentState::Stopping {
                TraceState::Cancelling
            } else {
                TraceState::Running
            }),
            (TraceKind::Request, SubagentActivity::Model { request_id, .. })
                if record.request_id == Some(request_id) =>
            {
                Some(TraceState::Running)
            }
            (
                TraceKind::Tool,
                SubagentActivity::ToolGroup {
                    representative: Some(representative),
                    ..
                },
            ) if record.tool_call
                == Some((&representative.tool_call_id, &representative.tool_id)) =>
            {
                Some(TraceState::Running)
            }
            (TraceKind::Compaction, SubagentActivity::Compacting) => Some(TraceState::Running),
            _ => None,
        };
        if let Some(state) = live {
            *record.state = state;
        }
    }
}
