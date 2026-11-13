use std::collections::{BTreeMap, BTreeSet};

use super::tests::{catalog, frozen_spec, role_with};
use super::{FrozenAgentComposition, ResolvedSubagentSpec};
use crate::capabilities::selection::AgentToolSelection;
use crate::capabilities::{CapabilitySourceState, ToolSourceId};
use crate::runtime::agent_profile::{AgentProfileAuthority, AgentScope, resolve_agent_profile};
use crate::runtime::identity::McpServerId;
use crate::tools::mcp::{McpServerBinding, McpTransportConfig, source_server_id};
use crate::tools::types::{ToolExecutionPolicy, ToolInvocationPolicy};

fn sources() -> [ToolSourceId; 2] {
    [
        ToolSourceId::Mcp(McpServerId::new("documents")),
        ToolSourceId::ManagedPython("reports".into()),
    ]
}

fn binding(source: &ToolSourceId) -> McpServerBinding {
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
        policy: ToolInvocationPolicy::default(),
        permissions: None,
        tool_permissions: std::collections::BTreeMap::new(),
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
fn resource_only_child_profile_digest_includes_frozen_execution_policy() {
    for source in sources() {
        let foreground = resource_only_spec(&source, binding(&source));
        let mut background_binding = binding(&source);
        background_binding.policy = ToolInvocationPolicy::new(ToolExecutionPolicy::BackgroundOnly);
        let background = resource_only_spec(&source, background_binding);
        assert_ne!(
            foreground.profile_digest(),
            background.profile_digest(),
            "resource execution policy changes the effective profile for {source}"
        );
    }
}

#[test]
fn resource_only_child_profile_digest_excludes_physical_binding_and_tool_scheduling() {
    type BindingMutation = fn(&mut McpServerBinding);
    let mutations: [(&str, BindingMutation); 3] = [
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
    ];
    for source in sources() {
        let original_binding = binding(&source);
        let base = resource_only_spec(&source, original_binding.clone());
        for (field, mutate) in mutations {
            let mut changed_binding = original_binding.clone();
            mutate(&mut changed_binding);
            assert_ne!(changed_binding, original_binding, "{field} really changed");
            let changed = resource_only_spec(&source, changed_binding);
            assert_eq!(
                changed.profile_digest(),
                base.profile_digest(),
                "{field} does not affect resource behavior for {source}"
            );
        }
    }
}

#[test]
fn resource_only_child_profile_digest_survives_frozen_spec_serde_roundtrip() {
    for source in sources() {
        let original = resource_only_spec(&source, binding(&source));
        let encoded = serde_json::to_vec(&original).expect("encode frozen child");
        let restored: ResolvedSubagentSpec =
            serde_json::from_slice(&encoded).expect("decode frozen child");
        assert_eq!(restored, original);
        assert_eq!(restored.profile_digest(), original.profile_digest());
    }
}
