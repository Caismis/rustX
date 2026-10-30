//! Bounded, inert projections of the authoritative compiled program.
//! No executor, runtime owner, discovery service, or authority is accepted here.

use super::{
    BTreeMap, BTreeSet, MAX_BLOCK_DEPTH, MAX_LOCAL_BYTES, MAX_LOOP_ITERATIONS, MAX_VALUE_BYTES,
    MAX_WORKFLOW_AGENTS, MAX_WORKFLOW_MODEL_CALLS, MAX_WORKFLOW_NODES, MAX_WORKFLOW_STEPS,
    Serialize, SubagentName, Value, WorkflowBlockProgram, WorkflowNodeProgram, WorkflowPort,
    WorkflowPredicate, WorkflowProgram, WorkflowReviewSubject, WorkflowToolIdentity, WorkflowValue,
};
use serde_json::json;
use std::borrow::Cow;

impl WorkflowProgram {
    /// Trusted Model intent with exact authored paths, available to structural
    /// checkers without introducing model execution or credential authority.
    #[must_use]
    pub fn model_policy_nodes(&self) -> Vec<ModelPolicyNode<'_>> {
        fn walk<'a>(
            block: &'a WorkflowBlockProgram,
            path: &str,
            found: &mut Vec<ModelPolicyNode<'a>>,
        ) {
            for (id, node) in &block.nodes {
                let path = format!("{path}.nodes.{id}");
                match node {
                    WorkflowNodeProgram::Model(model) => found.push(ModelPolicyNode {
                        path: format!("{path}.model"),
                        policy: &model.model,
                    }),
                    WorkflowNodeProgram::Loop { body, .. } => {
                        walk(body, &format!("{path}.body"), found);
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

impl WorkflowProgram {
    /// Every node's compiled output contract keyed by its authored path
    /// (`block.nodes.<id>`, nested blocks included). Loop
    /// outputs are the compiler's derived results, never authored schemas.
    ///
    /// This is an authoring fact of the owner's own program, so schemas are
    /// not redacted; the redacted [`WorkflowInspection`] stays the Explain
    /// surface.
    #[must_use]
    pub fn node_outputs(&self) -> BTreeMap<String, Value> {
        fn walk(block: &WorkflowBlockProgram, path: &str, found: &mut BTreeMap<String, Value>) {
            for (id, node) in &block.nodes {
                let path = format!("{path}.nodes.{id}");
                let output = match node {
                    WorkflowNodeProgram::Model(model) => Some(model.output.schema()),
                    WorkflowNodeProgram::Agent(agent) => Some(agent.output.schema()),
                    WorkflowNodeProgram::Tool { result, .. } => Some(result.schema()),
                    WorkflowNodeProgram::Review { .. } => Some(super::review::result_schema()),
                    WorkflowNodeProgram::Loop {
                        body,
                        output_schema,
                        ..
                    } => {
                        walk(body, &format!("{path}.body"), found);
                        Some(output_schema.clone())
                    }
                    WorkflowNodeProgram::Start
                    | WorkflowNodeProgram::Branch { .. }
                    | WorkflowNodeProgram::Return { .. } => None,
                };
                if let Some(output) = output {
                    found.insert(path, output);
                }
            }
        }
        let mut found = BTreeMap::new();
        walk(&self.block, "block", &mut found);
        found
    }
}

/// One immutable model policy, with the diagnostic path of its selector.
#[derive(Debug, Clone)]
pub struct ModelPolicyNode<'a> {
    pub path: String,
    pub policy: &'a crate::model::session::AuxiliaryModelPolicy,
}

/// Static program facts. Literals and task text are deliberately not exported.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WorkflowInspection {
    /// Projected once at the local report surface, including selected role policies.
    #[serde(skip)]
    pub runtime_requirements: BTreeSet<&'static str>,
    pub identity: WorkflowToolIdentity,
    pub stage: Cow<'static, str>,
    pub input_schema: Value,
    pub output_schema: Value,
    pub configured_timeout_ms: u64,
    pub conservative_steps: usize,
    pub conservative_agent_runs: usize,
    pub conservative_model_calls: usize,
    pub conservative_retained_bytes: usize,
    pub caps: BTreeMap<Cow<'static, str>, usize>,
    pub blocks: BTreeMap<String, BlockInspection>,
    pub tools: BTreeSet<crate::capabilities::selection::ExactToolSelector>,
    pub profiles: BTreeSet<SubagentName>,
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BlockInspection {
    pub entry: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub nodes: BTreeMap<String, NodeInspection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct NodeInspection {
    pub kind: Cow<'static, str>,
    pub edges: Vec<(WorkflowPort, String)>,
    pub bindings: BTreeMap<String, Value>,
    pub profile: Option<SubagentName>,
    /// Selector identities only; provider parameters may contain secrets.
    pub model: Option<ModelPolicyInspection>,
    pub selector: Option<crate::capabilities::selection::ExactToolSelector>,
    pub output_schema: Option<Value>,
    pub result_part: Option<usize>,
    pub result_kind: Option<Cow<'static, str>>,
    pub max_iterations: Option<u32>,
    pub review_subject: Option<Cow<'static, str>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ModelPolicyInspection {
    Session,
    Explicit {
        model: crate::model::catalog::ModelRef,
        profile: Option<crate::model::catalog::ModelProfileId>,
        max_output_tokens: Option<u32>,
    },
}

impl WorkflowProgram {
    /// Project only compiled facts, in authored identity order. Conservative
    /// maxima are reservations, never predictions of decisions or requests.
    #[must_use]
    pub fn inspect(&self) -> WorkflowInspection {
        let mut result = WorkflowInspection {
            runtime_requirements: BTreeSet::from(["invoking_attempt_frozen_admission"]),
            identity: self.tool_identity(),
            stage: "statically_compiled_online_admission_unresolved".into(),
            input_schema: schema(&self.block.input_schema),
            output_schema: schema(&self.block.output_schema),
            configured_timeout_ms: self.timeout_ms,
            conservative_steps: self.execution_bound,
            conservative_agent_runs: self.agent_bound,
            conservative_model_calls: self.model_call_bound,
            conservative_retained_bytes: self.retained_bound,
            caps: BTreeMap::from([
                ("steps".into(), MAX_WORKFLOW_STEPS),
                ("agent_runs".into(), MAX_WORKFLOW_AGENTS),
                ("model_calls".into(), MAX_WORKFLOW_MODEL_CALLS),
                ("model_messages".into(), super::MAX_MODEL_MESSAGES),
                ("model_message_bytes".into(), super::MAX_MODEL_MESSAGE_BYTES),
                ("model_message_parts".into(), super::MAX_MODEL_MESSAGE_PARTS),
                ("nodes".into(), MAX_WORKFLOW_NODES),
                ("loop_iterations".into(), MAX_LOOP_ITERATIONS as usize),
                ("block_depth".into(), MAX_BLOCK_DEPTH),
                ("value_bytes".into(), MAX_VALUE_BYTES),
                ("retained_bytes".into(), MAX_LOCAL_BYTES),
                ("program_bytes".into(), super::MAX_WORKFLOW_BYTES),
                ("value_depth".into(), super::MAX_VALUE_DEPTH),
                (
                    "reference_components".into(),
                    super::MAX_REFERENCE_COMPONENTS,
                ),
            ]),
            blocks: BTreeMap::new(),
            tools: self.tools.clone(),
            profiles: BTreeSet::new(),
        };
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
        WorkflowPredicate::Contains { left, right } => {
            json!({"type":"contains", "left":binding(left), "right":binding(right)})
        }
        WorkflowPredicate::NotContains { left, right } => {
            json!({"type":"not_contains", "left":binding(left), "right":binding(right)})
        }
        WorkflowPredicate::StartsWith { left, right } => {
            json!({"type":"starts_with", "left":binding(left), "right":binding(right)})
        }
        WorkflowPredicate::EndsWith { left, right } => {
            json!({"type":"ends_with", "left":binding(left), "right":binding(right)})
        }
        WorkflowPredicate::Empty { value } => json!({"type":"empty", "value":binding(value)}),
        WorkflowPredicate::NotEmpty { value } => {
            json!({"type":"not_empty", "value":binding(value)})
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

#[allow(clippy::too_many_lines)] // Exhaustive projection of the compiled node families.
fn block(program: &WorkflowBlockProgram, path: &str, result: &mut WorkflowInspection) {
    let mut nodes = BTreeMap::new();
    for (id, node) in &program.nodes {
        let mut view = NodeInspection {
            kind: "return".into(),
            edges: program.outgoing[id]
                .iter()
                .map(|edge| (edge.port.clone(), edge.to.clone()))
                .collect(),
            bindings: BTreeMap::new(),
            profile: None,
            model: None,
            selector: None,
            output_schema: None,
            result_part: None,
            result_kind: None,
            max_iterations: None,
            review_subject: None,
        };
        match node {
            WorkflowNodeProgram::Model(model) => {
                result.runtime_requirements.extend([
                    "invoking_attempt_frozen_model_resolution",
                    "auxiliary_model_execution_and_settlement",
                ]);
                view.kind = "model".into();
                view.model = Some(match &model.model {
                    crate::model::session::AuxiliaryModelPolicy::Session => {
                        ModelPolicyInspection::Session
                    }
                    crate::model::session::AuxiliaryModelPolicy::Explicit {
                        model,
                        profile,
                        max_output_tokens,
                        ..
                    } => ModelPolicyInspection::Explicit {
                        model: model.clone(),
                        profile: profile.clone(),
                        max_output_tokens: *max_output_tokens,
                    },
                });
                view.output_schema = Some(schema(&model.output.schema()));
                view.result_kind = Some(match model.output {
                    super::WorkflowModelOutput::Text {} => "text".into(),
                    super::WorkflowModelOutput::JsonSchema { .. } => "json_schema".into(),
                });
                view.bindings
                    .extend(
                        model
                            .messages
                            .iter()
                            .enumerate()
                            .flat_map(|(index, message)| {
                                message
                                    .content
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, part)| {
                                        matches!(part, super::WorkflowValue::Reference { .. })
                                    })
                                    .map(move |(position, part)| {
                                        (
                                            format!("messages.{index}.content.{position}"),
                                            binding(part),
                                        )
                                    })
                            }),
                    );
            }
            WorkflowNodeProgram::Agent(agent) => {
                result.runtime_requirements.insert("provider_execution");
                view.kind = "agent".into();
                view.profile = Some(agent.profile.clone());
                result.profiles.insert(agent.profile.clone());
                view.output_schema = Some(schema(&agent.output.schema()));
                view.bindings.extend(
                    agent
                        .task
                        .iter()
                        .enumerate()
                        .filter(|(_, part)| matches!(part, super::WorkflowValue::Reference { .. }))
                        .map(|(position, part)| (format!("task.{position}"), binding(part))),
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
                view.kind = "tool".into();
                view.selector = Some(selector.clone());
                view.bindings.insert("arguments".into(), binding(arguments));
                view.output_schema = Some(schema(&tool_result.schema()));
                let (part, kind) = match tool_result {
                    super::WorkflowToolResult::Json { part, .. } => (*part, "json"),
                    super::WorkflowToolResult::Text { part } => (*part, "text"),
                };
                view.result_part = Some(part);
                view.result_kind = Some(kind.into());
            }
            WorkflowNodeProgram::Start => {
                view.kind = "start".into();
            }
            WorkflowNodeProgram::Return { output } => {
                view.bindings.insert("output".into(), binding(output));
            }
            WorkflowNodeProgram::Branch { cases } => {
                result.runtime_requirements.insert("branch_outcome");
                view.kind = "branch".into();
                for case in cases {
                    view.bindings.insert(
                        format!("cases.{}.condition", case.id),
                        predicate(&case.condition),
                    );
                }
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
                view.kind = "loop".into();
                view.max_iterations = Some(*max_iterations);
                view.output_schema = Some(schema(output_schema));
                view.bindings.extend([
                    ("input".into(), binding(input)),
                    ("carry".into(), binding(carry)),
                    ("until".into(), predicate(until)),
                ]);
                block(body, &format!("{path}.nodes.{id}.body"), result);
            }
            WorkflowNodeProgram::Review { subject, context } => {
                result
                    .runtime_requirements
                    .insert("human_review_decision_and_interaction_availability");
                view.kind = "review".into();
                view.review_subject = Some(match subject {
                    WorkflowReviewSubject::Plan { .. } => "plan".into(),
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
