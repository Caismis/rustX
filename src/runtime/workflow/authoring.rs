//! Inert candidate analysis using the compiler's control-flow, lexical-reference
//! and structural compatibility proofs. This module cannot construct a program.
use super::expressions::{InferenceError, Schema, SchemaBudget, SchemaInference};
use super::{
    BTreeMap, SchemaMap, Value, WorkflowBlock, WorkflowCompileError, WorkflowDefinition,
    WorkflowNodeDefinition, WorkflowPredicate, WorkflowValue, control_flow, node_entry_scope,
    validate_root_schema, validate_workflow_schema,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
mod selections;
pub use selections::{WorkflowModelPolicy, WorkflowToolSelector};
#[cfg(test)]
mod tests;

pub const MAX_ANALYSIS_BYTES: usize = 256 * 1024;
pub const MAX_ANALYSIS_CHOICES: usize = 256;
pub const MAX_ANALYSIS_VISITS: usize = 2048;

/// Structured lexical address. A missing branch means Loop body; an explicit
/// branch (including one named `body`) always means a Parallel branch.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBlockStep {
    pub node: String,
    #[serde(
        default,
        deserialize_with = "crate::extensions::present_and_not_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    pub branch: Option<String>,
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
    Input { binding: String },
    Arguments,
    ReviewSubject,
    ReviewContext { index: u32 },
    Condition,
    ReturnOutput,
    BranchInput { branch: String },
    LoopInput,
    LoopUntil,
    LoopCarry,
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
        block = match (block.nodes.get(&step.node)?, &step.branch) {
            (WorkflowNodeDefinition::Loop { body, .. }, None) => body,
            (WorkflowNodeDefinition::Parallel { branches }, Some(branch)) => {
                &branches.get(branch)?.block
            }
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
        WorkflowNodeDefinition::Agent { output, .. } => {
            validate_root_schema(output, "output")?;
            output.clone()
        }
        WorkflowNodeDefinition::Tool { result, .. } => {
            let schema = result.schema();
            validate_workflow_schema(&schema, "result")?;
            schema
        }
        WorkflowNodeDefinition::Review { .. } => super::review::result_schema(),
        WorkflowNodeDefinition::Loop { body, .. } => {
            validate_root_schema(&body.output, "body.output")?;
            super::execution::loop_result_schema(&body.output)
        }
        WorkflowNodeDefinition::Parallel { branches } => {
            let mut properties = serde_json::Map::new();
            for (key, branch) in branches {
                validate_root_schema(&branch.block.output, "branch.output")?;
                properties.insert(key.clone(), branch.block.output.clone());
            }
            parallel_output(properties)
        }
        WorkflowNodeDefinition::Branch { .. } | WorkflowNodeDefinition::Return { .. } => {
            return Ok(None);
        }
    };
    Ok(Some(schema))
}

pub(super) fn parallel_output(properties: serde_json::Map<String, Value>) -> Value {
    let required = Value::Array(properties.keys().cloned().map(Value::String).collect());
    Value::Object(serde_json::Map::from_iter([
        ("type".into(), Value::String("object".into())),
        ("properties".into(), Value::Object(properties)),
        ("required".into(), required),
        ("additionalProperties".into(), Value::Bool(false)),
    ]))
}

fn valid_key(key: &str) -> bool {
    super::expressions::valid_local_key(key)
}
fn valid_address(address: &WorkflowNodeAddress) -> bool {
    address.block.len() <= super::MAX_BLOCK_DEPTH
        && valid_key(&address.node)
        && address
            .block
            .iter()
            .all(|step| valid_key(&step.node) && step.branch.as_deref().is_none_or(valid_key))
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
            WorkflowConsumerField::Input { binding }
                if binding.trim().is_empty() || binding.len() > 64 || binding.contains('.') =>
            {
                Err("invalid input binding identity")
            }
            WorkflowConsumerField::BranchInput { branch } if !valid_key(branch) => {
                Err("invalid branch identity")
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
            match node {
                WorkflowNodeDefinition::Loop { body, .. } => walk(body, depth + 1, count)?,
                WorkflowNodeDefinition::Parallel { branches } => {
                    if branches.len() > super::MAX_PARALLEL_BRANCHES {
                        return Err("candidate exceeds native branch bound");
                    }
                    for branch in branches.values() {
                        walk(&branch.block, depth + 1, count)?;
                    }
                }
                _ => {}
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

#[derive(Clone, Copy)]
enum Expression<'a> {
    Value(&'a WorkflowValue),
    Predicate(&'a WorkflowPredicate),
}
struct Consumer<'a> {
    expression: Expression<'a>,
    expected: Option<Cow<'a, Value>>,
    necessary: NecessaryCondition,
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
        (N::Model { input, .. } | N::Agent { input, .. }, F::Input { binding }) => {
            (Expression::Value(input.get(binding)?), None, false)
        }
        (N::Tool { arguments, .. }, F::Arguments) => (Expression::Value(arguments), None, false),
        (N::Review { subject, .. }, F::ReviewSubject) => (
            Expression::Value(subject.value()),
            matches!(subject, super::WorkflowReviewSubject::Plan { .. })
                .then(|| Cow::Owned(serde_json::json!({"type":"object"}))),
            false,
        ),
        (N::Review { context, .. }, F::ReviewContext { index }) => (
            Expression::Value(context.get(*index as usize)?),
            None,
            false,
        ),
        (N::Branch { condition }, F::Condition) => (Expression::Predicate(condition), None, false),
        (N::Return { output }, F::ReturnOutput) => (
            Expression::Value(output),
            Some(Cow::Borrowed(&block.output)),
            false,
        ),
        (N::Parallel { branches }, F::BranchInput { branch }) => {
            let branch = branches.get(branch)?;
            (
                Expression::Value(&branch.input),
                Some(Cow::Borrowed(&branch.block.input)),
                false,
            )
        }
        (N::Loop { input, body, .. }, F::LoopInput) => (
            Expression::Value(input),
            Some(Cow::Borrowed(&body.input)),
            false,
        ),
        (N::Loop { until, .. }, F::LoopUntil) => (Expression::Predicate(until), None, true),
        (N::Loop { carry, body, .. }, F::LoopCarry) => (
            Expression::Value(carry),
            Some(Cow::Borrowed(&body.input)),
            true,
        ),
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
        let scope = node_entry_scope(block, &flow.outgoing, &after, &id, &Value::Null);
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
    address: &WorkflowNodeAddress,
    invalid_consumer: bool,
    result_scope: bool,
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
    let producer = path
        .first()
        .filter(|key| key.as_str() != "args" && !(result_scope && key.as_str() == "result"))
        .map(|key| WorkflowNodeAddress {
            block: address.block.clone(),
            node: key.clone(),
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
) -> Result<WorkflowCandidateAnalysis, &'static str> {
    analyze_with_schema_budget(definition, query, SchemaBudget::default())
}

fn analyze_with_schema_budget(
    definition: &WorkflowDefinition,
    query: Option<&WorkflowAnalysisQuery>,
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
        analyze_query(definition, query, &mut analysis, &mut budget, schema_budget);
    }
    collect_outputs(
        &definition.block,
        &mut Vec::new(),
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
    analysis: &mut WorkflowCandidateAnalysis,
    budget: &mut Budget,
) {
    for (id, node) in &block.nodes {
        let (schema, diagnostic) = match declared_output(node) {
            Ok(Some(schema)) => (Some(schema), None),
            Ok(None) => continue,
            Err(_) => (None, Some("output declaration is incomplete or violates the closed Workflow schema contract".into())),
        };
        let declaration = WorkflowOutputDeclaration {
            producer: WorkflowNodeAddress {
                block: path.clone(),
                node: id.clone(),
            },
            schema,
            diagnostic,
        };
        if budget.admit(&declaration) {
            analysis.outputs.push(declaration);
        }
        match node {
            WorkflowNodeDefinition::Loop { body, .. } => {
                path.push(WorkflowBlockStep {
                    node: id.clone(),
                    branch: None,
                });
                collect_outputs(body, path, analysis, budget);
                path.pop();
            }
            WorkflowNodeDefinition::Parallel { branches } => {
                for (key, branch) in branches {
                    path.push(WorkflowBlockStep {
                        node: id.clone(),
                        branch: Some(key.clone()),
                    });
                    collect_outputs(&branch.block, path, analysis, budget);
                    path.pop();
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_lines)]
fn analyze_query(
    definition: &WorkflowDefinition,
    query: &WorkflowAnalysisQuery,
    analysis: &mut WorkflowCandidateAnalysis,
    budget: &mut Budget,
    schema_budget: SchemaBudget,
) {
    let address = &query.consumer.address;
    let Some(block) = block_at(&definition.block, &address.block) else {
        analysis.diagnostic = Some("consumer Block does not exist".into());
        analysis.incomplete = true;
        return;
    };
    let Some(consumer) = consumer(block, &query.consumer) else {
        analysis.diagnostic =
            Some("consumer field does not exist or does not belong to this node kind".into());
        analysis.incomplete = true;
        return;
    };
    let input_known = validate_root_schema(&block.input, "input").is_ok();
    let unknown_input = Value::Null;
    let input_schema = if input_known {
        &block.input
    } else {
        &unknown_input
    };
    let mut declarations = SchemaMap::from_schema(input_schema);
    for (id, node) in &block.nodes {
        match declared_output(node) {
            Ok(Some(schema)) => {
                declarations.0.insert(id.clone(), schema);
            }
            Err(_) => {
                declarations.0.insert(id.clone(), Value::Null);
            }
            Ok(None) => {}
        }
    }
    let scopes = entry_scopes(block, &declarations);
    let synthetic_result = consumer.result_scope;
    let result_scope = if consumer.result_scope {
        let WorkflowNodeDefinition::Loop { body, .. } = &block.nodes[&address.node] else {
            unreachable!()
        };
        let valid = validate_root_schema(&body.output, "body.output").is_ok();
        declarations = SchemaMap(BTreeMap::from([(
            "result".into(),
            if valid {
                body.output.clone()
            } else {
                Value::Null
            },
        )]));
        valid.then(|| SchemaMap(BTreeMap::from([("result".into(), Value::Null)])))
    } else {
        None
    };
    let scope = if consumer.result_scope {
        result_scope.as_ref()
    } else {
        scopes.as_ref().and_then(|scopes| scopes.get(&address.node))
    };
    let mut inference = ChoiceInference {
        schemas: SchemaInference::new(&declarations, &address.node, schema_budget),
        arrays: BTreeMap::new(),
        destinations: BTreeMap::new(),
    };
    let consumer = position(consumer, &query.consumer.path, &mut inference.schemas);
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
            address,
            invalid_consumer,
            synthetic_result,
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
        if path.join(".").to_lowercase().contains(&search) {
            if analysis.references.len() >= query.limit {
                budget.truncated = true;
                break;
            }
            let mut item = choice(
                path.clone(),
                scope,
                contract,
                address,
                invalid_consumer,
                synthetic_result,
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
