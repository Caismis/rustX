//! Finite Job controls, synchronized with explicit executor gates.

use std::sync::Arc;

use super::super::support::domain_controls::{background_invocation, json_content};
use super::super::{common, support};
use rustx::runtime::CancellationSignal;
use rustx::tools::background::BackgroundDispatchOutcome;
use rustx::tools::executor::ToolExecutor;
use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionStatus};

#[tokio::test]
async fn job_status_is_immediate_and_wait_captures_one_finite_job() {
    let fixture = common::native_fixture();
    let registry = fixture.runtime.background();
    let mut ids = Vec::new();
    let mut releases = Vec::new();
    for _ in 0..2 {
        let (tool, release) = support::fake::FakeTool::parking(
            common::tool_policies(
                "bash",
                "tool-bash",
                ToolExecutionPolicy::ModelSelectable,
                ToolConcurrencyPolicy::Sequential,
            ),
            support::fake::success_result("done"),
        );
        let mut started = tool.started();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &background_invocation("bash"),
                &executor,
                rustx::tools::environment::ToolEnvironment::new(),
            )
            .unwrap();
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .unwrap()
        else {
            panic!("admission must commit")
        };
        started.wait_for(|started| *started).await.unwrap();
        ids.push(execution_id);
        releases.push(release);
    }

    let target = serde_json::json!({"job_id": ids[0]});
    let status = common::run_tool(&fixture, "job_status", target.clone());
    tokio::pin!(status);
    let std::task::Poll::Ready(status) = futures_util::poll!(&mut status) else {
        panic!("status must not wait for progress or settlement")
    };
    assert_eq!(json_content(&status)["state"], "running");

    let wait = common::run_tool(&fixture, "job_wait", target.clone());
    tokio::pin!(wait);
    assert!(futures_util::poll!(&mut wait).is_pending());
    releases[1].send_replace(true);
    registry.wait_until_terminal(&ids[1]).await.unwrap();
    assert!(
        futures_util::poll!(&mut wait).is_pending(),
        "a different terminal job cannot satisfy this wait"
    );
    releases[0].send_replace(true);
    let terminal = json_content(&wait.await);
    assert_eq!(terminal["job_id"], ids[0].as_str());
    assert_eq!(terminal["state"], "succeeded");

    let cancelled = common::run_tool(&fixture, "job_cancel", target).await;
    assert_eq!(
        json_content(&cancelled),
        terminal,
        "terminal is irreversible"
    );
    let list = common::run_tool(&fixture, "job_list", serde_json::json!({})).await;
    let list = json_content(&list);
    assert_eq!(list["jobs"][0]["job_id"], ids[1].as_str());
    assert_eq!(list["jobs"][1]["job_id"], ids[0].as_str());
    assert_eq!(list["returned"], 2);
    assert_eq!(list["truncated"], false);
    assert!(list["jobs"][0].get("result").is_none());

    let foreign = common::native_fixture();
    for name in ["job_status", "job_wait", "job_cancel"] {
        let result = common::run_tool(&foreign, name, serde_json::json!({"job_id": ids[0]})).await;
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
    }
}

#[tokio::test]
async fn unknown_job_wait_returns_without_subscribing_forever() {
    let fixture = common::native_fixture();
    let id =
        rustx::runtime::identity::ToolExecutionId::new("exec_215a03ee-2332-70b6-8e2d-634da8066f98");
    let wait = fixture.runtime.background().wait_until_terminal(&id);
    tokio::pin!(wait);
    assert!(matches!(
        futures_util::poll!(&mut wait),
        std::task::Poll::Ready(None)
    ));
}

/// The executor acknowledges cancellation only after the explicit settlement
/// gate. Merely emitting a cancellation signal cannot satisfy `job_cancel`.
#[tokio::test]
async fn job_cancel_waits_for_physical_settlement_and_preserves_one_notification() {
    struct GatedSettlement {
        started: tokio::sync::watch::Sender<bool>,
        release: tokio::sync::watch::Receiver<bool>,
    }
    impl ToolExecutor for GatedSettlement {
        fn progress_capability(&self) -> rustx::tools::ToolProgressCapability {
            rustx::tools::ToolProgressCapability::None
        }

        fn start<'a>(
            &'a self,
            _invocation: rustx::tools::types::ToolInvocation,
            context: rustx::tools::executor::ToolExecutionContext<'a>,
        ) -> rustx::tools::executor::ToolExecutionHandle<'a> {
            let mut release = self.release.clone();
            let cancellation = context.cancellation.clone();
            rustx::tools::executor::ToolExecutionHandle::settled_by_operation(
                Box::pin(async move {
                    self.started.send_replace(true);
                    release.wait_for(|released| *released).await.unwrap();
                    let mut result = support::fake::success_result("settled");
                    result.status = ToolExecutionStatus::Cancelled {
                        reason: cancellation.reason(),
                        phase: rustx::tools::types::ToolCancellationPhase::DuringExecution,
                    };
                    result
                }),
                context.cancellation,
            )
        }
    }

    let fixture = common::native_fixture();
    let registry = fixture.runtime.background();
    let (started, mut starts) = tokio::sync::watch::channel(false);
    let (release, released) = tokio::sync::watch::channel(false);
    let executor: Arc<dyn ToolExecutor> = Arc::new(GatedSettlement {
        started,
        release: released,
    });
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &executor,
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .unwrap();
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .unwrap()
    else {
        panic!("admission must commit")
    };
    starts.wait_for(|started| *started).await.unwrap();
    let cancel = common::run_tool(
        &fixture,
        "job_cancel",
        serde_json::json!({"job_id": execution_id}),
    );
    tokio::pin!(cancel);
    assert!(futures_util::poll!(&mut cancel).is_pending());
    assert_eq!(
        registry.snapshot(&execution_id).unwrap().state,
        rustx::tools::background::BackgroundLifecycle::Cancelling
    );
    assert!(
        fixture
            .runtime
            .mailbox()
            .select_pending_batch()
            .unwrap()
            .is_none()
    );
    release.send_replace(true);
    assert_eq!(json_content(&cancel.await)["state"], "cancelled");
    let mailbox = fixture.runtime.mailbox();
    let batch = mailbox.select_pending_batch().unwrap().unwrap();
    assert_eq!(batch.items().len(), 1);
    let _ = mailbox.adopt_pending_batch(&batch, None).unwrap();
    let repeated = common::run_tool(
        &fixture,
        "job_cancel",
        serde_json::json!({"job_id": execution_id}),
    )
    .await;
    assert_eq!(json_content(&repeated)["state"], "cancelled");
    assert!(mailbox.select_pending_batch().unwrap().is_none());
}
