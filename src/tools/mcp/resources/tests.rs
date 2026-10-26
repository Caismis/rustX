//! Real transport tests count requests and order cancellation using channels.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::runtime::CancellationSignal;
use crate::runtime::identity::McpServerId;
use crate::tools::mcp::fixture::{self, FixtureServer};
use crate::tools::mcp::test_context::{NoProgress, context, runtime};
use crate::tools::mcp::{McpInvalidationState, McpServerBinding, McpTransportConfig};

use super::*;

struct HttpPeer {
    fixture: FixtureServer,
    resources_enabled: Arc<AtomicBool>,
    binding: McpServerBinding,
    server: tokio::task::JoinHandle<()>,
    shutdown: tokio_util::sync::CancellationToken,
}

impl HttpPeer {
    async fn start(fixture: FixtureServer) -> Self {
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        };
        let resources_enabled = Arc::new(AtomicBool::new(fixture.resources_supported));
        let enabled = resources_enabled.clone();
        let handler = fixture.clone();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.cancellation_token = shutdown.child_token();
        config.sse_keep_alive = None;
        let service = StreamableHttpService::<FixtureServer, LocalSessionManager>::new(
            move || {
                let mut handler = handler.clone();
                handler.resources_supported = enabled.load(Ordering::SeqCst);
                Ok(handler)
            },
            Arc::default(),
            config,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
                .await
                .unwrap();
        });
        Self {
            fixture,
            resources_enabled,
            server,
            shutdown,
            binding: McpServerBinding {
                credentials: crate::credentials::SourceCredentials::default(),
                policy: crate::tools::types::ToolInvocationPolicy::default(),
                transport: McpTransportConfig::StreamableHttp {
                    endpoint: format!("http://{address}/mcp"),
                    headers: BTreeMap::new(),
                },
            },
        }
    }

    async fn connect(
        &self,
        tools: &crate::tools::runtime::ConversationToolRuntime,
    ) -> Arc<McpConnection> {
        self.connect_as(tools, "resources").await
    }

    async fn connect_as(
        &self,
        tools: &crate::tools::runtime::ConversationToolRuntime,
        name: &str,
    ) -> Arc<McpConnection> {
        let invalidation = Arc::new(McpInvalidationState::default());
        let connected = McpServerRuntime::connect(
            &McpServerId::new(name),
            &self.binding,
            tools.workspace(),
            invalidation.clone(),
        )
        .await
        .unwrap();
        McpConnection::reconnectable(
            connected,
            self.binding.clone(),
            tools.workspace().clone(),
            invalidation,
            CancellationSignal::new(),
        )
    }

    async fn close(self, connection: &McpConnection) {
        assert!(connection.close().await.is_empty());
        self.shutdown.cancel();
        self.server.abort();
        let _ = self.server.await;
    }
}

fn json(result: &ToolExecutionResult) -> &serde_json::Value {
    assert_eq!(result.status, ToolExecutionStatus::Success, "{result:?}");
    let ToolResultContent::Json { value } = result
        .content
        .iter()
        .find(|block| matches!(block, ToolResultContent::Json { .. }))
        .unwrap()
    else {
        panic!("structured page: {result:?}");
    };
    value
}

#[tokio::test]
async fn negotiated_matrix_skips_unsupported_tool_lists_and_never_enumerates_at_startup() {
    for (tools, resources, empty) in [
        (false, true, false),
        (true, false, false),
        (true, true, false),
        (false, true, true),
    ] {
        let peer = HttpPeer::start(FixtureServer {
            resources_supported: resources,
            tools_disabled: !tools,
            tools_empty: true,
            resources_empty: empty,
            ..FixtureServer::default()
        })
        .await;
        let (_directory, tools_runtime) = runtime("conv_a38e2514-9865-7ead-bb93-e0051a6b54db");
        let connection = peer.connect(&tools_runtime).await;
        let transport = connection.acquire().await.unwrap();
        let discovered = connection
            .discover_acquired_catalog(transport.clone())
            .await
            .unwrap();
        assert_eq!(
            discovered.capabilities,
            crate::tools::mcp::McpNegotiatedCapabilities { tools, resources }
        );
        assert!(discovered.tools.is_empty());
        assert_eq!(
            peer.fixture.list_tools_calls.load(Ordering::SeqCst),
            usize::from(tools)
        );
        assert_eq!(peer.fixture.resource_state.lists.load(Ordering::SeqCst), 0);
        assert_eq!(
            peer.fixture.resource_state.templates.load(Ordering::SeqCst),
            0
        );
        assert_eq!(peer.fixture.resource_state.reads.load(Ordering::SeqCst), 0);
        let progress = NoProgress;
        let listed = connection
            .resource_request(
                McpResourceOperation::List { cursor: None },
                &context(&tools_runtime, None, &progress),
            )
            .await;
        if empty {
            assert_eq!(json(&listed)["resources"], serde_json::json!([]));
            let templates = connection
                .resource_request(
                    McpResourceOperation::Templates { cursor: None },
                    &context(&tools_runtime, None, &progress),
                )
                .await;
            assert_eq!(
                json(&templates)["resourceTemplates"][0]["uriTemplate"],
                "fixture://{name}"
            );
        } else if resources {
            assert_eq!(json(&listed)["nextCursor"], "page-2");
        } else {
            assert!(
                matches!(listed.status, ToolExecutionStatus::Failed { error } if error.contains("unsupported"))
            );
            assert_eq!(peer.fixture.resource_state.lists.load(Ordering::SeqCst), 0);
        }
        assert_eq!(transport.runtime().outstanding_http_requests(), 0);
        assert_eq!(transport.runtime().tracked_progress_requests(), 0);
        peer.close(&connection).await;
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one live peer exercises bounded pages and content translation end to end"
)]
async fn resource_http_pages_templates_direct_uris_blobs_and_errors_are_bounded() {
    let peer = HttpPeer::start(FixtureServer {
        resources_supported: true,
        tools_disabled: true,
        ..FixtureServer::default()
    })
    .await;
    let (_directory, tools) = runtime("conv_38be7f4e-622a-78f9-9cbb-f977a65e70f6");
    let connection = peer.connect(&tools).await;
    let progress = NoProgress;
    let context = context(&tools, None, &progress);
    let page = connection
        .resource_request(
            McpResourceOperation::List {
                cursor: Some("page-2".into()),
            },
            &context,
        )
        .await;
    assert_eq!(json(&page)["resources"][0]["uri"], "fixture://blob");
    assert!(json(&page).get("nextCursor").is_none());
    let template = connection
        .resource_request(McpResourceOperation::Templates { cursor: None }, &context)
        .await;
    assert_eq!(
        json(&template)["resourceTemplates"][0]["uriTemplate"],
        "fixture://{name}"
    );
    assert_eq!(json(&template)["nextCursor"], "templates-2");
    let end = connection
        .resource_request(
            McpResourceOperation::Templates {
                cursor: Some("templates-2".into()),
            },
            &context,
        )
        .await;
    assert_eq!(json(&end)["resourceTemplates"], serde_json::json!([]));
    for cursor in ["cycle", "oversized", "unknown"] {
        let failed = connection
            .resource_request(
                McpResourceOperation::List {
                    cursor: Some(cursor.into()),
                },
                &context,
            )
            .await;
        assert!(
            matches!(failed.status, ToolExecutionStatus::Failed { .. }),
            "{failed:?}"
        );
    }
    let counts = peer.fixture.resource_state.lists.load(Ordering::SeqCst);
    let invalid = connection
        .resource_request(
            McpResourceOperation::List {
                cursor: Some("x".repeat(MAX_CURSOR_BYTES + 1)),
            },
            &context,
        )
        .await;
    assert!(matches!(invalid.status, ToolExecutionStatus::Failed { .. }));
    assert_eq!(
        counts,
        peer.fixture.resource_state.lists.load(Ordering::SeqCst)
    );
    // No local file API is ever given this opaque URI; only this peer can answer it.
    let local_looking = connection
        .resource_request(
            McpResourceOperation::Read {
                uri: "file:///etc/passwd".into(),
            },
            &context,
        )
        .await;
    let body = json(&local_looking);
    assert_eq!(
        body["contents"][0]["text"],
        "peer content for file:///etc/passwd"
    );
    assert_eq!(body["contents"][1]["mimeType"], "text/markdown");
    let blob = connection
        .resource_request(
            McpResourceOperation::Read {
                uri: "fixture://blob".into(),
            },
            &context,
        )
        .await;
    assert!(json(&blob)["contents"][0].get("blob").is_none());
    assert_eq!(blob.artifacts.len(), 1);
    assert_eq!(
        tools
            .artifacts()
            .read_bounded(&blob.artifacts[0].artifact_id)
            .unwrap(),
        b"fixture binary"
    );
    let large = connection
        .resource_request(
            McpResourceOperation::Read {
                uri: "fixture://large".into(),
            },
            &context,
        )
        .await;
    assert_eq!(large.status, ToolExecutionStatus::Success);
    assert!(large.managed_output.is_some());
    assert!(large.truncation.is_some());
    for uri in ["fixture://invalid-blob", "fixture://error"] {
        let result = connection
            .resource_request(McpResourceOperation::Read { uri: uri.into() }, &context)
            .await;
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
    }
    assert_eq!(peer.fixture.list_tools_calls.load(Ordering::SeqCst), 0);
    peer.close(&connection).await;
}

#[tokio::test]
async fn resource_cancellation_before_and_after_http_dispatch_settles_ownership_without_replay() {
    let peer = HttpPeer::start(FixtureServer {
        resources_supported: true,
        tools_disabled: true,
        ..FixtureServer::default()
    })
    .await;
    let (_directory, tools) = runtime("conv_495d4851-1410-7cce-a04d-a5be0f225ce6");
    let connection = peer.connect(&tools).await;
    let transport = connection.acquire().await.unwrap();
    let progress = NoProgress;
    let mut context = context(&tools, None, &progress);
    let signal = CancellationSignal::new();
    context.cancellation = crate::runtime::ExecutionCancellation::detached(
        signal.clone(),
        crate::runtime::types::CancellationReason::UserRequested,
    );
    signal.cancel();
    let cancelled = connection
        .resource_request(
            McpResourceOperation::Read {
                uri: "fixture://text".into(),
            },
            &context,
        )
        .await;
    assert!(matches!(
        cancelled.status,
        ToolExecutionStatus::Cancelled { .. }
    ));
    assert_eq!(peer.fixture.resource_state.reads.load(Ordering::SeqCst), 0);
    let signal = CancellationSignal::new();
    context.cancellation = crate::runtime::ExecutionCancellation::detached(
        signal.clone(),
        crate::runtime::types::CancellationReason::UserRequested,
    );
    let pending = connection.resource_request(
        McpResourceOperation::Read {
            uri: "fixture://withhold".into(),
        },
        &context,
    );
    tokio::pin!(pending);
    tokio::select! {
        result = &mut pending => panic!("withheld read finished before cancellation: {result:?}"),
        () = peer.fixture.resource_state.read_entered.notified() => {}
    }
    assert_eq!(peer.fixture.resource_state.reads.load(Ordering::SeqCst), 1);
    signal.cancel();
    let result = pending.await;
    assert!(
        matches!(result.status, ToolExecutionStatus::OutcomeUnknown { .. }),
        "{result:?}"
    );
    assert_eq!(transport.runtime().outstanding_http_requests(), 0);
    assert_eq!(transport.runtime().tracked_progress_requests(), 0);
    assert_eq!(peer.fixture.resource_state.reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        connection.acquire().await.unwrap().generation(),
        transport.generation(),
        "local HTTP cancellation does not kill a live generation"
    );
    peer.close(&connection).await;
}

#[tokio::test]
async fn replacement_capability_removal_updates_existing_catalog_and_refuses_resource_dispatch() {
    let peer = HttpPeer::start(FixtureServer {
        resources_supported: true,
        tools_empty: true,
        ..FixtureServer::default()
    })
    .await;
    let (_directory, tools) = runtime("conv_45911ff7-d835-751f-bf83-c87f4a154f89");
    let connection = peer.connect(&tools).await;
    let progress = NoProgress;
    let context = context(&tools, None, &progress);
    let first = connection
        .resource_request(McpResourceOperation::List { cursor: None }, &context)
        .await;
    assert_eq!(first.status, ToolExecutionStatus::Success);
    let old = connection.acquire().await.unwrap();
    peer.resources_enabled.store(false, Ordering::SeqCst);
    old.runtime()
        .note_transport_loss("deterministic replacement barrier");
    let refused = connection
        .resource_request(
            McpResourceOperation::Read {
                uri: "fixture://text".into(),
            },
            &context,
        )
        .await;
    assert!(
        matches!(refused.status, ToolExecutionStatus::Failed { error } if error.contains("unsupported"))
    );
    assert_eq!(peer.fixture.resource_state.reads.load(Ordering::SeqCst), 0);
    assert!(connection.acquire().await.unwrap().generation() > old.generation());
    let (_, discovery) = connection.catalog_snapshot().unwrap();
    assert!(
        !discovery.unwrap().capabilities.resources,
        "safe adoption sees removal through the existing observation"
    );
    peer.close(&connection).await;
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one transport matrix checks protocol negotiation and all three resource operations"
)]
async fn resource_only_stdio_negotiates_modern_and_legacy_without_tools_list() {
    if fixture::legacy::serve_if_legacy_fixture_mode().await
        || fixture::serve_if_fixture_mode(FixtureServer::from_env()).await
    {
        return;
    }
    for legacy in [false, true] {
        let (directory, tools) = runtime("conv_7cdd269a-94f2-7f17-8ce2-513008f35978");
        let journal = directory.path().join("wire.jsonl");
        let mut environment = BTreeMap::from([
            (fixture::RESOURCES_ENV.into(), "1".into()),
            (fixture::TOOLS_DISABLED_ENV.into(), "1".into()),
            (
                fixture::resources::RESOURCE_OBSERVATION_FILE_ENV.into(),
                journal.to_string_lossy().into_owned(),
            ),
        ]);
        environment.insert(
            if legacy {
                fixture::legacy::LEGACY_FIXTURE_MODE_ENV
            } else {
                fixture::FIXTURE_MODE_ENV
            }
            .into(),
            "1".into(),
        );
        let wire = directory.path().join("legacy-wire.txt");
        if legacy {
            environment.insert(
                fixture::legacy::LEGACY_JOURNAL_ENV.into(),
                wire.to_string_lossy().into_owned(),
            );
        }
        let binding = McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            policy: crate::tools::types::ToolInvocationPolicy::default(),
            transport: McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                args: fixture::fixture_spawn_args(
                    "tools::mcp::resources::tests::resource_only_stdio_negotiates_modern_and_legacy_without_tools_list",
                ),
                cwd: None,
                environment,
            },
        };
        let connected = McpServerRuntime::connect(
            &McpServerId::new("resource-stdio"),
            &binding,
            tools.workspace(),
            Arc::default(),
        )
        .await
        .unwrap();
        let discovery = connected.discover().await.unwrap();
        assert!(discovery.capabilities.resources);
        assert!(!discovery.capabilities.tools);
        assert!(discovery.tools.is_empty());
        if legacy {
            assert_eq!(
                connected.protocol_version(),
                &fixture::legacy::LEGACY_FIXTURE_REVISION
            );
        }
        let connection = McpConnection::fixed(connected.clone());
        let progress = NoProgress;
        let context = context(&tools, None, &progress);
        assert_eq!(
            connection
                .resource_request(McpResourceOperation::List { cursor: None }, &context)
                .await
                .status,
            ToolExecutionStatus::Success
        );
        let result = connection
            .resource_request(
                McpResourceOperation::Read {
                    uri: "fixture://text".into(),
                },
                &context,
            )
            .await;
        assert_eq!(result.status, ToolExecutionStatus::Success, "{result:?}");
        let templates = connection
            .resource_request(McpResourceOperation::Templates { cursor: None }, &context)
            .await;
        assert_eq!(templates.status, ToolExecutionStatus::Success);
        let blob = connection
            .resource_request(
                McpResourceOperation::Read {
                    uri: "fixture://blob".into(),
                },
                &context,
            )
            .await;
        assert_eq!(blob.status, ToolExecutionStatus::Success);
        assert_eq!(
            tools
                .artifacts()
                .read_bounded(&blob.artifacts[0].artifact_id)
                .unwrap(),
            b"fixture binary"
        );
        assert_eq!(std::fs::read_to_string(journal).unwrap().lines().count(), 4);
        connected.close().await.unwrap();
        if legacy {
            assert!(
                !fixture::legacy::read_journal(&wire)
                    .iter()
                    .any(|method| method == "tools/list")
            );
        }
    }
}

#[test]
fn resource_decoding_and_metadata_limits_reject_excess_without_allocating_decoded_blobs() {
    let (_directory, tools) = runtime("conv_25301e25-e6b7-70e0-8cbb-ef362d48d277");
    let progress = NoProgress;
    let context = context(&tools, None, &progress);
    let oversized = ResourceContents::blob(
        "A".repeat(MAX_BLOB_BYTES.div_ceil(3) * 4 + 1),
        "fixture://blob",
    );
    assert!(
        translate_resource_contents(vec![oversized], &context, Instant::now())
            .unwrap_err()
            .contains("decoded byte limit")
    );
    let too_many = vec![ResourceContents::text("a", "fixture://text"); MAX_RESOURCE_BLOCKS + 1];
    assert!(
        translate_resource_contents(too_many, &context, Instant::now())
            .unwrap_err()
            .contains("64 content block")
    );
}

async fn run_helper(
    entry: &(
        crate::tools::types::ToolDefinition,
        Arc<dyn crate::tools::executor::ToolExecutor>,
    ),
    arguments: serde_json::Value,
    tools: &crate::tools::runtime::ConversationToolRuntime,
) -> ToolExecutionResult {
    use crate::tools::types::{ToolInvocation, ToolInvocationId, ToolInvocationMode};
    let progress = NoProgress;
    entry
        .1
        .start(
            ToolInvocation {
                id: ToolInvocationId::Agent {
                    call_id: crate::runtime::identity::ToolCallId::new("resource-helper"),
                },
                tool_id: entry.0.id.clone(),
                tool_name: entry.0.name.clone(),
                mode: ToolInvocationMode::Foreground,
                arguments,
            },
            context(tools, None, &progress),
        )
        .completion
        .await
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one frozen group exercises authorized and unauthorized live peers"
)]
async fn native_resource_helpers_route_only_frozen_sources_with_one_identity_ordered_group() {
    use crate::capabilities::ToolSourceId;
    use crate::tools::mcp::{McpRuntimeGeneration, McpRuntimeRetirementRegistry};
    let (_directory, tools) = runtime("conv_70c3904d-66d8-7921-b642-648830cff4af");
    let retirement = McpRuntimeRetirementRegistry::new();
    let mut peers = Vec::new();
    let mut generations = Vec::new();
    let mut authorized = BTreeMap::new();
    // Deliberately reversed creation order proves result order uses identities.
    for name in ["zulu", "alpha", "excluded"] {
        let peer = HttpPeer::start(FixtureServer {
            resources_supported: true,
            tools_disabled: true,
            ..FixtureServer::default()
        })
        .await;
        let connection = peer.connect_as(&tools, name).await;
        let generation = McpRuntimeGeneration::from_connection(
            McpServerId::new(name),
            connection.clone(),
            None,
            tokio::runtime::Handle::current(),
            &retirement,
        );
        if name != "excluded" {
            authorized.insert(
                ToolSourceId::Mcp(McpServerId::new(name)),
                generation.binding(),
            );
        }
        generations.push(generation);
        peers.push((peer, connection));
    }
    let helpers = crate::tools::mcp::resource_definitions(authorized);
    assert_eq!(helpers.len(), 3);
    assert_eq!(
        helpers
            .iter()
            .map(|entry| entry.0.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "list_mcp_resources",
            "list_mcp_resource_templates",
            "read_mcp_resource"
        ]
    );
    let listed = run_helper(&helpers[0], serde_json::json!({}), &tools).await;
    let aggregate = json(&listed);
    assert_eq!(aggregate["servers"][0]["server"], "alpha");
    assert_eq!(aggregate["servers"][1]["server"], "zulu");
    for page in aggregate["servers"].as_array().unwrap() {
        assert_eq!(page["content"][0]["value"]["nextCursor"], "page-2");
        assert_eq!(page["status"]["type"], "success");
    }
    for (peer, _) in &peers[..2] {
        assert_eq!(peer.fixture.resource_state.lists.load(Ordering::SeqCst), 1);
    }
    let named = run_helper(
        &helpers[2],
        serde_json::json!({"server":"alpha","uri":"fixture://text"}),
        &tools,
    )
    .await;
    assert_eq!(named.status, ToolExecutionStatus::Success);
    assert_eq!(
        peers[0]
            .0
            .fixture
            .resource_state
            .reads
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        peers[1]
            .0
            .fixture
            .resource_state
            .reads
            .load(Ordering::SeqCst),
        1
    );
    for server in ["excluded", "unknown"] {
        let denied = run_helper(
            &helpers[2],
            serde_json::json!({"server":server,"uri":"fixture://text"}),
            &tools,
        )
        .await;
        assert!(matches!(denied.status, ToolExecutionStatus::Failed { .. }));
    }
    let invalid_cursor =
        run_helper(&helpers[0], serde_json::json!({"cursor":"page-2"}), &tools).await;
    assert!(matches!(
        invalid_cursor.status,
        ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(
        peers[2]
            .0
            .fixture
            .resource_state
            .reads
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        peers[2]
            .0
            .fixture
            .resource_state
            .lists
            .load(Ordering::SeqCst),
        0
    );
    for generation in generations {
        assert!(generation.retire_and_close().await.is_none());
    }
    for (peer, connection) in peers {
        peer.close(&connection).await;
    }
}

#[tokio::test]
async fn resource_progress_is_owned_and_forwarded_through_the_normal_tool_lifecycle() {
    struct Progress(std::sync::atomic::AtomicUsize);
    impl crate::tools::executor::ProgressReporter for Progress {
        fn report(&self, _: crate::tools::types::ToolProgress) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let peer = HttpPeer::start(FixtureServer {
        resources_supported: true,
        tools_disabled: true,
        ..FixtureServer::default()
    })
    .await;
    let (_directory, tools) = runtime("conv_9df6836c-7d45-77b1-8e49-080872ee19eb");
    let connection = peer.connect(&tools).await;
    let silent = NoProgress;
    let progress = Progress(std::sync::atomic::AtomicUsize::new(0));
    let mut context = context(&tools, None, &silent);
    context.progress = &progress;
    let result = connection
        .resource_request(
            McpResourceOperation::Read {
                uri: "fixture://progress".into(),
            },
            &context,
        )
        .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(progress.0.load(Ordering::SeqCst), 1);
    let transport = connection.acquire().await.unwrap();
    assert_eq!(transport.runtime().tracked_progress_requests(), 0);
    assert_eq!(transport.runtime().outstanding_http_requests(), 0);
    peer.close(&connection).await;
}
