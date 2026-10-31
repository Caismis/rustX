//! Inert candidate analysis using the compiler's control-flow, lexical-reference
//! and structural compatibility proofs. This module cannot construct a program.
use super::expressions::{InferenceError, Schema, SchemaBudget, SchemaInference};
use super::{
    BTreeMap, SchemaMap, Value, WorkflowBlock, WorkflowCompileError, WorkflowDefinition,
    WorkflowNodeDefinition, WorkflowPredicate, WorkflowValue, control_flow, validate_root_schema,
    validate_workflow_schema,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeSet;
mod selections;
pub use selections::{WorkflowModelPolicy, WorkflowToolSelector};
#[cfg(test)]
mod tests;

pub const MAX_ANALYSIS_BYTES: usize = 256 * 1024;
pub const MAX_ANALYSIS_CHOICES: usize = 256;
pub const MAX_ANALYSIS_VISITS: usize = 2048;

/// Structured lexical address of a Loop or Iteration body.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBlockStep {
    pub node: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNodeAddress {
    pub block: Vec<WorkflowBlockStep>,
    pub node: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowConsumerField {
    /// A variable in one Model prompt message. Every part of a message shares
    /// its scope, so this also answers for a variable not inserted yet.
    Message {
        index: u32,
    },
    /// A variable in an Agent's task, answered as for a Model message.
    Task,
    Arguments,
    /// A variable in a Human Input's content, answered as for a Model message.
    ReviewContent,
    Condition {
        case_id: String,
    },
    /// Select a comparable variable independently of the current counterpart.
    ConditionVariable {
        case_id: String,
    },
    ReturnOutput,
    /// The value of one Loop variable.
    LoopVariable {
        index: u32,
    },
    /// The Loop's break condition, read in its body's scope.
    BreakCondition,
    /// Select a comparable variable of the break condition.
    BreakConditionVariable,
    IterationIterator,
    /// The Iteration's output, read in its body's scope.
    IterationOutput,
    /// The operand of one Variable Assigner write.
    AssignerValue {
        index: u32,
    },
}

/// Exact authored position within a value construction or predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowExpressionStep {
    Field { key: String },
    Item { index: u32 },
    Value,
    Left,
    Right,
    Predicate,
    PredicateAt { index: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowConsumerAddress {
    pub address: WorkflowNodeAddress,
    pub field: WorkflowConsumerField,
    #[serde(default)]
    pub path: Vec<WorkflowExpressionStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAnalysisQuery {
    pub consumer: WorkflowConsumerAddress,
    #[serde(default)]
    pub search: String,
    /// An existing/manual reference to classify even if it is absent from the
    /// declaration catalog. This never creates fallback or indexing semantics.
    #[serde(default)]
    pub reference: Option<Vec<String>>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReferenceAvailability {
    Available,
    Unavailable,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReferenceCompatibility {
    Compatible,
    Incompatible,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOutputDeclaration {
    pub producer: WorkflowNodeAddress,
    /// None means an invalid/incomplete output contract, never an empty schema.
    #[schemars(with = "Option<BTreeMap<String, Value>>")]
    pub schema: Option<Value>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReferenceChoice {
    /// `args`/`result` have no executable producer address.
    pub producer: Option<WorkflowNodeAddress>,
    pub path: Vec<String>,
    #[schemars(with = "Option<BTreeMap<String, Value>>")]
    pub schema: Option<Value>,
    pub availability: WorkflowReferenceAvailability,
    pub compatibility: WorkflowReferenceCompatibility,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCandidateAnalysis {
    pub outputs: Vec<WorkflowOutputDeclaration>,
    pub references: Vec<WorkflowReferenceChoice>,
    pub selected: Option<WorkflowReferenceChoice>,
    /// Lexical proof failure or invalid consumer; never an execution verdict.
    pub diagnostic: Option<String>,
    pub incomplete: bool,
    pub truncated: bool,
}

/// Resolve an address without interpreting local node ids as global identities.
#[must_use]
pub fn block_at<'a>(
    root: &'a WorkflowBlock,
    path: &[WorkflowBlockStep],
) -> Option<&'a WorkflowBlock> {
    let mut block = root;
    for step in path {
        block = match block.nodes.get(&step.node)? {
            WorkflowNodeDefinition::Loop { body, .. }
            | WorkflowNodeDefinition::Iteration { body, .. } => body,
            _ => return None,
        };
    }
    Some(block)
}

/// The same declared/derived contract the compiler commits after each producer.
/// No binding, dependency or readiness is implied by a declaration.
/// # Errors
/// Rejects an unsupported or incomplete closed-schema declaration.
pub fn declared_output(
    node: &WorkflowNodeDefinition,
) -> Result<Option<Value>, WorkflowCompileError> {
    let schema = match node {
        WorkflowNodeDefinition::Model { output, .. } => {
            super::validate_model_output(output, "candidate")?;
            output.schema()
        }
        WorkflowNodeDefinition::Agent { output, routes, .. } => {
            if let Some(schema) = output.structured() {
                validate_root_schema(schema, "output.schema")?;
            }
            super::agent_value_schema(output, routes)
        }
        WorkflowNodeDefinition::Tool { result, .. } => {
            let schema = result.schema();
            validate_workflow_schema(&schema, "result")?;
            schema
        }
        WorkflowNodeDefinition::Review { actions, .. } => super::review::result_schema(actions),
        WorkflowNodeDefinition::Loop { loop_variables, .. } => {
            super::loop_variables_schema(loop_variables)
        }
        WorkflowNodeDefinition::Iteration {
            output,
            error_handle_mode,
            flatten_output,
            body,
            ..
        } => super::execution::iteration_result_schema(
            produced(output, body).as_ref(),
            *error_handle_mode,
            *flatten_output,
        ),
        WorkflowNodeDefinition::Start {}
        | WorkflowNodeDefinition::Branch { .. }
        | WorkflowNodeDefinition::Return { .. }
        | WorkflowNodeDefinition::LoopEnd {}
        | WorkflowNodeDefinition::Assigner { .. } => {
            return Ok(None);
        }
    };
    Ok(Some(schema))
}

/// What an Iteration's rounds produce, when its output names a field of a
/// body node; the compiler also resolves other variables of the body scope.
fn produced(output: &WorkflowValue, body: &WorkflowBlock) -> Option<Value> {
    let WorkflowValue::Reference { path } = output else {
        return None;
    };
    let (producer, fields) = path.split_first()?;
    let mut schema = declared_output(body.nodes.get(producer)?).ok()??;
    for field in fields {
        schema = schema.get("properties")?.get(field)?.clone();
    }
    Some(schema)
}

fn valid_key(key: &str) -> bool {
    super::expressions::valid_local_key(key)
}
fn valid_address(address: &WorkflowNodeAddress) -> bool {
    address.block.len() <= super::MAX_BLOCK_DEPTH
        && valid_key(&address.node)
        && address.block.iter().all(|step| valid_key(&step.node))
}

impl WorkflowAnalysisQuery {
    /// # Errors
    /// Rejects query depth/count/text beyond the finite authoring envelope.
    pub fn validate(&self) -> Result<(), &'static str> {
        if serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > 16 * 1024) {
            return Err("analysis query exceeds 16 KiB");
        }
        if !valid_address(&self.consumer.address) || self.consumer.path.len() > super::MAX_VALUE_DEPTH
            || self.search.len() > 256 || self.limit == 0 || self.limit > MAX_ANALYSIS_CHOICES
            || self.reference.as_ref().is_some_and(|path| path.is_empty() || path.len() > super::MAX_REFERENCE_COMPONENTS || path.iter().any(|s| s.is_empty() || s.len() > 64))
            || self.consumer.path.iter().any(|step| matches!(step, WorkflowExpressionStep::Field { key } if key.is_empty() || key.len() > 64)) {
            return Err("analysis query exceeds address, text, depth or result bounds");
        }
        match &self.consumer.field {
            WorkflowConsumerField::Condition { case_id }
            | WorkflowConsumerField::ConditionVariable { case_id }
                if !valid_key(case_id) =>
            {
                Err("invalid case identity")
            }
            _ => Ok(()),
        }
    }
}

/// Validate candidate traversal independently of whether the graph is finished.
/// # Errors
/// Rejects aggregate executable bytes, block depth or node count over native bounds.
pub fn validate_candidate(definition: &WorkflowDefinition) -> Result<(), &'static str> {
    fn walk(block: &WorkflowBlock, depth: usize, count: &mut usize) -> Result<(), &'static str> {
        *count += block.nodes.len();
        if depth > super::MAX_BLOCK_DEPTH || *count > super::MAX_WORKFLOW_NODES {
            return Err("candidate exceeds native block depth or node bound");
        }
        for node in block.nodes.values() {
            if let WorkflowNodeDefinition::Loop { body, .. }
            | WorkflowNodeDefinition::Iteration { body, .. } = node
            {
                walk(body, depth + 1, count)?;
            }
        }
        Ok(())
    }
    if serde_json::to_vec(definition).map_or(true, |bytes| bytes.len() > super::MAX_WORKFLOW_BYTES)
    {
        return Err("executable candidate exceeds 512 KiB");
    }
    walk(&definition.block, 0, &mut 0)
}

/// The position of a Model message variable, whichever one is selected.
static UNSET_REFERENCE: WorkflowValue = WorkflowValue::Reference { path: Vec::new() };
/// A Loop's break condition before its first comparison.
static NO_CONDITION: WorkflowPredicate = WorkflowPredicate::And {
    predicates: Vec::new(),
};

#[derive(Clone, Copy)]
enum Expression<'a> {
    Value(&'a WorkflowValue),
    Predicate(&'a WorkflowPredicate),
}
struct Consumer<'a> {
    expression: Expression<'a>,
    expected: Option<Cow<'a, Value>>,
    necessary: NecessaryCondition,
    /// Read in the node's body after a round, not where the node is.
    result_scope: bool,
    arrays: Vec<ArrayContext<'a>>,
}

struct ArrayContext<'a> {
    item: &'a WorkflowValue,
    /// The selected slot is a hole; unchanged siblings are inferred once.
    siblings: Option<Vec<Schema<'a>>>,
}

struct ChoiceInference<'a> {
    schemas: SchemaInference<'a>,
    arrays: BTreeMap<usize, (WorkflowReferenceCompatibility, Option<String>)>,
    destinations: BTreeMap<usize, WorkflowReferenceCompatibility>,
}

#[derive(Clone, Copy)]
enum NecessaryCondition {
    None,
    ToolArgumentsObject,
    EqualityOperand,
    ForbiddenField,
}

#[derive(Clone, Copy)]
enum DestinationContract<'a> {
    Unknown,
    Complete(&'a Value),
    ToolArgumentsObject,
    Equality(Option<&'a Value>),
    ForbiddenField,
}

#[derive(Clone, Copy)]
struct ConsumerContract<'a> {
    destination: DestinationContract<'a>,
    arrays: &'a [ArrayContext<'a>],
    selected: Option<&'a WorkflowValue>,
}

fn consumer<'a>(
    block: &'a WorkflowBlock,
    address: &WorkflowConsumerAddress,
) -> Option<Consumer<'a>> {
    use WorkflowConsumerField as F;
    use WorkflowNodeDefinition as N;
    let node = block.nodes.get(&address.address.node)?;
    let (expression, expected, result_scope) = match (node, &address.field) {
        (N::Model { messages, .. }, F::Message { index }) => {
            messages.get(*index as usize)?;
            (Expression::Value(&UNSET_REFERENCE), None, false)
        }
        (N::Agent { .. }, F::Task)
        | (N::Review { .. }, F::ReviewContent)
        | (N::Return { .. }, F::ReturnOutput) => (Expression::Value(&UNSET_REFERENCE), None, false),
        (N::Tool { arguments, .. }, F::Arguments) => (Expression::Value(arguments), None, false),
        (N::Branch { cases }, F::Condition { case_id } | F::ConditionVariable { case_id }) => (
            Expression::Predicate(&cases.iter().find(|case| &case.id == case_id)?.condition),
            None,
            false,
        ),
        (N::Loop { loop_variables, .. }, F::LoopVariable { index }) => {
            let variable = loop_variables.get(*index as usize)?;
            (
                Expression::Value(&variable.value),
                Some(Cow::Owned(variable.var_type.schema())),
                false,
            )
        }
        (
            N::Loop {
                break_condition, ..
            },
            F::BreakCondition | F::BreakConditionVariable,
        ) => (
            Expression::Predicate(break_condition.as_ref().unwrap_or(&NO_CONDITION)),
            None,
            true,
        ),
        (N::Iteration { iterator, .. }, F::IterationIterator) => (
            Expression::Value(iterator),
            Some(Cow::Owned(serde_json::json!({"type": "array"}))),
            false,
        ),
        (N::Iteration { output, .. }, F::IterationOutput) => {
            (Expression::Value(output), None, true)
        }
        (N::Assigner { items }, F::AssignerValue { index }) => {
            let item = items.get(*index as usize)?;
            (
                Expression::Value(item.value.as_ref().unwrap_or(&UNSET_REFERENCE)),
                None,
                false,
            )
        }
        _ => return None,
    };
    Some(Consumer {
        expression,
        expected: expected.filter(|schema| validate_workflow_schema(schema, "expected").is_ok()),
        necessary: if matches!((node, &address.field), (N::Tool { .. }, F::Arguments)) {
            NecessaryCondition::ToolArgumentsObject
        } else {
            NecessaryCondition::None
        },
        result_scope,
        arrays: Vec::new(),
    })
}

fn position<'a>(
    mut current: Consumer<'a>,
    path: &[WorkflowExpressionStep],
    inference: &mut SchemaInference<'a>,
) -> Option<Consumer<'a>> {
    use WorkflowExpressionStep as S;
    for step in path {
        // No descendant can repair a field forbidden by an enclosing contract.
        if !matches!(current.necessary, NecessaryCondition::ForbiddenField) {
            current.necessary = NecessaryCondition::None;
        }
        current.expression = match (current.expression, step) {
            (Expression::Value(WorkflowValue::Object { fields }), S::Field { key }) => {
                if current
                    .expected
                    .as_ref()
                    .is_some_and(|schema| super::schema_forbids_property(schema, key))
                {
                    current.necessary = NecessaryCondition::ForbiddenField;
                }
                current.expected = current.expected.and_then(|schema| match schema {
                    Cow::Borrowed(schema) => schema.get("properties")?.get(key).map(Cow::Borrowed),
                    Cow::Owned(schema) => {
                        schema.get("properties")?.get(key).cloned().map(Cow::Owned)
                    }
                });
                Expression::Value(fields.get(key)?)
            }
            (Expression::Value(WorkflowValue::Array { items }), S::Item { index }) => {
                let index = *index as usize;
                let item = items.get(index)?;
                let siblings = inference.siblings(items, index).ok();
                current.arrays.push(ArrayContext { item, siblings });
                current.expected = current.expected.and_then(|schema| match schema {
                    Cow::Borrowed(schema) => schema.get("items").map(Cow::Borrowed),
                    Cow::Owned(schema) => schema.get("items").cloned().map(Cow::Owned),
                });
                Expression::Value(item)
            }
            (Expression::Value(WorkflowValue::Template { parts }), S::Item { index }) => {
                current.expected = None;
                Expression::Value(parts.get(*index as usize)?)
            }
            (Expression::Predicate(WorkflowPredicate::Boolean { value }), S::Value) => {
                current.expected = Some(Cow::Owned(serde_json::json!({"type":"boolean"})));
                Expression::Value(value)
            }
            (
                Expression::Predicate(
                    WorkflowPredicate::Equal { left, right }
                    | WorkflowPredicate::NotEqual { left, right },
                ),
                S::Left | S::Right,
            ) => {
                let (selected, other) = if matches!(step, S::Left) {
                    (left, right)
                } else {
                    (right, left)
                };
                let kind = super::expressions::intrinsic_value_type(other)
                    .map(str::to_owned)
                    .or_else(|| match other {
                        WorkflowValue::Reference { path } => inference
                            .resolve(path)
                            .ok()
                            .and_then(|schema| super::schema_type(schema).map(str::to_owned)),
                        _ => None,
                    });
                current.expected = kind.map(|kind| Cow::Owned(serde_json::json!({"type":kind})));
                current.necessary = NecessaryCondition::EqualityOperand;
                Expression::Value(selected)
            }
            (
                Expression::Predicate(
                    WorkflowPredicate::Contains { left, right }
                    | WorkflowPredicate::NotContains { left, right }
                    | WorkflowPredicate::StartsWith { left, right }
                    | WorkflowPredicate::EndsWith { left, right },
                ),
                S::Left | S::Right,
            ) => {
                current.expected = Some(Cow::Owned(serde_json::json!({"type":"string"})));
                Expression::Value(if matches!(step, S::Left) { left } else { right })
            }
            (
                Expression::Predicate(
                    WorkflowPredicate::Empty { value } | WorkflowPredicate::NotEmpty { value },
                ),
                S::Value,
            ) => {
                current.expected = Some(Cow::Owned(serde_json::json!({"type":"string"})));
                Expression::Value(value)
            }
            (Expression::Predicate(WorkflowPredicate::Not { predicate }), S::Predicate) => {
                Expression::Predicate(predicate)
            }
            (
                Expression::Predicate(
                    WorkflowPredicate::And { predicates } | WorkflowPredicate::Or { predicates },
                ),
                S::PredicateAt { index },
            ) => Expression::Predicate(predicates.get(*index as usize)?),
            _ => return None,
        };
    }
    matches!(current.expression, Expression::Value(_)).then_some(current)
}

/// Shared native lexical proof, independent of unfinished bindings/instructions.
fn entry_scopes(
    block: &WorkflowBlock,
    declarations: &SchemaMap,
    base: &SchemaMap,
) -> Option<BTreeMap<String, SchemaMap>> {
    let flow = control_flow(block).ok()?;
    // A Return cannot be a predecessor even in an unfinished candidate.
    if block.nodes.iter().any(|(id, node)| {
        matches!(node, WorkflowNodeDefinition::Return { .. }) && !flow.outgoing[id].is_empty()
    }) {
        return None;
    }
    let mut after = BTreeMap::new();
    let mut before = BTreeMap::new();
    for id in flow.topological {
        let scope = flow.scopes.scope(&id, &after, base);
        // Unknown args and producer contracts retain their independent
        // presence facts through the compiler's native intersection.
        let next = declarations
            .0
            .get(&id)
            .map_or_else(|| scope.clone(), |_| scope.with_prefix(&id, &Value::Null));
        after.insert(id.clone(), next);
        before.insert(id, scope);
    }
    Some(before)
}

struct Budget {
    remaining: usize,
    truncated: bool,
}
impl Budget {
    fn admit(&mut self, value: &impl Serialize) -> bool {
        let bytes = serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len() + 1);
        if bytes > self.remaining {
            self.truncated = true;
            return false;
        }
        self.remaining -= bytes;
        true
    }
}

fn destination_compatibility(
    schema: Option<&Value>,
    contract: DestinationContract<'_>,
    inference: &mut SchemaInference<'_>,
) -> WorkflowReferenceCompatibility {
    use WorkflowReferenceCompatibility as C;
    match (schema, contract) {
        (_, DestinationContract::ForbiddenField) => C::Incompatible,
        (actual, DestinationContract::Equality(other)) => {
            let actual = actual.and_then(super::schema_type);
            let other = other.and_then(super::schema_type);
            if actual.is_some_and(|kind| !super::expressions::equality_type_supported(kind))
                || other.is_some_and(|kind| !super::expressions::equality_type_supported(kind))
            {
                C::Incompatible
            } else if actual.is_some() && other.is_some() {
                if super::expressions::equality_types_compatible(actual, other) {
                    C::Compatible
                } else {
                    C::Incompatible
                }
            } else {
                C::Unknown
            }
        }
        (Some(actual), DestinationContract::ToolArgumentsObject)
            if super::validate_tool_arguments_root(actual).is_err() =>
        {
            C::Incompatible
        }
        (Some(actual), DestinationContract::Complete(expected)) => {
            match inference.compatible(actual, expected) {
                Ok(true) => C::Compatible,
                Ok(false) => C::Incompatible,
                Err(_) => C::Unknown,
            }
        }
        _ => C::Unknown,
    }
}

fn array_compatibility<'a>(
    contract: ConsumerContract<'a>,
    schema: Option<&'a Value>,
    inference: &mut SchemaInference<'a>,
) -> (WorkflowReferenceCompatibility, Option<String>) {
    use WorkflowReferenceCompatibility as C;
    if contract.arrays.is_empty() {
        return (C::Compatible, None);
    }
    let unknown = || {
        (
            C::Unknown,
            Some(
                "array construction compatibility is unknown until element schemas are known"
                    .into(),
            ),
        )
    };
    let (Some(schema), Some(selected)) = (schema, contract.selected) else {
        return unknown();
    };
    let Ok(schema) = inference.borrowed(schema) else {
        return unknown();
    };
    let mut incomplete = false;
    for context in contract.arrays {
        let Some(siblings) = &context.siblings else {
            incomplete = true;
            continue;
        };
        let Ok(item) = inference.infer(context.item, 0, Some((selected, &schema))) else {
            incomplete = true;
            continue;
        };
        if let Err(error) = inference.prove_replacement(siblings, item) {
            if matches!(error, InferenceError::Budget) {
                incomplete = true;
            } else {
                return (C::Incompatible, Some(error.to_string()));
            }
        }
    }
    if incomplete {
        unknown()
    } else {
        (C::Compatible, None)
    }
}

fn choice<'a>(
    path: Vec<String>,
    scope: Option<&SchemaMap>,
    contract: ConsumerContract<'a>,
    owners: &BTreeMap<String, Vec<WorkflowBlockStep>>,
    invalid_consumer: bool,
    inference: &mut ChoiceInference<'a>,
) -> WorkflowReferenceChoice {
    use WorkflowReferenceAvailability as A;
    use WorkflowReferenceCompatibility as C;
    let declarations = inference.schemas.declarations;
    let resolution = inference.schemas.resolve(&path);
    let resolution_pending = matches!(resolution, Err(InferenceError::Budget));
    let declared = resolution.ok();
    let proven = scope
        .filter(|scope| {
            path.first()
                .is_some_and(|producer| scope.0.contains_key(producer))
        })
        .and(declared);
    let producer_schema = path.first().and_then(|key| declarations.0.get(key));
    // Declarations have already been validated or derived by declared_output.
    // Only Null is unknown; aggregate schemas may exceed authored depth bounds.
    let known_schema = producer_schema.is_some_and(|schema| !schema.is_null());
    let outside_scope = scope.is_some_and(|scope| {
        path.first()
            .is_none_or(|producer| !scope.0.contains_key(producer))
    });
    let availability = if invalid_consumer
        || producer_schema.is_none()
        || outside_scope
        || (known_schema && declared.is_none() && !resolution_pending)
    {
        A::Unavailable
    } else if proven.is_some() {
        A::Available
    } else if resolution_pending || !known_schema {
        A::Pending
    } else if scope.is_some() {
        A::Unavailable
    } else {
        A::Pending
    };
    let schema = proven.or(declared).filter(|schema| !schema.is_null());
    let key = schema.map(|schema| std::ptr::from_ref(schema) as usize);
    let destination_compatibility =
        if let Some(cached) = key.and_then(|key| inference.destinations.get(&key)) {
            *cached
        } else {
            let result =
                destination_compatibility(schema, contract.destination, &mut inference.schemas);
            if let Some(key) = key {
                inference.destinations.insert(key, result);
            }
            result
        };
    let (array_compatibility, array_diagnostic) = if destination_compatibility == C::Incompatible {
        (C::Compatible, None)
    } else if let Some(cached) = key.and_then(|key| inference.arrays.get(&key)) {
        cached.clone()
    } else {
        let result = array_compatibility(contract, schema, &mut inference.schemas);
        if let Some(key) = key {
            inference.arrays.insert(key, result.clone());
        }
        result
    };
    let compatibility = match (destination_compatibility, array_compatibility) {
        (C::Incompatible, _) | (_, C::Incompatible) => C::Incompatible,
        (_, C::Unknown) => C::Unknown,
        (destination, _) => destination,
    };
    let diagnostic = if matches!(contract.destination, DestinationContract::ForbiddenField) {
        Some("destination schema forbids this field (additionalProperties: false)".into())
    } else if array_compatibility == C::Incompatible {
        array_diagnostic
    } else if compatibility == C::Incompatible {
        Some("reference cannot satisfy this consumer's native type constraints".into())
    } else if inference.schemas.budget.exhausted && compatibility == C::Unknown {
        Some("analysis schema inference budget exhausted; compatibility is unknown".into())
    } else if availability == A::Available && array_compatibility == C::Unknown {
        array_diagnostic
    } else {
        match availability {
        A::Pending if !known_schema => Some("reference schema is incomplete; type compatibility is unknown".into()),
        A::Pending => Some("lexical availability is pending until the candidate's control flow and input contracts can be proven".into()),
        A::Unavailable => Some("reference is unavailable at this exact consumer position, or traverses an optional/unsupported field".into()),
        A::Available => None,
    }
    };
    let producer = path.first().and_then(|key| {
        Some(WorkflowNodeAddress {
            block: owners.get(key)?.clone(),
            node: key.clone(),
        })
    });
    WorkflowReferenceChoice {
        producer,
        path,
        schema: schema.and_then(|schema| inference.schemas.copy(schema).ok()),
        availability,
        compatibility,
        diagnostic,
    }
}

/// Analyze one already bounded candidate. Output contracts remain visible when
/// instructions, bindings or graph construction are incomplete. No program is
/// compiled, admitted, prepared or executed here.
/// # Errors
/// Rejects requests outside the finite request envelope.
pub fn analyze(
    definition: &WorkflowDefinition,
    query: Option<&WorkflowAnalysisQuery>,
    title_matches: &BTreeSet<WorkflowNodeAddress>,
) -> Result<WorkflowCandidateAnalysis, &'static str> {
    analyze_with_schema_budget(definition, query, title_matches, SchemaBudget::default())
}

fn analyze_with_schema_budget(
    definition: &WorkflowDefinition,
    query: Option<&WorkflowAnalysisQuery>,
    title_matches: &BTreeSet<WorkflowNodeAddress>,
    schema_budget: SchemaBudget,
) -> Result<WorkflowCandidateAnalysis, &'static str> {
    validate_candidate(definition)?;
    if let Some(query) = query {
        query.validate()?;
    }
    let mut analysis = WorkflowCandidateAnalysis {
        outputs: Vec::new(),
        references: Vec::new(),
        selected: None,
        diagnostic: None,
        incomplete: false,
        truncated: false,
    };
    let mut budget = Budget {
        remaining: MAX_ANALYSIS_BYTES - 4096,
        truncated: false,
    };
    if let Some(query) = query {
        analyze_query(
            definition,
            query,
            title_matches,
            &mut analysis,
            &mut budget,
            schema_budget,
        );
    }
    let input = if validate_root_schema(&definition.block.input, "input").is_ok() {
        &definition.block.input
    } else {
        &Value::Null
    };
    collect_outputs(
        &definition.block,
        &mut Vec::new(),
        &mut SchemaMap::from_schema(input),
        &mut analysis,
        &mut budget,
    );
    analysis.truncated = budget.truncated;
    analysis.incomplete |=
        analysis.truncated || analysis.outputs.iter().any(|o| o.schema.is_none());
    Ok(analysis)
}

fn collect_outputs(
    block: &WorkflowBlock,
    path: &mut Vec<WorkflowBlockStep>,
    declarations: &mut SchemaMap,
    analysis: &mut WorkflowCandidateAnalysis,
    budget: &mut Budget,
) {
    let declared = declare(block, declarations);
    for (id, node) in &block.nodes {
        let Some(schema) = declared.get(id) else {
            continue;
        };
        let declaration = WorkflowOutputDeclaration {
            producer: WorkflowNodeAddress {
                block: path.clone(),
                node: id.clone(),
            },
            schema: (!schema.is_null()).then(|| schema.clone()),
            diagnostic: schema.is_null().then(|| "output declaration is incomplete or violates the closed Workflow schema contract".into()),
        };
        if budget.admit(&declaration) {
            analysis.outputs.push(declaration);
        }
        if let Some((body, entry)) = container(node, declarations) {
            let mut inside = declarations.with_prefix(id, &entry);
            path.push(WorkflowBlockStep { node: id.clone() });
            collect_outputs(body, path, &mut inside, analysis, budget);
            path.pop();
        }
    }
}

/// Add the declarations of `block`'s nodes, returning each declared node's
/// schema (`Null` when incomplete). An Iteration whose output is its own
/// `item` or `index` is typed from its iterator, as the compiler types it.
fn declare(block: &WorkflowBlock, declarations: &mut SchemaMap) -> BTreeMap<String, Value> {
    let mut declared = BTreeMap::new();
    for (id, node) in &block.nodes {
        match declared_output(node) {
            Ok(Some(schema)) => {
                declared.insert(id.clone(), schema);
            }
            Err(_) => {
                declared.insert(id.clone(), Value::Null);
            }
            Ok(None) => {}
        }
    }
    declarations.0.extend(declared.clone());
    for (id, node) in &block.nodes {
        if let WorkflowNodeDefinition::Iteration {
            output: WorkflowValue::Reference { path },
            error_handle_mode,
            flatten_output,
            ..
        } = node
            && let Some((own, fields)) = path.split_first()
            && own == id
            && let Some((_, entry)) = container(node, declarations)
            && let Some(produced) = fields
                .iter()
                .try_fold(&entry, |schema, field| schema.get("properties")?.get(field))
        {
            let schema = super::execution::iteration_result_schema(
                Some(produced),
                *error_handle_mode,
                *flatten_output,
            );
            declarations.0.insert(id.clone(), schema.clone());
            declared.insert(id.clone(), schema);
        }
    }
    declared
}

/// A container's body and what its ID names inside it: a Loop's variables,
/// or an Iteration's `item` and `index` (`Null` while its iterator's items
/// are unknown).
fn container<'a>(
    node: &'a WorkflowNodeDefinition,
    declarations: &SchemaMap,
) -> Option<(&'a WorkflowBlock, Value)> {
    match node {
        WorkflowNodeDefinition::Loop {
            loop_variables,
            body,
            ..
        } => Some((body, super::loop_variables_schema(loop_variables))),
        WorkflowNodeDefinition::Iteration { iterator, body, .. } => {
            let item = match iterator {
                WorkflowValue::Reference { path } => {
                    path.split_first().and_then(|(producer, fields)| {
                        fields
                            .iter()
                            .try_fold(declarations.0.get(producer)?, |schema, field| {
                                schema.get("properties")?.get(field)
                            })?
                            .get("items")
                            .cloned()
                    })
                }
                _ => None,
            };
            Some((
                body,
                item.map_or(Value::Null, |item| {
                    serde_json::json!({
                        "type": "object",
                        "properties": {"item": item, "index": {"type": "integer"}},
                        "required": ["item", "index"],
                        "additionalProperties": false,
                    })
                }),
            ))
        }
        _ => None,
    }
}

/// What one block sees: the declarations of every variable around it and
/// of its own nodes, where each is declared, and each node's entry scope.
struct BlockContext<'a> {
    block: &'a WorkflowBlock,
    declarations: SchemaMap,
    /// The block of each producer; `args` has none.
    owners: BTreeMap<String, Vec<WorkflowBlockStep>>,
    scopes: Option<BTreeMap<String, SchemaMap>>,
    /// What is in scope where the block starts.
    base: Option<SchemaMap>,
}

/// As the compiler: a body starts from the scope at its container, where
/// the container's ID names its own variables (a Loop's variables, an
/// Iteration's `item` and `index`).
fn block_context<'a>(
    definition: &'a WorkflowDefinition,
    path: &[WorkflowBlockStep],
) -> Option<BlockContext<'a>> {
    let root = &definition.block;
    let input = if validate_root_schema(&root.input, "input").is_ok() {
        &root.input
    } else {
        &Value::Null
    };
    let mut declarations = SchemaMap::from_schema(input);
    let mut owners = BTreeMap::new();
    let mut base = Some(SchemaMap::from_schema(&Value::Null));
    let mut block = root;
    let mut at = Vec::new();
    for step in path.iter().chain(std::iter::once(&WorkflowBlockStep {
        node: String::new(),
    })) {
        for id in block.nodes.keys() {
            owners.insert(id.clone(), at.clone());
        }
        declare(block, &mut declarations);
        let scopes = base
            .as_ref()
            .and_then(|base| entry_scopes(block, &declarations, base));
        if at.len() == path.len() {
            return Some(BlockContext {
                block,
                declarations,
                owners,
                scopes,
                base,
            });
        }
        let (body, entry) = container(block.nodes.get(&step.node)?, &declarations)?;
        base = scopes
            .and_then(|mut scopes| scopes.remove(&step.node))
            .map(|scope| scope.with_prefix(&step.node, &Value::Null));
        declarations.0.insert(step.node.clone(), entry);
        at.push(step.clone());
        block = body;
    }
    None
}

#[allow(clippy::too_many_lines)]
fn analyze_query(
    definition: &WorkflowDefinition,
    query: &WorkflowAnalysisQuery,
    title_matches: &BTreeSet<WorkflowNodeAddress>,
    analysis: &mut WorkflowCandidateAnalysis,
    budget: &mut Budget,
    schema_budget: SchemaBudget,
) {
    let address = &query.consumer.address;
    let Some(context) = block_context(definition, &address.block) else {
        analysis.diagnostic = Some("consumer Block does not exist".into());
        analysis.incomplete = true;
        return;
    };
    let Some(consumer) = consumer(context.block, &query.consumer) else {
        analysis.diagnostic =
            Some("consumer field does not exist or does not belong to this node kind".into());
        analysis.incomplete = true;
        return;
    };
    let input_known = validate_root_schema(&definition.block.input, "input").is_ok();
    // A break condition or an Iteration output reads the body after a round:
    // everything around it, the container's own variables and every body node.
    let inner = consumer.result_scope.then(|| {
        let mut path = address.block.clone();
        path.push(WorkflowBlockStep {
            node: address.node.clone(),
        });
        block_context(definition, &path)
    });
    let BlockContext {
        declarations,
        owners,
        scopes,
        base,
        block,
    } = match inner {
        Some(Some(inner)) => inner,
        Some(None) => {
            analysis.diagnostic = Some("consumer body does not exist".into());
            analysis.incomplete = true;
            return;
        }
        None => context,
    };
    let scope = if consumer.result_scope {
        base.map(|base| {
            block
                .nodes
                .keys()
                .fold(base, |scope, id| scope.with_prefix(id, &Value::Null))
        })
    } else {
        scopes.and_then(|mut scopes| scopes.remove(&address.node))
    };
    let scope = scope.as_ref();
    let mut inference = ChoiceInference {
        schemas: SchemaInference::new(&declarations, &address.node, schema_budget),
        arrays: BTreeMap::new(),
        destinations: BTreeMap::new(),
    };
    let mut consumer = position(consumer, &query.consumer.path, &mut inference.schemas);
    if let Some(consumer) = &mut consumer
        && matches!(
            query.consumer.field,
            WorkflowConsumerField::ConditionVariable { .. }
        )
    {
        consumer.expected = None;
        consumer.necessary = NecessaryCondition::EqualityOperand;
    }
    let invalid_consumer = consumer.is_none();
    let expected = consumer
        .as_ref()
        .and_then(|consumer| consumer.expected.as_deref());
    let destination = match consumer.as_ref().map(|consumer| consumer.necessary) {
        Some(NecessaryCondition::ForbiddenField) => DestinationContract::ForbiddenField,
        Some(NecessaryCondition::ToolArgumentsObject) => DestinationContract::ToolArgumentsObject,
        Some(NecessaryCondition::EqualityOperand) => DestinationContract::Equality(expected),
        _ => expected
            .filter(|schema| validate_workflow_schema(schema, "expected").is_ok())
            .map_or(DestinationContract::Unknown, DestinationContract::Complete),
    };
    let contract = ConsumerContract {
        destination,
        arrays: consumer
            .as_ref()
            .map_or(&[], |consumer| consumer.arrays.as_slice()),
        selected: consumer
            .as_ref()
            .and_then(|consumer| match consumer.expression {
                Expression::Value(value) => Some(value),
                Expression::Predicate(_) => None,
            }),
    };
    if invalid_consumer {
        analysis.diagnostic = Some("consumer expression position does not name a value".into());
    } else if scope.is_none() {
        analysis.diagnostic = Some(
            "candidate control flow or scope contract is incomplete; no availability proof".into(),
        );
    }
    analysis.incomplete = invalid_consumer || scope.is_none() || !input_known;
    if let Some(reference) = &query.reference {
        let selected = choice(
            reference.clone(),
            scope,
            contract,
            &owners,
            invalid_consumer,
            &mut inference,
        );
        if budget.admit(&selected) {
            analysis.selected = Some(selected);
        }
    }
    let search = query.search.to_lowercase();
    let mut pending: Vec<_> = declarations
        .0
        .iter()
        .rev()
        .map(|(id, schema)| (vec![id.clone()], schema))
        .collect();
    let mut visits = 0;
    while let Some((path, schema)) = pending.pop() {
        visits += 1;
        if visits > MAX_ANALYSIS_VISITS {
            budget.truncated = true;
            break;
        }
        // Presentation titles widen discovery before the same bounded catalog
        // admission. They never name synthetic inputs/results or prove scope.
        let title_match = owners.get(&path[0]).is_some_and(|block| {
            title_matches.contains(&WorkflowNodeAddress {
                block: block.clone(),
                node: path[0].clone(),
            })
        });
        if title_match || path.join(".").to_lowercase().contains(&search) {
            if analysis.references.len() >= query.limit {
                budget.truncated = true;
                break;
            }
            let mut item = choice(
                path.clone(),
                scope,
                contract,
                &owners,
                invalid_consumer,
                &mut inference,
            );
            // Optional declared fields still expose their contracts, while the
            // shared resolver truthfully marks them unavailable for traversal.
            if item.schema.is_none() && !schema.is_null() {
                item.schema = inference.schemas.copy(schema).ok();
            }
            if budget.admit(&item) {
                analysis.references.push(item);
            }
        }
        if path.len() < super::MAX_REFERENCE_COMPONENTS
            && let Some(properties) = schema.get("properties").and_then(Value::as_object)
        {
            for (key, child) in properties.iter().rev() {
                if pending.len() + visits >= MAX_ANALYSIS_VISITS {
                    budget.truncated = true;
                    break;
                }
                if key.is_empty() || key.len() > 64 {
                    continue;
                }
                let mut child_path = path.clone();
                child_path.push(key.clone());
                pending.push((child_path, child));
            }
        }
    }
    if inference.schemas.budget.exhausted {
        budget.truncated = true;
        analysis.diagnostic.get_or_insert_with(|| {
            "analysis schema inference budget exhausted; unfinished proofs remain unknown".into()
        });
    }
}
