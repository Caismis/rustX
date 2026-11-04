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
        WorkflowValue::Template { .. } => Some("string"),
        WorkflowValue::Reference { .. } => None,
    }
}

pub(super) fn equality_type_supported(kind: &str) -> bool {
    matches!(kind, "boolean" | "string" | "null" | "number" | "integer")
}

pub(super) fn equality_types_compatible(left: Option<&str>, right: Option<&str>) -> bool {
    matches!(
        (left, right),
        (Some("number" | "integer"), Some("number" | "integer"))
    ) || left.is_some_and(|kind| equality_type_supported(kind) && Some(kind) == right)
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
                return Err(invalid("equality requires matching scalar types"));
            }
        }
        WorkflowPredicate::Greater { left, right }
        | WorkflowPredicate::GreaterEqual { left, right }
        | WorkflowPredicate::Less { left, right }
        | WorkflowPredicate::LessEqual { left, right } => {
            let left = value_schema(left, available, node, depth + 1)?;
            let right = value_schema(right, available, node, depth + 1)?;
            if !matches!(schema_type(&left), Some("number" | "integer"))
                || !matches!(schema_type(&right), Some("number" | "integer"))
            {
                return Err(invalid("numeric comparison requires two numeric operands"));
            }
        }
        WorkflowPredicate::Contains { left, right }
        | WorkflowPredicate::NotContains { left, right } => {
            let left_schema = value_schema(left, available, node, depth + 1)?;
            let right_schema = value_schema(right, available, node, depth + 1)?;
            let valid = match schema_type(&left_schema) {
                Some("array") => {
                    if let Some(item) = left_schema.get("items") {
                        super::schemas_compatible(&right_schema, item)
                            || matches!(
                                (schema_type(item), schema_type(&right_schema)),
                                (Some("number" | "integer"), Some("number" | "integer"))
                            )
                    } else if let Some(items) = left_schema.get("const").and_then(Value::as_array) {
                        items.iter().all(|item| {
                            equality_types_compatible(
                                Some(json_type(item)),
                                schema_type(&right_schema),
                            )
                        }) && schema_type(&right_schema).is_some_and(equality_type_supported)
                    } else {
                        false
                    }
                }
                Some("string") => {
                    schema_type(&right_schema) == Some("string")
                        && !matches!(right,WorkflowValue::Literal{value:Value::String(text)} if text.is_empty())
                }
                _ => false,
            };
            if !valid {
                return Err(invalid(
                    "containment requires strings or an array with a matching item",
                ));
            }
        }
        WorkflowPredicate::StartsWith { left, right }
        | WorkflowPredicate::EndsWith { left, right } => {
            if matches!(right,WorkflowValue::Literal{value:Value::String(text)} if text.is_empty())
            {
                return Err(invalid("string comparison requires a nonempty value"));
            }
            if schema_type(&value_schema(left, available, node, depth + 1)?) != Some("string")
                || schema_type(&value_schema(right, available, node, depth + 1)?) != Some("string")
            {
                return Err(invalid("string comparison requires two string operands"));
            }
        }
        WorkflowPredicate::Empty { value } | WorkflowPredicate::NotEmpty { value } => {
            value_schema(value, available, node, depth + 1)?;
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
    values: &imbl::OrdMap<String, Value>,
) -> Result<Value, WorkflowRunError> {
    let value = match expression {
        WorkflowValue::Reference { path } => {
            let first = path
                .first()
                .ok_or_else(|| WorkflowRunError::InvalidValue("empty reference".into()))?;
            let mut value = if first == "args" {
                Some(input)
            } else {
                Some(
                    values
                        .get(first)
                        .ok_or_else(|| WorkflowRunError::UncommittedReference(path.clone()))?,
                )
            };
            for key in &path[1..] {
                value = value.and_then(|value| value.get(key));
            }
            value.cloned().ok_or_else(|| {
                WorkflowRunError::InvalidValue(format!("uncommitted reference {path:?}"))
            })?
        }
        WorkflowValue::Literal { value } => value.clone(),
        WorkflowValue::Object { fields } => evaluate_object(
            fields.iter().map(|(key, value)| (key.as_str(), value)),
            input,
            values,
        )?,
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
        WorkflowValue::Template { parts } => {
            Value::String(super::model::render_parts(parts, |part| {
                evaluate_value(part, input, values)
            })?)
        }
    };
    bounded_value_bytes(&value)?;
    Ok(value)
}

/// Construct one bounded object, stopping before retaining an overflowing field.
/// Loop initializers and Object expressions share this construction boundary.
pub(super) fn evaluate_object<'a>(
    fields: impl IntoIterator<Item = (&'a str, &'a WorkflowValue)>,
    input: &Value,
    values: &imbl::OrdMap<String, Value>,
) -> Result<Value, WorkflowRunError> {
    let mut result = serde_json::Map::new();
    let mut bytes = 2;
    for (key, expression) in fields {
        let value = evaluate_value(expression, input, values)?;
        bytes += serde_json::to_vec(key).expect("JSON key").len()
            + bounded_value_bytes(&value)?
            + 1
            + usize::from(!result.is_empty());
        if bytes > MAX_VALUE_BYTES {
            return Err(WorkflowRunError::InvalidValue(
                "constructed object byte bound exceeded".into(),
            ));
        }
        result.insert(key.to_owned(), value);
    }
    Ok(Value::Object(result))
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

/// Dify's explicit empty operator, separate from the strict boolean predicate.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(value) => !*value,
        Value::Number(value) => value.as_f64() == Some(0.0),
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
    }
}

pub(super) fn evaluate_predicate(
    predicate: &WorkflowPredicate,
    input: &Value,
    values: &imbl::OrdMap<String, Value>,
) -> Result<bool, WorkflowRunError> {
    Ok(match predicate {
        WorkflowPredicate::Boolean { value } => evaluate_value(value, input, values)?
            .as_bool()
            .ok_or_else(|| WorkflowRunError::InvalidValue("nonboolean predicate".into()))?,
        WorkflowPredicate::Equal { left, right } | WorkflowPredicate::NotEqual { left, right } => {
            let left = evaluate_value(left, input, values)?;
            let right = evaluate_value(right, input, values)?;
            (if left.is_number() && right.is_number() {
                super::transform::compare(&left, &right) == Some(std::cmp::Ordering::Equal)
            } else {
                left == right
            }) == matches!(predicate, WorkflowPredicate::Equal { .. })
        }
        WorkflowPredicate::Greater { left, right }
        | WorkflowPredicate::GreaterEqual { left, right }
        | WorkflowPredicate::Less { left, right }
        | WorkflowPredicate::LessEqual { left, right } => {
            let left = evaluate_value(left, input, values)?;
            let right = evaluate_value(right, input, values)?;
            if !left.is_number() || !right.is_number() {
                return Err(WorkflowRunError::InvalidValue(
                    "non-numeric comparison operand".into(),
                ));
            }
            let order = super::transform::compare(&left, &right).ok_or_else(|| {
                WorkflowRunError::InvalidValue("numeric operands are not comparable".into())
            })?;
            match predicate {
                WorkflowPredicate::Greater { .. } => order.is_gt(),
                WorkflowPredicate::GreaterEqual { .. } => !order.is_lt(),
                WorkflowPredicate::Less { .. } => order.is_lt(),
                _ => !order.is_gt(),
            }
        }
        WorkflowPredicate::Contains { left, right }
        | WorkflowPredicate::NotContains { left, right } => {
            let left = evaluate_value(left, input, values)?;
            let right = evaluate_value(right, input, values)?;
            let contains = match left {
                Value::Array(items) => items.iter().any(|item| {
                    if item.is_number() && right.is_number() {
                        super::transform::compare(item, &right) == Some(std::cmp::Ordering::Equal)
                    } else {
                        item == &right
                    }
                }),
                Value::String(text) => text.contains(right.as_str().ok_or_else(|| {
                    WorkflowRunError::InvalidValue("non-string containment operand".into())
                })?),
                _ => {
                    return Err(WorkflowRunError::InvalidValue(
                        "containment requires a string or array".into(),
                    ));
                }
            };
            contains == matches!(predicate, WorkflowPredicate::Contains { .. })
        }
        WorkflowPredicate::StartsWith { left, right }
        | WorkflowPredicate::EndsWith { left, right } => {
            let left = evaluate_value(left, input, values)?;
            let right = evaluate_value(right, input, values)?;
            let left = left
                .as_str()
                .ok_or_else(|| WorkflowRunError::InvalidValue("nonstring left operand".into()))?;
            let right = right
                .as_str()
                .ok_or_else(|| WorkflowRunError::InvalidValue("nonstring right operand".into()))?;
            if matches!(predicate, WorkflowPredicate::StartsWith { .. }) {
                left.starts_with(right)
            } else {
                left.ends_with(right)
            }
        }
        WorkflowPredicate::Empty { value } | WorkflowPredicate::NotEmpty { value } => {
            let value = evaluate_value(value, input, values)?;
            let empty = is_empty(&value);
            empty == matches!(predicate, WorkflowPredicate::Empty { .. })
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
    /// Operators use Dify's type-specific vocabulary without string coercion or
    /// rounding integers through f64. Compile and execution must agree.
    #[test]
    fn typed_conditions_compare_numbers_arrays_and_explicit_empty_values() {
        use serde_json::json;
        let values = imbl::OrdMap::new();
        for (operator, left, right, expected) in [
            (
                "greater",
                json!(9_007_199_254_740_993u64),
                json!(9_007_199_254_740_992u64),
                true,
            ),
            ("less", json!(-3), json!(-2.5), true),
            ("greater_equal", json!(2), json!(2.0), true),
            ("less_equal", json!(2.0), json!(2), true),
            ("equal", json!(2), json!(2.0), true),
            ("not_equal", json!(2), json!(2.0), false),
            ("greater", json!(2), json!(2), false),
            ("contains", json!([1, 2, 3]), json!(2.0), true),
            ("not_contains", json!([true, false]), json!(false), false),
            ("contains", json!(["a", "b"]), json!("b"), true),
            ("contains", json!([]), json!(1), false),
        ] {
            let predicate:WorkflowPredicate=serde_json::from_value(json!({"type":operator,"left":{"type":"literal","value":left},"right":{"type":"literal","value":right}})).unwrap();
            validate_predicate(&predicate, &SchemaMap::default(), "condition", 0).unwrap();
            assert_eq!(
                evaluate_predicate(&predicate, &Value::Null, &values).unwrap(),
                expected,
                "{operator} {left} {right}"
            );
        }
        for value in [
            json!(null),
            json!(false),
            json!(0),
            json!(0.0),
            json!(""),
            json!([]),
            json!({}),
        ] {
            for (operator, expected) in [("empty", true), ("not_empty", false)] {
                let predicate = serde_json::from_value(
                    json!({"type":operator,"value":{"type":"literal","value":value}}),
                )
                .unwrap();
                validate_predicate(&predicate, &SchemaMap::default(), "condition", 0).unwrap();
                assert_eq!(
                    evaluate_predicate(&predicate, &Value::Null, &values).unwrap(),
                    expected,
                    "{operator} {value}"
                );
            }
        }
        for value in [
            json!(true),
            json!(1),
            json!(" "),
            json!([null]),
            json!({"key":null}),
        ] {
            let predicate = WorkflowPredicate::NotEmpty {
                value: WorkflowValue::Literal { value },
            };
            assert!(evaluate_predicate(&predicate, &Value::Null, &values).unwrap());
        }
        for (operator, left, right) in [
            ("greater", json!("2"), json!(1)),
            ("equal", json!(2), json!("2")),
            ("contains", json!([1]), json!("1")),
            ("equal", json!({}), json!({})),
        ] {
            let predicate=serde_json::from_value(json!({"type":operator,"left":{"type":"literal","value":left},"right":{"type":"literal","value":right}})).unwrap();
            assert!(
                validate_predicate(&predicate, &SchemaMap::default(), "condition", 0).is_err(),
                "{operator} {left} {right}"
            );
        }
    }

    /// Values carry no filesystem version: construction composes committed
    /// data from any producers, and an uncommitted reference still fails.
    #[test]
    fn constructed_values_compose_committed_data_and_reject_uncommitted_references() {
        let values = imbl::OrdMap::from(vec![
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
            Err(WorkflowRunError::UncommittedReference(path)) if path == ["missing"]
        ));
    }

    /// A template is one string: text as is, references as their string or
    /// JSON text. Its parts are text or references only, in scope.
    #[test]
    fn templates_render_text_and_references_into_one_string() {
        let values = imbl::OrdMap::from(vec![(
            "search".to_owned(),
            serde_json::json!({"topic": "rust", "count": 3}),
        )]);
        let text = |text: &str| WorkflowValue::Literal {
            value: Value::String(text.to_owned()),
        };
        let reference = |path: &[&str]| WorkflowValue::Reference {
            path: path.iter().map(|key| (*key).to_owned()).collect(),
        };
        let template = WorkflowValue::Template {
            parts: vec![
                text("about "),
                reference(&["search", "topic"]),
                text(", top "),
                reference(&["search", "count"]),
            ],
        };
        assert_eq!(
            evaluate_value(&template, &Value::Null, &values).unwrap(),
            serde_json::json!("about rust, top 3")
        );
        let available = SchemaMap(BTreeMap::from([(
            "search".to_owned(),
            serde_json::json!({"type":"object","properties":{"topic":{"type":"string"},"count":{"type":"integer"}},"required":["topic","count"],"additionalProperties":false}),
        )]));
        assert_eq!(
            value_schema(&template, &available, "tool", 0).unwrap(),
            serde_json::json!({"type": "string"})
        );
        let nested = WorkflowValue::Template {
            parts: vec![WorkflowValue::Array { items: vec![] }],
        };
        assert!(value_schema(&nested, &available, "tool", 0).is_err());
        let unavailable = WorkflowValue::Template {
            parts: vec![reference(&["missing"])],
        };
        assert!(value_schema(&unavailable, &available, "tool", 0).is_err());
    }
    #[test]
    fn bounded_object_construction_stops_on_the_second_large_field() {
        let fields = (0..128)
            .map(|index| {
                (
                    format!("v{index}"),
                    WorkflowValue::Reference {
                        path: vec!["args".into(), "payload".into()],
                    },
                )
            })
            .collect::<Vec<_>>();
        let evaluated = std::cell::Cell::new(0);
        let result = evaluate_object(
            fields
                .iter()
                .inspect(|_| evaluated.set(evaluated.get() + 1))
                .map(|(key, value)| (key.as_str(), value)),
            &serde_json::json!({"payload":"x".repeat(60_000)}),
            &imbl::OrdMap::new(),
        );
        assert!(
            matches!(result, Err(WorkflowRunError::InvalidValue(message)) if message == "constructed object byte bound exceeded")
        );
        assert_eq!(
            evaluated.get(),
            2,
            "remaining initializers must not be evaluated"
        );
    }
}
