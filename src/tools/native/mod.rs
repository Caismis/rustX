//! The native tool plane: runtime-intrinsic and native tools implemented as
//! normal [`ToolDefinition`] + [`ToolExecutor`] registrations.
//!
//! Read, Write, Edit, Glob, Grep, and Bash are all ordinary registrations;
//! their definitions may choose any execution policy through normal
//! registration/configuration. The only intentionally fixed policy is the
//! runtime intrinsic `background_task` (foreground-only, sequential).
//!
//! [`ToolDefinition`]: crate::tools::types::ToolDefinition
//! [`ToolExecutor`]: crate::tools::executor::ToolExecutor

mod background_task;
mod bash;
mod edit;
mod glob;
mod grep;
mod read;
mod support;
mod write;

use std::sync::Arc;

use crate::tools::background::ConversationBackgroundRegistry;
use crate::tools::executor::{ToolExecutor, ToolRegistry, ToolRegistryError};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
};

/// The conversation-owned resources native tools need beyond their
/// execution context.
#[derive(Clone)]
pub struct NativeToolResources {
    /// The conversation background registry used by the `background_task`
    /// intrinsic.
    pub background: ConversationBackgroundRegistry,
}

/// Registers every native tool with the registry.
///
/// # Errors
///
/// Returns the specific [`ToolRegistryError`] of the first registration
/// violation; the fixed intrinsic policy of `background_task` is enforced by
/// the registry itself.
pub fn register_native_tools(
    registry: &mut ToolRegistry,
    resources: NativeToolResources,
) -> Result<(), ToolRegistryError> {
    let NativeToolResources { background } = resources;
    let definitions = [
        (
            background_task::definition(),
            Arc::new(background_task::BackgroundTaskExecutor::new(background))
                as Arc<dyn ToolExecutor>,
        ),
        (
            read::definition(),
            Arc::new(read::ReadTool) as Arc<dyn ToolExecutor>,
        ),
        (
            write::definition(),
            Arc::new(write::WriteTool) as Arc<dyn ToolExecutor>,
        ),
        (
            edit::definition(),
            Arc::new(edit::EditTool) as Arc<dyn ToolExecutor>,
        ),
        (
            glob::definition(),
            Arc::new(glob::GlobTool) as Arc<dyn ToolExecutor>,
        ),
        (
            grep::definition(),
            Arc::new(grep::GrepTool) as Arc<dyn ToolExecutor>,
        ),
        (
            bash::definition(),
            Arc::new(bash::BashTool) as Arc<dyn ToolExecutor>,
        ),
    ];
    for (definition, executor) in definitions {
        registry.register(definition, executor)?;
    }
    Ok(())
}

/// The canonical native tool policies: foreground-only sequential by
/// default; `ModelSelectable` is a legal configuration choice for every
/// native tool.
const NATIVE_EXECUTION: ToolExecutionPolicy = ToolExecutionPolicy::ModelSelectable;
const NATIVE_CONCURRENCY: ToolConcurrencyPolicy = ToolConcurrencyPolicy::Sequential;

/// Builds a canonical native tool definition.
fn native_definition(
    id: &str,
    name: &str,
    description: &str,
    schema: serde_json::Value,
) -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new(id),
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema: schema,
        execution_policy: NATIVE_EXECUTION,
        concurrency_policy: NATIVE_CONCURRENCY,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}
