//! Local MCP integration using the official rmcp server implementation.

#[cfg(all(unix, feature = "mcp-fixture"))]
mod unix_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, JsonObject,
        ProgressNotificationParam, ServerCapabilities, ServerInfo, SubscriptionFilter, Tool,
    };
    use rmcp::service::{RequestContext, SubscriptionContext};
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };
    use rmcp::{RoleServer, ServerHandler, ServiceExt};
    use tokio_util::sync::CancellationToken;

    #[derive(Clone, Default)]
    struct FixtureServer {
        changed: Arc<AtomicBool>,
        sink: Arc<tokio::sync::Mutex<Option<rmcp::service::SubscriptionSink>>>,
        slow_started: Arc<tokio::sync::Notify>,
        cancel_observed: Arc<tokio::sync::Notify>,
        list_changed_supported: bool,
    }

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

    impl ServerHandler for FixtureServer {
        fn get_info(&self) -> ServerInfo {
            let mut capabilities = ServerCapabilities::builder().enable_tools();
            if self.list_changed_supported {
                capabilities = capabilities.enable_tool_list_changed();
            }
            ServerInfo::new(capabilities.build())
        }

        fn get_tool(&self, name: &str) -> Option<Tool> {
            Some(fixture_tool_named(name))
        }

        fn list_tools(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<
            Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>,
        > + Send {
            let tools = if self.changed.load(Ordering::Acquire) {
                vec![fixture_tool_named("echo"), fixture_tool_named("new_tool")]
            } else {
                vec![
                    fixture_tool_named("echo"),
                    fixture_tool_named("mutate"),
                    fixture_tool_named("slow"),
                ]
            };
            let result = rmcp::model::ListToolsResult {
                tools,
                ..Default::default()
            };
            std::future::ready(Ok(result))
        }

        fn accepted_subscription_filter(
            &self,
            _requested: &SubscriptionFilter,
        ) -> Option<SubscriptionFilter> {
            self.list_changed_supported
                .then(|| SubscriptionFilter::builder().tools_list_changed().build())
        }

        async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
            *self.sink.lock().await = Some(context.sink().clone());
            context.cancelled().await;
            Ok(())
        }

        fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + Send
        {
            let changed = self.changed.clone();
            let sink = self.sink.clone();
            let slow_started = self.slow_started.clone();
            let cancel_observed = self.cancel_observed.clone();
            async move {
                if request.name == "echo" {
                    Ok(CallToolResult::success(vec![ContentBlock::text("fixture echo")]).into())
                } else if request.name == "mutate" {
                    changed.store(true, Ordering::Release);
                    if let Some(token) = context.meta.get_progress_token() {
                        context
                            .peer
                            .notify_progress(
                                ProgressNotificationParam::new(token, 0.5)
                                    .with_total(3.5)
                                    .with_message("fractional"),
                            )
                            .await
                            .map_err(|error| {
                                rmcp::ErrorData::internal_error(
                                    format!("cannot notify progress: {error}"),
                                    None,
                                )
                            })?;
                    }
                    let sink = sink.lock().await.clone().ok_or_else(|| {
                        rmcp::ErrorData::internal_error("subscription is not ready", None)
                    })?;
                    sink.notify_tool_list_changed().await.map_err(|error| {
                        rmcp::ErrorData::internal_error(
                            format!("cannot notify tool list change: {error}"),
                            None,
                        )
                    })?;
                    Ok(CallToolResult::success(vec![ContentBlock::text("fixture changed")]).into())
                } else if request.name == "slow" {
                    if let Some(token) = context.meta.get_progress_token() {
                        context
                            .peer
                            .notify_progress(ProgressNotificationParam::new(token, 0.25))
                            .await
                            .map_err(|error| {
                                rmcp::ErrorData::internal_error(
                                    format!("cannot notify slow-call progress: {error}"),
                                    None,
                                )
                            })?;
                    }
                    slow_started.notify_one();
                    context.ct.cancelled().await;
                    cancel_observed.notify_one();
                    Ok(
                        CallToolResult::success(vec![ContentBlock::text("fixture cancelled")])
                            .into(),
                    )
                } else {
                    Err(rmcp::ErrorData::method_not_found::<
                        rmcp::model::CallToolRequestMethod,
                    >())
                }
            }
        }
    }

    fn fixture_tool_named(name: &str) -> Tool {
        let mut tool = Tool::default();
        tool.name = name.to_owned().into();
        tool.description = Some(format!("fixture {name}").into());
        let mut schema = JsonObject::new();
        schema.insert("type".to_owned(), serde_json::json!("object"));
        schema.insert("properties".to_owned(), serde_json::json!({}));
        schema.insert("additionalProperties".to_owned(), serde_json::json!(false));
        tool.input_schema = Arc::new(schema);
        tool
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn stdio_discovery_and_call_use_canonical_executor() {
        if std::env::var_os("RUSTX_M7_MCP_FIXTURE").is_some() {
            let server = FixtureServer {
                list_changed_supported: true,
                ..FixtureServer::default()
            }
            .serve(rmcp::transport::stdio())
            .await
            .expect("fixture server");
            server.waiting().await.expect("fixture server wait");
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let artifacts_dir = tempfile::tempdir().expect("artifacts");
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        let epoch = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let config = rustx::tools::mcp::McpServerConfig {
            server_id: rustx::runtime::identity::McpServerId::new("fixture"),
            transport: rustx::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: vec![
                    "--exact".to_owned(),
                    "unix_tests::stdio_discovery_and_call_use_canonical_executor".to_owned(),
                    "--quiet".to_owned(),
                    "--nocapture".to_owned(),
                    "--test-threads".to_owned(),
                    "1".to_owned(),
                ],
                cwd: None,
                environment: std::collections::BTreeMap::from([(
                    "RUSTX_M7_MCP_FIXTURE".to_owned(),
                    "1".to_owned(),
                )]),
            },
            policy: rustx::tools::types::ToolInvocationPolicy::default(),
        };
        let runtime = rustx::tools::mcp::McpServerRuntime::connect(&config, &workspace, epoch)
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
            rustx::tools::mcp::definitions(&config.server_id, config.policy, &runtime, tools);
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
        runtime.close().await;
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
        let config = rustx::tools::mcp::McpServerConfig {
            server_id: rustx::runtime::identity::McpServerId::new("http-fixture"),
            transport: rustx::tools::mcp::McpTransportConfig::StreamableHttp {
                endpoint: format!("http://{address}/mcp"),
                headers: std::collections::BTreeMap::new(),
            },
            policy: rustx::tools::types::ToolInvocationPolicy::default(),
        };
        let runtime = rustx::tools::mcp::McpServerRuntime::connect(
            &config,
            &workspace,
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
        )
        .await
        .expect("HTTP MCP connect");
        let tools = runtime.list_tools().await.expect("HTTP tools/list");
        assert_eq!(tools.len(), 3);
        let definitions =
            rustx::tools::mcp::definitions(&config.server_id, config.policy, &runtime, tools);
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
                    &config.server_id,
                    "mutate",
                )),
                tool_name: "mutate".to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            rustx::tools::executor::ToolExecutionContext {
                conversation_id: runtime_bundle.conversation_id(),
                execution_id: None,
                cancellation: rustx::runtime::CancellationSignal::new(),
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
        runtime.close().await;
        cancellation.cancel();
        server_task.abort();
        let _ = server_task.await;
    }
}
