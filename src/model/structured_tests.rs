use super::*;
use serde_json::json;

fn definition() -> Value {
    json!({"type":"object","properties":{"label":{"type":"string","enum":["yes","no"]}},"required":["label"],"additionalProperties":false})
}

fn schema() -> ResponseSchema {
    ResponseSchema::new("classification".into(), definition(), 128).unwrap()
}

#[test]
fn accepts_only_one_complete_validated_object() {
    let schema = schema();
    assert_eq!(
        schema.validate_output(" \n{\"label\":\"yes\"}\n").unwrap(),
        json!({"label":"yes"})
    );
    for text in [
        "",
        "not JSON",
        "{}",
        "[]",
        "null",
        "{\"label\":1}",
        "{\"label\":\"maybe\"}",
        "{\"label\":\"yes\",\"extra\":true}",
        "```json\n{\"label\":\"yes\"}\n```",
        "prefix {\"label\":\"yes\"}",
        "{\"label\":\"yes\"} {}",
    ] {
        let error = schema.validate_output(text).unwrap_err();
        assert_eq!(
            error.retry_disposition,
            ModelRetryDisposition::Never,
            "{text}"
        );
    }
    let error = schema.validate_output(&" ".repeat(129)).unwrap_err();
    assert_eq!(error.kind, ModelErrorKind::GenerationBudgetExceeded);
}

#[test]
fn unsupported_schema_reports_owning_path_without_dropping_constraints() {
    for keyword in [
        "oneOf",
        "$ref",
        "$defs",
        "minimum",
        "maxLength",
        "const",
        "pattern",
    ] {
        let mut value = definition();
        value["properties"]["label"][keyword] = json!("sensitive constraint");
        let error = ResponseSchema::new("test".into(), value, 128).unwrap_err();
        assert!(error.message.contains(&format!(
            "response_schema.schema.properties.label.{keyword}"
        )));
        assert!(!error.message.contains("sensitive constraint"));
    }
    for value in [
        json!({"type":"string"}),
        json!({"type":"object"}),
        json!({"type":"object","properties":{},"required":[],"additionalProperties":true}),
    ] {
        assert!(ResponseSchema::new("test".into(), value, 128).is_err());
    }
}

#[test]
fn strict_objects_require_every_property_and_typed_unique_enums() {
    for required in [json!([]), json!(["other"]), json!(["label", "label"])] {
        let mut value = definition();
        value["required"] = required;
        assert!(
            ResponseSchema::new("test".into(), value, 128)
                .unwrap_err()
                .message
                .contains("required")
        );
    }
    for values in [json!([]), json!(["yes", "yes"]), json!([1])] {
        let mut value = definition();
        value["properties"]["label"]["enum"] = values;
        assert!(
            ResponseSchema::new("test".into(), value, 128)
                .unwrap_err()
                .message
                .contains("enum")
        );
    }
}

#[test]
fn enum_limits_apply_to_the_whole_schema_and_large_string_enums() {
    let mut value = definition();
    let numbers: Vec<_> = (0..500).collect();
    value["properties"] =
        json!({"a":{"type":"integer","enum":numbers},"b":{"type":"integer","enum":numbers}});
    value["required"] = json!(["a", "b"]);
    ResponseSchema::new("test".into(), value.clone(), 128).unwrap();
    value["properties"]["b"]["enum"]
        .as_array_mut()
        .unwrap()
        .push(json!(500));
    let error = ResponseSchema::new("test".into(), value, 128).unwrap_err();
    assert!(error.message.contains("properties.b.enum"));
    assert!(error.message.contains("1000"));
    let mut value = definition();
    let strings: Vec<_> = (0..251)
        .map(|index| format!("{index:03}{}", "s".repeat(57)))
        .collect();
    value["properties"]["label"]["enum"] = json!(strings);
    assert!(serde_json::to_vec(&value).unwrap().len() < RESPONSE_SCHEMA_BYTES_MAX);
    assert!(
        ResponseSchema::new("test".into(), value, 128)
            .unwrap_err()
            .message
            .contains("15000")
    );
}

#[test]
fn oversized_schema_is_rejected_before_semantic_paths_or_enum_comparisons() {
    let mut value = definition();
    value["properties"]["label"]["enum"] = json!((0..100_000).collect::<Vec<_>>());
    let error = ResponseSchema::new("test".into(), value, 128).unwrap_err();
    assert_eq!(
        error.message,
        "response_schema.schema: encoded schema exceeds 16384 bytes"
    );
    let key = "sensitive".repeat(RESPONSE_SCHEMA_BYTES_MAX);
    let value = json!({key: false});
    let error = ResponseSchema::new("test".into(), value, 128).unwrap_err();
    assert!(error.message.len() < 100);
    assert!(!error.message.contains("sensitive"));
    let mut value = json!(false);
    for _ in 0..64 {
        value = json!([value]);
    }
    assert!(
        ResponseSchema::new("test".into(), value, 128)
            .unwrap_err()
            .message
            .contains("nesting")
    );
}

#[test]
fn schema_and_output_budgets_apply_before_dispatch_and_on_deserialization() {
    assert!(ResponseSchema::new("invalid name".into(), definition(), 128).is_err());
    for budget in [0, STRUCTURED_OUTPUT_BYTES_MAX + 1] {
        assert!(ResponseSchema::new("test".into(), definition(), budget).is_err());
    }
    let mut value = definition();
    value["description"] = json!("s".repeat(RESPONSE_SCHEMA_BYTES_MAX));
    assert!(
        ResponseSchema::new("test".into(), value, 128)
            .unwrap_err()
            .message
            .contains("bytes")
    );
    let mut child = json!({"type":"string"});
    for _ in 0..RESPONSE_SCHEMA_DEPTH_MAX {
        child = json!({"type":"array","items":child});
    }
    let mut value = definition();
    value["properties"]["label"] = child;
    assert!(
        ResponseSchema::new("test".into(), value, 128)
            .unwrap_err()
            .message
            .contains("depth")
    );
    let valid = serde_json::to_value(schema()).unwrap();
    assert_eq!(
        serde_json::from_value::<ResponseSchema>(valid.clone()).unwrap(),
        schema()
    );
    let mut invalid = valid;
    invalid["schema"]["$ref"] = json!("https://invalid.example/private");
    assert!(serde_json::from_value::<ResponseSchema>(invalid).is_err());
}

#[test]
fn nested_array_objects_are_validated_and_diagnostics_omit_sensitive_input() {
    let schema = ResponseSchema::new("sensitive_name".into(), json!({"type":"object","properties":{"items":{"type":"array","items":{"type":"object","properties":{"secret":{"type":"integer"}},"required":["secret"],"additionalProperties":false}}},"required":["items"],"additionalProperties":false}), 1024).unwrap();
    assert!(
        schema
            .validate_output(r#"{"items":[{"secret":42}]}"#)
            .is_ok()
    );
    let error = schema
        .validate_output(r#"{"items":[{"secret":"sensitive_value"}]}"#)
        .unwrap_err();
    assert!(!format!("{schema:?}{error:?}").contains("sensitive"));
    assert!(!format!("{schema:?}{error:?}").contains("secret"));
}

#[test]
fn prepared_schema_strategy_and_budget_are_fingerprinted_and_estimated() {
    use crate::context::tokens::{DefaultTokenEstimator, TokenEstimator};
    use crate::model::ModelProtocol;
    for protocol in [
        ModelProtocol::OpenAiChatCompletions,
        ModelProtocol::OpenAiResponses,
        ModelProtocol::AnthropicMessages,
    ] {
        let mut request =
            crate::scripted_suites::common::simple_request(protocol, "test", "source");
        let base_tokens = DefaultTokenEstimator.estimate_frozen_request(&request);
        let text_fingerprint = request.semantic_input_fingerprint();
        request.response_schema = Some(PreparedResponseSchema {
            schema: schema(),
            strategy: StructuredOutputStrategy::Native,
        });
        let native_fingerprint = request.semantic_input_fingerprint();
        assert_ne!(native_fingerprint, text_fingerprint);
        assert!(DefaultTokenEstimator.estimate_frozen_request(&request) > base_tokens);
        request.response_schema.as_mut().unwrap().strategy = StructuredOutputStrategy::Prompt;
        assert_ne!(request.semantic_input_fingerprint(), native_fingerprint);
        request.effective_system_prompt = schema().prompt_instruction();
        assert_eq!(
            DefaultTokenEstimator.estimate_frozen_request(&request),
            DefaultTokenEstimator.estimate_selected(
                &request.messages,
                &request.effective_system_prompt,
                &request.tools
            )
        );
        assert!(DefaultTokenEstimator.estimate_frozen_request(&request) > base_tokens);
        let frozen_fingerprint = request.semantic_input_fingerprint();
        request.response_schema.as_mut().unwrap().schema =
            ResponseSchema::new("classification".into(), definition(), 127).unwrap();
        assert_ne!(request.semantic_input_fingerprint(), frozen_fingerprint);
        assert_eq!(
            serde_json::from_value::<crate::model::ModelRequest>(
                serde_json::to_value(&request).unwrap()
            )
            .unwrap(),
            request
        );
    }
}
