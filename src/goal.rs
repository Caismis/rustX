//! Revisioned Goal authority. History and events are observations, never state.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::durable::{AcceptedInbound, ConversationStore, ConversationStoreError, InboundDraft};
use crate::runtime::identity::{AttemptId, MessageId};

/// Default number of driver-admitted autonomous rounds.
pub const DEFAULT_ROUND_BUDGET: u32 = 10;
/// Hard native autonomous-round limit.
pub const MAX_ROUND_BUDGET: u32 = 100;
/// UTF-8 byte bound for user task data.
pub const MAX_OBJECTIVE_BYTES: usize = 8192;

/// Exact observation required by every mutation of an existing Goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalRef {
    pub id: String,
    pub revision: u64,
}

/// Durable phase, independent of automatic continuation activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalPhase {
    Active,
    Paused,
    Blocked,
    Complete,
}

/// Trusted origin supplied by runtime, never by model arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalOrigin {
    HumanAttempt {
        message_id: MessageId,
        attempt_id: AttemptId,
    },
    RuntimeControl,
}

/// Authoritative bounded durable record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalSnapshot {
    pub reference: GoalRef,
    pub objective: String,
    pub phase: GoalPhase,
    pub blocked_reason: Option<String>,
    pub autonomous_round_budget: u32,
    pub autonomous_rounds_consumed: u32,
    pub origin: GoalOrigin,
    pub last_round_message_id: Option<MessageId>,
}

/// The current read model; observing it never arms continuation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalView {
    pub current: Option<GoalSnapshot>,
    pub armed: bool,
}

/// Explicit user/control mutations. Model adapters expose only Block/Complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalMutation {
    Pause,
    Resume,
    Block { reason: String },
    Complete,
    Edit { objective: String },
    Budget { rounds: u32 },
}

/// Typed Runtime Client control; an existing-state mutation always names its observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalControl {
    Show,
    Create {
        objective: String,
        #[serde(default = "default_control_budget")]
        budget: u32,
    },
    Mutate {
        expected: GoalRef,
        mutation: GoalMutation,
    },
}

fn default_control_budget() -> u32 {
    DEFAULT_ROUND_BUDGET
}

/// Semantic refusal, including the bounded current observation for stale CAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRejection {
    pub reason: String,
    pub current: Option<GoalSnapshot>,
}

pub type GoalResult = Result<GoalSnapshot, Box<GoalRejection>>;

/// Durable transition inputs; admission is deliberately a separate store seam.
pub enum GoalWrite {
    Create {
        objective: String,
        budget: u32,
        origin: GoalOrigin,
    },
    Mutate {
        expected: GoalRef,
        mutation: GoalMutation,
    },
}

fn reject(reason: &str, current: Option<&GoalSnapshot>) -> Box<GoalRejection> {
    Box::new(GoalRejection {
        reason: reason.to_owned(),
        current: current.cloned(),
    })
}

pub(crate) fn validate_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_OBJECTIVE_BYTES
}

/// Pure state machine, called within the durable write transaction.
pub(crate) fn transition(current: Option<&GoalSnapshot>, write: GoalWrite) -> GoalResult {
    match write {
        GoalWrite::Create {
            objective,
            budget,
            origin,
        } => {
            if current
                .as_ref()
                .is_some_and(|g| g.phase != GoalPhase::Complete)
            {
                return Err(reject("An unfinished Goal already exists", current));
            }
            if !validate_text(&objective) || !(1..=MAX_ROUND_BUDGET).contains(&budget) {
                return Err(reject(
                    "Objective or autonomous-round budget is out of bounds",
                    current,
                ));
            }
            let revision = current
                .as_ref()
                .map_or(Some(1), |g| g.reference.revision.checked_add(1))
                .ok_or_else(|| reject("Goal revision exhausted", current))?;
            Ok(GoalSnapshot {
                reference: GoalRef {
                    id: format!("goal-{revision}"),
                    revision,
                },
                objective,
                phase: GoalPhase::Active,
                blocked_reason: None,
                autonomous_round_budget: budget,
                autonomous_rounds_consumed: 0,
                origin,
                last_round_message_id: None,
            })
        }
        GoalWrite::Mutate { expected, mutation } => {
            let Some(mut goal) = current.cloned() else {
                return Err(reject("No current Goal", current));
            };
            if goal.reference != expected {
                return Err(reject(
                    "Stale GoalRef; observe current state before trying again",
                    current,
                ));
            }
            if goal.phase == GoalPhase::Complete {
                return Err(reject("Complete is terminal", current));
            }
            match mutation {
                GoalMutation::Pause if goal.phase == GoalPhase::Active => {
                    goal.phase = GoalPhase::Paused;
                }
                GoalMutation::Resume => {
                    // Active + disarmed is explicitly re-armable with CAS.
                    goal.phase = GoalPhase::Active;
                    goal.blocked_reason = None;
                }
                GoalMutation::Block { reason }
                    if goal.phase == GoalPhase::Active && validate_text(&reason) =>
                {
                    goal.phase = GoalPhase::Blocked;
                    goal.blocked_reason = Some(reason);
                }
                GoalMutation::Complete if goal.phase == GoalPhase::Active => {
                    goal.phase = GoalPhase::Complete;
                }
                GoalMutation::Edit { objective } if validate_text(&objective) => {
                    goal.objective = objective;
                }
                GoalMutation::Budget { rounds }
                    if (1..=MAX_ROUND_BUDGET).contains(&rounds)
                        && rounds >= goal.autonomous_rounds_consumed =>
                {
                    goal.autonomous_round_budget = rounds;
                }
                _ => return Err(reject("Invalid Goal transition or value", current)),
            }
            goal.reference.revision = goal
                .reference
                .revision
                .checked_add(1)
                .ok_or_else(|| reject("Goal revision exhausted", current))?;
            Ok(goal)
        }
    }
}

/// Bounded audit vocabulary; no objective/history and no recovery authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalFact {
    Written {
        previous: Option<GoalRef>,
        current: GoalRef,
        phase: GoalPhase,
        change: GoalChange,
    },
    RoundAdmitted {
        previous: GoalRef,
        current: GoalRef,
        round: u32,
        message_id: MessageId,
    },
    ActivationChanged {
        armed: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalChange {
    Create,
    Pause,
    Resume,
    Block,
    Complete,
    Edit,
    Budget,
}

impl GoalWrite {
    pub(crate) fn change(&self) -> GoalChange {
        match self {
            Self::Create { .. } => GoalChange::Create,
            Self::Mutate { mutation, .. } => match mutation {
                GoalMutation::Pause => GoalChange::Pause,
                GoalMutation::Resume => GoalChange::Resume,
                GoalMutation::Block { .. } => GoalChange::Block,
                GoalMutation::Complete => GoalChange::Complete,
                GoalMutation::Edit { .. } => GoalChange::Edit,
                GoalMutation::Budget { .. } => GoalChange::Budget,
            },
        }
    }
}

pub(crate) fn journal_envelope(
    conversation_id: &crate::runtime::identity::ConversationId,
    fact: GoalFact,
) -> crate::events::types::RuntimeEventEnvelope {
    crate::events::types::RuntimeEventEnvelope {
        schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
        event_id: crate::runtime::identity::EventId::new(""),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp: chrono::Utc::now(),
        event: crate::events::types::RuntimeEvent::Goal { fact },
    }
}

/// Foreground-only authority borrowed from the owning `AgentExecution`.
#[derive(Clone)]
pub(crate) struct GoalToolContext<'a> {
    pub domain: GoalDomain,
    pub creation_authorization: &'a Mutex<Option<GoalOrigin>>,
    pub mailbox: crate::runtime::inbound::ConversationInboundMailbox,
}

/// Leaf observer called under the domain mutex; never acquires coordinator locks.
pub(crate) trait GoalObserver: Send + Sync {
    fn changed(&self, view: GoalView);
    fn disarmed(&self);
}

/// One durable state owner, with separate process-local activation.
/// The mutex orders disarm/mutation against a driver's local reservation.
#[derive(Clone)]
pub struct GoalDomain {
    inner: Arc<GoalDomainInner>,
}

struct GoalDomainInner {
    store: Arc<dyn ConversationStore>,
    armed: Mutex<bool>,
    observer: std::sync::OnceLock<Arc<dyn GoalObserver>>,
    #[cfg(test)]
    before_tool_commit: crate::runtime::conversation_runtime::Gate,
    #[cfg(test)]
    during_write: crate::runtime::conversation_runtime::Gate,
    wake: Arc<tokio::sync::Notify>,
}

impl GoalDomain {
    pub(crate) fn new(store: Arc<dyn ConversationStore>, wake: Arc<tokio::sync::Notify>) -> Self {
        Self {
            inner: Arc::new(GoalDomainInner {
                store,
                armed: Mutex::new(false),
                observer: std::sync::OnceLock::new(),
                #[cfg(test)]
                before_tool_commit: crate::runtime::conversation_runtime::Gate::default(),
                #[cfg(test)]
                during_write: crate::runtime::conversation_runtime::Gate::default(),
                wake,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn tool_commit_gate(
        &self,
        inside: bool,
    ) -> &crate::runtime::conversation_runtime::Gate {
        if inside {
            &self.inner.during_write
        } else {
            &self.inner.before_tool_commit
        }
    }

    pub(crate) fn write_from_tool(
        &self,
        mailbox: &crate::runtime::inbound::ConversationInboundMailbox,
        write: GoalWrite,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Result<GoalResult, ConversationStoreError>, crate::runtime::inbound::MailboxError>
    {
        #[cfg(test)]
        self.inner.before_tool_commit.enter();
        mailbox.with_running_commit(|| self.write_if(write, || !cancellation.is_cancelled()))
    }

    /// Reads phase and activation without changing either.
    ///
    /// # Errors
    /// Returns a durable read failure.
    ///
    /// # Panics
    /// Panics if the activation lock was poisoned.
    pub fn view(&self) -> Result<GoalView, ConversationStoreError> {
        let armed = self
            .inner
            .armed
            .lock()
            .expect("Goal activation lock poisoned");
        Ok(GoalView {
            current: self.inner.store.load_goal()?,
            armed: *armed,
        })
    }

    /// Installed at the inactive `ConversationRuntime` bootstrap cut. Model
    /// tools and controls are lifecycle-gated, so no production write can
    /// intervene between the bootstrap read and observer installation.
    pub(crate) fn install_observer(&self, observer: Arc<dyn GoalObserver>) {
        assert!(
            self.inner.observer.set(observer).is_ok(),
            "Goal observer already installed"
        );
    }

    // Process-local activation facts are best-effort audit, after the winning
    // mutex transition. Failure cannot undo activation or hide its live view.
    fn activation_fact(&self, armed: bool) {
        let _ = self.inner.store.append_event(journal_envelope(
            self.inner.store.conversation_id(),
            GoalFact::ActivationChanged { armed },
        ));
    }

    pub(crate) fn write(&self, write: GoalWrite) -> Result<GoalResult, ConversationStoreError> {
        self.write_if(write, || true)
    }

    pub(crate) fn write_if(
        &self,
        write: GoalWrite,
        allowed: impl FnOnce() -> bool,
    ) -> Result<GoalResult, ConversationStoreError> {
        let mut armed = self
            .inner
            .armed
            .lock()
            .expect("Goal activation lock poisoned");
        if !allowed() {
            return Ok(Err(reject(
                "Goal command cancelled",
                self.inner.store.load_goal()?.as_ref(),
            )));
        }
        #[cfg(test)]
        self.inner.during_write.enter();
        let rearm = matches!(
            write,
            GoalWrite::Create { .. }
                | GoalWrite::Mutate {
                    mutation: GoalMutation::Resume,
                    ..
                }
        );
        let result = self.inner.store.write_goal(write)?;
        if let Ok(goal) = &result {
            let was_armed = *armed;
            if goal.phase != GoalPhase::Active {
                *armed = false;
            } else if rearm {
                *armed = true;
            }
            if was_armed != *armed {
                self.activation_fact(*armed);
            }
            if let Some(observer) = self.inner.observer.get() {
                observer.changed(GoalView {
                    current: Some(goal.clone()),
                    armed: *armed,
                });
            }
            self.inner.wake.notify_one();
        }
        Ok(result)
    }

    pub(crate) fn disarm(&self) {
        let mut armed = self
            .inner
            .armed
            .lock()
            .expect("Goal activation lock poisoned");
        if *armed {
            *armed = false;
            self.activation_fact(false);
            if let Some(observer) = self.inner.observer.get() {
                observer.disarmed();
            }
        }
    }

    pub(crate) fn reserve_and_accept(
        &self,
        draft: impl FnOnce(GoalRef) -> InboundDraft,
    ) -> Result<Option<AcceptedInbound>, ConversationStoreError> {
        let armed = self
            .inner
            .armed
            .lock()
            .expect("Goal activation lock poisoned");
        if !*armed {
            return Ok(None);
        }
        let Some(mut goal) = self.inner.store.load_goal()? else {
            return Ok(None);
        };
        if goal.phase != GoalPhase::Active
            || goal.autonomous_rounds_consumed >= goal.autonomous_round_budget
        {
            return Ok(None);
        }
        let expected = goal.reference.clone();
        let accepted = self
            .inner
            .store
            .accept_goal_round(&expected, draft(expected.clone()))?;
        if let Some(accepted) = &accepted {
            // Exact result of the atomic CAS under this domain mutex. No
            // fallible post-commit read and no competing domain write.
            goal.reference.revision += 1;
            goal.autonomous_rounds_consumed += 1;
            goal.last_round_message_id = Some(accepted.message_id.clone());
            if let Some(observer) = self.inner.observer.get() {
                observer.changed(GoalView {
                    current: Some(goal),
                    armed: *armed,
                });
            }
        }
        Ok(accepted)
    }
}

/// Synchronous coordinator participant, with no queue or task of its own.
pub(crate) struct GoalRoundDriver;

impl GoalRoundDriver {
    pub(crate) fn request(
        domain: &GoalDomain,
        mailbox: &crate::runtime::inbound::ConversationInboundMailbox,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, crate::runtime::inbound::MailboxError> {
        mailbox.accept_goal_round(domain, timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::SqliteConversationStore;
    use crate::message::{InboundKind, UserSource};
    use crate::runtime::identity::ConversationId;

    fn fixture() -> (Arc<SqliteConversationStore>, GoalDomain) {
        let store =
            Arc::new(SqliteConversationStore::in_memory(ConversationId::new("goal-test")).unwrap());
        let domain = GoalDomain::new(store.clone(), Arc::new(tokio::sync::Notify::new()));
        (store, domain)
    }

    fn create(domain: &GoalDomain) -> GoalSnapshot {
        domain
            .write(GoalWrite::Create {
                objective: "Keep working until verified".to_owned(),
                budget: 2,
                origin: GoalOrigin::RuntimeControl,
            })
            .unwrap()
            .unwrap()
    }

    fn draft(reference: GoalRef) -> InboundDraft {
        InboundDraft {
            message_id: None,
            source: UserSource::Runtime,
            kind: InboundKind::GoalContinuation(reference),
            content: vec![crate::message::UserContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "continue".to_owned(),
                },
            )],
            timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            correlation: None,
        }
    }

    #[test]
    fn goal84_cas_state_machine_revision_and_terminal_authority() {
        let (store, domain) = fixture();
        assert_eq!(
            domain.view().unwrap(),
            GoalView {
                current: None,
                armed: false
            }
        );
        let mut goal = create(&domain);
        assert_eq!(goal.reference.revision, 1);
        assert_eq!(goal.autonomous_rounds_consumed, 0);
        assert!(domain.view().unwrap().armed);
        assert!(
            domain
                .write(GoalWrite::Create {
                    objective: "second".into(),
                    budget: 10,
                    origin: GoalOrigin::RuntimeControl
                })
                .unwrap()
                .is_err()
        );
        let stale = goal.reference.clone();
        for mutation in [
            GoalMutation::Pause,
            GoalMutation::Edit {
                objective: "new objective".into(),
            },
            GoalMutation::Budget { rounds: 3 },
            GoalMutation::Resume,
            GoalMutation::Block {
                reason: "need access".into(),
            },
            GoalMutation::Resume,
            GoalMutation::Complete,
        ] {
            let before = goal.reference.revision;
            goal = domain
                .write(GoalWrite::Mutate {
                    expected: goal.reference.clone(),
                    mutation,
                })
                .unwrap()
                .unwrap();
            assert_eq!(goal.reference.revision, before + 1);
            assert_eq!(
                domain.view().unwrap().armed,
                goal.phase == GoalPhase::Active
            );
            let rejected = domain
                .write(GoalWrite::Mutate {
                    expected: stale.clone(),
                    mutation: GoalMutation::Complete,
                })
                .unwrap()
                .unwrap_err();
            assert_eq!(rejected.current, Some(goal.clone()));
        }
        for mutation in [
            GoalMutation::Resume,
            GoalMutation::Complete,
            GoalMutation::Pause,
            GoalMutation::Edit {
                objective: "rewrite".into(),
            },
        ] {
            assert!(
                domain
                    .write(GoalWrite::Mutate {
                        expected: goal.reference.clone(),
                        mutation
                    })
                    .unwrap()
                    .is_err()
            );
        }
        assert_eq!(store.load_goal().unwrap(), Some(goal.clone()));
        assert_ne!(create(&domain).reference.id, goal.reference.id);
    }

    #[test]
    fn goal84_journal_failure_rolls_back_durable_write_but_not_activation() {
        let (store, domain) = fixture();
        store.arm_fail_event_times(1);
        assert!(
            domain
                .write(GoalWrite::Create {
                    objective: "Must not leak into repeated facts".into(),
                    budget: 2,
                    origin: GoalOrigin::RuntimeControl,
                })
                .is_err()
        );
        assert_eq!(
            domain.view().unwrap(),
            GoalView {
                current: None,
                armed: false
            }
        );
        assert!(store.read_events(None, 64).unwrap().events.is_empty());
        let goal = create(&domain);
        let before = store.read_events(None, 64).unwrap().events;
        store.arm_fail_event_times(1);
        assert!(
            domain
                .write(GoalWrite::Mutate {
                    expected: goal.reference.clone(),
                    mutation: GoalMutation::Complete
                })
                .is_err()
        );
        assert_eq!(store.load_goal().unwrap(), Some(goal));
        assert_eq!(store.read_events(None, 64).unwrap().events, before);
        store.arm_fail_event_times(1);
        domain.disarm();
        assert!(
            !domain.view().unwrap().armed,
            "audit failure cannot undo a won disarm"
        );
        assert_eq!(store.read_events(None, 64).unwrap().events, before);
        assert!(
            !serde_json::to_string(&before)
                .unwrap()
                .contains("Keep working")
        );
    }

    #[test]
    fn goal84_atomic_round_frontier_failure_recovery_and_no_refund() {
        let (store, domain) = fixture();
        let goal = create(&domain);
        let facts_before = store.read_events(None, 64).unwrap().events;
        store.arm_fail_accept_times(1);
        assert!(domain.reserve_and_accept(draft).is_err());
        assert_eq!(store.read_events(None, 64).unwrap().events, facts_before);
        assert!(store.load_pending().unwrap().is_empty());
        assert_eq!(store.load_goal().unwrap(), Some(goal));
        let accepted = domain.reserve_and_accept(draft).unwrap().unwrap();
        let committed = store.load_goal().unwrap().unwrap();
        let facts = store.read_events(None, 64).unwrap().events;
        assert_eq!(facts.len(), facts_before.len() + 1);
        assert!(matches!(&facts.last().unwrap().event,
            crate::events::types::RuntimeEvent::Goal { fact: GoalFact::RoundAdmitted { round: 1, message_id, current, .. } }
                if message_id == &accepted.message_id && current == &committed.reference));
        assert_eq!(committed.reference.revision, 2);
        assert_eq!(committed.autonomous_rounds_consumed, 1);
        assert_eq!(committed.last_round_message_id, Some(accepted.message_id));
        domain.disarm();
        assert_eq!(store.load_goal().unwrap(), Some(committed.clone()));
        let recovered = GoalDomain::new(store.clone(), Arc::new(tokio::sync::Notify::new()));
        assert!(!recovered.view().unwrap().armed);
        assert!(recovered.reserve_and_accept(draft).unwrap().is_none());
        assert_eq!(store.load_pending().unwrap().len(), 1);
        assert_eq!(store.load_goal().unwrap(), Some(committed));
    }

    #[test]
    fn goal84_partial_or_altered_goal_tools_cannot_masquerade_as_disabled_composition() {
        use crate::extensions::ExtensionToolPlaneShape;
        let absent = crate::extensions::NativeAgentExtensions::none().expected_tool_plane();
        let complete = crate::extensions::NativeAgentExtensions::none()
            .and_goal()
            .expected_tool_plane();
        for count in 0..=3 {
            let mut registry = crate::tools::executor::ToolRegistry::new();
            for tool in crate::tools::native::goal_tool_registrations()
                .into_iter()
                .take(count)
            {
                registry.register(tool.definition, tool.executor).unwrap();
            }
            let shape = ExtensionToolPlaneShape::of_published_registry(&registry);
            assert_eq!(shape == absent, count == 0);
            assert_eq!(shape == complete, count == 3);
        }
        let mut tool = crate::tools::native::goal_tool_registrations().remove(0);
        tool.definition.description = "forged command".into();
        let mut registry = crate::tools::executor::ToolRegistry::new();
        registry.register(tool.definition, tool.executor).unwrap();
        let shape = ExtensionToolPlaneShape::of_published_registry(&registry);
        assert_ne!(shape, absent);
        assert_ne!(shape, complete);
    }

    #[test]
    fn goal84_commands_are_rejected_on_every_ordinary_tool_selection_surface() {
        use crate::capabilities::AgentActivation;
        for name in crate::tools::native::GOAL_TOOL_NAMES {
            assert_eq!(
                crate::capabilities::extension_provided_tool(name),
                Some("goal")
            );
            for policy in [
                AgentActivation {
                    profile: crate::local_runtime::config::AgentProfileDocument {
                        tools: crate::capabilities::selection::ToolSelectionDocument {
                            builtin: vec![name.into()],
                            sources: std::collections::BTreeMap::default(),
                        },
                        ..crate::local_runtime::config::builtin_root_profile()
                    },
                    ..AgentActivation::default()
                },
                AgentActivation {
                    tools: Some(vec![name.into()]),
                    ..AgentActivation::default()
                },
                AgentActivation {
                    exclude_tools: vec![name.into()],
                    ..AgentActivation::default()
                },
            ] {
                assert!(
                    policy
                        .validate()
                        .unwrap_err()
                        .contains("extensions.goal.enabled")
                );
            }
        }
    }

    #[test]
    fn goal84_human_and_goal_acceptance_share_a_deterministic_durable_order() {
        for human_first in [false, true] {
            let (store, domain) = fixture();
            let goal = create(&domain);
            let mut human = draft(goal.reference.clone());
            human.source = UserSource::Human;
            human.kind = InboundKind::Message;
            let (release, wait) = std::sync::mpsc::channel();
            let other = store.clone();
            let thread = std::thread::spawn(move || {
                wait.recv().unwrap();
                if human_first {
                    assert!(
                        other
                            .accept_goal_round(&goal.reference, draft(goal.reference.clone()))
                            .unwrap()
                            .is_none()
                    );
                } else {
                    other.accept_inbound(human).unwrap();
                }
            });
            if human_first {
                let mut human = draft(store.load_goal().unwrap().unwrap().reference);
                human.source = UserSource::Human;
                human.kind = InboundKind::Message;
                store.accept_inbound(human).unwrap();
            } else {
                assert!(domain.reserve_and_accept(draft).unwrap().is_some());
            }
            release.send(()).unwrap();
            thread.join().unwrap();
            let current = store.load_goal().unwrap().unwrap();
            assert_eq!(current.autonomous_rounds_consumed, u32::from(!human_first));
            assert_eq!(
                store.load_pending().unwrap().len(),
                if human_first { 1 } else { 2 }
            );
        }
    }

    #[test]
    fn goal84_human_before_frontier_and_single_concurrent_reservation() {
        let (store, domain) = fixture();
        let goal = create(&domain);
        let mut human = draft(goal.reference.clone());
        human.source = UserSource::Human;
        human.kind = InboundKind::Message;
        store.accept_inbound(human).unwrap();
        assert!(domain.reserve_and_accept(draft).unwrap().is_none());
        assert_eq!(store.load_goal().unwrap(), Some(goal));

        let (store, domain) = fixture();
        create(&domain);
        let gate = Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let domain = domain.clone();
            let gate = gate.clone();
            threads.push(std::thread::spawn(move || {
                gate.wait();
                domain.reserve_and_accept(draft).unwrap().is_some()
            }));
        }
        gate.wait();
        let admitted = threads
            .into_iter()
            .map(|thread| usize::from(thread.join().unwrap()))
            .sum::<usize>();
        assert_eq!(admitted, 1);
        assert_eq!(store.load_pending().unwrap().len(), 1);
        assert_eq!(
            store
                .load_goal()
                .unwrap()
                .unwrap()
                .autonomous_rounds_consumed,
            1
        );
    }

    #[test]
    fn goal84_revision_races_have_one_durable_winner() {
        for mutation in [
            GoalMutation::Pause,
            GoalMutation::Block {
                reason: "access".into(),
            },
            GoalMutation::Complete,
            GoalMutation::Edit {
                objective: "edited".into(),
            },
            GoalMutation::Budget { rounds: 3 },
        ] {
            for mutation_first in [false, true] {
                let (store, domain) = fixture();
                let goal = create(&domain);
                let expected = goal.reference.clone();
                let (release, wait) = std::sync::mpsc::channel();
                let other = store.clone();
                let operation = GoalWrite::Mutate {
                    expected: expected.clone(),
                    mutation: mutation.clone(),
                };
                let thread = std::thread::spawn(move || {
                    wait.recv().unwrap();
                    if mutation_first {
                        other
                            .accept_goal_round(&expected, draft(expected.clone()))
                            .unwrap()
                            .is_some()
                    } else {
                        other.write_goal(operation).unwrap().is_ok()
                    }
                });
                if mutation_first {
                    domain
                        .write(GoalWrite::Mutate {
                            expected: goal.reference.clone(),
                            mutation: mutation.clone(),
                        })
                        .unwrap()
                        .unwrap();
                } else {
                    domain.reserve_and_accept(draft).unwrap().unwrap();
                }
                release.send(()).unwrap();
                assert!(
                    !thread.join().unwrap(),
                    "the later stale contender must lose"
                );
                let current = store.load_goal().unwrap().unwrap();
                assert_eq!(current.reference.revision, 2);
                assert_eq!(
                    current.autonomous_rounds_consumed,
                    u32::from(!mutation_first)
                );
                assert_eq!(
                    store.load_pending().unwrap().len(),
                    usize::from(!mutation_first)
                );
            }
        }
    }

    #[test]
    fn goal84_model_schema_is_stable_narrow_and_source_cannot_be_spoofed() {
        let (store, domain) = fixture();
        let baseline = crate::tools::native::goal_tool_registrations()
            .into_iter()
            .map(|r| r.definition)
            .collect::<Vec<_>>();
        let mut goal = create(&domain);
        for mutation in [
            GoalMutation::Pause,
            GoalMutation::Resume,
            GoalMutation::Block {
                reason: "access".into(),
            },
            GoalMutation::Resume,
            GoalMutation::Complete,
        ] {
            goal = domain
                .write(GoalWrite::Mutate {
                    expected: goal.reference,
                    mutation,
                })
                .unwrap()
                .unwrap();
            assert_eq!(
                baseline,
                crate::tools::native::goal_tool_registrations()
                    .into_iter()
                    .map(|r| r.definition)
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(
            baseline.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["get_goal", "create_goal", "update_goal"]
        );
        let schema = serde_json::to_value(&baseline[2].input_schema).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        for action in ["pause", "resume", "edit", "budget"] {
            assert!(!validator.is_valid(
                &serde_json::json!({"action": action, "expected": goal.reference, "rounds": 100})
            ));
        }
        let schema = serde_json::to_value(&baseline[1].input_schema).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(
            !validator
                .is_valid(&serde_json::json!({"objective": "task", "origin": "forged-human"}))
        );
        assert!(
            baseline[1]
                .description
                .contains("Do not create a Goal merely")
        );
        assert!(baseline[1].description.contains("need not type /goal"));
        assert_eq!(
            store
                .load_goal()
                .unwrap()
                .unwrap()
                .autonomous_rounds_consumed,
            0
        );
    }

    #[test]
    fn goal84_closed_vocabulary_is_opt_in_and_children_are_refused() {
        use crate::extensions::*;
        assert!(
            NativeAgentExtensionsDocument::default()
                .resolve()
                .goal()
                .is_none()
        );
        let document: NativeAgentExtensionsDocument =
            serde_json::from_value(serde_json::json!({"goal": {"enabled": true}})).unwrap();
        let frozen = document.resolve();
        assert!(frozen.goal().is_some());
        assert_eq!(unsupported_child_scope(&frozen).unwrap().extension, "goal");
        let selected = NativeAgentExtensionSelection::of(&frozen);
        assert_eq!(selected.resolve(), frozen);
        assert_ne!(
            frozen.effective_digest_framing(),
            NativeAgentExtensionsDocument::default()
                .resolve()
                .effective_digest_framing()
        );
        assert_eq!(
            unsupported_child_scope(&selected.resolve())
                .unwrap()
                .extension,
            "goal"
        );
    }

    #[test]
    fn goal84_file_reopen_uses_domain_even_when_goal_journal_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conversation.sqlite");
        let id = ConversationId::new("persist-goal");
        let original = {
            let store = Arc::new(SqliteConversationStore::open(id.clone(), &path).unwrap());
            let domain = GoalDomain::new(store.clone(), Arc::new(tokio::sync::Notify::new()));
            let goal = create(&domain);
            let facts = store.read_events(None, 64).unwrap().events;
            assert!(facts.iter().any(|event| matches!(
                event.event,
                crate::events::types::RuntimeEvent::Goal {
                    fact: GoalFact::Written {
                        change: GoalChange::Create,
                        ..
                    }
                }
            )));
            assert!(facts.iter().any(|event| matches!(
                event.event,
                crate::events::types::RuntimeEvent::Goal {
                    fact: GoalFact::ActivationChanged { armed: true }
                }
            )));
            // Arm evidence exists, yet recovering below must start disarmed.
            let recovered = GoalDomain::new(store.clone(), Arc::new(tokio::sync::Notify::new()));
            assert_eq!(
                recovered.view().unwrap(),
                GoalView {
                    current: Some(goal.clone()),
                    armed: false
                }
            );
            goal
        };
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("DELETE FROM events", [])
            .unwrap();
        let disabled_store = Arc::new(SqliteConversationStore::open(id, &path).unwrap());
        assert_eq!(disabled_store.load_goal().unwrap(), Some(original.clone()));
        let enabled = GoalDomain::new(disabled_store, Arc::new(tokio::sync::Notify::new()));
        assert_eq!(
            enabled.view().unwrap(),
            GoalView {
                current: Some(original),
                armed: false
            }
        );
    }
}
