//! Read-model regressions: live activity, Goal idle ownership, and bounded
//! newest-first listings that never mutate lifecycle or observation.

use super::*;
use crate::runtime::ToolCallId;

// --- The subagent domain's own bounded discovery read model (#180) ---
//
// These prove the read model directly against the registry's own record
// vector, without a live child: the properties under test are ordering,
// filtering, counting, and bounding, none of which involve a process.
// The listing type is the subagent domain's own — nothing here names
// the model-facing `execution` control plane.

/// Issue #178: a live activity update lands in the registry read model
/// and rides the snapshot, while the lifecycle — the only authority —
/// is untouched. Stale or reordered revisions are dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_activity_updates_the_snapshot_without_touching_lifecycle() {
    let plane = plane(4);
    let child = stage_stubborn(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;

    let observation = SubagentObservation {
        revision: 1,
        activity: super::super::super::activity::SubagentActivity::Model {
            request_id: crate::runtime::identity::RequestId::new("req-1"),
            retry: 0,
        },
        counters: super::super::super::activity::SubagentActivityCounters {
            model_requests: 1,
            ..super::super::super::activity::SubagentActivityCounters::default()
        },
        ..SubagentObservation::default()
    };
    plane
        .registry
        .apply_activity(&accepted.subagent_id, observation.clone());

    let snapshot = plane
        .registry
        .snapshot(&accepted.subagent_id)
        .expect("snapshot");
    assert_eq!(snapshot.state, SubagentState::Running);
    assert_eq!(snapshot.observation, observation);

    // Stale and reordered revisions are dropped.
    plane
        .registry
        .apply_activity(&accepted.subagent_id, observation.clone());
    plane
        .registry
        .apply_activity(&accepted.subagent_id, SubagentObservation::default());
    let snapshot = plane
        .registry
        .snapshot(&accepted.subagent_id)
        .expect("snapshot");
    assert_eq!(snapshot.observation, observation);

    // Unknown identities are a no-op.
    plane.registry.apply_activity(
        &SubagentId::new("conv_5d71cacf-35af-790f-8022-fe16cd1b9fe0-subagent-99"),
        observation.clone(),
    );

    let _ = plane
        .registry
        .cancel(&accepted.subagent_id, CancellationReason::UserRequested);
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    drop(child);
}

/// Issue #178: the redacted execution profile flows from the resolved
/// specification into the snapshot at commit; settlement resets the
/// activity to neutral with a bumped revision while keeping the
/// counters, and post-terminal activity is dropped.
#[tokio::test]
async fn settlement_resets_activity_to_neutral_and_drops_late_updates() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;

    // The profile was derived from the frozen model authority at start.
    let running = plane
        .registry
        .snapshot(&accepted.subagent_id)
        .expect("running snapshot");
    let profile = running.profile.clone().expect("profile at commit");
    assert_eq!(profile.model, "local/model");
    assert_eq!(running.observation, SubagentObservation::default());

    let observation = SubagentObservation {
        revision: 3,
        activity: super::super::super::activity::SubagentActivity::ToolGroup {
            active: 1,
            representative: Some(
                crate::runtime::subagent::activity::SubagentToolRepresentative {
                    tool_call_id: ToolCallId::new("call-1"),
                    tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
                    progress: None,
                },
            ),
        },
        counters: super::super::super::activity::SubagentActivityCounters {
            tool_executions: 2,
            ..super::super::super::activity::SubagentActivityCounters::default()
        },
        ..SubagentObservation::default()
    };
    plane
        .registry
        .apply_activity(&accepted.subagent_id, observation);

    child
        .complete(ChildResultStatus::Succeeded, Some("the answer"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(
        settled.observation.activity,
        super::super::super::activity::SubagentActivity::AwaitingActivity,
        "the terminal snapshot is activity-neutral"
    );
    assert_eq!(
        settled.observation.revision, 4,
        "the settlement reset bumped the revision"
    );
    assert_eq!(settled.observation.counters.tool_executions, 2);
    assert_eq!(settled.profile, running.profile);

    // Post-terminal activity can never resurrect live-ness.
    let late = SubagentObservation {
        revision: 99,
        ..SubagentObservation::default()
    };
    plane.registry.apply_activity(&accepted.subagent_id, late);
    let after = plane
        .registry
        .snapshot(&accepted.subagent_id)
        .expect("snapshot");
    assert_eq!(after.observation.revision, 4);
    assert_eq!(
        after.observation.activity,
        super::super::super::activity::SubagentActivity::AwaitingActivity
    );
}

/// Seeds one synthetic record in allocation order, so listing order can
/// be asserted against a known allocation sequence.
fn seed_record(registry: &SubagentRegistry, id: &str, lifecycle: SubagentLifecycle) {
    let subagent_id = SubagentId::new(id);
    let mut state = registry.state.lock().expect("registry state");
    let index = state.records.len();
    state.records.push(SubagentRecord {
        execution: None,
        physical_settlement_proven: lifecycle.is_terminal(),
        delegate_delivery: DelegateDelivery::NotSent,
        parent_agent_id: AgentId::new("agent-parent"),
        subagent_id: subagent_id.clone(),
        child_agent_id: AgentId::new(format!("agent-{id}")),
        child_conversation_id: ConversationId::new(format!(
            "conv_00000000-0000-7000-8000-{index:012x}"
        )),
        origin: AgentActivationOrigin::CreationTool {
            tool_call_id: ToolCallId::new(format!("call-{id}")),
        },
        agent: SubagentName::parse("reviewer").expect("agent name"),
        definition_digest: serde_json::from_value(serde_json::Value::String(format!(
            "sha256:{}",
            "0".repeat(64)
        )))
        .expect("digest"),
        profile_digest: SubagentExecutionProfileDigest::from_committed_fact(format!(
            "sha256:{}",
            "1".repeat(64)
        )),
        ownership: SubagentOwnershipKind::Normal,
        terminal: Some(SubagentTerminalMode::Normal),
        workspace: std::path::PathBuf::from("/workspace"),
        lifecycle,
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
        notification: NotificationState::None,
        started_at: Utc::now(),
    });
    state.index.insert(subagent_id, index);
}

fn listed_ids(listing: &SubagentListing) -> Vec<String> {
    listing
        .snapshots
        .iter()
        .map(|snapshot| snapshot.subagent_id.to_string())
        .collect()
}

#[test]
fn goal84_owned_child_and_terminal_publication_prevent_goal_polling() {
    for lifecycle in [
        SubagentLifecycle::Running,
        SubagentLifecycle::PublishingTerminal,
    ] {
        let plane = plane(1);
        let domain = crate::goal::GoalDomain::new(plane.store.clone());
        let created = domain
            .write(crate::goal::GoalWrite::Create {
                objective: "Await the owned child result".into(),
                budget: 2,
                origin: crate::goal::GoalOrigin::RuntimeControl,
            })
            .unwrap()
            .unwrap();
        seed_record(&plane.registry, "owned", lifecycle);
        assert_eq!(
            plane
                .registry
                .with_goal_idle(|| panic!("Goal must await the native result path")),
            None::<()>
        );
        let view = domain.view().unwrap();
        assert_eq!(
            view.current,
            Some(created),
            "waiting neither consumes a round nor declares Blocked"
        );
        assert!(
            domain.authorizes_continuation().unwrap(),
            "the Goal stays Active and authorized; it simply may not poll"
        );
    }
    let plane = plane(1);
    seed_record(&plane.registry, "settled", SubagentLifecycle::Succeeded);
    assert_eq!(plane.registry.with_goal_idle(|| true), Some(true));
}

/// The registry's authoritative order is its own allocation order,
/// reversed: the most recently started child first.
#[test]
fn subagent_listing_is_newest_first_in_the_registrys_allocation_order() {
    let plane = plane(8);
    for id in ["s1", "s2", "s3"] {
        seed_record(&plane.registry, id, SubagentLifecycle::Running);
    }

    let listing = plane.registry.listing(false, 16);

    assert_eq!(listed_ids(&listing), vec!["s3", "s2", "s1"]);
    assert_eq!(listing.matched, 3);
}

/// `active_only` is the domain's own lifecycle classification, under
/// which `PublishingTerminal` is still active.
#[test]
fn subagent_listing_active_only_uses_the_domains_own_classification() {
    let plane = plane(8);
    seed_record(&plane.registry, "s1", SubagentLifecycle::Running);
    seed_record(&plane.registry, "s2", SubagentLifecycle::Succeeded);
    seed_record(&plane.registry, "s3", SubagentLifecycle::PublishingTerminal);
    seed_record(&plane.registry, "s4", SubagentLifecycle::Cancelled);

    assert_eq!(
        listed_ids(&plane.registry.listing(true, 16)),
        vec!["s3", "s1"]
    );
    assert_eq!(
        listed_ids(&plane.registry.listing(false, 16)),
        vec!["s4", "s3", "s2", "s1"]
    );
}

/// `matched` counts every matching record before the bound, and the
/// materialized prefix stays finite — so a caller can report truncation
/// without the registry ever building an unbounded response.
#[test]
fn subagent_listing_counts_matches_before_its_materialization_bound() {
    let plane = plane(8);
    for ordinal in 1..=9 {
        let lifecycle = if ordinal % 3 == 0 {
            SubagentLifecycle::Succeeded
        } else {
            SubagentLifecycle::Running
        };
        seed_record(&plane.registry, &format!("s{ordinal}"), lifecycle);
    }

    let all = plane.registry.listing(false, 2);
    assert_eq!(all.matched, 9);
    assert_eq!(listed_ids(&all), vec!["s9", "s8"]);

    let active = plane.registry.listing(true, 3);
    assert_eq!(active.matched, 6);
    assert_eq!(listed_ids(&active), vec!["s8", "s7", "s5"]);

    // A zero bound still reports the whole matching population.
    let none = plane.registry.listing(false, 0);
    assert!(none.snapshots.is_empty());
    assert_eq!(none.matched, 9);
}

/// Listing is a pure read: it changes no lifecycle and no observation.
#[test]
fn subagent_listing_mutates_neither_lifecycle_nor_observation() {
    let plane = plane(8);
    seed_record(&plane.registry, "s1", SubagentLifecycle::Running);
    let observation = SubagentObservation {
        revision: 7,
        ..SubagentObservation::default()
    };
    plane
        .registry
        .apply_activity(&SubagentId::new("s1"), observation);
    let before = plane.registry.all_snapshots();

    for _ in 0..3 {
        let _ = plane.registry.listing(false, 16);
        let _ = plane.registry.listing(true, 1);
    }

    let after = plane.registry.all_snapshots();
    assert_eq!(before, after);
    assert_eq!(after[0].state, SubagentState::Running);
    assert_eq!(after[0].observation.revision, 7);
}
