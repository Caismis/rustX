//! Actual manager/native runtime execution, no synthetic Human request.
use super::*;
use crate::runtime::workflow::scheduled::lifecycle::*;

async fn finished(
    f: &Fixture,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    id: &ScheduledWorkflowStartId,
) -> ScheduledWorkflowRecovery {
    loop {
        let done = runtime.settlement_signal().notified();
        tokio::pin!(done);
        done.as_mut().enable();
        let view = f
            .manager
            .scheduled_workflow_recovery(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                id.clone(),
                0,
                100,
            )
            .await
            .unwrap()
            .unwrap();
        if matches!(view.start.state, ScheduledWorkflowState::Settled { .. }) {
            return view;
        }
        done.await;
    }
}
async fn scheduled(f: &Fixture) -> ScheduleCommandReceipt {
    receipt(
        f.manager
            .schedule_command(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleCommand {
                    command_id: "native-workflow".into(),
                    mutation: ScheduleMutation::CreateWorkflow {
                        title: "Native scheduled run".into(),
                        workflow_id: crate::runtime::workflow::WorkflowId::parse("scheduled")
                            .unwrap(),
                        arguments: serde_json::json!({}),
                        timing: ScheduleTiming::After { seconds: 60 },
                    },
                },
            )
            .await
            .unwrap(),
    )
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_native_runtime_executes_pure_workflow_once_without_an_attempt() {
    bounded(async {
        let mut f=Fixture::with_session_count(None,1).await;f.install_scheduled_workflow("Native execution");
        let clock=Clock::new();f.manager.schedule_clock=clock.clone();
        let resident=f.load(0).await.unwrap().unwrap();let runtime=resident.inspect_runtime().unwrap();let created=scheduled(&f).await;
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted{start,..})=f.manager.schedule_operation(&f.sessions[0].id,&f.sessions[0].active_node,ScheduleOperation::Admit{id:created.id.clone(),expected_revision:created.revision}).await.unwrap() else {panic!("native target admission")};
        let view=finished(&f,&runtime,&start.id).await;
        assert!(matches!(view.start.state,ScheduledWorkflowState::Settled{outcome:ScheduledWorkflowOutcome::Completed,..}));
        assert_eq!(view.result,Some(serde_json::json!({"answer":"native durable return"})));
        assert!(!view.capacity_blocked && !view.deletion_blocked);
        let store=runtime.tool_runtime().durable_store();let events=store.read_events(None,256).unwrap().events;
        assert_eq!(events.iter().filter(|event|matches!(&event.event,RuntimeEvent::WorkflowStarted{run_id,tool_call_id:None,..} if run_id==&start.run_id)).count(),1);
        assert!(events.iter().all(|event|!matches!(event.event,RuntimeEvent::AttemptStarted{..})));
        assert!(store.load_pending().unwrap().is_empty());
        assert!(matches!(f.manager.schedule_operation(&f.sessions[0].id,&f.sessions[0].active_node,ScheduleOperation::Admit{id:created.id,expected_revision:1}).await.unwrap(),ScheduleOperationResult::Admission(ScheduleAdmission::AlreadyAdvanced{..})));
        f.close().await;
    }).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_cold_recovery_reads_and_cancellation_do_not_activate_or_refire() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        f.install_scheduled_workflow("Cold native execution");
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();
        let resident = f.load(0).await.unwrap().unwrap();
        let created = scheduled(&f).await;
        drop(resident);
        let conversation = f.id(0).await;
        f.manager.unload(&conversation).await.unwrap();
        let composed = f
            .manager
            .probe(&conversation)
            .compositions
            .load(Ordering::SeqCst);
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start, ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created.id.clone(),
                    expected_revision: 1,
                },
            )
            .await
            .unwrap()
        else {
            panic!("cold Workflow acceptance")
        };
        let before = f
            .manager
            .scheduled_workflow_recovery(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                start.id.clone(),
                0,
                100,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            before.start.state,
            ScheduledWorkflowState::Accepted { .. }
        ));
        let cancelled = f
            .manager
            .cancel_scheduled_workflow(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                start.id.clone(),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            cancelled.start.state,
            ScheduledWorkflowState::Settled {
                outcome: ScheduledWorkflowOutcome::Cancelled,
                ..
            }
        ));
        let repeated = f
            .manager
            .reconcile_scheduled_workflow(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                start.id.clone(),
                0,
                100,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repeated.start, cancelled.start);
        assert_eq!(
            f.manager
                .probe(&conversation)
                .compositions
                .load(Ordering::SeqCst),
            composed
        );
        assert!(matches!(
            f.manager
                .schedule_operation(
                    &f.sessions[0].id,
                    &f.sessions[0].active_node,
                    ScheduleOperation::Admit {
                        id: created.id,
                        expected_revision: 1
                    }
                )
                .await
                .unwrap(),
            ScheduleOperationResult::Admission(ScheduleAdmission::AlreadyAdvanced { .. })
        ));
        f.close().await;
    })
    .await;
}

fn review_definition(timeout: u64) -> serde_json::Value {
    let result = serde_json::json!({"type":"object","properties":{"accepted":{"type":"boolean"},"feedback":{"type":"string"}},"required":["accepted","feedback"],"additionalProperties":false});
    serde_json::json!({"description":"Native Scheduled Review","timeout_ms":timeout,"block":{"input":{"type":"object","properties":{},"additionalProperties":false},"output":result,"entry":"human","nodes":{"human":{"type":"review","subject":{"type":"plan","value":{"type":"reference","path":["args"]}},"context":[]},"done":{"type":"return","output":{"type":"reference","path":["human"]}}},"edges":[{"from":"human","to":"done"}]}})
}
async fn scheduled_review(mode: u8) {
    bounded(async {
        let mut f=Fixture::with_session_count(None,1).await;
        f.install_scheduled_workflow_definition(&review_definition(if mode==2 {1000} else {30_000}));
        let clock=Clock::new();f.manager.schedule_clock=clock.clone();
        let resident=f.load(0).await.unwrap().unwrap();let runtime=resident.inspect_runtime().unwrap();
        let owner=runtime.interaction_test_owner();owner.set_provider_available(true);
        let published=owner.pending_published.notified();
        tokio::pin!(published);published.as_mut().enable();
        let created=scheduled(&f).await;clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted{start,..})=f.manager.schedule_operation(&f.sessions[0].id,&f.sessions[0].active_node,ScheduleOperation::Admit{id:created.id,expected_revision:1}).await.unwrap() else {panic!("Scheduled Review admission")};
        published.await;let request=owner.pending_snapshot().into_iter().next().expect("native pending Review");
        assert!(matches!(&request.execution,crate::runtime::interaction::InteractionExecution::Scheduled{run} if run==&start.run_id));
        assert!(request.id.as_str().starts_with(&format!("scheduled:{}:",start.id)));
        assert!(!runtime.is_quiescent());
        if mode==0 {
            let crate::runtime::interaction::InteractionKind::Review{review,subject_digest}=&request.kind else {panic!("native Review")};
            owner.respond_async(&request.id,crate::runtime::interaction::InteractionResponse::Review{response:crate::events::review::ReviewResponse{instance:review.instance.clone(),subject_digest:subject_digest.clone(),decision:crate::events::review::ReviewDecision::Accepted}}).await.unwrap();
        } else if mode==1 {
            let cancelled=f.manager.cancel_scheduled_workflow(&f.sessions[0].id,&f.sessions[0].active_node,start.id.clone()).await.unwrap().unwrap();
            assert!(cancelled.start.cancellation_requested_at.is_some());
        }
        let view=finished(&f,&runtime,&start.id).await;
        let ScheduledWorkflowState::Settled{outcome,..}=view.start.state else {panic!("native physical settlement")};
        match mode {0=>assert_eq!(outcome,ScheduledWorkflowOutcome::Completed),1=>assert_eq!(outcome,ScheduledWorkflowOutcome::Cancelled),_=>assert_eq!(outcome,ScheduledWorkflowOutcome::TimedOut)}
        assert_eq!(view.effects.len(),1);assert!(view.effects[0].physical_settlement_proven);
        assert!(owner.pending_snapshot().is_empty());
        assert!(events(&runtime).iter().all(|event|!matches!(event,RuntimeEvent::AttemptStarted{..})));
        f.close().await;
    }).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_scheduled_review_has_native_identity_and_committed_return() {
    scheduled_review(0).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_live_scheduled_cancel_closes_review_before_releasing_ownership() {
    scheduled_review(1).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_total_deadline_settles_scheduled_review_without_an_attempt() {
    scheduled_review(2).await;
}

async fn reopened_workflow(claim_before_reopen: bool) {
    use crate::durable::ScheduledWorkflowStore;
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        f.install_scheduled_workflow("Before interruption");
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();
        let resident = f.load(0).await.unwrap().unwrap();
        let created = scheduled(&f).await;
        drop(resident);
        let conversation = f.id(0).await;
        f.manager.unload(&conversation).await.unwrap();
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start, ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created.id,
                    expected_revision: 1,
                },
            )
            .await
            .unwrap()
        else {
            panic!("cold acceptance")
        };
        if claim_before_reopen {
            let access = f
                .manager
                .sessions
                .acquire_session(&f.sessions[0].id, None)
                .await
                .unwrap();
            let controller = f
                .manager
                .sessions
                .catalog
                .lock()
                .await
                .controller()
                .unwrap();
            let store = crate::durable::SqliteConversationStore::recover_existing(
                conversation.clone(),
                &access.database_path,
                &controller,
            )
            .unwrap();
            let accepted = store.scheduled_workflow_get(&start.id).unwrap().unwrap();
            let claim = store
                .scheduled_workflow_claim(
                    &start.id,
                    accepted.contract.as_ref().unwrap(),
                    "interrupted-native-owner",
                    clock.now(),
                )
                .unwrap()
                .unwrap();
            drop(claim);
            drop(store);
            drop(access);
        } else {
            let mut changed = review_definition(30_000);
            changed["description"] = serde_json::json!("Changed after acceptance");
            f.install_scheduled_workflow_definition(&changed);
            f.manager
                .reconcile_configuration(
                    &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                        directory: f.workspaces[0].clone(),
                    },
                )
                .await
                .unwrap();
            // Host restart retires both process-local source snapshots and
            // adopted bindings before native resolution reads changed files.
            *f.manager.applications.lock() =
                crate::local_runtime::configuration::application::ApplicationState::default();
            f.manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()
                .remove(&f.sessions[0].id);
        }
        let resident = f.load(0).await.unwrap().unwrap();
        let runtime = resident.inspect_runtime().unwrap();
        let view = finished(&f, &runtime, &start.id).await;
        assert_eq!(
            matches!(
                view.start.state,
                ScheduledWorkflowState::Settled {
                    outcome: ScheduledWorkflowOutcome::OutcomeUnknown { .. },
                    ..
                }
            ),
            claim_before_reopen
        );
        if !claim_before_reopen {
            assert!(
                matches!(
                    view.start.state,
                    ScheduledWorkflowState::Settled {
                        outcome: ScheduledWorkflowOutcome::RejectedBeforeStart { .. },
                        ..
                    }
                ),
                "{:?}",
                view.start.state
            );
        }
        assert!(view.effects.is_empty());
        assert!(!view.capacity_blocked && !view.deletion_blocked);
        assert!(events(&runtime).iter().all(|event| !matches!(
            event,
            RuntimeEvent::AttemptStarted { .. } | RuntimeEvent::WorkflowStarted { .. }
        )));
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_reopened_claim_retires_without_executing_the_graph() {
    reopened_workflow(true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_changed_accepted_program_is_durably_rejected_before_execution() {
    reopened_workflow(false).await;
}

async fn scheduled_native_effect(kind: ScheduledEffectKind) {
    bounded(async {
        let mut f=Fixture::with_session_count(if kind==ScheduledEffectKind::Agent {Some("review")} else {None},1).await;
        f.gates[0].release();
        let empty=serde_json::json!({"type":"object","properties":{},"additionalProperties":false});
        let (node,output)=match kind {
            ScheduledEffectKind::Tool=>(serde_json::json!({"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":{"type":"literal","value":{"path":"rustx.toml"}},"result":{"type":"text","part":0}}),serde_json::json!({"type":"string"})),
            ScheduledEffectKind::Model=>(serde_json::json!({"type":"model","model":{"mode":"session"},"instruction":"Return done","input":{},"output":{"type":"text"}}),serde_json::json!({"type":"string"})),
            ScheduledEffectKind::Agent=>(serde_json::json!({"type":"agent","profile":"reviewer","task":"Return the Workflow output","output":empty}),empty.clone()),
            ScheduledEffectKind::Review=>unreachable!("Review has dedicated response/cancel/deadline tests"),
        };
        let returned=if kind==ScheduledEffectKind::Agent {serde_json::json!({"type":"reference","path":["work"]})}else{serde_json::json!({"type":"object","fields":{"value":{"type":"reference","path":["work"]}}})};
        let output=if kind==ScheduledEffectKind::Agent {output}else{serde_json::json!({"type":"object","properties":{"value":output},"required":["value"],"additionalProperties":false})};
        f.install_scheduled_workflow_definition(&serde_json::json!({"description":"Native scheduled effect","block":{"input":empty,"output":output,"entry":"work","nodes":{"work":node,"done":{"type":"return","output":returned}},"edges":[{"from":"work","to":"done"}]}}));
        if kind==ScheduledEffectKind::Agent {
            let path=f.workspaces[0].join(".agents/agents/reviewer.toml");std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,"description='Review'\ninstructions='Return the Workflow output'\n").unwrap();
            let path=f.workspaces[0].join("rustx.toml");let mut config:toml::Value=toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();config["agent"].as_table_mut().unwrap().insert("agents".into(),vec![toml::Value::from("reviewer")].into());std::fs::write(path,toml::to_string(&config).unwrap()).unwrap();
        }
        let clock=Clock::new();f.manager.schedule_clock=clock.clone();let resident=f.load(0).await.unwrap().unwrap();let runtime=resident.inspect_runtime().unwrap();let created=scheduled(&f).await;clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted{start,..})=f.manager.schedule_operation(&f.sessions[0].id,&f.sessions[0].active_node,ScheduleOperation::Admit{id:created.id,expected_revision:1}).await.unwrap() else {panic!("native effect acceptance")};
        let view=finished(&f,&runtime,&start.id).await;assert!(matches!(view.start.state,ScheduledWorkflowState::Settled{outcome:ScheduledWorkflowOutcome::Completed,..}),"{:?}",view.start.state);
        assert_eq!(view.effects.len(),1);assert_eq!(view.effects[0].kind,kind);assert!(view.effects[0].physical_settlement_proven);
        match (kind,&view.effects[0].owner) {(ScheduledEffectKind::Tool,ScheduledEffectOwner::Tool{..})|(ScheduledEffectKind::Model,ScheduledEffectOwner::Model{..})|(ScheduledEffectKind::Agent,ScheduledEffectOwner::Agent{..})=>{},_=>panic!("missing exact native effect owner")}
        match kind {ScheduledEffectKind::Tool=>assert!(view.result.unwrap()["value"].as_str().unwrap().contains("RESIDENCY_SESSION")),ScheduledEffectKind::Model=>assert_eq!(view.result,Some(serde_json::json!({"value":"done"}))),ScheduledEffectKind::Agent=>assert_eq!(view.result,Some(serde_json::json!({}))),ScheduledEffectKind::Review=>unreachable!()}
        assert!(events(&runtime).iter().all(|event|!matches!(event,RuntimeEvent::AttemptStarted{..})));
        f.close().await;
    }).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_scheduled_tool_uses_native_invocation_and_physical_settlement() {
    scheduled_native_effect(ScheduledEffectKind::Tool).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_scheduled_model_uses_auxiliary_owner_without_a_root_attempt() {
    scheduled_native_effect(ScheduledEffectKind::Model).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_scheduled_agent_uses_native_child_owner_without_a_root_attempt() {
    scheduled_native_effect(ScheduledEffectKind::Agent).await;
}

use sha2::Digest as _;
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_review_invalid_workflow_does_not_fence_valid_commands_or_admission() {
    bounded(async {
        for denied in [false, true] {
            let mut f = Fixture::with_session_count(None, 1).await;
            f.install_scheduled_workflow("Validation refusal");
            if denied {
                let path = f.workspaces[0].join("rustx.toml");
                let mut config: toml::Value =
                    toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                config["agent"]["workflows"] = toml::Value::Array(Vec::new());
                std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
            }
            let clock = Clock::new();
            f.manager.schedule_clock = clock.clone();
            let _resident = f.load(0).await.unwrap().unwrap();
            let session = &f.sessions[0];
            let invalid = f
                .manager
                .schedule_command(
                    &session.id,
                    &session.active_node,
                    ScheduleCommand {
                        command_id: "invalid-workflow".into(),
                        mutation: ScheduleMutation::CreateWorkflow {
                            title: "Invalid".into(),
                            workflow_id: crate::runtime::workflow::WorkflowId::parse("scheduled")
                                .unwrap(),
                            arguments: serde_json::json!({"extra":true}),
                            timing: ScheduleTiming::After { seconds: 60 },
                        },
                    },
                )
                .await;
            assert!(
                matches!(invalid, Ok(ScheduleCommandResult::Invalid { .. })),
                "{invalid:?}"
            );
            if !denied {
                let created = scheduled(&f).await;
                let invalid_update = f
                    .manager
                    .schedule_command(
                        &session.id,
                        &session.active_node,
                        ScheduleCommand {
                            command_id: "invalid-update".into(),
                            mutation: ScheduleMutation::UpdateWorkflow {
                                id: created.id,
                                expected_revision: created.revision,
                                title: "Invalid update".into(),
                                workflow_id: crate::runtime::workflow::WorkflowId::parse(
                                    "scheduled",
                                )
                                .unwrap(),
                                arguments: serde_json::json!({"extra":true}),
                                timing: Some(ScheduleTiming::After { seconds: 60 }),
                            },
                        },
                    )
                    .await
                    .unwrap();
                assert!(matches!(
                    invalid_update,
                    ScheduleCommandResult::Invalid { .. }
                ));
            }
            assert!(f.manager.diagnostics().schedule_write_fenced.is_empty());
            let valid = receipt(start(&f, 0, "valid-after-refusal").await.unwrap().unwrap());
            clock.add(60);
            assert!(matches!(
                f.manager
                    .schedule_operation(
                        &session.id,
                        &session.active_node,
                        ScheduleOperation::Admit {
                            id: valid.id,
                            expected_revision: valid.revision,
                        }
                    )
                    .await
                    .unwrap(),
                ScheduleOperationResult::Admission(ScheduleAdmission::Admitted { .. })
            ));
            f.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_review_reconcile_preserves_live_completed_return_and_digest() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        f.install_scheduled_workflow("Live owner publication");
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();
        let resident = f.load(0).await.unwrap().unwrap();
        let runtime = resident.inspect_runtime().unwrap();
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        runtime.install_scheduled_settlement_gate(gate.clone());
        let created = scheduled(&f).await;
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start, ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created.id,
                    expected_revision: created.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("native admission")
        };
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        let before = f
            .manager
            .reconcile_scheduled_workflow(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                start.id.clone(),
                0,
                100,
            )
            .await
            .unwrap()
            .unwrap();
        let still_claimed = matches!(before.start.state, ScheduledWorkflowState::Claimed { .. });
        drop(release);
        let result = finished(&f, &runtime, &start.id).await;
        assert!(
            still_claimed,
            "reconcile settled a registered live owner: {:?}",
            before.start.state
        );
        assert!(matches!(
            result.start.state,
            ScheduledWorkflowState::Settled {
                outcome: ScheduledWorkflowOutcome::Completed,
                ..
            }
        ));
        let expected = serde_json::json!({"answer":"native durable return"});
        assert_eq!(result.result, Some(expected.clone()));
        assert_eq!(
            result.start.result_digest.as_deref(),
            Some(
                format!(
                    "{:x}",
                    sha2::Sha256::digest(serde_json::to_vec(&expected).unwrap())
                )
                .as_str()
            )
        );
        f.close().await;
    })
    .await;
}
