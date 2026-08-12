//! The bounded registration contract of the native tool plane.
//!
//! Every native capability owns one module boundary and constructs itself:
//! a tool module exposes exactly one `registration(...)` function returning
//! the [`NativeToolRegistration`] pair of its canonical [`ToolDefinition`]
//! and its [`ToolExecutor`]. [`register_native_tools`] only composes the
//! known native tools — there is no discovery, no factory, no plugin
//! loading, and no registration macro.
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

use schemars::JsonSchema;
use schemars::generate::{SchemaGenerator, SchemaSettings};

use crate::runtime::identity::ToolId;
use crate::tools::executor::ToolExecutor;
use crate::tools::types::{ToolDefinition, ToolInvocationPolicy, ToolOrigin, ToolReplayPolicy};

/// One fully constructed native tool: its canonical definition and the
/// executor that serves it.
///
/// The pair is what the native tool plane hands to the registry; the
/// registry keeps owning registration validation and the
/// definition/executor relationship.
pub struct NativeToolRegistration {
    /// The canonical tool-owned definition (identity, description, input
    /// schema, policies).
    pub definition: ToolDefinition,
    /// The executor serving that definition.
    pub executor: Arc<dyn ToolExecutor>,
}

impl NativeToolRegistration {
    /// Pairs one canonical definition with its executor.
    #[must_use]
    pub fn new(definition: ToolDefinition, executor: Arc<dyn ToolExecutor>) -> Self {
        Self {
            definition,
            executor,
        }
    }
}

/// Builds a canonical native tool definition whose input schema is
/// generated from the tool's typed input contract `I`.
///
/// Native tools are always builtin-origin and never replayable; the two
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
/// # Panics
///
/// Panics only when a native input contract generates a non-object root
/// schema, which cannot happen for the derived struct contracts.
pub(crate) fn input_schema<I: JsonSchema>() -> serde_json::Value {
    let mut settings = SchemaSettings::draft2020_12();
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
