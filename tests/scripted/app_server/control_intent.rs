//! Reserved control admission commits intent while existing native wait owners remain blocked.
use super::*;
use crate::app_server::{connection::Reply, transport::Outbound};
use crate::tools::executor::ToolExecutor;
use futures_util::future::BoxFuture;
use std::{sync::Arc, task::Poll};

struct Release {
    job: Arc<super::super::super::AsyncGate>,
    child: Arc<crate::runtime::subagent::TerminalAuthorityHook>,
}
impl Drop for Release {
    fn drop(&mut self) {
        self.job.release();
        self.child.release();
    }
}

async fn held_job(
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    gate: Arc<super::super::super::AsyncGate>,
) -> (
    crate::runtime::identity::ToolExecutionId,
    Arc<std::sync::Mutex<Option<crate::runtime::cancellation::ExecutionCancellation>>>,
) {
    let cancellation = Arc::new(std::sync::Mutex::new(None));
    let executor: Arc<dyn ToolExecutor> =
        Arc::new(super::super::super::transports::BackgroundGate {
            gate: gate.clone(),
            cancellation: cancellation.clone(),
        });
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new("control-intent-job"),
        },
        tool_id: ToolId::new("control-intent-job"),
        tool_name: "control-intent-job".into(),
        mode: crate::tools::types::ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let background = runtime.tool_runtime().background();
    let prepared = background
        .prepare_dispatch(
            &invocation,
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .unwrap();
    let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        background
            .commit_dispatch(prepared, &crate::runtime::CancellationSignal::new())
            .unwrap()
    else {
        panic!("native background admission")
    };
    gate.entered().await;
    (execution_id, cancellation)
}

fn admitted(child: &ChildDelivery, id: i64, call: Method) -> BoxFuture<'static, Reply> {
    child.connection.admitted_request_for_test(Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(id),
        call,
    })
}

fn assert_pending(waits: &mut [BoxFuture<'static, Reply>]) {
    let waker = futures_util::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    for wait in waits {
        assert!(
            wait.as_mut().poll(&mut context).is_pending(),
            "native physical/canonical wait remains owned"
        );
    }
}

async fn refused(child: &ChildDelivery, id: i64, method: Method, expected: ErrorData) {
    let reply = admitted(child, id, method).await;
    let Response::Failure(failure) = reply.produced() else {
        panic!("bounded admission refusal")
    };
    assert_eq!(failure.id, Some(RequestId::Integer(id)));
    assert_eq!(failure.error.data.as_ref(), Some(&expected));
}

fn accept_receipt(reply: Reply, id: i64) -> MethodResult {
    let mut outbound = Outbound::reply(reply).unwrap();
    assert!(outbound.poll_hand_off::<()>(|_| Poll::Pending).is_pending());
    let Poll::Ready(Ok((record, Some(())))) = outbound.poll_hand_off(|record| {
        assert!(record.len() <= crate::app_server::transport::MAX_MESSAGE_BYTES);
        Poll::Ready(Ok(()))
    }) else {
        panic!("one bounded response acceptance")
    };
    let Response::Success(response) = serde_json::from_str(&record).unwrap() else {
        panic!("intent receipt")
    };
    assert_eq!(response.id, RequestId::Integer(id));
    response.result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t14_t20_two_execution_intents_bypass_six_waits_and_eight_unpolled_observations() {
    bounded(Box::pin(async {
        let child = ChildDelivery::with_delivery(false).await;
        let runtime = child.fixture.manager.configuration_runtime(&child.target.session_id).unwrap();
        let registry = runtime.subagent_registry().unwrap();
        let activation = registry.agent_snapshot(&child.agent).unwrap().current_activation.unwrap();
        let terminal = Arc::new(crate::runtime::subagent::TerminalAuthorityHook::default());
        registry.install_terminal_authority_hook(terminal.clone());
        let job_gate = Arc::new(super::super::super::AsyncGate::default()); job_gate.arm();
        let release = Release { job: job_gate.clone(), child: terminal.clone() };
        let (job_id, cancellation) = held_job(&runtime, job_gate.clone()).await;
        let mut waits = (0..6).map(|index| admitted(&child, 100 + index, if index % 2 == 0 {
            Method::JobWait { target: child.target.clone(), job_id: job_id.clone() }
        } else { Method::AgentWait { target: child.target.clone(), agent_id: child.agent.clone() } })).collect::<Vec<_>>();
        assert_pending(&mut waits);
        // Admission is synchronous before the first future poll. These eight
        // exact associations deliberately start no Unix/body read and own no execution.
        let observations = (0..8).map(|index| admitted(&child, 200 + index, Method::AgentObserve { target: child.target.clone(), agent_id: AgentId::new(format!("unpolled-{index}")), after: None })).collect::<Vec<_>>();
        let job_cancel = admitted(&child, 300, Method::JobCancel { target: child.target.clone(), job_id: job_id.clone() });
        let agent_cancel = admitted(&child, 301, Method::AgentInterrupt { target: child.target.clone(), agent_id: child.agent.clone() });
        refused(&child, 302, Method::ServerInfo {}, ErrorData::RequestCapacity).await;
        refused(&child, 303, Method::AgentObserve { target: child.target.clone(), agent_id: AgentId::new("ninth-unpolled"), after: None }, ErrorData::ObservationCapacityExceeded).await;
        refused(&child, 304, Method::TurnCancel { target: child.target.clone() }, ErrorData::RequestCapacity).await;
        let job_reply = job_cancel.await;
        let agent_reply = agent_cancel.await;
        assert!(matches!(job_reply.produced(), Response::Success(response) if matches!(&response.result, MethodResult::JobCancellationAccepted { job_id: captured } if captured == &job_id)));
        assert!(matches!(agent_reply.produced(), Response::Success(response) if matches!(&response.result, MethodResult::AgentInterruptionAccepted { agent_id, activation_id: Some(captured) } if agent_id == &child.agent && captured == &activation)));
        // Future completion is not response disposition: both control slots
        // remain owned while their real outbound records have accepted no bytes.
        refused(&child, 305, Method::TurnCancel { target: child.target.clone() }, ErrorData::RequestCapacity).await;
        assert_pending(&mut waits);
        assert!(cancellation.lock().unwrap().as_ref().unwrap().is_cancelled());
        assert_eq!(runtime.tool_runtime().background().snapshot(&job_id).unwrap().state, crate::tools::background::BackgroundLifecycle::Cancelling);
        assert_eq!(job_gate.parked_now(), 1, "original executor is still physically held");
        assert!(matches!(accept_receipt(job_reply, 300), MethodResult::JobCancellationAccepted { .. }));
        assert!(matches!(accept_receipt(agent_reply, 301), MethodResult::AgentInterruptionAccepted { .. }));
        let hook = terminal.clone();
        tokio::task::spawn_blocking(move || hook.wait_until_entered()).await.unwrap();
        assert_pending(&mut waits);
        assert!(!registry.snapshot(&activation).unwrap().is_settled(), "no receipt fabricates terminal authority");
        release.job.release(); release.child.release();
        for (index, wait) in waits.into_iter().enumerate() {
            match wait.await.commit() {
                Response::Success(response) => {
                    assert_eq!(response.id, RequestId::Integer(100 + i64::try_from(index).unwrap()));
                    if index % 2 == 0 { assert!(matches!(response.result, MethodResult::JobOutcome { job_id: settled, .. } if settled == job_id)); }
                    else { assert!(matches!(response.result, MethodResult::AgentWait { activation_id: Some(settled), outcome: Some(_), .. } if settled == activation)); }
                }
                response @ Response::Failure(_) => panic!("original native settlement: {response:?}"),
            }
        }
        drop(observations); drop(release);
        child.close().await;
    })).await;
}

#[path = "control_reconnect.rs"]
mod reconnect;
