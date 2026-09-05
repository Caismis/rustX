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

use std::sync::Arc;

use crate::durable::ConversationStore;
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
/// In an ordinary test run there is no entry selection in the environment
/// and this test does nothing. Re-executed by a parent test, this process
/// becomes a real subagent child running the production
/// `run_subagent_child` stack.
#[test]
fn child_process_entry() {
    if std::env::var_os(CHILD_ENTRY_ENV).is_none() {
        return;
    }
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
                 exec '{}' '{CHILD_ENTRY_TEST}' --exact --nocapture --test-threads=1\n",
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

        let conversation_id = ConversationId::new("conv-issue145-e2e");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
                .expect("in-memory durable store"),
        );
        let mailbox =
            crate::runtime::inbound::ConversationInboundMailbox::over_store(store.clone());
        let registry = SubagentRegistry::new(SubagentRegistryConfig {
            conversation_id: conversation_id.clone(),
            agent_id: AgentId::new("agent-parent"),
            mailbox,
            clock: Arc::new(crate::runtime::types::SystemClock),
            monotonic_clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
            spawn: crate::runtime::subagent::SubagentSpawnPlan {
                program: wrapper,
                runtime_root: runtime_root.clone(),
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                agent_status: crate::context::AgentStatusConfig::default(),
                context: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
            },
            workspace: crate::runtime::subagent::SubagentWorkspaceManager::new(
                &workspace,
                &runtime_root,
            ),
            max_active: 4,
        });
        // The one ordinal this conversation will allocate: `prepare` burns
        // it for the staged child.
        let child_runtime_group = runtime_root.join("subagents").join(
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1).as_str(),
        );
        Self {
            _dir: dir,
            registry,
            store,
            child_runtime_group,
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
            resolved: ResolvedSubagentSpec {
                agent: crate::runtime::subagent::SubagentName::parse("explore").expect("name"),
                definition_digest: serde_json::from_value(serde_json::json!(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                ))
                .expect("digest"),
                execution_deadline: None,
                workspace_policy:
                    crate::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
                instructions: "frozen child instructions".to_owned(),
                model: crate::model::frozen::test_frozen_model_spec(
                    serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
                ),
                tools: vec![crate::runtime::subagent::ResolvedSubagentTool::Builtin {
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                    definition,
                }],
                skills: Vec::new(),
                project_instructions: Vec::new(),
                materialization:
                    crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
            },
            approval_mode: crate::runtime::ApprovalMode::Policy,
            task: "inspect the repository".to_owned(),
            context: None,
            tool_call_id: crate::runtime::identity::ToolCallId::new("call-1"),
            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
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
        assert_eq!(
            std::fs::read_dir(&self.child_runtime_group)
                .expect("the semantic child grouping")
                .count(),
            0,
            "rollback removed exactly the staged physical incarnation"
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
