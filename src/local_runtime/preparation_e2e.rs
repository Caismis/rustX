//! The deterministic end-to-end regressions of Issue #145's pre-commit
//! cancellation ownership (Blocker 1), run against a **real child process**
//! with a controllable external-preparation seam.
//!
//! The child is this crate's own test binary re-executed in child mode
//! (the established pattern of [`crate::runtime::process_death`]): the
//! child needs both the real `rustx --subagent-child` runtime stack and the
//! `cfg(test)`-only seams, which a shipped binary deliberately never
//! carries. The seams involved:
//!
//! ```text
//! RUSTX_ISSUE145_CHILD_ENTRY        selects the child entry point below
//! RUSTX_ISSUE145_PREPARATION_GATE   a test-owned Unix socket the child's
//!                                   guarded external preparation parks on
//! RUSTX_ISSUE145_READY_MARKER       a file the child writes iff it answers
//!                                   Ready
//! ```
//!
//! The proven ordering of the test:
//!
//! ```text
//! child enters external preparation      (gate rendezvous line)
//!   -> parent attempt cancellation commits
//!   -> the parent handshake observes it, writes Cancel, and the child's
//!      preparation cancellation authority fires
//!   -> the gated external step settles, preparation settles
//!   -> the child exits without ever answering Ready
//!   -> the parent reaps it, contains its retained anchors, removes its
//!      physical incarnation root, and only then returns SpawnError::Cancelled
//!   -> NO Ready, NO durable SubagentOwnershipCommitted, NO record
//! ```
//!
//! The dangerous local race — Cancel consumed (signal set), *then* the
//! external step completes — cannot be ordered deterministically across a
//! process boundary (the parent cannot observe the child's event
//! consumption); it is proven in-process by
//! `subagent_child::tests::a_step_completing_after_cancellation_never_publishes_ready`,
//! where the gate registration exposes the exact cancellation signal the
//! gated step runs under.

use crate::durable::JournalStore;
use std::sync::Arc;

use crate::events::types::RuntimeEvent;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AgentId, ConversationId};
use crate::runtime::subagent::{
    ResolvedSubagentSpec, SubagentRegistry, SubagentRegistryConfig, SubagentStartError,
    SubagentStartSpec,
};

/// The liveness guard of one end-to-end run. Every ordering in this module
/// is established by the gate rendezvous, the wire, or a returned
/// settlement — never by a duration; this bound exists only so a broken
/// child fails loudly instead of hanging the suite.
const LIVENESS: std::time::Duration = std::time::Duration::from_mins(1);

/// Selects the child entry point in a re-executed test binary.
const CHILD_ENTRY_ENV: &str = "RUSTX_ISSUE145_CHILD_ENTRY";

/// The libtest path of the child entry point, used by the generated wrapper.
const CHILD_ENTRY_TEST: &str = "local_runtime::preparation_e2e::child_process_entry";

/// The gate socket env var (consumed by the composition seam).
const GATE_ENV: &str = "RUSTX_ISSUE145_PREPARATION_GATE";

/// The Ready-marker env var (consumed by the child driver seam).
const READY_MARKER_ENV: &str = "RUSTX_ISSUE145_READY_MARKER";

/// The one child entry point of this module.
///
/// The parent re-executes this test binary as a real subagent child running
/// the production
/// `run_subagent_child` stack.
#[test]
#[ignore = "fixture subprocess, invoked by its parent test"]
fn child_process_entry() {
    std::env::var_os(CHILD_ENTRY_ENV).expect("fixture parent must provide its environment");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("the child tokio runtime");
    let code = runtime.block_on(crate::local_runtime::subagent_child::run_subagent_child());
    std::process::exit(code);
}

/// The lab of one end-to-end run: the wrapper script the registry spawns as
/// the child program, the gate listener, and the proof files.
struct Lab {
    _dir: tempfile::TempDir,
    registry: SubagentRegistry,
    store: Arc<crate::durable::SqliteConversationStore>,
    child_runtime_group: std::path::PathBuf,
    product: crate::runtime::local_storage::ProductRoot,
    ready_marker: std::path::PathBuf,
    gate_listener: tokio::net::UnixListener,
}

impl Lab {
    /// Builds the lab: a real registry whose spawn plan points at a
    /// generated wrapper script re-executing this test binary in child
    /// mode.
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("lab");
        let workspace = dir.path().join("workspace");
        let runtime_root = dir.path().join("runtime");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&runtime_root).expect("runtime root");

        let gate_path = dir.path().join("preparation-gate.sock");
        let ready_marker = dir.path().join("ready.marker");
        let gate_listener =
            tokio::net::UnixListener::bind(&gate_path).expect("the gate listener binds");

        // The child program: a wrapper that re-executes this test binary in
        // child mode with the test seams armed. The registry's spawn appends
        // `--subagent-child` to argv; the wrapper deliberately does not
        // forward it — the libtest entry point *is* the child mode.
        let wrapper = dir.path().join("child.sh");
        let current = std::env::current_exe().expect("the test binary");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\n\
                 export {CHILD_ENTRY_ENV}=1\n\
                 export {GATE_ENV}='{}'\n\
                 export {READY_MARKER_ENV}='{}'\n\
                 exec '{}' '{CHILD_ENTRY_TEST}' --exact --ignored --nocapture --test-threads=1\n",
                gate_path.display(),
                ready_marker.display(),
                current.display(),
            ),
        )
        .expect("the child wrapper");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
                .expect("chmod wrapper");
        }

        let conversation_id = ConversationId::new("conv_2525f8d8-18aa-7bee-8704-e60e22b8e79b");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
                .expect("in-memory durable store"),
        );
        let mailbox =
            crate::runtime::inbound::ConversationInboundMailbox::over_store(store.clone());
        let product = crate::runtime::local_storage::ProductRoot::create(&runtime_root)
            .expect("product root");
        let registry = SubagentRegistry::new(SubagentRegistryConfig {
            conversation_id: conversation_id.clone(),
            agent_id: AgentId::new("agent-parent"),
            mailbox,
            clock: Arc::new(crate::runtime::types::SystemClock),
            monotonic_clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
            spawn: crate::runtime::subagent::SubagentSpawnPlan {
                session_id: crate::runtime::identity::SessionId::new(
                    "ses_01900000-0000-7000-8000-000000000001",
                ),

                program: wrapper,
                product_root: product.clone(),
            },
            workspace: std::path::PathBuf::from(&workspace),
            max_active: 4,
        });
        let child_runtime_group =
            runtime_root.join("sessions/ses_01900000-0000-7000-8000-000000000001/conversations");
        Self {
            _dir: dir,
            registry,
            store,
            child_runtime_group,
            product,
            ready_marker,
            gate_listener,
        }
    }

    /// The frozen start specification of the child under test: one real
    /// admitted Builtin capability, no external selection (the gate seam
    /// stands in for external preparation).
    fn spec() -> SubagentStartSpec {
        let definition = crate::tools::native::subagent_child_definition(
            "read",
            crate::tools::types::ToolInvocationPolicy::default(),
        )
        .expect("the child plane implements read");
        SubagentStartSpec {
            authority: crate::runtime::subagent::DurableAgentAuthority {
                title: "Explore task".into(),
                execution_policy: crate::runtime::subagent::InheritedExecutionPolicy::default(),
                resolved: ResolvedSubagentSpec {
                    resource_sources: std::collections::BTreeSet::new(),
                    environment: Vec::new(),
                    generation: crate::runtime::identity::RuntimeResourceRevision::new(1),
                    skill_roots: Vec::new(),

                    selection: crate::runtime::agent_profile::FrozenAgentSelection::default(),
                    agent: crate::runtime::subagent::SubagentName::parse("explore").expect("name"),
                    definition_digest: serde_json::from_value::<
                        crate::runtime::subagent::NamedAgentDefinitionDigest,
                    >(serde_json::json!(
                        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    ))
                    .expect("digest"),
                    execution_deadline: None,
                    instructions: "frozen child instructions".to_owned(),
                    model: crate::model::frozen::test_frozen_model_spec(
                        serde_json::from_value(serde_json::json!("local/model"))
                            .expect("model ref"),
                    ),
                    tools: vec![crate::runtime::subagent::ResolvedSubagentTool::Builtin {
                        tool_id: definition.id.clone(),
                        name: definition.name.clone(),
                        definition,
                    }],
                    skills: Vec::new(),
                    project_instructions: Vec::new(),
                    materialization:
                        crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(
                        ),
                    extensions: crate::extensions::NativeAgentExtensionsDocument::default()
                        .resolve(),
                },
            },
            admission: crate::runtime::subagent::ActivationAdmission {
                attachments: Vec::new(),
                task: "inspect the repository".to_owned(),
                context: None,
                origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                    tool_call_id: crate::runtime::identity::ToolCallId::new("call-1"),
                },
                terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                approval_mode: crate::runtime::types::ApprovalMode::FullAccess,
            },
        }
    }

    /// Parks until the child announces it entered external preparation, then
    /// returns the release channel.
    async fn await_external_preparation(&self) -> tokio::net::UnixStream {
        use tokio::io::AsyncBufReadExt;
        let (stream, _) = self
            .gate_listener
            .accept()
            .await
            .expect("the child connected to the preparation gate");
        let (read, write) = stream.into_split();
        let mut lines = tokio::io::BufReader::new(read).lines();
        let line = lines
            .next_line()
            .await
            .expect("the gate channel is readable")
            .expect("the child announced its preparation boundary");
        assert_eq!(
            line, "entered-external-preparation",
            "the child is provably inside external preparation"
        );
        lines
            .into_inner()
            .into_inner()
            .reunite(write)
            .expect("the gate stream reunites")
    }

    /// Every durable event committed in the parent's conversation.
    fn durable_events(&self) -> Vec<RuntimeEvent> {
        let mut all = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.store.read_events(cursor, 100).expect("events");
            if page.events.is_empty() {
                return all;
            }
            cursor = page.next_sequence;
            all.extend(page.events.into_iter().map(|envelope| envelope.event));
            if cursor.is_none() {
                return all;
            }
        }
    }

    /// The complete post-settlement assertions shared by both regressions.
    fn assert_never_owned(&self) {
        assert!(
            !self.ready_marker.exists(),
            "the child never answered Ready"
        );
        assert!(
            self.child_runtime_group.exists(),
            "the semantic grouping directory remains owned by the stable runtime root"
        );
        let allocations: Vec<_> = std::fs::read_dir(&self.child_runtime_group)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(allocations.len(), 1, "one UUID reservation was burned");
        assert!(
            ConversationId::parse(allocations[0].file_name().unwrap().to_str().unwrap()).is_ok()
        );
        let retained_directories: Vec<_> = std::fs::read_dir(&allocations[0])
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                entry
                    .file_type()
                    .unwrap()
                    .is_dir()
                    .then(|| entry.file_name())
            })
            .collect();
        assert_eq!(
            retained_directories,
            vec![std::ffi::OsString::from("physical-settlement")],
            "rollback removes mutable incarnation/materialization state and retains only settlement authority",
        );
        let conversation =
            ConversationId::parse(allocations[0].file_name().unwrap().to_str().unwrap()).unwrap();
        let session =
            crate::runtime::identity::SessionId::new("ses_01900000-0000-7000-8000-000000000001");
        let activations = crate::runtime::subagent::physical_recovery::consumed_activation_ids(
            &self.product,
            &session,
            &conversation,
        )
        .unwrap();
        assert_eq!(
            activations.len(),
            1,
            "exactly one activation identity was consumed"
        );
        assert!(
            crate::runtime::subagent::physical_recovery::prove_after_release(
                &self.product,
                &session,
                &conversation,
                &activations[0],
            )
            .unwrap()
            .is_some(),
            "the real child/rollback owner preserved physical proof"
        );
        assert!(
            self.durable_events()
                .iter()
                .all(|event| !matches!(event, RuntimeEvent::SubagentOwnershipCommitted { .. })),
            "no durable SubagentOwnershipCommitted survives a cancelled preparation"
        );
        assert!(
            self.registry.all_snapshots().is_empty(),
            "no ownership record or capacity consumption survives"
        );
    }
}

/// Attempt cancellation while a real child is inside external preparation:
/// the child never reaches `Ready`, never begins semantic work, every
/// staged physical resource settles before the start decision returns, and
/// no ownership fact survives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attempt_cancellation_during_external_preparation_settles_everything() {
    let lab = Lab::new();
    let attempt_cancellation = CancellationSignal::new();
    let prepare = tokio::spawn({
        let registry = lab.registry.clone();
        let spec = Lab::spec();
        let cancellation = attempt_cancellation.clone();
        async move { registry.prepare(&spec, &cancellation).await }
    });

    // 1. The child provably entered external preparation.
    let _gate = lab.await_external_preparation().await;

    // 2. The invoking attempt's cancellation commits now.
    attempt_cancellation.cancel();

    // 3. The start decision returns only after every staged physical
    //    resource settled (the rollback reaps the child, contains its
    //    retained anchors, and removes its runtime root).
    let outcome = tokio::time::timeout(LIVENESS, prepare)
        .await
        .expect("liveness: the cancelled preparation must settle")
        .expect("the prepare task must not panic");
    assert!(
        matches!(outcome, Err(SubagentStartError::Cancelled)),
        "a child cancelled before the ownership commit is never prepared: {}",
        outcome.map_or_else(|error| error.to_string(), |_| "prepared".to_owned())
    );
    lab.assert_never_owned();
}

#[cfg(feature = "mcp-fixture")]
async fn start_auth_mcp_fixture(
    private_ip: std::net::Ipv4Addr,
    expected_bearer: String,
    expected_sensitive: String,
) -> (
    std::net::SocketAddr,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio_util::task::AbortOnDropHandle<()>,
) {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = tokio::net::TcpListener::bind((private_ip, 0))
        .await
        .expect("bind Workspace HTTP MCP fixture");
    let address = listener.local_addr().expect("fixture address");
    let mut config = StreamableHttpServerConfig::default().with_json_response(true);
    config.sse_keep_alive = None;
    config.allowed_hosts = vec![address.to_string()];
    let fixture = crate::tools::mcp::fixture::FixtureServer {
        resources_supported: true,
        ..crate::tools::mcp::fixture::FixtureServer::default()
    };
    let service = StreamableHttpService::<
        crate::tools::mcp::fixture::FixtureServer,
        LocalSessionManager,
    >::new(move || Ok(fixture.clone()), Arc::default(), config);
    let authorized = Arc::new(AtomicUsize::new(0));
    let unauthorized = Arc::new(AtomicUsize::new(0));
    let auth_count = Arc::clone(&authorized);
    let unauth_count = Arc::clone(&unauthorized);
    let app = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let expected_bearer = expected_bearer.clone();
                let expected_sensitive = expected_sensitive.clone();
                let auth_count = Arc::clone(&auth_count);
                let unauth_count = Arc::clone(&unauth_count);
                async move {
                    let ok = request
                        .headers()
                        .get(http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        == Some(expected_bearer.as_str())
                        && request
                            .headers()
                            .get("x-sensitive")
                            .and_then(|value| value.to_str().ok())
                            == Some(expected_sensitive.as_str());
                    if ok {
                        auth_count.fetch_add(1, Ordering::SeqCst);
                        next.run(request).await
                    } else {
                        unauth_count.fetch_add(1, Ordering::SeqCst);
                        axum::response::IntoResponse::into_response(http::StatusCode::UNAUTHORIZED)
                    }
                }
            },
        ));
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    }));
    (address, authorized, unauthorized, server)
}

#[cfg(feature = "mcp-fixture")]
type RecordedMcpEnvironmentRequests =
    Arc<std::sync::Mutex<Vec<(String, String, std::collections::BTreeSet<String>)>>>;

#[cfg(feature = "mcp-fixture")]
fn spawn_mcp_environment_peer(
    mut client: std::os::unix::net::UnixStream,
    expected_origin: String,
    granted_values: std::collections::BTreeMap<String, String>,
) -> (RecordedMcpEnvironmentRequests, std::thread::JoinHandle<()>) {
    use rustx_execution_protocol::transport::{read_record, write_record};
    use rustx_execution_protocol::{ClientReply, ClientRequest, mcp_environment::Reply};

    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded_for_peer = Arc::clone(&recorded);
    let peer = std::thread::spawn(move || {
        while let Ok(request) = read_record::<ClientRequest>(&mut client) {
            let reply = match request {
                ClientRequest::Ping => ClientReply::Pong,
                ClientRequest::McpEnvironment(request) => {
                    let values =
                        if request.service == "inventory" && request.origin == expected_origin {
                            granted_values.clone()
                        } else {
                            std::collections::BTreeMap::new()
                        };
                    recorded_for_peer.lock().unwrap().push((
                        request.service,
                        request.origin,
                        request.names,
                    ));
                    ClientReply::McpEnvironment(Reply { values })
                }
                other => panic!("unexpected client request from subagent relay: {other:?}"),
            };
            write_record(&mut client, &reply).expect("write client reply");
        }
    });
    (recorded, peer)
}

#[cfg(feature = "mcp-fixture")]
fn workspace_http_subagent_spec(endpoint: String) -> SubagentStartSpec {
    use crate::credentials::{EnvironmentReference, SourceCredentials};
    use crate::tools::mcp::{McpServerBinding, McpTransportConfig};
    use std::collections::BTreeMap;

    let mut materialization =
        crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default();
    let server_id = crate::runtime::identity::McpServerId::new("inventory");
    let binding = McpServerBinding {
        credentials: SourceCredentials {
            headers: BTreeMap::from([(
                String::from("X-Sensitive"),
                EnvironmentReference::from_name("CLIENT_SENSITIVE_SECRET".to_owned()),
            )]),
            templates: BTreeMap::from([(
                String::from("CLIENT_TEMPLATE_SECRET"),
                EnvironmentReference::from_name("CLIENT_TEMPLATE_SECRET".to_owned()),
            )]),
            client_owned: true,
            workspace_source: true,
            ..Default::default()
        },
        transport: McpTransportConfig::StreamableHttp {
            endpoint,
            headers: BTreeMap::from([(
                String::from("Authorization"),
                String::from("Bearer $CLIENT_TEMPLATE_SECRET"),
            )]),
        },
        policy: crate::tools::types::ToolInvocationPolicy::default(),
        permissions: None,
        tool_permissions: BTreeMap::new(),
    };
    materialization.sources.insert(
        crate::capabilities::ToolSourceId::Mcp(server_id.clone()),
        binding,
    );

    SubagentStartSpec {
        authority: crate::runtime::subagent::DurableAgentAuthority {
            title: "Workspace HTTP child".into(),
            execution_policy: crate::runtime::subagent::InheritedExecutionPolicy::default(),
            resolved: ResolvedSubagentSpec {
                resource_sources: std::collections::BTreeSet::from([
                    crate::capabilities::ToolSourceId::Mcp(server_id),
                ]),
                environment: Vec::new(),
                generation: crate::runtime::identity::RuntimeResourceRevision::new(1),
                skill_roots: Vec::new(),
                selection: crate::runtime::agent_profile::FrozenAgentSelection::default(),
                agent: crate::runtime::subagent::SubagentName::parse("inventory-helper")
                    .expect("name"),
                definition_digest: serde_json::from_value::<
                    crate::runtime::subagent::NamedAgentDefinitionDigest,
                >(serde_json::json!(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                ))
                .expect("digest"),
                execution_deadline: None,
                instructions: "frozen child instructions".to_owned(),
                model: crate::model::frozen::test_frozen_model_spec(
                    serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
                ),
                tools: Vec::new(),
                skills: Vec::new(),
                project_instructions: Vec::new(),
                materialization,
                extensions: crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
            },
        },
        admission: crate::runtime::subagent::ActivationAdmission {
            attachments: Vec::new(),
            task: "connect workspace HTTP MCP".to_owned(),
            context: None,
            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                tool_call_id: crate::runtime::identity::ToolCallId::new("call-http-1"),
            },
            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
            approval_mode: crate::runtime::types::ApprovalMode::FullAccess,
        },
    }
}

#[cfg(feature = "mcp-fixture")]
fn workspace_http_subagent_registry(
    dir: &std::path::Path,
    canonical_workspace: std::path::PathBuf,
    session_id: crate::runtime::identity::SessionId,
    scope: crate::local_runtime::session_runtime_manager::SessionExecutionScope,
) -> SubagentRegistry {
    let runtime_root = dir.join("runtime");
    std::fs::create_dir_all(&runtime_root).expect("runtime");
    let wrapper = dir.join("child.sh");
    let current = std::env::current_exe().expect("test binary");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport {CHILD_ENTRY_ENV}=1\nexec '{}' '{CHILD_ENTRY_TEST}' --exact --ignored --nocapture --test-threads=1\n",
            current.display(),
        ),
    )
    .expect("wrapper");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
            .expect("chmod wrapper");
    }

    let conversation_id = ConversationId::new("conv_2525f8d8-18aa-7bee-8704-e60e22b8e79b");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
            .expect("in-memory store"),
    );
    let mailbox = crate::runtime::inbound::ConversationInboundMailbox::over_store(store);
    let product =
        crate::runtime::local_storage::ProductRoot::create(&runtime_root).expect("product root");
    SubagentRegistry::new(SubagentRegistryConfig {
        conversation_id,
        agent_id: AgentId::new("agent-parent"),
        mailbox,
        clock: Arc::new(crate::runtime::types::SystemClock),
        monotonic_clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
        spawn: crate::runtime::subagent::SubagentSpawnPlan {
            session_id,
            program: wrapper,
            product_root: product,
        },
        workspace: canonical_workspace,
        max_active: 4,
    })
    .with_execution_scope(Some(scope))
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagent_receives_parent_hydrated_client_workspace_http_mcp_credentials() {
    use rustx_execution_protocol::Capability;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::Ordering;

    let private_ip = match crate::tools::mcp::streamable_http::workspace_http::ensure_private_ipv4_or_reexec(
        "local_runtime::preparation_e2e::subagent_receives_parent_hydrated_client_workspace_http_mcp_credentials",
    ) {
        crate::tools::mcp::streamable_http::workspace_http::WorkspaceHttpFixtureIp::Ready(ip) => ip,
        crate::tools::mcp::streamable_http::workspace_http::WorkspaceHttpFixtureIp::DelegatedToIsolatedNetns => return,
    };
    let template_secret = "subagent-template-secret-91f";
    let sensitive_secret = "subagent-sensitive-secret-42c";
    let (address, authorized, unauthorized, _server) = start_auth_mcp_fixture(
        private_ip,
        format!("Bearer {template_secret}"),
        sensitive_secret.to_owned(),
    )
    .await;

    let dir = tempfile::tempdir().expect("lab");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let canonical_workspace = workspace.canonicalize().expect("canonical workspace");

    let origin = format!("http://{address}");
    let (admitted, client): (
        crate::local_runtime::session_runtime_manager::AdmittedTestExecution,
        _,
    ) = crate::local_runtime::session_runtime_manager::admitted_scope_for_workspace(
        &canonical_workspace,
        BTreeSet::from([Capability::Connection]),
    );
    let (recorded, peer) = spawn_mcp_environment_peer(
        client,
        origin.clone(),
        BTreeMap::from([
            (
                String::from("CLIENT_TEMPLATE_SECRET"),
                template_secret.to_owned(),
            ),
            (
                String::from("CLIENT_SENSITIVE_SECRET"),
                sensitive_secret.to_owned(),
            ),
        ]),
    );
    let registry = workspace_http_subagent_registry(
        dir.path(),
        canonical_workspace,
        admitted.session.clone(),
        admitted.scope.clone(),
    );

    let spec = workspace_http_subagent_spec(format!("{origin}/mcp"));
    let prepared = tokio::time::timeout(
        LIVENESS,
        registry.prepare(&spec, &CancellationSignal::new()),
    )
    .await
    .expect("liveness: subagent prepare must settle")
    .expect("subagent prepare must connect Workspace HTTP MCP with hydrated Client credentials");

    assert_eq!(
        *recorded.lock().unwrap(),
        vec![(
            String::from("inventory"),
            origin,
            BTreeSet::from([
                String::from("CLIENT_SENSITIVE_SECRET"),
                String::from("CLIENT_TEMPLATE_SECRET"),
            ]),
        )]
    );
    assert!(
        authorized.load(Ordering::SeqCst) >= 2,
        "child must make authenticated Streamable HTTP requests"
    );
    assert_eq!(unauthorized.load(Ordering::SeqCst), 0);

    let rollback = CancellationSignal::new();
    rollback.cancel();
    assert!(matches!(
        registry.commit(prepared, &rollback).await,
        Ok(crate::runtime::subagent::SubagentStartOutcome::RolledBack)
    ));
    drop(registry);
    drop(admitted);
    peer.join().unwrap();
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagent_prepare_cancels_promptly_while_client_withholds_mcp_environment() {
    use rustx_execution_protocol::transport::{read_record, write_record};
    use rustx_execution_protocol::{Capability, ClientReply, ClientRequest};
    use std::collections::BTreeSet;

    let dir = tempfile::tempdir().expect("lab");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let canonical_workspace = workspace.canonicalize().expect("canonical workspace");

    let (admitted, mut client): (
        crate::local_runtime::session_runtime_manager::AdmittedTestExecution,
        _,
    ) = crate::local_runtime::session_runtime_manager::admitted_scope_for_workspace(
        &canonical_workspace,
        BTreeSet::from([Capability::Connection]),
    );
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<()>();
    let seen_tx = std::sync::Mutex::new(Some(seen_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let peer = std::thread::spawn(move || {
        while let Ok(request) = read_record::<ClientRequest>(&mut client) {
            let reply = match request {
                ClientRequest::Ping => ClientReply::Pong,
                ClientRequest::McpEnvironment(_) => {
                    if let Some(sender) = seen_tx.lock().unwrap().take() {
                        let _ = sender.send(());
                    }
                    let _ = release_rx.recv();
                    break;
                }
                other => panic!("unexpected client request: {other:?}"),
            };
            write_record(&mut client, &reply).expect("write reply");
        }
    });

    let registry = workspace_http_subagent_registry(
        dir.path(),
        canonical_workspace,
        admitted.session.clone(),
        admitted.scope.clone(),
    );
    let spec = workspace_http_subagent_spec(String::from("http://192.0.2.1:9/mcp"));
    let attempt_cancellation = CancellationSignal::new();
    let prepare = tokio::spawn({
        let registry = registry.clone();
        let cancellation = attempt_cancellation.clone();
        async move { registry.prepare(&spec, &cancellation).await }
    });

    seen_rx.await.expect("McpEnvironment RPC entered wire");
    let cancel_started = std::time::Instant::now();
    attempt_cancellation.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), prepare)
        .await
        .expect("cancellation must settle well before the 5s RPC budget")
        .expect("prepare task must not panic");
    assert!(
        matches!(outcome, Err(SubagentStartError::Cancelled)),
        "expected Cancelled, got {}",
        outcome.map_or_else(|error| error.to_string(), |_| String::from("prepared"))
    );
    assert!(cancel_started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(registry.all_snapshots(), Vec::new());

    drop(release_tx);
    drop(registry);
    drop(admitted);
    peer.join().unwrap();
}
