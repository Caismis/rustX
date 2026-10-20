use crate::agent::cancellation::AgentCancellation;
use crate::agent::execution::AgentExecution;
use crate::agent::execution::test_fixtures::*;
use crate::durable::JournalStore;
use crate::events::types::{AttemptOutcome, RuntimeEvent};
use crate::message::types::ContentBlockIndex;
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::ConversationId;
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime_with_store};
use crate::tools::executor::ToolRegistry;
use std::sync::Arc;

/// A terminal Event Journal append is a required durable publication.
/// If it fails, the execution settlement candidate remains available to
/// the caller, but neither the local event projection nor an observer may
/// fabricate the uncommitted terminal fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_event_append_failure_never_fabricates_terminal_fact() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![
        ModelEvent::Started,
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "done".to_owned(),
        },
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    ]]));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("in-memory store"),
    );
    store.arm_fail_next_terminal_event();
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let observer = RecordingObserver::default();
    let mut execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("execution construction");
    execution.observe(&observer);

    let result = execution.run().await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert_eq!(
        result.durable_failure_kind,
        Some(super::DurableFailureKind::EventJournal)
    );
    assert!(result.durable_failure.is_some());
    let events = event_history(store.as_ref());
    assert!(
        !events.iter().any(|event| matches!(
            event,
            RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
                | RuntimeEvent::AttemptFailed { .. }
        )),
        "an uncommitted terminal candidate never enters the local projection"
    );
    assert!(
        !observer
            .events
            .lock()
            .expect("observer event lock")
            .iter()
            .any(|event| matches!(
                event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )),
        "publication follows the durable Event Journal commit"
    );
    let persisted = store.read_events(None, 64).expect("event journal").events;
    assert!(
        persisted
            .iter()
            .any(|envelope| matches!(envelope.event, RuntimeEvent::AttemptStarted { .. }))
    );
    assert!(
        !persisted.iter().any(|envelope| matches!(
            envelope.event,
            RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
                | RuntimeEvent::AttemptFailed { .. }
        )),
        "the failed terminal transaction leaves no durable terminal fact"
    );
    assert_eq!(store.terminal_event_attempts(), 1);
}
