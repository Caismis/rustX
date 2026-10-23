//! Runtime ownership of structured output fields at the provider boundary.
//!
//! Ordinary text requests retain their opaque native parameters. Structured
//! requests own only the exact output-format field, preserving native siblings
//! such as Responses verbosity and Anthropic effort.

use serde_json::{Value, json};

use crate::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
use crate::model::types::{ModelProtocol, ModelRequest};

pub(crate) fn validate_request(request: &ModelRequest) -> Result<(), ModelError> {
    let Some(output) = &request.response_schema else {
        return Ok(());
    };
    if output.is_native() != request.invocation.capabilities.json_schema_output {
        return Err(error(
            ModelErrorKind::Unsupported,
            "the prepared structured output strategy does not match the frozen native JSON Schema capability",
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
