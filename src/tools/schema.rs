//! JSON Schema contract of the tool plane.
//!
//! The canonical tool input schema is tool-owned and is the single
//! validation authority for business arguments: the runtime validates the
//! schema at registration, compiles it (clone/decorate only) into the
//! model-facing definition, and validates every invocation's stripped
//! business arguments against the original schema before dispatch. No
//! provider-side validation is ever relied on.
//!
//! The top-level runtime-reserved property namespace is `__rustx_`. The
//! reserved model-selectable invocation field is `__rustx_execution` with
//! model-facing semantics `{"type": "string", "enum": ["foreground",
//! "background"]}`. Registration rejects any canonical schema that claims a
//! reserved property; invocation arguments containing unexpected reserved
//! fields are rejected deterministically rather than forwarded.

use jsonschema::Validator;

use crate::tools::types::{
    ModelToolDefinition, ToolDefinition, ToolExecutionPolicy, ToolInvocationMode,
};

/// The runtime-reserved top-level property namespace of tool schemas.
pub const RUNTIME_PROPERTY_PREFIX: &str = "__rustx_";

/// The reserved model-selectable invocation field.
pub const EXECUTION_FIELD: &str = "__rustx_execution";

/// The accepted values of the reserved execution field.
pub const EXECUTION_FIELD_VALUES: [&str; 2] = ["foreground", "background"];

/// A tool schema or invocation-metadata validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// The schema document is not a valid JSON Schema.
    InvalidSchema(String),
    /// The canonical schema is not a root object schema suitable for model
    /// tool arguments, so deterministic root-property decoration is
    /// impossible.
    NotRootObjectSchema(String),
    /// The canonical schema claims a reserved `__rustx_*` top-level
    /// property.
    ReservedProperty(String),
    /// The invocation arguments claim a reserved `__rustx_*` property.
    ReservedInvocationProperty(String),
    /// A `ModelSelectable` invocation omitted the required
    /// `__rustx_execution` field.
    MissingExecutionField,
    /// The `__rustx_execution` value is not `foreground` or `background`.
    InvalidExecutionValue(String),
    /// The business arguments violate the canonical schema.
    InvalidArguments(Vec<String>),
}

impl core::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidSchema(message) => write!(f, "invalid JSON Schema: {message}"),
            Self::NotRootObjectSchema(message) => write!(f, "{message}"),
            Self::ReservedProperty(property) => write!(
                f,
                "canonical tool schema claims the reserved runtime property {property:?}; \
                 the {RUNTIME_PROPERTY_PREFIX:?} namespace is runtime-owned"
            ),
            Self::ReservedInvocationProperty(property) => write!(
                f,
                "invocation arguments claim the reserved runtime property {property:?}; \
                 reserved invocation metadata is never forwarded to executors"
            ),
            Self::MissingExecutionField => write!(
                f,
                "a ModelSelectable invocation must carry the required reserved field \
                 {EXECUTION_FIELD:?}"
            ),
            Self::InvalidExecutionValue(value) => write!(
                f,
                "the reserved field {EXECUTION_FIELD:?} must be one of \
                 [\"foreground\", \"background\"], got {value:?}"
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

/// Whether a top-level property name is reserved by the runtime.
#[must_use]
pub fn is_reserved_property(name: &str) -> bool {
    name.starts_with(RUNTIME_PROPERTY_PREFIX)
}

/// Validates a canonical tool input schema at registration.
///
/// The schema must be a valid JSON Schema document that is a root object
/// schema (`"type": "object"`), so the model-facing compiler can decorate it
/// deterministically, and it must not claim any reserved `__rustx_*`
/// top-level property.
///
/// # Errors
///
/// Returns [`SchemaError::InvalidSchema`] for malformed schemas,
/// [`SchemaError::NotRootObjectSchema`] for schemas that cannot support
/// deterministic root-property decoration, and
/// [`SchemaError::ReservedProperty`] for reserved property collisions.
pub fn validate_canonical_schema(schema: &serde_json::Value) -> Result<(), SchemaError> {
    let object = schema.as_object().ok_or_else(|| {
        SchemaError::NotRootObjectSchema(
            "the canonical tool input schema must be a root object schema".to_owned(),
        )
    })?;
    Validator::new(schema).map_err(|error| SchemaError::InvalidSchema(error.to_string()))?;
    if object.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return Err(SchemaError::NotRootObjectSchema(
            "the canonical tool input schema must declare \"type\": \"object\"".to_owned(),
        ));
    }
    if let Some(properties) = object
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        for name in properties.keys() {
            if is_reserved_property(name) {
                return Err(SchemaError::ReservedProperty(name.clone()));
            }
        }
    }
    Ok(())
}

/// Compiles the canonical definition into the provider-neutral model-facing
/// definition used by one model request.
///
/// The stored canonical schema is never mutated: for a `ModelSelectable`
/// tool the compiled schema is a clone decorated with the required reserved
/// `__rustx_execution` field; `ForegroundOnly`/`BackgroundOnly` schemas are
/// compiled verbatim.
///
/// # Errors
///
/// Returns [`SchemaError::ReservedProperty`] when the canonical schema
/// already claims a reserved property.
pub fn compile_model_definition(
    definition: &ToolDefinition,
) -> Result<ModelToolDefinition, SchemaError> {
    let mut input_schema = definition.input_schema.clone();
    if definition.execution_policy == ToolExecutionPolicy::ModelSelectable {
        decorate_execution_field(&mut input_schema);
    }
    Ok(ModelToolDefinition {
        id: definition.id.clone(),
        name: definition.name.clone(),
        description: definition.description.clone(),
        input_schema,
    })
}

/// Decorates a root object schema with the required reserved execution
/// field. The `properties` map and `required` array are created or extended
/// in place; the canonical schema passed here is always a compiled clone.
///
/// # Panics
///
/// Panics only when a compiled schema is not a root object schema, which is
/// impossible after registration validation.
fn decorate_execution_field(schema: &mut serde_json::Value) {
    let object = schema
        .as_object_mut()
        .expect("compiled schemas are root object schemas");
    object
        .entry("properties")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .expect("properties is an object")
        .insert(
            EXECUTION_FIELD.to_owned(),
            serde_json::json!({
                "type": "string",
                "enum": ["foreground", "background"]
            }),
        );
    let required = object
        .entry("required")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .expect("required is an array");
    if !required.iter().any(|value| value == EXECUTION_FIELD) {
        required.push(serde_json::Value::String(EXECUTION_FIELD.to_owned()));
    }
}

/// Resolves the effective invocation mode and strips the reserved runtime
/// invocation metadata from the call arguments.
///
/// Invocation order:
///
///
/// ```text
/// resolve tool
/// → extract/resolve rustX invocation metadata
/// → strip rustX metadata
/// → validate business arguments against the canonical schema
/// → dispatch executor
/// ```
///
/// For `ForegroundOnly`/`BackgroundOnly` no synthetic field is expected and
/// any `__rustx_*` argument is rejected; for `ModelSelectable` the reserved
/// `__rustx_execution` field is required, extracted, and removed before the
/// remaining business arguments are returned. The stripped arguments are
/// returned to be validated against the original canonical schema by the
/// caller.
///
/// # Errors
///
/// Returns the specific [`SchemaError`] of the first violation.
///
/// # Panics
///
/// Panics only when a `ModelSelectable` argument object fails to remain an
/// object after extraction, which is impossible by construction.
pub fn resolve_invocation_metadata(
    policy: ToolExecutionPolicy,
    arguments: &serde_json::Value,
) -> Result<(ToolInvocationMode, serde_json::Value), SchemaError> {
    match policy {
        ToolExecutionPolicy::ForegroundOnly => {
            reject_unexpected_reserved(arguments)?;
            Ok((ToolInvocationMode::Foreground, arguments.clone()))
        }
        ToolExecutionPolicy::BackgroundOnly => {
            reject_unexpected_reserved(arguments)?;
            Ok((ToolInvocationMode::Background, arguments.clone()))
        }
        ToolExecutionPolicy::ModelSelectable => {
            let Some(object) = arguments.as_object() else {
                return Err(SchemaError::MissingExecutionField);
            };
            let Some(value) = object
                .get(EXECUTION_FIELD)
                .and_then(serde_json::Value::as_str)
            else {
                return Err(SchemaError::MissingExecutionField);
            };
            let mode = match value {
                "foreground" => ToolInvocationMode::Foreground,
                "background" => ToolInvocationMode::Background,
                other => return Err(SchemaError::InvalidExecutionValue(other.to_owned())),
            };
            // The expected `__rustx_execution` field itself is extracted
            // first; only *other* reserved fields are rejected.
            reject_unexpected_reserved_except(object, EXECUTION_FIELD)?;
            let mut stripped = arguments.clone();
            stripped
                .as_object_mut()
                .expect("ModelSelectable arguments are objects")
                .remove(EXECUTION_FIELD);
            Ok((mode, stripped))
        }
    }
}

/// Rejects any reserved `__rustx_*` property in the invocation arguments.
///
/// # Errors
///
/// Returns [`SchemaError::ReservedInvocationProperty`] for the first
/// reserved property found.
fn reject_unexpected_reserved(arguments: &serde_json::Value) -> Result<(), SchemaError> {
    let Some(object) = arguments.as_object() else {
        return Ok(());
    };
    reject_unexpected_reserved_except(object, "")
}

/// Rejects reserved `__rustx_*` properties except the one being extracted.
///
/// # Errors
///
/// Returns [`SchemaError::ReservedInvocationProperty`] for the first
/// unexpected reserved property found.
fn reject_unexpected_reserved_except(
    object: &serde_json::Map<String, serde_json::Value>,
    except: &str,
) -> Result<(), SchemaError> {
    for name in object.keys() {
        if is_reserved_property(name) && name != except {
            return Err(SchemaError::ReservedInvocationProperty(name.clone()));
        }
    }
    Ok(())
}

/// Validates the stripped business arguments against the original canonical
/// schema.
///
/// # Errors
///
/// Returns [`SchemaError::InvalidArguments`] with every schema violation
/// when the arguments do not conform.
///
/// # Panics
///
/// Panics only when the canonical schema was not validated at registration,
/// which is impossible for registry-owned definitions.
pub fn validate_business_arguments(
    schema: &serde_json::Value,
    arguments: &serde_json::Value,
) -> Result<(), SchemaError> {
    let validator =
        Validator::new(schema).expect("canonical schemas are validated at registration");
    let mut errors = Vec::new();
    for error in validator.iter_errors(arguments) {
        errors.push(error.to_string());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(SchemaError::InvalidArguments(errors))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EXECUTION_FIELD, compile_model_definition, resolve_invocation_metadata,
        validate_business_arguments, validate_canonical_schema,
    };
    use crate::runtime::identity::ToolId;
    use crate::tools::types::{
        ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolInvocationMode, ToolOrigin,
        ToolReplayPolicy,
    };
    use serde_json::json;

    fn definition(policy: ToolExecutionPolicy, schema: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            id: ToolId::new("tool-x"),
            name: "x".to_owned(),
            description: String::new(),
            input_schema: schema,
            execution_policy: policy,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        }
    }

    #[test]
    fn registration_rejects_invalid_schemas() {
        assert!(matches!(
            validate_canonical_schema(&json!(42)),
            Err(super::SchemaError::NotRootObjectSchema(_))
        ));
        assert!(matches!(
            validate_canonical_schema(&json!({"type": "array"})),
            Err(super::SchemaError::NotRootObjectSchema(_))
        ));
        assert!(matches!(
            validate_canonical_schema(
                &json!({"type": "object", "properties": {"__rustx_secret": {}}})
            ),
            Err(super::SchemaError::ReservedProperty(_))
        ));
        assert!(validate_canonical_schema(&json!({"type": "object"})).is_ok());
    }

    #[test]
    fn model_selectable_compiled_schema_gains_required_execution_field() {
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ModelSelectable,
            json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        ))
        .expect("compile");
        let schema = &compiled.input_schema;
        assert_eq!(
            schema["properties"][EXECUTION_FIELD],
            json!({"type": "string", "enum": ["foreground", "background"]})
        );
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(EXECUTION_FIELD))
        );
    }

    #[test]
    fn canonical_schema_is_unchanged_after_compilation() {
        let original = json!({"type": "object", "properties": {"path": {"type": "string"}}});
        let selectable = definition(ToolExecutionPolicy::ModelSelectable, original.clone());
        let compiled = compile_model_definition(&selectable).expect("compile");
        assert_ne!(
            compiled.input_schema, original,
            "the compiled model-selectable schema is decorated"
        );
        assert_ne!(
            compiled.input_schema["properties"].get(EXECUTION_FIELD),
            None
        );
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ForegroundOnly,
            original.clone(),
        ))
        .expect("compile");
        assert_eq!(
            compiled.input_schema, original,
            "the foreground schema is compiled verbatim"
        );
        assert_eq!(
            selectable.input_schema, original,
            "the canonical schema is never mutated"
        );
    }

    #[test]
    fn foreground_and_background_schemas_do_not_gain_the_field() {
        for policy in [
            ToolExecutionPolicy::ForegroundOnly,
            ToolExecutionPolicy::BackgroundOnly,
        ] {
            let compiled = compile_model_definition(&definition(policy, json!({"type": "object"})))
                .expect("compile");
            assert!(
                compiled.input_schema["properties"]
                    .get(EXECUTION_FIELD)
                    .is_none(),
                "no synthetic field for non-model-selectable tools"
            );
        }
    }

    #[test]
    fn valid_foreground_selection_is_extracted_and_stripped() {
        let (mode, stripped) = resolve_invocation_metadata(
            ToolExecutionPolicy::ModelSelectable,
            &json!({"__rustx_execution": "foreground", "path": "a.txt"}),
        )
        .expect("resolve");
        assert_eq!(mode, ToolInvocationMode::Foreground);
        assert_eq!(stripped, json!({"path": "a.txt"}));
    }

    #[test]
    fn valid_background_selection_is_extracted_and_stripped() {
        let (mode, stripped) = resolve_invocation_metadata(
            ToolExecutionPolicy::ModelSelectable,
            &json!({"__rustx_execution": "background", "command": "sleep 1"}),
        )
        .expect("resolve");
        assert_eq!(mode, ToolInvocationMode::Background);
        assert_eq!(stripped, json!({"command": "sleep 1"}));
    }

    #[test]
    fn missing_or_invalid_mode_fails_without_forwarding() {
        assert!(matches!(
            resolve_invocation_metadata(
                ToolExecutionPolicy::ModelSelectable,
                &json!({"path": "a.txt"}),
            ),
            Err(super::SchemaError::MissingExecutionField)
        ));
        assert!(matches!(
            resolve_invocation_metadata(
                ToolExecutionPolicy::ModelSelectable,
                &json!({"__rustx_execution": "sideways", "path": "a.txt"}),
            ),
            Err(super::SchemaError::InvalidExecutionValue(_))
        ));
        assert!(matches!(
            resolve_invocation_metadata(ToolExecutionPolicy::ModelSelectable, &json!(null),),
            Err(super::SchemaError::MissingExecutionField)
        ));
    }

    #[test]
    fn unexpected_reserved_fields_are_rejected_for_fixed_policies() {
        assert!(matches!(
            resolve_invocation_metadata(
                ToolExecutionPolicy::ForegroundOnly,
                &json!({"__rustx_execution": "foreground", "path": "a.txt"}),
            ),
            Err(super::SchemaError::ReservedInvocationProperty(_))
        ));
        assert!(matches!(
            resolve_invocation_metadata(
                ToolExecutionPolicy::BackgroundOnly,
                &json!({"__rustx_other": 1}),
            ),
            Err(super::SchemaError::ReservedInvocationProperty(_))
        ));
    }

    #[test]
    fn business_validation_uses_the_original_schema() {
        let schema = json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
            "additionalProperties": false,
        });
        assert!(validate_business_arguments(&schema, &json!({"path": "a"})).is_ok());
        let error = validate_business_arguments(&schema, &json!({"path": 1})).expect_err("fail");
        assert!(matches!(error, super::SchemaError::InvalidArguments(_)));
    }
}
