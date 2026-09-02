//! The bounded registration contract of the native tool plane.
//!
//! Every native capability owns one module boundary and constructs itself:
//! a tool module exposes exactly one `registration(...)` function returning
//! the [`NativeToolRegistration`] object of its canonical [`ToolDefinition`],
//! [`ToolExecutor`], and optional business-argument normalizer.
//! [`register_native_tools`] only composes the
//! known native tools — there is no discovery, no factory, no plugin
//! loading, and no registration macro. The registration object is internal to
//! the native plane; the public tool-plane API stays [`ToolDefinition`],
//! [`ToolExecutor`], [`ToolRegistry`], and [`ToolExecutionResult`].
//!
//! [`ToolRegistry`]: crate::tools::executor::ToolRegistry
//! [`ToolExecutionResult`]: crate::tools::types::ToolExecutionResult
//!
//! A native tool's canonical input schema is *generated* from its typed
//! input contract instead of being handwritten JSON: the Rust input type is
//! the single source of truth for the model-facing argument contract, and
//! the generated schema is exactly what the registry validates invocations
//! against before dispatch.
//!
//! ```text
//! Rust input type -> generated schema -> ToolDefinition
//! ```
//!
//! MCP and Python tools keep supplying their schema externally; both paths
//! converge on the same [`ToolDefinition`] at the registry boundary.
//!
//! [`register_native_tools`]: super::register_native_tools

use std::sync::Arc;

use schemars::generate::{SchemaGenerator, SchemaSettings};
use schemars::transform::{Transform, transform_subschemas};
use schemars::{JsonSchema, Schema};

use crate::runtime::identity::ToolId;
use crate::tools::executor::{BusinessArgumentNormalizer, ToolExecutor};
use crate::tools::types::{ToolDefinition, ToolInvocationPolicy, ToolOrigin, ToolReplayPolicy};

/// One fully constructed native tool: its canonical definition, executor,
/// and optional business-argument normalizer.
///
/// This is the internal composition object of the native tool plane — a
/// native tool module builds one, and [`register_native_tools`] consumes
/// it. It is deliberately not part of the public tool-plane API: external
/// consumers depend on [`ToolDefinition`], [`ToolExecutor`],
/// [`ToolRegistry`], and [`ToolExecutionResult`], and the registry keeps
/// owning registration validation and the definition/executor
/// relationship and preflight normalization boundary.
///
/// [`ToolRegistry`]: crate::tools::executor::ToolRegistry
/// [`ToolExecutionResult`]: crate::tools::types::ToolExecutionResult
pub(crate) struct NativeToolRegistration {
    /// The canonical tool-owned definition (identity, description, input
    /// schema, policies).
    pub(crate) definition: ToolDefinition,
    /// The executor serving that definition.
    pub(crate) executor: Arc<dyn ToolExecutor>,
    /// Tool-owned normalization applied after runtime metadata is stripped
    /// and before the canonical schema is validated.
    pub(crate) normalizer: BusinessArgumentNormalizer,
    /// Whether this native registration is mandatory in a normal agent
    /// composition. Only the native Read module opts into this marker.
    pub(crate) mandatory: bool,
}

impl NativeToolRegistration {
    /// Pairs one canonical definition with its executor and identity
    /// normalization.
    pub(super) fn new(definition: ToolDefinition, executor: Arc<dyn ToolExecutor>) -> Self {
        Self {
            definition,
            executor,
            normalizer: identity_arguments,
            mandatory: false,
        }
    }

    /// Marks this native registration as mandatory for normal agent
    /// activation. The marker is internal activation metadata and never
    /// reaches the model-facing `ToolDefinition`.
    pub(super) fn mandatory(mut self) -> Self {
        self.mandatory = true;
        self
    }

    /// Attaches one tool-owned business-argument normalizer.
    pub(super) fn with_normalizer(mut self, normalizer: BusinessArgumentNormalizer) -> Self {
        self.normalizer = normalizer;
        self
    }
}

// The registry seam deliberately gives identity normalization the same fallible
// shape as tool-owned normalizers so every registered tool follows one path.
#[allow(clippy::unnecessary_wraps)]
fn identity_arguments(arguments: &serde_json::Value) -> Result<serde_json::Value, String> {
    Ok(arguments.clone())
}

/// Builds a canonical native tool definition whose input schema is
/// generated from the tool's typed input contract `I`.
///
/// Native tools are always builtin-origin and never replayable; the three
/// policy axes come from the composed [`ToolInvocationPolicy`].
pub(crate) fn native_definition<I: JsonSchema>(
    id: &str,
    name: &str,
    description: &str,
    policy: ToolInvocationPolicy,
) -> ToolDefinition {
    ToolDefinition {
        id: ToolId::new(id),
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema: input_schema::<I>(),
        execution_policy: policy.execution,
        concurrency_policy: policy.concurrency,
        approval_policy: policy.approval,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

/// Generates the canonical model-facing JSON Schema of one native tool
/// input contract.
///
/// The generated document is a self-contained root object schema:
/// subschemas are inlined, so no `$ref`/`$defs` indirection reaches the
/// provider surface, and the meta-schema declaration plus the generated
/// root title/description are dropped because a tool's identity and purpose
/// are carried by the [`ToolDefinition`], never by the schema document.
/// Per-property documentation is kept: it is the model-facing description
/// of each argument.
///
/// An optional field means the property may be *absent*
/// ([`OptionalIsAbsentNotNull`]); it does not implicitly accept `null`.
///
/// # Panics
///
/// Panics only when a native input contract generates a non-object root
/// schema, which cannot happen for the derived struct contracts.
pub(crate) fn input_schema<I: JsonSchema>() -> serde_json::Value {
    let mut settings = SchemaSettings::draft2020_12().with_transform(OptionalIsAbsentNotNull);
    settings.inline_subschemas = true;
    settings.meta_schema = None;
    let mut schema = SchemaGenerator::new(settings)
        .into_root_schema_for::<I>()
        .to_value();
    let object = schema
        .as_object_mut()
        .expect("native tool input contracts generate root object schemas");
    object.remove("title");
    object.remove("description");
    schema
}

/// The native input-contract rule that an optional property means an
/// *absent* property.
///
/// A Rust `Option<T>` field is excluded from `required` — that is the
/// optionality the model contract needs — but the schema generator also
/// widens the property's type union to `["T", "null"]` by default. That
/// would give omission two model-facing spellings (absent and `null`) for
/// one meaning, so the union is collapsed back to the inner type here, at
/// the one boundary that generates native input schemas.
///
/// The rule is deliberately narrow: only a type *union* that still has a
/// non-null member loses its `"null"` entry. A schema whose only type is
/// `null`, and an explicit `anyOf`/`const` alternative, are left untouched,
/// so a tool that ever needs `null` as a meaningful business value can
/// still express it explicitly in its own input contract.
///
/// Collapsing the union alone is not the whole rule, because the generator
/// widens an optional property in two further places that would otherwise
/// contradict the collapsed type:
///
/// - an `Option<SomeEnum>` also gains a `null` **enum member**, leaving a
///   `{"type": "string", "enum": [..., null]}` schema no instance can
///   satisfy through that member; and
/// - a defaulted optional gains `"default": null`, which advertises `null`
///   as the property's resting value when the contract's resting value is
///   the property's absence.
///
/// Both are removed under exactly the same condition as the union collapse —
/// a property that was widened for optionality — so a contract that means
/// `null` on its own terms keeps saying so.
#[derive(Debug, Clone, Copy)]
struct OptionalIsAbsentNotNull;

impl Transform for OptionalIsAbsentNotNull {
    fn transform(&mut self, schema: &mut Schema) {
        let collapsed = schema
            .get_mut("type")
            .and_then(serde_json::Value::as_array_mut)
            .filter(|types| types.iter().any(|entry| entry != "null"))
            .map(|types| {
                types.retain(|entry| entry != "null");
                types.clone()
            });
        if let Some(mut types) = collapsed {
            let value = if types.len() == 1 {
                types.remove(0)
            } else {
                serde_json::Value::Array(types)
            };
            schema.insert("type".to_owned(), value);
            // The property was widened for optionality; remove the two
            // residues of that widening that now contradict the collapsed
            // type.
            if let Some(members) = schema
                .get_mut("enum")
                .and_then(serde_json::Value::as_array_mut)
                && members.iter().any(serde_json::Value::is_null)
                && members.iter().any(|member| !member.is_null())
            {
                members.retain(|member| !member.is_null());
            }
            if schema
                .get("default")
                .is_some_and(serde_json::Value::is_null)
            {
                schema.remove("default");
            }
        }
        transform_subschemas(self, schema);
    }
}
