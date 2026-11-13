use super::*;
use crate::runtime::identity::ToolId;
use crate::tools::types::{ToolOrigin, ToolReplayPolicy};

fn definition(policy: ToolExecutionPolicy, schema: Value) -> ToolDefinition {
    ToolDefinition {
        id: ToolId::new("example"),
        name: "example".into(),
        description: "Business operation".into(),
        input_schema: schema,
        execution_policy: policy,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
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
        let canonical = Validator::new(&schema).unwrap();
        for policy in [
            ToolExecutionPolicy::ForegroundOnly,
            ToolExecutionPolicy::BackgroundOnly,
            ToolExecutionPolicy::ModelSelectable,
        ] {
            let original = definition(policy, schema.clone());
            let compiled = compile_model_definition(&original).unwrap();
            assert_eq!(original.input_schema, schema);
            let model = Validator::new(&compiled.input_schema).unwrap();
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
