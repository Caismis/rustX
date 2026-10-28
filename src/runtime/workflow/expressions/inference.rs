//! One native inference implementation for compilation and bounded authoring.
//! References borrow declarations; derived values share ownership. Only actual
//! materialization is charged as allocation, before copying any schema tree.
use super::{
    BTreeMap, MAX_VALUE_BYTES, MAX_VALUE_DEPTH, SchemaMap, Value, WorkflowCompileError,
    WorkflowValue, intrinsic_value_type, invalid, literal_depth, reference_schema_with,
    schema_type,
};
use std::{
    collections::{BTreeSet, HashSet},
    io::Write,
    rc::Rc,
};

pub(in super::super) const MAX_SCHEMA_ALLOCATION: usize = 4 * 1024 * 1024;
pub(in super::super) const MAX_SCHEMA_WORK: usize = 32 * 1024 * 1024;
pub(in super::super) const MAX_SCHEMA_STEPS: usize = 32 * 1024;

#[derive(Debug, thiserror::Error)]
pub(in super::super) enum InferenceError {
    #[error("analysis schema inference budget exhausted")]
    Budget,
    #[error("reference path was already proven unavailable")]
    Unavailable,
    #[error(transparent)]
    Invalid(#[from] WorkflowCompileError),
}
impl InferenceError {
    fn at(self, path: String) -> Self {
        match self {
            Self::Budget => Self::Budget,
            Self::Unavailable => Self::Unavailable,
            Self::Invalid(error) => Self::Invalid(error.at(path)),
        }
    }
    pub(in super::super) fn into_compile_error(self) -> WorkflowCompileError {
        match self {
            Self::Budget => invalid("analysis schema inference budget exhausted"),
            Self::Unavailable => invalid("reference path was already proven unavailable"),
            Self::Invalid(error) => error,
        }
    }
}

#[derive(Clone)]
pub(in super::super) enum Schema<'a> {
    Borrowed(&'a Value, usize),
    Derived(Rc<Value>, usize),
}
impl Schema<'_> {
    pub(in super::super) fn value(&self) -> &Value {
        match self {
            Self::Borrowed(value, _) => value,
            Self::Derived(value, _) => value,
        }
    }
    fn weight(&self) -> usize {
        match self {
            Self::Borrowed(_, weight) | Self::Derived(_, weight) => *weight,
        }
    }
    fn identity(&self) -> usize {
        std::ptr::from_ref(self.value()) as usize
    }
}

pub(in super::super) struct SchemaBudget {
    allocation: usize,
    work: usize,
    steps: usize,
    bounded: bool,
    pub(in super::super) exhausted: bool,
    #[cfg(test)]
    pub(in super::super) allocation_used: usize,
}
impl Default for SchemaBudget {
    fn default() -> Self {
        Self::limited(MAX_SCHEMA_ALLOCATION, MAX_SCHEMA_WORK, MAX_SCHEMA_STEPS)
    }
}
impl SchemaBudget {
    pub(in super::super) fn limited(allocation: usize, work: usize, steps: usize) -> Self {
        Self {
            allocation,
            work,
            steps,
            bounded: true,
            exhausted: false,
            #[cfg(test)]
            allocation_used: 0,
        }
    }
    pub(in super::super) fn unlimited() -> Self {
        Self {
            bounded: false,
            ..Self::limited(usize::MAX, usize::MAX, usize::MAX)
        }
    }
    fn take(&mut self, allocation: usize, work: usize, steps: usize) -> Result<(), InferenceError> {
        if self.bounded {
            if allocation > self.allocation || work > self.work || steps > self.steps {
                self.exhausted = true;
                return Err(InferenceError::Budget);
            }
            self.allocation -= allocation;
            self.work -= work;
            self.steps -= steps;
            #[cfg(test)]
            {
                self.allocation_used += allocation;
            }
        }
        Ok(())
    }
}

struct SizeWriter<'a> {
    bytes: usize,
    budget: &'a mut SchemaBudget,
    exhausted: bool,
}
impl Write for SizeWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Err(error) = self.budget.take(0, bytes.len(), 0) {
            self.exhausted = true;
            return Err(std::io::Error::other(error));
        }
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_VALUE_BYTES {
            return Err(std::io::Error::other("construction byte bound exceeded"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

enum Element<'s, 'a> {
    Empty,
    Finite(&'s Value, Vec<&'s Value>),
    Contract(&'s Schema<'a>),
}

pub(in super::super) struct SchemaInference<'a> {
    pub(in super::super) declarations: &'a SchemaMap,
    node: &'a str,
    pub(in super::super) budget: SchemaBudget,
    /// Only borrowed declaration addresses are cached. Derived values can be
    /// released between choices, so their addresses must never enter this map.
    weights: BTreeMap<usize, usize>,
    references: BTreeMap<Vec<String>, Option<&'a Value>>,
}
impl<'a> SchemaInference<'a> {
    pub(in super::super) fn new(
        declarations: &'a SchemaMap,
        node: &'a str,
        budget: SchemaBudget,
    ) -> Self {
        Self {
            declarations,
            node,
            budget,
            weights: BTreeMap::new(),
            references: BTreeMap::new(),
        }
    }

    // A conservative bound for JSON text plus tree/container storage. Counting
    // traverses borrowed data without allocating or serializing the schema.
    fn measure(&mut self, value: &Value) -> Result<usize, InferenceError> {
        if !self.budget.bounded {
            return Ok(0);
        }
        self.budget.take(0, 64, 1)?;
        let mut weight = 128usize;
        match value {
            Value::String(text) => weight = weight.saturating_add(text.len().saturating_mul(6)),
            Value::Array(values) => {
                for value in values {
                    weight = weight.saturating_add(self.measure(value)?);
                }
            }
            Value::Object(fields) => {
                for (key, value) in fields {
                    weight = weight.saturating_add(128 + key.len().saturating_mul(6));
                    weight = weight.saturating_add(self.measure(value)?);
                }
            }
            _ => {}
        }
        // Charge text visits too, before any later clone/validator/serialization.
        self.budget.take(
            0,
            match value {
                Value::String(text) => text.len(),
                _ => 0,
            },
            0,
        )?;
        Ok(weight)
    }

    pub(in super::super) fn borrowed(
        &mut self,
        value: &'a Value,
    ) -> Result<Schema<'a>, InferenceError> {
        if !self.budget.bounded {
            return Ok(Schema::Borrowed(value, 0));
        }
        let key = std::ptr::from_ref(value) as usize;
        let weight = if let Some(weight) = self.weights.get(&key) {
            *weight
        } else {
            let weight = self.measure(value)?;
            self.budget.take(256, 0, 0)?;
            self.weights.insert(key, weight);
            weight
        };
        Ok(Schema::Borrowed(value, weight))
    }

    pub(in super::super) fn resolve(
        &mut self,
        path: &[String],
    ) -> Result<&'a Value, InferenceError> {
        let work = path.iter().map(|key| key.len() * 16 + 64).sum();
        if let Some(schema) = self.references.get(path) {
            // An exhausted request retains facts already established. Cache
            // reads do not traverse or copy a schema or create a new proof.
            let _ = self.budget.take(0, work, 1);
            return schema.ok_or(InferenceError::Unavailable);
        }
        self.budget.take(0, work, 1)?;
        self.budget.take(path.len().saturating_mul(512), 0, 0)?;
        let schema: Result<_, InferenceError> =
            reference_schema_with(path, self.declarations, self.node, &mut |work| {
                self.budget.take(0, work, 1)
            });
        if matches!(schema, Err(InferenceError::Budget)) {
            return schema;
        }
        self.references
            .insert(path.to_vec(), schema.as_ref().ok().copied());
        schema
    }

    pub(in super::super) fn copy(&mut self, value: &Value) -> Result<Value, InferenceError> {
        let weight = self.measure(value)?;
        self.budget.take(weight, weight, 0)?;
        Ok(value.clone())
    }

    fn value_nodes(&mut self, value: &Value) -> Result<usize, InferenceError> {
        self.budget.take(0, 64, 1)?;
        let mut nodes = 1usize;
        match value {
            Value::Object(fields) => {
                for child in fields.values() {
                    nodes = nodes.saturating_add(self.value_nodes(child)?);
                }
            }
            Value::Array(items) => {
                for child in items {
                    nodes = nodes.saturating_add(self.value_nodes(child)?);
                }
            }
            _ => {}
        }
        Ok(nodes)
    }

    pub(in super::super) fn compatible(
        &mut self,
        actual: &Value,
        expected: &Value,
    ) -> Result<bool, InferenceError> {
        use super::super::SchemaProofStep;
        super::super::schemas_compatible_with(actual, expected, &mut |actual, expected, step| {
            let actual_weight = self.measure(actual)?;
            let expected_weight = self.measure(expected)?;
            let mut work = actual_weight.saturating_add(expected_weight);
            let allocation = match step {
                SchemaProofStep::CompileValidator => expected_weight.saturating_mul(4),
                SchemaProofStep::ValidateValue => {
                    // A nested value may apply the same enum/properties contract
                    // at every node. Reserve that product before Validator runs.
                    work = actual_weight
                        .saturating_add(expected_weight.saturating_mul(self.value_nodes(actual)?));
                    0
                }
                SchemaProofStep::Structure
                    if schema_type(actual) == Some("object")
                        && schema_type(expected) == Some("object") =>
                {
                    [actual, expected]
                        .iter()
                        .map(|schema| {
                            schema
                                .get("required")
                                .and_then(Value::as_array)
                                .map_or(0, Vec::len)
                        })
                        .sum::<usize>()
                        .saturating_mul(128)
                }
                SchemaProofStep::Structure => 0,
            };
            self.budget.take(allocation, work, 1)
        })
    }

    fn derived(
        &mut self,
        weight: usize,
        make: impl FnOnce() -> Value,
    ) -> Result<Schema<'a>, InferenceError> {
        self.budget.take(weight, weight, 1)?;
        Ok(Schema::Derived(Rc::new(make()), weight))
    }

    pub(in super::super) fn siblings(
        &mut self,
        items: &[WorkflowValue],
        selected: usize,
    ) -> Result<Vec<Schema<'a>>, InferenceError> {
        self.budget.take(items.len().saturating_mul(256), 0, 0)?;
        let mut seen = BTreeSet::new();
        let mut schemas = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if index != selected {
                let schema = self.infer(item, 0, None)?;
                if seen.insert(schema.identity()) {
                    schemas.push(schema);
                }
            }
        }
        Ok(schemas)
    }

    /// Exact AST identity replaces only the queried value. References and
    /// replacements share trees; derived object/array copies are precharged.
    pub(in super::super) fn infer(
        &mut self,
        expression: &WorkflowValue,
        depth: usize,
        replacement: Option<(&WorkflowValue, &Schema<'a>)>,
    ) -> Result<Schema<'a>, InferenceError> {
        self.budget.take(0, 0, 1)?;
        if depth > MAX_VALUE_DEPTH {
            return Err(invalid("construction depth/byte bound exceeded").into());
        }
        if let Some((selected, schema)) = replacement
            && std::ptr::eq(expression, selected)
        {
            return Ok(schema.clone());
        }
        let mut writer = SizeWriter {
            bytes: 0,
            budget: &mut self.budget,
            exhausted: false,
        };
        if serde_json::to_writer(&mut writer, expression).is_err() {
            return Err(if writer.exhausted {
                InferenceError::Budget
            } else {
                invalid("construction depth/byte bound exceeded").into()
            });
        }
        match expression {
            WorkflowValue::Reference { path } => {
                let schema = self.resolve(path)?;
                self.borrowed(schema)
            }
            WorkflowValue::Literal { value } => {
                literal_depth(value, depth)?;
                let weight = self.measure(value)?.saturating_add(512);
                self.derived(weight, || serde_json::json!({"type":intrinsic_value_type(expression).unwrap(),"const":value}))
            }
            WorkflowValue::Object { fields } => {
                self.budget.take(fields.len().saturating_mul(128), 0, 0)?;
                let mut children = Vec::new();
                let mut weight = 1024usize;
                for (key, value) in fields {
                    if key.is_empty() || key.len() > 64 {
                        return Err(invalid("invalid construction field").into());
                    }
                    let schema = self
                        .infer(value, depth + 1, replacement)
                        .map_err(|e| e.at(format!("fields.{key}")))?;
                    weight = weight
                        .saturating_add(schema.weight())
                        .saturating_add(256 + key.len() * 12);
                    children.push((key, schema));
                }
                self.derived(weight, || {
                    let properties: serde_json::Map<_, _> = children.iter().map(|(key, schema)| ((*key).clone(), schema.value().clone())).collect();
                    serde_json::json!({"type":"object","properties":properties,"required":fields.keys().collect::<Vec<_>>(),"additionalProperties":false})
                })
            }
            WorkflowValue::Array { items } => {
                self.budget.take(items.len().saturating_mul(128), 0, 0)?;
                let schemas = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        self.infer(item, depth + 1, replacement)
                            .map_err(|e| e.at(format!("items.{index}")))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let element = self.element(&schemas)?;
                match element {
                    Element::Empty => {
                        self.derived(512, || serde_json::json!({"type":"array","const":[]}))
                    }
                    Element::Contract(schema) => self.derived(
                        schema.weight().saturating_add(512),
                        || serde_json::json!({"type":"array","items":schema.value()}),
                    ),
                    Element::Finite(kind, values) => {
                        let mut weight = 1024usize;
                        for value in &values {
                            weight = weight.saturating_add(self.measure(value)?);
                        }
                        self.derived(weight, || serde_json::json!({"type":"array","items":{"type":kind,"enum":values}}))
                    }
                }
            }
        }
    }

    pub(in super::super) fn prove_array(
        &mut self,
        schemas: &[Schema<'a>],
    ) -> Result<(), InferenceError> {
        self.element(schemas).map(|_| ())
    }

    pub(in super::super) fn prove_replacement(
        &mut self,
        siblings: &[Schema<'a>],
        item: Schema<'a>,
    ) -> Result<(), InferenceError> {
        self.budget
            .take((siblings.len() + 1).saturating_mul(128), 0, 0)?;
        let mut schemas = siblings.to_vec();
        schemas.push(item);
        self.prove_array(&schemas)
    }

    /// Shared finite-value and existential common-contract proof. Address
    /// deduplication removes repeated obligations, never establishes a proof.
    fn element<'s>(
        &mut self,
        schemas: &'s [Schema<'a>],
    ) -> Result<Element<'s, 'a>, InferenceError> {
        self.budget.take(schemas.len().saturating_mul(256), 0, 0)?;
        let mut seen = BTreeSet::new();
        let mut values_seen = HashSet::new();
        let mut distinct = Vec::new();
        for schema in schemas {
            if seen.insert(schema.identity()) {
                self.budget.take(0, schema.weight(), 1)?;
                if values_seen.insert(schema.value()) {
                    distinct.push(schema);
                }
            }
        }
        let schemas = distinct;
        let Some(&first) = schemas.first() else {
            return Ok(Element::Empty);
        };
        if schemas
            .iter()
            .all(|schema| schema.value().get("const").is_some())
        {
            let mut values: Vec<&Value> = Vec::new();
            for &schema in &schemas {
                if schema_type(schema.value()) != schema_type(first.value()) {
                    return Err(invalid("constructed arrays require one element type").into());
                }
                let value = &schema.value()["const"];
                let mut duplicate = false;
                for prior in &values {
                    let weight = self.measure(value)?.saturating_add(self.measure(prior)?);
                    self.budget.take(0, weight, 1)?;
                    if value == *prior {
                        duplicate = true;
                        break;
                    }
                }
                if !duplicate {
                    values.push(value);
                }
            }
            Ok(Element::Finite(&first.value()["type"], values))
        } else {
            for &candidate in &schemas {
                let mut admits_all = true;
                for actual in &schemas {
                    if !self.compatible(actual.value(), candidate.value())? {
                        admits_all = false;
                        break;
                    }
                }
                if admits_all {
                    return Ok(Element::Contract(candidate));
                }
            }
            Err(invalid("constructed array element contracts are incompatible").into())
        }
    }
}
