//! Terminal settlement regressions: durable inbound results, workflow output
//! ordering, shared-workspace effects, and bounded terminal publication retry.

use super::*;
use crate::durable::inbox::ConversationInboundCapability;
use crate::runtime::subagent::process::PhysicalOutcome;
use crate::runtime::subagent::process::PhysicalSettlement;

#[tokio::test]
async fn a_successful_child_settles_through_the_durable_inbound() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("inspect the workspace")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("the answer"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Succeeded);
    // Issue #178: the successful answer content never rides the live
    // observation/control projection — `detail` is diagnostics-only,
    // and the durable pending inbound below is the one result channel.
    assert_eq!(settled.detail, None);
    // The result entered the parent's durable pending inbound with the
    // child agent provenance, exactly once — preceded by the
    // runtime-authored correlation notice of the same atomic
    // publication (Issue #192).
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 2, "the notice and the report");
    let notice = &pending.items[0];
    assert_eq!(
        notice.message.id,
        super::super::super::terminal_notice_message_id(&accepted.subagent_id)
    );
    assert!(matches!(
        notice.message.source,
        crate::message::types::UserSource::Runtime
    ));
    assert!(notice.sequence < pending.items[1].sequence);
    let item = &pending.items[1];
    assert_eq!(
        item.correlation.as_deref(),
        Some(super::super::super::terminal_correlation(&accepted.subagent_id).as_str())
    );
    assert!(matches!(
        item.message.source,
        crate::message::types::UserSource::Agent { ref agent_id }
            if *agent_id == accepted.child_agent_id
    ));
    let journal = events(&plane);
    assert!(journal.iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { subagent_id, .. }
            if *subagent_id == accepted.subagent_id
    )));
    assert!(journal.iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentTerminalPublished {
            subagent_id,
            state: SubagentTerminalState::Succeeded,
            ..
        } if *subagent_id == accepted.subagent_id
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_output_committed_before_cancel_remains_successful() {
    workflow_child_cancellation_ordering(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_cancel_before_output_commit_remains_cancelled() {
    workflow_child_cancellation_ordering(false).await;
}

async fn workflow_child_cancellation_ordering(late: bool) {
    let result =
        crate::local_runtime::subagent_child::tests::workflow_cancellation_ordering(late).await;
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(
        &plane,
        &workflow_spec(&plane.conversation_id, "output cancellation ordering"),
    )
    .await;
    plane
        .registry
        .cancel(&accepted.subagent_id, CancellationReason::UserRequested)
        .unwrap();
    child
        .cancelled_after_delegation(result.status, result.content.as_deref())
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    assert_eq!(
        settled.state,
        if late {
            SubagentState::Succeeded
        } else {
            SubagentState::Cancelled
        }
    );
    assert!(plane.store.select_pending_batch().unwrap().is_none());
    assert_eq!(
        events(&plane)
            .iter()
            .filter(|event| matches!(event,
        crate::events::types::RuntimeEvent::WorkflowAgentOutputCommitted { subagent_id, .. }
            if *subagent_id == accepted.subagent_id))
            .count(),
        usize::from(late)
    );
    assert_eq!(
        plane.registry.workflow_agent_output(&accepted.subagent_id),
        late.then(|| serde_json::json!({"summary": "committed answer"}))
    );
}

#[tokio::test]
async fn workflow_child_inspection_does_not_grant_durable_agent_controls() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(
        &plane,
        &workflow_spec(&plane.conversation_id, "return a structured result"),
    )
    .await;
    let inspection = plane
        .registry
        .workflow_inspection_snapshot(&accepted.child_agent_id)
        .expect("finite child is inspectable");
    assert_eq!(inspection.subagent_id, accepted.subagent_id);
    assert!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .is_none()
    );
    assert!(
        plane
            .registry
            .check_message_target(&accepted.child_agent_id)
            .is_err()
    );
    assert!(
        plane
            .registry
            .interrupt_agent_intent(&accepted.child_agent_id)
            .is_err()
    );
    assert!(
        plane
            .registry
            .agent_inspection_path(&accepted.child_agent_id)
            .is_some()
    );
    assert!(
        plane
            .registry
            .agent_workspace(&accepted.child_agent_id)
            .is_some()
    );
    child
        .complete(
            ChildResultStatus::Succeeded,
            Some(r#"{"summary":"answer"}"#),
        )
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
}

#[tokio::test]
async fn a_workflow_child_closes_native_lifecycle_without_parent_inbound() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(
        &plane,
        &workflow_spec(&plane.conversation_id, "return a structured result"),
    )
    .await;
    child
        .complete(
            ChildResultStatus::Succeeded,
            Some(r#"{"summary":"answer"}"#),
        )
        .await;

    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert!(
        plane
            .store
            .select_pending_batch()
            .expect("pending")
            .is_none()
    );
    let journal = events(&plane);
    assert!(journal.iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentTerminalSettled {
            subagent_id,
            state: SubagentTerminalState::Succeeded,
            ..
        } if *subagent_id == accepted.subagent_id
    )));
    assert!(!journal.iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentTerminalPublished {
            subagent_id, ..
        } if *subagent_id == accepted.subagent_id
    )));
    assert_eq!(
        journal
            .iter()
            .filter(|event| matches!(
                event,
                crate::events::types::RuntimeEvent::WorkflowAgentOutputCommitted {
                    subagent_id, output, ..
                } if *subagent_id == accepted.subagent_id
                    && *output == serde_json::json!({"summary": "answer"})
            ))
            .count(),
        1,
        "the Workflow value is committed exactly once with the child terminal fact"
    );
    // The live Workflow result channel is the registry's committed
    // value, not the observation snapshot (Issue #178): the snapshot
    // detail is diagnostics-only even for a successful Workflow child.
    assert_eq!(settled.detail, None);
    assert_eq!(
        plane.registry.workflow_agent_output(&accepted.subagent_id),
        Some(serde_json::json!({"summary": "answer"})),
    );
    let mut instance = crate::runtime::workflow::test_instance("test_workflow", "agent");
    instance.block.run.conversation_id = plane.conversation_id.clone();
    let mut other_instance = instance.clone();
    other_instance.block.invocations[0] = 1;
    assert_eq!(
        plane
            .registry
            .take_workflow_agent_output(&accepted.subagent_id, &other_instance),
        None
    );
    assert_eq!(
        plane
            .registry
            .take_workflow_agent_output(&accepted.subagent_id, &instance),
        Some(serde_json::json!({"summary":"answer"}))
    );
    assert_eq!(
        plane
            .registry
            .take_workflow_agent_output(&accepted.subagent_id, &instance),
        None,
        "terminal output transfers only once to its exact execution instance"
    );
}

#[tokio::test]
async fn semantic_success_with_unresolved_nested_containment_is_failed() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;
    let settlement = PhysicalSettlement {
        outcome: PhysicalOutcome::Completed(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("child success must not leak".to_owned()),
            diagnostic: None,
        }),
        nested: super::super::super::anchors::NestedUnitSettlement {
            contained: Vec::new(),
            unproven: vec![(
                crate::runtime::identity::ProcessUnitId::new("unit-unresolved"),
                "test containment proof is unavailable".to_owned(),
            )],
        },
        runtime_root_cleanup_error: None,
    };
    plane
        .registry
        .settle_from_driver(&accepted.subagent_id, settlement);
    drop(child.peer);

    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Failed);
    assert!(
        settled
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("nested supervised process unit"))
    );
    assert!(
        !settled
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("child success must not leak")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unresolved_nested_containment_keeps_the_agent_unavailable_through_recovery() {
    let plane = plane(4);
    let child = stage_with_unresolved_anchor(&plane);
    let spec = deadline_spec("nested containment", 100);
    let accepted = start(&plane, &spec).await;

    child
        .complete(ChildResultStatus::Succeeded, Some("child answer"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled nested-unresolved child");
    assert_eq!(settled.state, SubagentState::Failed);
    assert_eq!(settled.workspace, plane.dir.path().join("workspace"));

    let recovered = recovered_registry(&plane);
    assert!(matches!(
        recovered.wait_agent(&accepted.child_agent_id).await,
        Err(AgentControlError::Settlement)
    ));
    assert!(
        plane.dir.path().join("workspace").is_dir(),
        "physical uncertainty never removes the shared Session workspace"
    );
}

#[tokio::test]
async fn a_failed_child_settles_as_a_runtime_notice() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;
    child.complete(ChildResultStatus::Failed, None).await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Failed);
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert!(matches!(
        pending.items[0].message.source,
        crate::message::types::UserSource::Runtime
    ));
}

#[tokio::test]
async fn cancellation_is_canonical_over_a_late_result() {
    let plane = plane(4);
    let child = stage_stubborn(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;
    let cancelled = plane
        .registry
        .cancel(&accepted.subagent_id, CancellationReason::UserRequested)
        .expect("known");
    assert_eq!(cancelled.state, SubagentState::Cancelling);
    child
        .cancelled_after_delegation(ChildResultStatus::Succeeded, Some("late"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    // The committed cancellation intent wins over the late success.
    assert_eq!(settled.state, SubagentState::Cancelled);
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert!(matches!(
        pending.items[0].message.source,
        crate::message::types::UserSource::Runtime
    ));
}

/// A normal Agent works in the shared Session workspace: its file
/// effects stay in place after settlement, and it publishes exactly one
/// notice/report pair through canonical inbound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one cohesive retained-notice regression
async fn a_successful_agent_shares_the_session_workspace_and_publishes_one_adjacent_notice() {
    let plane = plane(4);
    let child = stage_stubborn(&plane);
    let spec = start_spec("write a source change");
    let accepted = start(&plane, &spec).await;
    let workspace = plane
        .registry
        .snapshot(&accepted.subagent_id)
        .unwrap()
        .workspace;
    assert_eq!(workspace, plane.dir.path().join("workspace"));
    std::fs::write(workspace.join("retained.txt"), "Agent-owned work").unwrap();
    child
        .complete(ChildResultStatus::Succeeded, Some("child answer"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    assert_eq!(settled.workspace, workspace);
    assert!(workspace.join("retained.txt").exists());
    let agent = plane
        .registry
        .agent_snapshot(&accepted.child_agent_id)
        .unwrap();
    assert_eq!(agent.state, super::super::super::AgentState::Inactive);

    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(
        pending.items.len(),
        2,
        "the notice and the report, exactly once each"
    );

    // The adjacent runtime-authored notice, ordered first.
    let notice = &pending.items[0];
    assert_eq!(
        notice.message.id,
        super::super::super::terminal_notice_message_id(&accepted.subagent_id)
    );
    assert_eq!(
        notice.correlation.as_deref(),
        Some(super::super::super::terminal_notice_correlation(&accepted.subagent_id).as_str())
    );
    assert!(
        matches!(
            notice.message.source,
            crate::message::types::UserSource::Runtime
        ),
        "the settlement fact is runtime-authored, never attributed to the child"
    );
    let notice_text = &notice.message.content[0];
    let crate::message::types::UserContentBlock::Text(notice_text) = notice_text else {
        panic!("the notice is text: {notice_text:?}");
    };
    assert!(
        notice_text.text.contains(&format!(
            "Agent {} activation {}",
            accepted.child_agent_id, accepted.subagent_id
        )),
        "the notice correlates both independent identities"
    );
    assert!(
        !notice_text.text.contains(&workspace.display().to_string()),
        "no physical path crosses the model boundary"
    );
    assert!(
        !notice_text.text.contains("dispose"),
        "the inactive Agent workspace is not a disposable terminal handoff"
    );

    // The terminal report remains last and purely child-authored.
    let report = &pending.items[1];
    assert!(
        notice.sequence < report.sequence,
        "the terminal result remains the last item of the publication"
    );
    assert_eq!(
        report.message.id,
        super::super::super::terminal_message_id(&accepted.subagent_id)
    );
    assert!(
        matches!(
            report.message.source,
            crate::message::types::UserSource::Agent { ref agent_id }
                if *agent_id == accepted.child_agent_id
        ),
        "the report is the child's own authorship"
    );
    let report_text = &report.message.content[0];
    let crate::message::types::UserContentBlock::Text(report_text) = report_text else {
        panic!("the report is text: {report_text:?}");
    };
    assert_eq!(
        report_text.text, "child answer",
        "the child-authored report is never amended with runtime facts"
    );

    // Exactly one terminal fact committed for the child.
    assert_eq!(
        published_terminal_states(&plane, &accepted.subagent_id),
        vec![SubagentTerminalState::Succeeded]
    );
}

/// A failed activation leaves its completed effects in the shared
/// workspace (failure is not rollback) and publishes one runtime-authored
/// failure notice, without physical paths.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_agent_leaves_shared_effects_and_publishes_one_runtime_notice() {
    let plane = plane(4);
    let child = stage_stubborn(&plane);
    let spec = start_spec("write a source change");
    let accepted = start(&plane, &spec).await;
    let workspace = plane
        .registry
        .snapshot(&accepted.subagent_id)
        .expect("running snapshot")
        .workspace;
    std::fs::write(workspace.join("retained.txt"), "retain this child work\n").expect("child work");
    child.complete(ChildResultStatus::Failed, None).await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Failed);
    assert!(workspace.join("retained.txt").exists());

    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 1, "one runtime-authored notice");
    let notice = &pending.items[0];
    assert!(matches!(
        notice.message.source,
        crate::message::types::UserSource::Runtime
    ));
    let crate::message::types::UserContentBlock::Text(text) = &notice.message.content[0] else {
        panic!("the notice is text");
    };
    assert!(text.text.contains("failed"), "the failure diagnostic");
    assert!(!text.text.contains("dispose"));
    assert!(!text.text.contains(&workspace.display().to_string()));
}

#[tokio::test]
async fn a_terminal_publication_failure_is_retried_then_abandoned() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;
    // The initial publication plus both bounded retries fail.
    plane.store.arm_fail_accept_times(3);
    child
        .complete(ChildResultStatus::Succeeded, Some("the answer"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("abandoned resolves the wait");
    assert_eq!(
        settled.settlement.publication,
        SubagentPublication::Abandoned
    );
    assert_eq!(settled.state, SubagentState::PublishingTerminal);
    // Issue #462 (B18): the child process was physically settled; that
    // proof alone never clears the independent publication failure.
    assert_eq!(
        settled.settlement.physical,
        SubagentPhysicalSettlement::Proven
    );
    assert!(matches!(
        plane.registry.wait_agent(&accepted.child_agent_id).await,
        Err(AgentControlError::Settlement)
    ));
    assert!(matches!(
        plane
            .registry
            .interrupt_agent(&accepted.child_agent_id)
            .await,
        Err(AgentControlError::Settlement)
    ));
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .unwrap()
            .state,
        AgentState::Unavailable
    );

    // Nothing reached the durable authority.
    assert!(
        plane
            .store
            .select_pending_batch()
            .expect("pending")
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn physical_settlement_failure_keeps_terminal_retry_classification_stable() {
    let plane = plane(4);
    let child = stage_with_unresolved_anchor(&plane);
    let spec = start_spec("inspect");
    let accepted = start(&plane, &spec).await;
    plane.store.arm_fail_accept_times(3);
    child
        .complete(
            ChildResultStatus::Succeeded,
            Some("success must stay private"),
        )
        .await;

    let abandoned = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("publication abandoned");
    assert_eq!(abandoned.state, SubagentState::PublishingTerminal);
    assert_eq!(
        abandoned.settlement.publication,
        SubagentPublication::Abandoned
    );
    let diagnostic = abandoned.detail.clone().expect("stable diagnostic");
    assert!(diagnostic.contains("physical settlement was not proven"));
    assert!(!diagnostic.contains("success must stay private"));

    assert!(
        plane
            .registry
            .retry_terminal_publication(&accepted.subagent_id)
            .expect("retry")
    );
    let settled = plane
        .registry
        .snapshot(&accepted.subagent_id)
        .expect("settled snapshot");
    assert_eq!(settled.state, SubagentState::Failed);
    assert_ne!(
        settled.settlement.publication,
        SubagentPublication::Abandoned
    );
    assert_eq!(settled.detail.as_deref(), Some(diagnostic.as_ref()));
    assert_eq!(
        events(&plane)
            .into_iter()
            .filter(|event| matches!(
                event,
                crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                    subagent_id,
                    state: SubagentTerminalState::Failed,
                    ..
                } if *subagent_id == accepted.subagent_id
            ))
            .count(),
        1,
        "retry publishes the frozen failed classification exactly once"
    );
}

/// A physically settled child still owns its capacity while its frozen
/// terminal candidate is unresolved. Once the identical publication is
/// durably accepted, the slot is released and the next commit may win.
#[tokio::test]
async fn publishing_terminal_retains_capacity_until_durable_settlement() {
    let plane = plane(1);
    let child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("first")).await;
    plane.store.arm_fail_accept_times(3);
    child
        .complete(ChildResultStatus::Succeeded, Some("first answer"))
        .await;
    let unresolved = plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .expect("abandoned publication is observable");
    assert_eq!(unresolved.state, SubagentState::PublishingTerminal);
    assert_eq!(
        unresolved.settlement.publication,
        SubagentPublication::Abandoned
    );

    let _second_child = stage_exit0(&plane);
    let second_prepared = plane
        .registry
        .prepare(&start_spec("second"), &CancellationSignal::new())
        .await
        .expect("private preparation is allowed");
    let error = plane
        .registry
        .commit(second_prepared, &CancellationSignal::new())
        .await
        .expect_err("unresolved terminal settlement retains capacity");
    assert!(matches!(
        error,
        SubagentStartError::CapacityExceeded { max: 1 }
    ));

    assert!(
        plane
            .registry
            .retry_terminal_publication(&first.subagent_id)
            .unwrap()
    );
    let settled = plane
        .registry
        .snapshot(&first.subagent_id)
        .expect("first snapshot");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert!(settled.is_settled());
}

#[tokio::test]
async fn an_ambiguous_publication_commit_resolves_exactly_once() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("the answer"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    // A retry of the same correlated publication is an idempotent
    // no-op, never a second message. Rebuild the byte-identical pair:
    // the frozen candidate timestamp is the committed one.
    let first = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("batch");
    assert_eq!(first.items.len(), 2, "the notice and the report");
    let committed_at = first.items[1]
        .message
        .timestamp
        .expect("terminal notifications carry a timestamp");
    let (draft, event) = super::super::super::terminal_publication(
        &plane.conversation_id,
        &accepted.subagent_id,
        &accepted.child_agent_id,
        SubagentTerminalState::Succeeded,
        vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: "the answer".to_owned(),
            },
        )],
        true,
        committed_at,
    );
    let notice = super::super::super::terminal_notice(
        &accepted.child_agent_id,
        &accepted.subagent_id,
        &SubagentName::parse("explore").expect("canonical name"),
        committed_at,
    );
    plane
        .store
        .accept_subagent_terminal(Some(notice), draft, event)
        .expect("idempotent retry");
    let second = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("batch");
    assert_eq!(second.items.len(), 2, "exactly once, both sides");
}
