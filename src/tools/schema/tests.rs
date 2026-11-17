use std::sync::Arc;

use super::*;
use crate::runtime::identity::ToolId;
use crate::tools::types::{ToolOrigin, ToolReplayPolicy};

fn definition(policy: ToolExecutionPolicy, schema: Value) -> ToolDefinition {
    ToolDefinition {
        id: ToolId::new("example"),
        name: "example".into(),
        description: "Business operation".into(),
        input_schema: Arc::new(schema),
        execution_policy: policy,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

#[test]
fn static_tool_references_use_root_defs_for_provider_requests() {
    let canonical = json!({
        "$id": "https://example.test/tool",
        "type": "object",
        "properties": {
            "child": {"$ref": "#"},
            "value": {"$ref": "#/$defs/value"},
            "literal": {"const": {"$ref": "https://example.test/tool"}},
        },
        "$defs": {"value": {"type": "integer"}},
        "additionalProperties": false,
    });
    let model = compile_model_definition(&definition(
        ToolExecutionPolicy::ModelSelectable,
        canonical.clone(),
    ))
    .unwrap();
    let reference = model.input_schema["properties"]["arguments"]["$ref"]
        .as_str()
        .unwrap();
    assert!(reference.starts_with("#/$defs/"), "{reference}");
    let root = model.input_schema.pointer(&reference[1..]).unwrap();
    assert_eq!(root["type"], "object");
    let child_ref = root["properties"]["child"]["$ref"].as_str().unwrap();
    assert!(child_ref.starts_with("#/$defs/"));
    assert_eq!(child_ref, reference);
    assert_eq!(root["properties"]["value"]["type"], "integer");
    assert_eq!(
        root["properties"]["literal"]["const"],
        canonical["properties"]["literal"]["const"]
    );
    assert!(model.input_schema.get("definitions").is_none());
}

#[test]
fn single_use_open_objects_stay_inline_and_preserve_arbitrary_json() {
    let canonical = json!({
        "type":"object", "additionalProperties":false,
        "properties": {
            "action":{"type":"string", "enum":["create","list"]},
            "metadata":{"type":"object", "additionalProperties":true},
        }, "required":["action"],
    });
    let compiled = compile_model_definition(&definition(
        ToolExecutionPolicy::ForegroundOnly,
        canonical.clone(),
    ))
    .unwrap();
    assert_eq!(compiled.input_schema["properties"]["arguments"], canonical);
    assert!(compiled.input_schema.get("$defs").is_none());
    let validator = build_validator(&compiled.input_schema).unwrap();
    for metadata in [
        json!({}),
        json!({"nested":{"list":[1,null,true,"x",{}]}}),
        json!({"$ref":"literal instance data"}),
    ] {
        assert!(validator.is_valid(&json!({"arguments":{"action":"create", "metadata":metadata}})));
    }
    assert!(
        !validator.is_valid(&json!({"arguments":{"action":"create", "metadata":"not an object"}}))
    );
}

#[test]
fn shared_schemas_remain_shared_instead_of_expanding_a_dag() {
    let canonical = json!({
        "type":"object", "properties":{
            "left":{"$ref":"#/$defs/shared"},
            "right":{"$ref":"#/$defs/shared"},
        },
        "$defs":{"shared":{"type":"object", "properties":{"value":{"type":"string"}}}},
    });
    let compiled =
        compile_model_definition(&definition(ToolExecutionPolicy::ForegroundOnly, canonical))
            .unwrap();
    let root = &compiled.input_schema["properties"]["arguments"];
    assert_eq!(root["type"], "object");
    let reference = root["properties"]["left"]["$ref"].as_str().unwrap();
    assert_eq!(root["properties"]["right"]["$ref"], reference);
    assert_eq!(compiled.input_schema["$defs"].as_object().unwrap().len(), 1);
    assert_eq!(
        compiled.input_schema.pointer(&reference[1..]).unwrap()["properties"]["value"]["type"],
        "string"
    );
}

#[test]
fn wrapped_projection_preserves_business_semantics_and_metadata_names() {
    let cases = [
        json!({"type":"object","enum":[{}]}),
        json!({"type":"object","const":{"command":"status"}}),
        json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"propertyNames":{"pattern":"^[a-z]+$"}}),
        json!({"type":"object","patternProperties":{"^e":{"type":"integer"}},"additionalProperties":false}),
        json!({"type":"object","minProperties":1,"maxProperties":1}),
        json!({"type":"object","properties":{"execution_mode":{"type":"integer"},"elevation_reason":{"type":"boolean"},"__rustx_business":{}},"required":["execution_mode","elevation_reason"]}),
        json!({"type":"object","allOf":[{"required":["a"]}],"anyOf":[{"required":["b"]},{"required":["c"]}]}),
        json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"object","dependencies":{"command":["mode"]}}),
        json!({"type":"object","properties":{"child":{"$ref":"#"}},"additionalProperties":false}),
        json!({"type":"object","properties":{"literal":{"const":{"$ref":"#/definitions/value"}},"value":{"$ref":"#/definitions/value"}},"definitions":{"value":{"type":"integer"}}}),
    ];
    let values = [
        json!({}),
        json!({"command":"status"}),
        json!({"command":"status","mode":1}),
        json!({"elevation_reason":1}),
        json!({"execution_mode":1,"elevation_reason":true,"__rustx_business":7}),
        json!({"a":1,"b":2}),
        json!({"child":{"child":{}}}),
        json!({"child":{"extra":true}}),
        json!({"literal":{"$ref":"#/definitions/value"},"value":1}),
    ];
    for schema in cases {
        let canonical = build_validator(&schema).unwrap();
        for policy in [
            ToolExecutionPolicy::ForegroundOnly,
            ToolExecutionPolicy::BackgroundOnly,
            ToolExecutionPolicy::ModelSelectable,
        ] {
            let original = definition(policy, schema.clone());
            let compiled = compile_model_definition(&original).unwrap();
            assert_eq!(original.input_schema.as_ref(), &schema);
            let model = build_validator(&compiled.input_schema).unwrap();
            for business in &values {
                for reason in [None, Some(json!(null)), Some(json!("Request elevation"))] {
                    let mut envelope = json!({"arguments":business});
                    if let Some(reason) = reason {
                        envelope[ELEVATION_REASON_FIELD] = reason;
                    }
                    if policy == ToolExecutionPolicy::ModelSelectable {
                        envelope[EXECUTION_MODE_FIELD] = json!("foreground");
                    }
                    assert_eq!(
                        model.is_valid(&envelope),
                        canonical.is_valid(business),
                        "schema={schema}, call={envelope}"
                    );
                    assert_eq!(
                        resolve_invocation_metadata(policy, &envelope).unwrap().2,
                        *business
                    );
                }
            }
        }
    }
}

#[test]
fn decoder_rejects_flat_calls_unknown_metadata_and_missing_business() {
    for call in [
        json!({}),
        json!({"command":"status"}),
        json!({"arguments":[]}),
        json!({"arguments":{},"command":"status"}),
        json!({"arguments":{},"__rustx_approval":true}),
        json!({"arguments":{},"execution_mode":"foreground"}),
    ] {
        assert!(
            resolve_invocation_metadata(ToolExecutionPolicy::ForegroundOnly, &call).is_err(),
            "{call}"
        );
    }
    assert!(matches!(
        resolve_invocation_metadata(
            ToolExecutionPolicy::ModelSelectable,
            &json!({"arguments":{}})
        ),
        Err(SchemaError::MissingExecutionMode)
    ));
    assert!(matches!(
        resolve_invocation_metadata(
            ToolExecutionPolicy::ModelSelectable,
            &json!({"arguments":{},"execution_mode":"later"})
        ),
        Err(SchemaError::InvalidExecutionMode(_))
    ));
}

#[test]
fn decoder_enforces_elevation_utf8_budget_and_preserves_business_names() {
    for reason in [json!(null), json!("  "), json!("\n")] {
        assert_eq!(
            resolve_invocation_metadata(
                ToolExecutionPolicy::ForegroundOnly,
                &json!({"arguments":{},"elevation_reason":reason})
            )
            .unwrap()
            .1,
            None
        );
    }
    assert_eq!(resolve_invocation_metadata(ToolExecutionPolicy::ForegroundOnly,&json!({"arguments":{"elevation_reason":7,"execution_mode":false},"elevation_reason":"  permit  "})).unwrap(),(ToolInvocationMode::Foreground,Some("permit".into()),json!({"elevation_reason":7,"execution_mode":false})));
    for reason in [
        json!("界".repeat(171)),
        json!("x".repeat(513)),
        json!(false),
        json!(7),
        json!([]),
    ] {
        assert!(matches!(
            resolve_invocation_metadata(
                ToolExecutionPolicy::ForegroundOnly,
                &json!({"arguments":{},"elevation_reason":reason})
            ),
            Err(SchemaError::InvalidElevationReason(_))
        ));
    }
    assert!(
        resolve_invocation_metadata(
            ToolExecutionPolicy::ForegroundOnly,
            &json!({"arguments":{},"elevation_reason":"界".repeat(170)})
        )
        .is_ok()
    );
}

#[test]
fn only_selectable_policy_has_outer_execution_selector() {
    for (policy, mode) in [
        (
            ToolExecutionPolicy::ForegroundOnly,
            ToolInvocationMode::Foreground,
        ),
        (
            ToolExecutionPolicy::BackgroundOnly,
            ToolInvocationMode::Background,
        ),
        (
            ToolExecutionPolicy::ModelSelectable,
            ToolInvocationMode::Background,
        ),
    ] {
        let model =
            compile_model_definition(&definition(policy, json!({"type":"object"}))).unwrap();
        let selectable = policy == ToolExecutionPolicy::ModelSelectable;
        assert_eq!(
            model.input_schema["properties"]
                .get(EXECUTION_MODE_FIELD)
                .is_some(),
            selectable
        );
        let mut envelope = json!({"arguments":{}});
        if selectable {
            envelope[EXECUTION_MODE_FIELD] = json!("background");
            assert!(
                model
                    .description
                    .contains(EXECUTION_MODE_DESCRIPTION_REMINDER)
            );
        }
        assert_eq!(
            resolve_invocation_metadata(policy, &envelope).unwrap().0,
            mode
        );
    }
}

#[test]
fn invalid_schema_is_rejected_before_it_can_be_hidden_by_reference_rewriting() {
    for schema in [
        json!({"type":"object","properties":{"x":{"type":7}}}),
        json!({"$schema":"http://json-schema.org/draft-07/schema#","type":"object","$ref":"#/definitions/a","required":7,"definitions":{"a":{}}}),
        json!({"type":"object","$ref":"#/missing"}),
    ] {
        assert!(
            compile_model_definition(&definition(ToolExecutionPolicy::ForegroundOnly, schema))
                .is_err()
        );
    }
}

#[test]
fn business_validation_never_reads_a_schema_reference_from_the_host() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private.json");
    let marker = "business-validation-must-not-read-local-content";
    std::fs::write(&path, json!({"x-private-marker": marker}).to_string()).unwrap();
    let schema = json!({"type": "object", "$ref": format!("file://{}", path.display())});
    let error = validate_business_arguments(&schema, &json!({})).unwrap_err();
    assert!(matches!(error, SchemaError::InvalidSchema(_)));
    assert!(error.to_string().contains("does not fetch resources"));
    assert!(!error.to_string().contains(marker));
    assert!(
        compile_model_definition(&definition(ToolExecutionPolicy::ForegroundOnly, schema)).is_err()
    );
}

#[test]
fn dependency_defaults_cannot_reintroduce_schema_host_access() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private.json");
    std::fs::write(&path, r#"{"x-private-marker":"do-not-read"}"#).unwrap();
    let schema = json!({"type": "object", "$ref": format!("file://{}", path.display())});
    // This intentionally bypasses our explicit builder. The dependency feature
    // boundary also protects owner paths that construct their own validator.
    assert!(Validator::new(&schema).is_err());
}
