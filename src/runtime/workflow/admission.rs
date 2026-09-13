//! Static dependency admission against one off-side resource generation.
use super::{
    Arc, BTreeMap, MAX_WORKFLOW_NODES, WorkflowAdmission, WorkflowAdmissionDiagnostic,
    WorkflowBlockProgram, WorkflowCatalog, WorkflowDependencyFailure, WorkflowNodeProgram,
    bound_workflow_diagnostic, tool,
};
use crate::runtime::agent_profile::{
    AgentProfileAuthority, AgentProfileDiagnostic, AgentScope, resolve_agent_profile,
};

impl WorkflowCatalog {
    /// Resolve every required static dependency before publication. A failed node
    /// leaves only diagnostics, never an executable subset of the source.
    pub(crate) fn admit(
        &mut self,
        available: &crate::capabilities::AvailableToolCatalog,
        availability: &crate::capabilities::CapabilityAvailability,
        skills: &crate::skills::SkillSnapshot,
        agents: &crate::runtime::subagent::AgentCatalog,
        servers: &crate::tools::mcp::McpServerBindings,
    ) {
        self.admit_with_leaf(
            available,
            availability,
            skills,
            agents,
            servers,
            &|definition| {
                available
                    .registration(definition)
                    .is_ok_and(|registration| {
                        registration.foreground() == crate::tools::deadline::ForegroundPolicy::Leaf
                    })
            },
        );
    }

    /// Offline candidate admission uses native leaf metadata and the exact same
    /// dependency traversal. External sources remain Unprepared. No executors
    /// or source owners are needed or called.
    pub(crate) fn admit_metadata(
        &mut self,
        available: &crate::capabilities::AvailableToolCatalog,
        availability: &crate::capabilities::CapabilityAvailability,
        skills: &crate::skills::SkillSnapshot,
        agents: &crate::runtime::subagent::AgentCatalog,
        native_leaves: &std::collections::BTreeSet<crate::runtime::identity::ToolId>,
    ) {
        self.admit_with_leaf(
            available,
            availability,
            skills,
            agents,
            &BTreeMap::default(),
            &|definition| native_leaves.contains(&definition.id),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn admit_with_leaf(
        &mut self,
        available: &crate::capabilities::AvailableToolCatalog,
        availability: &crate::capabilities::CapabilityAvailability,
        skills: &crate::skills::SkillSnapshot,
        agents: &crate::runtime::subagent::AgentCatalog,
        servers: &crate::tools::mcp::McpServerBindings,
        leaf: &dyn Fn(&crate::tools::types::ToolDefinition) -> bool,
    ) {
        let names = agents.names().into_iter().cloned().collect();
        // One-shot children cannot invoke Workflows. Supplying discovered ids
        // lets the shared resolver report scope, without a publication cycle.
        let workflows = self.entries.keys().cloned().collect();
        let authority = AgentProfileAuthority {
            tools: available,
            availability,
            skills,
            agents: &names,
            workflows: &workflows,
            scope: AgentScope::OneShotChild,
        };
        for entry in self.entries.values_mut() {
            let mut program = (*entry.source).clone();
            let mut diagnostics = Vec::new();
            admit_block(
                &mut program.block,
                "block",
                agents,
                &authority,
                &mut program.frozen_tools,
                &mut diagnostics,
                servers,
                leaf,
            );
            entry.admission = if diagnostics.is_empty() {
                WorkflowAdmission::Enabled(Arc::new(program))
            } else {
                // Traversal and the shared resolver both use canonical order.
                // The graph and profile bounds cap work; retained output is
                // additionally capped independently of the number of failures.
                diagnostics.truncate(MAX_WORKFLOW_NODES);
                WorkflowAdmission::Disabled(diagnostics)
            };
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn admit_block(
    block: &mut WorkflowBlockProgram,
    path: &str,
    agents: &crate::runtime::subagent::AgentCatalog,
    authority: &AgentProfileAuthority<'_>,
    tools: &mut BTreeMap<
        crate::capabilities::selection::ExactToolSelector,
        crate::tools::types::ToolDefinition,
    >,
    diagnostics: &mut Vec<WorkflowAdmissionDiagnostic>,
    servers: &crate::tools::mcp::McpServerBindings,
    leaf: &dyn Fn(&crate::tools::types::ToolDefinition) -> bool,
) {
    for (id, node) in &mut block.nodes {
        let path = format!("{path}.nodes.{id}");
        match node {
            WorkflowNodeProgram::Tool { selector, .. } => {
                let result = crate::capabilities::selection::resolve_selector(
                    selector,
                    authority.tools,
                    authority.availability,
                )
                .map_err(WorkflowDependencyFailure::Tool)
                .and_then(|definition| {
                    if tool::eligible(definition) && leaf(definition) {
                        Ok(definition.clone())
                    } else {
                        Err(WorkflowDependencyFailure::IneligibleTool(selector.clone()))
                    }
                });
                match result {
                    Ok(definition) => {
                        tools.insert(selector.clone(), definition);
                    }
                    Err(reason) => diagnostics.push(WorkflowAdmissionDiagnostic {
                        path: format!("{path}.selector"),
                        reason,
                    }),
                }
            }
            WorkflowNodeProgram::Agent(agent) => {
                if let Some(definition) = agents.get(&agent.profile) {
                    let effective = agent
                        .invocation_override
                        .clone()
                        .unwrap_or_default()
                        .effective_profile(definition);
                    let resolved = resolve_agent_profile(&effective, authority);
                    diagnostics.extend(resolved.diagnostics.iter().cloned().map(|reason| {
                        WorkflowAdmissionDiagnostic {
                            path: path.clone(),
                            reason: WorkflowDependencyFailure::Agent(reason),
                        }
                    }));
                    if resolved.diagnostics.is_empty() {
                        match crate::runtime::subagent::resolver::FrozenAgentComposition::freeze(
                            definition,
                            &resolved,
                            authority.skills,
                            servers,
                        ) {
                            Ok(frozen) => agent.resolved = Some(Arc::new(frozen)),
                            Err(error) => diagnostics.push(WorkflowAdmissionDiagnostic {
                                path: path.clone(),
                                reason: WorkflowDependencyFailure::Materialization {
                                    detail: bound_workflow_diagnostic(error.to_string()),
                                },
                            }),
                        }
                    }
                } else {
                    diagnostics.push(WorkflowAdmissionDiagnostic {
                        path: format!("{path}.profile"),
                        reason: WorkflowDependencyFailure::Agent(
                            AgentProfileDiagnostic::AgentUnavailable {
                                name: agent.profile.clone(),
                            },
                        ),
                    });
                }
            }
            WorkflowNodeProgram::Loop { body, .. } => admit_block(
                body,
                &format!("{path}.body"),
                agents,
                authority,
                tools,
                diagnostics,
                servers,
                leaf,
            ),
            WorkflowNodeProgram::Parallel { branches, .. } => {
                for (key, branch) in branches {
                    admit_block(
                        &mut branch.block,
                        &format!("{path}.branches.{key}.block"),
                        agents,
                        authority,
                        tools,
                        diagnostics,
                        servers,
                        leaf,
                    );
                }
            }
            _ => {}
        }
    }
}
