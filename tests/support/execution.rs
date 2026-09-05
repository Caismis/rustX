//! The shared fixture for the `execution` intrinsic control-plane suites.
//!
//! One `ExecutionFixture` registers exactly the `execution` intrinsic over
//! the conversation background registry and (optionally) a subagent
//! registry, so both the deterministic contract half
//! (`scripted_suites::background::execution_intrinsic`) and the boundary
//! conformance half (`boundary_suites::subagent::execution_routing`) drive
//! the same preflight path.

use std::sync::Arc;

use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::{AgentId, ConversationId, ToolCallId};
use rustx::runtime::subagent::{
    SubagentRegistry, SubagentRegistryConfig, SubagentSpawnPlan, SubagentWorkspaceManager,
};
use rustx::runtime::types::{CancellationReason, SystemClock};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{
    ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolResultContent,
};

use crate::scripted_suites::common;

/// The conversation-owned subagent plane of one deterministic test: a real
/// in-memory durable store, a real registry, and a staging seam for
/// scripted child processes.
pub(crate) struct SubagentPlane {
    pub(crate) registry: SubagentRegistry,
    pub(crate) store: Arc<rustx::durable::SqliteConversationStore>,
    pub(crate) conversation_id: ConversationId,
    pub(crate) runtime_root: std::path::PathBuf,
    /// The temporary directory owner, declared LAST: struct fields drop in
    /// declaration order, so the registry and every handle obtained from it
    /// drop before the directory is removed.
    #[allow(clippy::used_underscore_binding)]
    _dir: tempfile::TempDir,
}

pub(crate) fn subagent_plane() -> SubagentPlane {
    subagent_plane_for("conv-162")
}

/// The same plane under an explicit conversation identity, for the
/// conversation-isolation regressions that need two distinct conversations.
pub(crate) fn subagent_plane_for(conversation: &str) -> SubagentPlane {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = dir.path().join("workspace");
    let runtime_root = dir.path().join("runtime");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&runtime_root).expect("runtime root");
    let conversation_id = ConversationId::new(conversation);
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::in_memory(conversation_id.clone())
            .expect("in-memory store"),
    );
    let mailbox = rustx::runtime::inbound::ConversationInboundMailbox::over_store(store.clone());
    let registry = SubagentRegistry::new(SubagentRegistryConfig {
        conversation_id: conversation_id.clone(),
        agent_id: AgentId::new("agent-parent-162"),
        mailbox,
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(rustx::runtime::ManualMonotonicClock::new()),
        spawn: SubagentSpawnPlan {
            program: std::path::PathBuf::from("/nonexistent/rustx"),
            runtime_root: runtime_root.clone(),
            model_timeout_policy: rustx::model::ModelTimeoutPolicy::default(),
            agent_status: rustx::context::AgentStatusConfig::default(),
            context: rustx::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
        },
        workspace: SubagentWorkspaceManager::new(&workspace, &runtime_root),
        max_active: 4,
    });
    SubagentPlane {
        registry,
        store,
        conversation_id,
        runtime_root,
        _dir: dir,
    }
}

/// A background invocation of `tool` through the conversation's tool
/// runtime, mirroring `m5_background`'s fixture.
pub(crate) fn background_invocation(tool: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: ToolCallId::new("call-162-bg"),
        tool_id: rustx::runtime::identity::ToolId::new(format!("tool-{tool}")),
        tool_name: tool.to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    }
}

/// A registry that registers exactly the `execution` intrinsic over the
/// given domain registries, plus the conversation tool runtime whose
/// background registry backs the tool kind.
pub(crate) struct ExecutionFixture {
    /// The temporary directory owner, declared LAST: struct fields drop in
    /// declaration order, so the runtime drops before the directory.
    _dir: tempfile::TempDir,
    pub(crate) runtime: rustx::tools::runtime::ConversationToolRuntime,
    registry: ToolRegistry,
}

pub(crate) fn execution_fixture(subagents: Option<SubagentRegistry>) -> ExecutionFixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace directory");
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("artifact directory");
    let conversation_id = ConversationId::new("conv-162-execution");
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::open(
            conversation_id.clone(),
            &artifacts.join("conversation.sqlite"),
        )
        .expect("durable store"),
    );
    let runtime = rustx::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id,
        rustx::tools::runtime::ConversationRuntimeConfig {
            durable_binding: Some(rustx::durable::ConversationStoreBinding::new(store.clone())),
            ..rustx::tools::runtime::ConversationRuntimeConfig::new(&workspace_root, &artifacts)
        },
    )
    .expect("tool runtime");
    let registration =
        crate::tools::native::execution::registration(runtime.background().clone(), subagents);
    let mut registry = ToolRegistry::new();
    registry
        .register_with_activation_metadata(
            registration.definition,
            registration.executor,
            registration.normalizer,
            false,
        )
        .expect("execution registers");
    ExecutionFixture {
        _dir: dir,
        runtime,
        registry,
    }
}

/// Executes one `execution` invocation against the fixture's registry,
/// through the real preflight path.
pub(crate) async fn run_execution(
    fixture: &ExecutionFixture,
    arguments: serde_json::Value,
) -> rustx::tools::types::ToolExecutionResult {
    use rustx::tools::executor::{PreflightOutcome, ToolExecutionContext};
    use rustx::tools::types::ToolCall;
    let definition = fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "execution")
        .expect("execution registered");
    let call = ToolCall {
        id: ToolCallId::new("call-162-execution"),
        tool_id: definition.id,
        name: "execution".to_owned(),
        arguments,
    };
    let outcome = fixture.registry.preflight(&call).expect("preflight");
    let PreflightOutcome::Ready(prepared) = outcome else {
        panic!("execution calls preflight as ready");
    };
    let executor = fixture.registry.executor(&prepared.invocation.tool_id);
    let reporter = common::NoopProgress;
    let context = ToolExecutionContext::new(
        fixture.runtime.conversation_id(),
        None,
        rustx::runtime::ExecutionCancellation::detached(
            CancellationSignal::new(),
            CancellationReason::UserRequested,
        ),
        fixture.runtime.workspace(),
        &reporter,
        fixture.runtime.artifacts(),
        fixture.runtime.tool_output(),
        fixture.runtime.environment(),
    );
    executor.execute(prepared.invocation, context).await
}

/// The single JSON content block of a successful structured result.
pub(crate) fn json_content(result: &rustx::tools::types::ToolExecutionResult) -> serde_json::Value {
    assert_eq!(result.status, ToolExecutionStatus::Success);
    match &result.content[0] {
        ToolResultContent::Json { value } => value.clone(),
        other => panic!("expected JSON, got {other:?}"),
    }
}

/// The failure message of a failed result.
pub(crate) fn failure_message(result: &rustx::tools::types::ToolExecutionResult) -> String {
    match &result.status {
        ToolExecutionStatus::Failed { error } => error.clone(),
        other => panic!("expected failure, got {other:?}"),
    }
}
