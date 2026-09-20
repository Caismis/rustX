// Included in the execution test module to exercise the real startup gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue383_noop_and_absent_extensions_share_the_cancellation_boundary() {
    for composed in [false, true] {
        let adapter = Arc::new(ScriptedAdapter::new(vec![]));
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
            ))
            .unwrap(),
        );
        let probe = composed.then(|| {
            crate::extensions::probe::ProbeConfig::new(
                crate::runtime::identity::NativeContextContributor::TestAlpha,
                crate::extensions::probe::Behavior::Empty,
            )
        });
        let (_dir, tool_runtime) = native_probe_runtime(probe, store.clone());
        let (_capdir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, parked, _) = StartBoundaryPause::install(true, false);
        let mut parked = parked.unwrap();
        let mut execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .unwrap();
        execution.install_start_boundary_pause(pause);
        let canceller = cancellation.clone();
        let controller = tokio::spawn(async move {
            parked.await_park(1).await;
            canceller.cancel();
            parked.release();
        });
        let result = execution.run().await;
        controller.await.unwrap();
        assert!(matches!(result.outcome, AttemptOutcome::Cancelled { .. }));
        assert_eq!(adapter.request_count(), 0);
        assert!(request_snapshot_history(store.as_ref()).is_empty());
        let events = event_history(store.as_ref());
        assert!(!events.iter().any(|e| matches!(
            e,
            RuntimeEvent::ContextContributionEmitted { .. }
                | RuntimeEvent::ModelRequestStarted { .. }
        )));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::AttemptCancelled { .. }))
                .count(),
            1
        );
        assert!(matches!(
            events.last(),
            Some(RuntimeEvent::AttemptCancelled { .. })
        ));
    }
}

fn native_probe_runtime(
    probe: impl IntoIterator<Item = crate::extensions::probe::ProbeConfig>,
    store: Arc<crate::durable::SqliteConversationStore>,
) -> (
    tempfile::TempDir,
    crate::tools::runtime::ConversationToolRuntime,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("workspace")).unwrap();
    let mut composition = crate::extensions::NativeAgentExtensions::none();
    composition.test_contributors.extend(probe);
    let runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        store.conversation_id().clone(),
        crate::tools::runtime::ConversationRuntimeConfig {
            durable_binding: Some(crate::durable::ConversationStoreBinding::new(store)),
            ..crate::tools::runtime::ConversationRuntimeConfig::new(
                dir.path().join("workspace"),
                dir.path().join("artifacts"),
            )
            .with_extensions(composition)
        },
    )
    .unwrap();
    (dir, runtime)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue383_native_preparation_cancellation_commits_nothing() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }]]));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .unwrap(),
    );
    let (entered, mut arrived) = watch::channel(false);
    let (release, wait) = watch::channel(false);
    let mut probe = crate::extensions::probe::ProbeConfig::new(
        crate::runtime::identity::NativeContextContributor::TestAlpha,
        crate::extensions::probe::Behavior::Normal,
    );
    probe.gate = Some((entered, wait));
    let count = probe.captures.clone();
    let (_dir, tool_runtime) = native_probe_runtime([probe], store.clone());
    let (_capdir, _coordinator, lease) = capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let canceller = cancellation.clone();
    let controller = tokio::spawn(async move {
        arrived.wait_for(|arrived| *arrived).await.unwrap();
        canceller.cancel();
        assert!(canceller.is_cancelled());
        release.send_replace(true);
    });
    let result = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap()
    .run()
    .await;
    controller.await.unwrap();
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(adapter.request_count(), 0);
    assert!(request_snapshot_history(store.as_ref()).is_empty());
    assert!(
        store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::TestAlpha
                ),
                "active"
            )
            .unwrap()
            .is_none()
    );
    assert!(matches!(result.outcome, AttemptOutcome::Cancelled { .. }));
    let events = event_history(store.as_ref());
    assert!(!events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ModelRequestStarted { .. } | RuntimeEvent::ContextContributionEmitted { .. }
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
            .count(),
        1
    );
    assert!(matches!(
        events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue383_native_start_wins_and_receipts_remain_truthful() {
    let adapter = Arc::new(ParkedUntilCancelledAdapter::default());
    let model: Arc<dyn ModelAdapter> = adapter.clone();
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .unwrap(),
    );
    let probe = crate::extensions::probe::ProbeConfig::new(
        crate::runtime::identity::NativeContextContributor::TestAlpha,
        crate::extensions::probe::Behavior::Normal,
    );
    let (_dir, tool_runtime) = native_probe_runtime([probe], store.clone());
    let (_capdir, _coordinator, lease) = capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, _, parked) = StartBoundaryPause::install(false, true);
    let mut parked = parked.unwrap();
    let mut execution = AgentExecution::new(
        request_dyn(&model),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime_dyn(&model),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap();
    execution.install_start_boundary_pause(pause);
    let canceller = cancellation.clone();
    let controller = tokio::spawn(async move {
        parked.await_park(1).await;
        // Startup owns the cancellation gate. cancel() cannot complete until
        // the durable transaction returns and releases that same gate.
        parked.release();
        canceller.cancel();
    });
    let result = execution.run().await;
    controller.await.unwrap();
    assert!(matches!(result.outcome, AttemptOutcome::Cancelled { .. }));
    assert_eq!(adapter.request_count(), 1);
    let snapshots = request_snapshot_history(store.as_ref());
    assert_eq!(snapshots.len(), 1);
    let receipt = store
        .latest_contribution_emission(
            &crate::runtime::identity::ContextContributorIdentity::Native(
                crate::runtime::identity::NativeContextContributor::TestAlpha,
            ),
            "active",
        )
        .unwrap()
        .unwrap();
    assert!(
        snapshots[0]
            .contributions
            .iter()
            .any(|entry| entry.message_id == receipt.canonical_message_id)
    );
    let events = event_history(store.as_ref());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ContextContributionEmitted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
            .count(),
        1
    );
    assert!(matches!(
        events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
}

#[tokio::test]
async fn issue383_native_start_transaction_failure_rolls_back_receipts() {
    for fault in [
        crate::durable::sqlite::RequestStartFaultOperation::AfterContextAppend,
        crate::durable::sqlite::RequestStartFaultOperation::AfterContributionEventInsert,
        crate::durable::sqlite::RequestStartFaultOperation::AfterContributionHeadUpsert,
    ] {
        let adapter = Arc::new(ScriptedAdapter::new(vec![]));
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
            ))
            .unwrap(),
        );
        store.arm_request_start_fault_script([fault]);
        let probe = crate::extensions::probe::ProbeConfig::new(
            crate::runtime::identity::NativeContextContributor::TestAlpha,
            crate::extensions::probe::Behavior::Normal,
        );
        let (_dir, tool_runtime) = native_probe_runtime([probe], store.clone());
        let (_capdir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .unwrap()
        .run()
        .await;
        assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
        assert_eq!(adapter.request_count(), 0);
        assert!(request_snapshot_history(store.as_ref()).is_empty());
        assert!(
            store
                .latest_contribution_emission(
                    &crate::runtime::identity::ContextContributorIdentity::Native(
                        crate::runtime::identity::NativeContextContributor::TestAlpha
                    ),
                    "active"
                )
                .unwrap()
                .is_none()
        );
        assert!(!store.load_canonical().unwrap().iter().any(|message| matches!(message, MessageBlock::User(user) if matches!(user.kind, InboundKind::Context(_)))));
        let events = event_history(store.as_ref());
        assert!(!events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ModelRequestStarted { .. }
                | RuntimeEvent::ContextContributionEmitted { .. }
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::AttemptFailed { .. }))
                .count(),
            1
        );
        assert!(matches!(
            events.last(),
            Some(RuntimeEvent::AttemptFailed { .. })
        ));
    }
}
