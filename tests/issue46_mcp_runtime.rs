//! MCP protocol-revision negotiation (Issue #46).
//!
//! Every protocol assertion drives the real `McpServerRuntime` connect path
//! against an official-rmcp fixture server whose supported revision set is
//! configured deterministically. Nothing here inspects a version string
//! without also exercising discovery.
//!
//! # What the fixture can and cannot express
//!
//! rmcp's server narrows its revisions through
//! `ServerHandler::supported_protocol_versions`, which bounds
//! `server/discover` advertisement, `initialize` negotiation, and
//! per-request version validation alike. That is the seam these tests use,
//! and it exercises rmcp's real `UNSUPPORTED_PROTOCOL_VERSION` retry walk.
//!
//! What an rmcp server cannot impersonate is a peer that does not implement
//! `server/discover` at all: rmcp's server handshake treats *any*
//! non-`initialize` opener as an inline-lifecycle opener and permanently
//! requires self-contained request metadata on that session, even after it
//! answers the opener with `METHOD_NOT_FOUND`. So the
//! `ClientLifecycleMode::Auto` fallback onto the legacy
//! `initialize`/`notifications/initialized` handshake — which real 2025-era
//! TypeScript/Python SDK servers do take — is rmcp's own tested contract and
//! is deliberately not re-simulated here.

#[cfg(all(unix, feature = "mcp-fixture"))]
mod unix_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use rmcp::model::ProtocolVersion;
    use rustx::runtime::identity::McpServerId;
    use rustx::tools::mcp::fixture::{self, FixtureServer, PROTOCOL_VERSIONS_ENV};
    use rustx::tools::mcp::{
        McpError, McpInvalidationState, McpServerBinding, McpServerRuntime, McpTransportConfig,
    };
    use rustx::tools::types::ToolInvocationPolicy;

    struct NoProgress;

    impl rustx::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
    }

    /// A model catalog whose only model is never invoked: these tests
    /// compose the runtime, they do not run an attempt.
    const MODELS_JSON: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "https://local.fixture.invalid/v1",
      "apiKey": "$RUSTX_ISSUE46_KEY",
      "models": [
        {
          "id": "composed-model",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 4096,
          "capabilities": {
            "inputModalities": ["text"],
            "outputModalities": ["text"],
            "toolCalls": true,
            "reasoning": false
          },
          "compat": {"chatReasoningReplay": "omit"}
        }
      ]
    }
  }
}"#;

    /// A stdio binding that re-runs this test binary as its own MCP server,
    /// with the given fixture protocol behavior.
    fn fixture_binding(test_name: &str, environment: BTreeMap<String, String>) -> McpServerBinding {
        let mut environment = environment;
        environment.insert(fixture::FIXTURE_MODE_ENV.to_owned(), "1".to_owned());
        McpServerBinding {
            transport: McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture::fixture_spawn_args(test_name),
                cwd: None,
                environment,
            },
            policy: ToolInvocationPolicy::default(),
        }
    }

    async fn connect(
        test_name: &str,
        environment: BTreeMap<String, String>,
        workspace_dir: &tempfile::TempDir,
    ) -> Result<Arc<McpServerRuntime>, McpError> {
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        McpServerRuntime::connect(
            &McpServerId::new("fixture"),
            &fixture_binding(test_name, environment),
            &workspace,
            Arc::new(McpInvalidationState::new()),
        )
        .await
    }

    /// A server that speaks every revision the SDK knows negotiates the
    /// newest one rustX offers, and its catalog is discovered over that
    /// revision's inline lifecycle.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_current_revision_server_negotiates_the_newest_shared_revision() {
        if fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = connect(
            "unix_tests::a_current_revision_server_negotiates_the_newest_shared_revision",
            BTreeMap::new(),
            &workspace_dir,
        )
        .await
        .expect("a current-revision server must connect");
        assert_eq!(
            runtime.protocol_version(),
            &ProtocolVersion::V_2026_07_28,
            "the newest mutually supported revision wins"
        );
        assert_eq!(
            runtime
                .list_tools()
                .await
                .expect("tools/list")
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "mutate", "slow"],
            "discovery runs over the negotiated revision, once"
        );
        runtime.close().await.expect("physical settlement");
    }

    /// A server that speaks only one 2025-era revision is negotiated down to
    /// exactly that revision, through rmcp's real
    /// `UNSUPPORTED_PROTOCOL_VERSION` retry walk, and publishes the same
    /// catalog over it.
    ///
    /// Invalidation must also follow the negotiated revision:
    /// `subscriptions/listen` does not exist before 2026-07-28, so rustX must
    /// take `tools/list_changed` from the plain server notification that
    /// revision does define.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_2025_era_server_negotiates_that_revision_and_its_invalidation() {
        if fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = connect(
            "unix_tests::a_2025_era_server_negotiates_that_revision_and_its_invalidation",
            BTreeMap::from([(PROTOCOL_VERSIONS_ENV.to_owned(), "2025-06-18".to_owned())]),
            &workspace_dir,
        )
        .await
        .expect("a 2025-era server must connect");
        assert_eq!(
            runtime.protocol_version(),
            &ProtocolVersion::V_2025_06_18,
            "rustX negotiates the one revision the server actually speaks"
        );
        let tools = runtime.list_tools().await.expect("tools/list");
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "mutate", "slow"],
            "the 2025-era connection publishes the catalog exactly once"
        );

        // `mutate` flips the catalog and emits the notification. Exactly one
        // invalidation mechanism is installed, so exactly one epoch advance
        // is observable.
        let initial_epoch = runtime.change_epoch();
        let definitions = rustx::tools::mcp::definitions(
            &McpServerId::new("fixture"),
            ToolInvocationPolicy::default(),
            &runtime,
            tools,
        );
        let (definition, executor) = definitions
            .iter()
            .find(|(definition, _)| definition.name == "mutate")
            .expect("mutate definition");
        let artifacts_dir = tempfile::tempdir().expect("artifacts");
        let bundle = rustx::tools::runtime::ConversationToolRuntime::new(
            rustx::runtime::identity::ConversationId::new("issue46-legacy"),
            workspace_dir.path(),
            artifacts_dir.path(),
        )
        .expect("tool runtime");
        let result = rustx::tools::executor::ToolExecutor::execute(
            executor.as_ref(),
            rustx::tools::types::ToolInvocation {
                call_id: rustx::runtime::identity::ToolCallId::new("legacy-mutate"),
                tool_id: definition.id.clone(),
                tool_name: "mutate".to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            rustx::tools::executor::ToolExecutionContext {
                conversation_id: bundle.conversation_id(),
                execution_id: None,
                cancellation: rustx::runtime::CancellationSignal::new(),
                workspace: bundle.workspace(),
                progress: &NoProgress,
                artifacts: bundle.artifacts(),
                environment: bundle.environment(),
            },
        )
        .await;
        assert!(matches!(
            result.status,
            rustx::tools::types::ToolExecutionStatus::Success
        ));
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            runtime.wait_for_change(initial_epoch),
        )
        .await
        .expect("the pre-2026 tools/list_changed notification must invalidate");
        assert_eq!(
            runtime.change_epoch(),
            initial_epoch + 1,
            "one notification advances the shared epoch exactly once"
        );
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
        runtime.close().await.expect("physical settlement");
    }

    /// A server that speaks only a revision no MCP SDK knows shares no
    /// revision with rustX, and the failure is a bounded, precise
    /// compatibility error rather than a generic transport failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_server_with_no_shared_revision_fails_with_a_compatibility_error() {
        if fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let error = connect(
            "unix_tests::a_server_with_no_shared_revision_fails_with_a_compatibility_error",
            BTreeMap::from([(PROTOCOL_VERSIONS_ENV.to_owned(), "1999-01-01".to_owned())]),
            &workspace_dir,
        )
        .await
        .expect_err("a server with no shared revision must not connect");
        let McpError::ProtocolCompatibility(detail) = &error else {
            panic!("expected a protocol compatibility failure, got: {error:?}");
        };
        assert!(
            detail.contains("2026-07-28") && detail.contains("1999-01-01"),
            "the failure must name both sides: {detail}"
        );
    }

    /// rustX offers every revision the resolved rmcp build knows, newest
    /// first, and no revision is hard-coded as the only acceptable one.
    #[test]
    fn the_offered_revision_set_is_the_sdk_set_newest_first() {
        let offered = rustx::tools::mcp::supported_protocol_versions();
        let mut expected = ProtocolVersion::KNOWN_VERSIONS.to_vec();
        expected.sort_by(|left, right| right.as_str().cmp(left.as_str()));
        assert_eq!(offered, expected);
        assert_eq!(offered.first(), Some(&ProtocolVersion::V_2026_07_28));
        assert!(
            offered.len() > 1,
            "negotiation must have more than one revision to negotiate with"
        );
    }

    /// The whole ownership chain, end to end: one named `mcpServers` entry
    /// plus one keyed `mcpToolPolicies` entry become exactly one runtime
    /// server identity whose stdio command/args/env reach the real stdio
    /// transport, and whose tools reach the committed capability snapshot as
    /// canonical tools carrying the overlaid policy.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one complete composition fixture
    async fn a_named_map_entry_composes_into_exactly_one_runtime_server() {
        if fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let root = tempfile::tempdir().expect("temp root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let program = std::env::current_exe()
            .expect("test executable")
            .display()
            .to_string();
        let args = fixture::fixture_spawn_args(
            "unix_tests::a_named_map_entry_composes_into_exactly_one_runtime_server",
        );
        let session = serde_json::json!({
            "conversationId": "conv-46",
            "agentId": "agent-46",
            "model": {"model": "local/composed-model"},
            "context": {"reserveTokens": 1024, "keepRecentTokens": 8192},
            "mcpServers": {
                "exa-local": {
                    "type": "stdio",
                    "command": program,
                    "args": args,
                    "env": {fixture::FIXTURE_MODE_ENV: "1"},
                },
            },
            "mcpToolPolicies": {
                "exa-local": {"execution": "background_only", "concurrency": "parallel"},
            },
        });
        let models_path = root.path().join("models.json");
        let session_path = root.path().join("session.json");
        std::fs::write(&models_path, MODELS_JSON).expect("models.json");
        std::fs::write(
            &session_path,
            serde_json::to_vec_pretty(&session).expect("session json"),
        )
        .expect("session.json");

        let runtime = rustx::local_runtime::composition::LocalConversationRuntime::compose(
            &rustx::local_runtime::composition::LocalRuntimePaths {
                models: models_path,
                session: session_path,
                workspace,
                runtime_root: root.path().join("private"),
            },
            &rustx::local_runtime::composition::LocalRuntimeDependencies {
                credentials: Arc::new(rustx::model::catalog::MapCredentialEnvironment::new([(
                    "RUSTX_ISSUE46_KEY".to_owned(),
                    "issue46-secret".to_owned(),
                )])),
                ..Default::default()
            },
        )
        .await
        .expect("composition must succeed with a named-map MCP entry");

        let snapshot = runtime.capability().current_snapshot();
        let mcp_tools = snapshot
            .tool_registry()
            .definitions()
            .iter()
            .filter(|definition| {
                matches!(
                    &definition.origin,
                    rustx::tools::types::ToolOrigin::Mcp { server_id }
                        if server_id.as_str() == "exa-local"
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            mcp_tools
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "mutate", "slow"],
            "one map entry yields exactly one server's canonical tools"
        );
        for definition in &mcp_tools {
            assert_eq!(
                definition.execution_policy,
                rustx::tools::types::ToolExecutionPolicy::BackgroundOnly,
                "the keyed policy overlay reaches the canonical definition"
            );
            assert_eq!(
                definition.concurrency_policy,
                rustx::tools::types::ToolConcurrencyPolicy::Parallel
            );
        }
    }

    /// An inline-lifecycle connection opens exactly one `subscriptions/listen`
    /// stream: negotiation never installs a second invalidation mechanism
    /// alongside it, and never publishes a tool twice.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_inline_connection_opens_exactly_one_subscription() {
        use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService,
        };

        let cancellation = tokio_util::sync::CancellationToken::new();
        let mut server_config = StreamableHttpServerConfig::default();
        server_config.cancellation_token = cancellation.child_token();
        server_config.sse_keep_alive = None;
        let served = FixtureServer::with_list_changed();
        let listen_calls = served.listen_calls.clone();
        let service = StreamableHttpService::<FixtureServer, LocalSessionManager>::new(
            move || Ok(served.clone()),
            Arc::default(),
            server_config,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("HTTP listener");
        let address = listener.local_addr().expect("HTTP address");
        let server_task = tokio::spawn(async move {
            let _ = axum::serve(listener, axum::Router::new().nest_service("/mcp", service)).await;
        });

        let workspace_dir = tempfile::tempdir().expect("workspace");
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        let runtime = McpServerRuntime::connect(
            &McpServerId::new("http-fixture"),
            &McpServerBinding {
                transport: McpTransportConfig::StreamableHttp {
                    endpoint: format!("http://{address}/mcp"),
                    headers: BTreeMap::new(),
                },
                policy: ToolInvocationPolicy::default(),
            },
            &workspace,
            Arc::new(McpInvalidationState::new()),
        )
        .await
        .expect("HTTP MCP connect");
        assert_eq!(runtime.protocol_version(), &ProtocolVersion::V_2026_07_28);
        assert_eq!(
            listen_calls.load(std::sync::atomic::Ordering::Acquire),
            1,
            "one connection opens one subscription"
        );
        let tools = runtime.list_tools().await.expect("tools/list");
        // Composing the canonical registry proves no name or id is published
        // twice: the registry rejects a collision.
        let registry = rustx::tools::executor::ToolRegistry::new()
            .compose(rustx::tools::mcp::definitions(
                &McpServerId::new("http-fixture"),
                ToolInvocationPolicy::default(),
                &runtime,
                tools,
            ))
            .expect("no duplicate published tool");
        assert_eq!(registry.definitions().len(), 3);
        runtime.close().await.expect("physical settlement");
        cancellation.cancel();
        server_task.abort();
        let _ = server_task.await;
    }
}
