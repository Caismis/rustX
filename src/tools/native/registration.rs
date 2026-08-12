//! The bounded registration contract of the native tool plane.
//!
//! Every native capability owns one module boundary and constructs itself:
//! a tool module exposes exactly one `registration(...)` function returning
//! the [`NativeToolRegistration`] pair of its canonical [`ToolDefinition`]
//! and its [`ToolExecutor`]. [`register_native_tools`] only composes the
//! known native tools — there is no discovery, no factory, no plugin
//! loading, and no registration macro.
//!
//! [`register_native_tools`]: super::register_native_tools

use std::sync::Arc;

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

/// Builds a canonical native tool definition under the composed policy.
///
/// Native tools are always builtin-origin and never replayable; the two
/// policy axes come from the composed [`ToolInvocationPolicy`].
pub(crate) fn native_definition(
    id: &str,
    name: &str,
    description: &str,
    schema: serde_json::Value,
    policy: ToolInvocationPolicy,
) -> ToolDefinition {
    ToolDefinition {
        id: ToolId::new(id),
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema: schema,
        execution_policy: policy.execution,
        concurrency_policy: policy.concurrency,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}
