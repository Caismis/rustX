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
            rustx::tools::executor::ToolExecutionContext::new(
                runtime_bundle.conversation_id(),
                None,
                rustx::runtime::ExecutionCancellation::detached(
                    rustx::runtime::CancellationSignal::new(),
                    rustx::runtime::types::CancellationReason::UserRequested,
                ),
                runtime_bundle.workspace(),
                &progress,
                runtime_bundle.artifacts(),
                runtime_bundle.tool_output(),
                runtime_bundle.environment(),
            ),
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
            rustx::tools::executor::ToolExecutionContext::new(
                runtime_bundle.conversation_id(),
                None,
                rustx::runtime::ExecutionCancellation::detached(
                    slow_cancellation.clone(),
                    rustx::runtime::types::CancellationReason::UserRequested,
                ),
                runtime_bundle.workspace(),
                &slow_progress,
                runtime_bundle.artifacts(),
                runtime_bundle.tool_output(),
                runtime_bundle.environment(),
            ),
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

    /// The real rmcp stdio boundary feeds complete MCP text into the shared
    /// Tool Plane normalizer. Exact-boundary output remains direct JSON-like
    /// text, while boundary-plus-one and collectively oversized blocks use
    /// one complete managed spill with typed continuation metadata.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn foreground_mcp_result_normalization_is_collective_and_complete() {
        async fn invoke(
            bytes: Option<usize>,
            blocks: Option<&str>,
            server_name: &str,
        ) -> (
            tempfile::TempDir,
            tempfile::TempDir,
            rustx::tools::types::ToolExecutionResult,
        ) {
            let directory = tempfile::tempdir().expect("fixture directory");
            let workspace = rustx::tools::Workspace::new(directory.path()).expect("workspace");
            let server_id = rustx::runtime::identity::McpServerId::new(server_name);
            let mut environment = std::collections::BTreeMap::from([(
                rustx::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                "1".to_owned(),
            )]);
            if let Some(bytes) = bytes {
                environment.insert(
                    rustx::tools::mcp::fixture::RESULT_BYTES_ENV.to_owned(),
                    bytes.to_string(),
                );
            }
            if let Some(blocks) = blocks {
                environment.insert(
                    rustx::tools::mcp::fixture::RESULT_BLOCK_BYTES_ENV.to_owned(),
                    blocks.to_owned(),
                );
            }
            let binding = rustx::tools::mcp::McpServerBinding {
                transport: rustx::tools::mcp::McpTransportConfig::Stdio {
                    program: std::env::current_exe()
                        .expect("test executable")
                        .display()
                        .to_string(),
                    args: rustx::tools::mcp::fixture::fixture_spawn_args(
                        "unix_tests::foreground_mcp_result_normalization_is_collective_and_complete",
                    ),
                    cwd: None,
                    environment,
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
            .expect("MCP connect");
            let tools = runtime.list_tools().await.expect("tools/list");
            let definitions =
                rustx::tools::mcp::definitions(&server_id, binding.policy, &runtime, tools);
            let (definition, executor) = definitions
                .into_iter()
                .find(|(definition, _)| definition.name == "echo")
                .expect("echo definition");
            let artifacts = tempfile::tempdir().expect("artifacts");
            let tool_runtime = rustx::tools::runtime::ConversationToolRuntime::new(
                rustx::runtime::identity::ConversationId::new(format!("mcp-{server_name}")),
                directory.path(),
                artifacts.path(),
            )
            .expect("tool runtime");
            let progress = NoProgress;
            let result = rustx::tools::executor::ToolExecutor::execute(
                executor.as_ref(),
                rustx::tools::types::ToolInvocation {
                    call_id: rustx::runtime::identity::ToolCallId::new(format!(
                        "call-{server_name}"
                    )),
                    tool_id: definition.id,
                    tool_name: "echo".to_owned(),
                    mode: rustx::tools::types::ToolInvocationMode::Foreground,
                    arguments: serde_json::json!({}),
                },
                rustx::tools::executor::ToolExecutionContext::new(
                    tool_runtime.conversation_id(),
                    None,
                    rustx::runtime::ExecutionCancellation::detached(
                        rustx::runtime::CancellationSignal::new(),
                        rustx::runtime::types::CancellationReason::UserRequested,
                    ),
                    tool_runtime.workspace(),
                    &progress,
                    tool_runtime.artifacts(),
                    tool_runtime.tool_output(),
                    tool_runtime.environment(),
                ),
            )
            .await;
            runtime.close().await.expect("MCP close");
            (directory, artifacts, result)
        }

        if rustx::tools::mcp::fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }

        let (_exact_directory, _exact_artifacts, exact) = invoke(
            Some(rustx::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES),
            None,
            "mcp-exact",
        )
        .await;
        assert_eq!(
            exact.status,
            rustx::tools::types::ToolExecutionStatus::Success
        );
        assert!(exact.managed_output.is_none());
        assert!(exact.truncation.is_none());
        assert!(matches!(
            exact.content.first(),
            Some(rustx::tools::types::ToolResultContent::Text(text))
                if text.text.len() == rustx::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES
        ));

        let (_over_directory, _over_artifacts, over) = invoke(
            Some(rustx::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1),
            None,
            "mcp-over",
        )
        .await;
        assert_eq!(
            over.status,
            rustx::tools::types::ToolExecutionStatus::Success
        );
        let Some(rustx::tools::types::ManagedOutputContinuation::Complete { locator }) =
            &over.managed_output
        else {
            panic!("boundary-plus-one MCP result must be Complete: {over:?}");
        };
        assert_eq!(
            std::fs::read(locator).expect("complete MCP spill"),
            vec![b'x'; rustx::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1]
        );
        assert_eq!(
            std::fs::read_dir(locator.parent().expect("results directory"))
                .expect("results directory")
                .count(),
            1
        );
        assert!(over.content.iter().any(|content| matches!(
            content,
            rustx::tools::types::ToolResultContent::Text(text) if text.text.contains("Read or Grep")
        )));

        // Each block is below 16 KiB, but the deterministic newline-joined
        // aggregate is above it. Per-block truncation would incorrectly
        // classify this as a direct result; the shared capture spills once.
        let first = rustx::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES / 2;
        let second = rustx::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES - first;
        let (_aggregate_directory, _aggregate_artifacts, aggregate) =
            invoke(None, Some(&format!("{first},{second}")), "mcp-aggregate").await;
        assert_eq!(
            aggregate.status,
            rustx::tools::types::ToolExecutionStatus::Success
        );
        assert!(matches!(
            aggregate.managed_output,
            Some(rustx::tools::types::ManagedOutputContinuation::Complete { .. })
        ));
        assert!(aggregate.truncation.is_some());
    }

    /// A real background MCP call reuses the dispatch-owned output file for
    /// the complete final result. The terminal canonical publication keeps
    /// the typed continuation while staying inside the model-facing bound.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn background_mcp_result_reuses_the_dispatch_owned_locator() {
        if rustx::tools::mcp::fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let directory = tempfile::tempdir().expect("fixture directory");
        let workspace = rustx::tools::Workspace::new(directory.path()).expect("workspace");
        let server_id = rustx::runtime::identity::McpServerId::new("mcp-background");
        let binding = rustx::tools::mcp::McpServerBinding {
            transport: rustx::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: rustx::tools::mcp::fixture::fixture_spawn_args(
                    "unix_tests::background_mcp_result_reuses_the_dispatch_owned_locator",
                ),
                cwd: None,
                environment: std::collections::BTreeMap::from([
                    (
                        rustx::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                        "1".to_owned(),
                    ),
                    (
                        rustx::tools::mcp::fixture::RESULT_BYTES_ENV.to_owned(),
                        "100000".to_owned(),
                    ),
                ]),
            },
            policy: rustx::tools::types::ToolInvocationPolicy::default(),
        };
        let mcp_runtime = rustx::tools::mcp::McpServerRuntime::connect(
            &server_id,
            &binding,
            &workspace,
            Arc::new(rustx::tools::mcp::McpInvalidationState::new()),
        )
        .await
        .expect("MCP connect");
        let tools = mcp_runtime.list_tools().await.expect("tools/list");
        let (definition, executor) =
            rustx::tools::mcp::definitions(&server_id, binding.policy, &mcp_runtime, tools)
                .into_iter()
                .find(|(definition, _)| definition.name == "echo")
                .expect("echo definition");
        let artifacts = tempfile::tempdir().expect("artifacts");
        let tool_runtime = rustx::tools::runtime::ConversationToolRuntime::new(
            rustx::runtime::identity::ConversationId::new("mcp-background-conversation"),
            directory.path(),
            artifacts.path(),
        )
        .expect("tool runtime");
        let invocation = rustx::tools::types::ToolInvocation {
            call_id: rustx::runtime::identity::ToolCallId::new("mcp-background-call"),
            tool_id: definition.id,
            tool_name: "echo".to_owned(),
            mode: rustx::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = tool_runtime
            .background()
            .prepare_dispatch(
                &invocation,
                &executor,
                rustx::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare background MCP dispatch");
        let rustx::tools::background::BackgroundDispatchOutcome::Accepted {
            execution_id,
            result: accepted,
        } = tool_runtime
            .background()
            .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
            .expect("commit background MCP dispatch")
        else {
            panic!("background MCP dispatch was not accepted");
        };
        let advertised = match &accepted.content[0] {
            rustx::tools::types::ToolResultContent::Json { value } => value["output_path"]
                .as_str()
                .expect("accepted output path")
                .to_owned(),
            content => panic!("accepted result is JSON: {content:?}"),
        };
        assert!(std::path::Path::new(&advertised).exists());
        let terminal = tool_runtime
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .expect("terminal MCP result");
        assert_eq!(
            terminal.state,
            rustx::tools::background::BackgroundLifecycle::Succeeded
        );
        let result = terminal.result.expect("terminal result");
        assert_eq!(
            result.managed_output,
            Some(rustx::tools::ManagedOutputContinuation::Complete {
                locator: std::path::PathBuf::from(&advertised),
            })
        );
        assert_eq!(
            std::fs::read(&advertised).expect("complete MCP output"),
            vec![b'x'; 100_000]
        );
        assert_eq!(
            std::fs::read_dir(tool_runtime.tool_output().root().join("results"))
                .expect("results directory")
                .count(),
            0,
            "background MCP never allocates a secondary result spill"
        );
        assert_eq!(
            std::fs::read_dir(tool_runtime.tool_output().root().join("tasks"))
                .expect("tasks directory")
                .count(),
            1
        );
        let batch = tool_runtime
            .mailbox()
            .select_pending_batch()
            .expect("terminal mailbox")
            .expect("terminal inbound");
        let rustx::message::types::UserContentBlock::Text(text) =
            &batch.items()[0].message().content[0]
        else {
            panic!("MCP terminal publication is text-only");
        };
        assert!(text.text.len() <= rustx::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES + 256);
        assert!(text.text.contains(&advertised));
        assert!(text.text.contains("Read or Grep"));
        mcp_runtime.close().await.expect("MCP close");
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
            rustx::tools::executor::ToolExecutionContext::new(
                runtime_bundle.conversation_id(),
                None,
                rustx::runtime::ExecutionCancellation::detached(
                    rustx::runtime::CancellationSignal::new(),
                    rustx::runtime::types::CancellationReason::UserRequested,
                ),
                runtime_bundle.workspace(),
                &progress,
                runtime_bundle.artifacts(),
                runtime_bundle.tool_output(),
                runtime_bundle.environment(),
            ),
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
            rustx::tools::executor::ToolExecutionContext::new(
                runtime_bundle.conversation_id(),
                None,
                rustx::runtime::ExecutionCancellation::detached(
                    slow_cancellation.clone(),
                    rustx::runtime::types::CancellationReason::UserRequested,
                ),
                runtime_bundle.workspace(),
                &progress,
                runtime_bundle.artifacts(),
                runtime_bundle.tool_output(),
                runtime_bundle.environment(),
            ),
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
