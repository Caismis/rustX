//! Shared fixtures for distinct Job and Agent controls.

use std::sync::Arc;

use rustx::runtime::identity::{AgentId, ConversationId, ToolCallId};
use rustx::runtime::subagent::{SubagentRegistry, SubagentRegistryConfig, SubagentSpawnPlan};
use rustx::runtime::types::SystemClock;
use rustx::runtime::workspace::WorkspaceManager;
use rustx::tools::types::{
    ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolResultContent,
};

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
    subagent_plane_for("conv_35227a88-2fb4-735f-ad8e-ec0b35ff2a42")
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
            session_id: crate::runtime::identity::SessionId::new(
                "ses_01900000-0000-7000-8000-000000000001",
            ),

            program: std::path::PathBuf::from("/nonexistent/rustx"),
            product_root: crate::runtime::local_storage::ProductRoot::create(&runtime_root.clone())
                .expect("product root"),
        },
        workspace: WorkspaceManager::new(&workspace, &runtime_root),
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
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new("call-162-bg"),
        },
        tool_id: rustx::runtime::identity::ToolId::new(format!("tool-{tool}")),
        tool_name: tool.to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    }
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
