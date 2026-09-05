//! MCP protocol-revision negotiation (Issue #46).
//!
//! Every protocol assertion drives the real `McpServerRuntime` connect path
//! against a fixture whose protocol behavior is configured deterministically.
//! Nothing here inspects a version string without also exercising the
//! connection and discovery it belongs to.
//!
//! # The two fixture kinds, and why both exist
//!
//! Most tests use the official-rmcp [`FixtureServer`], narrowing its
//! revisions through `ServerHandler::supported_protocol_versions` — the seam
//! that bounds `server/discover` advertisement, `initialize` negotiation, and
//! per-request version validation alike. That covers rmcp's real
//! `UNSUPPORTED_PROTOCOL_VERSION` retry walk *inside* the inline lifecycle.
//!
//! An rmcp server cannot express the one peer that matters most for
//! interoperability, though: a server that has never heard of
//! `server/discover`. rmcp's server handshake treats any non-`initialize`
//! opener as an inline-lifecycle opener and permanently requires
//! self-contained request metadata on that session, even after answering the
//! opener with `METHOD_NOT_FOUND`. So
//! `a_discover_less_server_falls_back_to_the_legacy_initialize_handshake`
//! uses [`fixture::legacy`], a minimal hand-written pre-2026 wire fixture,
//! to cover rustX's own legacy-path behavior end to end.

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

    /// Executes one discovered tool through the canonical executor boundary,
    /// the same path the Agent Loop uses.
    async fn call_canonical_tool(
        runtime: &Arc<McpServerRuntime>,
        server_id: &McpServerId,
        tools: Vec<rustx::tools::mcp::CanonicalMcpTool>,
        name: &str,
        workspace_dir: &tempfile::TempDir,
        conversation: &str,
    ) -> rustx::tools::types::ToolExecutionResult {
        let definitions = rustx::tools::mcp::definitions(
            server_id,
            ToolInvocationPolicy::default(),
            runtime,
            tools,
        );
        let (definition, executor) = definitions
            .iter()
            .find(|(definition, _)| definition.name == name)
            .expect("the tool must be discovered");
        let artifacts_dir = tempfile::tempdir().expect("artifacts");
        let bundle = rustx::tools::runtime::ConversationToolRuntime::new(
            rustx::runtime::identity::ConversationId::new(conversation),
            workspace_dir.path(),
            artifacts_dir.path(),
        )
        .expect("tool runtime");
        rustx::tools::executor::ToolExecutor::start(
            executor.as_ref(),
            rustx::tools::types::ToolInvocation {
                call_id: rustx::runtime::identity::ToolCallId::new("canonical-call"),
                tool_id: definition.id.clone(),
                tool_name: name.to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            rustx::tools::executor::ToolExecutionContext::new(
                bundle.conversation_id(),
                None,
                rustx::runtime::ExecutionCancellation::detached(
                    rustx::runtime::CancellationSignal::new(),
                    rustx::runtime::types::CancellationReason::UserRequested,
                ),
                bundle.workspace(),
                &NoProgress,
                bundle.artifacts(),
                bundle.tool_output(),
                bundle.environment(),
            ),
        )
        .completion
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
            "mcp_runtime::unix_tests::a_current_revision_server_negotiates_the_newest_shared_revision",
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

    /// A **discover-capable** server that advertises only one 2025-era
    /// revision is negotiated down to exactly that revision, through rmcp's
    /// real `UNSUPPORTED_PROTOCOL_VERSION` retry walk inside the inline
    /// lifecycle.
    ///
    /// This covers the *negotiation walk*, not legacy interoperability: the
    /// peer still implements `server/discover`. The discover-less legacy
    /// handshake is covered by
    /// `a_discover_less_server_falls_back_to_the_legacy_initialize_handshake`.
    ///
    /// Invalidation must follow the negotiated revision even here:
    /// `subscriptions/listen` does not exist before 2026-07-28, so rustX must
    /// take `tools/list_changed` from the plain server notification.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn discover_negotiation_walks_down_to_an_older_shared_revision() {
        if fixture::serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = connect(
            "mcp_runtime::unix_tests::discover_negotiation_walks_down_to_an_older_shared_revision",
            BTreeMap::from([(PROTOCOL_VERSIONS_ENV.to_owned(), "2025-06-18".to_owned())]),
            &workspace_dir,
        )
        .await
        .expect("a server advertising only 2025-06-18 must connect");
        assert_eq!(
            runtime.protocol_version(),
            &ProtocolVersion::V_2025_06_18,
            "rustX negotiates down to the one revision the server advertises"
        );
        let server_id = McpServerId::new("fixture");
        let tools = runtime.list_tools().await.expect("tools/list");
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "mutate", "slow"],
            "the negotiated-down connection publishes the catalog exactly once"
        );

        let initial_epoch = runtime.change_epoch();
        let result = call_canonical_tool(
            &runtime,
            &server_id,
            tools,
            "mutate",
            &workspace_dir,
            "issue46-walkdown",
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

    /// **The legacy interoperability regression.** A genuine pre-2026 peer —
    /// one that has never heard of `server/discover` — drives rustX through
    /// the whole legacy path:
    ///
    /// ```text
    /// server/discover            -> METHOD_NOT_FOUND (connection stays open)
    /// ClientLifecycleMode::Auto  -> falls back
    /// initialize                 -> legacy_handshake_version() offered
    ///                            <- InitializeResult(2025-06-18)
    /// notifications/initialized  -> sent
    /// tools/list                 -> canonical catalog published
    /// tools/call mutate          <- plain notifications/tools/list_changed
    /// ```
    ///
    /// Every rustX-owned behavior on that path is asserted here:
    /// `legacy_handshake_version()` as the offered revision, the
    /// post-handshake protocol-membership validation, the legacy
    /// `tools/list_changed` sink instead of `subscriptions/listen`, and the
    /// stdio unit's physical settlement.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one complete legacy-handshake regression
    async fn a_discover_less_server_falls_back_to_the_legacy_initialize_handshake() {
        use rustx::tools::mcp::fixture::legacy;

        if legacy::serve_if_legacy_fixture_mode().await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let journal_dir = tempfile::tempdir().expect("journal");
        let journal = journal_dir.path().join("legacy-journal");
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        let server_id = McpServerId::new("legacy-fixture");
        let binding = McpServerBinding {
            transport: McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture::fixture_spawn_args(
                    "mcp_runtime::unix_tests::a_discover_less_server_falls_back_to_the_legacy_initialize_handshake",
                ),
                cwd: None,
                environment: BTreeMap::from([
                    (legacy::LEGACY_FIXTURE_MODE_ENV.to_owned(), "1".to_owned()),
                    (
                        legacy::LEGACY_JOURNAL_ENV.to_owned(),
                        journal.display().to_string(),
                    ),
                ]),
            },
            policy: ToolInvocationPolicy::default(),
        };

        let runtime = McpServerRuntime::connect(
            &server_id,
            &binding,
            &workspace,
            Arc::new(McpInvalidationState::new()),
        )
        .await
        .expect("a discover-less legacy server must still connect");
        assert_eq!(
            runtime.protocol_version(),
            &legacy::LEGACY_FIXTURE_REVISION,
            "the negotiated revision is the one the legacy InitializeResult named"
        );

        // rustX must have offered the newest revision it speaks that predates
        // the inline lifecycle — never an inline-only revision, and never a
        // revision outside its own offered set.
        let expected_legacy_offer = rustx::tools::mcp::supported_protocol_versions()
            .into_iter()
            .find(|version| version.as_str() < ProtocolVersion::V_2026_07_28.as_str())
            .expect("rustX offers at least one pre-inline revision");
        let after_connect = legacy::read_journal(&journal);
        assert_eq!(
            count(&after_connect, legacy::JOURNAL_DISCOVER),
            1,
            "exactly one server/discover probe: {after_connect:?}"
        );
        assert_eq!(
            after_connect
                .iter()
                .filter(|entry| entry.starts_with(legacy::JOURNAL_INITIALIZE_PREFIX))
                .collect::<Vec<_>>(),
            [&format!(
                "{}{expected_legacy_offer}",
                legacy::JOURNAL_INITIALIZE_PREFIX
            )],
            "exactly one legacy initialize, offering legacy_handshake_version(): \
             {after_connect:?}"
        );

        let tools = runtime.list_tools().await.expect("tools/list");
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "mutate"],
            "the legacy connection publishes the canonical catalog exactly once"
        );
        // The fixture handles messages strictly in order, so answering
        // `tools/list` proves the preceding notification was processed.
        let after_list = legacy::read_journal(&journal);
        assert_eq!(
            count(&after_list, legacy::JOURNAL_INITIALIZED),
            1,
            "exactly one notifications/initialized: {after_list:?}"
        );

        let initial_epoch = runtime.change_epoch();
        let result = call_canonical_tool(
            &runtime,
            &server_id,
            tools,
            "mutate",
            &workspace_dir,
            "issue46-legacy",
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
        .expect("the plain pre-2026 tools/list_changed notification must invalidate");
        assert_eq!(
            runtime.change_epoch(),
            initial_epoch + 1,
            "one plain notification advances the shared epoch exactly once"
        );
        assert_eq!(
            runtime
                .list_tools()
                .await
                .expect("refreshed tools/list")
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "new_tool"],
            "the refreshed catalog is the mutated one"
        );

        let final_journal = legacy::read_journal(&journal);
        assert_eq!(
            count(&final_journal, legacy::JOURNAL_SUBSCRIBE),
            0,
            "a pre-2026 peer must never be asked for subscriptions/listen: {final_journal:?}"
        );
        assert_eq!(
            count(&final_journal, legacy::JOURNAL_DISCOVER),
            1,
            "the discover probe is never retried after the fallback: {final_journal:?}"
        );
        assert_eq!(
            count(&final_journal, legacy::JOURNAL_MUTATE),
            1,
            "the canonical executor called the remote tool exactly once: {final_journal:?}"
        );

        runtime
            .close()
            .await
            .expect("the owned stdio unit must publish physical settlement");
    }

    fn count(journal: &[String], entry: &str) -> usize {
        journal.iter().filter(|line| *line == entry).count()
    }

    /// **The Issue #81 production peer shape.** A genuine pre-2026 peer
    /// whose session middleware rejects the unknown pre-`initialize`
    /// `server/discover` probe with a correlated `-32600`
    /// (`INVALID_REQUEST`) "Unsupported protocol version" error — not the
    /// `-32601` rmcp 3.1.2's `Auto` mode required for the legacy fallback.
    ///
    /// ```text
    /// server/discover            -> -32600 Unsupported protocol version
    ///                               (connection stays open)
    /// ClientLifecycleMode::Auto  -> classifies the peer as legacy
    /// initialize                 -> legacy_handshake_version() offered
    ///                            <- InitializeResult(2025-06-18)
    /// tools/list                 -> canonical catalog published
    /// ```
    ///
    /// The client must not abort after the newest revision is rejected:
    /// the negotiated revision is the highest revision both sides speak
    /// (2025-06-18), and discovery succeeds over it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_legacy_server_rejecting_the_probe_with_a_non_modern_error_falls_back() {
        use rustx::tools::mcp::fixture::legacy;

        if legacy::serve_if_legacy_fixture_mode().await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let journal_dir = tempfile::tempdir().expect("journal");
        let journal = journal_dir.path().join("legacy-journal");
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        let server_id = McpServerId::new("legacy-invalid-request");
        let binding = McpServerBinding {
            transport: McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture::fixture_spawn_args(
                    "mcp_runtime::unix_tests::a_legacy_server_rejecting_the_probe_with_a_non_modern_error_falls_back",
                ),
                cwd: None,
                environment: BTreeMap::from([
                    (legacy::LEGACY_FIXTURE_MODE_ENV.to_owned(), "1".to_owned()),
                    (
                        legacy::LEGACY_JOURNAL_ENV.to_owned(),
                        journal.display().to_string(),
                    ),
                    (
                        legacy::LEGACY_DISCOVER_ERROR_ENV.to_owned(),
                        legacy::DISCOVER_ERROR_INVALID_REQUEST.to_owned(),
                    ),
                ]),
            },
            policy: ToolInvocationPolicy::default(),
        };

        let runtime = McpServerRuntime::connect(
            &server_id,
            &binding,
            &workspace,
            Arc::new(McpInvalidationState::new()),
        )
        .await
        .expect("a non-modern probe rejection must fall back to the legacy handshake");
        assert_eq!(
            runtime.protocol_version(),
            &legacy::LEGACY_FIXTURE_REVISION,
            "the highest revision both sides speak wins after the fallback"
        );
        let expected_legacy_offer = rustx::tools::mcp::supported_protocol_versions()
            .into_iter()
            .find(|version| version.as_str() < ProtocolVersion::V_2026_07_28.as_str())
            .expect("rustX offers at least one pre-inline revision");
        let journal = legacy::read_journal(&journal);
        assert_eq!(
            count(&journal, legacy::JOURNAL_DISCOVER),
            1,
            "exactly one server/discover probe: {journal:?}"
        );
        assert_eq!(
            journal
                .iter()
                .filter(|entry| entry.starts_with(legacy::JOURNAL_INITIALIZE_PREFIX))
                .collect::<Vec<_>>(),
            [&format!(
                "{}{expected_legacy_offer}",
                legacy::JOURNAL_INITIALIZE_PREFIX
            )],
            "exactly one legacy initialize on the same connection, offering \
             legacy_handshake_version(): {journal:?}"
        );
        assert_eq!(
            runtime
                .list_tools()
                .await
                .expect("tools/list over the fallback connection")
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo", "mutate"],
            "discovery succeeds over the negotiated legacy revision"
        );
        runtime
            .close()
            .await
            .expect("the owned stdio unit must publish physical settlement");
    }

    /// A legacy peer that echoes a revision no MCP SDK knows shares no
    /// revision with rustX: the post-handshake membership validation
    /// rejects it with a bounded [`McpError::ProtocolCompatibility`], and
    /// the spawned stdio unit is still physically settled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_legacy_server_with_no_shared_revision_is_a_compatibility_error() {
        use rustx::tools::mcp::fixture::legacy;

        if legacy::serve_if_legacy_fixture_mode().await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        let binding = McpServerBinding {
            transport: McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture::fixture_spawn_args(
                    "mcp_runtime::unix_tests::a_legacy_server_with_no_shared_revision_is_a_compatibility_error",
                ),
                cwd: None,
                environment: BTreeMap::from([
                    (legacy::LEGACY_FIXTURE_MODE_ENV.to_owned(), "1".to_owned()),
                    (
                        legacy::LEGACY_REVISION_ENV.to_owned(),
                        "1999-01-01".to_owned(),
                    ),
                ]),
            },
            policy: ToolInvocationPolicy::default(),
        };
        let error = McpServerRuntime::connect(
            &McpServerId::new("legacy-no-overlap"),
            &binding,
            &workspace,
            Arc::new(McpInvalidationState::new()),
        )
        .await
        .expect_err("a legacy peer with no shared revision must not connect");
        let McpError::ProtocolCompatibility(detail) = &error else {
            panic!("expected a protocol compatibility failure, got: {error:?}");
        };
        assert!(
            detail.contains("1999-01-01"),
            "the failure must name the revision the server echoed: {detail}"
        );
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
            "mcp_runtime::unix_tests::a_server_with_no_shared_revision_fails_with_a_compatibility_error",
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
            "mcp_runtime::unix_tests::a_named_map_entry_composes_into_exactly_one_runtime_server",
        );
        let session = serde_json::json!({
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
        let models_path = root.path().join("models.jsonc");
        let config_path = root.path().join("rustx.jsonc");
        std::fs::write(&models_path, MODELS_JSON).expect("models.jsonc");
        std::fs::write(
            &config_path,
            serde_json::to_vec_pretty(&session).expect("session json"),
        )
        .expect("rustx.jsonc");

        let runtime = rustx::local_runtime::composition::LocalConversationRuntime::compose(
            &rustx::local_runtime::composition::LocalRuntimePaths {
                models: models_path,
                config: config_path,
                skill_paths: Vec::new(),
                no_skills: false,
                no_builtin_tools: false,
                no_tools: false,
                startup_session: rustx::local_runtime::StartupSession::Empty,
                session_name: None,
                tools: None,
                exclude_tools: Vec::new(),
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
        let listen_ready = served.listen_ready.clone();
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
        // Connection completion does not itself linearize delivery of the
        // server-side subscription handler. Await its explicit fixture
        // acknowledgement before inspecting the exact-once counter.
        listen_ready.notified().await;
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

    /// A stdio binding that re-runs this test binary as the raw-wire
    /// corruption fixture (Issue #174 review: a confirmed structurally
    /// invalid MCP peer message is a rustX protocol failure, never
    /// peer-only traffic). The child re-executes exactly the fixture test
    /// (the same `--exact` convention as the other stdio fixtures).
    fn raw_fixture_binding(
        test_name: &str,
        corruption: &str,
        invalid_phase: &str,
        journal: &std::path::Path,
    ) -> McpServerBinding {
        McpServerBinding {
            transport: McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: rustx::tools::mcp::fixture::fixture_spawn_args(test_name),
                cwd: None,
                environment: BTreeMap::from([
                    (
                        rustx::tools::mcp::fixture::raw::RAW_FIXTURE_MODE_ENV.to_owned(),
                        "1".to_owned(),
                    ),
                    (
                        rustx::tools::mcp::fixture::raw::RAW_CORRUPTION_ENV.to_owned(),
                        corruption.to_owned(),
                    ),
                    (
                        rustx::tools::mcp::fixture::raw::RAW_INVALID_PHASE_ENV.to_owned(),
                        invalid_phase.to_owned(),
                    ),
                    (
                        rustx::tools::mcp::fixture::raw::RAW_JOURNAL_ENV.to_owned(),
                        journal.display().to_string(),
                    ),
                ]),
            },
            policy: ToolInvocationPolicy::default(),
        }
    }

    /// Connects the raw fixture directly at the runtime boundary.
    async fn connect_raw_fixture(
        test_name: &str,
        corruption: &str,
        invalid_phase: &str,
        workspace_dir: &tempfile::TempDir,
    ) -> Result<Arc<McpServerRuntime>, McpError> {
        let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
        McpServerRuntime::connect(
            &McpServerId::new("raw-fixture"),
            &raw_fixture_binding(
                test_name,
                corruption,
                invalid_phase,
                &workspace_dir.path().join("raw-journal"),
            ),
            &workspace,
            Arc::new(McpInvalidationState::new()),
        )
        .await
    }

    /// Discovers the catalog and returns the canonical `echo` definition
    /// and executor pair.
    async fn raw_echo(
        runtime: &Arc<McpServerRuntime>,
    ) -> (
        rustx::tools::types::ToolDefinition,
        Arc<dyn rustx::tools::executor::ToolExecutor>,
    ) {
        let tools = runtime.list_tools().await.expect("tools/list");
        assert_eq!(tools.len(), 1, "the catalog is intact: {tools:?}");
        assert_eq!(tools[0].name, "echo");
        let definitions = rustx::tools::mcp::definitions(
            &McpServerId::new("raw-fixture"),
            ToolInvocationPolicy::default(),
            runtime,
            tools,
        );
        definitions
            .into_iter()
            .find(|(definition, _)| definition.name == "echo")
            .expect("echo definition")
    }

    /// Executes one tool call through the canonical executor boundary, the
    /// same path the Agent Loop uses.
    async fn execute_raw(
        definition: &rustx::tools::types::ToolDefinition,
        executor: &dyn rustx::tools::executor::ToolExecutor,
        workspace_dir: &tempfile::TempDir,
        call_id: &str,
    ) -> rustx::tools::types::ToolExecutionResult {
        let artifacts_dir = tempfile::tempdir().expect("artifacts");
        let bundle = rustx::tools::runtime::ConversationToolRuntime::new(
            rustx::runtime::identity::ConversationId::new("raw-fixture"),
            workspace_dir.path(),
            artifacts_dir.path(),
        )
        .expect("tool runtime");
        rustx::tools::executor::ToolExecutor::start(
            executor,
            rustx::tools::types::ToolInvocation {
                call_id: rustx::runtime::identity::ToolCallId::new(call_id),
                tool_id: definition.id.clone(),
                tool_name: "echo".to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            rustx::tools::executor::ToolExecutionContext::new(
                bundle.conversation_id(),
                None,
                rustx::runtime::ExecutionCancellation::detached(
                    rustx::runtime::CancellationSignal::new(),
                    rustx::runtime::types::CancellationReason::UserRequested,
                ),
                bundle.workspace(),
                &NoProgress,
                bundle.artifacts(),
                bundle.tool_output(),
                bundle.environment(),
            ),
        )
        .completion
        .await
    }

    /// Connects the raw fixture and drives one `echo` call through the
    /// canonical executor boundary, returning the fixture's inbound journal.
    /// Used by the noise regression, whose exchange must succeed.
    async fn drive_raw_fixture(
        test_name: &str,
        corruption: &str,
        workspace_dir: &tempfile::TempDir,
    ) -> Vec<String> {
        let runtime = connect_raw_fixture(
            test_name,
            corruption,
            rustx::tools::mcp::fixture::raw::INVALID_PHASE_INITIALIZE,
            workspace_dir,
        )
        .await
        .expect("the raw fixture negotiates despite its noise line");
        let (definition, executor) = raw_echo(&runtime).await;
        let result = execute_raw(&definition, executor.as_ref(), workspace_dir, "raw-echo").await;
        assert!(
            matches!(
                result.status,
                rustx::tools::types::ToolExecutionStatus::Success
            ),
            "the call completes: {:?}",
            result.status
        );
        runtime.close().await.expect("physical settlement");
        rustx::tools::mcp::fixture::raw::read_journal(&workspace_dir.path().join("raw-journal"))
    }

    /// Plain non-protocol noise is deliberately ignored by the generic MCP
    /// framing: a non-JSON line at import-time and mid-call never corrupts
    /// the wire, and — being `Syntax`-class input — is *not* answered with
    /// a protocol error reply. This is an implementation characteristic of
    /// the transport, not a user-facing logging contract.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn plain_non_protocol_noise_is_deliberately_ignored_by_the_generic_framing() {
        if rustx::tools::mcp::fixture::raw::serve_if_raw_fixture_mode().await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let journal = drive_raw_fixture(
            "mcp_runtime::unix_tests::plain_non_protocol_noise_is_deliberately_ignored_by_the_generic_framing",
            rustx::tools::mcp::fixture::raw::CORRUPTION_NOISE,
            &workspace_dir,
        )
        .await;
        assert!(
            !journal.iter().any(
                |entry| entry == rustx::tools::mcp::fixture::raw::JOURNAL_CLIENT_PROTOCOL_ERROR
            ),
            "noise is ignored without any protocol-level reply: {journal:?}"
        );
    }

    /// A genuinely malformed MCP protocol message — well-formed JSON that
    /// is not a valid MCP message — emitted while the handshake is pending
    /// is a rustX structural failure, not peer-only traffic: the generic
    /// runtime observes the violation, the connect fails with a bounded
    /// protocol diagnostic naming the server, no runtime is published, and
    /// the physical process settles through the ordinary connect-failure
    /// ownership. rmcp's bounded peer-facing `Invalid Request` reply still
    /// goes out (the fixture journals it), but that reply is not the
    /// diagnostic rustX acts on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn protocol_invalid_output_during_initialize_fails_the_connection_structurally() {
        if rustx::tools::mcp::fixture::raw::serve_if_raw_fixture_mode().await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let error = connect_raw_fixture(
            "mcp_runtime::unix_tests::protocol_invalid_output_during_initialize_fails_the_connection_structurally",
            rustx::tools::mcp::fixture::raw::CORRUPTION_INVALID,
            rustx::tools::mcp::fixture::raw::INVALID_PHASE_INITIALIZE,
            &workspace_dir,
        )
        .await
        .expect_err("a structurally invalid handshake-time peer message must fail the connect");
        let McpError::ProtocolViolation(diagnostic) = &error else {
            panic!("the failure is a protocol violation, got: {error:?}");
        };
        assert!(
            diagnostic.contains("raw-fixture"),
            "the diagnostic names the server identity: {diagnostic}"
        );
        assert!(
            diagnostic.contains("could not be decoded as an MCP message"),
            "the diagnostic is bounded and actionable: {diagnostic}"
        );
        // The connect error returned only after the physical settlement was
        // proven, so the fixture process has exited and its journal is
        // complete: rmcp's peer-facing reply to the corrupt line is
        // preserved (peer behavior), while rustX's verdict is the failure
        // above (rustX behavior).
        let journal = rustx::tools::mcp::fixture::raw::read_journal(
            &workspace_dir.path().join("raw-journal"),
        );
        assert!(
            journal.iter().any(
                |entry| entry == rustx::tools::mcp::fixture::raw::JOURNAL_CLIENT_PROTOCOL_ERROR
            ),
            "rmcp still answers the peer with a bounded Invalid Request: {journal:?}"
        );
    }

    /// The same protocol-invalid peer message driven through the capability
    /// coordinator is attributed to the failing server's own capability
    /// source: the candidate stays preparable (the failure is isolated to
    /// its source), the source is `Unavailable` with the bounded protocol
    /// diagnostic, and no catalog is frozen from the violated connection.
    /// Managed Python packages inherit exactly this attribution through
    /// their synthesized `python:<folder>` server identity — there is no
    /// Python-specific corruption path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn protocol_corruption_is_attributed_to_the_mcp_capability_source() {
        if rustx::tools::mcp::fixture::raw::serve_if_raw_fixture_mode().await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let store_dir = tempfile::tempdir().expect("environment store");
        let server_id = McpServerId::new("raw-fixture");
        let coordinator = rustx::capabilities::CapabilityCoordinator::new(
            rustx::capabilities::CapabilityCoordinatorConfig {
                conversation_id: rustx::runtime::identity::ConversationId::new("conv-raw-attribution"),
                workspace: rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace"),
                base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
                tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
                skill_discovery: rustx::skills::SkillDiscoveryConfig {
                    automatic_roots: vec![workspace_dir.path().join(".agents/skills")],
                    explicit_paths: Vec::new(),
                },
                mcp_servers: BTreeMap::from([(
                    server_id.clone(),
                    raw_fixture_binding(
                        "mcp_runtime::unix_tests::protocol_corruption_is_attributed_to_the_mcp_capability_source",
                        rustx::tools::mcp::fixture::raw::CORRUPTION_INVALID,
                        rustx::tools::mcp::fixture::raw::INVALID_PHASE_INITIALIZE,
                        &workspace_dir.path().join("raw-journal"),
                    ),
                )]),
                base_environment: rustx::tools::environment::ToolEnvironment::new(),
                environment_store_root: store_dir.path().join("env-store"),
            },
        )
        .expect("coordinator");
        let candidate = tokio::time::timeout(
            std::time::Duration::from_mins(2),
            coordinator.prepare_candidate(),
        )
        .await
        .expect("protocol corruption must not hang capability preparation")
        .expect("an isolated source failure must not fail the whole candidate");
        let Some(rustx::capabilities::CapabilitySourceState::Unavailable { reason }) = candidate
            .availability()
            .get(&rustx::capabilities::CapabilitySourceId::Mcp(server_id))
        else {
            panic!(
                "the corrupted server is unavailable on its own source: {:?}",
                candidate.availability()
            );
        };
        assert!(
            reason.contains("MCP protocol violation") && reason.contains("raw-fixture"),
            "the availability diagnostic is the bounded protocol failure: {reason}"
        );
        // No catalog is frozen from a connection that violated the
        // protocol: the committed snapshot publishes no `echo` tool.
        let snapshot = coordinator.commit(candidate).expect("commit");
        assert!(
            !snapshot
                .tool_registry()
                .definitions()
                .iter()
                .any(|definition| definition.name == "echo"),
            "a violated connection freezes no catalog"
        );
    }

    /// A structurally invalid MCP message emitted mid-`tools/call` leaves the
    /// in-flight call's external outcome unknown — never a success and never
    /// a confirmed failure, because the dispatched call may have partially or
    /// fully completed — and poisons the connection generation: subsequent
    /// calls are rejected with the same protocol fact instead of being served
    /// by a transport that already violated the protocol. Physical settlement
    /// still goes through the ordinary runtime close.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn protocol_invalid_output_during_a_call_is_outcome_unknown_and_poisons_the_generation() {
        if rustx::tools::mcp::fixture::raw::serve_if_raw_fixture_mode().await {
            return;
        }
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = connect_raw_fixture(
            "mcp_runtime::unix_tests::protocol_invalid_output_during_a_call_is_outcome_unknown_and_poisons_the_generation",
            rustx::tools::mcp::fixture::raw::CORRUPTION_INVALID,
            rustx::tools::mcp::fixture::raw::INVALID_PHASE_CALL,
            &workspace_dir,
        )
        .await
        .expect("the handshake and discovery are clean in the call-phase fixture");
        let (definition, executor) = raw_echo(&runtime).await;
        let result = execute_raw(&definition, executor.as_ref(), &workspace_dir, "raw-echo").await;
        let rustx::tools::types::ToolExecutionStatus::OutcomeUnknown { detail } = &result.status
        else {
            panic!(
                "a call crossed by a protocol violation after dispatch has an unknown outcome: {:?}",
                result.status
            );
        };
        assert!(
            detail.contains("MCP protocol violation") && detail.contains("raw-fixture"),
            "the unknown outcome carries the bounded protocol diagnostic: {detail}"
        );
        // The generation is poisoned: a later call is rejected with the
        // same protocol fact instead of being treated as healthy.
        let second =
            execute_raw(&definition, executor.as_ref(), &workspace_dir, "raw-echo-2").await;
        let rustx::tools::types::ToolExecutionStatus::Failed { error } = &second.status else {
            panic!(
                "a poisoned generation must reject later calls: {:?}",
                second.status
            );
        };
        assert!(
            error.contains("MCP protocol violation"),
            "the rejection carries the protocol fact: {error}"
        );
        // Physical settlement of the poisoned generation still goes through
        // the ordinary close ownership. The fixture process has exited once
        // close returns, so its journal is complete.
        runtime.close().await.expect("physical settlement");
        let journal = rustx::tools::mcp::fixture::raw::read_journal(
            &workspace_dir.path().join("raw-journal"),
        );
        assert!(
            journal
                .iter()
                .any(|entry| entry == rustx::tools::mcp::fixture::raw::JOURNAL_ECHO),
            "the call reached the peer: {journal:?}"
        );
        assert!(
            journal.iter().any(
                |entry| entry == rustx::tools::mcp::fixture::raw::JOURNAL_CLIENT_PROTOCOL_ERROR
            ),
            "rmcp still answers the peer with a bounded Invalid Request: {journal:?}"
        );
    }
}
