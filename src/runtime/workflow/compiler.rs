//! Workflow graph, reference, schema, and node compiler.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    AGENT_ROUTE_FIELD, LOOP_NODE_MAX_COUNT, MAX_AGENT_ROUTE_DESCRIPTION_BYTES, MAX_AGENT_ROUTES,
    MAX_BLOCK_DEPTH, MAX_LOCAL_BYTES, MAX_MODEL_MESSAGE_BYTES, MAX_MODEL_MESSAGE_PARTS,
    MAX_MODEL_MESSAGES, MAX_PARALLEL_LIMIT, MAX_RETRIES, MAX_REVIEW_ACTIONS,
    MAX_RUN_ID_COMPONENT_BYTES, MAX_VALUE_BYTES, MAX_VALUE_DEPTH, MAX_WORKFLOW_BYTES,
    MAX_WORKFLOW_NODES, RETRY_INTERVAL_MS, REVIEW_TIMEOUT_PORT, SubagentName, WorkflowAgentOutput,
    WorkflowAgentProgram, WorkflowAgentRoute, WorkflowBlock, WorkflowBlockProgram,
    WorkflowDefinition, WorkflowEdgeProgram, WorkflowId, WorkflowModelErrorStrategy,
    WorkflowModelMessage, WorkflowModelOutput, WorkflowModelProgram, WorkflowModelRole,
    WorkflowNodeDefinition, WorkflowNodeProgram, WorkflowPort, WorkflowProgram, WorkflowRetry,
    WorkflowReturnOutput, WorkflowReviewAction, WorkflowRunDefinitionSnapshot, WorkflowValue,
    WorkflowValueErrorStrategy, WorkflowVarType, agent_value_schema, code, empty_object_schema,
    execution, expressions, flow, is_empty_object_schema, loop_variables_schema, references,
    review, tool, transform, valid_local_key, validate_predicate, value_schema,
};

/// A compile-time graph/type/reference rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowCompileError {
    /// Authored field context supplied by the compiler, never inferred from prose.
    At { path: String, cause: Box<Self> },
    /// A required textual field is empty or over its bound.
    InvalidField(String),
    /// A JSON Schema is invalid or not a root object schema.
    InvalidSchema(String),
    /// A node or edge refers to an unknown id.
    DanglingReference(String),
    /// The graph has no single explicit entry.
    InvalidEntry(String),
    /// The graph contains a cycle; node is the smallest positive-indegree Kahn residual.
    Cycle { node: String },
    /// A node is unreachable from the explicit entry.
    Unreachable(String),
    /// A path can leave a node without a deterministic successor/terminal.
    Unterminated(String),
    /// A Branch has invalid cases or an incomplete set of exit connections.
    InvalidBranch(String),
    /// A workflow reference cannot be resolved on every relevant path.
    InvalidReference(String),
    /// A reference has an incompatible statically known schema.
    IncompatibleReference(String),
}

impl fmt::Display for WorkflowCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::At { path, cause } => write!(formatter, "{path}: {cause}"),
            Self::InvalidField(detail)
            | Self::InvalidSchema(detail)
            | Self::DanglingReference(detail)
            | Self::InvalidEntry(detail)
            | Self::Unterminated(detail)
            | Self::InvalidBranch(detail)
            | Self::InvalidReference(detail)
            | Self::IncompatibleReference(detail) => formatter.write_str(detail),
            Self::Cycle { node } => write!(
                formatter,
                "workflow graph contains a cycle; residual node {node:?}"
            ),
            Self::Unreachable(node) => write!(formatter, "workflow node {node:?} is unreachable"),
        }
    }
}

impl std::error::Error for WorkflowCompileError {}

impl WorkflowCompileError {
    pub(super) fn at(self, prefix: impl AsRef<str>) -> Self {
        match self {
            Self::At { path, cause } => Self::At {
                path: format!("{}.{path}", prefix.as_ref()),
                cause,
            },
            cause => Self::At {
                path: prefix.as_ref().into(),
                cause: Box::new(cause),
            },
        }
    }

    /// The deepest native compiler rejection, independent of authored context.
    #[must_use]
    pub fn cause(&self) -> &Self {
        match self {
            Self::At { cause, .. } => cause.cause(),
            cause => cause,
        }
    }

    /// Authored field path, supplied by the validation owner.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::At { path, .. } => path,
            _ => "$",
        }
    }
}

#[allow(clippy::too_many_lines)] // one bounded graph-to-program validation pipeline
fn compile_program(
    id: WorkflowId,
    source_revision: Option<String>,
    document: crate::workflow_file::WorkflowFileDocument,
) -> Result<WorkflowProgram, WorkflowCompileError> {
    document
        .validate()
        .map_err(|error| WorkflowCompileError::InvalidField(error.reason).at(error.path))?;
    let snapshot_document = document.clone();
    let definition = document.definition;
    if definition.timeout_ms == 0 || definition.timeout_ms > 86_400_000 {
        return Err(
            WorkflowCompileError::InvalidField("timeout_ms must be 1..=86400000".into())
                .at("timeout_ms"),
        );
    }
    if definition.description.trim().is_empty() {
        return Err(WorkflowCompileError::InvalidField(format!(
            "workflow {id} has an empty description"
        ))
        .at("description"));
    }
    if definition.description.len() > 4096 {
        return Err(WorkflowCompileError::InvalidField(format!(
            "workflow {id} description is too large"
        ))
        .at("description"));
    }
    if !crate::bounded_file::json_fits(&definition, MAX_WORKFLOW_BYTES) {
        return Err(WorkflowCompileError::InvalidField(
            "aggregate program size exceeded".into(),
        ));
    }
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&definition).expect("validated definition"))
    );
    let mut total_nodes = 0;
    let (block, _) = compile_block(definition.block, Vec::new(), &mut total_nodes, None)
        .map_err(|error| error.at("block"))?;
    let retained_bound = execution::static_retained_bound(&block);
    let (execution_bound, agent_bound, model_call_bound) =
        execution::static_execution_bound(&block).map_err(|e| e.at("block"))?;
    if retained_bound > MAX_LOCAL_BYTES {
        return Err(WorkflowCompileError::InvalidField(
            "aggregate retained-data reservation exceeds the run budget".into(),
        )
        .at("block"));
    }
    let has_source_presentation = snapshot_document
        .editor
        .as_ref()
        .is_some_and(|editor| editor.nodes.iter().any(|node| node.position.is_some()));
    let source_revision = source_revision.unwrap_or_else(|| format!("program:{digest}"));
    let snapshot = Arc::new(WorkflowRunDefinitionSnapshot {
        workflow_id: id.clone(),
        program_digest: digest.clone(),
        source_revision,
        has_source_presentation,
        document: snapshot_document,
    });
    Ok(WorkflowProgram {
        digest,
        id,
        description: definition.description,
        tools: collect_tool_selectors(&block),
        block,
        execution_bound,
        agent_bound,
        model_call_bound,
        retained_bound,
        timeout_ms: definition.timeout_ms,
        frozen_tools: BTreeMap::new(),
        permissions: definition.permissions,
        snapshot,
    })
}

fn collect_tool_selectors(
    block: &WorkflowBlockProgram,
) -> BTreeSet<crate::capabilities::selection::ExactToolSelector> {
    let mut selectors = BTreeSet::new();
    for node in block.nodes.values() {
        match node {
            WorkflowNodeProgram::Tool { selector, .. } => {
                selectors.insert(selector.clone());
            }
            WorkflowNodeProgram::Loop { body, .. }
            | WorkflowNodeProgram::Iteration { body, .. } => {
                selectors.extend(collect_tool_selectors(body));
            }
            _ => {}
        }
    }
    selectors
}

/// What a Loop or Iteration body sees around it.
struct Container {
    /// The variables in scope at the container, plus its own.
    base: SchemaMap,
    owner: String,
    /// A Loop's variables by label; `None` in an Iteration.
    loop_variables: Option<BTreeMap<String, WorkflowVarType>>,
}

/// A body and the variables available whenever a round completes,
/// for the Loop's break condition and the Iteration's output.
fn compile_body(
    body: &WorkflowBlock,
    path: &[String],
    owner: &str,
    total_nodes: &mut usize,
    container: &Container,
) -> Result<(WorkflowBlockProgram, SchemaMap), WorkflowCompileError> {
    if !is_empty_object_schema(&body.input) {
        return Err(WorkflowCompileError::InvalidField(
            "a body declares no input; it reads the variables around it".into(),
        )
        .at("body"));
    }
    let mut child_path = path.to_vec();
    child_path.extend([owner.to_owned(), "body".into()]);
    compile_block(body.clone(), child_path, total_nodes, Some(container)).map_err(|e| e.at("body"))
}

#[allow(clippy::too_many_lines)]
fn compile_block(
    definition: WorkflowBlock,
    path: Vec<String>,
    total_nodes: &mut usize,
    container: Option<&Container>,
) -> Result<(WorkflowBlockProgram, SchemaMap), WorkflowCompileError> {
    *total_nodes += definition.nodes.len();
    if path.len() / 2 > MAX_BLOCK_DEPTH || *total_nodes > MAX_WORKFLOW_NODES {
        return Err(WorkflowCompileError::InvalidField(
            "aggregate block depth/node bound exceeded".into(),
        ));
    }
    validate_root_schema(&definition.input, "input").map_err(|e| e.at("input"))?;
    let base = container.map_or_else(
        || SchemaMap::from_schema(&definition.input),
        |container| container.base.clone(),
    );
    // As Dify's node IDs, a body's IDs never hide a variable around it.
    if let Some(shadowed) = definition.nodes.keys().find(|id| base.0.contains_key(*id)) {
        return Err(WorkflowCompileError::InvalidField(format!(
            "node id {shadowed:?} is already used around this body"
        ))
        .at(format!("nodes.{shadowed}")));
    }
    let ControlFlow {
        outgoing,
        topological,
        entry,
        scopes,
    } = control_flow(&definition)?;

    let writers = definition
        .nodes
        .iter()
        .filter_map(|(id, node)| match node {
            WorkflowNodeDefinition::Assigner { items } => Some((id, items)),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (index, (left, items)) in writers.iter().enumerate() {
        for (right, other) in &writers[index + 1..] {
            if scopes.unordered_together(left, right)
                && items
                    .iter()
                    .any(|item| other.iter().any(|other| item.variable == other.variable))
            {
                return Err(WorkflowCompileError::InvalidField(format!(
                    "Assigners {left:?} and {right:?} write the same variable without an execution order"
                )).at(format!("nodes.{right}.items")));
            }
        }
    }

    let reads = definition
        .nodes
        .iter()
        .map(|(id, node)| (id, references::node(node)))
        .collect::<Vec<_>>();
    for (writer, items) in &writers {
        for (reader, paths) in &reads {
            if writer != reader
                && scopes.unordered_together(writer, reader)
                && items.iter().any(|item| {
                    paths.iter().any(|path| {
                        path.first() == item.variable.first()
                            && (path.len() == 1 || path.get(1) == item.variable.get(1))
                    })
                })
            {
                return Err(WorkflowCompileError::InvalidField(format!(
                    "nodes {writer:?} and {reader:?} read and write the same Loop variable without an execution order"
                )).at(format!("nodes.{reader}")));
            }
        }
    }

    let mut nodes = BTreeMap::new();
    let mut available_after: BTreeMap<String, SchemaMap> = BTreeMap::new();
    let mut returned = Vec::new();
    for node_id in topological {
        let node = definition
            .nodes
            .get(&node_id)
            .expect("topological node exists");
        let available_before = if matches!(node, WorkflowNodeDefinition::Aggregate { .. }) {
            scopes.potential_scope(&node_id, &available_after, &base)
        } else {
            scopes.scope(&node_id, &available_after, &base)
        };
        let compiled = (|| { Ok(match node {
            WorkflowNodeDefinition::Start {} => {
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Start
            }
            WorkflowNodeDefinition::Template { variables, template } => {
                transform::validate_template(template, variables)?;
                for (name,value) in variables { value_schema(value,&available_before,&node_id,0).map_err(|e| e.at(format!("variables.{name}")))?; }
                available_after.insert(node_id.clone(), available_before.with_prefix(&node_id,&transform::text_schema()));
                WorkflowNodeProgram::Template { variables:variables.clone(), template:template.clone() }
            }
            WorkflowNodeDefinition::List { input, filter, sort, limit, extract } => {
                let array = value_schema(input,&available_before,&node_id,0).map_err(|e| e.at("input"))?;
                transform::validate_list_values(&array, filter.as_ref(), extract.as_ref(), &available_before, &node_id)?;
                let output_schema = transform::list_schema(&array,filter.as_ref(),sort.as_ref(),*limit)?;
                available_after.insert(node_id.clone(),available_before.with_prefix(&node_id,&output_schema));
                WorkflowNodeProgram::List { input:input.clone(),filter:filter.clone(),sort:sort.clone(),limit:*limit,extract:extract.clone(),output_schema }
            }
            WorkflowNodeDefinition::Classifier { model,input,instruction,classes } => {
                let model = transform::classifier(model,input,instruction,classes,&available_before,&node_id)?;
                if container.is_none() && classes.iter().any(|class| !outgoing[&node_id].iter().any(|edge| edge.port == WorkflowPort::case(&class.id))) { return Err(WorkflowCompileError::InvalidBranch("Classifier requires a successor per class".into())); }
                available_after.insert(node_id.clone(),available_before.with_prefix(&node_id,&transform::classifier_result_schema()));
                WorkflowNodeProgram::Model(model)
            }
            WorkflowNodeDefinition::Code { code_language, variables, code: script, outputs, retry, on_error } => {
                code::validate(variables, script, &available_before, &node_id)?;
                let schema = code::output_schema(outputs)?;
                validate_workflow_schema(&schema, "Code outputs")?;
                validate_retry(retry.as_ref(), "Code", &node_id)?;
                if let WorkflowValueErrorStrategy::DefaultValue { value } = on_error
                    && !jsonschema::Validator::new(&schema).is_ok_and(|validator| validator.is_valid(value)) {
                    return Err(WorkflowCompileError::InvalidField("Code default value must satisfy its outputs".into()).at("on_error.value"));
                }
                available_after.insert(node_id.clone(), available_before.with_prefix(&node_id, &schema));
                WorkflowNodeProgram::Tool {
                    selector: crate::capabilities::selection::ExactToolSelector::Builtin { name: "bash".into() },
                    arguments: tool::WorkflowToolArguments::Code { language: *code_language, variables: variables.clone(), code: script.clone() },
                    result: tool::WorkflowToolProjection::Code { schema }, retry: *retry, on_error: on_error.clone(),
                }
            }
            WorkflowNodeDefinition::Aggregate { groups, grouped } => {
                transform::validate_groups(groups,*grouped)?;
                let active=transform::active_groups(groups,*grouped);
                for (index,group) in active.iter().enumerate() {
                    scopes.validate_aggregate(&node_id,&group.variables,&base).map_err(|e| e.at(format!("groups.{index}.variables")))?;
                    for (position,path) in group.variables.iter().enumerate() {
                        let actual = value_schema(&WorkflowValue::Reference {path:path.clone()},&available_before,&node_id,0).map_err(|e| e.at(format!("groups.{index}.variables.{position}")))?;
                        if !schemas_compatible(&actual,&group.output_type.ok_or_else(|| WorkflowCompileError::InvalidField("aggregate type is not selected".into()))?.schema()) { return Err(WorkflowCompileError::IncompatibleReference("aggregate source type does not match the output group".into()).at(format!("groups.{index}.variables.{position}"))); }
                    }
                }
                let output_schema = transform::aggregate_schema(groups,*grouped)?;
                available_after.insert(node_id.clone(),available_before.with_prefix(&node_id,&output_schema));
                WorkflowNodeProgram::Aggregate {groups:active.to_vec(),grouped:*grouped,output_schema}
            }
            WorkflowNodeDefinition::Model { model, messages, output, retry, on_error } => {
                let model = model.lower().map_err(|error| error.at("model"))?;
                validate_model(&model, messages, output, &available_before, &node_id)?;
                validate_model_failure(retry.as_ref(), on_error, output, &node_id)?;
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &output.schema()),
                );
                WorkflowNodeProgram::Model(WorkflowModelProgram {
                    classes: Vec::new(),
                    model,
                    messages: messages.clone(),
                    output: output.clone(),
                    retry: *retry,
                    on_error: on_error.clone(),
                })
            }
            WorkflowNodeDefinition::Loop {
                loop_variables,
                break_condition,
                loop_count,
                body,
            } => {
                if *loop_count == 0 || *loop_count > LOOP_NODE_MAX_COUNT {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "Loop {node_id:?} loop_count must be 1..={LOOP_NODE_MAX_COUNT}"
                    )).at("loop_count"));
                }
                let mut labels = BTreeSet::new();
                for (index, variable) in loop_variables.iter().enumerate() {
                    if !valid_local_key(&variable.label) || !labels.insert(&variable.label) {
                        return Err(WorkflowCompileError::InvalidField(format!(
                            "Loop {node_id:?} variable labels must be unique valid keys"
                        )).at(format!("loop_variables.{index}.label")));
                    }
                    let actual = value_schema(&variable.value, &available_before, &node_id, 0)
                        .map_err(|e| e.at(format!("loop_variables.{index}.value")))?;
                    if !schemas_compatible(&actual, &variable.var_type.schema()) {
                        return Err(WorkflowCompileError::IncompatibleReference(format!(
                            "Loop variable {:?} value is not a {:?}", variable.label, variable.var_type
                        )).at(format!("loop_variables.{index}.value")));
                    }
                }
                let output_schema = loop_variables_schema(loop_variables);
                let container = Container {
                    base: available_before.with_prefix(&node_id, &output_schema),
                    owner: node_id.clone(),
                    loop_variables: Some(loop_variables.iter().map(|variable| (variable.label.clone(), variable.var_type)).collect()),
                };
                let (body, end) = compile_body(body, &path, &node_id, total_nodes, &container)?;
                if let Some(condition) = break_condition {
                    validate_predicate(condition, &end, &node_id, 0).map_err(|e| e.at("break_condition"))?;
                }
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &output_schema),
                );
                WorkflowNodeProgram::Loop {
                    loop_variables: loop_variables.clone(),
                    break_condition: break_condition.clone(),
                    loop_count: *loop_count,
                    body: Box::new(body),
                    output_schema,
                }
            }
            WorkflowNodeDefinition::Iteration {
                iterator,
                output,
                is_parallel,
                parallel_nums,
                error_handle_mode,
                flatten_output,
                body,
            } => {
                if !matches!(iterator, WorkflowValue::Reference { .. }) {
                    return Err(WorkflowCompileError::InvalidField(
                        "Iteration input must reference an array".into(),
                    ).at("iterator"));
                }
                let array = value_schema(iterator, &available_before, &node_id, 0).map_err(|e| e.at("iterator"))?;
                let Some(item) = array.get("items").filter(|_| schema_type(&array) == Some("array")) else {
                    return Err(WorkflowCompileError::IncompatibleReference(
                        "Iteration input must be an array with declared items".into(),
                    ).at("iterator"));
                };
                if *is_parallel && (*parallel_nums == 0 || *parallel_nums > MAX_PARALLEL_LIMIT) {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "Iteration {node_id:?} parallel_nums must be 1..={MAX_PARALLEL_LIMIT}"
                    )).at("parallel_nums"));
                }
                if !matches!(output, WorkflowValue::Reference { .. }) {
                    return Err(WorkflowCompileError::InvalidField(
                        "Iteration output must reference a variable of its body".into(),
                    ).at("output"));
                }
                let entry = serde_json::json!({
                    "type": "object",
                    "properties": {"item": item, "index": {"type": "integer"}},
                    "required": ["item", "index"],
                    "additionalProperties": false,
                });
                let container = Container {
                    base: available_before.with_prefix(&node_id, &entry),
                    owner: node_id.clone(),
                    loop_variables: None,
                };
                let (body, end) = compile_body(body, &path, &node_id, total_nodes, &container)?;
                let produced = value_schema(output, &end, &node_id, 0).map_err(|e| e.at("output"))?;
                let output_schema = execution::iteration_result_schema(Some(&produced), *error_handle_mode, *flatten_output);
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &output_schema),
                );
                WorkflowNodeProgram::Iteration {
                    iterator: iterator.clone(),
                    output: output.clone(),
                    parallel: if *is_parallel { *parallel_nums } else { 1 },
                    error_handle_mode: *error_handle_mode,
                    flatten_output: *flatten_output,
                    body: Box::new(body),
                    output_schema,
                }
            }
            WorkflowNodeDefinition::LoopEnd {} => {
                if container.is_none_or(|container| container.loop_variables.is_none()) {
                    return Err(WorkflowCompileError::InvalidField(
                        "Exit Loop belongs directly in a Loop body".into(),
                    ));
                }
                if !outgoing[&node_id].is_empty() {
                    return Err(WorkflowCompileError::Unterminated(format!(
                        "Exit Loop {node_id:?} cannot have outgoing edges"
                    )));
                }
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::LoopEnd
            }
            WorkflowNodeDefinition::Assigner { items } => {
                let Some((owner, variables)) = container.and_then(|container| Some((&container.owner, container.loop_variables.as_ref()?))) else {
                    return Err(WorkflowCompileError::InvalidField(
                        "a Variable Assigner writes Loop variables, so it belongs directly in a Loop body".into(),
                    ));
                };
                if items.is_empty() || items.len() > 32 {
                    return Err(WorkflowCompileError::InvalidField(
                        "a Variable Assigner makes 1..=32 writes".into(),
                    ).at("items"));
                }
                let mut types = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    let var_type = match item.variable.as_slice() {
                        [loop_id, label] if loop_id == owner => variables.get(label).copied(),
                        _ => None,
                    }.ok_or_else(|| WorkflowCompileError::DanglingReference(
                        "an assigned variable must be a variable of the Loop the Assigner is in".into(),
                    ).at(format!("items.{index}.variable")))?;
                    if !item.operation.applies_to(var_type) {
                        return Err(WorkflowCompileError::InvalidField(format!(
                            "{:?} cannot write a {var_type:?} variable", item.operation
                        )).at(format!("items.{index}.operation")));
                    }
                    match (item.operation.operand(var_type), &item.value) {
                        (None, None) => {}
                        (Some(expected), Some(value)) => {
                            let actual = value_schema(value, &available_before, &node_id, 0).map_err(|e| e.at(format!("items.{index}.value")))?;
                            if !schemas_compatible(&actual, &expected) {
                                return Err(WorkflowCompileError::IncompatibleReference(format!(
                                    "{:?} needs a value of {expected}", item.operation
                                )).at(format!("items.{index}.value")));
                            }
                        }
                        (Some(_), None) => return Err(WorkflowCompileError::InvalidField(
                            "this write needs a value".into(),
                        ).at(format!("items.{index}.value"))),
                        (None, Some(_)) => return Err(WorkflowCompileError::InvalidField(
                            "this write takes no value".into(),
                        ).at(format!("items.{index}.value"))),
                    }
                    types.push(var_type);
                }
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Assigner { items: items.clone(), types }
            }
            WorkflowNodeDefinition::Review { content, actions, timeout_ms } => {
                validate_review(content, actions, *timeout_ms, &available_before, &node_id)?;
                let exits = actions.iter().map(|action| WorkflowPort::case(&action.id))
                    .chain(std::iter::once(WorkflowPort::case(REVIEW_TIMEOUT_PORT)));
                if container.is_none() && exits.into_iter().any(|port| !outgoing[&node_id].iter().any(|edge| edge.port == port)) {
                    return Err(WorkflowCompileError::InvalidBranch(format!(
                        "Human Input {node_id:?} requires a successor for every action and its timeout"
                    )));
                }
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &review::result_schema(actions)),
                );
                WorkflowNodeProgram::Review {
                    content: content.clone(),
                    actions: actions.clone(),
                    timeout_ms: *timeout_ms,
                }
            }
            WorkflowNodeDefinition::Tool {
                selector,
                arguments,
                result,
                retry,
                on_error,
            } => {
                let selector = selector.lower().map_err(|error| error.at("selector"))?;
                if let crate::capabilities::selection::ExactToolSelector::Builtin { name } = &selector
                    && let Some(extension) = crate::capabilities::extension_provided_tool(name)
                {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "builtin:{name} belongs to Agent Extension {extension}, not ordinary Tool selection"
                    )).at("selector"));
                }
                let input = value_schema(arguments, &available_before, &node_id, 0).map_err(|e| e.at("arguments"))?;
                validate_tool_arguments_root(&input).map_err(|e| e.at("arguments"))?;
                let output = result.schema();
                validate_workflow_schema(&output, "Tool result").map_err(|e| e.at("result.schema"))?;
                validate_retry(retry.as_ref(), "Tool", &node_id)?;
                if let WorkflowValueErrorStrategy::DefaultValue { value } = on_error
                    && !jsonschema::Validator::new(&output).is_ok_and(|validator| validator.is_valid(value))
                {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "Tool {node_id:?} default value must satisfy its result schema"
                    ))
                    .at("on_error.value"));
                }
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &output),
                );
                WorkflowNodeProgram::Tool {
                    selector,
                    arguments: tool::WorkflowToolArguments::Value(arguments.clone()),
                    result: tool::WorkflowToolProjection::Native(result.clone()),
                    retry: *retry,
                    on_error: on_error.clone(),
                }
            }
            WorkflowNodeDefinition::Agent {
                profile,
                task,
                output,
                routes,
                permissions,
                on_error,
            } => {
                let profile = SubagentName::parse(profile).map_err(|_| WorkflowCompileError::InvalidField("Agent selection requires a valid named profile".into()).at("profile"))?;
                validate_agent(task, output, routes, on_error, &available_before, &node_id)?;
                let exits = routes.iter().map(|route| WorkflowPort::case(&route.id))
                    .chain(on_error.eq(&WorkflowValueErrorStrategy::FailBranch).then_some(WorkflowPort::Fail));
                if container.is_none() && exits.into_iter().any(|port| !outgoing[&node_id].iter().any(|edge| edge.port == port)) {
                    return Err(WorkflowCompileError::InvalidBranch(format!(
                        "Agent {node_id:?} requires a successor for every route and its fail branch"
                    )));
                }
                let agent = WorkflowAgentProgram {
                    resolved: None,
                    profile,
                    task: task.clone(),
                    output: output.clone(),
                    routes: routes.clone(),
                    resource_permissions: *permissions,
                    on_error: on_error.clone(),
                };
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &agent.value_schema()),
                );
                WorkflowNodeProgram::Agent(agent)
            }
            WorkflowNodeDefinition::Branch { cases } => {
                for (index, case) in cases.iter().enumerate() {
                    validate_predicate(&case.condition, &available_before, &node_id, 0)
                        .map_err(|e| e.at(format!("cases.{index}.condition")))?;
                }
                let ports = cases.iter().map(|case| WorkflowPort::case(&case.id))
                    .chain(std::iter::once(WorkflowPort::False));
                // A body's path may end at any port, as Dify's.
                if container.is_none() && ports.into_iter().any(|port| !outgoing[&node_id].iter().any(|edge| edge.port == port)) {
                    return Err(WorkflowCompileError::InvalidBranch(format!(
                        "Branch {node_id:?} requires a successor for every case and ELSE"
                    )));
                }
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Branch { cases: cases.clone() }
            }
            WorkflowNodeDefinition::Return { outputs } => {
                if container.is_some() {
                    return Err(WorkflowCompileError::InvalidField(
                        "Return ends the workflow; a body ends where its nodes do".into(),
                    ));
                }
                if !outgoing[&node_id].is_empty() {
                    return Err(WorkflowCompileError::Unterminated(format!(
                        "Return node {node_id:?} cannot have outgoing edges"
                    )));
                }
                let (output, schema) = return_output(outputs, &available_before, &node_id)?;
                returned.push(schema);
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Return { output }
            }
        }) })().map_err(|e: WorkflowCompileError| e.at(format!("nodes.{node_id}")))?;
        let edges = &outgoing[&node_id];
        match &compiled {
            // Return and Branch ports were validated above. A body's path
            // may end at any node.
            WorkflowNodeProgram::Return { .. }
            | WorkflowNodeProgram::Branch { .. }
            | WorkflowNodeProgram::Review { .. } => {}
            WorkflowNodeProgram::Agent(agent) if !agent.routes.is_empty() => {}
            WorkflowNodeProgram::Model(model) if !model.classes.is_empty() => {}
            _ if container.is_none()
                && !edges.iter().any(|edge| edge.port == WorkflowPort::Next) =>
            {
                return Err(WorkflowCompileError::Unterminated(format!(
                    "node {node_id:?} requires a successor on its success (next) exit"
                ))
                .at(format!("nodes.{node_id}")));
            }
            _ => {}
        }
        nodes.insert(node_id, compiled);
    }
    if container.is_none()
        && !nodes
            .values()
            .any(|node| matches!(node, WorkflowNodeProgram::Return { .. }))
    {
        return Err(WorkflowCompileError::Unterminated(
            "every workflow must contain a reachable Return node".to_owned(),
        ));
    }
    // A finite DAG whose only zero-outdegree nodes are Returns necessarily
    // terminates on every path. Do not enumerate exponentially many paths.
    let declarations = SchemaMap(
        available_after
            .iter()
            .filter_map(|(id, scope)| scope.0.get(id).map(|schema| (id.clone(), schema.clone())))
            .collect(),
    );
    let completed = scopes.completion_scope(&declarations, &base);
    Ok((
        WorkflowBlockProgram {
            path,
            input_schema: definition.input,
            output_schema: merge_return_schemas(&returned)?,
            entry,
            nodes,
            outgoing,
        },
        completed,
    ))
}

pub(super) struct ControlFlow {
    pub(super) entry: String,
    pub(super) scopes: flow::ActivationScopes,
    pub(super) outgoing: BTreeMap<String, Vec<WorkflowEdgeProgram>>,
    pub(super) topological: Vec<String>,
}

/// Shared finite control-flow proof for compilation and partial authoring.
#[allow(clippy::too_many_lines)]
pub(super) fn control_flow(
    definition: &WorkflowBlock,
) -> Result<ControlFlow, WorkflowCompileError> {
    if definition.nodes.is_empty() || definition.nodes.len() > MAX_WORKFLOW_NODES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "block must contain between one and {MAX_WORKFLOW_NODES} nodes"
        )));
    }
    let starts = definition
        .nodes
        .iter()
        .filter_map(|(id, node)| matches!(node, WorkflowNodeDefinition::Start {}).then_some(id))
        .collect::<Vec<_>>();
    if starts.len() != 1 {
        return Err(WorkflowCompileError::InvalidEntry(
            "block must contain exactly one Start node".into(),
        )
        .at("nodes"));
    }
    let entry = starts[0].clone();
    for node_id in definition.nodes.keys() {
        if node_id == "args" || !valid_local_key(node_id) {
            return Err(WorkflowCompileError::InvalidField(format!(
                "workflow node id {node_id:?} must be non-empty, at most 64 bytes, and contain no dots"
            )).at(format!("nodes.{node_id}")));
        }
    }
    for (id, node) in &definition.nodes {
        if let WorkflowNodeDefinition::Branch { cases } = node {
            if cases.is_empty() || cases.len() > 32 {
                return Err(WorkflowCompileError::InvalidBranch(
                    "Branch requires 1..=32 cases".into(),
                )
                .at(format!("nodes.{id}.cases")));
            }
            let mut ids = BTreeSet::new();
            for (index, case) in cases.iter().enumerate() {
                if !valid_local_key(&case.id) || is_reserved_port(&case.id) || !ids.insert(&case.id)
                {
                    return Err(WorkflowCompileError::InvalidBranch(
                        "case IDs must be unique valid local keys and cannot name a reserved port"
                            .into(),
                    )
                    .at(format!("nodes.{id}.cases.{index}.id")));
                }
            }
        }
    }
    let mut outgoing: BTreeMap<String, Vec<WorkflowEdgeProgram>> = definition
        .nodes
        .keys()
        .cloned()
        .map(|node| (node, Vec::new()))
        .collect();
    let mut incoming: BTreeMap<String, usize> = definition
        .nodes
        .keys()
        .cloned()
        .map(|node| (node, 0))
        .collect();
    for (edge_index, edge) in definition.edges.iter().enumerate() {
        if !definition.nodes.contains_key(&edge.from) {
            return Err(WorkflowCompileError::DanglingReference(format!(
                "edge source {:?} is not a workflow node",
                edge.from
            ))
            .at(format!("edges.{edge_index}.from")));
        }
        if !definition.nodes.contains_key(&edge.to) {
            return Err(WorkflowCompileError::DanglingReference(format!(
                "edge destination {:?} is not a workflow node",
                edge.to
            ))
            .at(format!("edges.{edge_index}.to")));
        }
        let node = definition.nodes.get(&edge.from).expect("checked above");
        let port = match (node, edge.port.clone()) {
            (WorkflowNodeDefinition::Classifier { classes, .. }, Some(port))
                if classes
                    .iter()
                    .any(|class| WorkflowPort::case(&class.id) == port) =>
            {
                port
            }
            (WorkflowNodeDefinition::Classifier { .. }, _) => {
                return Err(WorkflowCompileError::InvalidBranch(
                    "Classifier edge must name a class ID".into(),
                )
                .at(format!("edges.{edge_index}.port")));
            }
            (WorkflowNodeDefinition::Branch { cases }, Some(port))
                if port == WorkflowPort::False
                    || cases
                        .iter()
                        .any(|case| WorkflowPort::case(&case.id) == port) =>
            {
                port
            }
            (WorkflowNodeDefinition::Branch { .. }, _) => {
                return Err(WorkflowCompileError::InvalidBranch(format!(
                    "Branch node {:?} must use an authored case ID or false (ELSE)",
                    edge.from
                ))
                .at(format!("edges.{edge_index}.port")));
            }
            (
                WorkflowNodeDefinition::Model {
                    on_error: WorkflowModelErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Agent {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Tool {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Code {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                },
                Some(WorkflowPort::Fail),
            ) => WorkflowPort::Fail,
            (WorkflowNodeDefinition::Agent { routes, .. }, Some(port))
                if routes
                    .iter()
                    .any(|route| WorkflowPort::case(&route.id) == port) =>
            {
                port
            }
            (WorkflowNodeDefinition::Review { actions, .. }, Some(port))
                if port == WorkflowPort::case(REVIEW_TIMEOUT_PORT)
                    || actions
                        .iter()
                        .any(|action| WorkflowPort::case(&action.id) == port) =>
            {
                port
            }
            (WorkflowNodeDefinition::Review { .. }, _) => {
                return Err(WorkflowCompileError::InvalidBranch(format!(
                    "Human Input node {:?} must use one of its action IDs or timeout",
                    edge.from
                ))
                .at(format!("edges.{edge_index}.port")));
            }
            (WorkflowNodeDefinition::Agent { routes, .. }, _) if !routes.is_empty() => {
                return Err(WorkflowCompileError::InvalidBranch(format!(
                    "routed Agent node {:?} must use one of its route IDs",
                    edge.from
                ))
                .at(format!("edges.{edge_index}.port")));
            }
            (_, None) => WorkflowPort::Next,
            (_, Some(port)) => {
                return Err(WorkflowCompileError::InvalidField(format!(
                    "ordinary node {:?} cannot have port {port:?}",
                    edge.from
                ))
                .at(format!("edges.{edge_index}.port")));
            }
        };
        let successors = outgoing.get_mut(&edge.from).expect("node exists");
        // Validate in authored order before sorting the immutable runtime edges.
        // The later identical connection owns the duplicate-edge diagnostic.
        if successors
            .iter()
            .any(|successor| successor.port == port && successor.to == edge.to)
        {
            return Err(WorkflowCompileError::InvalidBranch(format!(
                "node {:?} already has this {port:?} connection",
                edge.from
            ))
            .at(format!("edges.{edge_index}.port")));
        }
        successors.push(WorkflowEdgeProgram {
            to: edge.to.clone(),
            port,
        });
        *incoming.get_mut(&edge.to).expect("node exists") += 1;
    }
    for edges in outgoing.values_mut() {
        edges.sort_by(|left, right| left.port.cmp(&right.port).then(left.to.cmp(&right.to)));
    }
    // A fail branch is a path of its own, so it must lead somewhere.
    for (id, node) in &definition.nodes {
        if matches!(
            node,
            WorkflowNodeDefinition::Model {
                on_error: WorkflowModelErrorStrategy::FailBranch,
                ..
            } | WorkflowNodeDefinition::Agent {
                on_error: WorkflowValueErrorStrategy::FailBranch,
                ..
            } | WorkflowNodeDefinition::Tool {
                on_error: WorkflowValueErrorStrategy::FailBranch,
                ..
            } | WorkflowNodeDefinition::Code {
                on_error: WorkflowValueErrorStrategy::FailBranch,
                ..
            }
        ) && !outgoing[id]
            .iter()
            .any(|edge| edge.port == WorkflowPort::Fail)
        {
            return Err(WorkflowCompileError::InvalidField(format!(
                "node {id:?} handles errors with a fail branch but has no fail connection"
            ))
            .at(format!("nodes.{id}.on_error")));
        }
    }
    if incoming[&entry] != 0 {
        return Err(WorkflowCompileError::InvalidEntry(format!(
            "entry node {entry:?} must not have an incoming edge"
        ))
        .at(format!("nodes.{entry}")));
    }
    let topological = topological_order(&outgoing, &incoming)?;
    let reachable = reachable_nodes(&entry, &outgoing);
    if let Some(unreachable) = definition
        .nodes
        .keys()
        .find(|node| !reachable.contains(*node))
    {
        return Err(WorkflowCompileError::Unreachable(unreachable.clone())
            .at(format!("nodes.{unreachable}")));
    }

    let scopes = flow::ActivationScopes::new(definition, &entry, &outgoing, &topological)?;
    Ok(ControlFlow {
        entry,
        scopes,
        outgoing,
        topological,
    })
}

impl WorkflowProgram {
    /// Compiles and validates one definition for one configured identity.
    ///
    /// # Errors
    ///
    /// Returns a [`WorkflowCompileError`] when the definition's graph,
    /// schemas, references, or override authoring are invalid.
    pub fn compile(
        id: WorkflowId,
        definition: WorkflowDefinition,
    ) -> Result<Self, WorkflowCompileError> {
        compile_program(
            id,
            None,
            crate::workflow_file::WorkflowFileDocument::from(definition),
        )
    }

    /// Compiles and validates one full workflow document while preserving its
    /// exact source revision and same-revision presentation metadata as an
    /// immutable admission-bound snapshot.
    ///
    /// # Errors
    ///
    /// Returns a [`WorkflowCompileError`] when the definition or presentation
    /// document violates structural or capacity bounds.
    pub fn compile_document(
        id: WorkflowId,
        source_revision: impl Into<String>,
        document: crate::workflow_file::WorkflowFileDocument,
    ) -> Result<Self, WorkflowCompileError> {
        compile_program(id, Some(source_revision.into()), document)
    }
}

/// Native language shape only; the selected Tool's complete contract is separate.
pub(super) fn validate_tool_arguments_root(schema: &Value) -> Result<(), WorkflowCompileError> {
    if schema_type(schema) != Some("object") {
        return Err(WorkflowCompileError::InvalidField(
            "Tool arguments must construct an object".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_root_schema(
    schema: &Value,
    label: &str,
) -> Result<(), WorkflowCompileError> {
    validate_workflow_schema(schema, label)?;
    if schema_type(schema) != Some("object") {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow {label} schema must have root type object"
        )));
    }
    jsonschema::Validator::new(schema).map_err(|error| {
        WorkflowCompileError::InvalidSchema(format!("workflow {label}: {error}"))
    })?;
    Ok(())
}

/// The deliberately closed JSON Schema vocabulary accepted by Workflow v1.
///
/// This is not the general JSON Schema language. The compiler's compatibility
/// proof is sound because every value-constraining keyword it admits is handled
/// recursively by [`schemas_compatible`]. A new keyword must be added here and
/// to that proof together, otherwise it is rejected as an unsupported schema.
/// `description` and `default` are annotations; a property's `default` is
/// filled in at block entry (see [`apply_schema_defaults`]).
const WORKFLOW_SCHEMA_KEYWORDS: &[&str] = &[
    "additionalProperties",
    "const",
    "default",
    "description",
    "enum",
    "items",
    "maxLength",
    "properties",
    "required",
    "type",
];

const WORKFLOW_SCHEMA_TYPES: &[&str] = &[
    "array", "boolean", "integer", "null", "number", "object", "string",
];

#[allow(clippy::too_many_lines)] // one recursive closed-vocabulary schema validator
pub(super) fn validate_workflow_schema(
    schema: &Value,
    path: &str,
) -> Result<(), WorkflowCompileError> {
    validate_workflow_schema_at(schema, path, 0)
}

#[allow(clippy::too_many_lines)]
fn validate_workflow_schema_at(
    schema: &Value,
    path: &str,
    depth: usize,
) -> Result<(), WorkflowCompileError> {
    if depth > MAX_VALUE_DEPTH {
        return Err(WorkflowCompileError::InvalidSchema(
            "schema depth exceeded".into(),
        ));
    }
    let Some(object) = schema.as_object() else {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} must be an object"
        )));
    };
    if let Some(unsupported) = object
        .keys()
        .find(|keyword| !WORKFLOW_SCHEMA_KEYWORDS.contains(&keyword.as_str()))
    {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} uses unsupported keyword {unsupported:?}; Workflow v1 accepts only {}",
            WORKFLOW_SCHEMA_KEYWORDS.join(", ")
        )));
    }
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} must declare one string type"
        )));
    };
    if object.get("description").is_some_and(|description| {
        description
            .as_str()
            .is_none_or(|text| text.len() > crate::model::structured::SCHEMA_DESCRIPTION_BYTES_MAX)
    }) {
        return Err(WorkflowCompileError::InvalidSchema(
            "schema description must be a string of at most 4096 UTF-8 bytes".into(),
        )
        .at("description"));
    }
    if !WORKFLOW_SCHEMA_TYPES.contains(&kind) {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} has unsupported type {kind:?}"
        )));
    }
    // The current validator's numeric const path compares through f64.
    // Do not use that approximation as a static finite-value proof. Ordinary
    // integer/number schemas and numeric data remain supported.
    if matches!(kind, "integer" | "number")
        && (object.contains_key("const") || object.contains_key("enum"))
    {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?}: numeric const/enum are outside the conservative static subset"
        )));
    }

    if let Some(enum_values) = object.get("enum") {
        let Some(enum_values) = enum_values.as_array() else {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} enum must be a non-empty array"
            )));
        };
        let mut seen = std::collections::HashSet::new();
        if enum_values.is_empty() || enum_values.iter().any(|value| !seen.insert(value)) {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} enum must contain unique values"
            )));
        }
    }
    if let (Some(constant), Some(Value::Array(enum_values))) =
        (object.get("const"), object.get("enum"))
        && !enum_values.contains(constant)
    {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} const must be included in enum"
        )));
    }
    if let Some(max_length) = object.get("maxLength") {
        if kind != "string" {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} maxLength is only valid for string schemas"
            )));
        }
        if !max_length
            .as_u64()
            .is_some_and(|max| (1..=MAX_VALUE_BYTES as u64).contains(&max))
        {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} maxLength must be an integer from 1 to {MAX_VALUE_BYTES}"
            ))
            .at("maxLength"));
        }
    }
    if let Some(default) = object.get("default") {
        let mut constraint = object.clone();
        constraint.remove("default");
        let valid = jsonschema::Validator::new(&Value::Object(constraint))
            .is_ok_and(|validator| validator.is_valid(default));
        if !valid {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} default does not satisfy its own schema"
            ))
            .at("default"));
        }
    }
    for value in object
        .get("const")
        .into_iter()
        .chain(object.get("default"))
        .chain(
            object
                .get("enum")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
    {
        expressions::bounded_value_bytes(value).map_err(|error| {
            WorkflowCompileError::InvalidSchema(format!("schema {path:?} finite value: {error}"))
        })?;
    }

    match kind {
        "object" => {
            if let Some(properties) = object.get("properties") {
                let Some(properties) = properties.as_object() else {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} properties must be an object"
                    )));
                };
                for (name, property) in properties {
                    validate_workflow_schema_at(
                        property,
                        &format!("{path}.properties.{name}"),
                        depth + 1,
                    )
                    .map_err(|e| e.at(format!("properties.{name}")))?;
                }
            }
            if let Some(required) = object.get("required") {
                let Some(required) = required.as_array() else {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} required must be an array of property names"
                    )));
                };
                let empty = serde_json::Map::new();
                let properties = object
                    .get("properties")
                    .and_then(Value::as_object)
                    .unwrap_or(&empty);
                let mut seen = std::collections::HashSet::new();
                for (index, name) in required.iter().enumerate() {
                    let Some(name) = name.as_str() else {
                        return Err(WorkflowCompileError::InvalidSchema(format!(
                            "workflow schema {path}.required[{index}] must be a string"
                        )));
                    };
                    if !properties.contains_key(name) {
                        return Err(WorkflowCompileError::InvalidSchema(format!(
                            "workflow schema {path:?} required field {name:?} has no property schema"
                        )));
                    }
                    if !seen.insert(name) {
                        return Err(WorkflowCompileError::InvalidSchema(format!(
                            "workflow schema {path:?} required contains duplicate field {name:?}"
                        )));
                    }
                }
            }
            if let Some(additional) = object.get("additionalProperties")
                && !additional.is_boolean()
            {
                return Err(WorkflowCompileError::InvalidSchema(format!(
                    "workflow schema {path:?} additionalProperties must be boolean"
                )));
            }
            if object.contains_key("items") {
                return Err(WorkflowCompileError::InvalidSchema(format!(
                    "workflow schema {path:?} items is only valid for array schemas"
                )));
            }
        }
        "array" => {
            if let Some(items) = object.get("items") {
                if !items.is_object() {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} items must be one schema object"
                    )));
                }
                validate_workflow_schema_at(items, &format!("{path}.items"), depth + 1)
                    .map_err(|e| e.at("items"))?;
            }
            for keyword in ["properties", "required", "additionalProperties"] {
                if object.contains_key(keyword) {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} {keyword} is only valid for object schemas"
                    )));
                }
            }
        }
        _ => {
            for keyword in ["properties", "required", "additionalProperties", "items"] {
                if object.contains_key(keyword) {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} {keyword} is incompatible with scalar type {kind:?}"
                    )));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn validate_agent_output(
    output: &WorkflowAgentOutput,
    routes: &[WorkflowAgentRoute],
    node: &str,
) -> Result<(), WorkflowCompileError> {
    if let Some(schema) = output.structured() {
        if !routes.is_empty() && (schema.get("const").is_some() || schema.get("enum").is_some()) {
            return Err(WorkflowCompileError::InvalidSchema(
                "Agent routes cannot add switch to a root const or enum output contract".into(),
            )
            .at("output.schema"));
        }
        validate_root_schema(schema, &format!("Agent {node} output"))
            .map_err(|e| e.at("output.schema"))?;
        if !routes.is_empty()
            && schema
                .pointer(&format!("/properties/{AGENT_ROUTE_FIELD}"))
                .is_some()
        {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "Agent {node:?} routes reserve the {AGENT_ROUTE_FIELD:?} output field"
            ))
            .at("output.schema"));
        }
    }
    Ok(())
}

fn validate_agent(
    task: &[WorkflowValue],
    output: &WorkflowAgentOutput,
    routes: &[WorkflowAgentRoute],
    on_error: &WorkflowValueErrorStrategy,
    available: &SchemaMap,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    if validate_prompt(
        task,
        available,
        node,
        &format!("Agent {node:?} task"),
        "task",
    )? {
        return Err(
            WorkflowCompileError::InvalidField(format!("Agent {node:?} task is empty")).at("task"),
        );
    }
    validate_agent_output(output, routes, node)?;
    if routes.len() > MAX_AGENT_ROUTES {
        return Err(WorkflowCompileError::InvalidBranch(format!(
            "Agent {node:?} has at most {MAX_AGENT_ROUTES} routes"
        ))
        .at("routes"));
    }
    let mut ids = BTreeSet::new();
    for (index, route) in routes.iter().enumerate() {
        if !valid_local_key(&route.id) || is_reserved_port(&route.id) || !ids.insert(&route.id) {
            return Err(WorkflowCompileError::InvalidBranch(
                "route IDs must be unique valid local keys and cannot name a reserved port".into(),
            )
            .at(format!("routes.{index}.id")));
        }
        if route.description.trim().is_empty()
            || route.description.len() > MAX_AGENT_ROUTE_DESCRIPTION_BYTES
        {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Agent {node:?} route descriptions tell the child when to choose them, within {MAX_AGENT_ROUTE_DESCRIPTION_BYTES} bytes"
            ))
            .at(format!("routes.{index}.description")));
        }
    }
    if let WorkflowValueErrorStrategy::DefaultValue { value } = on_error {
        let schema = agent_value_schema(output, routes);
        if !jsonschema::Validator::new(&schema).is_ok_and(|validator| validator.is_valid(value)) {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Agent {node:?} default value must satisfy the node's own value contract"
            ))
            .at("on_error.value"));
        }
    }
    Ok(())
}

/// Ports a Branch case or an Agent route cannot be named after.
/// As Dify's End: each output variable a unique field read from a variable.
/// The value is the object those fields make, and its schema.
fn return_output(
    outputs: &[WorkflowReturnOutput],
    available: &SchemaMap,
    node: &str,
) -> Result<(WorkflowValue, Value), WorkflowCompileError> {
    let (mut fields, mut properties) = (BTreeMap::new(), serde_json::Map::new());
    for (index, output) in outputs.iter().enumerate() {
        if !valid_local_key(&output.variable) || fields.contains_key(&output.variable) {
            return Err(WorkflowCompileError::InvalidField(
                "output variables must be unique valid local keys".into(),
            )
            .at(format!("outputs.{index}.variable")));
        }
        let value = WorkflowValue::Reference {
            path: output.value_selector.clone(),
        };
        let schema = value_schema(&value, available, node, 0)
            .map_err(|e| e.at(format!("outputs.{index}.value_selector")))?;
        properties.insert(output.variable.clone(), schema);
        fields.insert(output.variable.clone(), value);
    }
    let required = fields.keys().cloned().collect::<Vec<_>>();
    Ok((
        WorkflowValue::Object { fields },
        serde_json::json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}),
    ))
}

/// The workflow's output: every Return's fields, required when every Return
/// has it. A field two Returns share must have one type.
fn merge_return_schemas(returned: &[Value]) -> Result<Value, WorkflowCompileError> {
    let Some((first, rest)) = returned.split_first() else {
        return Ok(empty_object_schema());
    };
    let mut merged = first.clone();
    for schema in rest {
        for (key, property) in schema["properties"].as_object().into_iter().flatten() {
            let properties = merged["properties"].as_object_mut().expect("object schema");
            // The wider of the two, when one admits every value of the other.
            match properties.get(key) {
                Some(existing) if schemas_compatible(property, existing) => {}
                Some(existing) if !schemas_compatible(existing, property) => {
                    return Err(WorkflowCompileError::IncompatibleReference(format!(
                        "Return nodes give output {key:?} different types"
                    )));
                }
                _ => {
                    properties.insert(key.clone(), property.clone());
                }
            }
        }
        let required = merged["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|key| {
                schema["required"]
                    .as_array()
                    .is_some_and(|keys| keys.contains(key))
            })
            .cloned()
            .collect::<Vec<_>>();
        merged["required"] = Value::Array(required);
    }
    Ok(merged)
}

fn validate_review(
    content: &[WorkflowValue],
    actions: &[WorkflowReviewAction],
    timeout_ms: u64,
    available: &SchemaMap,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    let fixed_chars = content
        .iter()
        .filter_map(|part| match part {
            WorkflowValue::Literal {
                value: Value::String(text),
            } => Some(text.chars().count()),
            _ => None,
        })
        .sum::<usize>();
    let limit = crate::events::interaction::MAX_QUESTION_TEXT_CHARS;
    if fixed_chars > limit {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Human Input {node:?} content exceeds {limit} Unicode characters"
        ))
        .at("content"));
    }
    if validate_prompt(
        content,
        available,
        node,
        &format!("Human Input {node:?} content"),
        "content",
    )? {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Human Input {node:?} content is empty"
        ))
        .at("content"));
    }
    if actions.is_empty() || actions.len() > MAX_REVIEW_ACTIONS {
        return Err(WorkflowCompileError::InvalidBranch(format!(
            "Human Input {node:?} has 1..={MAX_REVIEW_ACTIONS} actions"
        ))
        .at("actions"));
    }
    let (mut ids, mut titles) = (BTreeSet::new(), BTreeSet::new());
    for (index, action) in actions.iter().enumerate() {
        if !valid_local_key(&action.id)
            || is_reserved_port(&action.id)
            || action.id == REVIEW_TIMEOUT_PORT
            || !ids.insert(&action.id)
        {
            return Err(WorkflowCompileError::InvalidBranch(
                "action IDs must be unique valid local keys and cannot name a reserved port".into(),
            )
            .at(format!("actions.{index}.id")));
        }
        let title = action.title.trim();
        if title.is_empty()
            || action.title.chars().count() > crate::events::interaction::MAX_OPTION_LABEL_CHARS
            || !titles.insert(title)
        {
            return Err(WorkflowCompileError::InvalidField(format!(
                "action titles must be unique non-empty text of at most {} characters",
                crate::events::interaction::MAX_OPTION_LABEL_CHARS
            ))
            .at(format!("actions.{index}.title")));
        }
    }
    if timeout_ms == 0 || timeout_ms > 86_400_000 {
        return Err(
            WorkflowCompileError::InvalidField("timeout_ms must be 1..=86400000".into())
                .at("timeout_ms"),
        );
    }
    Ok(())
}

pub(super) fn is_reserved_port(id: &str) -> bool {
    matches!(id, "false" | "next" | "fail")
}

/// Check one prompt's parts, fixed text or references, against its bounds and
/// the available scope. Returns whether it has no text and no variable.
pub(super) fn validate_prompt(
    parts: &[WorkflowValue],
    available: &SchemaMap,
    node: &str,
    what: &str,
    at: &str,
) -> Result<bool, WorkflowCompileError> {
    if parts.len() > MAX_MODEL_MESSAGE_PARTS {
        return Err(WorkflowCompileError::InvalidField(format!(
            "{what} exceeds {MAX_MODEL_MESSAGE_PARTS} parts"
        ))
        .at(at));
    }
    let mut text = 0usize;
    for (position, part) in parts.iter().enumerate() {
        match part {
            WorkflowValue::Literal {
                value: Value::String(fixed),
            } => text += fixed.len(),
            WorkflowValue::Reference { .. } => {
                value_schema(part, available, node, 0)
                    .map_err(|e| e.at(format!("{at}.{position}")))?;
            }
            _ => {
                return Err(WorkflowCompileError::InvalidField(format!(
                    "{what} content is text or references only"
                ))
                .at(format!("{at}.{position}")));
            }
        }
    }
    if text > MAX_MODEL_MESSAGE_BYTES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "{what} text exceeds {MAX_MODEL_MESSAGE_BYTES} bytes"
        ))
        .at(at));
    }
    Ok(parts.iter().all(|part| {
        matches!(part, WorkflowValue::Literal { value: Value::String(fixed) } if fixed.trim().is_empty())
    }))
}

fn validate_model(
    model: &crate::model::session::DirectModelPolicy,
    messages: &[WorkflowModelMessage],
    output: &WorkflowModelOutput,
    available: &SchemaMap,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    validate_model_policy(model).map_err(|error| error.at("model"))?;
    if messages.is_empty() || messages.len() > MAX_MODEL_MESSAGES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Model {node:?} needs a system message and at most {MAX_MODEL_MESSAGES} messages"
        ))
        .at("messages"));
    }
    for (index, message) in messages.iter().enumerate() {
        let at = format!("messages.{index}");
        if (index == 0) != (message.role == WorkflowModelRole::System) {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Model {node:?} message {index} must be {}",
                if index == 0 {
                    "the system message"
                } else {
                    "a user or assistant message"
                }
            ))
            .at(format!("{at}.role")));
        }
        let empty = validate_prompt(
            &message.content,
            available,
            node,
            &format!("Model {node:?} message {index}"),
            &format!("{at}.content"),
        )?;
        if index > 0 && empty {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Model {node:?} message {index} is empty"
            ))
            .at(format!("{at}.content")));
        }
    }
    if !messages
        .iter()
        .any(|message| message.role == WorkflowModelRole::User)
    {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Model {node:?} needs at least one user message"
        ))
        .at("messages"));
    }
    validate_model_output(output, node)
}

fn validate_retry(
    retry: Option<&WorkflowRetry>,
    kind: &str,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    if let Some(retry) = retry
        && (!(1..=MAX_RETRIES).contains(&retry.max_retries)
            || !RETRY_INTERVAL_MS.contains(&retry.interval_ms))
    {
        return Err(WorkflowCompileError::InvalidField(format!(
            "{kind} {node:?} retries 1..={MAX_RETRIES} times, 100..=5000 ms apart"
        ))
        .at("retry"));
    }
    Ok(())
}

fn validate_model_failure(
    retry: Option<&WorkflowRetry>,
    on_error: &WorkflowModelErrorStrategy,
    output: &WorkflowModelOutput,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    validate_retry(retry, "Model", node)?;
    let WorkflowModelErrorStrategy::DefaultValue {
        text,
        structured_output,
    } = on_error
    else {
        return Ok(());
    };
    if text.len() > MAX_MODEL_MESSAGE_BYTES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Model {node:?} default text exceeds {MAX_MODEL_MESSAGE_BYTES} bytes"
        ))
        .at("on_error.text"));
    }
    match (output, structured_output) {
        (WorkflowModelOutput::Text {}, None) => Ok(()),
        (WorkflowModelOutput::JsonSchema { schema }, Some(value))
            if jsonschema::Validator::new(schema).is_ok_and(|validator| validator.is_valid(value)) =>
        {
            Ok(())
        }
        _ => Err(WorkflowCompileError::InvalidField(format!(
            "Model {node:?} default structured_output must satisfy its declared schema, and only a declared schema has one"
        ))
        .at("on_error.structured_output")),
    }
}

/// Shared output-contract proof, independent of unfinished messages.
pub(super) fn validate_model_output(
    output: &WorkflowModelOutput,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    if let WorkflowModelOutput::JsonSchema { schema } = output {
        expressions::bounded_value_bytes(schema).map_err(|_| {
            WorkflowCompileError::InvalidSchema(
                "Model output schema exceeds its byte/depth bound".into(),
            )
            .at("output.schema")
        })?;
        validate_root_schema(schema, &format!("Model {node} output"))
            .map_err(|e| e.at("output.schema"))?;
        crate::model::structured::ResponseSchema::new(
            "workflow_model".into(),
            schema.clone(),
            MAX_VALUE_BYTES,
        )
        .map_err(|error| WorkflowCompileError::InvalidSchema(error.message).at("output.schema"))?;
    }
    Ok(())
}

/// Structural policy validity is independent of model resources or credentials.
/// Protocol-specific ownership, Profile defaults and output maxima are checked
/// by the existing model resolver against the invoking Attempt's registry.
pub(super) fn validate_model_policy(
    policy: &crate::model::session::DirectModelPolicy,
) -> Result<(), WorkflowCompileError> {
    use crate::model::session::DirectModelPolicy;
    let DirectModelPolicy::Explicit {
        model,
        profile,
        request_params,
        max_output_tokens,
    } = policy
    else {
        return Ok(());
    };
    if model.as_str().len() > MAX_RUN_ID_COMPONENT_BYTES
        || crate::model::catalog::ModelRef::parse(model.as_str()).is_err()
    {
        return Err(WorkflowCompileError::InvalidField(
            "Model selection has an invalid or oversized identity".into(),
        )
        .at("model"));
    }
    if let Some(profile) = profile
        && (profile.as_str().len() > MAX_RUN_ID_COMPONENT_BYTES
            || crate::model::catalog::ModelProfileId::parse(profile.as_str()).is_err())
    {
        return Err(WorkflowCompileError::InvalidField(
            "Model Profile has an invalid or oversized identity".into(),
        )
        .at("profile"));
    }
    expressions::bounded_value_bytes(&Value::Object(request_params.clone())).map_err(|_| {
        WorkflowCompileError::InvalidField(
            "Model request parameters exceed their byte/depth bound".into(),
        )
        .at("request_params")
    })?;
    crate::model::invocation::validate_request_params_numbers(
        request_params,
        crate::model::invocation::RequestParamsLayer::SessionOverrides,
    )
    .map_err(|error| WorkflowCompileError::InvalidField(error.to_string()).at("request_params"))?;
    if *max_output_tokens == Some(0) {
        return Err(WorkflowCompileError::InvalidField(
            "Model max_output_tokens must be positive".into(),
        )
        .at("max_output_tokens"));
    }
    Ok(())
}

fn topological_order(
    outgoing: &BTreeMap<String, Vec<WorkflowEdgeProgram>>,
    incoming: &BTreeMap<String, usize>,
) -> Result<Vec<String>, WorkflowCompileError> {
    let mut counts = incoming.clone();
    let mut queue = incoming
        .iter()
        .filter_map(|(node, count)| (*count == 0).then_some(node.clone()))
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(outgoing.len());
    while let Some(node) = queue.pop_front() {
        order.push(node.clone());
        for edge in &outgoing[&node] {
            let count = counts.get_mut(&edge.to).expect("edge destination exists");
            *count -= 1;
            if *count == 0 {
                queue.push_back(edge.to.clone());
            }
        }
    }
    if order.len() != outgoing.len() {
        // BTreeMap order selects the lexicographically smallest Kahn residual,
        // independent of authored edge order. It may be downstream of a cycle.
        let node = counts
            .iter()
            .find_map(|(node, count)| (*count > 0).then_some(node.clone()))
            .expect("incomplete topological order has residual nodes");
        return Err(WorkflowCompileError::Cycle { node: node.clone() }.at(format!("nodes.{node}")));
    }
    Ok(order)
}

fn reachable_nodes(
    entry: &str,
    outgoing: &BTreeMap<String, Vec<WorkflowEdgeProgram>>,
) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([entry.to_owned()]);
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node.clone()) {
            continue;
        }
        queue.extend(outgoing[&node].iter().map(|edge| edge.to.clone()));
    }
    seen
}

#[derive(Debug, Clone, Default)]
pub(super) struct SchemaMap(pub(super) BTreeMap<String, Value>);

impl SchemaMap {
    /// A block's entry environment: `args` after defaults are filled in.
    pub(super) fn from_schema(schema: &Value) -> Self {
        Self(BTreeMap::from([(
            "args".into(),
            schema_with_defaults_required(schema),
        )]))
    }

    pub(super) fn with_prefix(&self, prefix: &str, schema: &Value) -> Self {
        let mut result = self.clone();
        // Keep the complete frozen producer schema. Reducing it to required
        // fields would erase constraints such as enum/const and make a later
        // binding appear compatible merely because both values are objects.
        result.0.insert(prefix.to_owned(), schema.clone());
        result
    }
}

pub(super) fn schema_type(schema: &Value) -> Option<&str> {
    schema.get("type").and_then(Value::as_str)
}

fn schema_required(schema: &Value) -> BTreeSet<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// The schema of a block input after [`apply_schema_defaults`]: every
/// property with a `default` is present, so it is required.
fn schema_with_defaults_required(schema: &Value) -> Value {
    let Some(object) = schema.as_object() else {
        return schema.clone();
    };
    let mut result = object.clone();
    if let Some(Value::Object(properties)) = result.get_mut("properties") {
        let defaulted = properties
            .iter()
            .filter(|(_, property)| property.get("default").is_some())
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for property in properties.values_mut() {
            *property = schema_with_defaults_required(property);
        }
        if !defaulted.is_empty() {
            let mut required = schema_required(schema)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            for name in defaulted {
                if !required.contains(&name) {
                    required.push(name);
                }
            }
            result.insert(
                "required".into(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }
    }
    if let Some(items) = result.get_mut("items") {
        *items = schema_with_defaults_required(items);
    }
    Value::Object(result)
}

pub(super) fn finite_schema_values(schema: &Value) -> Option<&[Value]> {
    if let Some(constant) = schema.get("const") {
        if let Some(Value::Array(enum_values)) = schema.get("enum") {
            return enum_values
                .contains(constant)
                .then_some(std::slice::from_ref(constant));
        }
        return Some(std::slice::from_ref(constant));
    }
    schema
        .get("enum")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

pub(super) fn schema_forbids_property(schema: &Value, key: &str) -> bool {
    schema_type(schema) == Some("object")
        && schema.get("additionalProperties") == Some(&Value::Bool(false))
        && schema
            .get("properties")
            .and_then(|properties| properties.get(key))
            .is_none()
}

#[derive(Clone, Copy)]
pub(super) enum SchemaProofStep {
    Structure,
    CompileValidator,
    ValidateValue,
}

pub(super) fn schemas_compatible(actual: &Value, expected: &Value) -> bool {
    schemas_compatible_with(actual, expected, &mut |_, _, _| {
        Ok::<(), std::convert::Infallible>(())
    })
    .unwrap_or_else(|error| match error {})
}

/// Derived contracts use a pure union for values such as List's nullable
/// first/last record. User-written schemas still use the closed vocabulary
/// admitted by `validate_workflow_schema`; union siblings are not discarded.
fn schema_alternatives(schema: &Value) -> Option<&[Value]> {
    let object = schema.as_object()?;
    if object.len() != 1 {
        return None;
    }
    object
        .get("anyOf")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

/// Every recursive proof and finite-value validation is admitted before work.
/// Compilation supplies an unlimited policy; authoring uses its request budget.
#[allow(clippy::too_many_lines)]
pub(super) fn schemas_compatible_with<E>(
    actual: &Value,
    expected: &Value,
    admit: &mut impl FnMut(&Value, &Value, SchemaProofStep) -> Result<(), E>,
) -> Result<bool, E> {
    admit(actual, expected, SchemaProofStep::Structure)?;
    if let Some(values) = finite_schema_values(actual) {
        if values.is_empty() {
            return Ok(false);
        }
        admit(actual, expected, SchemaProofStep::CompileValidator)?;
        let Ok(expected_validator) = jsonschema::Validator::new(expected) else {
            return Ok(false);
        };
        for value in values {
            admit(value, expected, SchemaProofStep::ValidateValue)?;
            if !expected_validator.is_valid(value) {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let Some(alternatives) = schema_alternatives(actual) {
        if alternatives.is_empty() {
            return Ok(false);
        }
        for alternative in alternatives {
            if !schemas_compatible_with(alternative, expected, admit)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let Some(alternatives) = schema_alternatives(expected) {
        for alternative in alternatives {
            if schemas_compatible_with(actual, alternative, admit)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if finite_schema_values(expected).is_some() {
        return Ok(false);
    }
    let Some(actual_type) = schema_type(actual) else {
        return Ok(false);
    };
    let Some(expected_type) = schema_type(expected) else {
        return Ok(false);
    };
    if actual_type != expected_type && !(actual_type == "integer" && expected_type == "number") {
        return Ok(false);
    }
    if let Some(expected_max) = expected.get("maxLength").and_then(Value::as_u64)
        && actual
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_none_or(|actual_max| actual_max > expected_max)
    {
        return Ok(false);
    }
    if expected_type == "object" {
        let empty = serde_json::Map::new();
        let expected_properties = expected
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let actual_properties = actual
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let expected_required = schema_required(expected);
        let actual_required = schema_required(actual);
        for key in &expected_required {
            if !actual_properties.contains_key(*key)
                || !expected_properties.contains_key(*key)
                || !actual_required.contains(key)
            {
                return Ok(false);
            }
        }
        // Required members recurse here exactly once, together with optional
        // declared members; a required-property chain must not grow as 2^depth.
        for (key, expected_property) in expected_properties {
            match actual_properties.get(key) {
                Some(actual_property) => {
                    if !schemas_compatible_with(actual_property, expected_property, admit)? {
                        return Ok(false);
                    }
                }
                None if !actual
                    .get("additionalProperties")
                    .is_some_and(|value| value == &Value::Bool(false)) =>
                {
                    return Ok(false);
                }
                None => {}
            }
        }
        if expected
            .get("additionalProperties")
            .is_some_and(|value| value == &Value::Bool(false))
            && (!actual
                .get("additionalProperties")
                .is_some_and(|value| value == &Value::Bool(false))
                || actual_properties
                    .keys()
                    .any(|key| schema_forbids_property(expected, key)))
        {
            return Ok(false);
        }
    }
    if expected_type == "array" {
        match (actual.get("items"), expected.get("items")) {
            (Some(actual_items), Some(expected_items)) => {
                if !schemas_compatible_with(actual_items, expected_items, admit)? {
                    return Ok(false);
                }
            }
            (None, Some(_)) => return Ok(false),
            _ => {}
        }
    }
    Ok(true)
}
