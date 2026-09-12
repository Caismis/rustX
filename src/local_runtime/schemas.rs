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
            "agent.schema.json",
            serde_json::to_value(schemars::schema_for!(super::config::AgentProfileDocument))
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
            "settings.schema.json"
                | "rustx.schema.json"
                | "models.schema.json"
                | "agent.schema.json"
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
    use crate::runtime::workflow::{WorkflowDefinition, WorkflowNodeDefinition};
    use serde_json::json;

    fn workflow(node: &serde_json::Value, tools: &serde_json::Value) -> serde_json::Value {
        json!({
            "description":"selector contract", "tools":tools,
            "block":{
                "input":{"type":"object"}, "output":{"type":"object"}, "entry":"work",
                "nodes":{
                    "work":node,
                    "done":{"type":"return","output":{"type":"reference","path":["work"]}}
                },
                "edges":[{"from":"work","to":"done"}]
            }
        })
    }

    #[test]
    fn workflow_tool_authoring_and_schema_are_exact_only() {
        use crate::capabilities::selection::ExactToolSelector;
        let schema = super::generate()["workflow.schema.json"].clone();
        let validator = jsonschema::Validator::new(&schema).unwrap();
        let variants = schema["$defs"]["ExactToolSelector"]["oneOf"]
            .as_array()
            .unwrap();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0]["properties"]["origin"]["const"], "builtin");
        assert_eq!(variants[1]["properties"]["origin"]["const"], "source");
        for selector in [
            json!({"origin":"builtin","name":"read"}),
            json!({"origin":"source","source_id":"github","name":"get_issue"}),
            json!({"origin":"source","source_id":"python:data-analysis","name":"run_python"}),
            json!({"origin":"source","source_id":"a/b ? 工具","name":" leading / 工具*? "}),
        ] {
            let node = json!({"type":"tool","selector":selector,
                "arguments":{"type":"literal","value":{}},
                "result":{"type":"json","part":0,"schema":{"type":"object"}}});
            let document = workflow(&node, &json!([selector]));
            assert!(validator.is_valid(&document));
            let parsed: WorkflowDefinition =
                serde_yaml::from_str(&serde_yaml::to_string(&document).unwrap()).unwrap();
            let WorkflowNodeDefinition::Tool {
                selector: exact, ..
            } = &parsed.block.nodes["work"]
            else {
                panic!("Tool node")
            };
            // Exhaustive: every authored executable selector has one exact name.
            let name = match exact {
                ExactToolSelector::Builtin { name } | ExactToolSelector::Source { name, .. } => {
                    name
                }
            };
            assert_eq!(name, selector["name"].as_str().unwrap());
            for pointer in ["/tools/0", "/block/nodes/work/selector"] {
                let mut invalid = document.clone();
                *invalid.pointer_mut(pointer).unwrap() =
                    json!({"origin":"all","source_id":"github"});
                assert!(!validator.is_valid(&invalid), "{pointer}");
                assert!(
                    serde_yaml::from_str::<WorkflowDefinition>(
                        &serde_yaml::to_string(&invalid).unwrap()
                    )
                    .is_err(),
                    "{pointer}"
                );
            }
        }
    }

    #[test]
    fn workflow_agent_override_authoring_and_schema_keep_all_and_exact() {
        use crate::capabilities::selection::SourceToolSelection;
        let schema = super::generate()["workflow.schema.json"].clone();
        let validator = jsonschema::Validator::new(&schema).unwrap();
        for (selection, expected) in [
            (json!("all"), SourceToolSelection::All),
            (
                json!(["get_issue"]),
                SourceToolSelection::Exact(vec!["get_issue".into()]),
            ),
        ] {
            let document = workflow(
                &json!({"type":"agent","profile":"reviewer","task":"Review",
                "output":{"type":"object"},"override":{"tools":{"sources":{"github":selection}}}}),
                &json!([]),
            );
            assert!(validator.is_valid(&document));
            let parsed: WorkflowDefinition =
                serde_yaml::from_str(&serde_yaml::to_string(&document).unwrap()).unwrap();
            let WorkflowNodeDefinition::Agent {
                invocation_override: Some(overrides),
                ..
            } = &parsed.block.nodes["work"]
            else {
                panic!("Agent override")
            };
            assert_eq!(
                overrides.tools.as_ref().unwrap().sources.values().next(),
                Some(&expected)
            );
            crate::runtime::workflow::WorkflowProgram::compile(
                crate::runtime::workflow::WorkflowId::parse("selector-contract").unwrap(),
                parsed,
                &[crate::runtime::subagent::SubagentName::parse("reviewer").unwrap()].into(),
            )
            .unwrap();
        }
    }

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
