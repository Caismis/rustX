//! The native tool plane: runtime-intrinsic and native tools implemented as
//! normal [`ToolDefinition`] + [`ToolExecutor`] registrations.
//!
//! Read, Write, Edit, Glob, Grep, and Bash are all ordinary registrations;
//! their executor implementations and their execution-ownership policies are
//! independent, so each tool is configured with its own
//! [`ToolInvocationPolicy`] (`ForegroundOnly`, `BackgroundOnly`, or
//! `ModelSelectable`) through the concrete bounded
//! [`NativeToolPolicies`] configuration. The only intentionally fixed
//! policy is the runtime intrinsic `background_task` (foreground-only,
//! sequential), enforced by the registry itself.
//!
//! The default is foreground-only sequential for every ordinary native
//! tool: the model-facing surface of the native tool plane is conservative
//! by default, and `ModelSelectable`/`BackgroundOnly` are explicit
//! per-tool configuration choices.
//!
//! [`ToolDefinition`]: crate::tools::types::ToolDefinition
//! [`ToolExecutor`]: crate::tools::executor::ToolExecutor

mod background_task;
mod bash;
// The supervisor process entry points are reachable only from main.rs
// (supervisor-mode dispatch) and from test binaries via self-exec; they are
// documented-hidden binary entry points, never tool-plane API.
#[cfg(unix)]
#[doc(hidden)]
pub mod bash_supervisor;
mod edit;
mod glob;
mod grep;
mod read;
mod support;
mod write;

use std::sync::Arc;

use crate::tools::background::ConversationBackgroundRegistry;
use crate::tools::executor::{ToolExecutor, ToolRegistry, ToolRegistryError};
use crate::tools::types::{ToolDefinition, ToolInvocationPolicy, ToolOrigin, ToolReplayPolicy};

/// The conversation-owned resources native tools need beyond their
/// execution context.
#[derive(Clone)]
pub struct NativeToolResources {
    /// The conversation background registry used by the `background_task`
    /// intrinsic.
    pub background: ConversationBackgroundRegistry,
}

/// The concrete, bounded per-tool execution-policy configuration of the six
/// ordinary native tools.
///
/// Execution policy belongs to the registered tool definition, not to the
/// native tool plane as a whole: each ordinary native tool independently
/// selects its execution and concurrency policy. This deliberately models
/// only the six known M5 tools — no generic policy maps, plugin
/// configuration frameworks, strategy traits, factories, or global
/// configuration registries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeToolPolicies {
    /// The policy of the native Read tool.
    pub read: ToolInvocationPolicy,
    /// The policy of the native Write tool.
    pub write: ToolInvocationPolicy,
    /// The policy of the native Edit tool.
    pub edit: ToolInvocationPolicy,
    /// The policy of the native Glob tool.
    pub glob: ToolInvocationPolicy,
    /// The policy of the native Grep tool.
    pub grep: ToolInvocationPolicy,
    /// The policy of the native Bash tool.
    pub bash: ToolInvocationPolicy,
}

impl Default for NativeToolPolicies {
    fn default() -> Self {
        Self::uniform(ToolInvocationPolicy::default())
    }
}

impl NativeToolPolicies {
    /// Applies one policy to every ordinary native tool.
    #[must_use]
    pub const fn uniform(policy: ToolInvocationPolicy) -> Self {
        Self {
            read: policy,
            write: policy,
            edit: policy,
            glob: policy,
            grep: policy,
            bash: policy,
        }
    }
}

/// Registers every native tool with the registry under its configured
/// per-tool policy.
///
/// Each ordinary native tool definition receives exactly its own policy:
/// `read` from `policies.read`, `write` from `policies.write`, `edit` from
/// `policies.edit`, `glob` from `policies.glob`, `grep` from
/// `policies.grep`, and `bash` from `policies.bash`. The runtime intrinsic
/// `background_task` is intentionally outside this configurable set and
/// stays fixed to foreground-only sequential execution, which the registry
/// enforces regardless of the configured policies.
///
/// # Errors
///
/// Returns the specific [`ToolRegistryError`] of the first registration
/// violation; the fixed intrinsic policy of `background_task` is enforced by
/// the registry itself.
pub fn register_native_tools(
    registry: &mut ToolRegistry,
    resources: NativeToolResources,
    policies: NativeToolPolicies,
) -> Result<(), ToolRegistryError> {
    let NativeToolResources { background } = resources;
    let definitions = [
        (
            background_task::definition(),
            Arc::new(background_task::BackgroundTaskExecutor::new(background))
                as Arc<dyn ToolExecutor>,
        ),
        (
            read::definition(policies.read),
            Arc::new(read::ReadTool) as Arc<dyn ToolExecutor>,
        ),
        (
            write::definition(policies.write),
            Arc::new(write::WriteTool) as Arc<dyn ToolExecutor>,
        ),
        (
            edit::definition(policies.edit),
            Arc::new(edit::EditTool) as Arc<dyn ToolExecutor>,
        ),
        (
            glob::definition(policies.glob),
            Arc::new(glob::GlobTool) as Arc<dyn ToolExecutor>,
        ),
        (
            grep::definition(policies.grep),
            Arc::new(grep::GrepTool) as Arc<dyn ToolExecutor>,
        ),
        (
            bash::definition(policies.bash),
            Arc::new(bash::BashTool::new()) as Arc<dyn ToolExecutor>,
        ),
    ];
    for (definition, executor) in definitions {
        registry.register(definition, executor)?;
    }
    Ok(())
}

/// Builds a canonical native tool definition under the configured policy.
fn native_definition(
    id: &str,
    name: &str,
    description: &str,
    schema: serde_json::Value,
    policy: ToolInvocationPolicy,
) -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new(id),
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema: schema,
        execution_policy: policy.execution,
        concurrency_policy: policy.concurrency,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}
