//! Structural authoring schemas derived from native document types.
//! Cross-reference, trust, and execution semantics stay in native validators.

use serde_json::Value;
use std::collections::BTreeMap;

/// Generate the checked-in editor schemas, deterministically and offline.
///
/// # Panics
/// Only if an authoritative schema implementation produces non-serializable data.
#[must_use]
pub fn generate() -> BTreeMap<&'static str, Value> {
    let settings = super::launch::authoring_schema();
    let mut project = settings.clone();
    if let Some(properties) = project.get_mut("properties").and_then(Value::as_object_mut) {
        for &field in super::launch::USER_PATH_FIELDS
            .iter()
            .chain(super::launch::HOST_POLICY_FIELDS)
        {
            properties.remove(field);
        }
    }
    if let Some(properties) = project
        .pointer_mut("/$defs/McpAuthoring/properties")
        .and_then(Value::as_object_mut)
    {
        for &field in super::launch::MCP_SECRET_FIELDS {
            properties.remove(field);
        }
    }
    let mut schemas = BTreeMap::from([
        (
            "subagent.schema.json",
            serde_json::to_value(schemars::schema_for!(super::config::SubagentDocument))
                .expect("schema serializes"),
        ),
        (
            "models.schema.json",
            serde_json::to_value(schemars::schema_for!(crate::model::authoring::Catalog))
                .expect("schema serializes"),
        ),
        ("settings.schema.json", settings),
        ("rustx.schema.json", project),
        (
            "workflow.schema.json",
            serde_json::to_value(schemars::schema_for!(
                crate::runtime::workflow::WorkflowDefinition
            ))
            .expect("schema serializes"),
        ),
    ]);
    for (name, schema) in &mut schemas {
        if matches!(
            *name,
            "settings.schema.json" | "rustx.schema.json" | "models.schema.json"
        ) {
            toml_domain(schema);
        }
        schema.sort_all_objects();
    }
    schemas
}

// JSON Schema is an editor artifact. TOML has omission but no null literal;
// typed reset modes are described by the authoring enums, never Option's JSON null.
fn toml_domain(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.get("default") == Some(&Value::Null) {
                object.remove("default");
            }
            if let Some(Value::Array(types)) = object.get_mut("type") {
                types.retain(|t| t != "null");
            }
            for union in ["anyOf", "oneOf"] {
                if let Some(Value::Array(branches)) = object.get_mut(union) {
                    branches.retain(|branch| branch.get("type").is_none_or(|kind| kind != "null"));
                }
            }
            for child in object.values_mut() {
                toml_domain(child);
            }
        }
        Value::Array(array) => {
            for child in array {
                toml_domain(child);
            }
        }
        _ => {}
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn cfg235_checked_in_schemas_match_authoritative_generation() {
        for (name, schema) in super::generate() {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("schemas")
                .join(name);
            let actual = std::fs::read_to_string(&path).expect("checked-in schema");
            assert!(
                actual == format!("{}\n", serde_json::to_string_pretty(&schema).unwrap()),
                "{} drifted; run cargo run --example generate_schemas",
                path.display()
            );
        }
    }
}
