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
//! physical root    (one fresh spawn-incarnation namespace per child)
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

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};

use crate::events::types::SubagentOwnershipKind;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AgentId, ConversationId, SubagentId};
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::interaction::InteractionRef;
use crate::runtime::types::{CancellationReason, DurabilityGate};
use crate::runtime::workflow::WorkflowId;
use crate::runtime::{MonotonicClock, RuntimeClock};

use super::SubagentTerminalState;
use super::activity::{SubagentExecutionProfile, SubagentObservation};
use super::catalog::{NamedAgentDefinitionDigest, SubagentName};
use super::process::SubagentSpawnPlan;
use super::resolver::ResolvedSubagentSpec;
use super::resolver::SubagentExecutionProfileDigest;

mod agents;
mod control;
mod drain;
mod interactions;
mod observation;
mod ownership_commit;
mod preparation;
mod recovery_settlement;
mod settlement;
#[cfg(test)]
mod test_hooks;
pub use agents::{
    AgentControlError, AgentMessageAccepted, AgentSnapshot, AgentState, AgentWaitResult,
    DurableAgentAuthority, MAX_AGENT_LIST_LIMIT,
};
pub(crate) use control::GuidanceTicket;
pub use control::SubagentSteerError;
pub use interactions::ChildPendingHeadProof;
pub(crate) use interactions::{InteractionPublicationAuthority, SubagentInteractionSink};
pub(crate) use observation::{AgentObservationValue, SubagentActivitySink};
pub use observation::{
    SubagentListing, SubagentObserver, SubagentPhysicalSettlement, SubagentPublication,
    SubagentSettlement, SubagentSnapshot, SubagentState,
};
pub use preparation::{
    ActivationAdmission, AgentActivationOrigin, PreparedSubagent, SubagentAccepted,
    SubagentStartError, SubagentStartOutcome, SubagentStartSpec,
};
#[cfg(test)]
use test_hooks::{AllocationTestHook, ResumeCleanupHook};
#[cfg(test)]
pub(crate) use test_hooks::{
    CancellationBoundaryHook, CommitBoundaryHook, ControlHandoffHook, GateReleaseHook,
    ResumeTestGates, SteerAcknowledgementHook, TerminalAuthorityHook,
};

/// The highest lifecycle state of one subagent child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubagentLifecycle {
    Starting,
    /// Message admission is closed; already admitted input is draining.
    Stopping,
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
    /// The child process/control plane settled without a valid semantic
    /// terminal result, so the child outcome is unknown.
    Interrupted,
}

impl SubagentLifecycle {
    const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    const fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

/// The canonicalized terminal outcome awaiting publication.
/// The decision of the commit linearization point.
enum Decision {
    Accepted {
        started_at: DateTime<Utc>,
        deadline_at_millis: Option<u64>,
        committed: Box<crate::events::types::RuntimeEventEnvelope>,
    },
    RolledBack,
    Failed(SubagentStartError),
    WaitingForGoalIdle,
}

/// The canonicalized terminal outcome awaiting publication.
#[derive(Debug, Clone)]
struct TerminalCandidate {
    physical_settlement_proven: bool,
    state: TerminalState,
    /// The bounded result content (succeeded only).
    content: Option<String>,
    /// The parent-validated Workflow value, when this is a successful
    /// Workflow-owned child. Keeping the parsed value on the frozen terminal
    /// candidate lets a retry rebuild the exact compound durable transition.
    workflow_value: Option<serde_json::Value>,
    /// The bounded failure diagnostic (failed only).
    diagnostic: Option<String>,
    /// The cancellation detail (cancelled only).
    reason: Option<CancellationReason>,
    /// The publication timestamp, frozen at canonicalization so a bounded
    /// retry rebuilds the byte-identical draft and an ambiguous commit
    /// resolves as the idempotent correlation retry, never a conflict.
    timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalState {
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

/// The parent-side terminal protocol selected for a child.
///
/// `Normal` preserves the asynchronous `subagent` intrinsic's ordinary
/// parent-inbound result publication. `WorkflowOutput` routes the child's
/// validated JSON terminal content to the `WorkflowRuntime` boundary instead;
/// it never creates a normal parent `ToolResult` or inbound transcript fact.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SubagentTerminalMode {
    /// Publish the ordinary named-subagent result through its native path.
    #[default]
    Normal,
    /// Require the reserved `workflow_output` protocol with this frozen schema.
    WorkflowOutput {
        /// The output contract the child Agent Loop must satisfy.
        output_schema: serde_json::Value,
        /// The immutable Workflow identity owning this `AgentRun`.
        workflow_id: WorkflowId,
        /// The immutable Workflow invocation identity.
        run_id: crate::runtime::workflow::WorkflowRunId,
        /// The stable Workflow node identity owning this `AgentRun`.
        node_id: Box<crate::runtime::workflow::WorkflowNodeInstance>,
    },
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

/// The registry-known arbitration phase of one live parent-authored steer
/// ticket (Issue #193).
///
/// The phase is the record's explicit statement of *why* the ticket still
/// exists, distinguishing the two semantic classes the acknowledgement
/// window contains:
///
/// - a steer whose child decision is still unknown to the registry
///   ([`Self::AwaitingChildDecision`]); and
/// - a steer whose child-side `Accepted` answer has already reached the
///   registry but whose parent acknowledgement commit is still pending
///   ([`Self::ChildAcceptedPendingParentCommit`]).
///
/// The phase is advanced only by the steer's own caller, when the
/// driver-resolved child outcome proves the child durably accepted — under
/// this same arbitration mutex, so it linearizes against cancellation and
/// terminal authority. Cancellation and the terminal authority dispose of
/// tickets in either phase; the phase exists so the ownership model says
/// explicitly that an accepted-but-unacknowledged steer is a different
/// semantic object from an unresolved one, and that disposing of its
/// ticket is lifecycle cleanup, never a denial of the acceptance itself
/// (see [`SubagentRecord::steer_tickets`] and
/// [`SubagentRegistry::send_message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteerTicketPhase {
    /// Admitted; the child conversation's durable decision has not yet
    /// reached the registry. Terminal-before-answer refusal, cancellation
    /// refusal, and child refusal are all still possible.
    AwaitingChildDecision,
    /// The child's durable `Accepted` answer has reached the registry and
    /// message acknowledgement is still owed. Cancellation, settlement and
    /// caller abandonment can clean the ticket but cannot erase acceptance.
    ChildAcceptedPendingParentCommit,
}

/// One live parent-authored steer ticket (Issue #193): the registry-owned
/// admission/liveness/arbitration record of one admitted-but-not-yet-
/// committed steer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SteerTicket {
    /// The ticket's transport correlation identity — names which child
    /// acceptance answer belongs to which steer request; never a
    /// conversation identity, an inbound sequence, or a lifecycle fact.
    guidance_id: u64,
    /// The steer's registry-known arbitration phase (see
    /// [`SteerTicketPhase`]).
    phase: SteerTicketPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DelegateDelivery {
    NotSent,
    /// Write attempted, or replay cannot prove prior non-delivery.
    Started,
    /// Child acknowledged its canonical inbound commit.
    Accepted,
}

struct SubagentRecord {
    execution: Option<super::process::ExecutionObservation>,
    /// Proven process, nested-work and required resource settlement, independent of logical terminal.
    physical_settlement_proven: bool,
    delegate_delivery: DelegateDelivery,
    parent_agent_id: AgentId,
    subagent_id: SubagentId,
    child_agent_id: AgentId,
    child_conversation_id: ConversationId,
    origin: AgentActivationOrigin,
    agent: SubagentName,
    definition_digest: NamedAgentDefinitionDigest,
    /// The deterministic identity of the **effective execution profile** this
    /// child committed with (Issue #258): the definition plus whatever an
    /// authorized invocation override replaced.
    ///
    /// It is a durable execution fact, committed with ownership, so a
    /// recovery-projected record restores exactly the committed value and
    /// never recomputes it from the current catalog.
    profile_digest: SubagentExecutionProfileDigest,
    /// Immutable domain identity, including for non-executable historical records.
    ownership: SubagentOwnershipKind,
    /// Frozen live terminal protocol; never reconstructed for historical activations.
    terminal: Option<SubagentTerminalMode>,
    /// The invoking Session's execution binding frozen at creation.
    workspace: std::path::PathBuf,
    lifecycle: SubagentLifecycle,
    cancel_reason: Option<CancellationReason>,
    /// The live parent-authored steer tickets this record has admitted but
    /// not yet disposed of (Issue #193).
    ///
    /// This is the whole steer/cancel arbitration state, and it lives here —
    /// under the registry mutex — precisely because that mutex is the *one*
    /// authority that totally orders steer admission, cancellation-intent
    /// commit, steer commit, and terminal-authority commit. The vector
    /// holds one [`SteerTicket`] per live steer, and every ticket carries
    /// its explicit [`SteerTicketPhase`]: a steer whose child decision is
    /// still unknown to the registry is `AwaitingChildDecision`, while a
    /// steer whose child-side `Accepted` answer has already reached the
    /// registry — but whose parent acknowledgement commit is still
    /// pending — is `ChildAcceptedPendingParentCommit`. Transport ordering
    /// proves nothing here and is never consulted.
    ///
    /// # What the ticket records — and what it deliberately does not
    ///
    /// The ticket is the registry's **admission/liveness/arbitration
    /// record**: it proves that a steer was admitted against a `Running`
    /// child, gives caller abandonment and the cancellation and terminal
    /// authorities one bounded, mutex-linearized state to dispose of, and
    /// keeps arbitration state bounded by live steer callers rather than by
    /// child lifetime.
    ///
    /// The ticket does **not** store the child-side acceptance fact. That
    /// fact is committed durably in the child conversation and is carried
    /// across the acknowledgement window by the steer caller itself (the
    /// driver-resolved child outcome), because the terminal authority can
    /// win — and clean the registry lifecycle state — *before* the
    /// registry has ever observed the child's answer. Cleanup of ticket
    /// state can therefore never destroy the classification of an
    /// already-accepted steer: the classification is decided at the
    /// steer's own commit from the caller-held child outcome, which no
    /// registry cleanup touches (see [`SubagentRegistry::send_message`]).
    ///
    /// # Ticket ownership and disposal
    ///
    /// Every ticket has **one explicit owner** — the [`GuidanceTicket`]
    /// guard returned by `admit_guidance_locked` — and exactly
    /// one terminal disposition, so arbitration state is never abandoned:
    ///
    /// - completing or dropping the message future runs the guard's
    ///   `Drop`, which removes exactly its own ticket ([`GuidanceTicket`])
    ///   — so no abandoned caller leaves a ticket behind;
    /// - the cancellation linearization point in
    ///   [`SubagentRegistry::cancel`] removes every ticket (whichever
    ///   phase it is in); this cleanup cannot erase accepted child input;
    /// - the terminal-authority commit in
    ///   [`SubagentRegistry::settle_from_driver`] removes every remaining
    ///   ticket (whichever phase it is in), so a terminal record never
    ///   retains live steering arbitration state. Removing a
    ///   `ChildAcceptedPendingParentCommit` ticket is lifecycle cleanup
    ///   only: the accepted fact is preserved by the caller-held child
    ///   outcome, and the steer's later commit still reports it accepted
    ///   (Issue #193 architecture review blocker 1).
    ///
    /// All four disposal paths acquire this same mutex for a short
    /// in-memory section and never do async work. Because every ticket is
    /// removed by one of those four disposals — and the guard drop runs
    /// synchronously with the abandoned future — the vector cannot grow
    /// without bound while a child runs, and no ticket survives a child's
    /// terminal settlement.
    steer_tickets: Vec<SteerTicket>,
    /// The one live deadline task, owned by this record and aborted as soon
    /// as cancellation intent or terminal settlement becomes authoritative.
    deadline_task: Option<tokio::task::JoinHandle<()>>,
    /// The narrow cancellation handle into the driver task — never an OS
    /// process handle.
    control: Option<tokio::sync::mpsc::UnboundedSender<super::process::DriverCommand>>,
    /// The bounded terminal failure/cancellation diagnostic. A successful
    /// child's answer content never appears here: the durable terminal
    /// inbound publication is the one result channel (Issue #178).
    detail: Option<Arc<str>>,
    /// The latest live activity projection reported by the child (Issue
    /// #178). Observation-plane state only: never a lifecycle input.
    observation: SubagentObservation,
    /// The redacted execution profile frozen at child start (Issue #178).
    /// `None` only for recovery-projected records, whose frozen launch
    /// specification no longer exists in this process.
    profile: Option<Arc<SubagentExecutionProfile>>,
    /// The parent-validated Workflow output value of a successful
    /// Workflow-owned child: the live Workflow result channel, deliberately
    /// kept out of the observation snapshot.
    terminal_workflow_value: Option<serde_json::Value>,
    pending_terminal: Option<TerminalCandidate>,
    publication_abandoned: bool,
    notification: NotificationState,
    started_at: DateTime<Utc>,
}

impl SubagentRecord {
    /// Reconstruct one historical activation from committed ownership. Recovery
    /// grants neither a control channel nor proof of Delegate acceptance. The
    /// ownership domain alone selects mailbox notification semantics; a Workflow
    /// can never acquire durable Agent controls through this constructor.
    fn recovered(
        parent_agent_id: AgentId,
        evidence: &crate::runtime::recovery::SubagentEvidence,
    ) -> Result<Self, crate::durable::ConversationStoreError> {
        use crate::durable::ConversationStoreError;
        let agent = SubagentName::parse(&evidence.agent)
            .map_err(|error| ConversationStoreError::InvalidReference(error.to_string()))?;
        let definition_digest = serde_json::from_value(serde_json::Value::String(
            evidence.definition_digest.clone(),
        ))
        .map_err(|error| ConversationStoreError::InvalidReference(error.to_string()))?;
        Ok(Self {
            execution: None,
            physical_settlement_proven: false,
            delegate_delivery: DelegateDelivery::Started,
            parent_agent_id,
            subagent_id: evidence.subagent_id.clone(),
            child_agent_id: evidence.child_agent_id.clone(),
            child_conversation_id: evidence.child_conversation_id.clone(),
            origin: evidence.origin.clone(),
            agent,
            definition_digest,
            profile_digest: SubagentExecutionProfileDigest::from_committed_fact(
                evidence.profile_digest.clone(),
            ),
            ownership: evidence.ownership,
            terminal: None,
            workspace: evidence.workspace.clone(),
            lifecycle: SubagentLifecycle::Interrupted,
            cancel_reason: None,
            steer_tickets: Vec::new(),
            deadline_task: None,
            control: None,
            detail: None,
            observation: SubagentObservation::default(),
            profile: None,
            terminal_workflow_value: None,
            pending_terminal: None,
            publication_abandoned: false,
            notification: match evidence.ownership {
                SubagentOwnershipKind::Normal => NotificationState::Delivered,
                SubagentOwnershipKind::Workflow => NotificationState::None,
            },
            started_at: evidence.started_at,
        })
    }

    fn observation_state(&self) -> SubagentState {
        match self.lifecycle {
            SubagentLifecycle::Running => SubagentState::Running,
            SubagentLifecycle::Starting | SubagentLifecycle::Stopping => SubagentState::Stopping,
            SubagentLifecycle::Cancelling => SubagentState::Cancelling,
            SubagentLifecycle::PublishingTerminal => SubagentState::PublishingTerminal,
            SubagentLifecycle::Succeeded => SubagentState::Succeeded,
            SubagentLifecycle::Failed => SubagentState::Failed,
            SubagentLifecycle::Cancelled => SubagentState::Cancelled,
            SubagentLifecycle::Interrupted => SubagentState::Interrupted,
        }
    }

    fn snapshot(&self) -> SubagentSnapshot {
        let state = self.observation_state();
        SubagentSnapshot {
            ownership: self.ownership,
            parent_agent_id: self.parent_agent_id.clone(),
            subagent_id: self.subagent_id.clone(),
            child_agent_id: self.child_agent_id.clone(),
            child_conversation_id: self.child_conversation_id.clone(),
            origin: self.origin.clone(),
            agent: self.agent.as_str().to_owned(),
            definition_digest: self.definition_digest.as_str().to_owned(),
            profile_digest: self.profile_digest.as_str().to_owned(),
            workspace: self.workspace.clone(),
            state,
            // The reason is meaningful exactly while cancellation intent is
            // live or the child settled cancelled; a successful (Workflow)
            // settlement that raced an in-flight intent reports no stale
            // reason.
            cancel_reason: match self.lifecycle {
                SubagentLifecycle::Cancelling | SubagentLifecycle::Cancelled => self.cancel_reason,
                _ => None,
            },
            detail: self.detail.clone(),
            observation: self.observation.clone(),
            profile: self.profile.clone(),
            settlement: SubagentSettlement {
                publication: if self.publication_abandoned {
                    SubagentPublication::Abandoned
                } else if self.lifecycle.is_terminal() {
                    SubagentPublication::Committed
                } else {
                    SubagentPublication::Pending
                },
                physical: if self.physical_settlement_proven {
                    SubagentPhysicalSettlement::Proven
                } else {
                    SubagentPhysicalSettlement::Unproven
                },
            },
            started_at: self.started_at,
        }
    }
}

struct RegistryState {
    max_active: usize,
    /// Pins an idle Goal admission while its durable append runs off-lock.
    goal_idle_committing: bool,
    /// Registered staged children, ordered by their already allocated native
    /// identity ordinal. Notifications never confer eligibility.
    capacity_waiters: BTreeMap<u64, CancellationSignal>,
    /// Exact staging owners that reserved capacity while ownership durability commits off-lock.
    ownership_committing: BTreeMap<SubagentId, (CancellationSignal, Option<CancellationReason>)>,
    #[cfg(test)]
    capacity_wait_entered: Option<tokio::sync::oneshot::Sender<()>>,
    #[cfg(test)]
    capacity_wait_pause: Option<tokio::sync::oneshot::Receiver<()>>,
    #[cfg(test)]
    capacity_wait_finished: Option<tokio::sync::oneshot::Sender<()>>,
    next_ordinal: u64,
    next_response_id: u64,
    /// The transport correlation allocator of parent-authored guidance
    /// (Issue #193). It names which child acceptance answer belongs to which
    /// steer request and is never a conversation identity, an inbound
    /// sequence, or a lifecycle fact.
    next_guidance_id: u64,
    agents: BTreeMap<AgentId, agents::AgentRecord>,
    agent_observation_order: Vec<AgentId>,
    agent_observation_index: HashMap<AgentId, usize>,
    observation_generation: u64,
    observation_layouts: HashMap<AgentId, crate::runtime::read_layout::ObservationLayoutCache>,
    /// Complete recovered physical obligation set, including allocations with
    /// no Reserved event, activation record or resume reservation. Reconciler
    /// completion never releases these runtime owners; only exact proof does.
    recovery_pending: std::collections::BTreeSet<SubagentId>,
    recovery_unreserved: std::collections::BTreeMap<SubagentId, ConversationId>,
    recovery_inflight: std::collections::BTreeSet<SubagentId>,
    terminal_inflight: std::collections::BTreeSet<SubagentId>,
    #[cfg(test)]
    recovery_probe_hook: Option<Box<dyn FnOnce() + Send>>,
    #[cfg(test)]
    recovery_proof_wait: std::collections::BTreeSet<SubagentId>,
    records: Vec<SubagentRecord>,
    index: HashMap<SubagentId, usize>,
    /// Live routed interactions owned by child coordinators. This is a root
    /// projection cache only: it contains no waiter, settlement, or
    /// cancellation authority. The originating child coordinator remains the
    /// semantic owner of every entry.
    routed_interactions:
        HashMap<InteractionRef, crate::runtime_client::snapshot::RuntimeClientInteractionHeader>,
    routed_order: BTreeMap<u64, InteractionRef>,
    routed_direct: HashMap<InteractionRef, SubagentId>,
    routed_by_child: HashMap<ConversationId, BTreeMap<u64, InteractionRef>>,
    routed_generation: u64,
    next_routed_sequence: u64,
    observer: Option<Arc<dyn SubagentObserver>>,
    failure_sink: Option<Arc<dyn SubagentDurabilityFailureSink>>,
    /// The owning `ConversationRuntime`'s durability frontier (Issue #60):
    /// a new conversation-owned durable ownership commit must linearize
    /// against the runtime's `DurabilityFailed` commit on this shared gate.
    /// Installed by `ConversationRuntime::new` after the ownership
    /// transfer; a standalone registry has none and commits through the
    /// unbound-mailbox path.
    durability_gate: Option<Arc<DurabilityGate>>,
    #[cfg(test)]
    commit_hook: Option<Arc<CommitBoundaryHook>>,
    #[cfg(test)]
    control_handoff_hook: Option<Arc<ControlHandoffHook>>,
    #[cfg(test)]
    seal_boundary_hook: Option<Arc<CommitBoundaryHook>>,
    #[cfg(test)]
    gate_release_hook: Option<Arc<GateReleaseHook>>,
    /// Test seam: pauses the first live cancellation contender (deadline
    /// expiry or an explicit caller) immediately before it can acquire the
    /// registry mutex that commits `Running -> Cancelling`.
    #[cfg(test)]
    cancellation_boundary_hook: Option<Arc<CancellationBoundaryHook>>,
    /// Test seam: pauses the terminal settlement path immediately before it
    /// can acquire the registry mutex that creates the terminal candidate
    /// and commits `... -> PublishingTerminal`.
    #[cfg(test)]
    terminal_authority_hook: Option<Arc<TerminalAuthorityHook>>,
    /// Test seam: pauses one composed steer acknowledgement immediately
    /// after the child's durable decision has reached the steer caller and
    /// immediately before the registry commit that consumes the ticket.
    #[cfg(test)]
    steer_acknowledgement_hook: Option<Arc<SteerAcknowledgementHook>>,
    /// Test seam: one-shot completion latches for fired record-owned
    /// deadline tasks. A test registers a latch (by predictable child
    /// identity) before the child starts; the owning commit claims it when
    /// it creates the deadline task, and the latch resolves only after the
    /// fired deadline's cancellation call has fully returned.
    #[cfg(test)]
    deadline_completion: HashMap<SubagentId, tokio::sync::oneshot::Sender<()>>,
    /// Test seam: pre-staged children `prepare` consumes instead of
    /// spawning the real child binary.
    #[cfg(test)]
    staged_overrides: std::collections::VecDeque<super::process::StagedChild>,
    #[cfg(test)]
    prepared_policies: Vec<super::InheritedExecutionPolicy>,
    /// Signals the production allocation attempt; substitutes only process
    /// staging AFTER the real identity/incarnation reservation has completed.
    #[cfg(test)]
    allocation_test_hook: Option<AllocationTestHook>,
    #[cfg(test)]
    allocation_failure: Option<super::process::SpawnError>,
    #[cfg(test)]
    allocation_attempts: usize,
    #[cfg(test)]
    resume_test_gates: Option<ResumeTestGates>,
    #[cfg(test)]
    resume_owner_abort: Option<tokio::task::AbortHandle>,
    #[cfg(test)]
    reserved_commit_hook: Option<Arc<CommitBoundaryHook>>,
    #[cfg(test)]
    resume_cleanup_hook: Option<ResumeCleanupHook>,
    #[cfg(test)]
    authority_install_hook: Option<Arc<CommitBoundaryHook>>,
    #[cfg(test)]
    resume_preflight_hook: Option<Arc<CommitBoundaryHook>>,
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
    /// The monotonic clock used only for whole-child execution deadlines.
    /// The timer is created after ownership commits; this clock is separate
    /// from the UTC timestamp clock used by durable events.
    pub monotonic_clock: Arc<dyn MonotonicClock>,
    /// The process spawn plan.
    pub spawn: SubagentSpawnPlan,
    /// The invoking Session's authorized execution workspace binding. Every
    /// new child receives it; a durable Agent freezes it at creation and
    /// keeps it across activations. It is never a runtime-owned resource:
    /// no child allocates, settles or disposes it.
    pub workspace: std::path::PathBuf,
    /// The per-conversation concurrency bound.
    pub max_active: usize,
}

/// The sole logical owner and registry of a conversation's subagent
/// children.
///
/// Cheaply cloneable: every clone shares the one registry state, the same
/// contract as the background registry.
pub struct SubagentRegistry {
    recovery_reconciliation: tokio::sync::watch::Sender<bool>,
    config: SubagentRegistryConfig,
    identities: Arc<dyn crate::runtime::identity::UuidV7Generator>,
    state: Arc<Mutex<RegistryState>>,
    state_version: tokio::sync::watch::Sender<u64>,
    #[cfg(test)]
    pub(crate) interaction_published: Arc<tokio::sync::Notify>,
    /// An early root Runtime Client human-provider availability hint. Each
    /// child driver receives a reliable subscription for fast fail-closed
    /// behavior, but the publication authority below remains the admission
    /// decision and is not inferred from this cached value.
    provider_available: tokio::sync::watch::Sender<bool>,
    /// The root Runtime Client publication frontier. This is installed by a
    /// live root host before activation; without it, child publication fails
    /// closed even if a stale provider watch says `true`.
    publication_authority: Arc<Mutex<Option<Arc<dyn InteractionPublicationAuthority>>>>,
    upstream_interactions:
        Arc<Mutex<Option<Arc<dyn crate::runtime::interaction::InteractionRoute>>>>,
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
        let max_active = config.max_active;
        Self {
            recovery_reconciliation: tokio::sync::watch::Sender::new(true),
            config,
            identities: Arc::new(crate::runtime::identity::SystemUuidV7Generator),
            state: Arc::new(Mutex::new(RegistryState {
                max_active,
                goal_idle_committing: false,
                capacity_waiters: BTreeMap::new(),
                ownership_committing: BTreeMap::new(),
                #[cfg(test)]
                capacity_wait_entered: None,
                #[cfg(test)]
                capacity_wait_pause: None,
                #[cfg(test)]
                capacity_wait_finished: None,
                next_ordinal: 1,
                next_response_id: 1,
                next_guidance_id: 1,
                agents: BTreeMap::new(),
                agent_observation_order: Vec::new(),
                agent_observation_index: HashMap::new(),
                observation_generation: 0,
                observation_layouts: HashMap::new(),
                recovery_pending: std::collections::BTreeSet::new(),
                recovery_unreserved: std::collections::BTreeMap::new(),
                recovery_inflight: std::collections::BTreeSet::new(),
                terminal_inflight: std::collections::BTreeSet::new(),
                #[cfg(test)]
                recovery_probe_hook: None,
                #[cfg(test)]
                recovery_proof_wait: std::collections::BTreeSet::new(),
                records: Vec::new(),
                index: HashMap::new(),
                routed_interactions: HashMap::new(),
                routed_order: BTreeMap::new(),
                routed_direct: HashMap::new(),
                routed_by_child: HashMap::new(),
                routed_generation: 0,
                next_routed_sequence: 0,
                observer: None,
                failure_sink: None,
                durability_gate: None,
                #[cfg(test)]
                commit_hook: None,
                #[cfg(test)]
                control_handoff_hook: None,
                #[cfg(test)]
                seal_boundary_hook: None,
                #[cfg(test)]
                gate_release_hook: None,
                #[cfg(test)]
                cancellation_boundary_hook: None,
                #[cfg(test)]
                terminal_authority_hook: None,
                #[cfg(test)]
                steer_acknowledgement_hook: None,
                #[cfg(test)]
                deadline_completion: HashMap::new(),
                #[cfg(test)]
                staged_overrides: std::collections::VecDeque::new(),
                #[cfg(test)]
                prepared_policies: Vec::new(),
                #[cfg(test)]
                allocation_test_hook: None,
                #[cfg(test)]
                allocation_failure: None,
                #[cfg(test)]
                allocation_attempts: 0,
                #[cfg(test)]
                resume_test_gates: None,
                #[cfg(test)]
                resume_owner_abort: None,
                #[cfg(test)]
                reserved_commit_hook: None,
                #[cfg(test)]
                resume_cleanup_hook: None,
                #[cfg(test)]
                authority_install_hook: None,
                #[cfg(test)]
                resume_preflight_hook: None,
            })),
            state_version: tokio::sync::watch::Sender::new(0),
            #[cfg(test)]
            interaction_published: Arc::new(tokio::sync::Notify::new()),
            provider_available: tokio::sync::watch::channel(false).0,
            publication_authority: Arc::new(Mutex::new(None)),
            upstream_interactions: Arc::new(Mutex::new(None)),
        }
    }

    /// Installs the owning runtime's durability frontier (Issue #60).
    ///
    /// `ConversationRuntime::new` installs it after the ownership transfer;
    /// the runtime remains inactive until activation, so no ownership
    /// commit can race the installation. A standalone registry never has
    /// one.
    pub(crate) fn install_durability_gate(&self, gate: Arc<DurabilityGate>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.durability_gate = Some(gate);
    }

    /// Supplies allocation independently of ordering, including deterministic
    /// collision generators at the real preparation boundary.
    #[must_use]
    pub fn with_identity_generator(
        mut self,
        identities: Arc<dyn crate::runtime::identity::UuidV7Generator>,
    ) -> Self {
        self.identities = identities;
        self
    }

    /// The conversation this registry belongs to (construction ownership
    /// validation of the runtime that consumes it).
    #[must_use]
    pub(crate) fn conversation_id(&self) -> &ConversationId {
        &self.config.conversation_id
    }

    /// The parent (delegating) agent identity of this registry's domain.
    #[must_use]
    pub(crate) fn parent_agent_id(&self) -> &AgentId {
        &self.config.agent_id
    }

    /// Whether this registry's canonical mailbox is exactly the supplied
    /// mailbox: structural identity (same durable inbound capability and
    /// same process-local mailbox state), never a file-path comparison.
    #[must_use]
    pub(crate) fn shares_mailbox_domain(&self, other: &ConversationInboundMailbox) -> bool {
        self.config.mailbox.shares_domain_with(other)
    }

    /// Updates the shared admission limit under the native publication fence.
    /// Existing execution retains its separate immutable policy.
    pub(crate) fn publish_configuration(
        &self,
        config: &crate::local_runtime::config::CurrentRuntimeConfig,
    ) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.max_active = config.subagents.max_concurrent;
        self.state_version.send_modify(|version| *version += 1);
    }

    pub(crate) fn configuration_busy(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        !state.capacity_waiters.is_empty()
            || state.records.iter().any(|record| {
                record.lifecycle.is_active()
                    || matches!(record.lifecycle, SubagentLifecycle::PublishingTerminal)
            })
    }

    /// Whether the registry owns neither a child record nor its committing owner.
    ///
    /// A `ConversationRuntime` construction requires a pristine logical
    /// subagent plane: a registry with live children can never be silently
    /// adopted by a runtime that did not own their start.
    #[must_use]
    pub(crate) fn is_pristine(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.records.is_empty() && state.ownership_committing.is_empty()
    }

    /// Reseeds the ordinal sequence from the durable authority during
    /// startup recovery, so a recovered conversation never reissues an
    /// ordinal that already entered durable authority.
    pub fn restore_sequence_watermark(&self, highest_ordinal: u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.next_ordinal = state.next_ordinal.max(highest_ordinal + 1);
    }

    /// Install the observer and capture durable Agent owners at one registry cut.
    /// Finite activation allocation order never chooses a durable Agent's latest state.
    pub(crate) fn install_observer_and_agent_snapshots(
        &self,
        observer: Arc<dyn SubagentObserver>,
    ) -> (
        Vec<(AgentSnapshot, SubagentSnapshot)>,
        crate::runtime::identity::ObservationCollectionCut,
    ) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let snapshots: Vec<_> = state
            .agent_observation_order
            .iter()
            .rev()
            .take(32)
            .filter_map(|id| {
                let agent = Self::agent_snapshot_locked(&state, id)?;
                let activation =
                    state.records[*state.index.get(&agent.latest_activation)?].snapshot();
                Some((agent, activation))
            })
            .collect();
        let cut = Self::observation_cut_locked(&state);
        for (ordinal, (agent, activation)) in snapshots.iter().enumerate() {
            observer.on_snapshot(Some(agent), activation, cut, Some(ordinal as u64));
        }
        for record in state.records.iter().rev().take(32) {
            if record.ownership == SubagentOwnershipKind::Workflow {
                observer.on_snapshot(None, &record.snapshot(), cut, None);
            }
        }
        observer.on_interaction_head(&Self::pending_interaction_head_locked(&state));
        state.observer = Some(observer);
        (snapshots, cut)
    }

    /// Installs the durability-failure sink.
    pub fn install_failure_sink(&self, sink: Arc<dyn SubagentDurabilityFailureSink>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.failure_sink = Some(sink);
    }

    fn clone_for_task(&self) -> Self {
        Self {
            recovery_reconciliation: self.recovery_reconciliation.clone(),
            config: self.config.clone(),
            identities: Arc::clone(&self.identities),
            state: Arc::clone(&self.state),
            state_version: self.state_version.clone(),
            #[cfg(test)]
            interaction_published: Arc::clone(&self.interaction_published),
            provider_available: self.provider_available.clone(),
            publication_authority: Arc::clone(&self.publication_authority),
            upstream_interactions: Arc::clone(&self.upstream_interactions),
        }
    }
}

#[cfg(test)]
mod tests;
