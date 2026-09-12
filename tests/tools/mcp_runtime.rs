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

use crate::launch_fixture::LaunchFixture;
#[cfg(all(unix, feature = "mcp-fixture"))]
mod unix_tests {
    use super::LaunchFixture;
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
    const MODELS_TOML: &str = r#"[providers.local]
base_url = "https://local.fixture.invalid/v1"
api_key = "$RUSTX_ISSUE46_KEY"

[[providers.local.models]]
id = "composed-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096

[providers.local.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[providers.local.models.compat]
chat_reasoning_replay = "omit"
"#;

    /// A stdio binding that re-runs this test binary as its own MCP server,
    /// with the given fixture protocol behavior.
    fn fixture_binding(test_name: &str, environment: BTreeMap<String, String>) -> McpServerBinding {
        let mut environment = environment;
        environment.insert(fixture::FIXTURE_MODE_ENV.to_owned(), "1".to_owned());
        McpServerBinding {
            credentials: rustx::credentials::SourceCredentials::default(),
            activation: rustx::capabilities::activation::SourceActivation::Enabled,
            resource_workspace: None,
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
        call_canonical_tool_with(
            runtime,
            server_id,
            tools,
            name,
            serde_json::json!({}),
            workspace_dir,
            conversation,
        )
        .await
    }

    /// The same canonical executor boundary with explicit tool arguments.
    async fn call_canonical_tool_with(
        runtime: &Arc<McpServerRuntime>,
        server_id: &McpServerId,
        tools: Vec<rustx::tools::mcp::CanonicalMcpTool>,
        name: &str,
        arguments: serde_json::Value,
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
                id: rustx::tools::types::ToolInvocationId::Agent {
                    call_id: rustx::runtime::identity::ToolCallId::new("canonical-call"),
                },
                tool_id: definition.id.clone(),
                tool_name: name.to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments,
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
            credentials: rustx::credentials::SourceCredentials::default(),
            activation: rustx::capabilities::activation::SourceActivation::Enabled,
            resource_workspace: None,
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
            credentials: rustx::credentials::SourceCredentials::default(),
            activation: rustx::capabilities::activation::SourceActivation::Enabled,
            resource_workspace: None,
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
            credentials: rustx::credentials::SourceCredentials::default(),
            activation: rustx::capabilities::activation::SourceActivation::Enabled,
            resource_workspace: None,
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
    /// plus explicit source selection and a keyed policy become exactly one runtime
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
            "agent_id": "agent-46",
            "tools": {"sources": {"exa-local": "all"}},
            "model": {"model": "local/composed-model"},
            "context": {"reserve_tokens": 1024, "keep_recent_tokens": 8192},
            "mcp_servers": {
                "exa-local": {
                    "enabled": true,
                    "type": "stdio",
                    "command": program,
                    "args": args,
                    "env": {fixture::FIXTURE_MODE_ENV: "1"},
                },
            },
            "mcp_tool_policies": {
                "exa-local": {"execution": "background_only", "concurrency": "parallel"},
            },
        });
        let models_path = root.path().join("models.toml");
        let config_path = root.path().join("rustx.toml");
        std::fs::write(&models_path, MODELS_TOML).expect("models.toml");
        crate::launch_fixture::write_documents(
            &config_path,
            &toml::to_string_pretty(&session).unwrap(),
            &["mcp_servers", "mcp_tool_policies"],
        );

        let runtime = rustx::local_runtime::composition::LocalConversationRuntime::compose(
            &(LaunchFixture {
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
            })
            .resolve(),
            &rustx::local_runtime::composition::LocalRuntimeDependencies {
                credentials: Some(Arc::new(
                    rustx::model::catalog::MapCredentialEnvironment::new([(
                        "RUSTX_ISSUE46_KEY".to_owned(),
                        "issue46-secret".to_owned(),
                    )]),
                )),
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
                credentials: rustx::credentials::SourceCredentials::default(),
                activation: rustx::capabilities::activation::SourceActivation::Enabled,
                resource_workspace: None,
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
            credentials: rustx::credentials::SourceCredentials::default(),
            activation: rustx::capabilities::activation::SourceActivation::Enabled,
            resource_workspace: None,
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
                id: rustx::tools::types::ToolInvocationId::Agent {
                    call_id: rustx::runtime::identity::ToolCallId::new(call_id),
                },
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
        // complete: the corrupt line reached rustX as a violation, and the
        // handshake never completed.
        //
        // Whether the peer also observed rmcp's bounded `Invalid Request`
        // reply is deliberately not asserted — see the call-phase
        // regression below for why that is rmcp's best-effort courtesy to
        // the violator rather than a rustX contract.
        let journal = rustx::tools::mcp::fixture::raw::read_journal(
            &workspace_dir.path().join("raw-journal"),
        );
        assert!(
            journal
                .iter()
                .any(|entry| entry
                    .starts_with(rustx::tools::mcp::fixture::raw::JOURNAL_INBOUND_PREFIX)),
            "the handshake reached the peer: {journal:?}"
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
                source_demand: rustx::capabilities::source::ToolSourceDemand::new([rustx::capabilities::ToolSourceId::Mcp(server_id.clone())], rustx::runtime::resources::ManagedPythonCatalog::default()),
                conversation_id: rustx::runtime::identity::ConversationId::new("conv-raw-attribution"),
                workspace: rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace"),
                base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
                extension_tools: rustx::extensions::ExtensionToolPlane::none(),
                tool_activation: rustx::capabilities::ToolActivationPolicy {profile: crate::local_runtime::config::AgentProfileDocument { tools: crate::capabilities::selection::ToolSelectionDocument { builtin: crate::local_runtime::config::builtin_root_profile().tools.builtin, sources: [(rustx::capabilities::ToolSourceId::Mcp(server_id.clone()), rustx::capabilities::selection::SourceToolSelection::All)].into() }, ..crate::local_runtime::config::builtin_root_profile() }, ..Default::default()},
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
            .get(&rustx::capabilities::ToolSourceId::Mcp(server_id))
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
        // What is deliberately *not* asserted: that the peer observed rmcp's
        // bounded `Invalid Request` reply to the corrupt line. rmcp writes
        // that reply inline while decoding the offending line, but the
        // observation seam records the violation from the same line one
        // layer lower — inside the read rmcp is decoding — so rustX's
        // reaction is already running: the settled call poisons the
        // generation and asks the owned stdio unit to retire. Whether the
        // peer process is still reading its stdin when that reply lands is
        // a race rustX creates on purpose by fencing the protocol boundary
        // promptly, and no wait can manufacture a line a killed peer never
        // read. It is rmcp's best-effort courtesy to the violator, not a
        // rustX contract, and this test asserts only facts rustX owns.
        //
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
    }
    // -----------------------------------------------------------------
    // MCP-01 (Issue #240): modern client semantics and SDK cache policy
    // -----------------------------------------------------------------

    /// One HTTP exchange as the wire actually carried it.
    #[derive(Clone)]
    struct HttpExchange {
        request: http::HeaderMap,
        response: http::HeaderMap,
    }

    impl HttpExchange {
        fn request_header(&self, name: &str) -> Option<String> {
            self.request
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        }
    }

    /// One in-process Streamable HTTP host serving the official-rmcp fixture,
    /// recording every inbound request header and every outbound response
    /// header.
    ///
    /// The recorder is a passive observer around rmcp's own server service:
    /// it never adds, removes, or rewrites a header, so what it reports is
    /// exactly what the SDK generated on one side and what rustX's request
    /// ownership wrapper forwarded on the other.
    struct HttpFixtureHost {
        endpoint: String,
        exchanges: Arc<std::sync::Mutex<Vec<HttpExchange>>>,
        cancellation: tokio_util::sync::CancellationToken,
        task: tokio::task::JoinHandle<()>,
    }

    async fn record_exchange(
        axum::extract::State(exchanges): axum::extract::State<
            Arc<std::sync::Mutex<Vec<HttpExchange>>>,
        >,
        request: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        let recorded = request.headers().clone();
        let response = next.run(request).await;
        exchanges
            .lock()
            .expect("HTTP exchange recorder lock")
            .push(HttpExchange {
                request: recorded,
                response: response.headers().clone(),
            });
        response
    }

    impl HttpFixtureHost {
        async fn start(fixture: FixtureServer) -> Self {
            use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
            use rmcp::transport::streamable_http_server::{
                StreamableHttpServerConfig, StreamableHttpService,
            };

            let cancellation = tokio_util::sync::CancellationToken::new();
            // Defaults, deliberately: `legacy_session_mode` stays on, so this
            // server *is* session-capable. A modern connection that carries no
            // session id therefore proves a negotiated protocol property, not
            // a server that never had sessions to give.
            let mut config = StreamableHttpServerConfig::default();
            config.cancellation_token = cancellation.child_token();
            config.sse_keep_alive = None;
            let service = StreamableHttpService::<FixtureServer, LocalSessionManager>::new(
                move || Ok(fixture.clone()),
                Arc::default(),
                config,
            );
            let exchanges = Arc::new(std::sync::Mutex::new(Vec::new()));
            let router = axum::Router::new().nest_service("/mcp", service).layer(
                axum::middleware::from_fn_with_state(Arc::clone(&exchanges), record_exchange),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("HTTP listener");
            let address = listener.local_addr().expect("HTTP address");
            let task = tokio::spawn(async move {
                let _ = axum::serve(listener, router).await;
            });
            Self {
                endpoint: format!("http://{address}/mcp"),
                exchanges,
                cancellation,
                task,
            }
        }

        fn binding(&self) -> McpServerBinding {
            McpServerBinding {
                credentials: rustx::credentials::SourceCredentials::default(),
                activation: rustx::capabilities::activation::SourceActivation::Enabled,
                resource_workspace: None,
                transport: McpTransportConfig::StreamableHttp {
                    endpoint: self.endpoint.clone(),
                    headers: BTreeMap::new(),
                },
                policy: ToolInvocationPolicy::default(),
            }
        }

        async fn connect(&self, workspace_dir: &tempfile::TempDir) -> Arc<McpServerRuntime> {
            let workspace = rustx::tools::Workspace::new(workspace_dir.path()).expect("workspace");
            McpServerRuntime::connect(
                &McpServerId::new("http-fixture"),
                &self.binding(),
                &workspace,
                Arc::new(McpInvalidationState::new()),
            )
            .await
            .expect("the HTTP fixture must connect")
        }

        fn exchanges(&self) -> Vec<HttpExchange> {
            self.exchanges
                .lock()
                .expect("HTTP exchange recorder lock")
                .clone()
        }

        async fn shutdown(self) {
            self.cancellation.cancel();
            self.task.abort();
            let _ = self.task.await;
        }
    }

    /// **The MCP 2026-07-28 negotiation contract.** A peer that advertises
    /// only `2026-07-28` is reached through `server/discover` and negotiates
    /// exactly that revision.
    ///
    /// The proof is wire behavior, not a helper predicate:
    ///
    /// - the fixture counts the `server/discover` probes it answered, and
    ///   exactly one arrived;
    /// - the fixture serves *only* `2026-07-28`, and rmcp's `Auto` legacy
    ///   fallback offers `legacy_handshake_version()` — a pre-inline
    ///   revision this server does not speak. A legacy handshake could
    ///   therefore not have produced a live connection at all, so the
    ///   established connection is necessarily the inline one.
    ///
    /// rustX stays unpinned: it keeps offering the SDK's complete revision
    /// set, which is what lets the same code negotiate down elsewhere in
    /// this suite.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_modern_peer_negotiates_2026_07_28_through_server_discover() {
        let fixture = FixtureServer {
            list_changed_supported: true,
            supported_versions: Some(vec![ProtocolVersion::V_2026_07_28]),
            ..FixtureServer::default()
        };
        let discover_calls = fixture.discover_calls.clone();
        let host = HttpFixtureHost::start(fixture).await;
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = host.connect(&workspace_dir).await;

        assert_eq!(
            runtime.protocol_version(),
            &ProtocolVersion::V_2026_07_28,
            "the modern revision is what the connection actually negotiated"
        );
        assert_eq!(
            discover_calls.load(std::sync::atomic::Ordering::Acquire),
            1,
            "the inline lifecycle probed `server/discover` exactly once"
        );
        assert!(
            rustx::tools::mcp::supported_protocol_versions().len() > 1,
            "rustX is not pinned to one wire revision: it offers the SDK's whole set"
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
            "the modern connection serves the catalog over the negotiated revision"
        );
        runtime.close().await.expect("physical settlement");
        host.shutdown().await;
    }

    /// **The modern invalidation contract.** A negotiated `2026-07-28`
    /// connection installs `subscriptions/listen` exactly once, and one
    /// received `tools/list_changed` advances the shared invalidation epoch
    /// exactly once.
    ///
    /// Ordering is proven by the fixture's own acknowledgements — the
    /// subscription-installed notify and the epoch's change notify — never
    /// by elapsed time. The legacy callback path is not installed here: the
    /// server emits the change *through the subscription sink*, so an epoch
    /// advance is evidence the modern mechanism carried it, and the
    /// unchanged listen counter is evidence no second mechanism exists.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_modern_change_notification_advances_the_epoch_exactly_once() {
        let fixture = FixtureServer {
            list_changed_supported: true,
            supported_versions: Some(vec![ProtocolVersion::V_2026_07_28]),
            ..FixtureServer::default()
        };
        let listen_calls = fixture.listen_calls.clone();
        let listen_ready = fixture.listen_ready.clone();
        let host = HttpFixtureHost::start(fixture).await;
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = host.connect(&workspace_dir).await;
        listen_ready.notified().await;

        assert_eq!(runtime.protocol_version(), &ProtocolVersion::V_2026_07_28);
        assert_eq!(
            listen_calls.load(std::sync::atomic::Ordering::Acquire),
            1,
            "exactly one revision-appropriate invalidation mechanism is installed"
        );

        let server_id = McpServerId::new("http-fixture");
        let tools = runtime.list_tools().await.expect("tools/list");
        let epoch_before = runtime.change_epoch();
        let result = call_canonical_tool(
            &runtime,
            &server_id,
            tools,
            "mutate",
            &workspace_dir,
            "issue240-modern-invalidation",
        )
        .await;
        assert!(matches!(
            result.status,
            rustx::tools::types::ToolExecutionStatus::Success
        ));
        runtime.wait_for_change(epoch_before).await;
        assert_eq!(
            runtime.change_epoch(),
            epoch_before + 1,
            "one modern change notification advances the epoch exactly once"
        );
        assert_eq!(
            listen_calls.load(std::sync::atomic::Ordering::Acquire),
            1,
            "the modern path never additionally installs the legacy callback"
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
            "the refresh that follows invalidation observes the new catalog"
        );
        // The refresh above is a full round trip on the same ordered stream
        // the notification travelled, so any duplicate delivery of that one
        // change would already have been processed by now. The epoch is
        // still exactly one advance ahead: one event, one advance.
        assert_eq!(
            runtime.change_epoch(),
            epoch_before + 1,
            "no second invalidation mechanism replayed the same change"
        );
        runtime.close().await.expect("physical settlement");
        host.shutdown().await;
    }

    /// **The positive-`ttlMs` regression.** A server that declares its
    /// catalog fresh for ten minutes cannot make the next rustX refresh skip
    /// the peer.
    ///
    /// The observation is behavioral: the fixture counts the `tools/list`
    /// requests that actually reached it. With rmcp's response cache live,
    /// the second refresh would be answered from the SDK's own store and the
    /// counter would stay at one, which is exactly the second semantic
    /// capability cache rustX must not have.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_positive_ttl_never_answers_the_next_catalog_refresh_from_the_sdk_cache() {
        let fixture = FixtureServer {
            list_changed_supported: true,
            list_tools_ttl_ms: Some(600_000),
            ..FixtureServer::default()
        };
        let list_calls = fixture.list_tools_calls.clone();
        let host = HttpFixtureHost::start(fixture).await;
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = host.connect(&workspace_dir).await;

        let first = runtime.list_tools().await.expect("first tools/list");
        assert_eq!(
            list_calls.load(std::sync::atomic::Ordering::Acquire),
            1,
            "the first refresh contacted the peer"
        );
        let second = runtime.list_tools().await.expect("second tools/list");
        assert_eq!(
            list_calls.load(std::sync::atomic::Ordering::Acquire),
            2,
            "the second refresh contacted the peer again despite the positive ttlMs"
        );
        assert_eq!(first, second, "the peer answered both refreshes itself");
        runtime.close().await.expect("physical settlement");
        host.shutdown().await;
    }

    /// **The failed-refresh observability regression.** After a successful
    /// catalog carrying a positive `ttlMs`, a refresh that really fails is
    /// observed as the failure it is.
    ///
    /// This is the external contract, stated without reference to which SDK
    /// cache branch could violate it: no cached success may ever stand in
    /// for a failed refresh. Handing the capability coordinator a fabricated
    /// success would rob it of the one fact its last-known-good contract is
    /// built on — that *this* refresh failed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failed_catalog_refresh_is_never_answered_by_a_stale_cached_success() {
        let fixture = FixtureServer {
            list_changed_supported: true,
            list_tools_ttl_ms: Some(600_000),
            list_tools_fail_from: Some(2),
            ..FixtureServer::default()
        };
        let list_calls = fixture.list_tools_calls.clone();
        let host = HttpFixtureHost::start(fixture).await;
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = host.connect(&workspace_dir).await;

        runtime
            .list_tools()
            .await
            .expect("the first refresh succeeds and declares a positive ttlMs");
        let failure = runtime
            .list_tools()
            .await
            .expect_err("the second refresh must surface the real server failure");
        assert!(
            matches!(failure, McpError::Discovery(_)),
            "a correlated remote catalog failure stays a discovery failure: {failure:?}"
        );
        assert_eq!(
            list_calls.load(std::sync::atomic::Ordering::Acquire),
            2,
            "the failing refresh reached the peer instead of a cached entry"
        );
        // The transport is healthy: only the catalog request failed, so the
        // generation is still usable and the failure is not transport loss.
        runtime.close().await.expect("physical settlement");
        host.shutdown().await;
    }

    /// **The Streamable HTTP transparency contract.** For a negotiated
    /// `2026-07-28` connection, rustX's request-ownership wrapper is
    /// protocol-transparent: the SDK's SEP-2243 routing metadata arrives at
    /// the server unchanged, and no session identity exists on either side.
    ///
    /// Both halves are proven from the recorded wire, and both are facts
    /// rustX does not manufacture: `Mcp-Method`, `Mcp-Name`, the
    /// `Mcp-Param-*` promoted from the called tool's `x-mcp-header`
    /// annotation, and `Mcp-Protocol-Version` are all generated inside rmcp
    /// and merely forwarded. The statelessness half is a negotiated property
    /// rather than a server that never had sessions to give: the host runs
    /// with rmcp's default `legacy_session_mode`, so it is session-capable by
    /// construction, and the test asserts that default holds alongside the
    /// absence of any session id on the wire.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn modern_streamable_http_is_stateless_and_forwards_sdk_routing_headers() {
        let fixture = FixtureServer {
            list_changed_supported: true,
            supported_versions: Some(vec![ProtocolVersion::V_2026_07_28]),
            modern_conformance_tools: true,
            ..FixtureServer::default()
        };
        let host = HttpFixtureHost::start(fixture).await;
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = host.connect(&workspace_dir).await;
        assert_eq!(runtime.protocol_version(), &ProtocolVersion::V_2026_07_28);

        let server_id = McpServerId::new("http-fixture");
        let tools = runtime.list_tools().await.expect("tools/list");
        assert!(
            tools
                .iter()
                .any(|tool| tool.name == rustx::tools::mcp::fixture::ROUTED_TOOL),
            "the routing conformance tool is published: {tools:?}"
        );
        let result = call_canonical_tool_with(
            &runtime,
            &server_id,
            tools,
            rustx::tools::mcp::fixture::ROUTED_TOOL,
            serde_json::json!({"region": "us-west1"}),
            &workspace_dir,
            "issue240-routing-headers",
        )
        .await;
        assert!(
            matches!(
                result.status,
                rustx::tools::types::ToolExecutionStatus::Success
            ),
            "the routed call succeeded: {result:?}"
        );
        assert!(
            result
                .model_facing_projection()
                .as_text()
                .contains("us-west1"),
            "the request body still carried the argument the header was promoted from: {result:?}"
        );

        let exchanges = host.exchanges();
        let call = exchanges
            .iter()
            .find(|exchange| exchange.request_header("mcp-method").as_deref() == Some("tools/call"))
            .expect("the tools/call POST was recorded");
        assert_eq!(
            call.request_header("mcp-name").as_deref(),
            Some(rustx::tools::mcp::fixture::ROUTED_TOOL),
            "rmcp's `Mcp-Name` reached the server unchanged"
        );
        assert_eq!(
            call.request_header("mcp-protocol-version").as_deref(),
            Some(ProtocolVersion::V_2026_07_28.as_str()),
            "the negotiated revision travels with every modern request"
        );
        assert_eq!(
            call.request_header(&format!(
                "mcp-param-{}",
                rustx::tools::mcp::fixture::ROUTED_TOOL_HEADER
            ))
            .as_deref(),
            Some("us-west1"),
            "the SDK-promoted `Mcp-Param-*` routing header survived the ownership wrapper"
        );
        assert!(
            exchanges.iter().all(|exchange| {
                exchange.request.get("mcp-session-id").is_none()
                    && exchange.response.get("mcp-session-id").is_none()
            }),
            "a modern peer issues no MCP session id, and rustX invents none"
        );
        assert!(
            rmcp::transport::streamable_http_server::StreamableHttpServerConfig::default()
                .legacy_session_mode,
            "the host under test is session-capable by construction, so the absence of a \
             session id above is a negotiated 2026-07-28 property (SEP-2567) rather than a \
             server that never had sessions to issue"
        );
        runtime.close().await.expect("physical settlement");
        host.shutdown().await;
    }

    /// **The modern result-framing contract.** MCP 2026 complete results
    /// whose `structuredContent` is a scalar or an array are accepted and
    /// projected deterministically into rustX's existing canonical tool
    /// result representation.
    ///
    /// `structuredContent` is arbitrary JSON at the adapter boundary and
    /// stays arbitrary JSON in the canonical projection: rustX narrows it to
    /// neither an object nor a typed output schema, and no new typed-output
    /// framework exists to make that true.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn modern_result_framing_accepts_non_object_structured_content() {
        let fixture = FixtureServer {
            list_changed_supported: true,
            supported_versions: Some(vec![ProtocolVersion::V_2026_07_28]),
            modern_conformance_tools: true,
            ..FixtureServer::default()
        };
        let host = HttpFixtureHost::start(fixture).await;
        let workspace_dir = tempfile::tempdir().expect("workspace");
        let runtime = host.connect(&workspace_dir).await;
        let server_id = McpServerId::new("http-fixture");
        let tools = runtime.list_tools().await.expect("tools/list");

        for (tool, expected) in [
            (
                rustx::tools::mcp::fixture::STRUCTURED_SCALAR_TOOL,
                serde_json::json!(rustx::tools::mcp::fixture::STRUCTURED_SCALAR_VALUE),
            ),
            (
                rustx::tools::mcp::fixture::STRUCTURED_ARRAY_TOOL,
                serde_json::json!([1, 2, 3]),
            ),
        ] {
            let result = call_canonical_tool(
                &runtime,
                &server_id,
                tools.clone(),
                tool,
                &workspace_dir,
                "issue240-structured-content",
            )
            .await;
            assert!(
                matches!(
                    result.status,
                    rustx::tools::types::ToolExecutionStatus::Success
                ),
                "{tool} must succeed: {result:?}"
            );
            assert!(
                result.content.iter().any(|content| matches!(
                    content,
                    rustx::tools::types::ToolResultContent::Json { value } if *value == expected
                )),
                "{tool} projects its structured content verbatim: {:?}",
                result.content
            );
        }
        runtime.close().await.expect("physical settlement");
        host.shutdown().await;
    }
}
