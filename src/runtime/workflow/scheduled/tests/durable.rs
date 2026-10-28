//! The durable/native ownership seam, using real compilation and SQLite.
#![allow(clippy::too_many_lines)] // Real SQLite interruption/proof fixtures keep each transaction scenario together.
use super::*;
use crate::durable::{InboxStore, JournalStore, ScheduleStore, ScheduledWorkflowStore};
use crate::runtime::workflow::scheduled::lifecycle::*;
use crate::scheduler::*;
use chrono::{DateTime, Duration, Utc};

struct Clock(DateTime<Utc>);
impl crate::runtime::types::RuntimeClock for Clock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}
fn clock() -> Clock {
    Clock(
        DateTime::parse_from_rfc3339("2026-10-24T06:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    )
}
fn create(
    store: &crate::durable::SqliteConversationStore,
    current: &ScheduledWorkflowPreparation,
    key: &str,
) -> crate::scheduler::ScheduleSnapshot {
    let prepared = current.prepare(&id(), &args()).unwrap();
    let command = ScheduleCommand {
        command_id: key.into(),
        mutation: ScheduleMutation::CreateWorkflow {
            title: "Scheduled native work".into(),
            workflow_id: id(),
            arguments: args(),
            timing: ScheduleTiming::After { seconds: 1 },
        },
    };
    let ScheduleCommandResult::Applied { receipt } = store
        .schedule_workflow_command(
            &current.binding,
            &command,
            &clock(),
            &ScheduleCommandAuthority::RuntimeControl,
            prepared.contract(),
        )
        .unwrap()
    else {
        panic!("prepared Workflow is accepted")
    };
    store.schedule_get(&receipt.id).unwrap().unwrap()
}
fn admit(
    store: &crate::durable::SqliteConversationStore,
    schedule: &ScheduleSnapshot,
) -> ScheduledWorkflowHead {
    let later = Clock(clock().0 + Duration::seconds(1));
    let ScheduleAdmission::WorkflowAdmitted { start, .. } = store
        .admit_scheduled_target(&schedule.id, schedule.revision, &later)
        .unwrap()
    else {
        panic!("native Workflow admission")
    };
    *start
}
#[test]
fn scheduler85_workflow_admission_is_atomic_and_survives_definition_deletion() {
    let root = tempfile::tempdir().unwrap();
    let current = fixture(
        root.path(),
        &Inputs {
            kind: NodeKind::PureReturn,
            ..Inputs::default()
        },
    );
    let store =
        crate::durable::SqliteConversationStore::in_memory(current.binding.conversation_id.clone())
            .unwrap();
    let schedule = create(&store, &current, "atomic");
    let generation = store.schedule_summary().unwrap().generation;
    store
        .fail_accept_remaining
        .store(1, std::sync::atomic::Ordering::SeqCst);
    assert!(
        store
            .admit_scheduled_target(&schedule.id, 1, &Clock(clock().0 + Duration::seconds(1)))
            .is_err()
    );
    assert!(store.scheduled_workflow_pending(256).unwrap().is_empty());
    assert_eq!(
        store.schedule_get(&schedule.id).unwrap().unwrap().revision,
        1
    );
    assert_eq!(store.schedule_summary().unwrap().generation, generation);
    let start = admit(&store, &schedule);
    assert!(store.load_pending().unwrap().is_empty());
    assert_eq!(
        store.schedule_summary().unwrap().pending,
        vec![start.trigger_id.clone()]
    );
    assert!(matches!(
        store
            .schedule_command(
                &current.binding,
                &ScheduleCommand {
                    command_id: "delete".into(),
                    mutation: ScheduleMutation::Delete {
                        id: schedule.id.clone(),
                        expected_revision: 2
                    }
                },
                &clock(),
                &ScheduleCommandAuthority::RuntimeControl
            )
            .unwrap(),
        ScheduleCommandResult::Deleted { .. }
    ));
    assert!(store.schedule_get(&schedule.id).unwrap().is_none());
    assert_eq!(
        store
            .scheduled_workflow_get(&start.id)
            .unwrap()
            .unwrap()
            .head,
        start
    );
    assert_eq!(store.schedule_summary().unwrap().pending.len(), 1);
}
#[test]
fn scheduler85_claim_is_not_reissued_after_reopen_or_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let current = fixture(
        root.path(),
        &Inputs {
            kind: NodeKind::PureReturn,
            ..Inputs::default()
        },
    );
    let path = root.path().join("durable.sqlite");
    let store = crate::durable::SqliteConversationStore::open(
        current.binding.conversation_id.clone(),
        &path,
    )
    .unwrap();
    let schedule = create(&store, &current, "claim");
    let start = admit(&store, &schedule);
    let prepared = current
        .reconstruct(
            store
                .scheduled_workflow_get(&start.id)
                .unwrap()
                .unwrap()
                .contract
                .as_ref()
                .unwrap(),
            &args(),
        )
        .unwrap();
    let claim = store
        .scheduled_workflow_claim(
            &start.id,
            prepared.contract(),
            "original-native-owner",
            clock().0,
        )
        .unwrap()
        .unwrap();
    assert!(
        store
            .scheduled_workflow_claim(&start.id, prepared.contract(), "duplicate", clock().0)
            .unwrap()
            .is_none()
    );
    drop(claim);
    drop(store);
    let store = crate::durable::SqliteConversationStore::open(
        current.binding.conversation_id.clone(),
        &path,
    )
    .unwrap();
    assert!(
        store
            .scheduled_workflow_claim(&start.id, prepared.contract(), "after-restart", clock().0)
            .unwrap()
            .is_none()
    );
    store
        .scheduled_workflow_cancel(&start.id, clock().0)
        .unwrap();
    assert!(matches!(
        store
            .scheduled_workflow_get(&start.id)
            .unwrap()
            .unwrap()
            .head
            .state,
        ScheduledWorkflowState::Claimed { .. }
    ));
    assert_eq!(
        store.schedule_summary().unwrap().pending.len(),
        1,
        "cancellation is not physical proof"
    );
    let result = store
        .scheduled_workflow_settle(
            &start.id,
            ScheduledWorkflowOutcome::OutcomeUnknown {
                reason: "crash immediately after Claim, before effects".into(),
            },
            ScheduledWorkflowQuiescence {
                source: QuiescenceSource::RecoveredNoEffects,
            },
            None,
            clock().0,
        )
        .unwrap();
    assert_eq!(result, ScheduledWorkflowSettlement::Settled);
    assert!(store.schedule_summary().unwrap().pending.is_empty());
    assert_eq!(
        store
            .scheduled_workflow_settle(
                &start.id,
                ScheduledWorkflowOutcome::Cancelled,
                ScheduledWorkflowQuiescence {
                    source: QuiescenceSource::RecoveredNoEffects
                },
                None,
                clock().0
            )
            .unwrap(),
        ScheduledWorkflowSettlement::AlreadySettled
    );
}
#[test]
fn scheduler85_changed_accepted_dependency_is_durably_rejected_with_zero_effects() {
    let root = tempfile::tempdir().unwrap();
    let current = fixture(
        root.path(),
        &Inputs {
            kind: NodeKind::Tool,
            ..Inputs::default()
        },
    );
    let path = root.path().join("rejected.sqlite");
    let store = crate::durable::SqliteConversationStore::open(
        current.binding.conversation_id.clone(),
        &path,
    )
    .unwrap();
    let schedule = create(&store, &current, "reject");
    let start = admit(&store, &schedule);
    let saved = store.scheduled_workflow_get(&start.id).unwrap().unwrap();
    let changed = fixture(
        root.path(),
        &Inputs {
            kind: NodeKind::Tool,
            tool_description: "different native capability",
            revision: 2,
            ..Inputs::default()
        },
    );
    let (refused, effects) = crate::local_runtime::static_effects::measure(|| {
        changed.reconstruct(
            saved.contract.as_ref().unwrap(),
            saved.arguments.as_ref().unwrap(),
        )
    });
    assert!(refused.is_err());
    assert_no_graph_effects(effects);
    assert!(
        store
            .scheduled_workflow_reject(&start.id, "native dependency contract changed", clock().0)
            .unwrap()
    );
    drop(store);
    let store = crate::durable::SqliteConversationStore::open(
        current.binding.conversation_id.clone(),
        &path,
    )
    .unwrap();
    assert!(matches!(
        store
            .scheduled_workflow_get(&start.id)
            .unwrap()
            .unwrap()
            .head
            .state,
        ScheduledWorkflowState::Settled {
            outcome: ScheduledWorkflowOutcome::RejectedBeforeStart { .. },
            ..
        }
    ));
    assert!(
        store
            .scheduled_workflow_claim(
                &start.id,
                saved.contract.as_ref().unwrap(),
                "cannot-resurrect",
                clock().0
            )
            .unwrap()
            .is_none()
    );
}
#[test]
fn scheduler85_missing_native_proof_stays_blocked_until_the_actual_owner_settles() {
    let root = tempfile::tempdir().unwrap();
    let current = fixture(
        root.path(),
        &Inputs {
            kind: NodeKind::Tool,
            ..Inputs::default()
        },
    );
    let store =
        crate::durable::SqliteConversationStore::in_memory(current.binding.conversation_id.clone())
            .unwrap();
    let schedule = create(&store, &current, "unknown");
    let start = admit(&store, &schedule);
    let prepared = current.prepare(&id(), &args()).unwrap();
    let claim = store
        .scheduled_workflow_claim(&start.id, prepared.contract(), "native-owner", clock().0)
        .unwrap()
        .unwrap();
    let node = crate::runtime::workflow::WorkflowNodeInstance {
        block: crate::runtime::workflow::WorkflowBlockInstance {
            run: start.run_id.clone(),
            definition: crate::runtime::workflow::WorkflowDefinitionPath {
                workflow_id: id(),
                blocks: vec![],
            },
            invocations: vec![0],
        },
        node: "work".into(),
        visit: 0,
    };
    store
        .scheduled_workflow_begin_effect(&claim, &node, ScheduledEffectKind::Tool)
        .unwrap();
    store
        .scheduled_workflow_effect_owner(
            &claim,
            &node,
            ScheduledEffectOwner::Tool {
                invocation: ToolInvocationId::Workflow {
                    node: Box::new(node.clone()),
                },
            },
        )
        .unwrap();
    for _ in 0..3 {
        store
            .scheduled_workflow_cancel(&start.id, clock().0)
            .unwrap();
        store
            .scheduled_workflow_recover_effects(
                &start.id,
                0,
                100,
                &ScheduledWorkflowOwnerRetirement {
                    start_id: start.id.clone(),
                    recovered_writer: true,
                    agent_domain_quiescent: false,
                    interactions_closed: true,
                },
            )
            .unwrap();
        assert_eq!(
            store
                .scheduled_workflow_settle(
                    &start.id,
                    ScheduledWorkflowOutcome::OutcomeUnknown {
                        reason: "missing native proof".into()
                    },
                    ScheduledWorkflowQuiescence {
                        source: QuiescenceSource::RecoveredOwners
                    },
                    None,
                    clock().0
                )
                .unwrap(),
            ScheduledWorkflowSettlement::AwaitingNativeProof
        );
        let view = store
            .scheduled_workflow_recovery(&start.id, 0, 100)
            .unwrap()
            .unwrap();
        assert_eq!(view.start.run_id, start.run_id);
        assert!(view.capacity_blocked && view.deletion_blocked);
        assert!(!view.effects[0].physical_settlement_proven);
    }
    // Only the retained, unique native claim can publish this completion.
    // The operator RPC has no corresponding bool/proof input.
    store
        .scheduled_workflow_effect_settled(
            &claim,
            &node,
            true,
            Some("original native owner confirmed physical settlement"),
        )
        .unwrap();
    assert_eq!(
        store
            .scheduled_workflow_settle(
                &start.id,
                ScheduledWorkflowOutcome::OutcomeUnknown {
                    reason: "physical owner retired; external result remains unknown".into()
                },
                ScheduledWorkflowQuiescence {
                    source: QuiescenceSource::RecoveredOwners
                },
                None,
                clock().0
            )
            .unwrap(),
        ScheduledWorkflowSettlement::Settled
    );
    assert!(
        !store
            .scheduled_workflow_recovery(&start.id, 0, 100)
            .unwrap()
            .unwrap()
            .capacity_blocked
    );
}
#[test]
fn scheduler85_recovery_pages_do_not_expand_into_unbounded_work_lists() {
    let root = tempfile::tempdir().unwrap();
    let current = fixture(
        root.path(),
        &Inputs {
            kind: NodeKind::Tool,
            ..Inputs::default()
        },
    );
    let store =
        crate::durable::SqliteConversationStore::in_memory(current.binding.conversation_id.clone())
            .unwrap();
    let schedule = create(&store, &current, "pages");
    let start = admit(&store, &schedule);
    let prepared = current.prepare(&id(), &args()).unwrap();
    let claim = store
        .scheduled_workflow_claim(&start.id, prepared.contract(), "native-owner", clock().0)
        .unwrap()
        .unwrap();
    for visit in 0..3 {
        let node = crate::runtime::workflow::WorkflowNodeInstance {
            block: crate::runtime::workflow::WorkflowBlockInstance {
                run: start.run_id.clone(),
                definition: crate::runtime::workflow::WorkflowDefinitionPath {
                    workflow_id: id(),
                    blocks: vec![],
                },
                invocations: vec![0],
            },
            node: "work".into(),
            visit,
        };
        store
            .scheduled_workflow_begin_effect(&claim, &node, ScheduledEffectKind::Tool)
            .unwrap();
    }
    let first = store
        .scheduled_workflow_recovery(&start.id, 0, 2)
        .unwrap()
        .unwrap();
    assert_eq!(first.effects.len(), 2);
    let second = store
        .scheduled_workflow_recovery(&start.id, first.next_effect_ordinal.unwrap(), 2)
        .unwrap()
        .unwrap();
    assert_eq!(second.effects.len(), 1);
    assert!(second.next_effect_ordinal.is_none());
    assert!(
        store
            .scheduled_workflow_recovery(&start.id, 0, 101)
            .is_err()
    );
    assert!(
        store
            .scheduled_workflow_recovery(&start.id, u64::MAX, 1)
            .is_err()
    );
    assert!(
        store
            .read_events(None, 256)
            .unwrap()
            .events
            .iter()
            .all(|event| !matches!(
                event.event,
                crate::events::types::RuntimeEvent::AttemptStarted { .. }
            ))
    );
}

#[test]
fn scheduler85_claimed_blocker_cannot_hide_later_accepted_workflow_or_reactivate_itself() {
    let root = tempfile::tempdir().unwrap();
    let current = fixture(
        root.path(),
        &Inputs {
            kind: NodeKind::PureReturn,
            ..Inputs::default()
        },
    );
    let store =
        crate::durable::SqliteConversationStore::in_memory(current.binding.conversation_id.clone())
            .unwrap();
    let first = create(&store, &current, "blocked");
    let blocked = admit(&store, &first);
    let prepared = current.prepare(&id(), &args()).unwrap();
    let _claim = store
        .scheduled_workflow_claim(
            &blocked.id,
            prepared.contract(),
            "retained-native-owner",
            clock().0,
        )
        .unwrap()
        .unwrap();
    let second = create(&store, &current, "ready");
    let ready = admit(&store, &second);
    let summary = store.schedule_summary().unwrap();
    assert_eq!(summary.pending.len(), 2);
    assert!(summary.pending.contains(&blocked.trigger_id));
    assert!(summary.pending.contains(&ready.trigger_id));
    assert_eq!(summary.ready, vec![ready.trigger_id]);
}

#[test]
fn scheduler85_settlement_compacts_payloads_without_losing_lifetime_start_identity() {
    let root = tempfile::tempdir().unwrap();
    let current = fixture(
        root.path(),
        &Inputs {
            kind: NodeKind::PureReturn,
            ..Inputs::default()
        },
    );
    let store =
        crate::durable::SqliteConversationStore::in_memory(current.binding.conversation_id.clone())
            .unwrap();
    let prepared = current.prepare(&id(), &args()).unwrap();
    let mut starts = Vec::new();
    for index in 0..201 {
        let schedule = create(&store, &current, &format!("retention-{index}"));
        let start = admit(&store, &schedule);
        let now = clock().0 + Duration::seconds(10 + index);
        match index % 3 {
            0 => {
                let _claim = store
                    .scheduled_workflow_claim(
                        &start.id,
                        prepared.contract(),
                        "native-executor",
                        clock().0,
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    store
                        .scheduled_workflow_settle(
                            &start.id,
                            ScheduledWorkflowOutcome::Completed,
                            ScheduledWorkflowQuiescence {
                                source: QuiescenceSource::NativeExecutor
                            },
                            Some(&serde_json::json!({"answer":"durable"})),
                            now
                        )
                        .unwrap(),
                    ScheduledWorkflowSettlement::Settled
                );
            }
            1 => {
                store.scheduled_workflow_cancel(&start.id, now).unwrap();
            }
            _ => {
                store
                    .scheduled_workflow_reject(&start.id, "dependency changed before Claim", now)
                    .unwrap();
            }
        }
        starts.push(start.id);
    }
    let oldest = store.scheduled_workflow_get(&starts[0]).unwrap().unwrap();
    assert!(oldest.contract.is_none() && oldest.arguments.is_none() && oldest.result.is_none());
    assert!(oldest.head.result_digest.is_some());
    assert!(matches!(
        oldest.head.state,
        ScheduledWorkflowState::Settled { .. }
    ));
    assert!(
        store
            .scheduled_workflow_claim(&oldest.head.id, prepared.contract(), "duplicate", clock().0)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        starts
            .iter()
            .filter(|id| store
                .scheduled_workflow_get(id)
                .unwrap()
                .unwrap()
                .contract
                .is_some())
            .count(),
        200
    );
    store
        .prune_schedules(&Clock(clock().0 + Duration::days(31)))
        .unwrap();
    assert!(
        store
            .schedule_summary()
            .unwrap()
            .oldest_completed_utc
            .is_none()
    );
    assert_eq!(starts.len(), 201);
    assert!(starts.iter().all(|id| {
        store
            .scheduled_workflow_get(id)
            .unwrap()
            .is_some_and(|start| start.contract.is_none())
    }));
}
