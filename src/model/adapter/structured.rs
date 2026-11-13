//! Runtime ownership of structured output fields at the provider boundary.
//!
//! Ordinary text requests retain their opaque native parameters. Structured
//! requests own only the exact output-format field, preserving native siblings
//! such as Responses verbosity and Anthropic effort.

use serde_json::{Value, json};

use crate::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
use crate::model::structured::StructuredOutputStrategy;
use crate::model::types::{ModelProtocol, ModelRequest};

pub(crate) fn validate_request(request: &ModelRequest) -> Result<(), ModelError> {
    let Some(output) = &request.response_schema else {
        return Ok(());
    };
    if !output.matches_capability(request.invocation.capabilities.json_schema_output) {
        return Err(error(
            ModelErrorKind::Unsupported,
            "the prepared structured output strategy does not match the frozen native JSON Schema capability",
        ));
    }
    if let StructuredOutputStrategy::PromptFallback { operation_id } = &output.strategy
        && (operation_id.as_str().is_empty()
            || !request.has_prompt_fallback_authorization(operation_id)
            || !request
                .effective_system_prompt
                .ends_with(&output.schema.prompt_instruction()))
    {
        return Err(error(
            ModelErrorKind::InvalidRequest,
            "structured output fallback lacks exact operation authorization or its frozen prompt contract",
        ));
    }
    let params = request.request_params();
    let (parent, field) = match request.protocol() {
        ModelProtocol::OpenAiChatCompletions => (None, "response_format"),
        ModelProtocol::OpenAiResponses => (Some("text"), "format"),
        ModelProtocol::AnthropicMessages => (Some("output_config"), "format"),
    };
    let (object, path) = if let Some(parent) = parent {
        let Some(value) = params.get(parent) else {
            return Ok(());
        };
        let object = value.as_object().ok_or_else(|| {
            error(
                ModelErrorKind::InvalidRequest,
                &format!(
                    "effective request parameters {parent} must be an object for structured output"
                ),
            )
        })?;
        (object, format!("{parent}.{field}"))
    } else {
        (params, field.to_owned())
    };
    if object.contains_key(field) {
        return Err(error(
            ModelErrorKind::InvalidRequest,
            &format!(
                "effective request parameters declare runtime-owned structured output field {path}"
            ),
        ));
    }
    Ok(())
}

/// Applies the schema to the final request after the ordinary shallow overlay.
/// The collision check prevents overwrites; only the owned format field is added.
pub(crate) fn finalize_request(
    mut value: Value,
    request: &ModelRequest,
) -> Result<Value, ModelError> {
    validate_request(request)?;
    let Some(output) = request
        .response_schema
        .as_ref()
        .filter(|value| value.is_native())
    else {
        return Ok(value);
    };
    match request.protocol() {
        ModelProtocol::OpenAiChatCompletions => {
            value["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": output.name(),
                    "strict": true,
                    "schema": output.schema(),
                },
            });
        }
        ModelProtocol::OpenAiResponses => {
            if value.get("text").is_none() {
                value["text"] = json!({});
            }
            value["text"]["format"] = json!({
                "type": "json_schema",
                "name": output.name(),
                "strict": true,
                "schema": output.schema(),
            });
        }
        ModelProtocol::AnthropicMessages => {
            if value.get("output_config").is_none() {
                value["output_config"] = json!({});
            }
            // Anthropic's format contract has neither a name nor a strict flag.
            value["output_config"]["format"] = json!({
                "type": "json_schema",
                "schema": output.schema(),
            });
        }
    }
    Ok(value)
}

/// Recognizes complete provider diagnostics about an output feature, never a
/// field name embedded in a schema-validation explanation. The caller owns the
/// HTTP/type/native-request checks and decides whether optional metadata agrees.
pub(crate) fn explicit_format_rejection_message(message: &str, field: &str) -> bool {
    let message = message.trim().trim_end_matches('.');
    for quoted_field in [
        field.to_owned(),
        format!("'{field}'"),
        format!("\"{field}\""),
    ] {
        if [
            format!("Unsupported parameter: {quoted_field}"),
            format!("Unsupported parameter: {quoted_field} is not supported with this model"),
            format!("{quoted_field} is not supported"),
            format!("{quoted_field} is not supported by this model"),
        ]
        .iter()
        .any(|expected| message == expected)
        {
            return true;
        }
    }
    let type_field = format!("{field}.type");
    for quoted_field in [
        type_field.clone(),
        format!("'{type_field}'"),
        format!("\"{type_field}\""),
    ] {
        if message
            == format!(
                "Unsupported value: {quoted_field} does not support 'json_schema' with this model"
            )
        {
            return true;
        }
        let Some(values) = message
            .strip_prefix(&quoted_field)
            .and_then(|suffix| suffix.trim_start().strip_prefix(':'))
            .and_then(|suffix| {
                suffix
                    .trim_start()
                    .strip_prefix("value is not one of the allowed values ")
            })
            .and_then(|values| values.strip_prefix('['))
            .and_then(|values| values.strip_suffix(']'))
        else {
            continue;
        };
        // The documented compatible-provider diagnostic advertises text / JSON
        // object output instead of JSON Schema. Other enum failures, especially
        // those inside the schema or including json_schema, are not evidence.
        return !values.is_empty()
            && values.split(',').all(|value| {
                matches!(
                    value.trim(),
                    "'text'" | "'json_object'" | "\"text\"" | "\"json_object\""
                )
            });
    }
    false
}

fn error(kind: ModelErrorKind, message: &str) -> ModelError {
    ModelError {
        kind,
        message: message.to_owned(),
        retry_disposition: ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    }
}
