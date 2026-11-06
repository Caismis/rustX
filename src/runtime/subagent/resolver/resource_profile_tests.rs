use std::collections::{BTreeMap, BTreeSet};

use super::tests::{catalog, frozen_spec, role_with};
use super::{FrozenAgentComposition, ResolvedSubagentSpec};
use crate::capabilities::selection::AgentToolSelection;
use crate::capabilities::{CapabilitySourceState, ToolSourceId};
use crate::runtime::agent_profile::{AgentProfileAuthority, AgentScope, resolve_agent_profile};
use crate::runtime::identity::McpServerId;
use crate::tools::mcp::{McpServerBinding, McpTransportConfig, source_server_id};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocationPolicy,
};

fn sources() -> [ToolSourceId; 2] {
    [
        ToolSourceId::Mcp(McpServerId::new("documents")),
        ToolSourceId::ManagedPython("reports".into()),
    ]
}

fn binding(source: &ToolSourceId, approval: ToolApprovalPolicy) -> McpServerBinding {
    McpServerBinding {
        credentials: serde_json::from_value(serde_json::json!({
            "environment": { "API_KEY": "$SOURCE_KEY" }
        }))
        .expect("source credential references"),
        transport: match source {
            ToolSourceId::Mcp(_) => McpTransportConfig::StreamableHttp {
                endpoint: "https://example.invalid/resources".into(),
                headers: BTreeMap::new(),
            },
            ToolSourceId::ManagedPython(_) => McpTransportConfig::Stdio {
                program: "python-resource-server".into(),
                args: Vec::new(),
                cwd: None,
                environment: BTreeMap::new(),
            },
        },
        policy: ToolInvocationPolicy {
            approval,
            ..Default::default()
        },
    }
}

/// Exercise selection and freezing before supplying the unrelated model fixture.
fn resource_only_spec(source: &ToolSourceId, binding: McpServerBinding) -> ResolvedSubagentSpec {
    let available = catalog(Vec::new()).with_resource_sources([source.clone()].into());
    let availability = [(source.clone(), CapabilitySourceState::Ready)].into();
    let skills = crate::skills::SkillSnapshot::new(Vec::new());
    let definition = role_with(
        vec![AgentToolSelection::All {
            source_id: source.clone(),
        }],
        crate::extensions::NativeAgentExtensions::none(),
    );
    let profile = resolve_agent_profile(
        definition.profile(),
        &AgentProfileAuthority {
            tools: &available,
            availability: &availability,
            skills: &skills,
            agents: &BTreeSet::new(),
            workflows: &BTreeSet::new(),
            scope: AgentScope::OneShotChild,
        },
    );
    let configured = [(source_server_id(source), binding)].into();
    let frozen = FrozenAgentComposition::freeze(&definition, &profile, &skills, &configured)
        .expect("freeze");
    let spec = ResolvedSubagentSpec {
        selection: frozen.selection,
        agent: definition.name().clone(),
        definition_digest: definition.digest().clone(),
        tools: frozen.tools,
        resource_sources: frozen.resource_sources,
        skills: frozen.skills,
        materialization: frozen.materialization,
        extensions: frozen.extensions,
        ..frozen_spec()
    };
    assert!(spec.tools.is_empty());
    assert_eq!(spec.resource_sources, [source.clone()].into());
    assert_eq!(spec.materialization.sources.len(), 1);
    spec
}

#[test]
fn resource_only_child_profile_digest_includes_frozen_approval() {
    for source in sources() {
        let never = resource_only_spec(&source, binding(&source, ToolApprovalPolicy::Never));
        let always = resource_only_spec(&source, binding(&source, ToolApprovalPolicy::Always));
        let mut only_approval_changed = never.clone();
        only_approval_changed
            .materialization
            .sources
            .get_mut(&source)
            .unwrap()
            .policy
            .approval = ToolApprovalPolicy::Always;
        assert_eq!(
            only_approval_changed, always,
            "the two real frozen contracts differ only in source approval"
        );
        assert_ne!(
            never.profile_digest(),
            always.profile_digest(),
            "resource approval changes the effective profile for {source} even without Tools"
        );
    }
}

#[test]
fn resource_only_child_profile_digest_excludes_physical_binding_and_tool_scheduling() {
    type BindingMutation = fn(&mut McpServerBinding);
    let mutations: [(&str, BindingMutation); 5] = [
        ("transport", |binding| match &mut binding.transport {
            McpTransportConfig::StreamableHttp { endpoint, .. } => {
                *endpoint = "https://relocated.invalid/resources".into();
            }
            McpTransportConfig::Stdio { program, .. } => {
                *program = "/relocated/python-resource-server".into();
            }
        }),
        ("credential references", |binding| {
            binding.credentials.environment.insert(
                "API_KEY".into(),
                serde_json::from_value(serde_json::json!("$ROTATED_SOURCE_KEY"))
                    .expect("credential reference"),
            );
        }),
        ("captured credentials", |binding| {
            binding
                .credentials
                .capture(crate::credentials::CredentialSnapshot::new([(
                    "SOURCE_KEY".into(),
                    "test-only-rotated-secret".into(),
                )]));
        }),
        ("tool execution mode", |binding| {
            binding.policy.execution = ToolExecutionPolicy::BackgroundOnly;
        }),
        ("tool concurrency", |binding| {
            binding.policy.concurrency = ToolConcurrencyPolicy::Parallel;
        }),
    ];
    for source in sources() {
        for approval in [ToolApprovalPolicy::Never, ToolApprovalPolicy::Always] {
            let original_binding = binding(&source, approval);
            let base = resource_only_spec(&source, original_binding.clone());
            for (field, mutate) in mutations {
                let mut changed_binding = original_binding.clone();
                mutate(&mut changed_binding);
                assert_ne!(changed_binding, original_binding, "{field} really changed");
                let changed = resource_only_spec(&source, changed_binding);
                assert_eq!(
                    changed.profile_digest(),
                    base.profile_digest(),
                    "{field} does not affect resource behavior for {source} with {approval:?} approval"
                );
            }
        }
    }
}

#[test]
fn resource_only_child_profile_digest_survives_frozen_spec_serde_roundtrip() {
    for source in sources() {
        let digests = [ToolApprovalPolicy::Never, ToolApprovalPolicy::Always].map(|approval| {
            let original = resource_only_spec(&source, binding(&source, approval));
            let encoded = serde_json::to_vec(&original).expect("encode frozen child");
            let restored: ResolvedSubagentSpec =
                serde_json::from_slice(&encoded).expect("decode frozen child");
            assert_eq!(restored, original);
            assert_eq!(
                restored.materialization.sources[&source].policy.approval,
                approval
            );
            assert_eq!(restored.profile_digest(), original.profile_digest());
            restored.profile_digest()
        });
        assert_ne!(
            digests[0], digests[1],
            "the child recomputes distinct approval profiles from frozen bytes for {source}"
        );
    }
}
