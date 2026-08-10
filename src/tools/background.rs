//! The conversation-owned background execution registry.
//!
//! One conversation owns one authoritative background registry. It exists
//! outside any `AgentExecution`: an attempt receives/clones a handle to it,
//! but detached task handles and records are never owned by a single
//! attempt. The registry is the authoritative state machine of every
//! background execution — never messages, never Agent Status text, never
//! Event Journal text — and it makes cross-conversation access structurally
//! impossible: there is no process-global execution lookup table, and every
//! operation is scoped to the registry's conversation.
//!
//! # Dispatch ownership commit
//!
//! Background dispatch is two-stage. [`ConversationBackgroundRegistry::prepare_dispatch`]
//! validates the invocation, allocates the deterministic `exec_N`
//! [`ToolExecutionId`], creates a private prepared record with its own
//! cancellation resources, and spawns the runner behind a start/commit gate
//! (the runner cannot begin before the gate is released). [`ConversationBackgroundRegistry::commit_dispatch`]
//! is the one deterministic linearization point of background ownership:
//! the registry synchronization boundary is acquired first, the final
//! attempt-cancellation observation happens at that same protected boundary,
//! and only then does the commit happen:
//!
//! - attempt cancellation observable at the boundary: the prepared dispatch
//!   rolls back completely under the same boundary — no published record,
//!   no accepted result, the runner is aborted and never begins;
//! - conversation ownership wins: the record is published as `Starting`,
//!   ownership transfers exactly once, the accepted result is produced, and
//!   a later attempt cancellation can never reclaim the detached execution.
//!
//! There is no unchecked window between the deciding cancellation
//! observation and the prepared→owned registry transition.
//!
//! # Cancellation-vs-completion race
//!
//! The first registry transition that commits either terminal completion or
//! cancellation intent wins the race. If completion
//! (`Succeeded`/`Failed`/`Cancelled`) commits first, a later cancel is an
//! idempotent no-op returning the terminal snapshot. If cancellation
//! intent commits first (`Starting`/`Running` → `Cancelling`), cancellation
//! owns settlement: the cancellation reason is retained for final
//! settlement, and a later normal executor return cannot overwrite the
//! cancellation winner with `Succeeded` — the stored terminal result is
//! canonicalized to `Cancelled` with the retained reason. Only an explicit
//! runtime/process-control failure after cancellation intent settles as
//! `Failed`.
//!
//! # Terminal inbound publication
//!
//! Every successfully dispatched background execution reaches exactly one
//! terminal registry state, and a terminal transition claims at most one
//! runtime inbound notification: a timestamped
//! [`UserMessageBlock`] with
//! [`UserSource::Runtime`] through the owning
//! [`ConversationInboundMailbox`]. Registry notification state prevents
//! duplicate publication. On publication failure the
//! authoritative terminal registry state is retained and the failure is
//! reported without rolling the execution back.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::events::{RuntimeEvent, RuntimeEventSink};
use crate::message::content::TextBlock;
use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
use crate::runtime::RuntimeClock;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolExecutionId, ToolId};
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::types::CancellationReason;
use serde::{Deserialize, Serialize};

use crate::tools::artifacts::ArtifactStore;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
use crate::tools::limits::bound_tool_progress;
use crate::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolProgress,
    ToolResultContent,
};
use crate::tools::workspace::Workspace;

/// The one cancellation reason of conversation-owned background cancellation.
///
/// Background cancellation is only ever requested through the conversation
/// control path (`background_task(action = cancel)` or direct registry
/// cancellation), which is a user-requested control action. The registry
/// retains this reason when cancellation intent commits so the canonicalized
/// terminal result always agrees with the registry winner.
const BACKGROUND_CANCEL_REASON: CancellationReason = CancellationReason::UserRequested;

/// The public lifecycle of one background execution.
///
/// Terminal states are absorbing. The allowed public transitions are:
///
/// ```text
/// Starting  → Running
/// Starting  → Cancelling
/// Starting  → Succeeded / Failed
/// Running   → Cancelling
/// Running   → Succeeded / Failed
/// Cancelling → Cancelled
/// ```
///
/// An internal unpublished prepared state implements dispatch atomicity but
/// never leaks as an accepted execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundLifecycle {
    /// The dispatch committed and the runner is starting.
    Starting,
    /// The runner is executing.
    Running,
    /// Cancellation intent committed and owns settlement.
    Cancelling,
    /// The execution succeeded.
    Succeeded,
    /// The execution failed.
    Failed,
    /// The execution was cancelled through the cancellation path.
    Cancelled,
}

impl BackgroundLifecycle {
    /// Whether this state is terminal (absorbing).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// Whether this state is active (non-terminal).
    #[must_use]
    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }

    /// The stable serialized name of the state.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The one canonical read-only snapshot of one background execution.
///
/// The snapshot is reused by registry queries, `background_task(status)`,
/// `background_task(cancel)`, Agent Status projection input, and
/// deterministic tests. It never exposes internal task handles or process
/// ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundExecutionSnapshot {
    /// The detached runtime execution identity.
    pub execution_id: ToolExecutionId,
    /// The canonical tool identity.
    pub tool_id: ToolId,
    /// The model-facing tool name.
    pub tool_name: String,
    /// The authoritative lifecycle state.
    pub state: BackgroundLifecycle,
    /// The latest bounded progress snapshot, when any was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ToolProgress>,
    /// The bounded terminal result, when terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolExecutionResult>,
}

/// The outcome of a committed background dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundDispatchOutcome {
    /// Conversation ownership committed: the execution is detached and the
    /// accepted attempt-facing result is produced. The accepted result is
    /// the result of the model-issued tool call, not the final result of the
    /// detached execution.
    Accepted {
        /// The allocated runtime execution identity.
        execution_id: ToolExecutionId,
        /// The accepted result: bounded structured content identifying
        /// `execution_id`, `state`, and the tool.
        result: ToolExecutionResult,
    },
    /// Attempt cancellation won before the ownership commit: the prepared
    /// dispatch was rolled back, no execution is detached, and no accepted
    /// result exists.
    RolledBack,
}

/// A background dispatch failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundDispatchError {
    /// The invocation is not a background invocation.
    NotBackgroundInvocation,
    /// The execution sequence space is exhausted.
    SequenceExhausted,
    /// An internal dispatch failure.
    Internal(String),
}

impl core::fmt::Display for BackgroundDispatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotBackgroundInvocation => write!(
                f,
                "only background invocations can be dispatched to the background registry"
            ),
            Self::SequenceExhausted => write!(f, "the execution sequence space is exhausted"),
            Self::Internal(message) => write!(f, "background dispatch failed: {message}"),
        }
    }
}

impl std::error::Error for BackgroundDispatchError {}

/// A prepared but not yet committed background dispatch.
///
/// Between [`ConversationBackgroundRegistry::prepare_dispatch`] and
/// [`ConversationBackgroundRegistry::commit_dispatch`] the runner is parked
/// behind its start gate and the dispatch is private. Dropping the prepared
/// dispatch without committing rolls it back: the runner is aborted and no
/// detached execution exists.
#[derive(Debug)]
pub struct PreparedBackgroundDispatch {
    registry: ConversationBackgroundRegistry,
    execution_id: ToolExecutionId,
    committed: bool,
}

impl Drop for PreparedBackgroundDispatch {
    fn drop(&mut self) {
        if !self.committed {
            self.registry.rollback_prepared(&self.execution_id);
        }
    }
}

/// The execution resources of the conversation background registry.
#[derive(Clone)]
pub struct BackgroundResources {
    /// The owning conversation inbound mailbox for terminal notifications.
    pub mailbox: ConversationInboundMailbox,
    /// The conversation workspace for detached executors.
    pub workspace: Workspace,
    /// The conversation artifact store for detached executors.
    pub artifacts: ArtifactStore,
    /// The explicit authorized tool environment.
    pub environment: ToolEnvironment,
    /// The runtime clock stamping terminal inbound messages.
    pub clock: Arc<dyn RuntimeClock>,
    /// The narrow non-durable execution-fact sink, when attached.
    pub event_sink: Option<Arc<dyn RuntimeEventSink>>,
}

/// The per-execution publication state of the terminal inbound message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationState {
    Pending,
    Publishing,
    Published,
    Failed,
}

/// One published background record.
struct BackgroundRecord {
    execution_id: ToolExecutionId,
    tool_call_id: ToolCallId,
    tool_id: ToolId,
    tool_name: String,
    lifecycle: BackgroundLifecycle,
    cancellation: CancellationSignal,
    /// The retained cancellation reason when cancellation intent committed
    /// (`Cancelling`): the registry keeps it for final settlement, so the
    /// canonicalized terminal result always agrees with the registry
    /// winner.
    cancel_reason: Option<CancellationReason>,
    progress: Option<ToolProgress>,
    result: Option<ToolExecutionResult>,
    notification: NotificationState,
}

/// One prepared (not yet committed) background dispatch.
struct PreparedRecord {
    record: BackgroundRecord,
    gate: Arc<Notify>,
    runner: tokio::task::JoinHandle<()>,
}

/// The synchronized registry state.
struct BackgroundRegistryState {
    next_execution_sequence: u64,
    prepared: HashMap<ToolExecutionId, PreparedRecord>,
    records: Vec<BackgroundRecord>,
    index: HashMap<ToolExecutionId, usize>,
    /// Test-only synchronization hook at the dispatch ownership commit
    /// boundary; never present outside `#[cfg(test)]`.
    #[cfg(test)]
    commit_hook: Option<Arc<test_sync::CommitBoundaryHook>>,
}

/// The conversation-owned authoritative background registry.
///
/// The registry is cheaply cloneable; all clones share one synchronized
/// state machine. Dispatch, settlement, cancellation, and queries all pass
/// through the same synchronization boundary, so no timing assumption is
/// ever made.
pub struct ConversationBackgroundRegistry {
    conversation_id: ConversationId,
    inner: Arc<Mutex<BackgroundRegistryState>>,
    resources: BackgroundResources,
}

impl Clone for ConversationBackgroundRegistry {
    fn clone(&self) -> Self {
        Self {
            conversation_id: self.conversation_id.clone(),
            inner: self.inner.clone(),
            resources: self.resources.clone(),
        }
    }
}

impl core::fmt::Debug for ConversationBackgroundRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConversationBackgroundRegistry")
            .field("conversation_id", &self.conversation_id)
            .finish_non_exhaustive()
    }
}

impl ConversationBackgroundRegistry {
    /// Creates the background registry of one conversation.
    #[must_use]
    pub fn new(conversation_id: ConversationId, resources: BackgroundResources) -> Self {
        Self {
            conversation_id,
            inner: Arc::new(Mutex::new(BackgroundRegistryState {
                next_execution_sequence: 0,
                prepared: HashMap::new(),
                records: Vec::new(),
                index: HashMap::new(),
                #[cfg(test)]
                commit_hook: None,
            })),
            resources,
        }
    }

    /// Installs the test-only synchronization hook at the dispatch
    /// ownership commit boundary. Only available under `#[cfg(test)]`;
    /// never used by production code.
    #[cfg(test)]
    pub(crate) fn install_commit_boundary_hook(&self, hook: Arc<test_sync::CommitBoundaryHook>) {
        let mut state = self.state();
        state.commit_hook = Some(hook);
    }

    /// The conversation this registry belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The conversation resources shared with detached runners.
    #[must_use]
    pub fn resources(&self) -> &BackgroundResources {
        &self.resources
    }

    /// Stage one: prepares a background dispatch.
    ///
    /// Validates that the invocation is a background invocation, allocates
    /// the next deterministic `exec_N` execution id under the registry
    /// synchronization boundary, creates the private prepared record with
    /// its own cancellation signal, and spawns the runner behind the
    /// start/commit gate. The runner cannot begin before the gate is
    /// released by [`ConversationBackgroundRegistry::commit_dispatch`].
    ///
    /// # Errors
    ///
    /// Returns [`BackgroundDispatchError::NotBackgroundInvocation`] for a
    /// foreground invocation and
    /// [`BackgroundDispatchError::SequenceExhausted`] when the sequence
    /// space is exhausted.
    pub fn prepare_dispatch(
        &self,
        invocation: &ToolInvocation,
        executor: &Arc<dyn ToolExecutor>,
    ) -> Result<PreparedBackgroundDispatch, BackgroundDispatchError> {
        if invocation.mode != ToolInvocationMode::Background {
            return Err(BackgroundDispatchError::NotBackgroundInvocation);
        }
        let mut state = self.state();
        let next = state
            .next_execution_sequence
            .checked_add(1)
            .ok_or(BackgroundDispatchError::SequenceExhausted)?;
        state.next_execution_sequence = next;
        let execution_id = ToolExecutionId::new(format!("exec_{next}"));
        let cancellation = CancellationSignal::new();
        let gate = Arc::new(Notify::new());
        let runner = self.spawn_runner(
            execution_id.clone(),
            invocation.clone(),
            executor.clone(),
            cancellation.clone(),
            gate.clone(),
        );
        let prepared = PreparedRecord {
            record: BackgroundRecord {
                execution_id: execution_id.clone(),
                tool_call_id: invocation.call_id.clone(),
                tool_id: invocation.tool_id.clone(),
                tool_name: invocation.tool_name.clone(),
                lifecycle: BackgroundLifecycle::Starting,
                cancellation,
                cancel_reason: None,
                progress: None,
                result: None,
                notification: NotificationState::Pending,
            },
            gate,
            runner,
        };
        state.prepared.insert(execution_id.clone(), prepared);
        Ok(PreparedBackgroundDispatch {
            registry: self.clone(),
            execution_id,
            committed: false,
        })
    }

    /// Stage two: commits the prepared dispatch (the linearization point).
    ///
    /// The registry synchronization boundary is acquired first; the final
    /// attempt-cancellation observation happens at that same protected
    /// boundary, so there is no unchecked window between the deciding
    /// cancellation observation and the prepared→owned transition. If the
    /// attempt cancellation is observable, the prepared dispatch rolls back
    /// completely under the boundary — no published record, no accepted
    /// result, and the runner is aborted and never begins. Otherwise
    /// conversation ownership commits exactly once: the record is published
    /// as `Starting`, the runner gate is released, and the accepted
    /// attempt-facing result is produced. No await or cancellation
    /// checkpoint can split the ownership commit from the accepted result.
    #[must_use]
    pub fn commit_dispatch(
        &self,
        mut prepared: PreparedBackgroundDispatch,
        attempt_cancellation: &CancellationSignal,
    ) -> BackgroundDispatchOutcome {
        let mut state = self.state();
        // TEST-ONLY ownership-commit boundary: the registry lock is held and
        // the deciding cancellation observation is next. Tests park here to
        // make the linearization exact.
        #[cfg(test)]
        if let Some(hook) = &state.commit_hook {
            hook.enter();
        }
        if attempt_cancellation.is_cancelled() {
            // The deciding cancellation observation and the rollback share
            // this critical section: the prepared record is removed and the
            // runner aborted here, and the prepared handle's drop semantics
            // are neutralized so no second rollback path exists.
            if let Some(prepared_record) = state.prepared.remove(&prepared.execution_id) {
                prepared_record.runner.abort();
            }
            prepared.committed = true;
            return BackgroundDispatchOutcome::RolledBack;
        }
        let Some(prepared_record) = state.prepared.remove(&prepared.execution_id) else {
            prepared.committed = true;
            return BackgroundDispatchOutcome::RolledBack;
        };
        let result = accepted_result(&prepared.execution_id, &prepared_record.record.tool_name);
        let execution_id = prepared.execution_id.clone();
        let next_index = state.records.len();
        state.index.insert(execution_id.clone(), next_index);
        state.records.push(prepared_record.record);
        drop(state);
        prepared.committed = true;
        prepared_record.gate.notify_one();
        BackgroundDispatchOutcome::Accepted {
            execution_id,
            result,
        }
    }

    /// Requests cancellation of one execution and returns the canonical
    /// snapshot after processing the request.
    ///
    /// Cancellation is idempotent: for an already-cancelling or terminal
    /// execution the current snapshot is returned unchanged and the state is
    /// never destructively changed. An unknown execution id returns `None`.
    ///
    /// When cancellation intent commits, the cancellation reason is
    /// retained in the record; the registry is the settlement authority and
    /// uses it to canonicalize the final terminal result, so the registry
    /// winner and the stored result can never disagree.
    #[must_use]
    pub fn cancel(&self, execution_id: &ToolExecutionId) -> Option<BackgroundExecutionSnapshot> {
        let mut state = self.state();
        let index = *state.index.get(execution_id)?;
        let record = &mut state.records[index];
        match record.lifecycle {
            BackgroundLifecycle::Starting | BackgroundLifecycle::Running => {
                record.lifecycle = BackgroundLifecycle::Cancelling;
                record.cancel_reason = Some(BACKGROUND_CANCEL_REASON);
                record.cancellation.cancel();
            }
            BackgroundLifecycle::Cancelling
            | BackgroundLifecycle::Succeeded
            | BackgroundLifecycle::Failed
            | BackgroundLifecycle::Cancelled => {}
        }
        let snapshot = snapshot_of(record);
        Some(snapshot)
    }

    /// The canonical snapshot of one execution, active or terminal.
    #[must_use]
    pub fn snapshot(&self, execution_id: &ToolExecutionId) -> Option<BackgroundExecutionSnapshot> {
        let state = self.state();
        let index = *state.index.get(execution_id)?;
        Some(snapshot_of(&state.records[index]))
    }

    /// The active (Starting/Running/Cancelling) snapshots in execution
    /// allocation order. Terminal executions never appear here.
    #[must_use]
    pub fn active_snapshot(&self) -> Vec<BackgroundExecutionSnapshot> {
        let state = self.state();
        state
            .records
            .iter()
            .filter(|record| record.lifecycle.is_active())
            .map(snapshot_of)
            .collect()
    }

    /// All snapshots (active and terminal) in execution allocation order.
    ///
    /// Terminal records remain queryable for the conversation lifetime.
    #[must_use]
    pub fn all_snapshots(&self) -> Vec<BackgroundExecutionSnapshot> {
        let state = self.state();
        state.records.iter().map(snapshot_of).collect()
    }

    /// The runner-owned settlement boundary of one execution.
    ///
    /// The first registry transition that commits either terminal completion
    /// or cancellation intent wins the race (see the module documentation).
    /// A terminal transition may claim at most one runtime inbound
    /// publication; duplicate settlement calls are idempotent no-ops.
    ///
    /// When cancellation intent already owns settlement (`Cancelling`), a
    /// later normal executor return cannot contradict the registry winner:
    /// the stored terminal result is canonicalized to `Cancelled` with the
    /// retained cancellation reason, preserving useful bounded result data
    /// and artifacts where present. Only an explicit runtime/process-control
    /// failure after cancellation intent settles as `Failed`.
    pub fn finish(&self, execution_id: &ToolExecutionId, result: &ToolExecutionResult) {
        let mut state = self.state();
        let Some(index) = state.index.get(execution_id).copied() else {
            return;
        };
        let record = &mut state.records[index];
        if record.lifecycle.is_terminal() {
            return;
        }
        let (settled, stored) = match record.lifecycle {
            BackgroundLifecycle::Starting | BackgroundLifecycle::Running => match result.status {
                ToolExecutionStatus::Success => (BackgroundLifecycle::Succeeded, result.clone()),
                ToolExecutionStatus::Cancelled { .. } => {
                    (BackgroundLifecycle::Cancelled, result.clone())
                }
                ToolExecutionStatus::Failed { .. }
                | ToolExecutionStatus::TimedOut
                | ToolExecutionStatus::Interrupted => (BackgroundLifecycle::Failed, result.clone()),
            },
            BackgroundLifecycle::Cancelling => {
                // Cancellation intent already owns settlement. A normal
                // executor return must not overwrite the cancellation
                // winner; only an explicit runtime/process-control failure
                // is represented as Failed.
                if matches!(result.status, ToolExecutionStatus::Failed { .. }) {
                    (BackgroundLifecycle::Failed, result.clone())
                } else {
                    let mut canonical = result.clone();
                    canonical.status = ToolExecutionStatus::Cancelled {
                        reason: record.cancel_reason.unwrap_or(BACKGROUND_CANCEL_REASON),
                    };
                    (BackgroundLifecycle::Cancelled, canonical)
                }
            }
            BackgroundLifecycle::Succeeded
            | BackgroundLifecycle::Failed
            | BackgroundLifecycle::Cancelled => return,
        };
        record.lifecycle = settled;
        record.result = Some(stored.clone());
        if record.notification != NotificationState::Pending {
            return;
        }
        record.notification = NotificationState::Publishing;
        let message = terminal_inbound_message(
            execution_id,
            &record.tool_name,
            settled,
            &stored.artifacts,
            self.resources.clock.now(),
        );
        match self.resources.mailbox.enqueue(message) {
            Ok(_) => {
                record.notification = NotificationState::Published;
            }
            Err(_error) => {
                // The authoritative terminal registry state is retained;
                // the execution is never rolled back to active. The
                // notification failure is recorded and reported.
                record.notification = NotificationState::Failed;
            }
        }
    }

    /// Updates the latest bounded progress snapshot of one execution and
    /// emits the corresponding canonical execution fact through the narrow
    /// event seam, when one is attached.
    ///
    /// Every progress notification is normalized through the one shared
    /// UTF-8-safe bound (`bound_tool_progress`), the same normalization the
    /// foreground path uses. Only the current/latest bounded progress
    /// snapshot is retained; no unbounded progress history exists in the
    /// registry. Progress of a terminal execution is ignored.
    pub fn report_progress(&self, execution_id: &ToolExecutionId, progress: ToolProgress) {
        let bounded = bound_tool_progress(progress);
        let mut state = self.state();
        let Some(index) = state.index.get(execution_id).copied() else {
            return;
        };
        let record = &mut state.records[index];
        if !record.lifecycle.is_active() {
            return;
        }
        record.progress = Some(bounded.clone());
        let event = RuntimeEvent::ToolExecutionProgress {
            tool_call_id: record.tool_call_id.clone(),
            tool_id: record.tool_id.clone(),
            execution_id: Some(execution_id.clone()),
            progress: bounded,
        };
        drop(state);
        if let Some(sink) = &self.resources.event_sink {
            sink.emit(event);
        }
    }

    /// The synchronized registry state.
    ///
    /// # Panics
    ///
    /// Panics only if the registry lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    fn state(&self) -> std::sync::MutexGuard<'_, BackgroundRegistryState> {
        self.inner
            .lock()
            .expect("background registry lock poisoned")
    }

    /// The runner-owned start boundary: the published `Starting` record
    /// transitions to `Running` immediately before the executor begins.
    /// A record already claimed by cancellation intent stays `Cancelling`.
    pub fn mark_running(&self, execution_id: &ToolExecutionId) {
        let mut state = self.state();
        let Some(index) = state.index.get(execution_id).copied() else {
            return;
        };
        let record = &mut state.records[index];
        if record.lifecycle == BackgroundLifecycle::Starting {
            record.lifecycle = BackgroundLifecycle::Running;
        }
    }

    /// Rolls a prepared dispatch back: the runner is aborted and the private
    /// record is dropped. No detached execution exists afterwards.
    fn rollback_prepared(&self, execution_id: &ToolExecutionId) {
        let mut state = self.state();
        if let Some(prepared) = state.prepared.remove(execution_id) {
            prepared.runner.abort();
        }
    }

    /// Spawns the gated runner of one background execution.
    fn spawn_runner(
        &self,
        execution_id: ToolExecutionId,
        invocation: ToolInvocation,
        executor: Arc<dyn ToolExecutor>,
        cancellation: CancellationSignal,
        gate: Arc<Notify>,
    ) -> tokio::task::JoinHandle<()> {
        let registry = self.clone();
        tokio::spawn(async move {
            gate.notified().await;
            registry.mark_running(&execution_id);
            let reporter = BackgroundProgressReporter {
                registry: registry.clone(),
                execution_id: execution_id.clone(),
            };
            let resources = &registry.resources;
            let context = ToolExecutionContext {
                conversation_id: &registry.conversation_id,
                execution_id: Some(&execution_id),
                cancellation: cancellation.clone(),
                workspace: &resources.workspace,
                progress: &reporter,
                artifacts: &resources.artifacts,
                environment: &resources.environment,
            };
            let result = executor.execute(invocation, context).await;
            registry.finish(&execution_id, &result);
        })
    }
}

fn snapshot_of(record: &BackgroundRecord) -> BackgroundExecutionSnapshot {
    BackgroundExecutionSnapshot {
        execution_id: record.execution_id.clone(),
        tool_id: record.tool_id.clone(),
        tool_name: record.tool_name.clone(),
        state: record.lifecycle,
        progress: record.progress.clone(),
        result: record.result.clone(),
    }
}

/// The deterministic accepted result of a successful background dispatch.
fn accepted_result(execution_id: &ToolExecutionId, tool_name: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json {
            value: serde_json::json!({
                "execution_id": execution_id.as_str(),
                "state": "starting",
                "tool": tool_name,
            }),
        }],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}

/// The timestamped compact terminal inbound message of one settlement.
///
/// The message contains a compact deterministic terminal summary; full
/// output is never dumped into the inbound message (detailed inspection
/// remains `background_task(status)`). Artifact references are included
/// where useful.
fn terminal_inbound_message(
    execution_id: &ToolExecutionId,
    tool_name: &str,
    state: BackgroundLifecycle,
    artifacts: &[crate::message::content::FileReference],
    timestamp: chrono::DateTime<chrono::Utc>,
) -> UserMessageBlock {
    let mut content = vec![UserContentBlock::Text(TextBlock {
        text: format!(
            "Background execution {} ({tool_name}) settled: {}",
            execution_id.as_str(),
            state.name()
        ),
    })];
    for artifact in artifacts {
        content.push(UserContentBlock::File(artifact.clone()));
    }
    UserMessageBlock {
        id: MessageId::new(format!("background-{}-terminal", execution_id.as_str())),
        content,
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        timestamp: Some(timestamp),
    }
}

/// The background progress reporter handed to detached executors: it
/// updates the registry's latest progress snapshot and emits the
/// corresponding canonical execution fact.
struct BackgroundProgressReporter {
    registry: ConversationBackgroundRegistry,
    execution_id: ToolExecutionId,
}

impl ProgressReporter for BackgroundProgressReporter {
    fn report(&self, progress: ToolProgress) {
        self.registry.report_progress(&self.execution_id, progress);
    }
}

/// Test-only synchronization for the dispatch ownership commit boundary.
///
/// [`CommitBoundaryHook::enter`] is called by `commit_dispatch` while the
/// registry synchronization lock is held, immediately before the deciding
/// attempt-cancellation observation. It signals `entered` and parks the
/// calling thread until the test calls `proceed`, so a test can prove the
/// exact linearization: cancellation made observable after `entered` but
/// before `proceed` is necessarily observed at the protected boundary and
/// rolls the prepared dispatch back; a commit released without
/// interruption is never reclaimable by a later attempt cancellation.
///
/// All synchronization is `std` (mutex + condvar) because the commit
/// boundary is a `std` mutex critical section; the parking blocks the OS
/// thread, so the race tests run on a multi-threaded runtime. This module
/// exists only under `#[cfg(test)]`.
#[cfg(test)]
pub(crate) mod test_sync {
    use std::sync::{Condvar, Mutex};

    /// The two-phase gate of the commit boundary.
    #[derive(Debug, Default)]
    pub(crate) struct CommitBoundaryHook {
        state: Mutex<HookState>,
        condvar: Condvar,
    }

    #[derive(Debug, Default)]
    struct HookState {
        entered: bool,
        proceed: bool,
    }

    impl CommitBoundaryHook {
        /// Signals that the commit boundary was entered (the registry lock
        /// is held and the deciding cancellation observation is next), then
        /// blocks until [`CommitBoundaryHook::proceed`].
        pub(crate) fn enter(&self) {
            let mut state = self.state.lock().expect("commit hook lock poisoned");
            state.entered = true;
            self.condvar.notify_all();
            while !state.proceed {
                state = self.condvar.wait(state).expect("commit hook wait poisoned");
            }
        }

        /// Blocks until the commit boundary was entered.
        pub(crate) fn wait_entered(&self) {
            let mut state = self.state.lock().expect("commit hook lock poisoned");
            while !state.entered {
                state = self.condvar.wait(state).expect("commit hook wait poisoned");
            }
        }

        /// Releases a parked commit boundary.
        pub(crate) fn proceed(&self) {
            let mut state = self.state.lock().expect("commit hook lock poisoned");
            state.proceed = true;
            self.condvar.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::future::BoxFuture;
    use tokio::sync::{Notify, watch};

    use super::test_sync::CommitBoundaryHook;
    use super::{
        BACKGROUND_CANCEL_REASON, BackgroundDispatchOutcome, BackgroundLifecycle,
        BackgroundResources, ConversationBackgroundRegistry,
    };
    use crate::events::RecordingEventSink;
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolExecutionId, ToolId};
    use crate::runtime::inbound::ConversationInboundMailbox;
    use crate::runtime::types::CancellationReason;
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
    use crate::tools::types::{
        ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolProgress,
    };
    use crate::tools::workspace::Workspace;

    fn success() -> ToolExecutionResult {
        ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
        }
    }

    fn background_invocation(tool: &str) -> ToolInvocation {
        ToolInvocation {
            call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new(format!("tool-{tool}")),
            tool_name: tool.to_owned(),
            mode: ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        }
    }

    struct TestRegistry {
        _dir: tempfile::TempDir,
        registry: ConversationBackgroundRegistry,
        mailbox: ConversationInboundMailbox,
    }

    fn registry(conversation_id: &str) -> TestRegistry {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        let conversation = ConversationId::new(conversation_id);
        let mailbox = ConversationInboundMailbox::new(conversation.clone());
        let registry = ConversationBackgroundRegistry::new(
            conversation.clone(),
            BackgroundResources {
                mailbox: mailbox.clone(),
                workspace: Workspace::new(&workspace_root).expect("workspace"),
                artifacts: ArtifactStore::new(conversation, &artifacts).expect("artifacts"),
                environment: ToolEnvironment::new(),
                clock: Arc::new(crate::runtime::SystemClock),
                event_sink: None,
            },
        );
        TestRegistry {
            _dir: dir,
            registry,
            mailbox,
        }
    }

    /// An executor that waits for the release notify and then returns a
    /// fixed result, deliberately ignoring the cancellation signal.
    struct IgnoreCancellationExecutor {
        started: watch::Sender<bool>,
        release: Arc<Notify>,
        result: ToolExecutionResult,
    }

    impl IgnoreCancellationExecutor {
        fn new(result: ToolExecutionResult) -> (Self, watch::Receiver<bool>, Arc<Notify>) {
            let (started, started_rx) = watch::channel(false);
            let release = Arc::new(Notify::new());
            (
                Self {
                    started,
                    release: release.clone(),
                    result,
                },
                started_rx,
                release,
            )
        }
    }

    impl ToolExecutor for IgnoreCancellationExecutor {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: ToolExecutionContext<'a>,
        ) -> BoxFuture<'a, ToolExecutionResult> {
            self.started.send_replace(true);
            let release = self.release.clone();
            let result = self.result.clone();
            Box::pin(async move {
                release.notified().await;
                result
            })
        }
    }

    fn prepare(
        fixture: &TestRegistry,
        executor: &Arc<dyn ToolExecutor>,
    ) -> super::PreparedBackgroundDispatch {
        fixture
            .registry
            .prepare_dispatch(&background_invocation("bash"), executor)
            .expect("prepare")
    }

    /// Cancellation observable at the ownership-commit boundary rolls the
    /// prepared dispatch back: no published record, no accepted result, and
    /// the runner never begins. The test parks the commit exactly between
    /// lock acquisition and the deciding cancellation observation, so the
    /// race is proven without timing assumptions.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_observable_at_the_commit_boundary_rolls_back() {
        let fixture = registry("conv-bg");
        let (executor, mut started, _release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = prepare(&fixture, &executor);
        let attempt_cancellation = crate::runtime::cancellation::CancellationSignal::new();
        let hook = Arc::new(CommitBoundaryHook::default());
        fixture.registry.install_commit_boundary_hook(hook.clone());

        let registry = fixture.registry.clone();
        let attempt_for_task = attempt_cancellation.clone();
        let commit_task = tokio::task::spawn_blocking(move || {
            registry.commit_dispatch(prepared, &attempt_for_task)
        });
        // The commit is parked inside its critical section: the deciding
        // cancellation observation is next. The hook interactions run on
        // the blocking pool so no tokio worker thread is ever blocked.
        let entered = {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.wait_entered())
        };
        entered.await.expect("commit boundary entered");
        attempt_cancellation.cancel();
        let proceed = {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.proceed())
        };
        proceed.await.expect("commit boundary released");
        let outcome = commit_task.await.expect("commit task returns an outcome");
        assert_eq!(
            outcome,
            BackgroundDispatchOutcome::RolledBack,
            "cancellation observable at the boundary means no accepted result"
        );
        assert_eq!(
            fixture.registry.all_snapshots().len(),
            0,
            "no detached execution is published"
        );
        let started_outcome = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            started.wait_for(|started| *started),
        )
        .await;
        assert!(
            !matches!(started_outcome, Ok(Ok(_))),
            "the rolled-back runner must never begin"
        );
    }

    /// Ownership committed at the boundary is never reclaimable: after the
    /// commit completes, a later attempt cancellation cannot stop the
    /// conversation-owned runner.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commit_wins_and_later_attempt_cancellation_cannot_reclaim() {
        let fixture = registry("conv-bg");
        let (executor, mut started, release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = prepare(&fixture, &executor);
        let attempt_cancellation = crate::runtime::cancellation::CancellationSignal::new();
        let hook = Arc::new(CommitBoundaryHook::default());
        fixture.registry.install_commit_boundary_hook(hook.clone());

        let registry = fixture.registry.clone();
        let attempt_for_task = attempt_cancellation.clone();
        let commit_task = tokio::task::spawn_blocking(move || {
            registry.commit_dispatch(prepared, &attempt_for_task)
        });
        // Release the boundary immediately: ownership commits while the
        // attempt cancellation is still fresh. The hook interactions run
        // on the blocking pool so no tokio worker thread is ever blocked.
        let boundary = {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || {
                hook.wait_entered();
                hook.proceed();
            })
        };
        boundary.await.expect("commit boundary released");
        let outcome = commit_task.await.expect("commit task returns an outcome");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("expected accepted");
        };
        // Attempt cancellation after the commit cannot reclaim the work.
        attempt_cancellation.cancel();
        started
            .wait_for(|started| *started)
            .await
            .expect("the conversation-owned runner still starts");
        release.notify_one();
        let terminal = wait_for_terminal(&fixture, &execution_id).await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the conversation-owned execution settles normally after the commit"
        );
    }

    /// Cancellation winner consistency: cancellation commits while the
    /// executor runs; the executor ignores cancellation and returns
    /// `Success`; the registry canonicalizes the stored terminal result to
    /// `Cancelled` with the retained reason, and exactly one terminal
    /// inbound publication exists.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_winner_canonicalizes_the_terminal_result() {
        let fixture = registry("conv-bg");
        let (executor, mut started, release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = prepare(&fixture, &executor);
        let outcome = fixture.registry.commit_dispatch(
            prepared,
            &crate::runtime::cancellation::CancellationSignal::new(),
        );
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        started
            .wait_for(|started| *started)
            .await
            .expect("runner started");
        // Cancellation wins in the registry while the executor is running.
        let cancelling = fixture.registry.cancel(&execution_id).expect("cancel");
        assert_eq!(cancelling.state, BackgroundLifecycle::Cancelling);
        // The executor ignores cancellation and returns Success.
        release.notify_one();
        let terminal = wait_for_terminal(&fixture, &execution_id).await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Cancelled,
            "the registry cancellation winner owns settlement"
        );
        let result = terminal.result.expect("terminal result");
        assert_eq!(
            result.status,
            ToolExecutionStatus::Cancelled {
                reason: BACKGROUND_CANCEL_REASON,
            },
            "the stored terminal result agrees with the registry winner"
        );
        let batch = fixture.mailbox.drain().expect("one terminal batch");
        assert_eq!(
            batch.items().len(),
            1,
            "exactly one terminal inbound publication"
        );
        assert!(fixture.mailbox.drain().is_none());
    }

    /// Oversized multibyte progress cannot panic or strand the execution:
    /// the shared UTF-8-safe bound normalizes the message and the execution
    /// still reaches its terminal state.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_multibyte_progress_cannot_panic_or_strand() {
        struct ProgressThenDone;
        impl ToolExecutor for ProgressThenDone {
            fn execute<'a>(
                &'a self,
                _invocation: ToolInvocation,
                context: ToolExecutionContext<'a>,
            ) -> BoxFuture<'a, ToolExecutionResult> {
                let message = format!("{}😀", "x".repeat(1024));
                context.progress.report(ToolProgress {
                    message: Some(message),
                    completed: Some(1),
                    total: Some(2),
                });
                Box::pin(async move { success() })
            }
        }
        let sink = Arc::new(RecordingEventSink::new());
        let sink_dyn: Arc<dyn crate::events::RuntimeEventSink> = sink.clone();
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        let conversation = ConversationId::new("conv-bg");
        let mailbox = ConversationInboundMailbox::new(conversation.clone());
        let registry = ConversationBackgroundRegistry::new(
            conversation.clone(),
            BackgroundResources {
                mailbox: mailbox.clone(),
                workspace: Workspace::new(&workspace_root).expect("workspace"),
                artifacts: ArtifactStore::new(conversation, &artifacts).expect("artifacts"),
                environment: ToolEnvironment::new(),
                clock: Arc::new(crate::runtime::SystemClock),
                event_sink: Some(sink_dyn),
            },
        );
        let fixture = TestRegistry {
            _dir: dir,
            registry,
            mailbox,
        };
        let executor: Arc<dyn ToolExecutor> = Arc::new(ProgressThenDone);
        let prepared = fixture
            .registry
            .prepare_dispatch(&background_invocation("bash"), &executor)
            .expect("prepare");
        let outcome = fixture.registry.commit_dispatch(
            prepared,
            &crate::runtime::cancellation::CancellationSignal::new(),
        );
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        let terminal = wait_for_terminal(&fixture, &execution_id).await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the oversized progress must not strand the execution"
        );
        let progress = terminal.progress.expect("progress snapshot");
        let message = progress.message.expect("message");
        assert!(
            message.len() <= crate::tools::limits::MAX_PROGRESS_MESSAGE_BYTES,
            "the snapshot message is bounded"
        );
        assert_eq!(progress.completed, Some(1));
        assert_eq!(progress.total, Some(2));
        let progress_events = sink
            .as_ref()
            .events()
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    crate::events::RuntimeEvent::ToolExecutionProgress { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(progress_events.len(), 1);
    }

    async fn wait_for_terminal(
        fixture: &TestRegistry,
        execution_id: &ToolExecutionId,
    ) -> super::BackgroundExecutionSnapshot {
        // Polls the authoritative registry state itself (the very state
        // under test) with a strict deadlock guard.
        for _ in 0..400 {
            let snapshot = fixture.registry.snapshot(execution_id).expect("snapshot");
            if snapshot.state.is_terminal() {
                return snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("execution never reached a terminal state");
    }

    /// The unused-reason guard: `BACKGROUND_CANCEL_REASON` is the
    /// conversation-owned cancellation reason.
    #[test]
    fn background_cancel_reason_is_user_requested() {
        assert_eq!(BACKGROUND_CANCEL_REASON, CancellationReason::UserRequested);
    }
}
