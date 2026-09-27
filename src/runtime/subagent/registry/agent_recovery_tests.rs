// Included inside registry::tests so the existing deterministic staged-child
// and physical-settlement fixtures remain the single test substrate.
#[tokio::test]
async fn initial_uncommitted_physical_authority_consumes_activation_identity_on_restart() {
    let plane = plane(4);
    let orphaned_conversation = ConversationId::from_uuid(uuid::Uuid::now_v7()).unwrap();
    let consumed = SubagentId::for_conversation(&plane.conversation_id, 1);
    let spawn = &plane.registry.config.spawn;
    let owner = super::super::physical_recovery::ParentPhysicalLease::reserve(
        &spawn.product_root,
        &spawn.session_id,
        &orphaned_conversation,
        &consumed,
    )
    .unwrap();
    assert!(events(&plane).is_empty(), "ownership has not committed");

    // No ownership event names this child Conversation. The positive durable
    // allocation fact must still prevent reuse in the parent's ordinal domain.
    let recovered = SubagentRegistry::new(plane.registry.config.clone());
    recovered.restore_agents(plane.store.as_ref()).unwrap();
    assert!(recovered.all_snapshots().is_empty());
    assert!(recovered.list_agents(MAX_AGENT_LIST_LIMIT).agents.is_empty());
    assert_eq!(recovered.state.lock().unwrap().next_ordinal, 2);
    assert!(events(&plane).is_empty(), "allocation invents no logical ownership");
    assert!(recovered.owns_idle_work());
    assert_eq!(recovered.unproven_settlements(), vec![consumed.clone()]);
    assert_eq!(recovered.with_goal_idle(|| true), None);
    drop(owner);
    drop(super::super::physical_recovery::prove_after_release(
        &spawn.product_root, &spawn.session_id, &orphaned_conversation, &consumed,
    ).unwrap().unwrap());
    recovered.reconcile_recovered_settlements();
    assert!(!recovered.owns_idle_work());
    assert!(recovered.unproven_settlements().is_empty());
    assert_eq!(recovered.with_goal_idle(|| true), Some(true));
    let version = *recovered.state_version.borrow();
    recovered.reconcile_recovered_settlements();
    assert_eq!(*recovered.state_version.borrow(), version);
    assert_eq!(recovered.state.lock().unwrap().next_ordinal, 2);
    assert!(events(&plane).is_empty(), "physical proof invents no logical ownership");
    assert!(
        super::super::physical_recovery::ParentPhysicalLease::reserve(
            &spawn.product_root,
            &spawn.session_id,
            &orphaned_conversation,
            &consumed,
        )
        .is_err(),
        "the exact consumed namespace cannot be overwritten"
    );
}

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
    .join("physical-settlement").join(admitted.subagent_id.as_str());
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
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::sync_channel(0);
    recovered.state.lock().unwrap().recovery_probe_hook = Some(Box::new(move || {
        entered.send(()).unwrap();
        released.recv().unwrap();
    }));
    // The startup reconciler owns the exact claim before native proof becomes available.
    entry.await.unwrap();
    assert!(recovered.state.try_lock().is_ok());
    drop(lease);
    drop(crate::runtime::subagent::physical_recovery::prove_after_release(
        &spawn.product_root, &spawn.session_id, &admitted.child_conversation_id, &admitted.subagent_id,
    ).unwrap().expect("the exact inherited lease is released"));
    recovered.reconcile_recovered_settlements();
    assert_eq!(recovered.with_goal_idle(|| true), None, "native proof alone has not crossed the durable/in-memory settlement cut");
    assert_eq!(events(&plane).iter().filter(|event| matches!(event, crate::events::types::RuntimeEvent::SubagentPhysicalSettlementProven { subagent_id, .. } if *subagent_id == admitted.subagent_id)).count(), 0);
    release.send(()).unwrap();
    recovered.wait_recovery_reconciliation().await;
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
    workspace.acquire(&CancellationSignal::new()).await.unwrap().settle();
    assert_eq!(events(&plane).iter().filter(|event| matches!(event, crate::events::types::RuntimeEvent::SubagentPhysicalSettlementProven { subagent_id, .. } if *subagent_id == admitted.subagent_id)).count(), 1);
    assert_physical_receipt_retry_is_idempotent(&plane, &admitted);
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
        assert_eq!(abandoned.settlement.publication, SubagentPublication::Abandoned);
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
        let plane = plane_with_storage(4, true);
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
        let lease = workspace.acquire(&CancellationSignal::new()).await;
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
            .join("physical-settlement").join(reserved.as_str());
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
            let proof = crate::runtime::subagent::physical_recovery::prove_after_release(
                &spawn.product_root, &spawn.session_id, &admitted.child_conversation_id, &reserved,
            ).unwrap().expect("the exact inherited lease is released");
            // A fork between proof acquisition and drop retains the CLOEXEC
            // descriptors until exec/exit. Park that exact window on a pipe.
            let inherited = InheritedRecoveryProof::hold(proof);
            assert!(crate::runtime::subagent::physical_recovery::prove(
                &spawn.product_root, &spawn.session_id, &admitted.child_conversation_id, &reserved,
            ).unwrap().is_none(), "the fork still owns the inherited proof lock");
            recovered.reconcile_recovered_settlements();
            {
                let state = recovered.state.lock().unwrap();
                assert!(state.recovery_pending.contains(&reserved));
                assert!(state.recovery_inflight.is_empty(), "the failed probe released its claim");
                let agent = &state.agents[&admitted.child_agent_id];
                assert_eq!(agent.resuming.as_ref().unwrap().activation_id, reserved);
                assert!(agent.workspace.is_poisoned());
                assert!(!state.goal_idle_committing);
                assert!(state.ownership_committing.is_empty());
            }
            assert!(recovered.all_snapshots()[0].is_settled(), "the old activation is not the idle blocker");
            assert!(!events(&plane).iter().any(|event| matches!(event,
                crate::events::types::RuntimeEvent::AgentActivationAdmission {
                    activation_id, phase: AgentActivationAdmissionPhase::RolledBack { physical_settlement_proven: true }, ..
                } if activation_id == &reserved)), "child release and an external proof are not durable rollback");
            assert_eq!(recovered.with_goal_idle(|| true), None);
            drop(inherited);
            // Descriptor release is not the registry's durable settlement cut.
            // Join the existing recovery owner; never require its next Try probe
            // to win against a concurrent fork inheriting a CLOEXEC descriptor.
            recovered.wait_recovery_reconciliation().await;
            assert!(recovered.unproven_settlements().is_empty());
            {
                let state = recovered.state.lock().unwrap();
                assert!(state.recovery_inflight.is_empty());
                assert!(state.agents[&admitted.child_agent_id].resuming.is_none());
            }
            assert!(!workspace.is_poisoned());
            assert_eq!(events(&plane).iter().filter(|event| matches!(event,
                crate::events::types::RuntimeEvent::AgentActivationAdmission {
                    activation_id, phase: AgentActivationAdmissionPhase::RolledBack { physical_settlement_proven: true }, ..
                } if activation_id == &reserved)).count(), 1);
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
            let reopened_store = Arc::new(crate::durable::SqliteConversationStore::open(
                plane.conversation_id.clone(), &plane.dir.path().join("parent.sqlite"),
            ).unwrap());
            let mut config = plane.registry.config.clone();
            config.mailbox = ConversationInboundMailbox::over_store(reopened_store.clone());
            let reopened = SubagentRegistry::new(config);
            reopened.restore_agents(reopened_store.as_ref()).unwrap();
            assert!(reopened.unproven_settlements().is_empty());
            assert_eq!(reopened.with_goal_idle(|| true), Some(true));
            let agent = reopened.agent_snapshot(&admitted.child_agent_id).unwrap();
            assert_eq!(agent.state, AgentState::Inactive);
            assert!(agent.current_activation.is_none());
            assert_eq!(reopened.state.lock().unwrap().next_ordinal, 3);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One real crash, recovery and physical drain boundary.
async fn recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop() {
    use crate::runtime::subagent::physical_recovery::{ParentPhysicalLease, prove_after_release};
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    for crash in [false, true] {
    let plane = plane_with_storage(4, true);
    make_clean_git_workspace(&plane);
    let child = stage_exit0(&plane);
    let mut authority = spec("durable isolated Agent");
    authority.authority.resolved.workspace_policy = crate::runtime::workspace::WorkspacePolicy::GitWorktree {
        require_clean_parent: true,
    };
    let admitted = start(&plane, &authority).await;
    child.complete(ChildResultStatus::Succeeded, Some("done")).await;
    plane.registry.wait_until_settled(&admitted.subagent_id).await.unwrap();
    let mut config = plane.registry.config.clone();
    config.workspace = WorkspaceManager::new(plane.dir.path().join("workspace"), &plane.runtime_root);
    let recovered = SubagentRegistry::new(config.clone());
    recovered.restore_agents(plane.store.as_ref()).unwrap();
    let activation = SubagentId::for_conversation(&plane.conversation_id, 2);
    let spawn = &plane.registry.config.spawn;
    let ready = plane.dir.path().join("git-ready");
    let release = plane.dir.path().join("git-release");
    assert!(std::process::Command::new("mkfifo").args([&ready, &release]).status().unwrap().success());
    let armed = plane.dir.path().join("git-armed");
    std::fs::write(&armed, []).unwrap();
    let hook = plane.dir.path().join("fsmonitor");
    std::fs::write(&hook, format!(
        "#!/bin/sh\nif [ -f '{}' ]; then\nrm '{}'\nprintf R > '{}'\nread release < '{}'\nfi\nprintf 'token\\0/\\0'\n",
        armed.display(), armed.display(), ready.display(), release.display(),
    )).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    git(&plane.dir.path().join("workspace"), &["config", "core.fsmonitor", hook.to_str().unwrap()]);
    let executing = tokio::task::spawn_blocking(move || {
        let mut byte = [0];
        std::fs::File::open(ready).unwrap().read_exact(&mut byte).unwrap();
        assert_eq!(byte, [b'R']);
    });
    let mut send = Box::pin(recovered.send_message(&admitted.child_agent_id, "resume verification",
        AgentActivationOrigin::ClientControl, CancellationSignal::new()));
    assert!(futures_util::poll!(&mut send).is_pending());
    executing.await.unwrap();
    // Real recovered-workspace Git status is executing its fsmonitor helper,
    // blocked on an explicit pipe. Reserved has not committed.
    assert!(!events(&plane).iter().any(|event| matches!(event,
        crate::events::types::RuntimeEvent::AgentActivationAdmission { activation_id, .. } if activation_id == &activation)));
    if !crash {
        let mut interrupt = Box::pin(recovered.interrupt_agent(&admitted.child_agent_id));
        assert!(futures_util::poll!(&mut interrupt).is_pending());
        assert!(matches!(send.await, Err(AgentControlError::Start(SubagentStartError::Cancelled))));
        assert!(interrupt.await.unwrap().outcome.is_none());
        assert_eq!(recovered.agent_snapshot(&admitted.child_agent_id).unwrap().state, AgentState::Inactive);
        assert_eq!(recovered.all_snapshots().len(), 1);
        // No release byte was sent: the activation signal settled real Git.
        continue;
    }
    recovered.state.lock().unwrap().resume_owner_abort.take().unwrap().abort();
    assert!(matches!(send.await, Err(AgentControlError::Admission(_))));
    drop(recovered);
    let store = Arc::new(crate::durable::SqliteConversationStore::open(
        plane.conversation_id.clone(), &plane.dir.path().join("parent.sqlite"),
    ).unwrap());
    config.mailbox = ConversationInboundMailbox::over_store(store.clone());
    config.workspace = WorkspaceManager::new(plane.dir.path().join("workspace"), &plane.runtime_root);
    let reopened = SubagentRegistry::new(config);
    reopened.restore_agents(store.as_ref()).unwrap();
    let blocked = reopened.send_message(&admitted.child_agent_id, "blocked",
        AgentActivationOrigin::ClientControl, CancellationSignal::new()).await;
    assert!(matches!(blocked, Err(AgentControlError::Settlement)), "expected physical exclusion: {blocked:?}");
    assert_eq!(reopened.state.lock().unwrap().next_ordinal, 3);
    assert!(reopened.owns_idle_work());
    assert_eq!(reopened.unproven_settlements(), vec![activation.clone()]);
    assert_eq!(reopened.with_goal_idle(|| true), None);
    assert_eq!(reopened.agent_snapshot(&admitted.child_agent_id).unwrap().state, AgentState::Unavailable);
    let (runtime, reopened) = crate::runtime::conversation_runtime::runtime_with_recovered_registry_for_test(
        &plane.dir, &plane.conversation_id, &plane.registry.config.agent_id,
        store.clone(), reopened.clone(),
    ).await;
    // Awaiting the bounded reconciliation owner is deliberately not proof:
    // the FIFO still owns the helper and no release byte has been sent.
    assert!(runtime.settle_child_physical_lifetime(false).await.is_err());
    assert!(runtime.shutdown().await.is_err());
    assert!(!runtime.is_quiescent());
    assert!(reopened.owns_idle_work());
    assert_eq!(reopened.unproven_settlements(), vec![activation.clone()]);

    std::fs::OpenOptions::new().write(true).open(release).unwrap().write_all(b"release\n").unwrap();
    // The kernel lease release and exact supervisor receipts, never PID absence,
    // establish the physical cut before reconciliation can release the workspace.
    drop(prove_after_release(&spawn.product_root, &spawn.session_id,
        &admitted.child_conversation_id, &activation).unwrap().unwrap());
    // Shutdown joined the previous bounded owner while the helper was held.
    // Join a fresh owner after release rather than assuming the next Try probe
    // can reacquire a descriptor that a concurrent fork may still have inherited.
    reopened.start_recovery_reconciliation();
    reopened.wait_recovery_reconciliation().await;
    assert_eq!(reopened.agent_snapshot(&admitted.child_agent_id).unwrap().state, AgentState::Inactive);
    assert!(!reopened.owns_idle_work());
    assert!(reopened.unproven_settlements().is_empty());
    assert_eq!(reopened.with_goal_idle(|| true), Some(true));
    let (settled_runtime, _) = crate::runtime::conversation_runtime::runtime_with_recovered_registry_for_test(
        &plane.dir, &plane.conversation_id, &plane.registry.config.agent_id,
        store.clone(), reopened.clone(),
    ).await;
    settled_runtime.settle_child_physical_lifetime(false).await.unwrap();
    settled_runtime.shutdown().await.unwrap();
    assert!(settled_runtime.is_quiescent());
    assert_eq!(reopened.state.lock().unwrap().next_ordinal, 3);
    let journal = events(&plane);
    let version = *reopened.state_version.borrow();
    reopened.reconcile_recovered_settlements();
    assert_eq!(*reopened.state_version.borrow(), version);
    assert_eq!(events(&plane), journal);
    let workspace = reopened.state.lock().unwrap().agents[&admitted.child_agent_id].workspace.clone();
    workspace.acquire(&CancellationSignal::new()).await.unwrap().settle();
    assert!(ParentPhysicalLease::reserve(&spawn.product_root, &spawn.session_id,
        &admitted.child_conversation_id, &activation).is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_authority_publication_reopens_without_logical_facts() {
    use super::super::physical_recovery::{AllocationBoundary, ParentPhysicalLease};
    for boundary in [AllocationBoundary::Created, AllocationBoundary::LeaseCreated,
        AllocationBoundary::Initialized, AllocationBoundary::Published] {
        let plane = plane_with_storage(4, true);
        let spawn = &plane.registry.config.spawn;
        let child = ConversationId::generate();
        let activation = SubagentId::for_conversation(&plane.conversation_id, 1);
        let result = ParentPhysicalLease::reserve_at_boundaries(
            &spawn.product_root, &spawn.session_id, &child, &activation,
            |at| if at == boundary { Err(std::io::Error::other("injected initialization failure")) } else { Ok(()) },
        );
        assert!(result.is_err(), "no child/helper authority escaped at {boundary:?}");
        for _ in 0..2 {
            let store = Arc::new(crate::durable::SqliteConversationStore::open(
                plane.conversation_id.clone(), &plane.dir.path().join("parent.sqlite"),
            ).unwrap());
            let mut config = plane.registry.config.clone();
            config.mailbox = ConversationInboundMailbox::over_store(store.clone());
            let recovered = SubagentRegistry::new(config);
            recovered.restore_agents(store.as_ref()).unwrap();
            recovered.wait_recovery_reconciliation().await;
            assert_eq!(recovered.state.lock().unwrap().next_ordinal, 2);
            assert!(recovered.list_agents(MAX_AGENT_LIST_LIMIT).agents.is_empty());
            assert!(recovered.all_snapshots().is_empty());
            assert!(!recovered.owns_idle_work());
            assert!(recovered.unproven_settlements().is_empty());
            assert_eq!(recovered.with_goal_idle(|| true), Some(true));
            assert!(store.read_events(None, 128).unwrap().events.is_empty());
            let version = *recovered.state_version.borrow();
            recovered.reconcile_recovered_settlements();
            assert_eq!(*recovered.state_version.borrow(), version);
            let (runtime, _) = crate::runtime::conversation_runtime::runtime_with_recovered_registry_for_test(
                &plane.dir, &plane.conversation_id, &plane.registry.config.agent_id, store, recovered,
            ).await;
            runtime.settle_child_physical_lifetime(false).await.unwrap();
            runtime.shutdown().await.unwrap();
            assert!(runtime.is_quiescent());
        }
        assert!(ParentPhysicalLease::reserve(&spawn.product_root, &spawn.session_id, &child, &activation).is_err());
        let next = SubagentId::for_conversation(&plane.conversation_id, 2);
        drop(ParentPhysicalLease::reserve(&spawn.product_root, &spawn.session_id, &child, &next).unwrap());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persistent_child_result_belongs_to_concluding_terminal_attempt() {
    use crate::local_runtime::subagent_child::tests::persistent_answer_activation;
    for mode in ["refusal", "B", "narration", "guidance"] {
        let plane = plane_with_storage(4, true);
        let first_child = stage_exit0(&plane);
        let first = start(&plane, &start_spec("first activation")).await;
        let first_result = persistent_answer_activation(&plane.dir, first.child_conversation_id.clone(), "A").await;
        assert_eq!(first_result.status, ChildResultStatus::Succeeded);
        assert_eq!(first_result.content.as_deref(), Some("A"));
        first_child.complete(first_result.status, first_result.content.as_deref()).await;
        plane.registry.wait_until_settled(&first.subagent_id).await.unwrap();
        let mut child = stage_exit0(&plane);
        let (resumed, _) = tokio::join!(plane.registry.send_message(&first.child_agent_id, "later task", AgentActivationOrigin::ClientControl, CancellationSignal::new()), child.accept_delegate());
        let resumed = resumed.unwrap();
        assert_eq!(resumed.agent_id, first.child_agent_id);
        assert_ne!(resumed.activation_id, first.subagent_id);
        let current = plane.registry.snapshot(&resumed.activation_id).unwrap();
        assert_eq!(current.child_conversation_id, first.child_conversation_id);
        let result = persistent_answer_activation(&plane.dir, current.child_conversation_id, mode).await;
        let expected = match mode { "B" => Some("B"), "guidance" => Some("Concluding answer"), _ => None };
        assert_eq!(result.content.as_deref(), expected);
        assert_eq!(result.status, if expected.is_some() { ChildResultStatus::Succeeded } else { ChildResultStatus::Failed });
        if expected.is_none() { assert_eq!(result.diagnostic.as_deref(), Some("the attempt completed without a final answer")); }
        super::super::ipc::write_child_frame(&mut child.peer, &ChildFrame::Result(result)).await.unwrap();
        let terminal = plane.registry.wait_until_settled(&resumed.activation_id).await.unwrap();
        assert_eq!(terminal.state, if expected.is_some() { SubagentState::Succeeded } else { SubagentState::Failed });
        let pending = plane.store.select_pending_batch().unwrap().unwrap();
        let report = pending.items.iter().find(|item| item.correlation.as_deref() == Some(super::super::terminal_correlation(&resumed.activation_id).as_str())).unwrap();
        let text = report.message.content.iter().filter_map(|block| match block { crate::message::types::UserContentBlock::Text(text) => Some(text.text.as_str()), _ => None }).collect::<String>();
        assert!(text.contains(expected.unwrap_or("the attempt completed without a final answer")));
        assert_eq!(events(&plane).iter().filter(|event| matches!(event, crate::events::types::RuntimeEvent::SubagentTerminalPublished { subagent_id, .. } if *subagent_id == resumed.activation_id)).count(), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn published_initialization_error_retains_live_registry_obligation_until_exact_proof() {
    use super::super::physical_recovery::{AllocationBoundary, ParentPhysicalLease};
    let plane = plane_with_storage(4, true);
    let spawn = &plane.registry.config.spawn;
    let child = ConversationId::generate();
    let activation = SubagentId::for_conversation(&plane.conversation_id, 1);
    let path = super::super::child_conversation_store_path(spawn.product_root.root(), &spawn.session_id, &child)
        .parent().unwrap().join("physical-settlement").join(activation.as_str()).join("physical-settlement.json");
    let receipt = std::sync::Mutex::new(Vec::new());
    let allocation = ParentPhysicalLease::reserve_at_boundaries(&spawn.product_root, &spawn.session_id, &child, &activation, |at| {
        if at != AllocationBoundary::Published { return Ok(()); }
        *receipt.lock().unwrap() = std::fs::read(&path)?;
        std::fs::write(&path, b"corrupt published evidence")?;
        Err(std::io::Error::other("failure after publication"))
    });
    assert!(plane.registry.finish_physical_reservation(&child, &activation, allocation).is_err());
    assert!(plane.registry.owns_idle_work());
    assert_eq!(plane.registry.with_goal_idle(|| true), None);
    assert_eq!(plane.registry.unproven_settlements(), vec![activation.clone()]);
    assert!(events(&plane).is_empty());
    let reopened = SubagentRegistry::new(plane.registry.config.clone());
    reopened.restore_agents(plane.store.as_ref()).unwrap();
    assert_eq!(reopened.unproven_settlements(), vec![activation]);
    // Restore the exact original authority receipt, never a heuristic proof.
    std::fs::write(path, &*receipt.lock().unwrap()).unwrap();
    plane.registry.wait_recovery_reconciliation().await;
    reopened.wait_recovery_reconciliation().await;
    assert_eq!(plane.registry.with_goal_idle(|| true), Some(true));
    assert_eq!(reopened.with_goal_idle(|| true), Some(true));
    assert!(events(&plane).is_empty());
}

fn assert_physical_receipt_retry_is_idempotent(plane: &TestPlane, admitted: &SubagentAccepted) {
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
}

/// Retain a proof's open-file descriptions in a real fork until the test releases
/// its pipe. No child Rust runtime, allocator, unwinding or competing waiter runs.
struct InheritedRecoveryProof {
    pid: nix::unistd::Pid,
    release: Option<std::os::unix::net::UnixStream>,
}

impl InheritedRecoveryProof {
    #[allow(unsafe_code)] // Test-local fork; child uses only async-signal-safe syscalls.
    fn hold(proof: crate::runtime::subagent::physical_recovery::RecoveredPhysicalProof) -> Self {
        use std::os::fd::AsRawFd;
        let (release, child) = std::os::unix::net::UnixStream::pair().unwrap();
        let release_fd = release.as_raw_fd();
        let child_fd = child.as_raw_fd();
        // SAFETY: all allocations precede fork. The child only closes its write
        // endpoint, reads until parent EOF, and _exits without Rust destructors.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork: {}", std::io::Error::last_os_error());
        if pid == 0 {
            unsafe {
                libc::close(release_fd);
                let mut byte = 0u8;
                loop {
                    let read = libc::read(child_fd, (&raw mut byte).cast::<libc::c_void>(), 1);
                    if read < 0 && nix::errno::Errno::last() == nix::errno::Errno::EINTR {
                        continue;
                    }
                    libc::_exit(i32::from(read != 0));
                }
            }
        }
        drop(child);
        let inherited = Self { pid: nix::unistd::Pid::from_raw(pid), release: Some(release) };
        drop(proof);
        inherited
    }
}

impl Drop for InheritedRecoveryProof {
    fn drop(&mut self) {
        // EOF releases the parked child even when an assertion unwinds. This
        // guard alone owns its terminal status and joins descriptor release.
        drop(self.release.take());
        loop {
            match nix::sys::wait::waitpid(self.pid, None) {
                Err(nix::errno::Errno::EINTR) => {}
                result => {
                    assert_eq!(result.unwrap(), nix::sys::wait::WaitStatus::Exited(self.pid, 0));
                    break;
                }
            }
        }
    }
}
