use crate::agent::cancellation::AgentCancellation;
use crate::agent::execution::AgentExecution;
use crate::agent::execution::test_fixtures::*;
use crate::events::types::{AttemptOutcome, RuntimeEvent};
use crate::model::error::ModelErrorKind;
use crate::model::event::ModelEvent;
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime};
use crate::tools::executor::ToolRegistry;
use std::sync::Arc;

/// A generic provider request-size failure is terminal request ownership,
/// not conversation-history pressure. Once an adapter normalizes it as
/// `InvalidRequest`, the Agent Loop emits no compaction lifecycle and
/// performs no summary/retry invocation.
#[tokio::test]
async fn invalid_request_size_failure_never_compacts_or_retries() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Failed {
        error: crate::model::error::ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message: "Request exceeds the maximum size of 32 MB".to_owned(),
            retry_disposition: crate::model::error::ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: Some("request_too_large".to_owned()),
            context_overflow: None,
            malformed_tool_proposal: None,
            timeout_phase: None,
            generation: None,
        },
    }]]));
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
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
    .expect("execution construction")
    .run()
    .await;

    assert_eq!(
        adapter.request_count(),
        1,
        "no summary request or overflow retry is issued"
    );
    assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
    let events = tool_runtime
        .durable_store()
        .read_events(None, 128)
        .expect("event journal")
        .events;
    assert!(
        !events.iter().any(|event| matches!(
            event.event,
            RuntimeEvent::CompactionStarted { .. }
                | RuntimeEvent::CompactionCompleted { .. }
                | RuntimeEvent::CompactionFailed { .. }
        )),
        "generic request-size failure produces no compaction lifecycle"
    );
    assert!(
        tool_runtime
            .durable_store()
            .projection_head()
            .unwrap()
            .is_none()
    );
}
