//! Local MCP integration using the official rmcp server implementation.

#[cfg(all(unix, feature = "mcp-fixture"))]
mod unix_tests {
    use std::sync::Arc;

    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };
    use rustx::tools::mcp::fixture::FixtureServer;
    use tokio_util::sync::CancellationToken;

    struct NoProgress;

    impl rustx::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
    }

    #[derive(Clone, Default)]
    struct RecordingProgress {
        values: Arc<std::sync::Mutex<Vec<rustx::tools::types::ToolProgress>>>,
        notified: Arc<tokio::sync::Notify>,
    }

    impl rustx::tools::executor::ProgressReporter for RecordingProgress {
        fn report(&self, progress: rustx::tools::types::ToolProgress) {
            self.values.lock().expect("progress lock").push(progress);
            self.notified.notify_one();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn stdio_discovery_and_call_use_canonical_executor() {
        if rustx::tools::mcp::fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let artifacts_dir = tempfile::tempdir().expect("artifacts");
        let cancel_marker = workspace_dir.path().join("fixture-cancel-observed");
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        let invalidation = Arc::new(rustx::tools::mcp::McpInvalidationState::new());
        let server_id = rustx::runtime::identity::McpServerId::new("fixture");
        let binding = rustx::tools::mcp::McpServerBinding {
            transport: rustx::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: rustx::tools::mcp::fixture::fixture_spawn_args(
                    "unix_tests::stdio_discovery_and_call_use_canonical_executor",
                ),
                cwd: None,
                environment: std::collections::BTreeMap::from([
                    (
                        rustx::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                        "1".to_owned(),
                    ),
                    (
                        rustx::tools::mcp::fixture::CANCEL_FILE_ENV.to_owned(),
                        cancel_marker.display().to_string(),
                    ),
                ]),
            },
            policy: rustx::tools::types::ToolInvocationPolicy::default(),
        };
        let runtime = rustx::tools::mcp::McpServerRuntime::connect(
            &server_id,
            &binding,
            &workspace,
            invalidation,
        )
        .await
        .expect("MCP connect");
        let tools = runtime.list_tools().await.expect("tools/list");
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "mutate", "slow"]
        );
        let definitions =
            rustx::tools::mcp::definitions(&server_id, binding.policy, &runtime, tools);
        let mutate_index = definitions
            .iter()
            .position(|(definition, _)| definition.name == "mutate")
            .expect("mutate definition");
        let executor = definitions[mutate_index].1.clone();
        let runtime_bundle = rustx::tools::runtime::ConversationToolRuntime::new(
            rustx::runtime::identity::ConversationId::new("m7-mcp"),
            workspace_dir.path(),
            artifacts_dir.path(),
        )
        .expect("tool runtime");
        let progress = RecordingProgress::default();
        let initial_epoch = runtime.change_epoch();
        let result = rustx::tools::executor::ToolExecutor::execute(
            executor.as_ref(),
            rustx::tools::types::ToolInvocation {
                call_id: rustx::runtime::identity::ToolCallId::new("call"),
                tool_id: definitions[mutate_index].0.id.clone(),
                tool_name: "mutate".to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            rustx::tools::executor::ToolExecutionContext {
                conversation_id: runtime_bundle.conversation_id(),
                execution_id: None,
                cancellation: rustx::runtime::CancellationSignal::new(),
                cancellation_reason: rustx::runtime::types::CancellationReason::UserRequested,
                workspace: runtime_bundle.workspace(),
                progress: &progress,
                artifacts: runtime_bundle.artifacts(),
                environment: runtime_bundle.environment(),
            },
        )
        .await;
        assert!(matches!(
            result.status,
            rustx::tools::types::ToolExecutionStatus::Success
        ));
        assert!(matches!(
            result.content.first(),
            Some(rustx::tools::types::ToolResultContent::Text(text)) if text.text == "fixture changed"
        ));
        let progress_values = progress.values.lock().expect("progress lock").clone();
        assert!(
            progress_values
                .iter()
                .any(|progress| { progress.completed == Some(0.5) && progress.total == Some(3.5) })
        );
        let slow_index = definitions
            .iter()
            .position(|(definition, _)| definition.name == "slow")
            .expect("slow definition");
        let slow_executor = definitions[slow_index].1.clone();
        let slow_progress = RecordingProgress::default();
        let slow_cancellation = rustx::runtime::CancellationSignal::new();
        let slow_future = rustx::tools::executor::ToolExecutor::execute(
            slow_executor.as_ref(),
            rustx::tools::types::ToolInvocation {
                call_id: rustx::runtime::identity::ToolCallId::new("slow-call"),
                tool_id: definitions[slow_index].0.id.clone(),
                tool_name: "slow".to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            rustx::tools::executor::ToolExecutionContext {
                conversation_id: runtime_bundle.conversation_id(),
                execution_id: None,
                cancellation: slow_cancellation.clone(),
                cancellation_reason: rustx::runtime::types::CancellationReason::UserRequested,
                workspace: runtime_bundle.workspace(),
                progress: &slow_progress,
                artifacts: runtime_bundle.artifacts(),
                environment: runtime_bundle.environment(),
            },
        );
        tokio::pin!(slow_future);
        tokio::select! {
            () = slow_progress.notified.notified() => {}
            result = &mut slow_future => panic!("slow call completed before cancellation: {result:?}"),
        }
        slow_cancellation.cancel();
        let slow_result = slow_future.await;
        assert!(matches!(
            slow_result.status,
            rustx::tools::types::ToolExecutionStatus::Cancelled { .. }
        ));
        // The server-side cancellation context must become observable in
        // the fixture process, not only that rustX returned `Cancelled`.
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            wait_for_file(&cancel_marker),
        )
        .await
        .expect("the fixture server must observe the cancellation notification");
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            runtime.wait_for_change(initial_epoch),
        )
        .await
        .expect("tools/list_changed notification");
        assert_eq!(
            runtime
                .list_tools()
                .await
                .expect("refreshed tools/list")
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "new_tool"]
        );
        runtime
            .close()
            .await
            .expect("the owned stdio unit must publish physical settlement");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn streamable_http_discovery_and_call_use_the_same_runtime_boundary() {
        let cancellation = CancellationToken::new();
        let mut server_config = StreamableHttpServerConfig::default();
        server_config.cancellation_token = cancellation.child_token();
        server_config.sse_keep_alive = None;
        let fixture = FixtureServer {
            list_changed_supported: true,
            ..FixtureServer::default()
        };
        let slow_started = fixture.slow_started.clone();
        let cancel_observed = fixture.cancel_observed.clone();
        let service = StreamableHttpService::<FixtureServer, LocalSessionManager>::new(
            move || Ok(fixture.clone()),
            Arc::default(),
            server_config,
        );
        let router = axum::Router::new().nest_service("/mcp", service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("HTTP listener");
        let address = listener.local_addr().expect("HTTP address");
        let server_task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        let server_id = rustx::runtime::identity::McpServerId::new("http-fixture");
        let binding = rustx::tools::mcp::McpServerBinding {
            transport: rustx::tools::mcp::McpTransportConfig::StreamableHttp {
                endpoint: format!("http://{address}/mcp"),
                headers: std::collections::BTreeMap::new(),
            },
            policy: rustx::tools::types::ToolInvocationPolicy::default(),
        };
        let runtime = rustx::tools::mcp::McpServerRuntime::connect(
            &server_id,
            &binding,
            &workspace,
            Arc::new(rustx::tools::mcp::McpInvalidationState::new()),
        )
        .await
        .expect("HTTP MCP connect");
        let tools = runtime.list_tools().await.expect("HTTP tools/list");
        assert_eq!(tools.len(), 3);
        let definitions =
            rustx::tools::mcp::definitions(&server_id, binding.policy, &runtime, tools);
        let initial_epoch = runtime.change_epoch();
        let executor = definitions
            .iter()
            .find(|(definition, _)| definition.name == "mutate")
            .map(|(_, executor)| executor.clone())
            .expect("HTTP mutate executor");
        let artifacts_dir = tempfile::tempdir().expect("artifacts");
        let runtime_bundle = rustx::tools::runtime::ConversationToolRuntime::new(
            rustx::runtime::identity::ConversationId::new("m7-http"),
            workspace_dir.path(),
            artifacts_dir.path(),
        )
        .expect("tool runtime");
        let progress = NoProgress;
        let result = rustx::tools::executor::ToolExecutor::execute(
            executor.as_ref(),
            rustx::tools::types::ToolInvocation {
                call_id: rustx::runtime::identity::ToolCallId::new("http-call"),
                tool_id: rustx::runtime::identity::ToolId::new(rustx::tools::mcp::mcp_tool_id(
                    &server_id, "mutate",
                )),
                tool_name: "mutate".to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            rustx::tools::executor::ToolExecutionContext {
                conversation_id: runtime_bundle.conversation_id(),
                execution_id: None,
                cancellation: rustx::runtime::CancellationSignal::new(),
                cancellation_reason: rustx::runtime::types::CancellationReason::UserRequested,
                workspace: runtime_bundle.workspace(),
                progress: &progress,
                artifacts: runtime_bundle.artifacts(),
                environment: runtime_bundle.environment(),
            },
        )
        .await;
        assert!(matches!(
            result.status,
            rustx::tools::types::ToolExecutionStatus::Success
        ));
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            runtime.wait_for_change(initial_epoch),
        )
        .await
        .expect("HTTP tools/list_changed notification");
        assert_eq!(
            runtime
                .list_tools()
                .await
                .expect("HTTP refreshed tools/list")
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "new_tool"]
        );
        // HTTP cancellation: the server-side cancellation context becomes
        // observable (the fixture's `slow` tool waits on it), not only that
        // rustX returned `Cancelled`.
        let slow_executor = definitions
            .iter()
            .find(|(definition, _)| definition.name == "slow")
            .map(|(_, executor)| executor.clone())
            .expect("HTTP slow executor");
        let slow_cancellation = rustx::runtime::CancellationSignal::new();
        let slow_future = rustx::tools::executor::ToolExecutor::execute(
            slow_executor.as_ref(),
            rustx::tools::types::ToolInvocation {
                call_id: rustx::runtime::identity::ToolCallId::new("http-slow"),
                tool_id: rustx::runtime::identity::ToolId::new(rustx::tools::mcp::mcp_tool_id(
                    &server_id, "slow",
                )),
                tool_name: "slow".to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            rustx::tools::executor::ToolExecutionContext {
                conversation_id: runtime_bundle.conversation_id(),
                execution_id: None,
                cancellation: slow_cancellation.clone(),
                cancellation_reason: rustx::runtime::types::CancellationReason::UserRequested,
                workspace: runtime_bundle.workspace(),
                progress: &progress,
                artifacts: runtime_bundle.artifacts(),
                environment: runtime_bundle.environment(),
            },
        );
        tokio::pin!(slow_future);
        tokio::select! {
            () = slow_started.notified() => {}
            result = &mut slow_future => panic!("HTTP slow call completed before cancellation: {result:?}"),
        }
        slow_cancellation.cancel();
        let slow_result = slow_future.await;
        assert!(matches!(
            slow_result.status,
            rustx::tools::types::ToolExecutionStatus::Cancelled { .. }
        ));
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            cancel_observed.notified(),
        )
        .await
        .expect("the HTTP fixture server must observe the cancellation notification");

        runtime
            .close()
            .await
            .expect("the owned stdio unit must publish physical settlement");
        cancellation.cancel();
        server_task.abort();
        let _ = server_task.await;
    }

    /// Pagination: the official-rmcp fixture serves a five-tool catalog two
    /// tools per page; the canonical registry contains the finite complete
    /// sorted catalog.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn paginated_tools_list_produces_the_finite_complete_sorted_catalog() {
        if rustx::tools::mcp::fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        let server_id = rustx::runtime::identity::McpServerId::new("paged-fixture");
        let binding = rustx::tools::mcp::McpServerBinding {
            transport: rustx::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: rustx::tools::mcp::fixture::fixture_spawn_args(
                    "unix_tests::paginated_tools_list_produces_the_finite_complete_sorted_catalog",
                ),
                cwd: None,
                environment: std::collections::BTreeMap::from([
                    (
                        rustx::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                        "1".to_owned(),
                    ),
                    (
                        rustx::tools::mcp::fixture::PAGE_SIZE_ENV.to_owned(),
                        "2".to_owned(),
                    ),
                ]),
            },
            policy: rustx::tools::types::ToolInvocationPolicy::default(),
        };
        let runtime = rustx::tools::mcp::McpServerRuntime::connect(
            &server_id,
            &binding,
            &workspace,
            Arc::new(rustx::tools::mcp::McpInvalidationState::new()),
        )
        .await
        .expect("paged MCP connect");
        let tools = runtime.list_tools().await.expect("paged tools/list");
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta", "delta", "echo", "gamma"],
            "the canonical catalog is the finite complete sorted set"
        );
        let registry = rustx::tools::executor::ToolRegistry::new()
            .compose(rustx::tools::mcp::definitions(
                &server_id,
                binding.policy,
                &runtime,
                tools,
            ))
            .expect("registry");
        assert_eq!(
            registry
                .definitions()
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta", "delta", "echo", "gamma"]
        );
        runtime
            .close()
            .await
            .expect("the owned stdio unit must publish physical settlement");
    }

    /// Waits for a marker file with a strict deadline (a deadlock guard,
    /// never a synchronization mechanism).
    async fn wait_for_file(path: &std::path::Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "marker file never appeared: {}",
                path.display()
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
