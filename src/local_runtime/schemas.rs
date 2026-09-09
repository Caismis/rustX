//! Structural authoring schemas derived from native document types.
//! Cross-reference, trust, and execution semantics stay in native validators.

use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn camel_case(name: &str) -> String {
    let mut uppercase = false;
    name.chars()
        .filter_map(|character| {
            if character == '_' {
                uppercase = true;
                None
            } else if uppercase {
                uppercase = false;
                Some(character.to_ascii_uppercase())
            } else {
                Some(character)
            }
        })
        .collect()
}

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
        .pointer_mut("/$defs/McpServerDocument/properties")
        .and_then(Value::as_object_mut)
    {
        for &field in super::launch::MCP_SECRET_FIELDS {
            properties.remove(field);
        }
    }
    let mut schemas = BTreeMap::from([
        (
            "models.schema.json",
            serde_json::to_value(schemars::schema_for!(
                crate::model::catalog::ModelCatalogDocument
            ))
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
    for schema in schemas.values_mut() {
        schema.sort_all_objects();
    }
    schemas
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
