//! Conservative typed construction and lexical reference resolution.
use super::{
    BTreeMap, MAX_REFERENCE_COMPONENTS, MAX_VALUE_BYTES, MAX_VALUE_DEPTH, SchemaMap, Serialize,
    Value, WorkflowCompileError, WorkflowPredicate, WorkflowRunError, WorkflowValue,
    schema_required, schema_type, schemas_compatible,
};

pub(super) fn valid_local_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn invalid(detail: impl Into<String>) -> WorkflowCompileError {
    WorkflowCompileError::InvalidReference(detail.into())
}

fn check_size<T: Serialize>(value: &T, depth: usize) -> Result<(), WorkflowCompileError> {
    if depth > MAX_VALUE_DEPTH
        || serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_VALUE_BYTES)
    {
        return Err(invalid("construction depth/byte bound exceeded"));
    }
    Ok(())
}

pub(super) fn value_schema(
    expression: &WorkflowValue,
    available: &SchemaMap,
    node: &str,
    depth: usize,
) -> Result<Value, WorkflowCompileError> {
    check_size(expression, depth)?;
    match expression {
        WorkflowValue::Reference { path } => {
            if path.is_empty()
                || path.len() > MAX_REFERENCE_COMPONENTS
                || path.iter().any(|key| key.is_empty() || key.len() > 64)
            {
                return Err(invalid("invalid reference path"));
            }
            let mut schema = available.0.get(&path[0]).ok_or_else(|| {
                invalid(format!(
                    "node {node} references unavailable producer {:?}",
                    path[0]
                ))
            })?;
            for key in &path[1..] {
                if schema_type(schema) != Some("object")
                    || !schema_required(schema).contains(key.as_str())
                {
                    return Err(invalid(format!(
                        "node {node}: unavailable field in {path:?}"
                    )));
                }
                schema = schema
                    .get("properties")
                    .and_then(|properties| properties.get(key))
                    .ok_or_else(|| invalid(format!("invalid field path {path:?}")))?;
            }
            Ok(schema.clone())
        }
        WorkflowValue::Literal { value } => {
            literal_depth(value, depth)?;
            Ok(serde_json::json!({"type": json_type(value), "const": value}))
        }
        WorkflowValue::Object { fields } => {
            let mut properties = serde_json::Map::new();
            for (key, value) in fields {
                if key.is_empty() || key.len() > 64 {
                    return Err(invalid("invalid construction field"));
                }
                properties.insert(
                    key.clone(),
                    value_schema(value, available, node, depth + 1)?,
                );
            }
            Ok(
                serde_json::json!({"type":"object", "properties": properties,
                "required": fields.keys().collect::<Vec<_>>(), "additionalProperties": false}),
            )
        }
        WorkflowValue::Array { items } => {
            let schemas = items
                .iter()
                .map(|item| value_schema(item, available, node, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            if schemas.is_empty() {
                return Ok(serde_json::json!({"type":"array", "const":[]}));
            }
            // Preserve finite literal alternatives exactly. Otherwise require a
            // single proven element contract, not an unsound guessed union.
            let first = &schemas[0];
            let element = if schemas.iter().all(|schema| schema.get("const").is_some()) {
                let mut values = Vec::new();
                for schema in &schemas {
                    let value = schema["const"].clone();
                    if !values.contains(&value) {
                        values.push(value);
                    }
                }
                if schemas
                    .iter()
                    .any(|schema| schema_type(schema) != schema_type(first))
                {
                    return Err(invalid("constructed arrays require one element type"));
                }
                serde_json::json!({"type": first["type"], "enum": values})
            } else {
                let candidate = schemas
                    .iter()
                    .find(|candidate| {
                        schemas
                            .iter()
                            .all(|schema| schemas_compatible(schema, candidate))
                    })
                    .ok_or_else(|| {
                        invalid("constructed array element contracts are incompatible")
                    })?;
                candidate.clone()
            };
            Ok(serde_json::json!({"type":"array", "items":element}))
        }
    }
}

fn literal_depth(value: &Value, depth: usize) -> Result<(), WorkflowCompileError> {
    if depth > MAX_VALUE_DEPTH {
        return Err(invalid("literal depth exceeded"));
    }
    match value {
        Value::Object(fields) => {
            for value in fields.values() {
                literal_depth(value, depth + 1)?;
            }
        }
        Value::Array(items) => {
            for value in items {
                literal_depth(value, depth + 1)?;
            }
        }
        _ => (),
    }
    Ok(())
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
    }
}

pub(super) fn validate_predicate(
    predicate: &WorkflowPredicate,
    available: &SchemaMap,
    node: &str,
    depth: usize,
) -> Result<(), WorkflowCompileError> {
    check_size(predicate, depth)?;
    match predicate {
        WorkflowPredicate::Boolean { value } => {
            if schema_type(&value_schema(value, available, node, depth + 1)?) != Some("boolean") {
                return Err(WorkflowCompileError::IncompatibleReference(
                    "predicate requires boolean; no truthiness".into(),
                ));
            }
        }
        WorkflowPredicate::Equal { left, right } | WorkflowPredicate::NotEqual { left, right } => {
            let left = value_schema(left, available, node, depth + 1)?;
            let right = value_schema(right, available, node, depth + 1)?;
            if schema_type(&left) != schema_type(&right)
                || !matches!(schema_type(&left), Some("boolean" | "string" | "null"))
            {
                return Err(invalid(
                    "equality requires matching boolean/string/null types; numeric predicates are unsupported",
                ));
            }
        }
        WorkflowPredicate::Not { predicate } => {
            validate_predicate(predicate, available, node, depth + 1)?;
        }
        WorkflowPredicate::And { predicates } | WorkflowPredicate::Or { predicates } => {
            if predicates.is_empty() {
                return Err(invalid("boolean composition must be nonempty"));
            }
            for predicate in predicates {
                validate_predicate(predicate, available, node, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// Interpreter-only applicability, never embedded in authored/user JSON.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CommittedValue {
    pub value: Value,
    pub candidate: Option<crate::runtime::workspace::CandidateReference>,
}
impl From<Value> for CommittedValue {
    fn from(value: Value) -> Self {
        Self {
            value,
            candidate: None,
        }
    }
}
impl CommittedValue {
    pub fn depend_on(&mut self, other: &Self) -> Result<(), WorkflowRunError> {
        if let Some(reference) = &other.candidate {
            if self
                .candidate
                .as_ref()
                .is_some_and(|existing| existing != reference)
            {
                return Err(WorkflowRunError::InvalidValue(
                    "incompatible candidate applicability".into(),
                ));
            }
            self.candidate = Some(reference.clone());
        }
        Ok(())
    }
    pub async fn assert_current(&self, run: &super::WorkflowRun) -> Result<(), WorkflowRunError> {
        if let Some(reference) = &self.candidate {
            run.candidate
                .as_ref()
                .ok_or_else(|| WorkflowRunError::InvalidValue("missing candidate owner".into()))?
                .assert_current(reference)
                .await
                .map_err(WorkflowRunError::InvalidValue)?;
        }
        Ok(())
    }
}

pub(super) fn evaluate_value(
    expression: &WorkflowValue,
    input: &CommittedValue,
    values: &BTreeMap<String, CommittedValue>,
) -> Result<CommittedValue, WorkflowRunError> {
    let mut committed = CommittedValue::from(Value::Null);
    let value = match expression {
        WorkflowValue::Reference { path } => {
            let first = path
                .first()
                .ok_or_else(|| WorkflowRunError::InvalidValue("empty reference".into()))?;
            let producer = if first == "args" {
                Some(input)
            } else {
                values.get(first)
            };
            if let Some(producer) = producer {
                committed.depend_on(producer)?;
            }
            let mut value = producer.map(|producer| &producer.value);
            for key in &path[1..] {
                value = value.and_then(|value| value.get(key));
            }
            value.cloned().ok_or_else(|| {
                WorkflowRunError::InvalidValue(format!("uncommitted reference {path:?}"))
            })?
        }
        WorkflowValue::Literal { value } => value.clone(),
        WorkflowValue::Object { fields } => {
            let mut result = serde_json::Map::new();
            let mut bytes = 2;
            for (key, expression) in fields {
                let value = evaluate_value(expression, input, values)?;
                bytes += serde_json::to_vec(key).expect("JSON key").len()
                    + serde_json::to_vec(&value.value).expect("JSON value").len()
                    + 2;
                if bytes.saturating_sub(usize::from(!fields.is_empty())) > MAX_VALUE_BYTES {
                    return Err(WorkflowRunError::InvalidValue(
                        "constructed object byte bound exceeded".into(),
                    ));
                }
                committed.depend_on(&value)?;
                result.insert(key.clone(), value.value);
            }
            Value::Object(result)
        }
        WorkflowValue::Array { items } => {
            let mut result = Vec::new();
            let mut bytes = 2;
            for item in items {
                let value = evaluate_value(item, input, values)?;
                bytes += serde_json::to_vec(&value.value).expect("JSON value").len() + 1;
                if bytes - 1 > MAX_VALUE_BYTES {
                    return Err(WorkflowRunError::InvalidValue(
                        "constructed array byte bound exceeded".into(),
                    ));
                }
                committed.depend_on(&value)?;
                result.push(value.value);
            }
            Value::Array(result)
        }
    };
    bounded_value_bytes(&value)?;
    committed.value = value;
    Ok(committed)
}

pub(super) fn bounded_value_bytes(value: &Value) -> Result<usize, WorkflowRunError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| WorkflowRunError::InvalidValue(error.to_string()))?
        .len();
    if bytes > MAX_VALUE_BYTES {
        return Err(WorkflowRunError::InvalidValue(
            "constructed value byte bound exceeded".into(),
        ));
    }
    let mut pending = vec![(value, 0)];
    while let Some((value, depth)) = pending.pop() {
        if depth > MAX_VALUE_DEPTH {
            return Err(WorkflowRunError::InvalidValue(
                "constructed value depth exceeded".into(),
            ));
        }
        match value {
            Value::Array(items) => pending.extend(items.iter().map(|value| (value, depth + 1))),
            Value::Object(fields) => {
                pending.extend(fields.values().map(|value| (value, depth + 1)));
            }
            _ => (),
        }
    }
    Ok(bytes)
}

pub(super) fn evaluate_predicate(
    predicate: &WorkflowPredicate,
    input: &CommittedValue,
    values: &BTreeMap<String, CommittedValue>,
) -> Result<CommittedValue, WorkflowRunError> {
    let mut result = CommittedValue::from(Value::Null);
    let boolean = match predicate {
        WorkflowPredicate::Boolean { value } => {
            let value = evaluate_value(value, input, values)?;
            result.depend_on(&value)?;
            value
                .value
                .as_bool()
                .ok_or_else(|| WorkflowRunError::InvalidValue("nonboolean predicate".into()))?
        }
        WorkflowPredicate::Equal { left, right } | WorkflowPredicate::NotEqual { left, right } => {
            let left = evaluate_value(left, input, values)?;
            let right = evaluate_value(right, input, values)?;
            result.depend_on(&left)?;
            result.depend_on(&right)?;
            (left.value == right.value) == matches!(predicate, WorkflowPredicate::Equal { .. })
        }
        WorkflowPredicate::Not { predicate } => {
            let value = evaluate_predicate(predicate, input, values)?;
            result.depend_on(&value)?;
            !value.value.as_bool().expect("predicate boolean")
        }
        WorkflowPredicate::And { predicates } | WorkflowPredicate::Or { predicates } => {
            let and = matches!(predicate, WorkflowPredicate::And { .. });
            let mut boolean = and;
            for predicate in predicates {
                let value = evaluate_predicate(predicate, input, values)?;
                result.depend_on(&value)?;
                let value = value.value.as_bool().expect("predicate boolean");
                if and {
                    boolean &= value;
                } else {
                    boolean |= value;
                }
                if boolean != and {
                    break;
                }
            }
            boolean
        }
    };
    result.value = Value::Bool(boolean);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constructed_values_reject_incompatible_candidate_versions() {
        let run = super::super::test_instance("applicability", "check")
            .block
            .run;
        let values = BTreeMap::from([
            (
                "a".into(),
                CommittedValue {
                    value: Value::Bool(true),
                    candidate: Some(crate::runtime::workspace::CandidateReference {
                        run: run.clone(),
                        version: 0,
                        content: "a".into(),
                    }),
                },
            ),
            (
                "b".into(),
                CommittedValue {
                    value: Value::Bool(true),
                    candidate: Some(crate::runtime::workspace::CandidateReference {
                        run,
                        version: 1,
                        content: "b".into(),
                    }),
                },
            ),
        ]);
        let expression = WorkflowValue::Array {
            items: vec![
                WorkflowValue::Reference {
                    path: vec!["a".into()],
                },
                WorkflowValue::Reference {
                    path: vec!["b".into()],
                },
            ],
        };
        assert!(
            matches!(evaluate_value(&expression, &Value::Null.into(), &values), Err(WorkflowRunError::InvalidValue(detail)) if detail.contains("incompatible candidate applicability"))
        );
    }
}
