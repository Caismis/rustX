//! One Agent semantic model and resolution boundary. Selection never grants
//! discovery, source activation, host policy, or physical launch authority.
use std::collections::{BTreeMap, BTreeSet};

use crate::capabilities::selection::{AgentToolSelection, ToolSelectionError};
use crate::capabilities::{AvailableToolCatalog, CapabilityAvailability};
use crate::extensions::NativeAgentExtensions;
use crate::model::session::SessionModelConfig;
use crate::runtime::resources::ProjectContextFile;
use crate::runtime::subagent::{
    SubagentExecutionDeadline, SubagentName, SubagentProjectInstructionPolicy,
};
use crate::runtime::workflow::WorkflowId;
use crate::runtime::workspace::WorkspacePolicy;
use crate::skills::SkillSnapshot;
use crate::tools::types::{ToolDefinition, ToolOrigin};

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
    pub project_instructions: SubagentProjectInstructionPolicy,
    pub workspace_policy: WorkspacePolicy,
}
impl AgentProfile {
    /// Lower strict authoring with project files already loaded by their trust owner.
    pub fn from_document(
        document: &crate::local_runtime::config::AgentProfileDocument,
        files: Vec<ProjectContextFile>,
    ) -> Result<Self, String> {
        document.tools.validate_spelling()?;
        if document.skills.iter().any(|name| name.trim().is_empty()) {
            return Err("Skill identity must be non-empty".into());
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
            project_instructions: SubagentProjectInstructionPolicy {
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProfileDiagnostic {
    Tool(ToolSelectionError),
    SkillUnavailable { name: String },
    AgentUnavailable { name: SubagentName },
    WorkflowUnavailable { id: WorkflowId },
    ScopeUnsupported { capability: String },
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
            Self::ScopeUnsupported { capability } => (6, capability.clone()),
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
    pub project_instructions: SubagentProjectInstructionPolicy,
    pub workspace_policy: WorkspacePolicy,
    pub diagnostics: Vec<AgentProfileDiagnostic>,
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
                .map(|entry| &entry.definition),
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
                capability: format!("agent:{name}"),
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
                capability: format!("workflow:{id}"),
            });
        } else {
            workflows.insert(id.clone());
        }
    }
    let mut extensions = profile.extensions.clone();
    if authority.scope == AgentScope::OneShotChild {
        if crate::extensions::unsupported_child_scope(&extensions).is_some() {
            diagnostics.push(AgentProfileDiagnostic::ScopeUnsupported {
                capability: "extension:goal".into(),
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
                    capability: format!("builtin:{}", tool.name),
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
