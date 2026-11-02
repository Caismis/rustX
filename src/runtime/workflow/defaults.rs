//! Input normalization is a bounded construction, including inserted defaults.
use std::io::{self, Write};

use super::{MAX_VALUE_BYTES, MAX_VALUE_DEPTH, Value, WorkflowLimit, WorkflowRunError};

const MAX_NORMALIZATION_WORK: usize = 65_536;

#[derive(Default)]
struct Budget {
    bytes: usize,
    work: usize,
}

fn exceeded() -> WorkflowRunError {
    WorkflowRunError::LimitExceeded(WorkflowLimit::RetainedData)
}

impl Budget {
    fn visit(&mut self, depth: usize) -> Result<(), WorkflowRunError> {
        if depth > MAX_VALUE_DEPTH || self.work == MAX_NORMALIZATION_WORK {
            return Err(exceeded());
        }
        self.work += 1;
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<(), WorkflowRunError> {
        if bytes > MAX_VALUE_BYTES - self.bytes {
            return Err(exceeded());
        }
        self.bytes += bytes;
        Ok(())
    }

    /// Count escaped JSON without allocating a serialization buffer. A scalar
    /// or key is copied only after its entire encoding fits the shared budget.
    fn serialized(&mut self, value: &impl serde::Serialize) -> Result<(), WorkflowRunError> {
        serde_json::to_writer(self, value).map_err(|_| exceeded())
    }

    fn property(&mut self, name: &str, first: bool) -> Result<(), WorkflowRunError> {
        self.charge(usize::from(!first) + 1)?; // comma and colon
        self.serialized(&name)
    }
}

impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.charge(bytes.len())
            .map_err(|_| io::Error::other("Workflow default expansion budget exceeded"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn normalize(
    schema: Option<&Value>,
    value: &Value,
    budget: &mut Budget,
    depth: usize,
) -> Result<Value, WorkflowRunError> {
    budget.visit(depth)?;
    match value {
        Value::Object(fields) => {
            budget.charge(2)?; // braces
            let properties = schema.and_then(|schema| schema.get("properties")?.as_object());
            let mut result = serde_json::Map::new();
            for (name, field) in fields {
                budget.property(name, result.is_empty())?;
                let field = normalize(
                    properties.and_then(|p| p.get(name)),
                    field,
                    budget,
                    depth + 1,
                )?;
                result.insert(name.clone(), field);
            }
            if let Some(properties) = properties {
                for (name, property) in properties {
                    budget.visit(depth)?;
                    if !fields.contains_key(name)
                        && let Some(default) = property.get("default")
                    {
                        budget.property(name, result.is_empty())?;
                        let field = normalize(Some(property), default, budget, depth + 1)?;
                        result.insert(name.clone(), field);
                    }
                }
            }
            Ok(Value::Object(result))
        }
        Value::Array(items) => {
            budget.charge(2)?; // brackets
            let item_schema = schema.and_then(|schema| schema.get("items"));
            let mut result = Vec::new();
            for item in items {
                budget.charge(usize::from(!result.is_empty()))?;
                result.push(normalize(item_schema, item, budget, depth + 1)?);
            }
            Ok(Value::Array(result))
        }
        _ => {
            budget.serialized(value)?;
            Ok(value.clone())
        }
    }
}

/// Fill absent defaults recursively, across one shared byte and work budget.
/// On failure, the caller's input is unchanged and the partial result retires.
///
/// # Errors
/// Returns a retained-data limit error before construction exceeds its byte,
/// depth or work budget.
pub(crate) fn apply_schema_defaults(
    schema: &Value,
    value: &mut Value,
) -> Result<(), WorkflowRunError> {
    *value = normalize(Some(schema), value, &mut Budget::default(), 0)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fanout_schema(depth: usize) -> Value {
        (0..depth).fold(json!({"type":"object","properties":{},"additionalProperties":false}), |child, _| {
            json!({"type":"object","properties":{"rows":{"type":"array","items":child,"default":vec![json!({});8]}},"additionalProperties":false})
        })
    }

    #[test]
    fn expansion_stops_during_construction_with_one_budget_for_every_descendant() {
        for depth in [5, 7, 10] {
            let schema = fanout_schema(depth);
            let mut budget = Budget::default();
            assert!(matches!(
                normalize(Some(&schema), &json!({}), &mut budget, 0),
                Err(WorkflowRunError::LimitExceeded(WorkflowLimit::RetainedData))
            ));
            assert!(budget.bytes <= MAX_VALUE_BYTES);
            assert!(budget.work <= MAX_NORMALIZATION_WORK);
            assert!(budget.bytes > MAX_VALUE_BYTES / 2);
            let mut input = json!({});
            assert!(apply_schema_defaults(&schema, &mut input).is_err());
            assert_eq!(input, json!({}));
        }
    }

    #[test]
    fn exact_byte_limit_counts_escaped_keys_strings_and_container_punctuation() {
        for input in [json!({"quote\"": ["\\\n", null, 1, true]}), json!([])] {
            let mut budget = Budget::default();
            assert_eq!(normalize(None, &input, &mut budget, 0).unwrap(), input);
            assert_eq!(budget.bytes, serde_json::to_vec(&input).unwrap().len());
        }
        let schema = json!({"type":"object","properties":{"text":{"type":"string","default":"x".repeat(MAX_VALUE_BYTES - 11)}}});
        let mut input = json!({});
        apply_schema_defaults(&schema, &mut input).unwrap();
        assert_eq!(serde_json::to_vec(&input).unwrap().len(), MAX_VALUE_BYTES);
        let mut budget = Budget::default();
        assert!(normalize(None, &json!("x".repeat(MAX_VALUE_BYTES)), &mut budget, 0).is_err());
    }
}
