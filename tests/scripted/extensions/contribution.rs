use super::*;
use crate::extensions::probe::{Behavior, ProbeConfig};
use crate::runtime::identity::{ContextContributorIdentity, NativeContextContributor};
use std::sync::atomic::Ordering;

fn composed(probes: Vec<ProbeConfig>) -> NativeAgentExtensions {
    let mut composition = NativeAgentExtensions::none();
    composition.test_contributors = probes;
    composition
}

#[tokio::test]
async fn issue383_concurrent_conversations_keep_domain_snapshots_and_receipts_isolated() {
    let (entered_a, mut arrived_a) = tokio::sync::watch::channel(false);
    let (entered_b, mut arrived_b) = tokio::sync::watch::channel(false);
    let (release, wait) = tokio::sync::watch::channel(false);
    let mut alpha = ProbeConfig::new(NativeContextContributor::TestAlpha, Behavior::Normal);
    let mut beta = ProbeConfig::new(NativeContextContributor::TestAlpha, Behavior::Normal);
    alpha.gate = Some((entered_a, wait.clone()));
    beta.gate = Some((entered_b, wait));
    beta.revision.store(9, Ordering::SeqCst);
    let a = composed(vec![alpha]);
    let b = composed(vec![beta]);
    let (_a, runtime_a) = todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9e", &a);
    let (_b, runtime_b) = todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9f", &b);
    let controller = async move {
        arrived_a.wait_for(|v| *v).await.unwrap();
        arrived_b.wait_for(|v| *v).await.unwrap();
        release.send_replace(true);
    };
    let (left, right, ()) = tokio::join!(
        run(
            &a,
            &runtime_a,
            ToolRegistry::new(),
            fake_model(vec![stop_turn()])
        ),
        run(
            &b,
            &runtime_b,
            ToolRegistry::new(),
            fake_model(vec![stop_turn()])
        ),
        controller,
    );
    assert!(matches!(left.0.outcome, AttemptOutcome::Completed { .. }));
    assert!(matches!(right.0.outcome, AttemptOutcome::Completed { .. }));
    let owner = ContextContributorIdentity::Native(NativeContextContributor::TestAlpha);
    assert_eq!(
        runtime_a
            .durable_store()
            .latest_contribution_emission(&owner, "active")
            .unwrap()
            .unwrap()
            .fingerprint,
        "revision-1"
    );
    assert_eq!(
        runtime_b
            .durable_store()
            .latest_contribution_emission(&owner, "active")
            .unwrap()
            .unwrap()
            .fingerprint,
        "revision-9"
    );
}

#[tokio::test]
async fn issue383_native_deferred_and_request_time_share_registered_provenance() {
    let mut probe = ProbeConfig::new(NativeContextContributor::TestAlpha, Behavior::Normal);
    probe.observe_tools = true;
    let captures = probe.captures.clone();
    let composition = composed(vec![probe]);
    let (_dir, runtime) =
        todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9e", &composition);
    let tool = FakeTool::new(
        common::tool_policies(
            "worker",
            "worker",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("done"),
    );
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let model = fake_model(vec![
        tool_turn(&[
            scripted("first", "worker", "worker"),
            scripted("second", "worker", "worker"),
        ]),
        stop_turn(),
    ]);
    let (result, _) = run(&composition, &runtime, tools, model).await;
    assert!(
        matches!(result.outcome, AttemptOutcome::Completed { .. }),
        "{:?}",
        result.outcome
    );
    assert_eq!(captures.load(Ordering::SeqCst), 2);
    let store = runtime.durable_store();
    let snapshots = store.read_request_snapshots(None, 10).unwrap().snapshots;
    let contributions = &snapshots[1].contributions;
    assert_eq!(contributions.len(), 3);
    assert_eq!(
        contributions
            .iter()
            .map(|c| c.emissions[0].key.as_str())
            .collect::<Vec<_>>(),
        ["tool-first", "tool-second", "active"]
    );
    for contribution in contributions {
        assert_eq!(
            contribution.producer,
            ContextContributorIdentity::Native(NativeContextContributor::TestAlpha)
        );
        assert_eq!(contribution.metadata, ContextKind::NativeEnvironment);
        assert!(
            store
                .latest_contribution_emission(
                    &contribution.producer,
                    &contribution.emissions[0].key
                )
                .unwrap()
                .is_some()
        );
    }
}

#[tokio::test]
async fn issue383_retry_and_corrective_requests_freeze_goal_and_native_receipts() {
    use crate::model::error::{
        MalformedToolProposalSource, ModelError, ModelErrorKind, ModelRetryDisposition,
    };
    for corrective in [false, true] {
        let probe = ProbeConfig::new(NativeContextContributor::TestAlpha, Behavior::Normal);
        let count = probe.captures.clone();
        let revision = probe.revision.clone();
        let composition = composed(vec![probe]).and_goal();
        let (_dir, runtime) =
            todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9e", &composition);
        let goal = runtime.goal().unwrap().clone();
        let initial = goal
            .write(crate::goal::GoalWrite::Create {
                objective: "original objective".into(),
                budget: 2,
                origin: crate::goal::GoalOrigin::RuntimeControl,
            })
            .unwrap()
            .unwrap();
        let (release, wait) = tokio::sync::watch::channel(false);
        let error = if corrective {
            ModelError::malformed_tool_proposal(
                MalformedToolProposalSource::StreamAssembly,
                "controlled malformed generation",
            )
        } else {
            ModelError {
                kind: ModelErrorKind::Transport,
                message: "controlled transient failure".into(),
                retry_disposition: ModelRetryDisposition::Transient,
                retry_after_ms: Some(0),
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            }
        };
        let call = scripted("probe-tool", "probe-worker", "worker");
        let tool = FakeTool::new(
            common::tool_policies(
                "worker",
                "probe-worker",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
            ),
            success_result("done"),
        );
        let mut tools = ToolRegistry::new();
        tool.register(&mut tools);
        let model = fake_model(vec![
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::ParkUntilReleased(wait),
                FakeStep::Emit(ModelEvent::Failed { error }),
            ],
            tool_turn(&[call]),
            stop_turn(),
        ]);
        let mut parked = model.parked();
        let controller = tokio::spawn(async move {
            parked.wait_for(|parked| *parked).await.unwrap();
            revision.store(2, Ordering::SeqCst);
            goal.write(crate::goal::GoalWrite::Mutate {
                expected: initial.reference,
                mutation: crate::goal::GoalMutation::Edit {
                    objective: "revised objective".into(),
                },
            })
            .unwrap()
            .unwrap();
            release.send_replace(true);
        });
        let (result, _) = run(&composition, &runtime, tools, model.clone()).await;
        controller.await.unwrap();
        assert!(
            matches!(result.outcome, AttemptOutcome::Completed { .. }),
            "{:?}",
            result.outcome
        );
        assert_eq!(model.requests().len(), 3);
        assert_eq!(
            count.load(Ordering::SeqCst),
            2,
            "once per logical step, never on retry"
        );
        let store = runtime.durable_store();
        let snapshots = store.read_request_snapshots(None, 10).unwrap().snapshots;
        assert_eq!(snapshots[0].contributions, snapshots[1].contributions);
        assert_eq!(
            snapshots[0].context_generation,
            snapshots[1].context_generation
        );
        assert!(snapshots[1].request_context_ids.is_empty());
        let goal_revision = |snapshot: &crate::model::RequestSnapshot| {
            snapshot
                .contributions
                .iter()
                .find_map(|entry| match &entry.metadata {
                    ContextKind::GoalStatus(goal) => Some(goal.reference.revision),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(goal_revision(&snapshots[0]), 1);
        assert_eq!(goal_revision(&snapshots[1]), 1);
        assert_eq!(goal_revision(&snapshots[2]), 2);
        let events = store.read_events(None, 100).unwrap().events;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeEvent::ContextContributionEmitted { .. }
                ))
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, RuntimeEvent::AttemptCompleted { .. }))
                .count(),
            1
        );
        assert!(matches!(
            events.last().unwrap().event,
            RuntimeEvent::AttemptCompleted { .. }
        ));
    }
}

#[tokio::test]
async fn issue383_common_budget_drops_content_and_receipt_together() {
    let probes = [
        NativeContextContributor::TestAlpha,
        NativeContextContributor::TestBeta,
    ]
    .into_iter()
    .map(|identity| {
        let mut probe = ProbeConfig::new(identity, Behavior::Normal);
        probe.payload_bytes = Some(600_000);
        probe
    })
    .collect();
    let composition = composed(probes);
    let (_dir, runtime) =
        todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9e", &composition);
    let model = fake_model(vec![stop_turn()]);
    let (result, _) = run(&composition, &runtime, ToolRegistry::new(), model).await;
    assert!(
        matches!(result.outcome, AttemptOutcome::Completed { .. }),
        "{:?}",
        result.outcome
    );
    let store = runtime.durable_store();
    let snapshots = store.read_request_snapshots(None, 10).unwrap();
    assert_eq!(snapshots.snapshots[0].contributions.len(), 1);
    for (identity, accepted) in [
        (NativeContextContributor::TestAlpha, true),
        (NativeContextContributor::TestBeta, false),
    ] {
        assert_eq!(
            store
                .latest_contribution_emission(
                    &ContextContributorIdentity::Native(identity),
                    "active"
                )
                .unwrap()
                .is_some(),
            accepted
        );
    }
    let snapshot = &snapshots.snapshots[0];
    let alpha = ContextContributorIdentity::Native(NativeContextContributor::TestAlpha);
    assert_eq!(snapshot.contributions[0].producer, alpha);
    assert_eq!(
        snapshot.context_generation.contributors,
        vec![crate::context::ContributorGeneration {
            identity: alpha,
            attestation: None,
        }],
        "persisted generation describes only the final accepted producer"
    );
}

#[tokio::test]
async fn issue383_domain_changes_after_capture_only_reach_the_next_logical_step() {
    let mut probe = ProbeConfig::new(NativeContextContributor::TestAlpha, Behavior::Normal);
    let revision = probe.revision.clone();
    let count = probe.captures.clone();
    let (entered, mut arrived) = tokio::sync::watch::channel(false);
    let (release, wait) = tokio::sync::watch::channel(false);
    probe.gate = Some((entered, wait));
    let composition = composed(vec![probe]);
    let (_dir, runtime) =
        todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9e", &composition);
    let call = scripted("probe-tool", "probe-worker", "worker");
    let tool = FakeTool::new(
        common::tool_policies(
            "worker",
            "probe-worker",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("done"),
    );
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let model = fake_model(vec![tool_turn(&[call]), stop_turn()]);
    let controller = tokio::spawn(async move {
        arrived.wait_for(|arrived| *arrived).await.unwrap();
        revision.store(2, Ordering::SeqCst);
        release.send_replace(true);
    });
    let (result, _) = run(&composition, &runtime, tools, model).await;
    controller.await.unwrap();
    assert!(
        matches!(result.outcome, AttemptOutcome::Completed { .. }),
        "{:?}",
        result.outcome
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let snapshots = runtime
        .durable_store()
        .read_request_snapshots(None, 10)
        .unwrap();
    assert_eq!(snapshots.snapshots.len(), 2);
    assert_eq!(
        snapshots.snapshots[0].contributions[0].emissions[0].fingerprint,
        "revision-1"
    );
    assert_eq!(
        snapshots.snapshots[1].contributions[0].emissions[0].fingerprint,
        "revision-2"
    );
}

#[tokio::test]
async fn issue383_duplicate_native_registration_rejects_attempt_before_start() {
    let composition = composed(vec![
        ProbeConfig::new(NativeContextContributor::TestAlpha, Behavior::Normal),
        ProbeConfig::new(NativeContextContributor::TestAlpha, Behavior::Normal),
    ]);
    let (_dir, runtime) =
        todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9e", &composition);
    let capability = common::capability_lease(ToolRegistry::new(), &runtime).await;
    let model = fake_model(vec![stop_turn()]);
    let request = AgentExecutionRequest {
        agent_id: AgentId::new("probe-agent"),
        conversation_id: runtime.conversation_id().clone(),
        attempt_id: AttemptId::new("probe-attempt"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(inbound("probe-inbound", "work")),
        ])
        .unwrap(),
        initial_turn_trigger: InitialTurnTrigger::FreshInbound(
            FreshInboundTurn::new(vec![MessageId::new("probe-inbound")]).unwrap(),
        ),
        model: support::attempt_model(model.clone(), "probe-model"),
    };
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    assert!(
        AgentExecution::new(
            request,
            capability.into_lease(),
            &cancellation,
            support::default_execution_policy(),
            context_runtime(&model, &composition),
            &runtime,
            rustx::agent::AttemptLifecycle::inert()
        )
        .is_err()
    );
    assert!(model.requests().is_empty());
    assert!(
        runtime
            .durable_store()
            .read_request_snapshots(None, 10)
            .unwrap()
            .snapshots
            .is_empty()
    );
    assert!(
        runtime
            .durable_store()
            .read_events(None, 10)
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn issue383_production_composed_noop_and_optional_failure_preserve_runtime_semantics() {
    let mut expected = None;
    for behavior in [None, Some(Behavior::Empty), Some(Behavior::OptionalFailure)] {
        let probes = behavior
            .map(|behavior| ProbeConfig::new(NativeContextContributor::TestAlpha, behavior))
            .into_iter()
            .collect();
        let composition = composed(probes);
        let (_dir, runtime) =
            todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9e", &composition);
        let model = fake_model(vec![stop_turn()]);
        let (result, _) = run(&composition, &runtime, ToolRegistry::new(), model.clone()).await;
        assert!(
            matches!(result.outcome, AttemptOutcome::Completed { .. }),
            "{:?}",
            result.outcome
        );
        assert_eq!(model.requests().len(), 1);
        let store = runtime.durable_store();
        let snapshots = store.read_request_snapshots(None, 10).unwrap().snapshots;
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0].request_context_ids.is_empty());
        assert!(snapshots[0].contributions.is_empty());
        assert!(snapshots[0].context_generation.contributors.is_empty());
        assert!(
            store
                .latest_contribution_emission(
                    &ContextContributorIdentity::Native(NativeContextContributor::TestAlpha),
                    "active",
                )
                .unwrap()
                .is_none()
        );
        let semantics = ordinary_semantics(&result, runtime.durable_store().as_ref());
        if let Some(expected) = &expected {
            assert_eq!(&semantics, expected);
        } else {
            expected = Some(semantics);
        }
        for probe in &composition.test_contributors {
            assert_eq!(probe.captures.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn issue383_shuffled_native_registration_and_producer_scoped_receipts() {
    let mut expected = None;
    for identities in [
        [
            NativeContextContributor::TestAlpha,
            NativeContextContributor::TestBeta,
        ],
        [
            NativeContextContributor::TestBeta,
            NativeContextContributor::TestAlpha,
        ],
    ] {
        let composition = composed(
            identities
                .into_iter()
                .map(|identity| ProbeConfig::new(identity, Behavior::Normal))
                .collect(),
        );
        let (_dir, runtime) =
            todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9e", &composition);
        let model = fake_model(vec![stop_turn()]);
        let (result, _) = run(&composition, &runtime, ToolRegistry::new(), model.clone()).await;
        assert!(
            matches!(result.outcome, AttemptOutcome::Completed { .. }),
            "{:?}",
            result.outcome
        );
        let store = runtime.durable_store();
        let snapshots = store.read_request_snapshots(None, 100).unwrap();
        let snapshot = &snapshots.snapshots[0];
        let order = snapshot
            .contributions
            .iter()
            .map(|record| record.producer.clone())
            .collect::<Vec<_>>();
        assert_eq!(order.len(), 2);
        if let Some(expected) = &expected {
            assert_eq!(&order, expected);
        } else {
            expected = Some(order);
        }
        for probe in &composition.test_contributors {
            assert_eq!(probe.captures.load(Ordering::SeqCst), 1);
            let producer = ContextContributorIdentity::Native(probe.identity);
            let receipt = store
                .latest_contribution_emission(&producer, "active")
                .unwrap()
                .unwrap();
            assert_eq!(receipt.producer, producer);
            assert!(
                snapshot
                    .contributions
                    .iter()
                    .any(|entry| entry.message_id == receipt.canonical_message_id
                        && entry.producer == producer)
            );
        }
    }
}

#[tokio::test]
async fn issue383_mandatory_and_integrity_failure_prevent_startup() {
    for behavior in [Behavior::MandatoryFailure, Behavior::Invalid] {
        let composition = composed(vec![ProbeConfig::new(
            NativeContextContributor::TestAlpha,
            behavior,
        )]);
        let (_dir, runtime) =
            todo_tool_runtime("conv_e676044a-be02-7f84-92a4-1d66bc24ce9e", &composition);
        let model = fake_model(vec![stop_turn()]);
        let (result, _) = run(&composition, &runtime, ToolRegistry::new(), model.clone()).await;
        assert!(
            matches!(result.outcome, AttemptOutcome::Failed { .. }),
            "{:?}",
            result.outcome
        );
        assert!(model.requests().is_empty());
        assert!(
            runtime
                .durable_store()
                .read_request_snapshots(None, 100)
                .unwrap()
                .snapshots
                .is_empty()
        );
        let events = runtime
            .durable_store()
            .read_events(None, 100)
            .unwrap()
            .events;
        assert!(matches!(
            events.last().unwrap().event,
            RuntimeEvent::AttemptFailed { .. }
        ));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, RuntimeEvent::AttemptFailed { .. }))
                .count(),
            1
        );
    }
}
