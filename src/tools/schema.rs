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
//!
//! # The canonical schema contract of a `ModelSelectable` tool
//!
//! `ModelSelectable` is the one policy under which rustX must *write into*
//! a tool's root schema, so it is the one policy that constrains the root's
//! shape. Under it a canonical schema must match the **decoratable root
//! profile**: the root object's instance semantics are owned entirely by
//! [`DECORATABLE_ROOT_INSTANCE_KEYWORDS`] (`type`, `properties`, `required`,
//! `additionalProperties`), alongside any purely descriptive keyword from
//! [`ROOT_ANNOTATION_KEYWORDS`]. Every other root keyword is refused.
//!
//! This is an allowlist on purpose. Injecting a required `execution_mode`
//! property changes what the root instance must look like, so *any* root
//! assertion rustX does not understand can silently contradict the
//! injection — `maxProperties` capping the object below the new required
//! count, a root `const`/`enum` pinning the whole object, a Draft-7
//! `dependencies` demanding the stripped selector, a composition branch that
//! never learned about it. Each produces the same fatal outcome: the tool
//! registers, the schema compiles, and no correct model call can ever exist.
//! Enumerating hazards could never be proven complete, so the profile
//! enumerates what is *safe* instead.
//!
//! The complementary hazard is the strip/validate asymmetry: the runtime
//! removes `execution_mode` *before* the canonical schema sees the
//! arguments, so a root that claims the name — in `properties`, in
//! `required`, or both — can never be satisfied either. That is checked
//! explicitly, because `properties` and `required` are inside the profile.
//!
//! One further rule reaches past the root, because decoration is an
//! *in-place* edit of the root schema and is only sound while the root
//! schema is the sole description of the root instance. A reference can
//! re-enter the decorated root from any depth — `{"child": {"$ref": "#"}}` —
//! so [`REFERENCE_APPLICATOR_KEYWORDS`] are refused throughout a
//! `ModelSelectable` canonical schema. Apart from references, nested
//! subschemas stay unrestricted: `properties.foo` may hold composition,
//! cardinality assertions, and an `execution_mode` of its own.
//!
//! Together the three rules give compilation a provable contract, not merely
//! a safe-looking root syntax:
//!
//! ```text
//! canonical(B) ⇔ compiled(B + top-level execution_mode)
//! ```
//!
//! Nothing here applies to `ForegroundOnly`/`BackgroundOnly` tools — they
//! receive no injected field, so an ordinary root schema (including the
//! native `ask_user` questionnaire contract or an MCP server's own
//! reference-heavy schema) stays valid, `execution_mode` included.

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

/// Optional runtime-owned elevation reason on every model-facing tool schema
/// (Issue #544). Stripped before business-argument validation and never
/// forwarded to executors.
pub const ELEVATION_REASON_FIELD: &str = "elevation_reason";

/// Maximum UTF-8 byte length of a nonblank [`ELEVATION_REASON_FIELD`] value.
pub const ELEVATION_REASON_MAX_UTF8_BYTES: usize = 512;

/// `$defs` name holding the undecorated canonical business schema while the
/// model-facing copy admits optional [`ELEVATION_REASON_FIELD`].
const RUSTX_BUSINESS_DEF: &str = "rustx_business";

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

/// The root keywords that may shape a `ModelSelectable` root instance.
///
/// These four are the entire instance semantics rustX supports for a
/// decoratable root, and decoration composes with each of them soundly:
/// `execution_mode` is inserted into this same schema object's `properties`
/// and appended to this same `required` array, so a closed root object
/// (`"additionalProperties": false`) stays satisfiable. `type` is already
/// pinned to `"object"` by [`validate_canonical_schema`].
///
/// Every ordinary native tool is exactly this shape.
pub const DECORATABLE_ROOT_INSTANCE_KEYWORDS: &[&str] =
    &["type", "properties", "required", "additionalProperties"];

/// The root keywords that carry no root-instance assertion at all.
///
/// Identifiers, vocabulary declarations, annotations, and definition
/// containers describe or name a schema without constraining the instance,
/// so decoration cannot contradict them. `$defs`/`definitions` hold
/// subschemas that only apply where something references them, and a root
/// `$ref`/`$dynamicRef` is not part of the profile.
pub const ROOT_ANNOTATION_KEYWORDS: &[&str] = &[
    "$schema",
    "$id",
    "$comment",
    "$defs",
    "definitions",
    "title",
    "description",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
];

/// The reference applicators refused anywhere in a `ModelSelectable`
/// canonical schema, at any depth.
///
/// Compilation decorates the root *in place*, so it is only sound while the
/// root schema is the sole description of the root instance. A reference can
/// re-enter the decorated root from inside a nested subschema — `{"child":
/// {"$ref": "#"}}` is the canonical example — which makes the injected
/// selector propagate into nested business objects and breaks the projection
/// equivalence in both directions: business arguments that satisfy the
/// canonical schema stop satisfying the compiled one, and invocations the
/// compiled schema accepts stop satisfying the canonical schema once the
/// top-level selector is stripped.
///
/// rustX refuses reference applicators outright rather than resolving URIs
/// to decide which ones reach the root: deciding that is a JSON Schema
/// reference resolver, and this contract is meant to be checkable by
/// inspection. The refusal is scoped to `ModelSelectable`, so a
/// reference-heavy schema stays fully usable under a fixed policy.
pub const REFERENCE_APPLICATOR_KEYWORDS: &[&str] = &["$ref", "$dynamicRef", "$recursiveRef"];

/// Keywords whose value is exactly one subschema.
///
/// `additionalProperties`, `unevaluatedItems`, and `unevaluatedProperties`
/// may also hold a boolean, which is a valid schema carrying nothing to
/// inspect.
const SUBSCHEMA_KEYWORDS: &[&str] = &[
    "additionalItems",
    "additionalProperties",
    "contains",
    "contentSchema",
    "else",
    "if",
    "not",
    "propertyNames",
    "then",
    "unevaluatedItems",
    "unevaluatedProperties",
];

/// Keywords whose value is an array of subschemas.
const SUBSCHEMA_LIST_KEYWORDS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

/// Keywords whose value maps *names* to subschemas.
///
/// A key here is a property or definition name, never a schema keyword, so
/// only the mapped values are schema nodes. That distinction is what lets a
/// business property legally be called `$ref`.
const SCHEMA_MAP_KEYWORDS: &[&str] = &[
    "properties",
    "patternProperties",
    "dependentSchemas",
    "$defs",
    "definitions",
];

/// Whether one root keyword belongs to the decoratable root profile./// Whether one root keyword belongs to the decoratable root profile.
///
/// The profile is an allowlist: anything absent from both
/// [`DECORATABLE_ROOT_INSTANCE_KEYWORDS`] and [`ROOT_ANNOTATION_KEYWORDS`]
/// is refused under `ModelSelectable`, whatever draft introduced it. That is
/// what makes the contract provable rather than a running list of known
/// hazards.
#[must_use]
pub fn is_decoratable_root_keyword(keyword: &str) -> bool {
    DECORATABLE_ROOT_INSTANCE_KEYWORDS.contains(&keyword)
        || ROOT_ANNOTATION_KEYWORDS.contains(&keyword)
}

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
    /// A `ModelSelectable` tool's canonical business schema already claims
    /// the top-level `execution_mode` name — as a property, as a `required`
    /// entry, or both — which rustX reserves for invocation ownership under
    /// that policy.
    ExecutionModeCollision(ExecutionModeClaim),
    /// A `ModelSelectable` tool's canonical schema uses a root keyword
    /// outside the decoratable root profile, so rustX cannot prove that
    /// decorating it leaves the schema satisfiable.
    UndecoratableRootSchema(String),
    /// A `ModelSelectable` tool's canonical schema uses a reference
    /// applicator, which may re-enter the decorated root from any depth.
    ReferenceApplicator {
        /// The reference keyword found.
        keyword: String,
        /// A JSON-pointer-style location of the schema object holding it.
        location: String,
    },
    /// A `ModelSelectable` invocation omitted the required `execution_mode`
    /// field.
    MissingExecutionMode,
    /// The `execution_mode` value is not `foreground` or `background`.
    InvalidExecutionMode(String),
    /// The `elevation_reason` value is not a string, null, or within bounds.
    InvalidElevationReason(String),
    /// The canonical schema already claims the reserved `elevation_reason` name.
    ElevationReasonCollision(ExecutionModeClaim),
    /// The canonical schema cannot carry optional top-level elevation metadata.
    UndecorableElevationRoot(String),
    /// The business arguments violate the canonical schema.
    InvalidArguments(Vec<String>),
}

/// How a canonical schema claims the reserved `execution_mode` name.
///
/// Both forms are fatal under `ModelSelectable`, but they fail differently
/// and deserve different wording: a declared property would be shadowed by
/// the injected selector, while a bare `required` entry can never be
/// satisfied at all, because the runtime strips the selector before the
/// canonical schema validates anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionModeClaim {
    /// The root `properties` map declares `execution_mode`.
    Property,
    /// The root `required` array demands `execution_mode`.
    Required,
    /// Both the root `properties` map and the root `required` array.
    PropertyAndRequired,
}

impl ExecutionModeClaim {
    /// The schema location this claim came from.
    const fn location(self) -> &'static str {
        match self {
            Self::Property => "\"properties\"",
            Self::Required => "\"required\"",
            Self::PropertyAndRequired => "\"properties\" and \"required\"",
        }
    }
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
            Self::ReservedInvocationProperty(_) => write!(
                f,
                "$ contains a property in the reserved {RUNTIME_PROPERTY_PREFIX:?} namespace. \
                 Remove all top-level {RUNTIME_PROPERTY_PREFIX}* fields; \
                 reserved invocation metadata is never forwarded to executors."
            ),
            Self::ExecutionModeCollision(claim) => write!(
                f,
                "the canonical tool schema claims the top-level {EXECUTION_MODE_FIELD:?} name in \
                 root {}, which rustX reserves for ModelSelectable invocation ownership; the \
                 runtime strips {EXECUTION_MODE_FIELD:?} before the canonical schema validates \
                 anything, so this tool could never accept a call. Either rename the tool's \
                 business field or choose a non-ModelSelectable execution policy",
                claim.location()
            ),
            Self::UndecoratableRootSchema(keyword) => write!(
                f,
                "the canonical tool schema uses the root keyword {keyword:?}, which is outside \
                 the decoratable root profile, so rustX cannot prove that injecting the required \
                 {EXECUTION_MODE_FIELD:?} selector leaves the schema satisfiable; describe the \
                 tool's top-level arguments with root \"type\"/\"properties\"/\"required\"/\
                 \"additionalProperties\" only — nested subschemas stay unrestricted — or choose \
                 a non-ModelSelectable execution policy, which needs no injected field"
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
            Self::ReferenceApplicator { keyword, location } => write!(
                f,
                "the canonical tool schema uses the reference applicator {keyword:?} at {}, and \
                 rustX compiles a ModelSelectable tool by decorating its root schema in place; a \
                 reference can re-enter that decorated root, which would make the injected \
                 {EXECUTION_MODE_FIELD:?} selector propagate into nested business objects. Inline \
                 the referenced subschema, or choose a non-ModelSelectable execution policy, \
                 which decorates nothing and accepts references freely",
                if location.is_empty() {
                    "the schema root".to_owned()
                } else {
                    format!("{location:?}")
                }
            ),
            Self::InvalidElevationReason(detail) => write!(
                f,
                "the {ELEVATION_REASON_FIELD:?} field must be a string of at most \
                 {ELEVATION_REASON_MAX_UTF8_BYTES} UTF-8 bytes or null, got {detail}"
            ),
            Self::ElevationReasonCollision(claim) => write!(
                f,
                "the canonical tool schema claims the top-level {ELEVATION_REASON_FIELD:?} name in \
                 root {}, which rustX reserves for permission-elevation metadata; rename the \
                 business field so model-facing decoration and pre-dispatch stripping stay \
                 equivalent",
                claim.location()
            ),
            Self::UndecorableElevationRoot(detail) => write!(
                f,
                "the canonical tool schema cannot carry optional top-level \
                 {ELEVATION_REASON_FIELD:?} metadata: {detail}"
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

/// Validates the policy-dependent `execution_mode` reservation and the
/// decoratable-root requirement.
///
/// `execution_mode` becomes a reserved model-facing invocation field when
/// and only when the effective execution policy is `ModelSelectable`. Under
/// that policy this rejects, deterministically and without any automatic
/// compatibility mapping:
///
/// - a canonical schema that claims the top-level `execution_mode` name,
///   whether by declaring it in root `properties`, by demanding it in root
///   `required`, or both; and
/// - a canonical schema whose root uses any keyword outside the decoratable
///   root profile (see [`is_decoratable_root_keyword`]), because rustX
///   cannot prove that injecting a required root property leaves such a
///   schema satisfiable.
///
/// - a canonical schema using a reference applicator at any depth (see
///   [`REFERENCE_APPLICATOR_KEYWORDS`]), because decoration edits the root in
///   place and a reference may re-enter it.
///
/// The three rejections together enforce the projection equivalence
/// `canonical(B)` ⇔ `compiled(B + top-level execution_mode)`, so a
/// registered tool can never reach the state where it compiles into a
/// model-facing definition yet no correct model call can succeed. A claim on
/// the reserved name reaches that state through the strip/validate asymmetry
/// — the runtime removes `execution_mode` before canonical validation, so
/// the claim can never be satisfied. A root keyword outside the profile
/// reaches it by contradicting the injection itself, as `maxProperties`, a
/// root `const`/`enum`, Draft-7 `dependencies`, or an unaware composition
/// branch all do. A reference reaches it by carrying the injected property
/// into nested business objects, breaking the equivalence both ways at once.
///
/// Neither rule applies under `ForegroundOnly`/`BackgroundOnly`, which
/// receive no synthetic field: an arbitrary composed root schema stays
/// valid there, `execution_mode` included.
///
/// # Errors
///
/// Returns [`SchemaError::ExecutionModeCollision`] when the schema claims
/// the reserved name, [`SchemaError::UndecoratableRootSchema`] when the root
/// cannot carry the injected selector, and
/// [`SchemaError::ReferenceApplicator`] when the schema references another
/// schema at any depth.
pub fn validate_execution_metadata_contract(
    policy: ToolExecutionPolicy,
    schema: &serde_json::Value,
) -> Result<(), SchemaError> {
    if policy != ToolExecutionPolicy::ModelSelectable {
        return Ok(());
    }
    let Some(root) = schema.as_object() else {
        return Ok(());
    };
    let declares = root
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|properties| properties.contains_key(EXECUTION_MODE_FIELD));
    let requires = root
        .get("required")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|required| {
            required
                .iter()
                .any(|entry| entry.as_str() == Some(EXECUTION_MODE_FIELD))
        });
    match (declares, requires) {
        (true, true) => {
            return Err(SchemaError::ExecutionModeCollision(
                ExecutionModeClaim::PropertyAndRequired,
            ));
        }
        (true, false) => {
            return Err(SchemaError::ExecutionModeCollision(
                ExecutionModeClaim::Property,
            ));
        }
        (false, true) => {
            return Err(SchemaError::ExecutionModeCollision(
                ExecutionModeClaim::Required,
            ));
        }
        (false, false) => {}
    }
    // The reference scan runs before the root profile so a root `$ref` is
    // reported as what it is, rather than as a generic profile violation.
    reject_reference_applicators(schema, "")?;
    for keyword in root.keys() {
        if !is_decoratable_root_keyword(keyword) {
            return Err(SchemaError::UndecoratableRootSchema(keyword.clone()));
        }
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
    let mut input_schema = compile_elevation_metadata(&definition.input_schema)?;
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

fn root_claims_property(schema: &serde_json::Value, name: &str) -> bool {
    schema
        .as_object()
        .and_then(|root| root.get("properties"))
        .and_then(serde_json::Value::as_object)
        .is_some_and(|properties| properties.contains_key(name))
}

fn root_requires_property(schema: &serde_json::Value, name: &str) -> bool {
    schema
        .as_object()
        .and_then(|root| root.get("required"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|required| required.iter().any(|entry| entry.as_str() == Some(name)))
}

fn elevation_property_schema() -> serde_json::Value {
    serde_json::json!({
        "type": ["string", "null"],
        "maxLength": ELEVATION_REASON_MAX_UTF8_BYTES,
        "description": ELEVATION_REASON_DESCRIPTION,
    })
}

fn elevation_collision_claim(schema: &serde_json::Value) -> Option<ExecutionModeClaim> {
    let declares = root_claims_property(schema, ELEVATION_REASON_FIELD);
    let requires = root_requires_property(schema, ELEVATION_REASON_FIELD);
    match (declares, requires) {
        (true, true) => Some(ExecutionModeClaim::PropertyAndRequired),
        (true, false) => Some(ExecutionModeClaim::Property),
        (false, true) => Some(ExecutionModeClaim::Required),
        (false, false) => None,
    }
}

/// Compiles model-facing elevation metadata without mutating the business
/// schema identity that nested `$ref: "#"` would re-enter.
///
/// The undecorated canonical schema is preserved under
/// `$defs/rustx_business`. Model-facing compilation then admits optional
/// [`ELEVATION_REASON_FIELD`] on every schema object that constrains the root
/// instance — including nested composition branches and inlined local `$ref`
/// targets — so a legal metadata call validates against the model schema if
/// and only if stripping metadata leaves arguments that satisfy the original
/// business schema.
fn compile_elevation_metadata(
    canonical: &serde_json::Value,
) -> Result<serde_json::Value, SchemaError> {
    reject_elevation_collision_on_root_instance(canonical, canonical, &mut Vec::new())?;

    let mut business = canonical.clone();
    rewrite_pound_refs_in_subschemas(&mut business);

    let mut compiled = canonical.clone();
    // Inline local root `$ref`s and inject elevation before rewriting nested
    // `#` pointers, so closed ref targets receive the optional field instead
    // of relying on ignored/ineffective `$ref` siblings.
    inject_elevation_for_root_instance(&mut compiled, canonical, &mut Vec::new())?;
    rewrite_pound_refs_in_subschemas(&mut compiled);

    let root = compiled.as_object_mut().ok_or_else(|| {
        SchemaError::UndecorableElevationRoot("canonical schema root must be a JSON object".into())
    })?;
    let defs = root
        .entry("$defs")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| SchemaError::UndecorableElevationRoot("$defs must be an object".into()))?;
    if defs.contains_key(RUSTX_BUSINESS_DEF) {
        return Err(SchemaError::UndecorableElevationRoot(
            "canonical schema already defines $defs.rustx_business".into(),
        ));
    }
    defs.insert(RUSTX_BUSINESS_DEF.to_owned(), business);
    Ok(compiled)
}

/// Rejects every root-applicable claim on the reserved elevation field,
/// including composition branches and local `$ref` targets that describe the
/// same instance as the document root.
fn reject_elevation_collision_on_root_instance(
    node: &serde_json::Value,
    document: &serde_json::Value,
    stack: &mut Vec<String>,
) -> Result<(), SchemaError> {
    if let Some(claim) = elevation_collision_claim(node) {
        return Err(SchemaError::ElevationReasonCollision(claim));
    }
    let Some(object) = node.as_object() else {
        return Ok(());
    };
    if object.contains_key("const") || object.contains_key("enum") {
        return Err(SchemaError::UndecorableElevationRoot(
            "root const/enum cannot admit optional elevation_reason".into(),
        ));
    }
    for key in ["$ref", "$recursiveRef"] {
        if let Some(reference) = object.get(key).and_then(serde_json::Value::as_str)
            && let Some(target) = resolve_local_schema_ref(document, reference)
            && !stack.contains(&reference.to_owned())
        {
            stack.push(reference.to_owned());
            reject_elevation_collision_on_root_instance(target, document, stack)?;
            stack.pop();
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(branches) = object.get(keyword).and_then(serde_json::Value::as_array) {
            for branch in branches {
                reject_elevation_collision_on_root_instance(branch, document, stack)?;
            }
        }
    }
    Ok(())
}

/// Rewrites `$ref`/`$recursiveRef` values of `"#"` to the undecorated business
/// definition, walking only real subschema positions so nested `const` /
/// `enum` / annotation data that happens to contain `{"$ref":"#"}` is left
/// alone.
fn rewrite_pound_refs_in_subschemas(schema: &mut serde_json::Value) {
    let Some(node) = schema.as_object_mut() else {
        return;
    };
    for key in ["$ref", "$recursiveRef"] {
        if node.get(key).and_then(serde_json::Value::as_str) == Some("#") {
            node.insert(
                key.to_owned(),
                serde_json::Value::String(format!("#/$defs/{RUSTX_BUSINESS_DEF}")),
            );
        }
    }
    let keys: Vec<String> = node.keys().cloned().collect();
    for keyword in keys {
        let Some(value) = node.get_mut(&keyword) else {
            continue;
        };
        if SUBSCHEMA_KEYWORDS.contains(&keyword.as_str()) {
            rewrite_pound_refs_in_subschemas(value);
        } else if SUBSCHEMA_LIST_KEYWORDS.contains(&keyword.as_str()) {
            if let serde_json::Value::Array(items) = value {
                for item in items {
                    rewrite_pound_refs_in_subschemas(item);
                }
            }
        } else if SCHEMA_MAP_KEYWORDS.contains(&keyword.as_str()) {
            if let serde_json::Value::Object(members) = value {
                for subschema in members.values_mut() {
                    rewrite_pound_refs_in_subschemas(subschema);
                }
            }
        } else if keyword == "items" {
            if let serde_json::Value::Array(items) = value {
                for item in items {
                    rewrite_pound_refs_in_subschemas(item);
                }
            } else {
                rewrite_pound_refs_in_subschemas(value);
            }
        } else if keyword == "dependencies"
            && let serde_json::Value::Object(members) = value
        {
            for subschema in members.values_mut() {
                if subschema.is_object() {
                    rewrite_pound_refs_in_subschemas(subschema);
                }
            }
        }
    }
}

fn resolve_local_schema_ref<'a>(
    document: &'a serde_json::Value,
    reference: &str,
) -> Option<&'a serde_json::Value> {
    if reference == "#" {
        return Some(document);
    }
    let path = reference.strip_prefix("#/")?;
    let mut current = document;
    for raw in path.split('/') {
        let token = raw.replace("~1", "/").replace("~0", "~");
        current = current.as_object()?.get(&token)?;
    }
    Some(current)
}

/// Injects optional elevation metadata into every schema object that
/// constrains the root instance.
fn inject_elevation_for_root_instance(
    node: &mut serde_json::Value,
    document: &serde_json::Value,
    stack: &mut Vec<String>,
) -> Result<(), SchemaError> {
    // Inline local `$ref`/`$recursiveRef` targets that describe this same
    // root instance. Sibling keywords next to `$ref` cannot open a closed
    // target (`additionalProperties: false`), so the target must receive the
    // optional field itself — and only for this root application, via inline.
    inline_local_root_ref(node, document, stack)?;

    {
        let Some(object) = node.as_object_mut() else {
            return Ok(());
        };
        if object.contains_key("const") || object.contains_key("enum") {
            return Err(SchemaError::UndecorableElevationRoot(
                "root const/enum cannot admit optional elevation_reason".into(),
            ));
        }
        for keyword in ["allOf", "anyOf", "oneOf"] {
            // Take branches out so we can recurse without overlapping borrows.
            let Some(serde_json::Value::Array(mut branches)) = object.remove(keyword) else {
                continue;
            };
            for branch in &mut branches {
                inject_elevation_for_root_instance(branch, document, stack)?;
            }
            object.insert(keyword.to_owned(), serde_json::Value::Array(branches));
        }
    }
    inject_elevation_into_object_schema(node)
}

fn inline_local_root_ref(
    node: &mut serde_json::Value,
    document: &serde_json::Value,
    stack: &mut Vec<String>,
) -> Result<(), SchemaError> {
    let Some(object) = node.as_object_mut() else {
        return Ok(());
    };
    let reference = ["$ref", "$recursiveRef"].iter().find_map(|key| {
        object
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(|value| ((*key).to_owned(), value.to_owned()))
    });
    let Some((keyword, reference)) = reference else {
        return Ok(());
    };
    // Document-root `#` stays a reference: nested rewriting points it at the
    // undecorated business def. Inlining `#` would re-merge the same `$ref`
    // and never open a closed target.
    if reference == "#" {
        return Ok(());
    }
    let Some(target) = resolve_local_schema_ref(document, &reference) else {
        return Ok(());
    };
    if stack.contains(&reference) {
        return Ok(());
    }
    let target = target.clone();
    object.remove(&keyword);
    if let serde_json::Value::Object(target_object) = target {
        for (key, value) in target_object {
            object.entry(key).or_insert(value);
        }
    } else {
        // Boolean / non-object targets have nothing to merge; keep siblings.
    }
    stack.push(reference);
    // Continue inlining if the merged target itself was a ref wrapper.
    let result = inline_local_root_ref(node, document, stack);
    stack.pop();
    result
}

fn inject_elevation_into_object_schema(schema: &mut serde_json::Value) -> Result<(), SchemaError> {
    if let Some(claim) = elevation_collision_claim(schema) {
        return Err(SchemaError::ElevationReasonCollision(claim));
    }
    let property_names_ok = match schema
        .as_object()
        .and_then(|object| object.get("propertyNames"))
    {
        None => true,
        Some(property_names) => {
            // Unknown whether the name schema is usable until we try.
            Validator::new(property_names).is_ok_and(|validator| {
                validator.is_valid(&serde_json::Value::String(
                    ELEVATION_REASON_FIELD.to_owned(),
                ))
            })
        }
    };
    if !property_names_ok {
        return Err(SchemaError::UndecorableElevationRoot(
            "root propertyNames rejects the reserved elevation_reason field".into(),
        ));
    }
    let Some(object) = schema.as_object_mut() else {
        return Ok(());
    };
    let is_objectish = object.get("type").and_then(serde_json::Value::as_str) == Some("object")
        || object.contains_key("properties")
        || object.contains_key("additionalProperties")
        || object.contains_key("minProperties")
        || object.contains_key("maxProperties")
        || object.contains_key("$ref")
        || object.contains_key("$recursiveRef");
    if !is_objectish {
        return Ok(());
    }

    // Cardinality must treat elevation as optional metadata: bump the bound
    // only when the field is present. Unconditional ±1 accepts illegal
    // business shapes or rejects empty-after-strip calls.
    adjust_property_cardinality_for_optional_elevation(object)?;

    object
        .entry("properties")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            SchemaError::UndecorableElevationRoot("properties must be an object".into())
        })?
        .insert(
            ELEVATION_REASON_FIELD.to_owned(),
            elevation_property_schema(),
        );
    Ok(())
}

fn adjust_property_cardinality_for_optional_elevation(
    object: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), SchemaError> {
    let max = match object.get("maxProperties") {
        Some(serde_json::Value::Number(number)) => {
            let Some(value) = number.as_u64() else {
                return Err(SchemaError::UndecorableElevationRoot(
                    "root maxProperties must be a non-negative integer".into(),
                ));
            };
            Some(value)
        }
        Some(_) => {
            return Err(SchemaError::UndecorableElevationRoot(
                "root maxProperties must be a non-negative integer".into(),
            ));
        }
        None => None,
    };
    let min = match object.get("minProperties") {
        Some(serde_json::Value::Number(number)) => {
            let Some(value) = number.as_u64() else {
                return Err(SchemaError::UndecorableElevationRoot(
                    "root minProperties must be a non-negative integer".into(),
                ));
            };
            Some(value)
        }
        Some(_) => {
            return Err(SchemaError::UndecorableElevationRoot(
                "root minProperties must be a non-negative integer".into(),
            ));
        }
        None => None,
    };
    if max.is_none() && min.is_none() {
        return Ok(());
    }
    object.remove("maxProperties");
    object.remove("minProperties");

    // If the schema already uses if/then/else for business reasons, refuse
    // rather than silently overwrite those applicators.
    if object.contains_key("if") || object.contains_key("then") || object.contains_key("else") {
        return Err(SchemaError::UndecorableElevationRoot(
            "root if/then/else combined with minProperties/maxProperties cannot admit optional elevation_reason"
                .into(),
        ));
    }

    let mut then_schema = serde_json::Map::new();
    let mut else_schema = serde_json::Map::new();
    if let Some(value) = max {
        then_schema.insert(
            "maxProperties".into(),
            serde_json::Value::Number((value + 1).into()),
        );
        else_schema.insert(
            "maxProperties".into(),
            serde_json::Value::Number(value.into()),
        );
    }
    if let Some(value) = min {
        then_schema.insert(
            "minProperties".into(),
            serde_json::Value::Number((value + 1).into()),
        );
        else_schema.insert(
            "minProperties".into(),
            serde_json::Value::Number(value.into()),
        );
    }
    object.insert(
        "if".into(),
        serde_json::json!({ "required": [ELEVATION_REASON_FIELD] }),
    );
    object.insert("then".into(), serde_json::Value::Object(then_schema));
    object.insert("else".into(), serde_json::Value::Object(else_schema));
    Ok(())
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

/// Rejects every reference applicator in a canonical schema, at any depth.
///
/// The walk descends **only** through positions JSON Schema defines as
/// carrying subschemas — [`SUBSCHEMA_KEYWORDS`], [`SUBSCHEMA_LIST_KEYWORDS`],
/// [`SCHEMA_MAP_KEYWORDS`], plus `items` and the Draft-7 `dependencies`,
/// which each have two legal shapes. Anything else a schema object holds is
/// left alone.
///
/// That direction matters. Descending into every object-shaped value and
/// excluding known data positions would repeat, one level down, the mistake
/// the root profile already corrected: it mistakes JSON shape for schema
/// position and misreads names and annotations as keywords. `{"$ref":
/// ["other"]}` under `dependentRequired` is a *property name* whose presence
/// implies another; the value of an unrecognized keyword is annotation data
/// that JSON Schema explicitly does not treat as a subschema. Neither is an
/// applicator, and neither is walked.
///
/// # Errors
///
/// Returns [`SchemaError::ReferenceApplicator`] for the first reference
/// found, with a JSON-pointer-style location.
fn reject_reference_applicators(
    schema: &serde_json::Value,
    location: &str,
) -> Result<(), SchemaError> {
    // A boolean is a valid schema with nothing to inspect; so is anything
    // malformed enough not to be an object, which registration rejects
    // elsewhere.
    let Some(node) = schema.as_object() else {
        return Ok(());
    };
    for keyword in REFERENCE_APPLICATOR_KEYWORDS {
        if node.contains_key(*keyword) {
            return Err(SchemaError::ReferenceApplicator {
                keyword: (*keyword).to_owned(),
                location: location.to_owned(),
            });
        }
    }
    for (keyword, value) in node {
        let next = format!("{location}/{}", escape_pointer_token(keyword));
        if SUBSCHEMA_KEYWORDS.contains(&keyword.as_str()) {
            reject_reference_applicators(value, &next)?;
        } else if SUBSCHEMA_LIST_KEYWORDS.contains(&keyword.as_str()) {
            reject_reference_applicators_in_list(value, &next)?;
        } else if SCHEMA_MAP_KEYWORDS.contains(&keyword.as_str()) {
            reject_reference_applicators_in_map(value, &next)?;
        } else if keyword == "items" {
            // One subschema in 2020-12; an array of them in older drafts.
            if value.is_array() {
                reject_reference_applicators_in_list(value, &next)?;
            } else {
                reject_reference_applicators(value, &next)?;
            }
        } else if keyword == "dependencies" {
            // Draft-7 mapped a property name to either a subschema or a list
            // of required property names. 2020-12 split those into
            // `dependentSchemas` and `dependentRequired` precisely because
            // the two shapes are not interchangeable.
            reject_reference_applicators_in_map(value, &next)?;
        }
    }
    Ok(())
}

/// Walks each element of a subschema array.
///
/// # Errors
///
/// Returns [`SchemaError::ReferenceApplicator`] for the first reference
/// found.
fn reject_reference_applicators_in_list(
    value: &serde_json::Value,
    location: &str,
) -> Result<(), SchemaError> {
    let Some(items) = value.as_array() else {
        return Ok(());
    };
    for (index, item) in items.iter().enumerate() {
        reject_reference_applicators(item, &format!("{location}/{index}"))?;
    }
    Ok(())
}

/// Walks the mapped values of a name-to-subschema map, never its keys.
///
/// A non-object value is skipped: that is the Draft-7 `dependencies` shape
/// listing required property names, which carries no subschema.
///
/// # Errors
///
/// Returns [`SchemaError::ReferenceApplicator`] for the first reference
/// found.
fn reject_reference_applicators_in_map(
    value: &serde_json::Value,
    location: &str,
) -> Result<(), SchemaError> {
    let Some(members) = value.as_object() else {
        return Ok(());
    };
    for (name, subschema) in members {
        if !subschema.is_object() {
            continue;
        }
        reject_reference_applicators(
            subschema,
            &format!("{location}/{}", escape_pointer_token(name)),
        )?;
    }
    Ok(())
}

/// Escapes one JSON Pointer reference token (RFC 6901).
fn escape_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
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
) -> Result<(ToolInvocationMode, Option<String>, serde_json::Value), SchemaError> {
    reject_reserved_invocation_properties(arguments)?;
    let (mode, mut stripped) = match policy {
        ToolExecutionPolicy::ForegroundOnly => (ToolInvocationMode::Foreground, arguments.clone()),
        ToolExecutionPolicy::BackgroundOnly => (ToolInvocationMode::Background, arguments.clone()),
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
            (mode, stripped)
        }
    };
    let elevation_reason = match stripped
        .as_object_mut()
        .and_then(|object| object.remove(ELEVATION_REASON_FIELD))
    {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) => {
            let trimmed = text.trim();
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
        Some(other) => {
            return Err(SchemaError::InvalidElevationReason(other.to_string()));
        }
    };
    Ok((mode, elevation_reason, stripped))
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
        ELEVATION_REASON_FIELD, ELEVATION_REASON_MAX_UTF8_BYTES,
        EXECUTION_MODE_DESCRIPTION_REMINDER, EXECUTION_MODE_FIELD, compile_model_definition,
        resolve_invocation_metadata, validate_business_arguments, validate_canonical_schema,
        validate_execution_metadata_contract,
    };
    use crate::runtime::identity::ToolId;
    use crate::tools::types::{
        ToolDefinition, ToolExecutionPolicy, ToolInvocationMode, ToolOrigin, ToolReplayPolicy,
    };
    use serde_json::json;

    fn definition(policy: ToolExecutionPolicy, schema: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            id: ToolId::new("tool-x"),
            name: "x".to_owned(),
            description: String::new(),
            input_schema: schema,
            execution_policy: policy,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        }
    }

    /// Whether `elevation_reason` is absent, null, or a string within the
    /// UTF-8 byte bound. Overlong / wrong-typed metadata is not a legal call.
    fn elevation_metadata_is_legal(arguments: &serde_json::Value) -> bool {
        match arguments.get(ELEVATION_REASON_FIELD) {
            None | Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::String(text)) => {
                let trimmed = text.trim();
                trimmed.is_empty() || trimmed.len() <= ELEVATION_REASON_MAX_UTF8_BYTES
            }
            Some(_) => false,
        }
    }

    /// Model-schema acceptance for a legal elevation metadata call, including
    /// the UTF-8 byte bound JSON Schema `maxLength` only approximates.
    fn model_accepts_legal_elevation_call(
        model_schema: &serde_json::Value,
        arguments: &serde_json::Value,
    ) -> bool {
        if !elevation_metadata_is_legal(arguments) {
            return false;
        }
        let Ok(validator) = jsonschema::Validator::new(model_schema) else {
            return false;
        };
        validator.is_valid(arguments)
    }

    fn strip_elevation(arguments: &serde_json::Value) -> serde_json::Value {
        let mut stripped = arguments.clone();
        if let Some(object) = stripped.as_object_mut() {
            object.remove(ELEVATION_REASON_FIELD);
        }
        stripped
    }

    /// Projection contract: for every legal metadata call, the model schema
    /// accepts it iff the stripped business args satisfy the canonical schema.
    fn assert_elevation_projection_equivalence(
        canonical: &serde_json::Value,
        model_schema: &serde_json::Value,
        candidates: &[serde_json::Value],
    ) {
        // Some fixtures (dangling `$dynamicRef`) are not standalone validating
        // documents; registration would refuse them. Skip the oracle there.
        let Ok(business_validator) = jsonschema::Validator::new(canonical) else {
            return;
        };
        let Ok(model_validator) = jsonschema::Validator::new(model_schema) else {
            panic!("compiled model schema must be a valid JSON Schema: {model_schema}");
        };
        for arguments in candidates {
            if !elevation_metadata_is_legal(arguments) {
                continue;
            }
            let stripped = strip_elevation(arguments);
            let business_ok = business_validator.is_valid(&stripped);
            let model_ok =
                model_validator.is_valid(arguments) && elevation_metadata_is_legal(arguments);
            assert_eq!(
                model_ok, business_ok,
                "elevation projection broken for {arguments}: model={model_ok} business={business_ok} under canonical {canonical} / model {model_schema}"
            );
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
        assert!(
            compiled.input_schema["properties"]
                .get(ELEVATION_REASON_FIELD)
                .is_some(),
            "the foreground schema gains elevation_reason"
        );
        assert!(
            compiled.input_schema["properties"]
                .get(EXECUTION_MODE_FIELD)
                .is_none(),
            "fixed policy does not gain execution_mode"
        );
        assert_elevation_projection_equivalence(
            &original,
            &compiled.input_schema,
            &[
                json!({"path": "a.txt"}),
                json!({"path": "a.txt", "elevation_reason": "need write"}),
                json!({"elevation_reason": "need write"}),
                json!({}),
                json!({"path": 1}),
            ],
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

    /// Every way a canonical root schema can claim the reserved
    /// `execution_mode` name is rejected under `ModelSelectable` — including
    /// a bare `required` entry with no declared property, which would
    /// otherwise register successfully and then reject every correct call,
    /// because the runtime strips the selector before canonical validation.
    #[test]
    fn every_execution_mode_claim_is_rejected_under_model_selectable() {
        let declared = json!({
            "type": "object",
            "properties": {"execution_mode": {"type": "string"}},
        });
        let required_only = json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command", "execution_mode"],
        });
        let both = json!({
            "type": "object",
            "properties": {"execution_mode": {"type": "string"}},
            "required": ["execution_mode"],
        });
        for (schema, expected) in [
            (&declared, super::ExecutionModeClaim::Property),
            (&required_only, super::ExecutionModeClaim::Required),
            (&both, super::ExecutionModeClaim::PropertyAndRequired),
        ] {
            assert!(
                validate_canonical_schema(schema).is_ok(),
                "the canonical schema validator stays policy-unaware: {schema}"
            );
            assert_eq!(
                validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, schema),
                Err(super::SchemaError::ExecutionModeCollision(expected)),
                "unexpected verdict for {schema}"
            );
            assert_eq!(
                compile_model_definition(&definition(
                    ToolExecutionPolicy::ModelSelectable,
                    schema.clone()
                ))
                .expect_err("compilation is refused"),
                super::SchemaError::ExecutionModeCollision(expected)
            );
        }
    }

    /// The identical schemas stay ordinary business schemas under a fixed
    /// execution policy, which needs no injected field.
    #[test]
    fn execution_mode_claims_are_legal_under_a_fixed_policy() {
        let schemas = [
            json!({"type": "object", "properties": {"execution_mode": {"type": "string"}}}),
            json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command", "execution_mode"],
            }),
        ];
        for policy in [
            ToolExecutionPolicy::ForegroundOnly,
            ToolExecutionPolicy::BackgroundOnly,
        ] {
            for schema in &schemas {
                validate_execution_metadata_contract(policy, schema).expect("fixed policy");
                let compiled = compile_model_definition(&definition(policy, schema.clone()))
                    .expect("fixed policy compiles");
                assert!(
                    compiled.input_schema["properties"]
                        .get(ELEVATION_REASON_FIELD)
                        .is_some(),
                    "fixed policy still gains elevation_reason under {policy:?}"
                );
                assert_elevation_projection_equivalence(
                    schema,
                    &compiled.input_schema,
                    &[
                        json!({"execution_mode": "whatever"}),
                        json!({"execution_mode": "whatever", "elevation_reason": "ask"}),
                        json!({"command": "ls", "execution_mode": "x"}),
                        json!({"command": "ls", "execution_mode": "x", "elevation_reason": "ask"}),
                        json!({"elevation_reason": "ask"}),
                        json!({}),
                    ],
                );
            }
        }
    }

    /// Only the decoratable root profile is accepted under `ModelSelectable`.
    ///
    /// The cases span cardinality assertions, whole-instance assertions, both
    /// dependency spellings, and every composition applicator across drafts.
    /// An allowlist catches all of them — including keywords nobody
    /// enumerated in advance — which is what makes the contract provable
    /// rather than a running list of known hazards.
    ///
    /// `canonical_ok` records whether the schema is also a valid standalone
    /// JSON Schema. Dangling references are refused earlier, by the
    /// policy-unaware validator; the profile still refuses them here.
    /// Root keywords that assert something about the object *as a whole*.
    ///
    /// Each entry pairs the keyword with a schema exercising it and whether
    /// that schema is also a valid standalone JSON Schema.
    fn undecoratable_assertion_cases() -> Vec<(&'static str, serde_json::Value, bool)> {
        let business = json!({"command": {"type": "string"}});
        vec![
            // Cardinality assertions: unsatisfiable once a property is added.
            (
                "maxProperties",
                json!({
                    "type": "object",
                    "properties": business,
                    "required": ["command"],
                    "maxProperties": 1,
                }),
                true,
            ),
            (
                "minProperties",
                json!({"type": "object", "properties": business, "minProperties": 1}),
                true,
            ),
            // Whole-instance assertions: pin the object, contradicting any
            // injected property.
            (
                "const",
                json!({"type": "object", "const": {"command": "ls"}}),
                true,
            ),
            (
                "enum",
                json!({"type": "object", "enum": [{"command": "ls"}]}),
                true,
            ),
            // Both dependency spellings: the Draft-7 keyword is as fatal as
            // the 2019-09 split that replaced it.
            (
                "dependencies",
                json!({
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "type": "object",
                    "properties": business,
                    "dependencies": {"command": ["execution_mode"]},
                }),
                true,
            ),
            (
                "dependentRequired",
                json!({"type": "object", "dependentRequired": {"command": ["other"]}}),
                true,
            ),
            (
                "dependentSchemas",
                json!({"type": "object", "dependentSchemas": {"command": {}}}),
                true,
            ),
        ]
    }

    /// Root keywords that apply *other schemas* to the same root instance, or
    /// constrain its property names. Reference applicators are covered
    /// separately, by `reference_applicators_are_refused_at_any_depth`.
    fn undecoratable_applicator_cases() -> Vec<(&'static str, serde_json::Value, bool)> {
        let business = json!({"command": {"type": "string"}});
        vec![
            (
                "allOf",
                json!({
                    "type": "object",
                    "properties": business,
                    "allOf": [{"required": ["execution_mode"]}],
                }),
                true,
            ),
            (
                "anyOf",
                json!({"type": "object", "anyOf": [{"type": "object"}]}),
                true,
            ),
            (
                "oneOf",
                json!({"type": "object", "oneOf": [{"type": "object"}]}),
                true,
            ),
            (
                "not",
                json!({"type": "object", "not": {"type": "array"}}),
                true,
            ),
            (
                "if",
                json!({"type": "object", "if": {"type": "object"}}),
                true,
            ),
            (
                "patternProperties",
                json!({"type": "object", "patternProperties": {"^e": {"type": "string"}}}),
                true,
            ),
            (
                "propertyNames",
                json!({"type": "object", "propertyNames": {"maxLength": 4}}),
                true,
            ),
            (
                "unevaluatedProperties",
                json!({"type": "object", "unevaluatedProperties": false}),
                true,
            ),
        ]
    }

    #[test]
    fn only_the_decoratable_root_profile_is_accepted_under_model_selectable() {
        let mut cases = undecoratable_assertion_cases();
        cases.extend(undecoratable_applicator_cases());

        for (keyword, schema, canonical_ok) in &cases {
            assert_eq!(
                validate_canonical_schema(schema).is_ok(),
                *canonical_ok,
                "unexpected policy-unaware verdict for root {keyword:?}: {schema}"
            );
            assert_eq!(
                validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, schema),
                Err(super::SchemaError::UndecoratableRootSchema(
                    (*keyword).to_owned()
                )),
                "root {keyword:?} must be refused under ModelSelectable"
            );
            assert_eq!(
                compile_model_definition(&definition(
                    ToolExecutionPolicy::ModelSelectable,
                    schema.clone()
                ))
                .expect_err("compilation is refused"),
                super::SchemaError::UndecoratableRootSchema((*keyword).to_owned())
            );
            for policy in [
                ToolExecutionPolicy::ForegroundOnly,
                ToolExecutionPolicy::BackgroundOnly,
            ] {
                match compile_model_definition(&definition(policy, schema.clone())) {
                    Ok(compiled) => {
                        // Sound wraps preserve projection; unsound roots must
                        // refuse compilation instead of emitting a schema that
                        // accepts calls whose stripped args fail the canonical
                        // business schema (const/enum).
                        assert_elevation_projection_equivalence(
                            schema,
                            &compiled.input_schema,
                            &[
                                json!({"command": "ls"}),
                                json!({"command": "ls", "elevation_reason": "ask"}),
                                json!({"elevation_reason": "ask"}),
                                json!({}),
                                json!({"command": "ls", "extra": 1}),
                                json!({"command": "ls", "extra": 1, "elevation_reason": "ask"}),
                            ],
                        );
                    }
                    Err(error) => {
                        assert!(
                            matches!(
                                error,
                                super::SchemaError::UndecorableElevationRoot(_)
                                    | super::SchemaError::ElevationReasonCollision(_)
                            ),
                            "root {keyword:?} refuses unsound elevation under {policy:?}: {error}"
                        );
                    }
                }
            }
        }
    }

    /// The refused root assertions really are fatal, through the two
    /// mechanisms the contract exists to prevent. This proves the profile
    /// blocks genuine dead states rather than merely being conservative.
    #[test]
    fn the_refused_root_assertions_really_are_fatal() {
        // Mechanism one: decoration itself contradicts a root assertion, so
        // the compiled model-facing schema has no valid instance at all.
        for hazard in [
            json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
                "maxProperties": 1,
            }),
            json!({"type": "object", "const": {"command": "ls"}}),
            json!({"type": "object", "enum": [{"command": "ls"}]}),
        ] {
            // Decorate exactly as compilation would, bypassing the contract.
            let mut decorated = hazard.clone();
            super::decorate_execution_mode(&mut decorated);
            let validator =
                jsonschema::Validator::new(&decorated).expect("the decorated schema is valid");
            for candidate in [
                json!({"command": "ls", "execution_mode": "foreground"}),
                json!({"command": "ls", "execution_mode": "background"}),
            ] {
                assert!(
                    !validator.is_valid(&candidate),
                    "a correct call must be impossible under {decorated}"
                );
            }
        }

        // Mechanism two: the canonical schema demands the selector the
        // runtime has already stripped, so business validation can never
        // pass however the model calls the tool.
        for hazard in [
            json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "dependencies": {"command": ["execution_mode"]},
            }),
            json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "allOf": [{"required": ["execution_mode"]}],
            }),
        ] {
            for mode in super::EXECUTION_MODE_VALUES {
                let call = json!({"command": "ls", "execution_mode": mode});
                let (_, _, stripped) =
                    resolve_invocation_metadata(ToolExecutionPolicy::ModelSelectable, &call)
                        .expect("a correct call resolves");
                assert!(
                    validate_business_arguments(&hazard, &stripped).is_err(),
                    "the stripped arguments can never satisfy {hazard}"
                );
            }
        }
    }

    /// Descriptive root keywords carry no instance assertion, so the profile
    /// accepts them and decoration proceeds normally.
    #[test]
    fn root_annotation_keywords_stay_decoratable() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "https://example.test/tool.json",
            "$comment": "an annotation",
            "$defs": {"unused": {"type": "string"}},
            "definitions": {"unused": {"type": "string"}},
            "title": "Tool",
            "description": "A tool.",
            "default": {"command": "ls"},
            "examples": [{"command": "ls"}],
            "deprecated": false,
            "readOnly": false,
            "writeOnly": false,
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        });
        for keyword in super::ROOT_ANNOTATION_KEYWORDS {
            assert!(
                schema.get(*keyword).is_some(),
                "the fixture exercises every annotation keyword, missing {keyword:?}"
            );
        }
        validate_canonical_schema(&schema).expect("annotations are valid JSON Schema");
        validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, &schema)
            .expect("annotations do not constrain the root instance");
        let compiled =
            compile_model_definition(&definition(ToolExecutionPolicy::ModelSelectable, schema))
                .expect("compile");
        let validator =
            jsonschema::Validator::new(&compiled.input_schema).expect("compiled schema is valid");
        assert!(
            validator.is_valid(&json!({"command": "ls", "execution_mode": "foreground"})),
            "the compiled schema accepts a correct call: {}",
            compiled.input_schema
        );
    }

    /// Reference applicators are refused at any depth under
    /// `ModelSelectable`, and only there.
    ///
    /// Compilation decorates the root in place, so a reference that re-enters
    /// the decorated root would make the injected selector propagate into
    /// nested business objects. rustX refuses references outright rather than
    /// resolving URIs to decide which ones reach the root.
    #[test]
    fn reference_applicators_are_refused_at_any_depth() {
        let cases = [
            ("$ref", "", json!({"type": "object", "$ref": "#"})),
            (
                "$ref",
                "/properties/child",
                json!({
                    "type": "object",
                    "properties": {"command": {"type": "string"}, "child": {"$ref": "#"}},
                    "required": ["command"],
                    "additionalProperties": false,
                }),
            ),
            (
                "$ref",
                "/properties/items/items",
                json!({
                    "type": "object",
                    "properties": {
                        "items": {"type": "array", "items": {"$ref": "#/$defs/x"}},
                    },
                    "$defs": {"x": {"type": "string"}},
                }),
            ),
            (
                "$dynamicRef",
                "/properties/child",
                json!({"type": "object", "properties": {"child": {"$dynamicRef": "#x"}}}),
            ),
            (
                "$recursiveRef",
                "/properties/child",
                json!({"type": "object", "properties": {"child": {"$recursiveRef": "#"}}}),
            ),
        ];
        for (keyword, location, schema) in &cases {
            assert_eq!(
                validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, schema),
                Err(super::SchemaError::ReferenceApplicator {
                    keyword: (*keyword).to_owned(),
                    location: (*location).to_owned(),
                }),
                "unexpected verdict for {schema}"
            );
            for policy in [
                ToolExecutionPolicy::ForegroundOnly,
                ToolExecutionPolicy::BackgroundOnly,
            ] {
                let compiled = compile_model_definition(&definition(policy, (*schema).clone()))
                    .unwrap_or_else(|error| {
                        panic!("references stay legal under {policy:?}: {error}")
                    });
                assert_eq!(
                    compiled.input_schema["$defs"]["rustx_business"]["properties"]
                        .get(ELEVATION_REASON_FIELD),
                    None,
                    "undecorated business $defs must not carry elevation_reason"
                );
                assert_elevation_projection_equivalence(
                    schema,
                    &compiled.input_schema,
                    &[
                        json!({}),
                        json!({"elevation_reason": "ask"}),
                        json!({"command": "ls"}),
                        json!({"command": "ls", "elevation_reason": "ask"}),
                        json!({"command": "ls", "child": {}}),
                        json!({"command": "ls", "child": {}, "elevation_reason": "ask"}),
                        json!({"items": ["x"]}),
                        json!({"items": ["x"], "elevation_reason": "ask"}),
                        json!({"child": {}}),
                        json!({"child": {}, "elevation_reason": "ask"}),
                    ],
                );
            }
        }

        let message = super::SchemaError::ReferenceApplicator {
            keyword: "$ref".to_owned(),
            location: "/properties/child".to_owned(),
        }
        .to_string();
        assert!(message.contains("$ref"));
        assert!(message.contains("/properties/child"));
        assert!(message.contains("Inline"));
    }

    /// The scan descends only through positions JSON Schema defines as
    /// carrying subschemas, so a `$ref` key that is a *property name* or
    /// *annotation data* is not an applicator.
    ///
    /// Each case would be a false positive under a walk that treated every
    /// object-shaped value as a schema node — the same shape-versus-position
    /// mistake the root profile already corrected.
    #[test]
    fn the_reference_scan_reads_schema_positions_not_json_shape() {
        let cases = [
            // A business property may legally be called `$ref`.
            (
                "property name",
                json!({"type": "object", "properties": {"$ref": {"type": "string"}}}),
            ),
            // `dependentRequired` maps a property name to further required
            // property names. No subschema is involved.
            (
                "dependentRequired name",
                json!({
                    "type": "object",
                    "properties": {
                        "config": {
                            "type": "object",
                            "dependentRequired": {"$ref": ["other"]},
                        },
                    },
                }),
            ),
            // The Draft-7 `dependencies` spelling of the same thing.
            (
                "draft-7 dependencies name",
                json!({
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "type": "object",
                    "properties": {
                        "config": {
                            "type": "object",
                            "dependencies": {"$ref": ["other"]},
                        },
                    },
                }),
            ),
            // An unrecognized keyword's value is annotation data, which JSON
            // Schema explicitly does not treat as a subschema.
            (
                "unknown annotation data",
                json!({
                    "type": "object",
                    "properties": {
                        "config": {
                            "type": "object",
                            "x-vendor-metadata": {"$ref": "literal metadata"},
                        },
                    },
                }),
            ),
            // Instance data, not schemas.
            (
                "enum value",
                json!({
                    "type": "object",
                    "properties": {"mode": {"enum": [{"$ref": "not-a-schema"}]}},
                }),
            ),
            (
                "const value",
                json!({
                    "type": "object",
                    "properties": {"mode": {"const": {"$ref": "not-a-schema"}}},
                }),
            ),
            (
                "examples value",
                json!({
                    "type": "object",
                    "properties": {"mode": {"type": "string"}},
                    "examples": [{"$ref": "not-a-schema"}],
                }),
            ),
        ];
        for (label, schema) in &cases {
            validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, schema)
                .unwrap_or_else(|error| panic!("{label} is not an applicator: {error}"));
        }
    }

    /// The same positions, used as real subschema carriers, are walked: a
    /// reference hiding in any of them is still found.
    #[test]
    fn the_reference_scan_walks_every_subschema_position() {
        let reference = json!({"$ref": "#"});
        let mut cases: Vec<(String, serde_json::Value)> = Vec::new();
        for keyword in super::SUBSCHEMA_KEYWORDS {
            cases.push((
                format!("/properties/config/{keyword}"),
                json!({
                    "type": "object",
                    "properties": {"config": {*keyword: reference.clone()}},
                }),
            ));
        }
        for keyword in super::SUBSCHEMA_LIST_KEYWORDS {
            cases.push((
                format!("/properties/config/{keyword}/0"),
                json!({
                    "type": "object",
                    "properties": {"config": {*keyword: [reference.clone()]}},
                }),
            ));
        }
        for keyword in super::SCHEMA_MAP_KEYWORDS {
            cases.push((
                format!("/properties/config/{keyword}/inner"),
                json!({
                    "type": "object",
                    "properties": {"config": {*keyword: {"inner": reference.clone()}}},
                }),
            ));
        }
        // `items` in both of its legal shapes.
        cases.push((
            "/properties/config/items".to_owned(),
            json!({"type": "object", "properties": {"config": {"items": reference.clone()}}}),
        ));
        cases.push((
            "/properties/config/items/0".to_owned(),
            json!({"type": "object", "properties": {"config": {"items": [reference.clone()]}}}),
        ));
        // Draft-7 `dependencies` in its subschema shape.
        cases.push((
            "/properties/config/dependencies/inner".to_owned(),
            json!({
                "type": "object",
                "properties": {
                    "config": {"dependencies": {"inner": reference.clone()}},
                },
            }),
        ));

        for (location, schema) in &cases {
            assert_eq!(
                validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, schema),
                Err(super::SchemaError::ReferenceApplicator {
                    keyword: "$ref".to_owned(),
                    location: location.clone(),
                }),
                "a reference at {location} must be found: {schema}"
            );
        }
    }

    /// The compilation contract, stated as the projection equivalence it
    /// actually needs:
    ///
    /// ```text
    /// canonical(B) ⇔ compiled(B + top-level execution_mode)
    /// ```
    ///
    /// Decoration is a clone-and-decorate of the root only, so for any
    /// business arguments and either mode the two schemas must agree. This is
    /// what the root profile, the reserved-name check, and the reference ban
    /// exist to guarantee.
    #[test]
    fn compilation_preserves_the_projection_equivalence() {
        let schemas = [
            json!({
                "type": "object",
                "properties": {"command": {"type": "string"}, "timeout": {"type": "integer"}},
                "required": ["command"],
                "additionalProperties": false,
            }),
            json!({
                "type": "object",
                "properties": {
                    "child": {
                        "type": "object",
                        "properties": {"execution_mode": {"type": "string"}},
                        "oneOf": [{"required": ["execution_mode"]}],
                        "additionalProperties": false,
                    },
                },
                "required": ["child"],
                "additionalProperties": false,
            }),
        ];
        let candidates = [
            json!({"command": "ls"}),
            json!({"command": "ls", "timeout": 5}),
            json!({"command": 42}),
            json!({"timeout": 5}),
            json!({"child": {"execution_mode": "anything"}}),
            json!({"child": {}}),
            json!({"child": {"execution_mode": "anything", "extra": 1}}),
            json!({}),
        ];
        for schema in &schemas {
            validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, schema)
                .expect("the fixture matches the contract");
            let compiled = compile_model_definition(&definition(
                ToolExecutionPolicy::ModelSelectable,
                schema.clone(),
            ))
            .expect("compile");
            let compiled_validator = jsonschema::Validator::new(&compiled.input_schema)
                .expect("the compiled schema is valid");
            for business in &candidates {
                let canonical_verdict = validate_business_arguments(schema, business).is_ok();
                for mode in super::EXECUTION_MODE_VALUES {
                    let mut invocation = business.clone();
                    invocation
                        .as_object_mut()
                        .expect("object candidate")
                        .insert(EXECUTION_MODE_FIELD.to_owned(), json!(mode));
                    assert_eq!(
                        compiled_validator.is_valid(&invocation),
                        canonical_verdict,
                        "projection equivalence broken for {business} with {mode:?} under {schema}"
                    );
                    // And the round trip: stripping recovers exactly the
                    // business arguments the canonical schema judged.
                    let (_, _, stripped) = resolve_invocation_metadata(
                        ToolExecutionPolicy::ModelSelectable,
                        &invocation,
                    )
                    .expect("a well-formed invocation resolves");
                    assert_eq!(&stripped, business);
                }
            }
        }
    }

    /// Apart from references, nested subschemas are unrestricted: the
    /// profile governs the root instance only, so a business property may
    /// hold composition, `const`, cardinality assertions, and an
    /// `execution_mode` of its own.
    #[test]
    fn nested_subschemas_stay_unrestricted() {
        let schema = json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "properties": {"execution_mode": {"const": "whatever"}},
                    "oneOf": [{"required": ["execution_mode"]}],
                    "maxProperties": 1,
                },
            },
            "required": ["config"],
        });
        validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, &schema)
            .expect("only the root instance is constrained");
        let compiled =
            compile_model_definition(&definition(ToolExecutionPolicy::ModelSelectable, schema))
                .expect("compile");
        let validator =
            jsonschema::Validator::new(&compiled.input_schema).expect("compiled schema is valid");
        assert!(
            validator.is_valid(&json!({
                "config": {"execution_mode": "whatever"},
                "execution_mode": "background",
            })),
            "the nested property keeps its own meaning: {}",
            compiled.input_schema
        );
    }

    /// `additionalProperties` is not a composition keyword: decoration adds
    /// the selector to the same object's own `properties`, so even
    /// `"additionalProperties": false` — the shape of every ordinary native
    /// tool — stays decoratable and satisfiable.
    #[test]
    fn additional_properties_false_stays_decoratable() {
        let schema = json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
            "additionalProperties": false,
        });
        validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, &schema)
            .expect("a closed root object is decoratable");
        let compiled =
            compile_model_definition(&definition(ToolExecutionPolicy::ModelSelectable, schema))
                .expect("compile");
        let validator =
            jsonschema::Validator::new(&compiled.input_schema).expect("compiled schema is valid");
        assert!(
            validator.is_valid(&json!({"command": "ls", "execution_mode": "foreground"})),
            "the compiled schema accepts a correct call: {}",
            compiled.input_schema
        );
    }

    #[test]
    fn the_schema_contract_errors_are_actionable() {
        let collision =
            super::SchemaError::ExecutionModeCollision(super::ExecutionModeClaim::Required)
                .to_string();
        assert!(collision.contains("execution_mode"));
        assert!(collision.contains("required"));
        assert!(collision.contains("rename"));
        assert!(collision.contains("ModelSelectable"));
        let undecoratable =
            super::SchemaError::UndecoratableRootSchema("anyOf".to_owned()).to_string();
        assert!(undecoratable.contains("anyOf"));
        assert!(undecoratable.contains("properties"));
        assert!(undecoratable.contains("ModelSelectable"));
    }

    #[test]
    fn valid_foreground_selection_is_extracted_and_stripped() {
        let (mode, _, stripped) = resolve_invocation_metadata(
            ToolExecutionPolicy::ModelSelectable,
            &json!({"execution_mode": "foreground", "path": "a.txt"}),
        )
        .expect("resolve");
        assert_eq!(mode, ToolInvocationMode::Foreground);
        assert_eq!(stripped, json!({"path": "a.txt"}));
    }

    #[test]
    fn valid_background_selection_is_extracted_and_stripped() {
        let (mode, _, stripped) = resolve_invocation_metadata(
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
    fn reserved_invocation_diagnostic_never_echoes_an_authored_key() {
        let expected = "$ contains a property in the reserved \"__rustx_\" namespace. Remove all top-level __rustx_* fields; reserved invocation metadata is never forwarded to executors.";
        for suffix in ["private-field".repeat(1024), "秘密字段".repeat(1024)] {
            let arguments = json!({format!("__rustx_{suffix}"): "private-value"});
            for policy in [
                ToolExecutionPolicy::ForegroundOnly,
                ToolExecutionPolicy::BackgroundOnly,
                ToolExecutionPolicy::ModelSelectable,
            ] {
                let error = resolve_invocation_metadata(policy, &arguments).unwrap_err();
                assert!(matches!(
                    error,
                    super::SchemaError::ReservedInvocationProperty(_)
                ));
                assert_eq!(error.to_string(), expected);
                assert!(error.to_string().len() < 256);
            }
        }
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
            let (mode, _, stripped) =
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

    #[test]
    fn elevation_max_properties_does_not_accept_two_business_props() {
        let canonical = json!({
            "type": "object",
            "properties": {
                "a": {"type": "string"},
                "b": {"type": "string"},
            },
            "maxProperties": 1,
        });
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ForegroundOnly,
            canonical.clone(),
        ))
        .expect("compile");
        assert_elevation_projection_equivalence(
            &canonical,
            &compiled.input_schema,
            &[
                json!({"a": "1"}),
                json!({"a": "1", "b": "2"}),
                json!({"a": "1", "elevation_reason": "ask"}),
                json!({"a": "1", "b": "2", "elevation_reason": "ask"}),
                json!({"elevation_reason": "ask"}),
                json!({}),
            ],
        );
    }

    #[test]
    fn elevation_min_properties_rejects_metadata_only_when_business_empty_is_invalid() {
        let canonical = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}},
            "minProperties": 1,
        });
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ForegroundOnly,
            canonical.clone(),
        ))
        .expect("compile");
        assert!(
            !model_accepts_legal_elevation_call(
                &compiled.input_schema,
                &json!({"elevation_reason": "ask"})
            ),
            "metadata alone must not satisfy business minProperties"
        );
        assert_elevation_projection_equivalence(
            &canonical,
            &compiled.input_schema,
            &[
                json!({}),
                json!({"a": "1"}),
                json!({"elevation_reason": "ask"}),
                json!({"a": "1", "elevation_reason": "ask"}),
            ],
        );
    }

    #[test]
    fn elevation_root_ref_to_closed_object_accepts_metadata() {
        let canonical = json!({
            "$ref": "#/$defs/body",
            "$defs": {
                "body": {
                    "type": "object",
                    "properties": {"command": {"type": "string"}},
                    "required": ["command"],
                    "additionalProperties": false,
                }
            }
        });
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ForegroundOnly,
            canonical.clone(),
        ))
        .expect("compile");
        assert!(
            model_accepts_legal_elevation_call(
                &compiled.input_schema,
                &json!({"command": "ls", "elevation_reason": "ask"})
            ),
            "closed $ref target must admit optional elevation_reason: {}",
            compiled.input_schema
        );
        assert_elevation_projection_equivalence(
            &canonical,
            &compiled.input_schema,
            &[
                json!({"command": "ls"}),
                json!({"command": "ls", "elevation_reason": "ask"}),
                json!({"command": "ls", "extra": 1}),
                json!({"elevation_reason": "ask"}),
                json!({}),
            ],
        );
    }

    #[test]
    fn elevation_multi_layer_composition_closed_objects_all_admit_metadata() {
        let canonical = json!({
            "allOf": [
                {
                    "allOf": [
                        {
                            "type": "object",
                            "properties": {"command": {"type": "string"}},
                            "required": ["command"],
                            "additionalProperties": false,
                        }
                    ]
                }
            ]
        });
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ForegroundOnly,
            canonical.clone(),
        ))
        .expect("compile");
        assert!(
            model_accepts_legal_elevation_call(
                &compiled.input_schema,
                &json!({"command": "ls", "elevation_reason": "ask"})
            ),
            "nested composition layers must all admit elevation: {}",
            compiled.input_schema
        );
        assert_elevation_projection_equivalence(
            &canonical,
            &compiled.input_schema,
            &[
                json!({"command": "ls"}),
                json!({"command": "ls", "elevation_reason": "ask"}),
                json!({"command": "ls", "extra": 1}),
                json!({"elevation_reason": "ask"}),
            ],
        );
    }

    #[test]
    fn elevation_collision_in_composition_branch_is_refused() {
        let canonical = json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "allOf": [{
                "type": "object",
                "properties": {
                    "elevation_reason": {"type": "number"},
                },
            }],
        });
        let error =
            compile_model_definition(&definition(ToolExecutionPolicy::ForegroundOnly, canonical))
                .expect_err("business elevation_reason in a root composition branch collides");
        assert!(matches!(
            error,
            super::SchemaError::ElevationReasonCollision(_)
        ));
    }

    #[test]
    fn elevation_rewrite_skips_nested_const_ref_data() {
        let canonical = json!({
            "type": "object",
            "properties": {
                "mode": {"const": {"$ref": "#"}},
            },
            "required": ["mode"],
            "additionalProperties": false,
        });
        let compiled = compile_model_definition(&definition(
            ToolExecutionPolicy::ForegroundOnly,
            canonical.clone(),
        ))
        .expect("compile");
        assert_eq!(
            compiled.input_schema["properties"]["mode"]["const"],
            json!({"$ref": "#"}),
            "const payload must not be rewritten as a schema $ref"
        );
        assert_elevation_projection_equivalence(
            &canonical,
            &compiled.input_schema,
            &[
                json!({"mode": {"$ref": "#"}}),
                json!({"mode": {"$ref": "#"}, "elevation_reason": "ask"}),
                json!({"mode": {"$ref": "#/$defs/rustx_business"}}),
                json!({"elevation_reason": "ask"}),
            ],
        );
    }

    #[test]
    fn elevation_reason_enforces_utf8_byte_limit() {
        let canonical = json!({"type": "object", "properties": {"path": {"type": "string"}}});
        let compiled =
            compile_model_definition(&definition(ToolExecutionPolicy::ForegroundOnly, canonical))
                .expect("compile");
        let field = &compiled.input_schema["properties"][ELEVATION_REASON_FIELD];
        assert_eq!(
            field["maxLength"],
            json!(ELEVATION_REASON_MAX_UTF8_BYTES),
            "model schema must advertise the length bound"
        );

        // 171 CJK codepoints × 3 bytes = 513 UTF-8 bytes.
        let overlong = "字".repeat(171);
        assert_eq!(overlong.len(), ELEVATION_REASON_MAX_UTF8_BYTES + 1);
        assert!(
            !elevation_metadata_is_legal(&json!({"elevation_reason": overlong.clone()})),
            "513 UTF-8 bytes is not legal elevation metadata"
        );
        let error = resolve_invocation_metadata(
            ToolExecutionPolicy::ForegroundOnly,
            &json!({"path": "a", "elevation_reason": overlong}),
        )
        .expect_err("runtime rejects overlong elevation_reason");
        assert!(matches!(
            error,
            super::SchemaError::InvalidElevationReason(_)
        ));
        let message = error.to_string();
        assert!(
            message.contains("UTF-8 bytes"),
            "diagnostic must say bytes, not characters: {message}"
        );
        assert!(!message.contains("characters"));
    }
}
