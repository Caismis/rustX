use crate::agent::cancellation::AgentCancellation;
use crate::agent::execution::AgentExecution;
use crate::agent::execution::test_fixtures::*;
use crate::runtime::identity::ConversationId;
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime};
use crate::tools::executor::ToolRegistry;
use std::sync::Arc;

#[tokio::test]
async fn capability_lease_owner_matches_runtime_before_execution() {
    let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let (lease, coordinator) = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease_and_coordinator();
    assert_eq!(coordinator.active_attempts(), 1);

    let execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("matching capability owner is accepted");

    assert_eq!(adapter.request_count(), 0, "construction is pre-execution");
    drop(execution);
    assert_eq!(coordinator.active_attempts(), 0);
}

#[tokio::test]
async fn capability_lease_rejects_different_conversation_before_execution() {
    let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let owner_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let (lease, coordinator) = capability_lease(ToolRegistry::new(), &owner_runtime)
        .await
        .into_lease_and_coordinator();
    assert_eq!(coordinator.active_attempts(), 1);
    let other_dir = tempfile::tempdir().expect("other runtime directory");
    std::fs::create_dir_all(other_dir.path().join("workspace")).expect("other workspace");
    let other_runtime = crate::tools::runtime::ConversationToolRuntime::new(
        ConversationId::new("conv_1eef1854-fea7-788b-8e49-ca0ec811fb0c"),
        other_dir.path().join("workspace"),
        other_dir.path().join("artifacts"),
    )
    .expect("other tool runtime");
    let mut other_request = request(&adapter);
    other_request.conversation_id =
        ConversationId::new("conv_1eef1854-fea7-788b-8e49-ca0ec811fb0c");

    let result = AgentExecution::new(
        other_request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &other_runtime,
        crate::agent::AttemptLifecycle::inert(),
    );

    assert!(matches!(
        result,
        Err(crate::runtime::inbound::MailboxError::CapabilityOwnershipMismatch { .. })
    ));
    assert_eq!(
        adapter.request_count(),
        0,
        "rejection precedes model requests"
    );
    assert_eq!(coordinator.active_attempts(), 0);
}

#[tokio::test]
async fn capability_lease_rejects_different_workspace_before_execution() {
    let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let owner_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let (lease, coordinator) = capability_lease(ToolRegistry::new(), &owner_runtime)
        .await
        .into_lease_and_coordinator();
    assert_eq!(coordinator.active_attempts(), 1);
    let other_dir = tempfile::tempdir().expect("other workspace directory");
    std::fs::create_dir_all(other_dir.path().join("workspace")).expect("other workspace");
    let other_runtime = crate::tools::runtime::ConversationToolRuntime::new(
        ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        other_dir.path().join("workspace"),
        other_dir.path().join("artifacts"),
    )
    .expect("other tool runtime");

    let result = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &other_runtime,
        crate::agent::AttemptLifecycle::inert(),
    );

    assert!(matches!(
        result,
        Err(crate::runtime::inbound::MailboxError::CapabilityOwnershipMismatch { .. })
    ));
    assert_eq!(
        adapter.request_count(),
        0,
        "rejection precedes model requests"
    );
    assert_eq!(coordinator.active_attempts(), 0);
}
