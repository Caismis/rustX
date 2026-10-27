#[tokio::test]
async fn first_turn_admission_preserves_precancel_and_committed_observer_order() {
    for precancel in [true, false] {
        let adapter = Arc::new(ScriptedAdapter::new(vec![]));
        let store = Arc::new(crate::durable::SqliteConversationStore::in_memory(
            ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        ).unwrap());
        let tool_runtime = tool_runtime_with_store(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18", Some(store.clone()),
        );
        let (_dir, _coordinator, lease) = capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        if precancel { cancellation.cancel(); }
        let observer = RecordingObserver {
            cancel_on_admission: (!precancel).then(|| (cancellation.clone(), store.clone())),
            ..RecordingObserver::default()
        };
        let mut execution = AgentExecution::new(
            request(&adapter), lease, &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime(&adapter), &tool_runtime, crate::agent::AttemptLifecycle::inert(),
        ).unwrap();
        execution.observe(&observer);
        let result = execution.run().await;
        assert!(matches!(result.outcome, AttemptOutcome::Cancelled { .. }));
        assert!(result.durable_failure.is_none());
        assert_eq!(adapter.request_count(), 0);
        assert!(request_snapshot_history(store.as_ref()).is_empty());
        let events = event_history(store.as_ref());
        assert_eq!(observer.events(), events, "committed callbacks retain native order");
        if precancel {
            assert!(matches!(events.as_slice(), [RuntimeEvent::AttemptStarted { .. }, RuntimeEvent::AttemptCancelled { .. }]));
        } else {
            assert!(matches!(events.as_slice(), [RuntimeEvent::AttemptStarted { .. }, RuntimeEvent::TurnStarted, RuntimeEvent::AttemptCancelled { .. }]));
        }
    }
}

#[tokio::test]
async fn first_turn_admission_failure_never_publishes_a_partial_start_or_calls_provider() {
    for second_member in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("admission.sqlite");
        let adapter = Arc::new(ScriptedAdapter::new(vec![]));
        let store = Arc::new(crate::durable::SqliteConversationStore::open(
            ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"), &path,
        ).unwrap());
        store.initialize(&[]).unwrap();
        if second_member {
            rusqlite::Connection::open(&path).unwrap().execute_batch(
                "CREATE TRIGGER reject_first_turn BEFORE INSERT ON events WHEN json_extract(NEW.event_json,'$.event.type')='turn_started' BEGIN SELECT RAISE(ABORT,'second admission member failure'); END;"
            ).unwrap();
        } else {
            store.arm_fail_event_times(1);
        }
        let tool_runtime = tool_runtime_with_store(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18", Some(store.clone()),
        );
        let (_dir, _coordinator, lease) = capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let observer = RecordingObserver::default();
        let mut execution = AgentExecution::new(
            request(&adapter), lease, &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime(&adapter), &tool_runtime, crate::agent::AttemptLifecycle::inert(),
        ).unwrap();
        execution.observe(&observer);
        let result = execution.run().await;
        assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
        assert_eq!(result.durable_failure_kind, Some(super::DurableFailureKind::EventJournal));
        assert!(result.durable_failure.as_deref().unwrap().contains("runtime event could not be persisted"));
        assert_eq!(adapter.request_count(), 0);
        assert!(request_snapshot_history(store.as_ref()).is_empty());
        let events = event_history(store.as_ref());
        assert!(matches!(events.as_slice(), [RuntimeEvent::AttemptFailed { .. }]), "only the separate failure terminal may commit: {events:?}");
        assert_eq!(observer.events(), events);
    }
}
