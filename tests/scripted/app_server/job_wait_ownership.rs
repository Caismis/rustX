//! Native terminal waits cannot retain manager admission through shutdown.
#![allow(clippy::too_many_lines, clippy::large_futures)]
use super::*;
use crate::runtime::identity::{ToolCallId, ToolId};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_review_job_wait_cannot_block_unload_or_replacement() {
    bounded(async {
        for replacement in [false, true] {
            let f = Fixture::with_session_count(None, 1).await;
            let connection = crate::app_server::connection::AppServerConnection::new(f.host.clone());
            super::protocol::initialize(&connection).await;
            let target = super::protocol::attach(&connection, &f, 0).await;
            let runtime = f.manager.configuration_runtime(&target.session_id).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(crate::tools::native::BashTool::new());
            let invocation = crate::tools::types::ToolInvocation {
                id: crate::tools::types::ToolInvocationId::Agent { call_id: ToolCallId::new("unbounded-bash") },
                tool_id: ToolId::new("tool-bash"), tool_name: "bash".into(),
                mode: crate::tools::types::ToolInvocationMode::Background,
                arguments: serde_json::json!({"command": format!("exec 9<>/dev/tcp/127.0.0.1/{}; printf '%s\n' \"$BASHPID\" >&9; exec /bin/sleep 3600", listener.local_addr().unwrap().port())}),
            };
            let background = runtime.tool_runtime().background();
            let prepared = background.prepare_dispatch(&invocation, &executor, crate::tools::environment::ToolEnvironment::new()).unwrap();
            let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } = background.commit_dispatch(prepared, &crate::runtime::CancellationSignal::new()).unwrap() else { panic!("accepted") };
            let (socket, _) = listener.accept().await.unwrap();
            let mut line = String::new(); BufReader::new(socket).read_line(&mut line).await.unwrap();
            let pid = nix::unistd::Pid::from_raw(line.trim().parse().unwrap());
            let probe = f.manager.probe(&target.conversation_id);
            probe.before_operation.arm();
            let mut wait = connection.admitted_request_for_test(crate::app_server::protocol::Request {
                jsonrpc: crate::app_server::protocol::JsonRpcVersion::V2,
                id: crate::app_server::protocol::RequestId::Integer(808),
                call: crate::app_server::protocol::Method::JobWait { target: target.clone(), job_id: execution_id.clone() },
            });
            assert!(futures_util::FutureExt::now_or_never(&mut wait).is_none());
            probe.before_operation.entered().await;
            probe.before_operation.release();
            let manager = f.manager.clone(); let session = f.sessions[0].clone();
            let mut retirement = tokio::spawn(async move {
                if replacement { manager.replace(&session.id, Some(&session.active_node)).await.map(|_| ()) }
                else { manager.unload(&session.active_conversation_id).await }
            });
            let finished = tokio::time::timeout(std::time::Duration::from_secs(5), &mut retirement).await;
            let circular = finished.is_err();
            if circular {
                // Failure-only cleanup; the successful path needs native retirement alone.
                runtime.background_cancel_intent(&execution_id).unwrap();
                bounded(retirement).await.unwrap().unwrap();
            } else { finished.unwrap().unwrap().unwrap(); }
            let reply = bounded(wait).await;
            assert!(matches!(reply.produced(), crate::app_server::protocol::Response::Success(success) if matches!(&success.result, crate::app_server::protocol::MethodResult::JobOutcome { job_id, state: crate::tools::background::BackgroundLifecycle::Cancelled } if job_id == &execution_id)));
            assert_eq!(nix::sys::signal::kill(pid, None), Err(nix::errno::Errno::ESRCH));
            drop(runtime);
            let loaded = f.manager.load(&target.session_id, None).await.unwrap();
            assert_ne!(loaded.incarnation_id(), target.runtime_incarnation);
            connection.close(); f.close().await;
            assert!(!circular, "JobWait admission lease prevented native retirement");
        }
    }).await;
}
