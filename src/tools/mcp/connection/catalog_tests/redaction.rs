//! Credential-bearing malformed stdio output never reaches canonical results.
use super::*;
use crate::credentials::{CredentialSnapshot, SourceCredentials};
use crate::tools::mcp::fixture::{fixture_spawn_args, raw};

const SECRET: &str = "PR503_FAKE_SECRET_a91";
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

struct ReleasePause(Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>);
impl Drop for ReleasePause {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_stdio_after_listing_redacts_pre_dispatch_failure_and_model_projection() {
    if raw::serve_if_raw_fixture_mode().await {
        return;
    }
    assert_malformed_stdio_redaction(
        SECRET,
        concat!(
            "tools::mcp::connection::catalog_tests::redaction::",
            "malformed_stdio_after_listing_redacts_pre_dispatch_failure_and_model_projection"
        ),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_stdio_redacts_long_credentials_before_truncating_the_wire_preview() {
    if raw::serve_if_raw_fixture_mode().await {
        return;
    }
    let secret = format!("PR503_FAKE_LONG_SECRET_{}_END", "a".repeat(256));
    assert_malformed_stdio_redaction(
        &secret,
        concat!(
            "tools::mcp::connection::catalog_tests::redaction::",
            "malformed_stdio_redacts_long_credentials_before_truncating_the_wire_preview"
        ),
    )
    .await;
}

#[allow(clippy::too_many_lines)] // One real stdio validation/retirement boundary.
async fn assert_malformed_stdio_redaction(secret: &str, test_name: &str) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let tools = ConversationToolRuntime::new(
        ConversationId::new("conv_14dafca7-8c38-7c20-8ac1-caa3300d9cab"),
        workspace,
        temp.path().join("artifacts"),
    )
    .unwrap();
    let journal = temp.path().join("stdio-journal");
    let release = temp.path().join("corruption.release");
    let mut credentials: SourceCredentials = serde_json::from_value(serde_json::json!({
        "environment": {(raw::RAW_CREDENTIAL_ENV): "$CATALOG_CREDENTIAL"}
    }))
    .unwrap();
    credentials.capture(CredentialSnapshot::new([(
        "CATALOG_CREDENTIAL".into(),
        secret.into(),
    )]));
    let binding = McpServerBinding {
        credentials,
        transport: McpTransportConfig::Stdio {
            program: std::env::current_exe().unwrap().display().to_string(),
            args: fixture_spawn_args(test_name),
            cwd: None,
            environment: [
                (raw::RAW_FIXTURE_MODE_ENV.into(), "1".into()),
                (
                    raw::RAW_CORRUPTION_ENV.into(),
                    raw::CORRUPTION_INVALID.into(),
                ),
                (
                    raw::RAW_INVALID_PHASE_ENV.into(),
                    raw::INVALID_PHASE_VALIDATION.into(),
                ),
                (raw::RAW_JOURNAL_ENV.into(), journal.display().to_string()),
                (
                    raw::RAW_INVALID_RELEASE_FILE_ENV.into(),
                    release.display().to_string(),
                ),
            ]
            .into(),
        },
        policy: crate::tools::types::ToolInvocationPolicy::default(),
    };
    let connection = McpConnection::lazy(
        McpServerId::new("credential-catalog"),
        binding.clone(),
        tools.workspace().clone(),
        Arc::default(),
        CancellationSignal::new(),
    );
    let expected = connection.discover_catalog().await.unwrap();
    assert_eq!(expected.len(), 1);
    let acquired = connection.acquire().await.unwrap();
    let runtime = connection.published_runtime().unwrap();
    assert!(runtime.unusable_reason().is_none());
    let pid = runtime
        .process
        .as_ref()
        .unwrap()
        .lock()
        .await
        .supervisor_child_pid
        .unwrap();
    let retirement = McpRuntimeRetirementRegistry::new();
    let generation = McpRuntimeGeneration::from_connection(
        McpServerId::new("credential-catalog"),
        connection.clone(),
        None,
        tokio::runtime::Handle::current(),
        &retirement,
    );
    let entries = crate::tools::mcp::executor::definitions_owned(
        &McpServerId::new("credential-catalog"),
        binding.policy,
        &generation.binding(),
        expected,
    );
    let (definition, executor) = &entries[0];
    let context = ToolExecutionContext::new(
        tools.conversation_id(),
        None,
        crate::runtime::ExecutionCancellation::detached(
            CancellationSignal::new(),
            crate::runtime::types::CancellationReason::UserRequested,
        ),
        tools.workspace(),
        &Silent,
        tools.artifacts(),
        tools.tool_output(),
        tools.environment(),
    );
    let pause = Arc::new(crate::tools::mcp::test_sync::ConnectOwnershipPause::default());
    let _release_pause = ReleasePause(pause.clone());
    *connection.catalog.before_validate_pause.lock().unwrap() = Some(pause.clone());
    let execution = executor
        .start(
            ToolInvocation {
                id: ToolInvocationId::Agent {
                    call_id: ToolCallId::new("credential-catalog-call"),
                },
                tool_id: definition.id.clone(),
                tool_name: definition.name.clone(),
                mode: ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            context,
        )
        .completion;
    tokio::pin!(execution);
    tokio::select! {
        _ = &mut execution => panic!("final catalog validation is held"),
        reached = tokio::time::timeout(WAIT, pause.wait_entered()) => reached.unwrap(),
    }
    std::fs::write(&release, []).unwrap();
    tokio::time::timeout(WAIT, async {
        while runtime.protocol_violation.violation().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    pause.release();
    let result = tokio::time::timeout(WAIT, execution).await.unwrap();
    let discovery_error = connection
        .discover_acquired_catalog(acquired)
        .await
        .expect_err("a poisoned transport cannot serve a catalog")
        .to_string();
    let poison_error = runtime.poisoned_protocol_violation().await.unwrap();
    assert!(generation.retire_and_close().await.is_none());
    assert_eq!(
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
            None
        ),
        Err(nix::errno::Errno::ESRCH),
        "the physical stdio supervisor is reaped before checking diagnostics"
    );
    let journal = std::fs::read_to_string(journal).unwrap();
    assert_eq!(
        journal
            .lines()
            .filter(|line| *line == raw::JOURNAL_TOOLS_LIST)
            .count(),
        1
    );
    assert!(
        !journal.contains("tools/call"),
        "no tool request crossed stdio"
    );
    assert_eq!(connection.established_count().await, 1);
    let crate::tools::types::ToolExecutionStatus::Failed { error } = &result.status else {
        panic!("the invalid frame must fail before dispatch");
    };
    assert!(error.contains("MCP protocol violation"));
    assert!(error.contains("structurally invalid stdio"));
    assert!(error.contains("method"));
    let projection = result.model_facing_projection().as_text();
    let health = runtime.unusable_reason().unwrap();
    let recorded = runtime.protocol_violation.violation().unwrap();
    for (name, diagnostic) in [
        ("failed status", error.as_str()),
        ("model-facing projection", projection.as_str()),
        ("before-discovery health check", discovery_error.as_str()),
        ("poisoned invocation diagnostic", poison_error.as_str()),
        ("runtime health diagnostic", health.as_str()),
        ("bounded framing evidence", recorded.as_str()),
    ] {
        assert!(!diagnostic.contains(secret), "{name} leaked the credential");
        assert!(
            !diagnostic.contains(&secret[..12.min(secret.len())]),
            "{name} leaked the credential prefix across preview truncation"
        );
        assert!(
            !diagnostic.contains(&secret[secret.len().saturating_sub(12)..]),
            "{name} leaked the credential suffix across preview truncation"
        );
        assert!(
            diagnostic.contains("<redacted>"),
            "{name} must retain a redaction marker"
        );
        assert!(
            diagnostic.contains("method"),
            "{name} must retain the wire diagnosis"
        );
    }
}
