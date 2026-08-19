//! The conversation-owned logical owner and registry of subagent children
//! (Issue #60).
//!
//! The registry owns the **logical** boundary of every subagent child of
//! one conversation:
//!
//! ```text
//! ownership        (prepare/commit linearization; the durable
//!                   SubagentOwnershipCommitted event is the single commit)
//! lifecycle        (typed SubagentLifecycle; terminal publication
//!                   exactly-once through the durable compound transaction)
//! identity         (SubagentId ordinals; never a PID)
//! cancellation     (intent commit -> driver command -> escalation)
//! settlement       (physical outcome -> terminal candidate -> durable
//!                   result acceptance -> capacity release)
//! recovery         (ordinal reseed from the durable authority)
//! ```
//!
//! It never holds an OS process handle. It stages a [`StagedChild`], and
//! the one ownership commit moves the handle into the driver task — the
//! sole low-level process owner ([`super::process`]). The registry holds
//! only the narrow driver command handle for cancellation.
//!
//! # Two-stage start
//!
//! [`SubagentRegistry::prepare`] performs every fallible stage privately —
//! input validation, identity allocation, process spawn, version
//! handshake, runtime activation — without publishing any conversation
//! state. [`SubagentRegistry::commit`] is then the one commit/rollback
//! linearization point: a failed or cancelled commit tears the staged child
//! down completely and leaves no registry record, no capacity consumption,
//! and no durable trace. A successful commit is the point of no return:
//! the attempt's later cancellation cannot reclaim the child.
//!
//! # Durability failure posture
//!
//! A terminal publication that cannot reach the durable authority enters
//! `PublishingTerminal`, is retried on the bounded policy, and is then
//! marked abandoned with a notification-plane failure diagnostic; it can
//! never become another record, and the runtime's durability-failed state
//! bars new submissions through the ordinary path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use crate::runtime::RuntimeClock;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::types::CancellationReason;

use super::ipc::DelegationFrame;
use super::process::{PhysicalOutcome, StagedChild, SubagentSpawnPlan};
use super::{
    MAX_CONTEXT_PACKAGE_BYTES, MAX_RESULT_CONTENT_BYTES, MAX_TASK_BYTES, SubagentProfile,
    SubagentTerminalState, ownership_event, terminal_publication,
};

/// The highest lifecycle state of one subagent child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubagentLifecycle {
    /// Ownership committed; the delegation is in flight or running.
    Running,
    /// Cancellation intent is committed; escalation may be in flight.
    Cancelling,
    /// The terminal outcome is known but its publication has not yet
    /// reached the durable authority.
    PublishingTerminal,
    /// The terminal result is durably published.
    Succeeded,
    /// The terminal failure is durably published.
    Failed,
    /// The terminal cancellation is durably published.
    Cancelled,
}

impl SubagentLifecycle {
    const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    const fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Cancelling)
    }
}

/// The canonicalized terminal outcome awaiting publication.
#[derive(Debug, Clone)]
struct TerminalCandidate {
    state: TerminalState,
    /// The bounded result content (succeeded only).
    content: Option<String>,
    /// The bounded failure diagnostic (failed only).
    diagnostic: Option<String>,
    /// The cancellation detail (cancelled only).
    reason: Option<CancellationReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalState {
    Succeeded,
    Failed,
    Cancelled,
}

/// The notification-plane state of one terminal result, mirroring the
/// background execution notification vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationState {
    None,
    Delivered,
    Failed,
}

impl NotificationState {
    const fn has_pending_delivery(self) -> bool {
        matches!(self, Self::Failed)
    }
}

struct SubagentRecord {
    subagent_id: SubagentId,
    child_agent_id: AgentId,
    child_conversation_id: ConversationId,
    tool_call_id: ToolCallId,
    profile: SubagentProfile,
    lifecycle: SubagentLifecycle,
    cancel_reason: Option<CancellationReason>,
    /// The narrow cancellation handle into the driver task — never an OS
    /// process handle.
    control: Option<tokio::sync::mpsc::Sender<super::process::DriverCommand>>,
    /// The bounded terminal result content or diagnostic.
    detail: Option<String>,
    pending_terminal: Option<TerminalCandidate>,
    publication_abandoned: bool,
    notification: NotificationState,
    started_at: DateTime<Utc>,
}

impl SubagentRecord {
    fn snapshot(&self) -> SubagentSnapshot {
        let state = match self.lifecycle {
            SubagentLifecycle::Running => SubagentState::Running,
            SubagentLifecycle::Cancelling => SubagentState::Cancelling,
            SubagentLifecycle::PublishingTerminal => SubagentState::PublishingTerminal,
            SubagentLifecycle::Succeeded => SubagentState::Succeeded,
            SubagentLifecycle::Failed => SubagentState::Failed,
            SubagentLifecycle::Cancelled => SubagentState::Cancelled,
        };
        SubagentSnapshot {
            subagent_id: self.subagent_id.clone(),
            child_agent_id: self.child_agent_id.clone(),
            child_conversation_id: self.child_conversation_id.clone(),
            tool_call_id: self.tool_call_id.clone(),
            profile: self.profile.name().to_owned(),
            state,
            detail: self.detail.clone(),
            publication_abandoned: self.publication_abandoned,
            settled: self.lifecycle.is_terminal() && !self.publication_abandoned,
            started_at: self.started_at,
        }
    }
}

struct RegistryState {
    next_ordinal: u64,
    records: Vec<SubagentRecord>,
    index: HashMap<SubagentId, usize>,
    observer: Option<Arc<dyn SubagentObserver>>,
    failure_sink: Option<Arc<dyn SubagentDurabilityFailureSink>>,
    #[cfg(test)]
    commit_hook: Option<Arc<CommitBoundaryHook>>,
}

/// The public lifecycle vocabulary of one subagent snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentState {
    /// Ownership committed; the delegation is in flight or running.
    Running,
    /// Cancellation intent committed; escalation may be in flight.
    Cancelling,
    /// The terminal outcome is known; publication is not yet durable.
    PublishingTerminal,
    /// The result is durably published.
    Succeeded,
    /// The failure is durably published.
    Failed,
    /// The cancellation is durably published.
    Cancelled,
}

/// A consistency snapshot of one subagent child.
///
/// Read-model materialization only: every field is derived from the
/// registry's state machine, never an authority of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentSnapshot {
    /// The conversation-owned subagent identity.
    pub subagent_id: SubagentId,
    /// The child agent identity (provenance of its answer).
    pub child_agent_id: AgentId,
    /// The child's own durable conversation identity.
    pub child_conversation_id: ConversationId,
    /// The delegating tool call.
    pub tool_call_id: ToolCallId,
    /// The frozen profile identity.
    pub profile: String,
    /// The lifecycle state.
    pub state: SubagentState,
    /// The bounded result content (succeeded) or failure/cancellation
    /// detail, once known.
    pub detail: Option<String>,
    /// Whether a terminal publication could not reach the durable
    /// authority and was abandoned.
    pub publication_abandoned: bool,
    /// Whether the child reached a settled state (terminal, publication
    /// not abandoned).
    pub settled: bool,
    /// When the ownership committed.
    pub started_at: DateTime<Utc>,
}

/// The inputs of one subagent start.
#[derive(Debug, Clone)]
pub struct SubagentStartSpec {
    /// The frozen execution profile.
    pub profile: SubagentProfile,
    /// The delegated task.
    pub task: String,
    /// The explicit bounded context package.
    pub context: Option<String>,
    /// The delegating tool call.
    pub tool_call_id: ToolCallId,
}

/// A privately prepared subagent start: everything fallible already
/// succeeded, but nothing is published or owned yet.
#[derive(Debug)]
pub struct PreparedSubagent {
    subagent_id: SubagentId,
    child_agent_id: AgentId,
    child_conversation_id: ConversationId,
    tool_call_id: ToolCallId,
    profile: SubagentProfile,
    task: String,
    context: Option<String>,
    staged: StagedChild,
}

/// The outcome of a successful ownership commit.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentAccepted {
    /// The conversation-owned subagent identity.
    pub subagent_id: SubagentId,
    /// The child agent identity.
    pub child_agent_id: AgentId,
    /// The child conversation identity.
    pub child_conversation_id: ConversationId,
    /// The frozen profile identity.
    pub profile: String,
    /// The tool result the delegating call receives.
    pub result: serde_json::Value,
}

/// The outcome of one [`SubagentRegistry::commit`].
#[derive(Debug)]
pub enum SubagentStartOutcome {
    /// Ownership committed; the child is running behind the start gate
    /// release.
    Accepted(SubagentAccepted),
    /// The attempt cancellation won the race against the commit; the
    /// staged child was torn down and nothing was published.
    RolledBack,
}

/// A typed start failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentStartError {
    /// The owning conversation is draining or draining-complete.
    ConversationInactive,
    /// The delegated task is empty or exceeds [`MAX_TASK_BYTES`].
    InvalidTask {
        /// The offending byte length.
        bytes: usize,
    },
    /// The explicit context package exceeds [`MAX_CONTEXT_PACKAGE_BYTES`].
    ContextOversized {
        /// The offending byte length.
        bytes: usize,
    },
    /// The per-conversation concurrency bound is reached.
    CapacityExceeded {
        /// The configured bound.
        max: usize,
    },
    /// Staging the child process failed.
    Spawn {
        /// The failure detail.
        detail: String,
    },
    /// The durable ownership commit failed.
    Durability {
        /// The failure detail.
        detail: String,
    },
}

impl core::fmt::Display for SubagentStartError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ConversationInactive => {
                write!(f, "the owning conversation is no longer active")
            }
            Self::InvalidTask { bytes } => write!(
                f,
                "the delegated task is empty or exceeds the {MAX_TASK_BYTES}-byte bound \
                 ({bytes} bytes)"
            ),
            Self::ContextOversized { bytes } => write!(
                f,
                "the context package exceeds the {MAX_CONTEXT_PACKAGE_BYTES}-byte bound \
                 ({bytes} bytes)"
            ),
            Self::CapacityExceeded { max } => {
                write!(f, "the per-conversation subagent bound ({max}) is reached")
            }
            Self::Spawn { detail } => write!(f, "could not start the child runtime: {detail}"),
            Self::Durability { detail } => {
                write!(f, "the durable ownership commit failed: {detail}")
            }
        }
    }
}

impl std::error::Error for SubagentStartError {}

/// The observation seam of the subagent plane (TUI / Runtime Client).
pub trait SubagentObserver: Send + Sync {
    /// Called under the registry lock with each new consistency snapshot;
    /// the implementation must be cheap and nonblocking.
    fn on_snapshot(&self, snapshot: &SubagentSnapshot);
}

/// The durability-failure reporting seam of the subagent plane.
pub trait SubagentDurabilityFailureSink: Send + Sync {
    /// A terminal publication could not reach the durable authority.
    fn terminal_publication_failed(&self, subagent_id: &SubagentId, diagnostic: &str);
}

/// The composition inputs of the registry.
#[derive(Clone)]
pub struct SubagentRegistryConfig {
    /// The owning conversation identity.
    pub conversation_id: ConversationId,
    /// The parent (delegating) agent identity.
    pub agent_id: AgentId,
    /// The conversation inbound mailbox (durable authority).
    pub mailbox: ConversationInboundMailbox,
    /// The runtime clock.
    pub clock: Arc<dyn RuntimeClock>,
    /// The process spawn plan.
    pub spawn: SubagentSpawnPlan,
    /// The per-conversation concurrency bound.
    pub max_active: usize,
}

/// The sole logical owner and registry of a conversation's subagent
/// children.
///
/// Cheaply cloneable: every clone shares the one registry state, the same
/// contract as the background registry.
pub struct SubagentRegistry {
    config: SubagentRegistryConfig,
    state: Arc<Mutex<RegistryState>>,
    state_version: tokio::sync::watch::Sender<u64>,
}

impl Clone for SubagentRegistry {
    fn clone(&self) -> Self {
        self.clone_for_task()
    }
}

impl SubagentRegistry {
    /// Creates the registry for one conversation.
    #[must_use]
    pub fn new(config: SubagentRegistryConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(RegistryState {
                next_ordinal: 1,
                records: Vec::new(),
                index: HashMap::new(),
                observer: None,
                failure_sink: None,
                #[cfg(test)]
                commit_hook: None,
            })),
            state_version: tokio::sync::watch::Sender::new(0),
        }
    }

    /// Reseeds the ordinal sequence from the durable authority during
    /// startup recovery, so a recovered conversation never reissues an
    /// ordinal that already entered durable authority.
    pub fn restore_sequence_watermark(&self, highest_ordinal: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.next_ordinal = state.next_ordinal.max(highest_ordinal + 1);
    }

    /// Installs the observation seam and immediately emits the current
    /// snapshot of every known record.
    pub fn install_observer_and_snapshots(&self, observer: Arc<dyn SubagentObserver>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for record in &state.records {
            observer.on_snapshot(&record.snapshot());
        }
        state.observer = Some(observer);
    }

    /// Installs the durability-failure sink.
    pub fn install_failure_sink(&self, sink: Arc<dyn SubagentDurabilityFailureSink>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.failure_sink = Some(sink);
    }

    /// **Prepare.** Runs every fallible stage privately: input validation,
    /// identity allocation, process spawn, and the activation handshake.
    /// Nothing is published, no capacity is consumed, and a failure leaves
    /// no trace.
    ///
    /// # Errors
    ///
    /// Returns the typed [`SubagentStartError`] of the first failing stage.
    pub async fn prepare(
        &self,
        spec: &SubagentStartSpec,
    ) -> Result<PreparedSubagent, SubagentStartError> {
        let task_bytes = spec.task.len();
        if spec.task.trim().is_empty() || task_bytes > MAX_TASK_BYTES {
            return Err(SubagentStartError::InvalidTask { bytes: task_bytes });
        }
        if let Some(context) = &spec.context {
            let bytes = context.len();
            if bytes > MAX_CONTEXT_PACKAGE_BYTES {
                return Err(SubagentStartError::ContextOversized { bytes });
            }
        }
        if self.config.mailbox.begin_running_admission().is_err() {
            return Err(SubagentStartError::ConversationInactive);
        }
        let ordinal = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let ordinal = state.next_ordinal;
            state.next_ordinal += 1;
            ordinal
        };
        let subagent_id = SubagentId::for_conversation(&self.config.conversation_id, ordinal);
        let child_conversation_id = ConversationId::new(subagent_id.as_str());
        let child_agent_id = AgentId::new(format!("agent-{subagent_id}"));
        let child_spec = self.config.spawn.child_spec(
            &subagent_id,
            &child_conversation_id,
            &child_agent_id,
            &self.config.agent_id,
            spec.profile,
        );
        let staged = super::process::spawn_staged(&self.config.spawn, &child_spec)
            .await
            .map_err(|error| SubagentStartError::Spawn {
                detail: error.to_string(),
            })?;
        Ok(PreparedSubagent {
            subagent_id,
            child_agent_id,
            child_conversation_id,
            tool_call_id: spec.tool_call_id.clone(),
            profile: spec.profile,
            task: spec.task.clone(),
            context: spec.context.clone(),
            staged,
        })
    }

    /// **Commit.** The one commit/rollback linearization point.
    ///
    /// A rolled-back or failed commit tears the staged child down
    /// completely (killed, reaped, runtime root removed) before returning;
    /// a successful commit publishes the durable ownership event, creates
    /// the record, releases the start gate, and returns the tool result.
    pub async fn commit(
        &self,
        prepared: PreparedSubagent,
        attempt_cancellation: &CancellationSignal,
    ) -> Result<SubagentStartOutcome, SubagentStartError> {
        if self.config.mailbox.begin_running_admission().is_err() {
            prepared.staged.rollback().await;
            return Err(SubagentStartError::ConversationInactive);
        }
        let PreparedSubagent {
            subagent_id,
            child_agent_id,
            child_conversation_id,
            tool_call_id,
            profile,
            task,
            context,
            staged,
        } = prepared;
        enum Decision {
            Accepted,
            RolledBack,
            Failed(SubagentStartError),
        }
        let decision = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mailbox = self.config.mailbox.clone();
            let clock = self.config.clock.clone();
            let config = &self.config;
            let decision = match mailbox.with_running_commit(|| {
                if mailbox.is_bound_inactive() {
                    return Decision::Failed(SubagentStartError::ConversationInactive);
                }
                #[cfg(test)]
                if let Some(hook) = &state.commit_hook {
                    hook.wait();
                }
                let active = state
                    .records
                    .iter()
                    .filter(|record| record.lifecycle.is_active() && !record.publication_abandoned)
                    .count();
                if active >= config.max_active {
                    return Decision::Failed(SubagentStartError::CapacityExceeded {
                        max: config.max_active,
                    });
                }
                if attempt_cancellation.is_cancelled() {
                    return Decision::RolledBack;
                }
                if let Err(error) = mailbox.commit_subagent_ownership(ownership_event(
                    &config.conversation_id,
                    &subagent_id,
                    &child_agent_id,
                    &child_conversation_id,
                    &tool_call_id,
                    profile,
                    clock.now(),
                )) {
                    return Decision::Failed(SubagentStartError::Durability {
                        detail: error.to_string(),
                    });
                }
                Decision::Accepted
            }) {
                Ok(decision) => decision,
                Err(_) => Decision::Failed(SubagentStartError::ConversationInactive),
            };
            if matches!(decision, Decision::Accepted) {
                let record = SubagentRecord {
                    subagent_id: subagent_id.clone(),
                    child_agent_id: child_agent_id.clone(),
                    child_conversation_id: child_conversation_id.clone(),
                    tool_call_id,
                    profile,
                    lifecycle: SubagentLifecycle::Running,
                    cancel_reason: None,
                    control: None,
                    detail: None,
                    pending_terminal: None,
                    publication_abandoned: false,
                    notification: NotificationState::None,
                    started_at: clock.now(),
                };
                let index = state.records.len();
                state.index.insert(subagent_id.clone(), index);
                state.records.push(record);
                publish_snapshot(&mut state, &self.state_version, index);
            }
            decision
        };
        match decision {
            Decision::RolledBack => {
                staged.rollback().await;
                Ok(SubagentStartOutcome::RolledBack)
            }
            Decision::Failed(error) => {
                staged.rollback().await;
                Err(error)
            }
            Decision::Accepted => {
                // Point of no return: the child is conversation-owned. The
                // OS handle moves into the driver task; the registry keeps
                // only the narrow command handle.
                let driver = staged.into_driver(DelegationFrame { task, context });
                let (commands, task) = driver.split();
                {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(&index) = state.index.get(&subagent_id) {
                        state.records[index].control = Some(commands);
                    }
                }
                let registry = self.clone_for_task();
                let settlement_id = subagent_id.clone();
                tokio::spawn(async move {
                    let outcome = task.await.unwrap_or(PhysicalOutcome::Lost {
                        diagnostic: "the child driver task failed".to_owned(),
                    });
                    registry.settle_from_driver(&settlement_id, outcome);
                });
                Ok(SubagentStartOutcome::Accepted(SubagentAccepted {
                    subagent_id,
                    child_agent_id,
                    child_conversation_id,
                    profile: profile.name().to_owned(),
                    result: serde_json::json!({
                        "status": "running",
                        "note": "The child runtime is running asynchronously. Its answer \
                                 arrives as a new turn from the child agent; do not retry \
                                 or poll for it."
                    }),
                }))
            }
        }
    }

    fn clone_for_task(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            state_version: self.state_version.clone(),
        }
    }

    /// The consistency snapshot of one subagent, if the registry knows it.
    #[must_use]
    pub fn snapshot(&self, subagent_id: &SubagentId) -> Option<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .index
            .get(subagent_id)
            .map(|&index| state.records[index].snapshot())
    }

    /// The consistency snapshots of every known subagent, in ordinal order.
    #[must_use]
    pub fn all_snapshots(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.records.iter().map(SubagentRecord::snapshot).collect()
    }

    /// The unsettled subagents in deterministic ordinal order (drain).
    #[must_use]
    pub fn unsettled_snapshot(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .records
            .iter()
            .filter(|record| {
                (record.lifecycle.is_active()
                    || matches!(record.lifecycle, SubagentLifecycle::PublishingTerminal))
                    && !record.publication_abandoned
            })
            .map(SubagentRecord::snapshot)
            .collect()
    }

    /// The subagents whose terminal publication was abandoned.
    #[must_use]
    pub fn abandoned_publications(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .records
            .iter()
            .filter(|record| record.publication_abandoned)
            .map(SubagentRecord::snapshot)
            .collect()
    }

    /// Whether any terminal notification still owes observable delivery
    /// work (drain observability).
    #[must_use]
    pub fn has_unresolved_delivery_work(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .records
            .iter()
            .any(|record| record.notification.has_pending_delivery())
    }

    /// **Cancellation.** Commits the cancellation intent under the
    /// registry lock and forwards it into the driver task.
    ///
    /// The call is synchronous and never blocks on process teardown: the
    /// driver owns the Cancel frame, the escalation, and the reap; the
    /// terminal settlement follows through [`Self::settle_from_driver`].
    /// Cancelling an unknown, terminal, or abandoned record is a no-op
    /// returning the current snapshot.
    #[must_use]
    pub fn cancel(
        &self,
        subagent_id: &SubagentId,
        reason: CancellationReason,
    ) -> Option<SubagentSnapshot> {
        if self.config.mailbox.begin_settlement_admission().is_err() {
            return self.snapshot(subagent_id);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let &index = state.index.get(subagent_id)?;
        let record = &mut state.records[index];
        if record.lifecycle.is_terminal() || record.publication_abandoned {
            return Some(record.snapshot());
        }
        if !matches!(record.lifecycle, SubagentLifecycle::Cancelling) {
            record.lifecycle = SubagentLifecycle::Cancelling;
            record.cancel_reason = Some(reason);
            if let Some(control) = &record.control {
                let _ = control.try_send(super::process::DriverCommand::Cancel);
            }
            publish_snapshot(&mut state, &self.state_version, index);
        }
        Some(state.records[index].snapshot())
    }

    /// Cancels every active subagent (runtime drain).
    pub fn cancel_all(&self, reason: CancellationReason) {
        let ids: Vec<SubagentId> = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .records
                .iter()
                .filter(|record| record.lifecycle.is_active())
                .map(|record| record.subagent_id.clone())
                .collect()
        };
        for id in ids {
            let _ = self.cancel(&id, reason.clone());
        }
    }

    /// Waits until one subagent is settled or abandoned (runtime drain;
    /// never agent-loop blocking).
    pub async fn wait_until_settled(&self, subagent_id: &SubagentId) -> Option<SubagentSnapshot> {
        let mut rx = self.state_version.subscribe();
        loop {
            let snapshot = self.snapshot(subagent_id)?;
            if snapshot.settled || snapshot.publication_abandoned {
                return Some(snapshot);
            }
            if rx.changed().await.is_err() {
                return None;
            }
        }
    }

    /// **Settlement.** Canonicalizes the driver's physical outcome against
    /// the lifecycle, then drives the durable result acceptance.
    ///
    /// Cancellation intent is canonical: once committed, every later
    /// physical outcome settles as cancelled — except an explicit
    /// process-control failure (`Lost`), which stays failed. The durable
    /// compound transaction makes the publication exactly-once.
    fn settle_from_driver(&self, subagent_id: &SubagentId, outcome: PhysicalOutcome) {
        if self.config.mailbox.begin_settlement_admission().is_err() {
            return;
        }
        let candidate = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(&index) = state.index.get(subagent_id) else {
                return;
            };
            let record = &mut state.records[index];
            if record.lifecycle.is_terminal() || record.publication_abandoned {
                return;
            }
            let cancelling = matches!(record.lifecycle, SubagentLifecycle::Cancelling);
            let candidate = match outcome {
                PhysicalOutcome::Completed(frame) => match (cancelling, frame.status) {
                    (false, super::ipc::ChildResultStatus::Succeeded) => TerminalCandidate {
                        state: TerminalState::Succeeded,
                        content: Some(bound(
                            frame.content.unwrap_or_default(),
                            MAX_RESULT_CONTENT_BYTES,
                        )),
                        diagnostic: None,
                        reason: None,
                    },
                    (false, super::ipc::ChildResultStatus::Failed) => TerminalCandidate {
                        state: TerminalState::Failed,
                        content: None,
                        diagnostic: Some(bound(
                            frame
                                .diagnostic
                                .unwrap_or_else(|| "the child attempt failed".to_owned()),
                            MAX_RESULT_CONTENT_BYTES,
                        )),
                        reason: None,
                    },
                    (false, super::ipc::ChildResultStatus::Cancelled) => TerminalCandidate {
                        state: TerminalState::Cancelled,
                        content: None,
                        diagnostic: None,
                        reason: Some(CancellationReason::UserRequested),
                    },
                    // Cancellation intent is canonical: a completed frame
                    // after the intent settles as cancelled.
                    (true, _) => TerminalCandidate {
                        state: TerminalState::Cancelled,
                        content: None,
                        diagnostic: None,
                        reason: record.cancel_reason.clone(),
                    },
                },
                PhysicalOutcome::Lost { diagnostic } => TerminalCandidate {
                    // An explicit process-control failure settles as failed
                    // even after cancellation intent.
                    state: TerminalState::Failed,
                    content: None,
                    diagnostic: Some(bound(diagnostic, MAX_RESULT_CONTENT_BYTES)),
                    reason: None,
                },
            };
            record.pending_terminal = Some(candidate.clone());
            record.detail = candidate
                .content
                .clone()
                .or_else(|| candidate.diagnostic.clone())
                .or_else(|| {
                    candidate
                        .reason
                        .as_ref()
                        .map(reason_text)
                        .map(str::to_owned)
                });
            candidate
        };
        self.publish_terminal(subagent_id, &candidate);
    }

    /// Attempts the durable terminal publication; on failure, enters
    /// `PublishingTerminal` and schedules the bounded retry.
    fn publish_terminal(&self, subagent_id: &SubagentId, candidate: &TerminalCandidate) {
        let attempt = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(&index) = state.index.get(subagent_id) else {
                return;
            };
            let record = &state.records[index];
            let (draft, event) = terminal_publication(
                &self.config.conversation_id,
                subagent_id,
                &record.child_agent_id,
                candidate_state(candidate),
                terminal_blocks(record, candidate),
                self.config.clock.now(),
            );
            let result = self.config.mailbox.accept_draft_with_event(draft, event);
            let record = &mut state.records[index];
            match result {
                Ok(_) => {
                    record.lifecycle = match candidate.state {
                        TerminalState::Succeeded => SubagentLifecycle::Succeeded,
                        TerminalState::Failed => SubagentLifecycle::Failed,
                        TerminalState::Cancelled => SubagentLifecycle::Cancelled,
                    };
                    record.pending_terminal = None;
                    record.notification = NotificationState::Delivered;
                    publish_snapshot(&mut state, &self.state_version, index);
                    return;
                }
                Err(error) => {
                    record.lifecycle = SubagentLifecycle::PublishingTerminal;
                    record.notification = NotificationState::Failed;
                    let diagnostic = error.to_string();
                    if let Some(sink) = &state.failure_sink {
                        sink.terminal_publication_failed(subagent_id, &diagnostic);
                    }
                    publish_snapshot(&mut state, &self.state_version, index);
                    diagnostic
                }
            }
        };
        // Bounded publication retry; the candidate is stable from
        // pending_terminal.
        let registry = self.clone_for_task();
        let id = subagent_id.clone();
        tokio::spawn(async move {
            for _ in 0..2 {
                if registry.retry_terminal_publication(&id) {
                    return;
                }
            }
            registry.mark_publication_abandoned(&id);
        });
        let _ = attempt;
    }

    /// One bounded publication retry. Returns whether the terminal is now
    /// durably committed.
    fn retry_terminal_publication(&self, subagent_id: &SubagentId) -> bool {
        if self.config.mailbox.begin_settlement_admission().is_err() {
            return false;
        }
        let candidate = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(&index) = state.index.get(subagent_id) else {
                return true;
            };
            let record = &state.records[index];
            if record.lifecycle.is_terminal() {
                return true;
            }
            record.pending_terminal.clone()
        };
        let Some(candidate) = candidate else {
            return true;
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(&index) = state.index.get(subagent_id) else {
            return true;
        };
        let record = &state.records[index];
        let (draft, event) = terminal_publication(
            &self.config.conversation_id,
            subagent_id,
            &record.child_agent_id,
            candidate_state(&candidate),
            terminal_blocks(record, &candidate),
            self.config.clock.now(),
        );
        match self.config.mailbox.accept_draft_with_event(draft, event) {
            Ok(_) => {
                let record = &mut state.records[index];
                record.lifecycle = match candidate.state {
                    TerminalState::Succeeded => SubagentLifecycle::Succeeded,
                    TerminalState::Failed => SubagentLifecycle::Failed,
                    TerminalState::Cancelled => SubagentLifecycle::Cancelled,
                };
                record.pending_terminal = None;
                record.notification = NotificationState::Delivered;
                publish_snapshot(&mut state, &self.state_version, index);
                true
            }
            Err(error) => {
                let diagnostic = error.to_string();
                if let Some(sink) = &state.failure_sink {
                    sink.terminal_publication_failed(subagent_id, &diagnostic);
                }
                false
            }
        }
    }

    /// Marks a terminal publication abandoned after the bounded retry.
    fn mark_publication_abandoned(&self, subagent_id: &SubagentId) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        state.records[index].publication_abandoned = true;
        publish_snapshot(&mut state, &self.state_version, index);
    }

    /// Installs a commit-boundary hook (tests only).
    #[cfg(test)]
    pub fn install_commit_boundary_hook(&self, hook: Arc<CommitBoundaryHook>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.commit_hook = Some(hook);
    }
}

/// Builds the bounded content blocks of a terminal publication.
fn terminal_blocks(
    record: &SubagentRecord,
    candidate: &TerminalCandidate,
) -> Vec<crate::message::types::UserContentBlock> {
    let text = match candidate.state {
        TerminalState::Succeeded => candidate.content.clone().unwrap_or_default(),
        TerminalState::Failed => format!(
            "Subagent {} (profile {}) failed: {}",
            record.subagent_id,
            record.profile.name(),
            candidate
                .diagnostic
                .clone()
                .unwrap_or_else(|| "unknown failure".to_owned())
        ),
        TerminalState::Cancelled => format!(
            "Subagent {} (profile {}) was cancelled ({}).",
            record.subagent_id,
            record.profile.name(),
            candidate
                .reason
                .as_ref()
                .map(reason_text)
                .unwrap_or("cancelled")
        ),
    };
    vec![crate::message::types::UserContentBlock::Text(
        crate::message::content::TextBlock { text },
    )]
}

/// Maps the registry's terminal vocabulary onto the durable event's.
const fn candidate_state(candidate: &TerminalCandidate) -> SubagentTerminalState {
    match candidate.state {
        TerminalState::Succeeded => SubagentTerminalState::Succeeded,
        TerminalState::Failed => SubagentTerminalState::Failed,
        TerminalState::Cancelled => SubagentTerminalState::Cancelled,
    }
}

/// The human-readable cancellation detail.
const fn reason_text(reason: &CancellationReason) -> &'static str {
    match reason {
        CancellationReason::UserRequested => "requested by the user",
        CancellationReason::RuntimeShutdown => "the runtime is shutting down",
        CancellationReason::ParentCancelled => "the parent operation was cancelled",
    }
}

/// Caps a frame-carried payload at the documented bound.
fn bound(mut value: String, max: usize) -> String {
    if value.len() > max {
        value.truncate(max);
    }
    value
}

/// Emits the record's snapshot to the observer and bumps the watch
/// version. Called under the registry lock.
fn publish_snapshot(
    state: &mut RegistryState,
    version: &tokio::sync::watch::Sender<u64>,
    index: usize,
) {
    let snapshot = state.records[index].snapshot();
    if let Some(observer) = &state.observer {
        observer.on_snapshot(&snapshot);
    }
    version.send_modify(|v| *v += 1);
}

/// A test-only pause inside the ownership-commit critical section
/// (production is unwired; mirrors the background dispatch hook).
#[cfg(test)]
#[derive(Debug, Default)]
pub struct CommitBoundaryHook {
    state: std::sync::Mutex<CommitHookState>,
    changed: std::sync::Condvar,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CommitHookState {
    #[default]
    Idle,
    Entered,
    Released,
}

#[cfg(test)]
impl CommitBoundaryHook {
    /// Blocks the commit section until [`Self::release`].
    pub fn wait(&self) {
        let mut state = self.state.lock().expect("subagent commit hook");
        *state = CommitHookState::Entered;
        self.changed.notify_all();
        while matches!(*state, CommitHookState::Entered) {
            state = self.changed.wait(state).expect("subagent commit hook");
        }
    }

    /// Waits until a commit section has entered the hook.
    pub fn wait_until_entered(&self) {
        let mut state = self.state.lock().expect("subagent commit hook");
        while matches!(*state, CommitHookState::Idle) {
            state = self.changed.wait(state).expect("subagent commit hook");
        }
    }

    /// Releases the paused commit section.
    pub fn release(&self) {
        let mut state = self.state.lock().expect("subagent commit hook");
        *state = CommitHookState::Released;
        self.changed.notify_all();
    }
}
