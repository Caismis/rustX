// Included inside registry::tests so the existing deterministic staged-child
// and physical-settlement fixtures remain the single test substrate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_physical_receipt_requires_owner_release_and_durable_proof() {
    use crate::runtime::subagent::physical_recovery::ChildPhysicalLease;
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let admitted = start(&plane, &spec("crash before terminal publication")).await;
    plane.store.arm_fail_accept_times(3);
    child
        .complete(ChildResultStatus::Succeeded, Some("unpublished answer"))
        .await;
    let terminal = plane
        .registry
        .wait_until_settled(&admitted.subagent_id)
        .await
        .unwrap();
    assert_eq!(
        terminal.settlement.publication,
        SubagentPublication::Abandoned
    );
    // The scripted driver's native reap is complete. Reproduce the exact
    // child-owned receipt/lease boundary, parking its writer with an OS lock.
    let spawn = &plane.registry.config.spawn;
    let incarnation = crate::runtime::subagent::child_conversation_store_path(
        spawn.product_root.root(),
        &spawn.session_id,
        &admitted.child_conversation_id,
    )
    .parent()
    .unwrap()
    .join("incarnation-recovery-proof");
    std::fs::create_dir_all(&incarnation).unwrap();
    let lease = ChildPhysicalLease::for_test(
        incarnation.clone(),
        admitted.subagent_id.clone(),
        admitted.child_conversation_id.clone(),
    )
    .unwrap();
    lease.publish_quiescent().unwrap();
    let evidence =
        crate::runtime::recovery::RecoveryEvidence::reconstruct(plane.store.as_ref()).unwrap();
    crate::runtime::recovery::RecoveryPlan::classify(&evidence)
        .reconcile(plane.store.as_ref(), &SystemClock)
        .unwrap();
    let recovered = SubagentRegistry::new(plane.registry.config.clone());
    recovered.restore_agents(plane.store.as_ref()).unwrap();
    assert_eq!(
        recovered.with_goal_idle(|| true),
        None,
        "receipt cannot pass a surviving incarnation's exact exclusive lease"
    );
    assert!(matches!(
        recovered
            .send_message(
                &admitted.child_agent_id,
                "cannot overlap",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new()
            )
            .await,
        Err(AgentControlError::Settlement)
    ));
    assert_eq!(recovered.all_snapshots().len(), 1);
    drop(lease);
    recovered.reconcile_recovered_settlements();
    assert_eq!(recovered.with_goal_idle(|| true), Some(true));
    let (agent, activation) = recovered
        .agent_snapshot_with_activation(&admitted.child_agent_id)
        .unwrap();
    assert_eq!(agent.state, AgentState::Inactive);
    assert_eq!(activation.state, SubagentState::Interrupted);
    assert!(activation.is_settled());
    assert!(
        incarnation.is_dir(),
        "inert physical evidence is retained until Session deletion"
    );
    let workspace = recovered.state.lock().unwrap().agents[&admitted.child_agent_id]
        .workspace
        .clone();
    workspace.acquire().await.unwrap().settle();
    assert_eq!(events(&plane).iter().filter(|event| matches!(event, crate::events::types::RuntimeEvent::SubagentPhysicalSettlementProven { subagent_id, .. } if *subagent_id == admitted.subagent_id)).count(), 1);
    let retry = crate::runtime::subagent::physical_settlement_event(
        &plane.conversation_id,
        &admitted.subagent_id,
        &admitted.child_agent_id,
        Utc::now(),
    );
    let first_receipt = plane.store.append_event(retry.clone()).unwrap();
    let mut later = retry;
    later.timestamp += chrono::Duration::seconds(1);
    assert_eq!(
        plane.store.append_event(later).unwrap(),
        first_receipt,
        "a lost durable acknowledgement retries the exact committed receipt"
    );
    let reopened = SubagentRegistry::new(plane.registry.config.clone());
    reopened.restore_agents(plane.store.as_ref()).unwrap();
    assert_eq!(reopened.with_goal_idle(|| true), Some(true));
    assert_eq!(
        reopened.agent_snapshot(&admitted.child_agent_id).unwrap(),
        agent
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent411_recovery_never_reopens_unproven_physical_containment() {
    for isolated in [false, true] {
        let plane = plane(4);
        if isolated {
            make_clean_git_workspace(&plane);
        }
        let child = stage_with_unresolved_anchor(&plane);
        let mut authority = spec("recover unresolved durable Agent");
        if isolated {
            authority.authority.resolved.workspace_policy =
                crate::runtime::workspace::WorkspacePolicy::GitWorktree {
                    require_clean_parent: true,
                };
        }
        let accepted = start(&plane, &authority).await;
        child
            .complete(ChildResultStatus::Succeeded, Some("unproven answer"))
            .await;
        let terminal = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .unwrap();
        assert_eq!(
            terminal.workspace_resource_state,
            if isolated {
                SubagentWorkspaceResourceState::PreservedUnresolved
            } else {
                SubagentWorkspaceResourceState::None
            }
        );
        assert_eq!(terminal.state, SubagentState::Failed);
        assert!(!terminal.is_settled());
        assert!(
            terminal.settlement.publication != SubagentPublication::Abandoned,
            "a shared containment failure must publish a valid terminal"
        );
        let recovered = SubagentRegistry::new(plane.registry.config.clone());
        recovered.restore_agents(plane.store.as_ref()).unwrap();
        let restored = recovered.agent_snapshot(&accepted.child_agent_id).unwrap();
        assert_eq!(restored.state, super::agents::AgentState::Unavailable);
        let refused = recovered
            .send_message(
                &accepted.child_agent_id,
                "must not restart",
                crate::runtime::subagent::AgentActivationOrigin::ClientControl,
                crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await;
        let error = refused.unwrap_err();
        assert!(
            matches!(error, super::agents::AgentControlError::Settlement),
            "{error}"
        );
        assert_eq!(recovered.list_agents(MAX_AGENT_LIST_LIMIT).agents.len(), 1);
        assert_eq!(
            recovered.all_snapshots().len(),
            1,
            "no second activation commits"
        );
        assert_eq!(
            recovered
                .agent_snapshot(&accepted.child_agent_id)
                .unwrap()
                .latest_activation,
            accepted.subagent_id
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent411_unreconciled_orphan_cannot_recover_resume_authority() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &spec("owned orphan frontier")).await;
    // The admitted child has not completed. This intentionally bypasses the
    // normal RecoveryPlan reconciliation to prove restore fails closed.
    let recovered = SubagentRegistry::new(plane.registry.config.clone());
    recovered.restore_agents(plane.store.as_ref()).unwrap();
    let refused = recovered
        .send_message(
            &accepted.child_agent_id,
            "must not overlap",
            crate::runtime::subagent::AgentActivationOrigin::ClientControl,
            crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await;
    let error = refused.unwrap_err();
    assert!(
        matches!(error, super::agents::AgentControlError::Settlement),
        "{error}"
    );
    assert_eq!(recovered.all_snapshots().len(), 1);
    child
        .complete(ChildResultStatus::Succeeded, Some("settled original"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent411_crash_reconciliation_cannot_invent_physical_resume_proof() {
    for isolated in [false, true] {
        let plane = plane(4);
        if isolated {
            make_clean_git_workspace(&plane);
        }
        let child = stage_exit0(&plane);
        let mut authority = spec("unpublished terminal at crash boundary");
        if isolated {
            authority.authority.resolved.workspace_policy =
                crate::runtime::workspace::WorkspacePolicy::GitWorktree {
                    require_clean_parent: true,
                };
        }
        let accepted = start(&plane, &authority).await;
        // Physically settle the staged process without a committed terminal
        // fact. Recovery is intentionally given only the durable orphan
        // evidence, exactly as after a crash, not this fixture's extra proof.
        plane.store.arm_fail_accept_times(3);
        child
            .complete(ChildResultStatus::Succeeded, Some("not durably reported"))
            .await;
        let abandoned = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .unwrap();
        assert!(abandoned.settlement.publication == SubagentPublication::Abandoned);
        let evidence =
            crate::runtime::recovery::RecoveryEvidence::reconstruct(plane.store.as_ref()).unwrap();
        crate::runtime::recovery::RecoveryPlan::classify(&evidence)
            .reconcile(plane.store.as_ref(), &SystemClock)
            .unwrap();
        let recovered = SubagentRegistry::new(plane.registry.config.clone());
        recovered.restore_agents(plane.store.as_ref()).unwrap();
        let restored = recovered
            .agent_snapshot_with_activation(&accepted.child_agent_id)
            .unwrap();
        assert_eq!(restored.0.state, super::agents::AgentState::Unavailable);
        assert_eq!(restored.1.state, SubagentState::Interrupted);
        assert_eq!(restored.0.conversation_id, accepted.child_conversation_id);
        assert!(
            !matches!(
                restored.1.workspace_resource_state,
                SubagentWorkspaceResourceState::PreservedUnresolved
            ),
            "clean Git/shared inspection alone lacks process containment proof"
        );
        let error = recovered
            .send_message(
                &accepted.child_agent_id,
                "no replacement process",
                crate::runtime::subagent::AgentActivationOrigin::ClientControl,
                crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, super::agents::AgentControlError::Settlement),
            "{error}"
        );
        assert_eq!(recovered.all_snapshots().len(), 1);
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One ordered durable prefix proves all rollback outcomes.
async fn agent411_resume_reservation_recovery_requires_rollback_containment_proof() {
    use crate::events::types::AgentActivationAdmissionPhase;
    for proof in [None, Some(false), Some(true)] {
        let plane = plane(4);
        let child = stage_exit0(&plane);
        let admitted = start(&plane, &spec("settled before resume staging")).await;
        child
            .complete(ChildResultStatus::Succeeded, Some("prior report"))
            .await;
        plane
            .registry
            .wait_until_settled(&admitted.subagent_id)
            .await
            .unwrap();
        let reserved = SubagentId::for_conversation(&plane.conversation_id, 2);
        let origin = AgentActivationOrigin::ClientControl;
        let ownership = plane
            .registry
            .config
            .spawn
            .product_root
            .runtime_ownership_admission()
            .await
            .unwrap();
        plane
            .registry
            .config
            .mailbox
            .commit_agent_activation_admission(
                &ownership,
                super::super::admission_event(
                    &plane.conversation_id,
                    &admitted.child_agent_id,
                    &reserved,
                    &origin,
                    AgentActivationAdmissionPhase::Reserved,
                    Utc::now(),
                ),
            )
            .unwrap();
        let wrong_origin = AgentActivationOrigin::MessageTool {
            tool_call_id: ToolCallId::new("different-origin"),
        };
        assert!(
            plane
                .registry
                .config
                .mailbox
                .commit_agent_activation_admission(
                    &ownership,
                    super::super::admission_event(
                        &plane.conversation_id,
                        &admitted.child_agent_id,
                        &reserved,
                        &wrong_origin,
                        AgentActivationAdmissionPhase::RolledBack {
                            physical_settlement_proven: true,
                        },
                        Utc::now()
                    ),
                )
                .is_err(),
            "rollback cannot retarget the reserved origin"
        );
        if let Some(physical_settlement_proven) = proof {
            plane
                .registry
                .config
                .mailbox
                .commit_agent_activation_admission(
                    &ownership,
                    super::super::admission_event(
                        &plane.conversation_id,
                        &admitted.child_agent_id,
                        &reserved,
                        &origin,
                        AgentActivationAdmissionPhase::RolledBack {
                            physical_settlement_proven,
                        },
                        Utc::now(),
                    ),
                )
                .unwrap();
        }
        let evidence =
            crate::runtime::recovery::RecoveryEvidence::reconstruct(plane.store.as_ref()).unwrap();
        let report = crate::runtime::recovery::RecoveryPlan::classify(&evidence)
            .reconcile(plane.store.as_ref(), &SystemClock)
            .unwrap();
        assert_eq!(
            report.highest_subagent_ordinal(),
            2,
            "reserved IDs cannot be reused after restart"
        );
        let recovered = SubagentRegistry::new(plane.registry.config.clone());
        recovered.restore_agents(plane.store.as_ref()).unwrap();
        let snapshot = recovered.agent_snapshot(&admitted.child_agent_id).unwrap();
        if proof == Some(true) {
            assert_eq!(snapshot.state, super::agents::AgentState::Inactive);
            assert!(snapshot.current_activation.is_none());
        } else {
            assert_eq!(snapshot.state, super::agents::AgentState::Unavailable);
            assert_eq!(snapshot.current_activation, Some(reserved.clone()));
            assert!(matches!(
                recovered.wait_agent(&admitted.child_agent_id).await,
                Err(super::agents::AgentControlError::Settlement)
            ));
        }
        let workspace = recovered.state.lock().unwrap().agents[&admitted.child_agent_id]
            .workspace
            .clone();
        let lease = workspace.acquire().await;
        assert_eq!(
            lease.is_ok(),
            proof == Some(true),
            "rollback proof {proof:?}"
        );
        if let Ok(lease) = lease {
            lease.settle();
        }
        assert_eq!(
            recovered.all_snapshots().len(),
            1,
            "reservation is not a committed activation"
        );
        if proof != Some(true) {
            let spawn = &plane.registry.config.spawn;
            let incarnation = crate::runtime::subagent::child_conversation_store_path(
                spawn.product_root.root(),
                &spawn.session_id,
                &admitted.child_conversation_id,
            )
            .parent()
            .unwrap()
            .join("incarnation-reserved-recovery");
            std::fs::create_dir_all(&incarnation).unwrap();
            let lease = crate::runtime::subagent::physical_recovery::ChildPhysicalLease::for_test(
                incarnation,
                reserved.clone(),
                admitted.child_conversation_id.clone(),
            )
            .unwrap();
            lease.publish_quiescent().unwrap();
            assert_eq!(recovered.with_goal_idle(|| true), None);
            drop(lease);
            recovered.reconcile_recovered_settlements();
            assert_eq!(recovered.with_goal_idle(|| true), Some(true));
            assert_eq!(
                recovered
                    .agent_snapshot(&admitted.child_agent_id)
                    .unwrap()
                    .state,
                AgentState::Inactive
            );
            let rollback = events(&plane).into_iter().find(|event| matches!(event, crate::events::types::RuntimeEvent::AgentActivationAdmission { activation_id, phase: AgentActivationAdmissionPhase::RolledBack { physical_settlement_proven: true }, .. } if activation_id == &reserved)).expect("the recovery owner commits exact proven rollback");
            assert!(matches!(
                rollback,
                crate::events::types::RuntimeEvent::AgentActivationAdmission {
                    origin: AgentActivationOrigin::ClientControl,
                    ..
                }
            ));
            let reopened = SubagentRegistry::new(plane.registry.config.clone());
            reopened.restore_agents(plane.store.as_ref()).unwrap();
            assert!(reopened.unproven_settlements().is_empty());
            assert_eq!(
                reopened.all_snapshots().len(),
                1,
                "reserved activation is never reattached or replayed"
            );
            let evidence =
                crate::runtime::recovery::RecoveryEvidence::reconstruct(plane.store.as_ref())
                    .unwrap();
            assert_eq!(
                crate::runtime::recovery::RecoveryPlan::classify(&evidence)
                    .reconcile(plane.store.as_ref(), &SystemClock)
                    .unwrap()
                    .highest_subagent_ordinal(),
                2
            );
        }
    }
}
