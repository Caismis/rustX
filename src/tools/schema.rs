//! JSON Schema contract of the tool plane.
//!
//! The canonical tool input schema is tool-owned and is the single
//! validation authority for business arguments: the runtime validates the
//! schema at registration, compiles it (clone/decorate only) into the
//! model-facing definition, and validates every invocation's stripped
//! business arguments against the original schema before dispatch. No
//! provider-side validation is ever relied on.
//!
//! Two distinct namespaces carry runtime-owned invocation metadata:
//!
//! - `__rustx_` is the reserved top-level property namespace. No canonical
//!   schema may claim one of those names and no invocation may carry one;
//!   they are rejected deterministically rather than forwarded.
//! - [`EXECUTION_MODE_FIELD`] (`execution_mode`) is the model-facing
//!   execution-ownership selector, and it is reserved **only** while the
//!   effective execution policy is [`ToolExecutionPolicy::ModelSelectable`].
//!   Under a fixed policy no synthetic field is injected, so a business
//!   property of that name stays legal.
//!
//! Because the reservation depends on the effective policy, the collision
//! check lives in [`validate_execution_metadata_contract`] — the bounded
//! layer that owns both the effective policy and the compiled model-facing
//! schema — and never in the policy-unaware canonical schema validation.

use jsonschema::Validator;

use crate::tools::types::{
    ModelToolDefinition, ToolDefinition, ToolExecutionPolicy, ToolInvocationMode,
};

/// The runtime-reserved top-level property namespace of tool schemas.
pub const RUNTIME_PROPERTY_PREFIX: &str = "__rustx_";

/// The model-facing execution-ownership selector of `ModelSelectable`
/// tools.
pub const EXECUTION_MODE_FIELD: &str = "execution_mode";

/// The accepted values of [`EXECUTION_MODE_FIELD`].
pub const EXECUTION_MODE_VALUES: [&str; 2] = ["foreground", "background"];

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
    /// A `ModelSelectable` tool's canonical business schema already defines
    /// a top-level `execution_mode` property, which rustX reserves for
    /// invocation ownership under that policy.
    ExecutionModeCollision,
    /// A `ModelSelectable` invocation omitted the required `execution_mode`
    /// field.
    MissingExecutionMode,
    /// The `execution_mode` value is not `foreground` or `background`.
    InvalidExecutionMode(String),
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
            Self::ExecutionModeCollision => write!(
                f,
                "the canonical tool schema defines a top-level {EXECUTION_MODE_FIELD:?} property, \
                 which rustX reserves for ModelSelectable invocation ownership; either rename the \
                 tool's business field or choose a non-ModelSelectable execution policy"
            ),
            Self::MissingExecutionMode => write!(
                f,
                "this tool requires the top-level {EXECUTION_MODE_FIELD:?} field on every call; \
                 retry the call with \"{EXECUTION_MODE_FIELD}\": \"foreground\" when you need its \
                 terminal result before continuing, or \"{EXECUTION_MODE_FIELD}\": \"background\" \
                 when the work should keep running independently"
            ),
            Self::InvalidExecutionMode(value) => write!(
                f,
                "the {EXECUTION_MODE_FIELD:?} field must be exactly \"foreground\" or \
                 \"background\", got {value}; retry the call with \
                 \"{EXECUTION_MODE_FIELD}\": \"foreground\" or \
                 \"{EXECUTION_MODE_FIELD}\": \"background\""
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
/// top-level property. This check is policy-unaware; the policy-dependent
/// `execution_mode` reservation is enforced by
/// [`validate_execution_metadata_contract`].
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

/// Validates the policy-dependent `execution_mode` reservation.
///
/// `execution_mode` becomes a reserved model-facing invocation field when
/// and only when the effective execution policy is `ModelSelectable`. A
/// canonical schema that already defines a top-level `execution_mode`
/// property is rejected deterministically under that policy rather than
/// silently renamed, shadowed, merged, or reinterpreted; the same schema
/// stays legal under `ForegroundOnly`/`BackgroundOnly`, where rustX injects
/// no synthetic field.
///
/// # Errors
///
/// Returns [`SchemaError::ExecutionModeCollision`] for a `ModelSelectable`
/// tool whose canonical schema claims the reserved field.
pub fn validate_execution_metadata_contract(
    policy: ToolExecutionPolicy,
    schema: &serde_json::Value,
) -> Result<(), SchemaError> {
    if policy != ToolExecutionPolicy::ModelSelectable {
        return Ok(());
    }
    let claims_field = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|properties| properties.contains_key(EXECUTION_MODE_FIELD));
    if claims_field {
        return Err(SchemaError::ExecutionModeCollision);
    }
    Ok(())
}

/// Compiles the canonical definition into the provider-neutral model-facing
/// definition used by one model request.
///
/// The stored canonical schema is never mutated: for a `ModelSelectable`
/// tool the compiled schema is a clone decorated with the required
/// `execution_mode` field and the compiled description carries the
/// runtime-owned reminder that the field is mandatory;
/// `ForegroundOnly`/`BackgroundOnly` definitions are compiled verbatim.
///
/// # Errors
///
/// Returns [`SchemaError::ExecutionModeCollision`] when a `ModelSelectable`
/// canonical schema already claims the reserved `execution_mode` property.
pub fn compile_model_definition(
    definition: &ToolDefinition,
) -> Result<ModelToolDefinition, SchemaError> {
    validate_execution_metadata_contract(definition.execution_policy, &definition.input_schema)?;
    let mut input_schema = definition.input_schema.clone();
    let mut description = definition.description.clone();
    if definition.execution_policy == ToolExecutionPolicy::ModelSelectable {
        decorate_execution_mode(&mut input_schema);
        description = with_execution_mode_reminder(&description);
    }
    Ok(ModelToolDefinition {
        id: definition.id.clone(),
        name: definition.name.clone(),
        description,
        input_schema,
    })
}

/// Appends the runtime-owned `execution_mode` reminder to a compiled
/// model-facing description without leaving stray separators when the
/// canonical description is empty.
fn with_execution_mode_reminder(description: &str) -> String {
    if description.trim().is_empty() {
        EXECUTION_MODE_DESCRIPTION_REMINDER.to_owned()
    } else {
        format!("{description}\n\n{EXECUTION_MODE_DESCRIPTION_REMINDER}")
    }
}

/// Decorates a root object schema with the required `execution_mode` field.
/// The `properties` map and `required` array are created or extended in
/// place; the canonical schema passed here is always a compiled clone.
///
/// # Panics
///
/// Panics only when a compiled schema is not a root object schema, which is
/// impossible after registration validation.
fn decorate_execution_mode(schema: &mut serde_json::Value) {
    let object = schema
        .as_object_mut()
        .expect("compiled schemas are root object schemas");
    object
        .entry("properties")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .expect("properties is an object")
        .insert(
            EXECUTION_MODE_FIELD.to_owned(),
            serde_json::json!({
                "type": "string",
                "enum": EXECUTION_MODE_VALUES,
                "description": EXECUTION_MODE_DESCRIPTION,
            }),
        );
    let required = object
        .entry("required")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .expect("required is an array");
    if !required.iter().any(|value| value == EXECUTION_MODE_FIELD) {
        required.push(serde_json::Value::String(EXECUTION_MODE_FIELD.to_owned()));
    }
}

/// Resolves the effective invocation mode and strips the runtime-owned
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
/// Reserved `__rustx_*` arguments are rejected under every policy. For
/// `ForegroundOnly`/`BackgroundOnly` no synthetic field is expected and the
/// arguments pass through unchanged — including a business property named
/// `execution_mode`, which is not reserved under a fixed policy. For
/// `ModelSelectable` the `execution_mode` field is required, extracted, and
/// removed before the remaining business arguments are returned. The
/// stripped arguments are returned to be validated against the original
/// canonical schema by the caller.
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
    reject_reserved_invocation_properties(arguments)?;
    match policy {
        ToolExecutionPolicy::ForegroundOnly => {
            Ok((ToolInvocationMode::Foreground, arguments.clone()))
        }
        ToolExecutionPolicy::BackgroundOnly => {
            Ok((ToolInvocationMode::Background, arguments.clone()))
        }
        ToolExecutionPolicy::ModelSelectable => {
            let Some(object) = arguments.as_object() else {
                return Err(SchemaError::MissingExecutionMode);
            };
            let Some(selector) = object.get(EXECUTION_MODE_FIELD) else {
                return Err(SchemaError::MissingExecutionMode);
            };
            let mode = match selector.as_str() {
                Some("foreground") => ToolInvocationMode::Foreground,
                Some("background") => ToolInvocationMode::Background,
                _ => {
                    return Err(SchemaError::InvalidExecutionMode(selector.to_string()));
                }
            };
            let mut stripped = arguments.clone();
            stripped
                .as_object_mut()
                .expect("ModelSelectable arguments are objects")
                .remove(EXECUTION_MODE_FIELD);
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
fn reject_reserved_invocation_properties(arguments: &serde_json::Value) -> Result<(), SchemaError> {
    let Some(object) = arguments.as_object() else {
        return Ok(());
    };
    for name in object.keys() {
        if is_reserved_property(name) {
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
        EXECUTION_MODE_DESCRIPTION_REMINDER, EXECUTION_MODE_FIELD, compile_model_definition,
        resolve_invocation_metadata, validate_business_arguments, validate_canonical_schema,
        validate_execution_metadata_contract,
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
    fn model_selectable_compiled_schema_gains_required_execution_mode() {
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ModelSelectable,
            json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        ))
        .expect("compile");
        let schema = &compiled.input_schema;
        let field = &schema["properties"][EXECUTION_MODE_FIELD];
        assert_eq!(field["type"], json!("string"));
        assert_eq!(field["enum"], json!(["foreground", "background"]));
        let description = field["description"].as_str().expect("field description");
        assert!(description.contains("foreground"));
        assert!(description.contains("background"));
        assert!(
            description.contains("terminal result"),
            "the description explains the semantic decision: {description}"
        );
        assert!(
            schema["required"]
                .as_array()
                .expect("required")
                .contains(&json!(EXECUTION_MODE_FIELD))
        );
    }

    #[test]
    fn model_selectable_compiled_description_reminds_the_model() {
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ModelSelectable,
            json!({"type": "object"}),
        ))
        .expect("compile");
        assert_eq!(compiled.description, EXECUTION_MODE_DESCRIPTION_REMINDER);

        let mut described = definition(
            ToolExecutionPolicy::ModelSelectable,
            json!({"type": "object"}),
        );
        described.description = "Runs a shell command.".to_owned();
        let compiled = compile_model_definition(&described).expect("compile");
        assert_eq!(
            compiled.description,
            format!("Runs a shell command.\n\n{EXECUTION_MODE_DESCRIPTION_REMINDER}")
        );
        assert_eq!(
            described.description, "Runs a shell command.",
            "the canonical description is never mutated"
        );
    }

    #[test]
    fn fixed_policy_descriptions_are_compiled_verbatim() {
        for policy in [
            ToolExecutionPolicy::ForegroundOnly,
            ToolExecutionPolicy::BackgroundOnly,
        ] {
            let mut fixed = definition(policy, json!({"type": "object"}));
            fixed.description = "Reads a file.".to_owned();
            let compiled = compile_model_definition(&fixed).expect("compile");
            assert_eq!(compiled.description, "Reads a file.");
        }
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
            compiled.input_schema["properties"].get(EXECUTION_MODE_FIELD),
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
                    .get(EXECUTION_MODE_FIELD)
                    .is_none(),
                "no synthetic field for non-model-selectable tools"
            );
        }
    }

    #[test]
    fn execution_mode_is_reserved_only_under_model_selectable() {
        let colliding = json!({
            "type": "object",
            "properties": {"execution_mode": {"type": "string"}},
        });
        assert!(
            validate_canonical_schema(&colliding).is_ok(),
            "the canonical schema validator stays policy-unaware"
        );
        assert!(matches!(
            validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, &colliding),
            Err(super::SchemaError::ExecutionModeCollision)
        ));
        assert!(matches!(
            compile_model_definition(&definition(
                ToolExecutionPolicy::ModelSelectable,
                colliding.clone()
            )),
            Err(super::SchemaError::ExecutionModeCollision)
        ));
        for policy in [
            ToolExecutionPolicy::ForegroundOnly,
            ToolExecutionPolicy::BackgroundOnly,
        ] {
            validate_execution_metadata_contract(policy, &colliding).expect("fixed policy");
            let compiled = compile_model_definition(&definition(policy, colliding.clone()))
                .expect("fixed policy compiles verbatim");
            assert_eq!(compiled.input_schema, colliding);
        }
    }

    #[test]
    fn the_collision_error_is_actionable() {
        let message = super::SchemaError::ExecutionModeCollision.to_string();
        assert!(message.contains("execution_mode"));
        assert!(message.contains("rename"));
        assert!(message.contains("ModelSelectable"));
    }

    #[test]
    fn valid_foreground_selection_is_extracted_and_stripped() {
        let (mode, stripped) = resolve_invocation_metadata(
            ToolExecutionPolicy::ModelSelectable,
            &json!({"execution_mode": "foreground", "path": "a.txt"}),
        )
        .expect("resolve");
        assert_eq!(mode, ToolInvocationMode::Foreground);
        assert_eq!(stripped, json!({"path": "a.txt"}));
    }

    #[test]
    fn valid_background_selection_is_extracted_and_stripped() {
        let (mode, stripped) = resolve_invocation_metadata(
            ToolExecutionPolicy::ModelSelectable,
            &json!({"execution_mode": "background", "command": "sleep 1"}),
        )
        .expect("resolve");
        assert_eq!(mode, ToolInvocationMode::Background);
        assert_eq!(stripped, json!({"command": "sleep 1"}));
    }

    #[test]
    fn missing_or_invalid_mode_fails_without_forwarding() {
        let missing = resolve_invocation_metadata(
            ToolExecutionPolicy::ModelSelectable,
            &json!({"path": "a.txt"}),
        )
        .expect_err("missing");
        assert!(matches!(missing, super::SchemaError::MissingExecutionMode));
        let message = missing.to_string();
        assert!(message.contains("\"execution_mode\": \"foreground\""));
        assert!(message.contains("\"execution_mode\": \"background\""));

        let invalid = resolve_invocation_metadata(
            ToolExecutionPolicy::ModelSelectable,
            &json!({"execution_mode": "sideways", "path": "a.txt"}),
        )
        .expect_err("invalid");
        assert_eq!(
            invalid,
            super::SchemaError::InvalidExecutionMode("\"sideways\"".to_owned())
        );
        assert!(
            invalid
                .to_string()
                .contains("\"execution_mode\": \"background\"")
        );

        assert!(matches!(
            resolve_invocation_metadata(
                ToolExecutionPolicy::ModelSelectable,
                &json!({"execution_mode": 1}),
            ),
            Err(super::SchemaError::InvalidExecutionMode(_))
        ));
        assert!(matches!(
            resolve_invocation_metadata(ToolExecutionPolicy::ModelSelectable, &json!(null)),
            Err(super::SchemaError::MissingExecutionMode)
        ));
    }

    #[test]
    fn the_retired_reserved_selector_no_longer_selects_a_mode() {
        assert!(matches!(
            resolve_invocation_metadata(
                ToolExecutionPolicy::ModelSelectable,
                &json!({"__rustx_execution": "background", "path": "a.txt"}),
            ),
            Err(super::SchemaError::ReservedInvocationProperty(_))
        ));
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ModelSelectable,
            json!({"type": "object"}),
        ))
        .expect("compile");
        assert!(
            !compiled
                .input_schema
                .to_string()
                .contains("__rustx_execution")
        );
        assert!(!compiled.description.contains("__rustx_execution"));
    }

    #[test]
    fn a_business_execution_mode_survives_a_fixed_policy_invocation() {
        for (policy, expected) in [
            (
                ToolExecutionPolicy::ForegroundOnly,
                ToolInvocationMode::Foreground,
            ),
            (
                ToolExecutionPolicy::BackgroundOnly,
                ToolInvocationMode::Background,
            ),
        ] {
            let arguments = json!({"execution_mode": "whatever the tool means"});
            let (mode, stripped) =
                resolve_invocation_metadata(policy, &arguments).expect("resolve");
            assert_eq!(mode, expected);
            assert_eq!(
                stripped, arguments,
                "no field is stripped under a fixed policy"
            );
        }
    }

    #[test]
    fn unexpected_reserved_fields_are_rejected_for_every_policy() {
        for policy in [
            ToolExecutionPolicy::ForegroundOnly,
            ToolExecutionPolicy::BackgroundOnly,
            ToolExecutionPolicy::ModelSelectable,
        ] {
            assert!(matches!(
                resolve_invocation_metadata(policy, &json!({"__rustx_other": 1})),
                Err(super::SchemaError::ReservedInvocationProperty(_))
            ));
        }
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
