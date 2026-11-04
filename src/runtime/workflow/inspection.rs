//! Bounded, inert projections of the authoritative compiled program.
//! No executor, runtime owner, discovery service, or authority is accepted here.

use super::{
    BTreeMap, BTreeSet, LOOP_NODE_MAX_COUNT, MAX_BLOCK_DEPTH, MAX_LOCAL_BYTES, MAX_VALUE_BYTES,
    MAX_WORKFLOW_AGENTS, MAX_WORKFLOW_MODEL_CALLS, MAX_WORKFLOW_NODES, MAX_WORKFLOW_STEPS,
    Serialize, SubagentName, Value, WorkflowBlockProgram, WorkflowNodeProgram, WorkflowPort,
    WorkflowPredicate, WorkflowProgram, WorkflowToolIdentity, WorkflowValue,
};
use serde_json::json;
use std::borrow::Cow;

impl WorkflowNodeProgram {
    /// The final value this node commits, after any internal response mapping.
    /// A Classifier's routing Model response is not its public node value.
    fn value_schema(&self) -> Option<Value> {
        match self {
            Self::Model(model) => Some(if model.classes.is_empty() {
                model.output.schema()
            } else {
                super::transform::classifier_result_schema()
            }),
            Self::Agent(agent) => Some(agent.value_schema()),
            Self::Tool { result, .. } => Some(result.schema()),
            Self::Review { actions, .. } => Some(super::review::result_schema(actions)),
            Self::Template { .. } => Some(super::transform::text_schema()),
            Self::List { output_schema, .. }
            | Self::Aggregate { output_schema, .. }
            | Self::Loop { output_schema, .. }
            | Self::Iteration { output_schema, .. } => Some(output_schema.clone()),
            Self::Start
            | Self::Branch { .. }
            | Self::Return { .. }
            | Self::LoopEnd
            | Self::Assigner { .. } => None,
        }
    }
}

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
                    WorkflowNodeProgram::Loop { body, .. }
                    | WorkflowNodeProgram::Iteration { body, .. } => {
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
                if let WorkflowNodeProgram::Loop { body, .. }
                | WorkflowNodeProgram::Iteration { body, .. } = node
                {
                    walk(body, &format!("{path}.body"), found);
                }
                if let Some(output) = node.value_schema() {
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
    pub result_kind: Option<Cow<'static, str>>,
    pub loop_count: Option<u32>,
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
                ("loop_count".into(), LOOP_NODE_MAX_COUNT as usize),
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
                        } else if matches!(key.as_str(), "items" | "anyOf") {
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
        WorkflowValue::Template { parts } => {
            json!({"type":"template", "parts":parts.iter().map(binding).collect::<Vec<_>>()})
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
        WorkflowPredicate::Greater { left, right } => {
            json!({"type":"greater", "left":binding(left), "right":binding(right)})
        }
        WorkflowPredicate::GreaterEqual { left, right } => {
            json!({"type":"greater_equal", "left":binding(left), "right":binding(right)})
        }
        WorkflowPredicate::Less { left, right } => {
            json!({"type":"less", "left":binding(left), "right":binding(right)})
        }
        WorkflowPredicate::LessEqual { left, right } => {
            json!({"type":"less_equal", "left":binding(left), "right":binding(right)})
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
            output_schema: node.value_schema().as_ref().map(schema),
            result_kind: None,
            loop_count: None,
        };
        match node {
            WorkflowNodeProgram::Template { variables, .. } => {
                view.kind = "template".into();
                view.bindings.extend(
                    variables
                        .iter()
                        .map(|(name, value)| (format!("variables.{name}"), binding(value))),
                );
            }
            WorkflowNodeProgram::List {
                input,
                filter,
                extract,
                ..
            } => {
                view.kind = "list".into();
                view.bindings.insert("input".into(), binding(input));
                if let Some(filter) = filter {
                    view.bindings
                        .insert("filter.value".into(), binding(&filter.value));
                }
                if let Some(extract) = extract {
                    view.bindings.insert("extract".into(), binding(extract));
                }
            }
            WorkflowNodeProgram::Aggregate { groups, .. } => {
                view.kind = "aggregate".into();
                for (index, group) in groups.iter().enumerate() {
                    for (position, path) in group.variables.iter().enumerate() {
                        view.bindings.insert(
                            format!("groups.{index}.variables.{position}"),
                            binding(&WorkflowValue::Reference { path: path.clone() }),
                        );
                    }
                }
            }
            WorkflowNodeProgram::Model(model) => {
                result.runtime_requirements.extend([
                    "invoking_attempt_frozen_model_resolution",
                    "auxiliary_model_execution_and_settlement",
                ]);
                view.kind = if model.classes.is_empty() {
                    "model"
                } else {
                    "classifier"
                }
                .into();
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
                ..
            } => {
                result.runtime_requirements.extend([
                    "native_argument_normalization_and_validation",
                    "capability_availability_at_admission",
                    "native_interaction_if_requested",
                ]);
                view.kind = "tool".into();
                view.selector = Some(selector.clone());
                match arguments {
                    super::tool::WorkflowToolArguments::Value(value) => {
                        view.bindings.insert("arguments".into(), binding(value));
                    }
                    super::tool::WorkflowToolArguments::Code { variables, .. } => {
                        view.kind = "code".into();
                        view.bindings.extend(
                            variables
                                .iter()
                                .map(|(name, value)| (format!("variables.{name}"), binding(value))),
                        );
                    }
                }
                view.result_kind = Some(
                    match tool_result {
                        super::tool::WorkflowToolProjection::Native(
                            super::WorkflowToolResult::Text {},
                        ) => "text",
                        _ => "json",
                    }
                    .into(),
                );
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
                loop_variables,
                break_condition,
                loop_count,
                body,
                ..
            } => {
                result.runtime_requirements.insert("actual_loop_iterations");
                view.kind = "loop".into();
                view.loop_count = Some(*loop_count);
                for variable in loop_variables {
                    view.bindings.insert(
                        format!("loop_variables.{}", variable.label),
                        binding(&variable.value),
                    );
                }
                if let Some(condition) = break_condition {
                    view.bindings
                        .insert("break_condition".into(), predicate(condition));
                }
                block(body, &format!("{path}.nodes.{id}.body"), result);
            }
            WorkflowNodeProgram::Iteration {
                iterator,
                output,
                body,
                ..
            } => {
                result.runtime_requirements.insert("actual_loop_iterations");
                view.kind = "iteration".into();
                view.bindings.extend([
                    ("iterator".into(), binding(iterator)),
                    ("output".into(), binding(output)),
                ]);
                block(body, &format!("{path}.nodes.{id}.body"), result);
            }
            WorkflowNodeProgram::LoopEnd => {
                view.kind = "loop_end".into();
            }
            WorkflowNodeProgram::Assigner { items, .. } => {
                view.kind = "assigner".into();
                for (index, item) in items.iter().enumerate() {
                    if let Some(value) = &item.value {
                        view.bindings
                            .insert(format!("items.{index}.value"), binding(value));
                    }
                }
            }
            WorkflowNodeProgram::Review { content, .. } => {
                result
                    .runtime_requirements
                    .insert("human_input_and_interaction_availability");
                view.kind = "review".into();
                view.bindings.extend(
                    content
                        .iter()
                        .enumerate()
                        .filter(|(_, part)| matches!(part, super::WorkflowValue::Reference { .. }))
                        .map(|(position, part)| (format!("content.{position}"), binding(part))),
                );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explain_redacts_nullable_list_records_and_return_projections() {
        let scalar = json!({
            "type":"string",
            "enum":["PRIVATE_ENUM_MARKER", "PRIVATE_DEFAULT_MARKER"],
            "default":"PRIVATE_DEFAULT_MARKER",
            "description":"PRIVATE_DESCRIPTION_MARKER",
        });
        let object = json!({
            "type":"object", "properties":{"id":scalar}, "required":["id"],
            "additionalProperties":false,
            "description":"PRIVATE_OBJECT_DESCRIPTION_MARKER",
            "default":{"id":"PRIVATE_DEFAULT_MARKER"},
        });
        for item in [scalar, object] {
            let definition = serde_json::from_value(json!({
                "description":"Explain a nullable List result",
                "block":{
                    "input":{
                        "type":"object", "properties":{"items":{"type":"array","items":item}},
                        "required":["items"], "additionalProperties":false,
                    },
                    "nodes":{
                        "start":{"type":"start"},
                        "list":{"type":"list","input":{"type":"reference","path":["args","items"]}},
                        "done":{"type":"return","outputs":[{"variable":"selected","value_selector":["list","first_record"]}]},
                    },
                    "edges":[{"from":"start","to":"list"},{"from":"list","to":"done"}],
                },
            })).unwrap();
            let program = WorkflowProgram::compile(
                super::super::WorkflowId::parse("private_list").unwrap(),
                definition,
            )
            .unwrap();
            let report = program.inspect();
            let serialized = serde_json::to_string(&report).unwrap();
            for marker in [
                "PRIVATE_ENUM_MARKER",
                "PRIVATE_DEFAULT_MARKER",
                "PRIVATE_DESCRIPTION_MARKER",
                "PRIVATE_OBJECT_DESCRIPTION_MARKER",
            ] {
                assert!(!serialized.contains(marker), "Explain leaked {marker}");
            }
            // The actual inferred union and both nullable projections remain
            // visible, including nested object property structure.
            let expected_item = schema(&item);
            let nullable = json!({"anyOf":[expected_item, {"type":"null"}]});
            let node = report.blocks["block"].nodes["list"]
                .output_schema
                .as_ref()
                .unwrap();
            assert_eq!(node["properties"]["result"]["items"], expected_item);
            for field in ["first_record", "last_record"] {
                assert_eq!(node["properties"][field], nullable);
            }
            assert_eq!(report.output_schema["properties"]["selected"], nullable);
            assert_eq!(report.blocks["block"].output_schema, report.output_schema);
            let unredacted = program.node_outputs();
            assert_eq!(
                unredacted["block.nodes.list"]["properties"]["first_record"]["anyOf"][0],
                item
            );
            let annotation = if item["type"] == "object" {
                &expected_item["properties"]["id"]
            } else {
                &expected_item
            };
            for field in ["enum", "default", "description"] {
                assert_eq!(annotation[field], json!({"redacted":true}));
            }
        }
    }

    #[test]
    fn numeric_inspection_keeps_the_operator_and_redacts_literal_operands() {
        for kind in ["greater", "greater_equal", "less", "less_equal"] {
            let condition:WorkflowPredicate=serde_json::from_value(json!({"type":kind,"left":{"type":"reference","path":["args","amount"]},"right":{"type":"literal","value":938_471}})).unwrap();
            let view = predicate(&condition);
            assert_eq!(view["type"], kind);
            assert_eq!(view["left"]["path"], json!(["args", "amount"]));
            assert!(!view.to_string().contains("938471"));
        }
    }
}
