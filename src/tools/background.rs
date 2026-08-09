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
//! performs the final attempt-cancellation checkpoint and then COMMITS
//! conversation ownership: the record is published as `Starting`, the gate
//! is released, and the accepted attempt-facing result is produced. The
//! commit is the background dispatch linearization point:
//!
//! - before commit, attempt cancellation can roll the prepared dispatch back
//!   (no accepted result, no detached execution);
//! - after commit, ordinary attempt cancellation can no longer reclaim or
//!   cancel the background work.
//!
//! # Cancellation-vs-completion race
//!
//! The first registry transition that commits either terminal completion or
//! cancellation intent wins the race. If completion
//! (`Succeeded`/`Failed`/`Cancelled`) commits first, a later cancel is an
//! idempotent no-op returning the terminal snapshot. If cancellation
//! intent commits first (`Starting`/`Running` → `Cancelling`), cancellation
//! owns settlement and a later normal executor return cannot overwrite the
//! cancellation winner with `Succeeded`.
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
use serde::{Deserialize, Serialize};

use crate::tools::artifacts::ArtifactStore;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
use crate::tools::limits::MAX_PROGRESS_MESSAGE_BYTES;
use crate::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolProgress,
    ToolResultContent,
};
use crate::tools::workspace::Workspace;

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
            })),
            resources,
        }
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
    /// Under the registry synchronization boundary, performs the final
    /// attempt-cancellation checkpoint. If the attempt cancellation is
    /// already observable, the prepared dispatch is rolled back — the
    /// runner is aborted, no execution is detached, and no accepted result
    /// exists. Otherwise conversation ownership commits: the record is
    /// published as `Starting`, the runner gate is released, and the
    /// accepted attempt-facing result is produced. No await or cancellation
    /// checkpoint can split the ownership commit from the accepted result.
    #[must_use]
    pub fn commit_dispatch(
        &self,
        mut prepared: PreparedBackgroundDispatch,
        attempt_cancellation: &CancellationSignal,
    ) -> BackgroundDispatchOutcome {
        if attempt_cancellation.is_cancelled() {
            // The rollback happens through the prepared handle's drop
            // semantics; this is also the final attempt-cancellation
            // checkpoint, so no accepted result may be produced.
            prepared.committed = true;
            self.rollback_prepared(&prepared.execution_id);
            return BackgroundDispatchOutcome::RolledBack;
        }
        let mut state = self.state();
        let Some(prepared_record) = state.prepared.remove(&prepared.execution_id) else {
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
    #[must_use]
    pub fn cancel(&self, execution_id: &ToolExecutionId) -> Option<BackgroundExecutionSnapshot> {
        let mut state = self.state();
        let index = *state.index.get(execution_id)?;
        let record = &mut state.records[index];
        match record.lifecycle {
            BackgroundLifecycle::Starting | BackgroundLifecycle::Running => {
                record.lifecycle = BackgroundLifecycle::Cancelling;
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
    pub fn finish(&self, execution_id: &ToolExecutionId, result: &ToolExecutionResult) {
        let mut state = self.state();
        let Some(index) = state.index.get(execution_id).copied() else {
            return;
        };
        let record = &mut state.records[index];
        if record.lifecycle.is_terminal() {
            return;
        }
        let settled = match record.lifecycle {
            BackgroundLifecycle::Starting | BackgroundLifecycle::Running => match result.status {
                ToolExecutionStatus::Success => BackgroundLifecycle::Succeeded,
                ToolExecutionStatus::Cancelled { .. } => BackgroundLifecycle::Cancelled,
                ToolExecutionStatus::Failed { .. }
                | ToolExecutionStatus::TimedOut
                | ToolExecutionStatus::Interrupted => BackgroundLifecycle::Failed,
            },
            BackgroundLifecycle::Cancelling => {
                // Cancellation intent already owns settlement. A normal
                // executor return must not overwrite the cancellation
                // winner; only an explicit runtime/process-control failure
                // is represented as Failed.
                match result.status {
                    ToolExecutionStatus::Failed { .. } => BackgroundLifecycle::Failed,
                    _ => BackgroundLifecycle::Cancelled,
                }
            }
            BackgroundLifecycle::Succeeded
            | BackgroundLifecycle::Failed
            | BackgroundLifecycle::Cancelled => return,
        };
        record.lifecycle = settled;
        record.result = Some(result.clone());
        if record.notification != NotificationState::Pending {
            return;
        }
        record.notification = NotificationState::Publishing;
        let message = terminal_inbound_message(
            execution_id,
            &record.tool_name,
            settled,
            &result.artifacts,
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
    /// Only the current/latest bounded progress snapshot is retained; no
    /// unbounded progress history exists in the registry. Progress of a
    /// terminal execution is ignored.
    pub fn report_progress(&self, execution_id: &ToolExecutionId, progress: ToolProgress) {
        let ToolProgress {
            message,
            completed,
            total,
        } = progress;
        let bounded = ToolProgress {
            message: message.map(|message| {
                let mut bounded = message;
                bounded.truncate(MAX_PROGRESS_MESSAGE_BYTES);
                bounded
            }),
            completed,
            total,
        };
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
