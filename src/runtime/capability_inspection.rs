//! Immutable, content-free projection of generation owners' native facts.
//!
//! Construction copies decisions; it cannot select a capability, prepare a
//! source, or admit a Workflow. No provider configuration or authored prose
//! enters this representation.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::capabilities::{CapabilityAvailability, CapabilitySourceState, ToolSourceId};
use crate::runtime::agent_profile::{AgentProfileDiagnostic, ResolvedAgentProfile};
use crate::runtime::identity::ToolId;
use crate::runtime::subagent::SubagentName;
use crate::runtime::workflow::{
    WorkflowAdmission, WorkflowAdmissionDiagnostic, WorkflowCatalog, WorkflowId,
};
use crate::skills::{SkillDiagnostic, SkillProvenance, SkillSnapshot};
use crate::tools::types::ToolOrigin;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityInspection {
    pub main: Option<AgentInspection>,
    pub agents: BTreeMap<SubagentName, AgentInspection>,
    pub workflows: BTreeMap<WorkflowId, WorkflowInspection>,
    pub sources: BTreeMap<ToolSourceId, SourceInspection>,
    pub skills: Vec<SkillProvenance>,
    pub skill_diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInspection {
    pub identity: AgentIdentity,
    pub source: Option<std::path::PathBuf>,
    pub tools: Vec<ToolInspection>,
    pub tool_selection: Vec<crate::capabilities::selection::AgentToolSelection>,
    pub skills: Vec<SkillProvenance>,
    pub disabled_skills: Vec<String>,
    pub agents: Vec<SubagentName>,
    pub workflows: Vec<WorkflowId>,
    pub extensions: Vec<ExtensionInspection>,
    pub diagnostics: Vec<AgentProfileDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum AgentIdentity {
    Main,
    Named(SubagentName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInspection {
    pub id: ToolId,
    pub name: String,
    pub origin: ToolOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeExtension {
    AgentStatus,
    Todo,
    Goal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInspection {
    pub identity: NativeExtension,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "diagnostics", rename_all = "snake_case")]
pub enum WorkflowInspection {
    Enabled,
    Disabled(Vec<WorkflowAdmissionDiagnostic>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SourceInspection {
    Inactive {
        activation: crate::capabilities::activation::SourceActivation,
    },
    Unprepared,
    Ready,
    Unavailable,
}

impl AgentInspection {
    pub(crate) fn from_resolved(profile: &ResolvedAgentProfile, skills: &SkillSnapshot) -> Self {
        Self {
            tool_selection: profile.tool_selection.clone(),
            identity: AgentIdentity::Main,
            source: None,
            tools: profile
                .tools
                .iter()
                .map(|tool| ToolInspection {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    origin: tool.origin.clone(),
                })
                .collect(),
            skills: skills
                .provenance()
                .iter()
                .filter(|entry| profile.skills.contains(&entry.name))
                .cloned()
                .collect(),
            disabled_skills: profile.disabled_skills.clone(),
            agents: profile.agents.iter().cloned().collect(),
            workflows: profile.workflows.iter().cloned().collect(),
            extensions: vec![
                ExtensionInspection {
                    identity: NativeExtension::AgentStatus,
                    active: profile.extensions.agent_status().is_some(),
                },
                ExtensionInspection {
                    identity: NativeExtension::Todo,
                    active: profile.extensions.todo().is_some(),
                },
                ExtensionInspection {
                    identity: NativeExtension::Goal,
                    active: profile.extensions.goal().is_some(),
                },
            ],
            diagnostics: profile
                .diagnostics
                .iter()
                .map(AgentProfileDiagnostic::redacted)
                .collect(),
        }
    }
}

impl CapabilityInspection {
    pub(crate) fn collect<'a>(
        main: Option<&ResolvedAgentProfile>,
        agents: impl Iterator<
            Item = (
                &'a SubagentName,
                &'a ResolvedAgentProfile,
                &'a std::path::Path,
            ),
        >,
        workflows: &WorkflowCatalog,
        availability: &CapabilityAvailability,
        skills: &SkillSnapshot,
    ) -> Self {
        Self {
            main: main.map(|profile| AgentInspection::from_resolved(profile, skills)),
            agents: agents
                .map(|(name, profile, source)| {
                    let mut projection = AgentInspection::from_resolved(profile, skills);
                    projection.identity = AgentIdentity::Named(name.clone());
                    projection.source = Some(source.to_path_buf());
                    (name.clone(), projection)
                })
                .collect(),
            workflows: workflows
                .entries()
                .iter()
                .map(|(id, entry)| {
                    (
                        id.clone(),
                        match &entry.admission {
                            WorkflowAdmission::Enabled(_) => WorkflowInspection::Enabled,
                            WorkflowAdmission::Disabled(reasons) => WorkflowInspection::Disabled(
                                reasons
                                    .iter()
                                    .map(WorkflowAdmissionDiagnostic::redacted)
                                    .collect(),
                            ),
                        },
                    )
                })
                .collect(),
            sources: availability
                .iter()
                .map(|(id, state)| {
                    (
                        id.clone(),
                        match state {
                            CapabilitySourceState::Inactive { activation } => {
                                SourceInspection::Inactive {
                                    activation: *activation,
                                }
                            }
                            CapabilitySourceState::Unprepared => SourceInspection::Unprepared,
                            CapabilitySourceState::Ready => SourceInspection::Ready,
                            CapabilitySourceState::Unavailable { .. } => {
                                SourceInspection::Unavailable
                            }
                        },
                    )
                })
                .collect(),
            skills: skills.provenance().to_vec(),
            skill_diagnostics: skills
                .diagnostics()
                .iter()
                .map(SkillDiagnostic::redacted)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::selection::{SourceResolutionFailure, ToolSelectionError};
    use crate::runtime::agent_profile::{
        AgentProfile, AgentProfileAuthority, AgentProfileKind, AgentScope, resolve_agent_profile,
    };
    use crate::runtime::identity::McpServerId;

    #[test]
    fn cfg275_wire_fixture_preserves_native_tags_and_order() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/runtime-client/capabilities-v33.json"
        ))
        .unwrap();
        let inspection: CapabilityInspection = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&inspection).unwrap(), value);
        assert!(matches!(
            inspection.skill_diagnostics[0],
            SkillDiagnostic::DuplicateIdentity { .. }
        ));
        assert!(matches!(
            inspection.workflows.values().next().unwrap(),
            WorkflowInspection::Disabled(_)
        ));
    }

    #[test]
    fn cfg275_online_reason_and_redaction_are_owned_before_inspection() {
        let source = ToolSourceId::Mcp(McpServerId::new("optional"));
        let document = crate::toml_authoring::parse::<crate::local_runtime::config::AgentProfileDocument>(
            b"description = 'SECRET_DESCRIPTION'\ninstructions = 'SECRET_INSTRUCTIONS'\n[tools.sources]\noptional = ['inspect']\n",
        ).unwrap();
        let profile =
            AgentProfile::from_document(&document, AgentProfileKind::Named, Vec::new()).unwrap();
        let tools = crate::capabilities::AvailableToolCatalog::metadata([]);
        let skills = SkillSnapshot::new(Vec::new());
        let resolve = |state| {
            let availability = BTreeMap::from([(source.clone(), state)]);
            let resolved = resolve_agent_profile(
                &profile,
                &AgentProfileAuthority {
                    tools: &tools,
                    availability: &availability,
                    skills: &skills,
                    agents: &std::collections::BTreeSet::new(),
                    workflows: &std::collections::BTreeSet::new(),
                    scope: AgentScope::OneShotChild,
                },
            );
            CapabilityInspection::collect(
                Some(&resolved),
                std::iter::empty(),
                &WorkflowCatalog::empty(),
                &availability,
                &skills,
            )
        };
        let unavailable = resolve(CapabilitySourceState::unavailable(
            "SECRET_EXTERNAL_FAILURE",
        ));
        let missing = resolve(CapabilitySourceState::Ready);
        assert!(matches!(
            unavailable.main.as_ref().unwrap().diagnostics[0],
            AgentProfileDiagnostic::Tool(ToolSelectionError::SourceUnavailable {
                reason: SourceResolutionFailure::Unavailable { .. },
                ..
            })
        ));
        assert!(matches!(
            missing.main.as_ref().unwrap().diagnostics[0],
            AgentProfileDiagnostic::Tool(ToolSelectionError::ExactToolAbsent { .. })
        ));
        let wire = serde_json::to_string(&unavailable).unwrap();
        assert!(!wire.contains("SECRET"));
        assert!(!format!("{unavailable:?}").contains("SECRET"));
        assert_eq!(
            serde_json::to_value(serde_json::from_str::<CapabilityInspection>(&wire).unwrap())
                .unwrap(),
            serde_json::to_value(&unavailable).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&missing).unwrap(),
            serde_json::to_value(resolve(CapabilitySourceState::Ready)).unwrap()
        );
    }
}
