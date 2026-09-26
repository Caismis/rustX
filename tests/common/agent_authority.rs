//! Explicit frozen authority for durable native Agent boundary fixtures.
use rustx::model::catalog::{
    CredentialSource, ModelCapabilities, ModelCompat, ModelRef, ProviderId, ResolvedCredential,
};
use rustx::model::frozen::{
    FrozenModelInvocation, FrozenModelSpec, FrozenProviderBinding, FrozenSummaryModel,
};
use rustx::model::session::SessionModelConfig;
use rustx::model::{ModelProtocol, RequestParams};

pub(crate) fn admit_agent(
    mut envelope: rustx::events::types::RuntimeEventEnvelope,
) -> (
    rustx::events::types::RuntimeEventEnvelope,
    rustx::runtime::subagent::DurableAgentAuthority,
) {
    if let rustx::events::types::RuntimeEvent::SubagentOwnershipCommitted {
        admitted_authority,
        child_agent_id,
        ownership: rustx::events::types::SubagentOwnershipKind::Normal,
        agent,
        definition_digest,
        workspace,
        ..
    } = &mut envelope.event
    {
        *admitted_authority = Some(child_agent_id.clone());
        let authority = rustx::runtime::subagent::DurableAgentAuthority {
            resolved: rustx::runtime::subagent::ResolvedSubagentSpec {
                environment: Vec::new(),
                generation: rustx::runtime::identity::RuntimeResourceRevision::new(1),
                skill_roots: Vec::new(),
                selection: rustx::runtime::agent_profile::FrozenAgentSelection::default(),
                agent: rustx::runtime::subagent::SubagentName::parse(agent).unwrap(),
                definition_digest: serde_json::from_value(serde_json::json!(definition_digest))
                    .unwrap(),
                execution_deadline: None,
                workspace_policy: if workspace.is_isolated() {
                    rustx::runtime::workspace::WorkspacePolicy::GitWorktree {
                        require_clean_parent: true,
                    }
                } else {
                    rustx::runtime::workspace::WorkspacePolicy::SharedWorkspace
                },
                instructions: "fixture".into(),
                model: frozen_model(
                    serde_json::from_value(serde_json::json!("local/model")).unwrap(),
                ),
                tools: Vec::new(),
                skills: Vec::new(),
                project_instructions: Vec::new(),
                materialization:
                    rustx::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
                extensions: rustx::extensions::NativeAgentExtensions::with_agent_status(
                    rustx::context::AgentStatusConfig::default(),
                )
                .and_todo(),
            },
            execution_policy: rustx::runtime::subagent::InheritedExecutionPolicy {
                model_timeout: rustx::model::ModelTimeoutPolicy::default(),
                tool_deadline: rustx::tools::deadline::ToolExecutionDeadlinePolicy::default(),
                context: rustx::context::SessionContextPolicy {
                    reserve_tokens: 8192,
                    keep_recent_tokens: 4096,
                    summary_output_cap: Some(1024),
                },
            },
            approval_mode: rustx::runtime::ApprovalMode::Policy,
        };
        return (envelope, authority);
    }
    panic!("fixture requires native Agent ownership")
}

fn frozen_model(model: ModelRef) -> FrozenModelSpec {
    FrozenModelSpec {
        configured: SessionModelConfig::of(model.clone()),
        primary: FrozenModelInvocation {
            binding: FrozenProviderBinding {
                resolved_credential: Some(ResolvedCredential::new("test-only-secret")),
                provider: ProviderId::new("test"),
                base_url: "http://127.0.0.1:9/v1".to_owned(),
                credential: CredentialSource::Environment("RUSTX_TEST_FROZEN_KEY".to_owned()),
            },
            model,
            wire_model: "test-model".into(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            context_window: 128_000,
            model_max_output_tokens: 512,
            max_output_tokens: 512,
            reasoning_profile: None,
            reasoning_enabled: false,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, false),
            declared_capabilities: ModelCapabilities::text_only(true, false),
            compat: ModelCompat::default(),
        },
        summary: FrozenSummaryModel::Session,
    }
}
