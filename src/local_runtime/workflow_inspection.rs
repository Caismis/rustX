//! Local authoring projection. Resolution and semantic validation are upstream;
//! this module owns presentation only, never runtime composition or publication.
use super::diagnostics::{Diagnostic, Report, Validity};
use super::launch::{HostEnvironment, LaunchRequest};
use crate::runtime::workflow::{
    WorkflowId,
    inspection::{DependencyState, ToolDependency, WorkflowInspection},
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct WorkflowProjection {
    pub id: WorkflowId,
    pub source: PathBuf,
    pub source_layer: &'static str,
    pub discovered: bool,
    pub configured_main_admission: bool,
    pub prospective_main_exposure: Option<bool>,
    pub execution_admission: &'static str,
    pub roles: BTreeMap<crate::runtime::subagent::SubagentName, RoleProjection>,
    pub sources: BTreeMap<
        crate::capabilities::ToolSourceId,
        crate::capabilities::activation::SourceActivation,
    >,
    pub dependencies: Vec<ToolDependency>,
    pub configured_agent_parallel_limit: usize,
    pub program: Option<WorkflowInspection>,
    pub unresolved_runtime_facts: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct RoleProjection {
    pub source: super::agent_resources::AgentSource,
    pub workflow_admitted: bool,
    pub selected_model: String,
    pub configured_timeout_ms: Option<u64>,
    pub tools: Vec<crate::capabilities::selection::ToolSelector>,
    pub workspace_policy: crate::runtime::workspace::WorkspacePolicy,
}

#[allow(clippy::too_many_lines)] // One shared-analysis projection; no semantic validation here.
pub(super) fn inspect(
    id: &WorkflowId,
    explain: bool,
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Report {
    let operation = if explain {
        "workflow_explain"
    } else {
        "workflow_check"
    };
    let (mut report, launch) = super::diagnostics::inspect(operation, request, host);
    report.launch = None;
    let Some(launch) = launch else {
        return report;
    };
    if !launch.trusted {
        report.validity = Validity::Incomplete;
        return report;
    }
    let source = launch
        .locations
        .workspace
        .join(".agents/workflows")
        .join(format!("{id}.yaml"));
    let Some(program) = launch.workflows.get(id) else {
        return Report::failure(
            operation,
            Some(source),
            "workflows",
            "Workflow id is not discovered",
            "create its canonical .agents/workflows/<name>.yaml resource before inspection",
        );
    };
    let inspection = program.inspect();
    let dependencies = launch
        .workflow_dependencies
        .get(id)
        .cloned()
        .expect("prospective metadata covers every compiled Workflow");
    let roles: BTreeMap<_, _> = inspection
        .profiles
        .iter()
        .map(|name| {
            let definition = launch
                .subagents
                .get(name)
                .expect("discovered admitted role was resolved");
            (
                name.clone(),
                RoleProjection {
                    source: launch.role_sources[name].clone(),
                    workflow_admitted: launch.config.subagents.workflow.contains(name),
                    selected_model: definition
                        .model()
                        .unwrap_or(&launch.config.model.model)
                        .to_string(),
                    configured_timeout_ms: definition
                        .execution_deadline()
                        .map(crate::runtime::subagent::SubagentExecutionDeadline::as_millis),
                    tools: definition.tools().to_vec(),
                    workspace_policy: definition.workspace_policy(),
                },
            )
        })
        .collect();
    for role in roles.values() {
        for selector in &role.tools {
            if let Some(source_id) = selector.source() {
                report.validity = Validity::Incomplete;
                let activation = launch
                    .source_activations
                    .get(source_id)
                    .copied()
                    .unwrap_or_default();
                report.diagnostics.push(Diagnostic {
                    classification: "warning",
                    category: if activation == crate::capabilities::activation::SourceActivation::Enabled { "unresolved" } else { "dependency_inert" },
                    file: Some(role.source.selected.clone()), path: format!("tools.sources.{source_id}"),
                    reason: format!("named role capability {selector}: source {activation:?}; online schema compatibility is not established"),
                    correction: "review the canonical role and source policy; child admission must freeze actual capabilities".into(),
                    line: None, column: None,
                });
            }
        }
    }
    // An Agent node's invocation override selects *child* capabilities, not
    // Workflow Tool-node leaves, so it is reported beside the role's own
    // selection rather than folded into the Tool dependency list. An
    // externally sourced selector stays an unresolved online fact: static
    // analysis never invents a schema for it.
    for node in program.agent_override_nodes() {
        let Some(selection) = &node.invocation_override.tools else {
            continue;
        };
        for selector in selection.selectors() {
            if let Some(source_id) = selector.source() {
                report.validity = Validity::Incomplete;
                let activation = launch
                    .source_activations
                    .get(source_id)
                    .copied()
                    .unwrap_or_default();
                report.diagnostics.push(Diagnostic {
                    classification: "warning",
                    category: if activation
                        == crate::capabilities::activation::SourceActivation::Enabled
                    {
                        "unresolved"
                    } else {
                        "dependency_inert"
                    },
                    file: Some(source.clone()),
                    path: format!("{}.tools.sources.{source_id}", node.path),
                    reason: format!(
                        "Agent override capability {selector}: source {activation:?}; online \
                         schema compatibility is not established"
                    ),
                    correction: "review the node override and source policy; child admission \
                                 must freeze actual capabilities"
                        .into(),
                    line: None,
                    column: None,
                });
            }
        }
    }
    for dependency in &dependencies {
        let (category, reason) = match dependency.state {
            DependencyState::Known => continue,
            DependencyState::Unresolved => (
                "unresolved",
                "Tool identity and schema require real online source discovery; compatibility is not established",
            ),
            DependencyState::Inert { .. } => (
                "dependency_inert",
                "Tool source is disabled, unconfigured, or untrusted; inspection does not activate it",
            ),
            DependencyState::Unavailable => {
                ("dependency_unavailable", "Tool source is known unavailable")
            }
        };
        report.validity = Validity::Incomplete;
        for path in &dependency.paths {
            report.diagnostics.push(Diagnostic {
                classification: "warning", category, file: Some(source.clone()),
                path: path.clone(), reason: reason.into(),
                correction: "review the declared source policy; runtime admission must resolve and freeze the actual capability".into(),
                line: None, column: None,
            });
        }
    }
    let mut runtime_requirements = inspection.runtime_requirements.clone();
    for role in roles.values() {
        if role.workspace_policy.is_isolated() {
            runtime_requirements.insert("named_role_workspace_policy_at_admission");
        }
        if !role.tools.is_empty() {
            runtime_requirements.insert("named_role_tool_invocation_and_source_availability");
        }
    }
    if !program.agent_override_nodes().is_empty() {
        runtime_requirements.insert("agent_override_reference_resolution_and_source_availability");
    }
    report.workflow = Some(WorkflowProjection {
        id: id.clone(),
        source,
        source_layer: "trusted_project",
        discovered: true,
        configured_main_admission: launch.workflows.main().contains(id),
        prospective_main_exposure: launch
            .selected_tools
            .as_ref()
            .map(|names| names.iter().any(|name| name == id.as_str())),
        execution_admission: "not_performed_requires_invoking_attempt_frozen_resources",
        roles,
        sources: launch.source_activations,
        dependencies,
        configured_agent_parallel_limit: launch.config.subagents.max_concurrent,
        program: explain.then_some(inspection),
        unresolved_runtime_facts: runtime_requirements.into_iter().collect(),
    });
    report
}
