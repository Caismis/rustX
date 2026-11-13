//! Provider-neutral tool calls use a strict runtime envelope. The business
//! schema lives in its own resource graph and is never decorated with runtime
//! properties. Executors receive only `arguments`; authorization receives the
//! separately decoded metadata.

mod envelope;

use crate::tools::types::{
    ModelToolDefinition, ToolDefinition, ToolExecutionPolicy, ToolInvocationMode,
};
use jsonschema::Validator;
use serde_json::{Value, json};

/// The runtime-reserved top-level property namespace of tool schemas.
pub const RUNTIME_PROPERTY_PREFIX: &str = "__rustx_";

/// The model-facing execution-ownership selector of `ModelSelectable`
/// tools.
pub const EXECUTION_MODE_FIELD: &str = "execution_mode";

/// The accepted values of [`EXECUTION_MODE_FIELD`].
pub const EXECUTION_MODE_VALUES: [&str; 2] = ["foreground", "background"];

/// Optional runtime-owned elevation reason on every model-facing tool schema
/// (Issue #544). Stripped before business-argument validation and never
/// forwarded to executors.
pub const ELEVATION_REASON_FIELD: &str = "elevation_reason";

/// Maximum UTF-8 byte length of a nonblank [`ELEVATION_REASON_FIELD`] value.
pub const ELEVATION_REASON_MAX_UTF8_BYTES: usize = 512;

/// Model-facing description of [`ELEVATION_REASON_FIELD`].
pub const ELEVATION_REASON_DESCRIPTION: &str = "Optional reason requesting human escalation for \
     this exact tool call under Plan or Automatic permission modes. Omit or leave null when no \
     escalation is requested. At most 512 UTF-8 bytes when present. This is a reason only — never \
     approval evidence — and it never reaches the tool's business executor.";

/// The model-facing description of the injected [`EXECUTION_MODE_FIELD`]
/// property. It states the semantic decision instead of listing the enum,
/// and it deliberately rejects a duration-based heuristic: the distinction
/// is whether subsequent agent work depends on the terminal result, not
/// whether the work is short or long.
pub const EXECUTION_MODE_DESCRIPTION: &str = "Required execution ownership for this tool call. \
     Use \"foreground\" when the agent needs this invocation to complete and return its terminal \
     result before continuing. Use \"background\" when the work should keep running independently \
     while the agent proceeds with other work. Decide by whether your next step depends on this \
     call's terminal result, not by how long the work is expected to take.";

/// The runtime-owned reminder appended to the compiled model-facing
/// description of a `ModelSelectable` tool. It belongs to model-definition
/// compilation, not to any individual tool.
pub const EXECUTION_MODE_DESCRIPTION_REMINDER: &str = "Execution ownership: every call to this tool must include the top-level \"execution_mode\" \
     field, set to \"foreground\" when the agent needs this call's terminal result before it can \
     continue, or \"background\" when the work should keep running independently while the agent \
     proceeds.";

/// Business arguments within the model-facing invocation envelope.
pub const ARGUMENTS_FIELD: &str = "arguments";

/// A schema compilation or invocation contract failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// Malformed JSON Schema or an unresolved resource.
    InvalidSchema(String),
    /// Business tool inputs must be object schemas.
    NotRootObjectSchema(String),
    /// A reserved outer runtime property was supplied.
    ReservedInvocationProperty(String),
    /// The required execution ownership was omitted.
    MissingExecutionMode,
    /// Execution ownership is not a supported value.
    InvalidExecutionMode(String),
    /// Elevation reason has invalid type or exceeds its byte budget.
    InvalidElevationReason(String),
    /// The invocation or business arguments violate their contract.
    InvalidArguments(Vec<String>),
}

impl core::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidSchema(message) => write!(f, "invalid JSON Schema: {message}"),
            Self::NotRootObjectSchema(message) => write!(f, "{message}"),
            Self::ReservedInvocationProperty(_) => write!(
                f,
                "$ contains a reserved __rustx_* runtime envelope property. Remove all reserved envelope fields."
            ),
            Self::MissingExecutionMode => write!(
                f,
                "this tool requires top-level \"execution_mode\": \"foreground\" or \"execution_mode\": \"background\""
            ),
            Self::InvalidExecutionMode(value) => write!(
                f,
                "execution_mode must be foreground or background, got {value}"
            ),
            Self::InvalidElevationReason(value) => write!(
                f,
                "elevation_reason must be a string of at most {ELEVATION_REASON_MAX_UTF8_BYTES} UTF-8 bytes or null, got {value}"
            ),
            Self::InvalidArguments(errors) => write!(
                f,
                "tool arguments violate the canonical schema: {}",
                errors.join("; ")
            ),
        }
    }
}
impl std::error::Error for SchemaError {}

/// Whether a property belongs to the reserved outer runtime namespace.
#[must_use]
pub fn is_reserved_property(name: &str) -> bool {
    name.starts_with(RUNTIME_PROPERTY_PREFIX)
}

fn validate_root_shape(schema: &Value) -> Result<(), SchemaError> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(SchemaError::NotRootObjectSchema(
            "the canonical tool input schema must declare \"type\": \"object\"".into(),
        ));
    }
    Ok(())
}

/// Validates the authored business schema without introducing runtime names.
///
/// # Errors
/// Returns an error for an invalid schema or a non-object business root.
pub fn validate_canonical_schema(schema: &Value) -> Result<(), SchemaError> {
    validate_root_shape(schema)?;
    envelope::validate_declaration(schema)
}

/// Compiles a definition and its frozen canonical validator in one resource
/// capture. Registry observation and calls never retrieve a schema again.
pub(crate) fn compile_tool_contract(
    definition: &ToolDefinition,
) -> Result<(ModelToolDefinition, Validator), SchemaError> {
    validate_root_shape(&definition.input_schema)?;
    let business = envelope::compile_business_resources(&definition.input_schema)?;
    let mut properties = json!({
        "arguments": {"$ref": business.root_uri},
        ELEVATION_REASON_FIELD: {"type": ["string", "null"], "maxLength": ELEVATION_REASON_MAX_UTF8_BYTES, "description": ELEVATION_REASON_DESCRIPTION}
    });
    let mut required = vec![json!(ARGUMENTS_FIELD)];
    let mut description = format!(
        "{}\n\nPass business tool inputs inside the required arguments object. Runtime permission metadata belongs outside arguments.",
        definition.description
    );
    if definition.execution_policy == ToolExecutionPolicy::ModelSelectable {
        properties[EXECUTION_MODE_FIELD] = json!({"type":"string", "enum":EXECUTION_MODE_VALUES, "description":EXECUTION_MODE_DESCRIPTION});
        required.push(json!(EXECUTION_MODE_FIELD));
        description.push_str("\n\n");
        description.push_str(EXECUTION_MODE_DESCRIPTION_REMINDER);
    }
    // The envelope has its own identity; it must not shadow an authored resource.
    let mut uri = "urn:rustx:invocation".to_owned();
    while business.resources.get(&uri).is_some() {
        uri.push('_');
    }
    let input_schema = json!({
        "$schema":"http://json-schema.org/draft-07/schema#", "$id":uri,
        "type":"object", "definitions":business.resources,
        "properties":properties, "required":required, "additionalProperties":false
    });
    Validator::new(&input_schema).map_err(|error| SchemaError::InvalidSchema(error.to_string()))?;
    Ok((
        ModelToolDefinition {
            id: definition.id.clone(),
            name: definition.name.clone(),
            description,
            input_schema,
        },
        business.validator,
    ))
}

/// Compiles the provider-neutral invocation envelope without changing the
/// authored business contract.
///
/// # Errors
/// Returns a schema compilation error before registration can commit.
pub fn compile_model_definition(
    definition: &ToolDefinition,
) -> Result<ModelToolDefinition, SchemaError> {
    compile_tool_contract(definition).map(|(model, _)| model)
}

/// Decodes the strict invocation envelope. Flat calls are not accepted.
///
/// # Errors
/// Returns the first metadata or envelope violation.
pub fn resolve_invocation_metadata(
    policy: ToolExecutionPolicy,
    call: &Value,
) -> Result<(ToolInvocationMode, Option<String>, Value), SchemaError> {
    let object = call.as_object().ok_or_else(|| {
        SchemaError::InvalidArguments(vec![
            "tool call must be an invocation envelope object".into(),
        ])
    })?;
    for name in object.keys() {
        if is_reserved_property(name) {
            return Err(SchemaError::ReservedInvocationProperty(name.clone()));
        }
        if name != ARGUMENTS_FIELD
            && name != ELEVATION_REASON_FIELD
            && !(name == EXECUTION_MODE_FIELD && policy == ToolExecutionPolicy::ModelSelectable)
        {
            return Err(SchemaError::InvalidArguments(vec![format!(
                "unknown invocation envelope field {name:?}; put business inputs inside arguments"
            )]));
        }
    }
    let arguments = object
        .get(ARGUMENTS_FIELD)
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            SchemaError::InvalidArguments(vec!["required arguments must be an object".into()])
        })?;
    let mode = match policy {
        ToolExecutionPolicy::ForegroundOnly => ToolInvocationMode::Foreground,
        ToolExecutionPolicy::BackgroundOnly => ToolInvocationMode::Background,
        ToolExecutionPolicy::ModelSelectable => match object.get(EXECUTION_MODE_FIELD) {
            Some(Value::String(value)) if value == "foreground" => ToolInvocationMode::Foreground,
            Some(Value::String(value)) if value == "background" => ToolInvocationMode::Background,
            Some(other) => return Err(SchemaError::InvalidExecutionMode(other.to_string())),
            None => return Err(SchemaError::MissingExecutionMode),
        },
    };
    let reason = match object.get(ELEVATION_REASON_FIELD) {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else if trimmed.len() > ELEVATION_REASON_MAX_UTF8_BYTES {
                return Err(SchemaError::InvalidElevationReason(format!(
                    "UTF-8 byte length {}",
                    trimmed.len()
                )));
            } else {
                Some(trimmed.to_owned())
            }
        }
        Some(other) => return Err(SchemaError::InvalidElevationReason(other.to_string())),
    };
    Ok((mode, reason, arguments.clone()))
}

/// Validates business arguments against an authored schema.
/// Registry calls use their captured validator directly.
///
/// # Errors
/// Returns invalid-schema or business-validation errors.
pub fn validate_business_arguments(schema: &Value, arguments: &Value) -> Result<(), SchemaError> {
    let validator =
        Validator::new(schema).map_err(|error| SchemaError::InvalidSchema(error.to_string()))?;
    validate_with(&validator, arguments)
}

pub(crate) fn validate_with(validator: &Validator, arguments: &Value) -> Result<(), SchemaError> {
    let errors: Vec<_> = validator
        .iter_errors(arguments)
        .map(|error| error.to_string())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(SchemaError::InvalidArguments(errors))
    }
}

#[cfg(test)]
mod tests;
