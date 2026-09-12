//! One Agent semantic model and resolution boundary. Selection never grants
//! discovery, source activation, host policy, or physical launch authority.
use std::collections::{BTreeMap, BTreeSet};

use crate::capabilities::selection::{AgentToolSelection, ToolSelectionError};
use crate::capabilities::{AvailableToolCatalog, CapabilityAvailability};
use crate::extensions::NativeAgentExtensions;
use crate::model::session::SessionModelConfig;
use crate::runtime::resources::ProjectContextFile;
use crate::runtime::subagent::{SubagentExecutionDeadline, SubagentName};
use crate::runtime::workflow::WorkflowId;
use crate::runtime::workspace::WorkspacePolicy;
use crate::skills::SkillSnapshot;
use crate::tools::types::{ToolDefinition, ToolOrigin};

/// The project-instruction policy of one Agent Profile.
///
/// Resource composition owns discovery; this policy decides only
/// how the generation's already-discovered chain composes with the
/// profile's own explicit files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProjectInstructionPolicy {
    /// Whether the invoking generation's normal project instruction chain is
    /// prepended to the explicit files.
    pub inherit: bool,
    /// The explicit profile-owned project instruction resources, already
    /// loaded by resource composition, in configured order.
    pub files: Vec<ProjectContextFile>,
}

/// Complete typed selected intent, independent of root versus named ownership.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentProfile {
    pub description: String,
    pub instructions: String,
    pub model: Option<SessionModelConfig>,
    pub execution_deadline: Option<SubagentExecutionDeadline>,
    pub tools: Vec<AgentToolSelection>,
    pub skills: Vec<String>,
    pub extensions: NativeAgentExtensions,
    pub agents: BTreeSet<SubagentName>,
    pub workflows: BTreeSet<WorkflowId>,
    pub project_instructions: AgentProjectInstructionPolicy,
    pub workspace_policy: WorkspacePolicy,
}
impl AgentProfile {
    /// Lower strict authoring with project files already loaded by their trust owner.
    ///
    /// # Errors
    /// Rejects malformed selections, invalid deadlines and native text bounds.
    pub fn from_document(
        document: &crate::local_runtime::config::AgentProfileDocument,
        files: Vec<ProjectContextFile>,
    ) -> Result<Self, String> {
        document.tools.validate_spelling()?;
        for name in &document.skills {
            crate::skills::package::validate_skill_name(name)?;
        }
        if document.description.len()
            > crate::runtime::subagent::catalog::MAX_SUBAGENT_DESCRIPTION_BYTES
            || document.instructions.len()
                > crate::runtime::subagent::catalog::MAX_SUBAGENT_INSTRUCTIONS_BYTES
        {
            return Err("Agent description or instructions exceeds its native bound".into());
        }
        Ok(Self {
            description: document.description.clone(),
            instructions: document.instructions.clone(),
            model: document.model.clone(),
            execution_deadline: document.execution_deadline()?,
            tools: document.tools.selectors(),
            skills: document.skills.clone(),
            extensions: document.extensions.resolve(),
            agents: document.agents.iter().cloned().collect(),
            workflows: document.workflows.iter().cloned().collect(),
            project_instructions: AgentProjectInstructionPolicy {
                inherit: document.agents_md.inherit,
                files,
            },
            workspace_policy: document.worktree.to_policy(),
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentScope {
    #[default]
    Root,
    OneShotChild,
}

/// Only immutable admitted facts enter resolution. No mutable source owners.
pub struct AgentProfileAuthority<'a> {
    pub tools: &'a AvailableToolCatalog,
    pub availability: &'a CapabilityAvailability,
    pub skills: &'a SkillSnapshot,
    pub agents: &'a BTreeSet<SubagentName>,
    pub workflows: &'a BTreeSet<WorkflowId>,
    pub scope: AgentScope,
}

/// Typed native facts retained with the generation, never emitted per model turn.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum AgentProfileDiagnostic {
    Tool(ToolSelectionError),
    SkillUnavailable { name: String },
    AgentUnavailable { name: SubagentName },
    WorkflowUnavailable { id: WorkflowId },
    ScopeUnsupported { capability: ScopeCapability },
}
/// Closed scope-ineligible capability identities, requiring no string parsing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "identity", rename_all = "snake_case")]
pub enum ScopeCapability {
    Agent(SubagentName),
    Workflow(WorkflowId),
    Goal,
    BuiltinTool(crate::runtime::identity::ToolId),
}
impl ScopeCapability {
    fn key(&self) -> String {
        match self {
            Self::Agent(name) => format!("agent:{name}"),
            Self::Workflow(id) => format!("workflow:{id}"),
            Self::Goal => "extension:goal".into(),
            Self::BuiltinTool(id) => format!("builtin:{id}"),
        }
    }
}
impl AgentProfileDiagnostic {
    fn key(&self) -> (u8, String) {
        match self {
            Self::Tool(ToolSelectionError::UnknownCapability { selector }) => (0, selector.clone()),
            Self::Tool(ToolSelectionError::SourceUnavailable { selector, .. }) => {
                (1, selector.clone())
            }
            Self::Tool(ToolSelectionError::ExactToolAbsent { source, name }) => {
                (2, format!("{source}/{name}"))
            }
            Self::SkillUnavailable { name } => (3, name.clone()),
            Self::AgentUnavailable { name } => (4, name.to_string()),
            Self::WorkflowUnavailable { id } => (5, id.to_string()),
            Self::ScopeUnsupported { capability } => (6, capability.key()),
        }
    }
}

/// Finite semantic decision. Source bindings, worktree acquisition and frozen
/// child model invocation remain owned by `ResolvedSubagentSpec`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAgentProfile {
    pub description: String,
    pub instructions: String,
    pub model: Option<SessionModelConfig>,
    pub execution_deadline: Option<SubagentExecutionDeadline>,
    pub tools: Vec<ToolDefinition>,
    pub skills: Vec<String>,
    pub extensions: NativeAgentExtensions,
    pub agents: BTreeSet<SubagentName>,
    pub workflows: BTreeSet<WorkflowId>,
    pub project_instructions: AgentProjectInstructionPolicy,
    pub workspace_policy: WorkspacePolicy,
    pub diagnostics: Vec<AgentProfileDiagnostic>,
}

pub(crate) fn is_dispatcher(definition: &ToolDefinition, workflows: &BTreeSet<WorkflowId>) -> bool {
    definition.origin == ToolOrigin::Builtin
        && (definition.id == crate::tools::native::subagent_tool_id()
            || workflows
                .iter()
                .any(|id| definition.id == crate::tools::native::workflow_tool_id(id)))
}

/// Well-formed but unavailable selection warns and suppresses. Dynamic
/// invocation authorization must check its requested replacement separately.
#[must_use]
pub fn resolve_agent_profile(
    profile: &AgentProfile,
    authority: &AgentProfileAuthority<'_>,
) -> ResolvedAgentProfile {
    let mut diagnostics = Vec::new();
    let mut tools = BTreeMap::new();
    for selector in &profile.tools {
        match crate::capabilities::selection::project(
            selector,
            authority
                .tools
                .tools()
                .iter()
                .map(|entry| &entry.definition)
                .filter(|definition| !is_dispatcher(definition, authority.workflows)),
            authority.availability,
        ) {
            Ok(selected) => {
                for tool in selected {
                    tools.insert(tool.id.clone(), tool.clone());
                }
            }
            Err(error) => diagnostics.push(AgentProfileDiagnostic::Tool(error)),
        }
    }
    let mut skills = BTreeSet::new();
    for name in &profile.skills {
        if authority
            .skills
            .catalog_entries()
            .iter()
            .any(|entry| entry.name == *name)
        {
            skills.insert(name.clone());
        } else {
            diagnostics.push(AgentProfileDiagnostic::SkillUnavailable { name: name.clone() });
        }
    }
    let mut agents = BTreeSet::new();
    for name in &profile.agents {
        if !authority.agents.contains(name) {
            diagnostics.push(AgentProfileDiagnostic::AgentUnavailable { name: name.clone() });
        } else if authority.scope == AgentScope::OneShotChild {
            diagnostics.push(AgentProfileDiagnostic::ScopeUnsupported {
                capability: ScopeCapability::Agent(name.clone()),
            });
        } else {
            agents.insert(name.clone());
        }
    }
    let mut workflows = BTreeSet::new();
    for id in &profile.workflows {
        if !authority.workflows.contains(id) {
            diagnostics.push(AgentProfileDiagnostic::WorkflowUnavailable { id: id.clone() });
        } else if authority.scope == AgentScope::OneShotChild {
            diagnostics.push(AgentProfileDiagnostic::ScopeUnsupported {
                capability: ScopeCapability::Workflow(id.clone()),
            });
        } else {
            workflows.insert(id.clone());
        }
    }
    let mut extensions = profile.extensions.clone();
    if authority.scope == AgentScope::OneShotChild {
        if crate::extensions::unsupported_child_scope(&extensions).is_some() {
            diagnostics.push(AgentProfileDiagnostic::ScopeUnsupported {
                capability: ScopeCapability::Goal,
            });
            extensions = extensions.without_goal();
        }
        tools.retain(|_, tool| {
            let unsupported = tool.origin == ToolOrigin::Builtin
                && (crate::runtime::subagent::catalog::CHILD_UNSAFE_BUILTIN_TOOLS
                    .contains(&tool.name.as_str())
                    || tool.name == crate::tools::native::SUBAGENT_TOOL_NAME);
            if unsupported {
                diagnostics.push(AgentProfileDiagnostic::ScopeUnsupported {
                    capability: ScopeCapability::BuiltinTool(tool.id.clone()),
                });
            }
            !unsupported
        });
    }
    diagnostics.sort_by_key(AgentProfileDiagnostic::key);
    diagnostics.dedup();
    ResolvedAgentProfile {
        description: profile.description.clone(),
        instructions: profile.instructions.clone(),
        model: profile.model.clone(),
        execution_deadline: profile.execution_deadline,
        tools: tools.into_values().collect(),
        skills: skills.into_iter().collect(),
        extensions,
        agents,
        workflows,
        project_instructions: profile.project_instructions.clone(),
        workspace_policy: profile.workspace_policy,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::selection::{SourceResolutionFailure, ToolSelectionDocument};
    use crate::capabilities::{CapabilitySourceState, ToolSourceId};
    use crate::local_runtime::config::AgentProfileDocument;
    use crate::runtime::identity::{McpServerId, ToolId};

    fn available() -> AvailableToolCatalog {
        AvailableToolCatalog::metadata(
            crate::tools::native::definitions(
                crate::tools::native::NativeToolPolicies::default(),
                &crate::runtime::subagent::AgentCatalog::empty(),
            )
            .into_iter()
            .map(|(definition, _)| definition),
        )
    }
    fn profile(names: &[&str]) -> AgentProfile {
        AgentProfile::from_document(
            &AgentProfileDocument {
                tools: ToolSelectionDocument {
                    builtin: names.iter().map(|name| (*name).into()).collect(),
                    ..Default::default()
                },
                ..Default::default()
            },
            Vec::new(),
        )
        .unwrap()
    }
    fn resolve(
        profile: &AgentProfile,
        tools: &AvailableToolCatalog,
        availability: &CapabilityAvailability,
        scope: AgentScope,
    ) -> ResolvedAgentProfile {
        resolve_agent_profile(
            profile,
            &AgentProfileAuthority {
                tools,
                availability,
                skills: &SkillSnapshot::new(Vec::new()),
                agents: &BTreeSet::from([SubagentName::parse("reviewer").unwrap()]),
                workflows: &BTreeSet::new(),
                scope,
            },
        )
    }
    #[test]
    fn cfg273_root_and_named_share_tools_skills_extensions_and_keep_independent_authority() {
        let tools = available();
        let mut root = profile(&["read"]);
        root.agents.insert(SubagentName::parse("reviewer").unwrap());
        let mut child = profile(&["read", "grep"]);
        child.skills = vec!["missing".into()];
        child.extensions = crate::extensions::NativeAgentExtensionSelection {
            todo: Some(crate::extensions::TodoExtensionDocument::default()),
            ..Default::default()
        }
        .resolve();
        let root = resolve(
            &root,
            &tools,
            &CapabilityAvailability::new(),
            AgentScope::Root,
        );
        let named = resolve(
            &child,
            &tools,
            &CapabilityAvailability::new(),
            AgentScope::OneShotChild,
        );
        let same_as_root = resolve(
            &child,
            &tools,
            &CapabilityAvailability::new(),
            AgentScope::Root,
        );
        assert_eq!(named, same_as_root);
        assert!(
            root.agents
                .contains(&SubagentName::parse("reviewer").unwrap())
        );
        assert!(named.tools.iter().any(|tool| tool.name == "grep"));
        assert!(!root.tools.iter().any(|tool| tool.name == "grep"));
        assert!(named.skills.is_empty());
        assert_eq!(
            named.diagnostics,
            [AgentProfileDiagnostic::SkillUnavailable {
                name: "missing".into()
            }]
        );
        assert!(named.extensions.todo().is_some());
    }
    #[test]
    fn cfg273_missing_builtin_and_catalog_selections_warn_in_canonical_order() {
        let mut profile = profile(&["read", "missing"]);
        profile.skills = vec!["zeta".into(), "alpha".into()];
        profile.agents = ["zeta", "alpha"]
            .map(|name| SubagentName::parse(name).unwrap())
            .into();
        profile.workflows = ["zeta", "alpha"]
            .map(|name| WorkflowId::parse(name).unwrap())
            .into();
        let first = resolve(
            &profile,
            &available(),
            &CapabilityAvailability::new(),
            AgentScope::Root,
        );
        profile.tools.reverse();
        profile.skills.reverse();
        let second = resolve(
            &profile,
            &available(),
            &CapabilityAvailability::new(),
            AgentScope::Root,
        );
        assert_eq!(first, second);
        assert_eq!(first.tools.len(), 1);
        assert_eq!(first.diagnostics.len(), 7);
        assert!(matches!(
            first.diagnostics[0],
            AgentProfileDiagnostic::Tool(ToolSelectionError::UnknownCapability { .. })
        ));
        assert_eq!(
            first.diagnostics[1],
            AgentProfileDiagnostic::SkillUnavailable {
                name: "alpha".into()
            }
        );
        assert!(first.agents.is_empty());
        assert!(first.workflows.is_empty());
    }
    #[test]
    fn cfg273_source_failures_are_distinct_and_selection_cannot_activate_sources() {
        let source = ToolSourceId::Mcp(McpServerId::new("github"));
        let mut profile = profile(&[]);
        profile.tools.push(AgentToolSelection::Source {
            source_id: source.clone(),
            name: "get_diff".into(),
        });
        let states = [
            (None, SourceResolutionFailure::Undefined),
            (
                Some(CapabilitySourceState::Inactive {
                    activation: crate::capabilities::activation::SourceActivation::Disabled,
                }),
                SourceResolutionFailure::Inactive(
                    crate::capabilities::activation::SourceActivation::Disabled,
                ),
            ),
            (
                Some(CapabilitySourceState::Inactive {
                    activation: crate::capabilities::activation::SourceActivation::Untrusted,
                }),
                SourceResolutionFailure::Inactive(
                    crate::capabilities::activation::SourceActivation::Untrusted,
                ),
            ),
            (
                Some(CapabilitySourceState::Unprepared),
                SourceResolutionFailure::Unprepared,
            ),
            (
                Some(CapabilitySourceState::Unavailable {
                    reason: "offline".into(),
                }),
                SourceResolutionFailure::Unavailable {
                    reason: "offline".into(),
                },
            ),
        ];
        for (state, expected) in states {
            let availability: CapabilityAvailability = state
                .map(|state| (source.clone(), state))
                .into_iter()
                .collect();
            let before = availability.clone();
            let resolved = resolve(&profile, &available(), &availability, AgentScope::Root);
            assert!(resolved.tools.is_empty());
            assert!(
                matches!(&resolved.diagnostics[0], AgentProfileDiagnostic::Tool(ToolSelectionError::SourceUnavailable { reason, .. }) if *reason == expected)
            );
            assert_eq!(availability, before);
        }
        let ready = [(source.clone(), CapabilitySourceState::Ready)].into();
        let missing = resolve(&profile, &available(), &ready, AgentScope::Root);
        assert_eq!(
            missing.diagnostics,
            [AgentProfileDiagnostic::Tool(
                ToolSelectionError::ExactToolAbsent {
                    source,
                    name: "get_diff".into()
                }
            )]
        );
    }
    #[test]
    fn cfg273_scope_ineligible_goal_suppresses_only_the_extension() {
        let mut profile = profile(&["read"]);
        let mut extensions = crate::extensions::NativeAgentExtensionsDocument::default();
        extensions.goal.enabled = true;
        profile.extensions = extensions.resolve();
        let root = resolve(
            &profile,
            &available(),
            &CapabilityAvailability::new(),
            AgentScope::Root,
        );
        let child = resolve(
            &profile,
            &available(),
            &CapabilityAvailability::new(),
            AgentScope::OneShotChild,
        );
        assert!(root.extensions.goal().is_some());
        assert!(child.extensions.goal().is_none());
        assert_eq!(child.tools, root.tools);
        assert_eq!(
            child.diagnostics,
            [AgentProfileDiagnostic::ScopeUnsupported {
                capability: ScopeCapability::Goal
            }]
        );
    }
    #[test]
    fn cfg273_resolved_generation_is_owned_and_preserves_host_tool_policy() {
        let mut definition = available()
            .definitions()
            .into_iter()
            .find(|tool| tool.name == "read")
            .unwrap();
        definition.id = ToolId::new("frozen-read");
        definition.approval_policy = crate::tools::types::ToolApprovalPolicy::Always;
        let first = resolve(
            &profile(&["read"]),
            &AvailableToolCatalog::metadata([definition.clone()]),
            &CapabilityAvailability::new(),
            AgentScope::Root,
        );
        assert_eq!(first.tools, [definition.clone()]);
        definition.description = "later publication".into();
        definition.approval_policy = crate::tools::types::ToolApprovalPolicy::Never;
        let later = resolve(
            &profile(&["read"]),
            &AvailableToolCatalog::metadata([definition]),
            &CapabilityAvailability::new(),
            AgentScope::Root,
        );
        assert_ne!(first.tools, later.tools);
        assert_eq!(
            first.tools[0].approval_policy,
            crate::tools::types::ToolApprovalPolicy::Always
        );
    }
}

#[cfg(test)]
mod composition_tests {
    use super::*;
    #[test]
    fn cfg273_delegation_composes_dispatcher_without_granting_child_tools() {
        let reviewer = SubagentName::parse("reviewer").unwrap();
        let named = crate::local_runtime::agent_resources::parse("description = 'Review'\ninstructions = 'Review code'\n[tools]\nbuiltin = ['read', 'grep']").unwrap();
        let catalog = crate::runtime::subagent::AgentCatalog::new([
            crate::runtime::subagent::NamedAgentDefinition::new(
                reviewer.clone(),
                AgentProfile::from_document(&named, Vec::new()).unwrap(),
                "reviewer.toml".into(),
            )
            .unwrap(),
        ])
        .unwrap();
        let definitions = crate::tools::native::definitions(
            crate::tools::native::NativeToolPolicies::default(),
            &catalog,
        )
        .into_iter()
        .map(|(definition, _)| definition)
        .collect::<Vec<_>>();
        let root = crate::local_runtime::agent_resources::parse(
            "agents = ['reviewer']\n[tools]\nbuiltin = ['read']",
        )
        .unwrap();
        let selected = crate::capabilities::select_definitions(
            &definitions.iter().collect::<Vec<_>>(),
            &crate::capabilities::AgentActivation {
                profile: root,
                admitted_agents: [reviewer].into(),
                ..Default::default()
            },
            &SkillSnapshot::new(Vec::new()),
            &CapabilityAvailability::new(),
        )
        .unwrap();
        assert!(
            selected
                .iter()
                .any(|tool| tool.id == crate::tools::native::subagent_tool_id())
        );
        assert!(selected.iter().any(|tool| tool.name == "read"));
        assert!(!selected.iter().any(|tool| tool.name == "grep"));
        assert_eq!(selected.len(), 2);
        let mut root_registry = crate::tools::executor::ToolRegistry::new();
        crate::tools::native::register_subagent_child_tools(
            &mut root_registry,
            &selected
                .iter()
                .filter(|tool| tool.name == "read")
                .map(|tool| (*tool).clone())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(matches!(
            root_registry.preflight(&crate::tools::types::ToolCall {
                id: crate::runtime::identity::ToolCallId::new("root-cannot-call-child-tool"),
                tool_id: crate::runtime::identity::ToolId::new("tool-grep"),
                name: "grep".into(),
                arguments: serde_json::json!({"pattern": "review"}),
            }),
            Err(crate::tools::executor::ToolPreflightError::UnknownTool { .. })
        ));
    }
    #[test]
    fn cfg273_admitted_skill_selection_is_identical_in_root_and_named_scope() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("review");
        std::fs::create_dir(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: review\ndescription: Review code\n---\nReview carefully.\n",
        )
        .unwrap();
        let workspace = crate::tools::workspace::Workspace::new(directory.path()).unwrap();
        let packages = crate::skills::SkillDiscovery::with_config(
            &workspace,
            crate::skills::SkillDiscoveryConfig {
                automatic_roots: Vec::new(),
                explicit_paths: vec![skill],
            },
        )
        .discover()
        .unwrap();
        let skills = SkillSnapshot::new(packages.into_iter().map(std::sync::Arc::new).collect());
        let document = crate::local_runtime::agent_resources::parse(
            "skills = ['review', 'missing']\n[tools]\nbuiltin = ['read']",
        )
        .unwrap();
        let profile = AgentProfile::from_document(&document, Vec::new()).unwrap();
        let tools = AvailableToolCatalog::metadata(
            crate::tools::native::definitions(
                crate::tools::native::NativeToolPolicies::default(),
                &crate::runtime::subagent::AgentCatalog::empty(),
            )
            .into_iter()
            .map(|(definition, _)| definition),
        );
        let mut authority = AgentProfileAuthority {
            tools: &tools,
            availability: &CapabilityAvailability::new(),
            skills: &skills,
            agents: &BTreeSet::new(),
            workflows: &BTreeSet::new(),
            scope: AgentScope::Root,
        };
        let root = resolve_agent_profile(&profile, &authority);
        authority.scope = AgentScope::OneShotChild;
        let named = resolve_agent_profile(&profile, &authority);
        assert_eq!(root, named);
        assert_eq!(root.skills, ["review"]);
        assert_eq!(
            root.diagnostics,
            [AgentProfileDiagnostic::SkillUnavailable {
                name: "missing".into()
            }]
        );
    }
    #[test]
    fn cfg273_known_builtin_unavailable_in_generation_is_suppressed() {
        let document =
            crate::local_runtime::agent_resources::parse("[tools]\nbuiltin = ['read']").unwrap();
        let profile = AgentProfile::from_document(&document, Vec::new()).unwrap();
        let resolved = resolve_agent_profile(
            &profile,
            &AgentProfileAuthority {
                tools: &AvailableToolCatalog::default(),
                availability: &CapabilityAvailability::new(),
                skills: &SkillSnapshot::new(Vec::new()),
                agents: &BTreeSet::new(),
                workflows: &BTreeSet::new(),
                scope: AgentScope::Root,
            },
        );
        assert!(resolved.tools.is_empty());
        assert_eq!(
            resolved.diagnostics,
            [AgentProfileDiagnostic::Tool(
                ToolSelectionError::UnknownCapability {
                    selector: "builtin:read".into()
                }
            )]
        );
    }
}
