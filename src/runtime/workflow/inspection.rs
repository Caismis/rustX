//! Bounded, inert projections of the authoritative compiled program.
//! No executor, runtime owner, discovery service, or authority is accepted here.

use super::{
    BTreeMap, BTreeSet, MAX_BLOCK_DEPTH, MAX_LOCAL_BYTES, MAX_LOOP_ITERATIONS,
    MAX_PARALLEL_BRANCHES, MAX_VALUE_BYTES, MAX_WORKFLOW_AGENTS, MAX_WORKFLOW_NODES,
    MAX_WORKFLOW_STEPS, Serialize, SubagentName, Value, WorkflowBlockProgram, WorkflowNodeProgram,
    WorkflowPort, WorkflowPredicate, WorkflowProgram, WorkflowReviewSubject, WorkflowToolIdentity,
    WorkflowValue, WorkflowWorkspace,
};
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct ToolDependency {
    pub selector: crate::capabilities::selection::ExactToolSelector,
    pub paths: Vec<String>,
    pub state: DependencyState,
}

#[derive(Debug)]
pub struct CapabilityError {
    pub workflow: super::WorkflowId,
    pub path: String,
    pub reason: String,
}
impl std::fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Workflow {} {}: {}",
            self.workflow, self.path, self.reason
        )
    }
}

impl WorkflowProgram {
    pub(super) fn selector_paths(
        &self,
        selector: &crate::capabilities::selection::ExactToolSelector,
    ) -> Vec<String> {
        fn walk(
            block: &WorkflowBlockProgram,
            path: &str,
            selector: &crate::capabilities::selection::ExactToolSelector,
            paths: &mut Vec<String>,
        ) {
            for (id, node) in &block.nodes {
                let path = format!("{path}.nodes.{id}");
                match node {
                    WorkflowNodeProgram::Tool {
                        selector: selected, ..
                    } if selected == selector => paths.push(format!("{path}.selector")),
                    WorkflowNodeProgram::Loop { body, .. } => {
                        walk(body, &format!("{path}.body"), selector, paths);
                    }
                    WorkflowNodeProgram::Parallel { branches, .. } => {
                        for (key, branch) in branches {
                            walk(
                                &branch.block,
                                &format!("{path}.branches.{key}.block"),
                                selector,
                                paths,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut paths = Vec::new();
        walk(&self.block, "block", selector, &mut paths);
        if paths.is_empty() {
            paths.push("tools".into());
        }
        paths
    }

    /// Every Agent node that carries a trusted static invocation override,
    /// with its precise authored path.
    ///
    /// The walk descends into Loop bodies and Parallel branches, so a nested
    /// Agent node's override receives the same static validation and the same
    /// precise diagnostic path as a root-level one.
    #[must_use]
    pub fn agent_override_nodes(&self) -> Vec<AgentOverrideNode<'_>> {
        fn walk<'a>(
            block: &'a WorkflowBlockProgram,
            path: &str,
            found: &mut Vec<AgentOverrideNode<'a>>,
        ) {
            for (id, node) in &block.nodes {
                let path = format!("{path}.nodes.{id}");
                match node {
                    WorkflowNodeProgram::Agent(agent) => {
                        if let Some(invocation_override) = &agent.invocation_override {
                            found.push(AgentOverrideNode {
                                path: format!("{path}.override"),
                                profile: &agent.profile,
                                invocation_override,
                            });
                        }
                    }
                    WorkflowNodeProgram::Loop { body, .. } => {
                        walk(body, &format!("{path}.body"), found);
                    }
                    WorkflowNodeProgram::Parallel { branches, .. } => {
                        for (key, branch) in branches {
                            walk(
                                &branch.block,
                                &format!("{path}.branches.{key}.block"),
                                found,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.block, "block", &mut found);
        found
    }
}

/// One Agent node's trusted static invocation override, with the authored
/// path a diagnostic must name.
#[derive(Debug, Clone)]
pub struct AgentOverrideNode<'a> {
    /// The precise authored path of the override, including nesting.
    pub path: String,
    /// The role the override specializes.
    pub profile: &'a SubagentName,
    /// The compiled override itself.
    pub invocation_override: &'a crate::runtime::subagent::SubagentInvocationOverride,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DependencyState {
    /// Real local metadata exists; concrete arguments still require native preparation.
    Known,
    Inert {
        activation: crate::capabilities::activation::SourceActivation,
    },
    Unavailable,
    /// No online schema or capability identity was fabricated.
    Unresolved,
}

/// Static program facts. Literals and task text are deliberately not exported.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowInspection {
    /// Projected once at the local report surface, including selected role policies.
    #[serde(skip)]
    pub runtime_requirements: BTreeSet<&'static str>,
    pub identity: WorkflowToolIdentity,
    pub stage: &'static str,
    pub input_schema: Value,
    pub output_schema: Value,
    pub configured_timeout_ms: u64,
    pub workspace: Option<WorkflowWorkspace>,
    pub candidate_handoff: &'static str,
    pub conservative_steps: usize,
    pub conservative_agent_runs: usize,
    pub conservative_retained_bytes: usize,
    pub caps: BTreeMap<&'static str, usize>,
    pub blocks: BTreeMap<String, BlockInspection>,
    pub tools: BTreeSet<crate::capabilities::selection::ExactToolSelector>,
    pub profiles: BTreeSet<SubagentName>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockInspection {
    pub entry: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub nodes: BTreeMap<String, NodeInspection>,
}

/// The bounded projection of one Agent node's trusted static invocation
/// override.
///
/// Presence is the fact this projection has to carry, so each dimension stays
/// an `Option`: `None` is "this dimension keeps the role's default" and
/// `Some` is "this dimension is replaced by exactly this". Only identities
/// are exported — capability selectors, Skill names, and the composed
/// extension names — never a Skill body, a prompt, a credential, or a
/// materialization secret.
#[derive(Debug, Clone, Serialize)]
pub struct AgentOverrideInspection {
    pub tools: Option<Vec<crate::capabilities::selection::AgentToolSelection>>,
    pub skills: Option<Vec<String>>,
    pub extensions: Option<Vec<&'static str>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeInspection {
    pub kind: &'static str,
    pub edges: Vec<(WorkflowPort, String)>,
    pub bindings: BTreeMap<String, Value>,
    pub profile: Option<SubagentName>,
    /// The node's invocation override, when it carries one.
    pub invocation_override: Option<AgentOverrideInspection>,
    pub selector: Option<crate::capabilities::selection::ExactToolSelector>,
    pub output_schema: Option<Value>,
    pub result_part: Option<usize>,
    pub result_kind: Option<&'static str>,
    pub branches: Vec<String>,
    pub max_iterations: Option<u32>,
    pub review_subject: Option<&'static str>,
}

impl WorkflowProgram {
    /// Project only compiled facts, in authored identity order. Conservative
    /// maxima are reservations, never predictions of decisions or requests.
    #[must_use]
    pub fn inspect(&self) -> WorkflowInspection {
        let mut result = WorkflowInspection {
            runtime_requirements: BTreeSet::from(["invoking_attempt_frozen_admission"]),
            identity: self.tool_identity(),
            stage: "statically_compiled_online_admission_unresolved",
            input_schema: schema(&self.block.input_schema),
            output_schema: schema(&self.block.output_schema),
            configured_timeout_ms: self.timeout_ms,
            workspace: self.workspace,
            candidate_handoff: if self.workspace.is_some() {
                "runtime_candidate_identity_requires_native_settlement"
            } else {
                "no_run_candidate_requested"
            },
            conservative_steps: self.execution_bound,
            conservative_agent_runs: self.agent_bound,
            conservative_retained_bytes: self.retained_bound,
            caps: BTreeMap::from([
                ("steps", MAX_WORKFLOW_STEPS),
                ("agent_runs", MAX_WORKFLOW_AGENTS),
                ("nodes", MAX_WORKFLOW_NODES),
                ("parallel_branches", MAX_PARALLEL_BRANCHES),
                ("loop_iterations", MAX_LOOP_ITERATIONS as usize),
                ("block_depth", MAX_BLOCK_DEPTH),
                ("value_bytes", MAX_VALUE_BYTES),
                ("retained_bytes", MAX_LOCAL_BYTES),
                ("program_bytes", super::MAX_WORKFLOW_BYTES),
                ("value_depth", super::MAX_VALUE_DEPTH),
                ("reference_components", super::MAX_REFERENCE_COMPONENTS),
            ]),
            blocks: BTreeMap::new(),
            tools: self.tools.clone(),
            profiles: BTreeSet::new(),
        };
        if self.workspace.is_some() {
            result
                .runtime_requirements
                .insert("workspace_candidate_acquisition_and_identity");
        }
        block(&self.block, "block", &mut result);
        result
    }
}

// Schema structure is real; authored constant/enum contents and annotations can
// contain secrets. The report explicitly identifies these redactions.
fn schema(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        if matches!(
                            key.as_str(),
                            "const" | "enum" | "description" | "title" | "default" | "examples"
                        ) {
                            json!({"redacted": true})
                        } else if key == "properties" {
                            Value::Object(
                                value
                                    .as_object()
                                    .expect("compiled properties")
                                    .iter()
                                    .map(|(name, property)| (name.clone(), schema(property)))
                                    .collect(),
                            )
                        } else if key == "items" {
                            schema(value)
                        } else {
                            value.clone()
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(schema).collect()),
        value => value.clone(),
    }
}

fn binding(value: &WorkflowValue) -> Value {
    match value {
        WorkflowValue::Reference { path } => json!({"type":"reference", "path":path}),
        WorkflowValue::Literal { value } => {
            json!({"type":"literal", "redacted":true, "value_type": match value {
                Value::Null => "null", Value::Bool(_) => "boolean", Value::Number(_) => "number",
                Value::String(_) => "string", Value::Array(_) => "array", Value::Object(_) => "object",
            }})
        }
        WorkflowValue::Object { fields } => {
            json!({"type":"object", "fields":fields.iter().map(|(k,v)| (k, binding(v))).collect::<BTreeMap<_,_>>()})
        }
        WorkflowValue::Array { items } => {
            json!({"type":"array", "items":items.iter().map(binding).collect::<Vec<_>>()})
        }
    }
}

fn predicate(value: &WorkflowPredicate) -> Value {
    match value {
        WorkflowPredicate::Boolean { value } => json!({"type":"boolean", "value":binding(value)}),
        WorkflowPredicate::Equal { left, right } => {
            json!({"type":"equal", "left":binding(left), "right":binding(right)})
        }
        WorkflowPredicate::NotEqual { left, right } => {
            json!({"type":"not_equal", "left":binding(left), "right":binding(right)})
        }
        WorkflowPredicate::Not { predicate: p } => json!({"type":"not", "predicate":predicate(p)}),
        WorkflowPredicate::And { predicates } => {
            json!({"type":"and", "predicates":predicates.iter().map(predicate).collect::<Vec<_>>()})
        }
        WorkflowPredicate::Or { predicates } => {
            json!({"type":"or", "predicates":predicates.iter().map(predicate).collect::<Vec<_>>()})
        }
    }
}

#[allow(clippy::too_many_lines)] // Exhaustive projection of the seven compiled node families.
fn block(program: &WorkflowBlockProgram, path: &str, result: &mut WorkflowInspection) {
    let mut nodes = BTreeMap::new();
    for (id, node) in &program.nodes {
        let mut view = NodeInspection {
            kind: "return",
            edges: program.outgoing[id]
                .iter()
                .map(|edge| (edge.port, edge.to.clone()))
                .collect(),
            bindings: BTreeMap::new(),
            profile: None,
            invocation_override: None,
            selector: None,
            output_schema: None,
            result_part: None,
            result_kind: None,
            branches: Vec::new(),
            max_iterations: None,
            review_subject: None,
        };
        match node {
            WorkflowNodeProgram::Agent(agent) => {
                result.runtime_requirements.insert("provider_execution");
                view.kind = "agent";
                view.profile = Some(agent.profile.clone());
                result.profiles.insert(agent.profile.clone());
                if let Some(invocation_override) = &agent.invocation_override {
                    result
                        .runtime_requirements
                        .insert("agent_override_reference_resolution_and_source_availability");
                    view.invocation_override = Some(AgentOverrideInspection {
                        tools: invocation_override
                            .tools
                            .as_ref()
                            .map(super::super::subagent::invocation::SubagentInvocationOverride::canonical_selectors_of),
                        skills: invocation_override.skills.clone(),
                        extensions: invocation_override
                            .extensions
                            .as_ref()
                            .map(|selection| {
                                crate::extensions::composed_extension_names(&selection.resolve())
                            }),
                    });
                }
                view.output_schema = Some(schema(&agent.output_schema));
                view.bindings.extend(
                    agent
                        .input
                        .iter()
                        .map(|(key, value)| (format!("input.{key}"), binding(value))),
                );
            }
            WorkflowNodeProgram::Tool {
                selector,
                arguments,
                result: tool_result,
            } => {
                result.runtime_requirements.extend([
                    "native_argument_normalization_and_validation",
                    "capability_availability_at_admission",
                    "native_interaction_if_requested",
                ]);
                view.kind = "tool";
                view.selector = Some(selector.clone());
                view.bindings.insert("arguments".into(), binding(arguments));
                view.output_schema = Some(schema(&tool_result.schema()));
                let (part, kind) = match tool_result {
                    super::WorkflowToolResult::Json { part, .. } => (*part, "json"),
                    super::WorkflowToolResult::Text { part } => (*part, "text"),
                };
                view.result_part = Some(part);
                view.result_kind = Some(kind);
            }
            WorkflowNodeProgram::Return { output } => {
                view.bindings.insert("output".into(), binding(output));
            }
            WorkflowNodeProgram::Branch { condition } => {
                result.runtime_requirements.insert("branch_outcome");
                view.kind = "branch";
                view.bindings
                    .insert("condition".into(), predicate(condition));
            }
            WorkflowNodeProgram::Loop {
                input,
                body,
                until,
                carry,
                max_iterations,
                output_schema,
            } => {
                result.runtime_requirements.insert("actual_loop_iterations");
                view.kind = "loop";
                view.max_iterations = Some(*max_iterations);
                view.output_schema = Some(schema(output_schema));
                view.bindings.extend([
                    ("input".into(), binding(input)),
                    ("carry".into(), binding(carry)),
                    ("until".into(), predicate(until)),
                ]);
                block(body, &format!("{path}.nodes.{id}.body"), result);
            }
            WorkflowNodeProgram::Parallel {
                branches,
                output_schema,
            } => {
                view.kind = "parallel";
                view.branches = branches.keys().cloned().collect();
                view.output_schema = Some(schema(output_schema));
                for (key, branch) in branches {
                    view.bindings
                        .insert(format!("branches.{key}.input"), binding(&branch.input));
                    block(
                        &branch.block,
                        &format!("{path}.nodes.{id}.branches.{key}.block"),
                        result,
                    );
                }
            }
            WorkflowNodeProgram::Review { subject, context } => {
                result
                    .runtime_requirements
                    .insert("human_review_decision_and_interaction_availability");
                view.kind = "review";
                view.review_subject = Some(match subject {
                    WorkflowReviewSubject::Plan { .. } => "plan",
                    WorkflowReviewSubject::Candidate { .. } => "candidate",
                });
                view.bindings
                    .insert("subject".into(), binding(subject.value()));
                for (index, value) in context.iter().enumerate() {
                    view.bindings
                        .insert(format!("context.{index}"), binding(value));
                }
            }
        }
        nodes.insert(id.clone(), view);
    }
    result.blocks.insert(
        path.into(),
        BlockInspection {
            entry: program.entry.clone(),
            input_schema: schema(&program.input_schema),
            output_schema: schema(&program.output_schema),
            nodes,
        },
    );
}
