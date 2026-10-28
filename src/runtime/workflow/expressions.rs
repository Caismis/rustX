//! Conservative typed construction and lexical reference resolution.
use super::{
    BTreeMap, MAX_REFERENCE_COMPONENTS, MAX_VALUE_BYTES, MAX_VALUE_DEPTH, SchemaMap, Serialize,
    Value, WorkflowCompileError, WorkflowPredicate, WorkflowRunError, WorkflowValue, schema_type,
};
mod inference;
pub(super) use inference::{InferenceError, Schema, SchemaBudget, SchemaInference};

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
    SchemaInference::new(available, node, SchemaBudget::unlimited())
        .infer(expression, depth, None)
        .map(|schema| schema.value().clone())
        .map_err(InferenceError::into_compile_error)
}

/// Resolve lexical references without copying their potentially large contract.
fn reference_schema_with<'a, E: From<WorkflowCompileError>>(
    path: &[String],
    available: &'a SchemaMap,
    node: &str,
    admit: &mut impl FnMut(usize) -> Result<(), E>,
) -> Result<&'a Value, E> {
    admit(64)?;
    if path.is_empty()
        || path.len() > MAX_REFERENCE_COMPONENTS
        || path.iter().any(|key| key.is_empty() || key.len() > 64)
    {
        return Err(invalid("invalid reference path").into());
    }
    let mut schema = available.0.get(&path[0]).ok_or_else(|| {
        E::from(invalid(format!(
            "node {node} references unavailable producer {:?}",
            path[0]
        )))
    })?;
    for key in &path[1..] {
        admit(key.len().saturating_mul(16).saturating_add(64))?;
        let mut required = false;
        if let Some(fields) = schema.get("required").and_then(Value::as_array) {
            for field in fields {
                admit(
                    field
                        .as_str()
                        .map_or(64, |field| field.len() + key.len() + 64),
                )?;
                if field.as_str() == Some(key) {
                    required = true;
                    break;
                }
            }
        }
        if schema_type(schema) != Some("object") || !required {
            return Err(invalid(format!("node {node}: unavailable field in {path:?}")).into());
        }
        schema = schema
            .get("properties")
            .and_then(|properties| properties.get(key))
            .ok_or_else(|| E::from(invalid(format!("invalid field path {path:?}"))))?;
    }
    if schema.is_null() {
        return Err(invalid("reference schema is not yet known").into());
    }
    Ok(schema)
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

/// Constructor shape is known even while its fields/items are unfinished.
pub(super) fn intrinsic_value_type(value: &WorkflowValue) -> Option<&'static str> {
    match value {
        WorkflowValue::Literal { value } => Some(json_type(value)),
        WorkflowValue::Object { .. } => Some("object"),
        WorkflowValue::Array { .. } => Some("array"),
        WorkflowValue::Reference { .. } => None,
    }
}

pub(super) fn equality_type_supported(kind: &str) -> bool {
    matches!(kind, "boolean" | "string" | "null")
}

pub(super) fn equality_types_compatible(left: Option<&str>, right: Option<&str>) -> bool {
    left.is_some_and(|kind| equality_type_supported(kind) && Some(kind) == right)
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
            if !equality_types_compatible(schema_type(&left), schema_type(&right)) {
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

pub(super) fn evaluate_value(
    expression: &WorkflowValue,
    input: &Value,
    values: &BTreeMap<String, Value>,
) -> Result<Value, WorkflowRunError> {
    let value = match expression {
        WorkflowValue::Reference { path } => {
            let first = path
                .first()
                .ok_or_else(|| WorkflowRunError::InvalidValue("empty reference".into()))?;
            let mut value = if first == "args" {
                Some(input)
            } else {
                values.get(first)
            };
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
                    + serde_json::to_vec(&value).expect("JSON value").len()
                    + 2;
                if bytes.saturating_sub(usize::from(!fields.is_empty())) > MAX_VALUE_BYTES {
                    return Err(WorkflowRunError::InvalidValue(
                        "constructed object byte bound exceeded".into(),
                    ));
                }
                result.insert(key.clone(), value);
            }
            Value::Object(result)
        }
        WorkflowValue::Array { items } => {
            let mut result = Vec::new();
            let mut bytes = 2;
            for item in items {
                let value = evaluate_value(item, input, values)?;
                bytes += serde_json::to_vec(&value).expect("JSON value").len() + 1;
                if bytes - 1 > MAX_VALUE_BYTES {
                    return Err(WorkflowRunError::InvalidValue(
                        "constructed array byte bound exceeded".into(),
                    ));
                }
                result.push(value);
            }
            Value::Array(result)
        }
    };
    bounded_value_bytes(&value)?;
    Ok(value)
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
    input: &Value,
    values: &BTreeMap<String, Value>,
) -> Result<bool, WorkflowRunError> {
    Ok(match predicate {
        WorkflowPredicate::Boolean { value } => evaluate_value(value, input, values)?
            .as_bool()
            .ok_or_else(|| WorkflowRunError::InvalidValue("nonboolean predicate".into()))?,
        WorkflowPredicate::Equal { left, right } | WorkflowPredicate::NotEqual { left, right } => {
            let left = evaluate_value(left, input, values)?;
            let right = evaluate_value(right, input, values)?;
            (left == right) == matches!(predicate, WorkflowPredicate::Equal { .. })
        }
        WorkflowPredicate::Not { predicate } => !evaluate_predicate(predicate, input, values)?,
        WorkflowPredicate::And { predicates } | WorkflowPredicate::Or { predicates } => {
            let and = matches!(predicate, WorkflowPredicate::And { .. });
            let mut boolean = and;
            for predicate in predicates {
                let value = evaluate_predicate(predicate, input, values)?;
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Values carry no filesystem version: construction composes committed
    /// data from any producers, and an uncommitted reference still fails.
    #[test]
    fn constructed_values_compose_committed_data_and_reject_uncommitted_references() {
        let values = BTreeMap::from([
            ("a".to_owned(), serde_json::json!({"passed": true})),
            ("b".to_owned(), serde_json::json!({"passed": false})),
        ]);
        let reference = |path: &[&str]| WorkflowValue::Reference {
            path: path.iter().map(|key| (*key).to_owned()).collect(),
        };
        let expression = WorkflowValue::Array {
            items: vec![reference(&["a", "passed"]), reference(&["b", "passed"])],
        };
        assert_eq!(
            evaluate_value(&expression, &Value::Null, &values).unwrap(),
            serde_json::json!([true, false])
        );
        let predicate = WorkflowPredicate::NotEqual {
            left: reference(&["a", "passed"]),
            right: reference(&["b", "passed"]),
        };
        assert!(evaluate_predicate(&predicate, &Value::Null, &values).unwrap());
        assert!(matches!(
            evaluate_value(&reference(&["missing"]), &Value::Null, &values),
            Err(WorkflowRunError::InvalidValue(detail)) if detail.contains("uncommitted reference")
        ));
    }
}
