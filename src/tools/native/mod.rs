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
//! # Module ownership
//!
//! One native capability owns one module boundary: a tool module owns its
//! name, its description, its input contract, its executor, and its
//! tool-private helpers, and constructs itself through its own
//! `registration(...)` function returning the plane-internal registration
//! pair of a definition and its executor. This module only *composes* the
//! known native tools — the composition is explicit and deterministic, with
//! no discovery, no plugin loading, and no generic tool factory.
//!
//! The registration pair is an implementation detail: the public tool-plane
//! API stays [`ToolDefinition`], [`ToolExecutor`], [`ToolRegistry`], and
//! [`ToolExecutionResult`].
//!
//! [`ToolRegistry`]: crate::tools::executor::ToolRegistry
//! [`ToolExecutionResult`]: crate::tools::types::ToolExecutionResult
//!
//! [`ToolDefinition`]: crate::tools::types::ToolDefinition
//! [`ToolExecutor`]: crate::tools::executor::ToolExecutor

mod background_task;
mod bash;
mod edit;
mod glob;
mod grep;
mod input;
mod read;
mod registration;
// The private native-search substrate shared by Glob and Grep. It is not a
// tool: it is never registered, never reaches the model, and exists only
// because Glob and Grep must observe one filesystem universe.
mod search;
mod support;
mod write;

#[cfg(test)]
pub(crate) use bash::{BashTestControl, BashTool};

use registration::NativeToolRegistration;

// The per-invocation Bash supervisor process entry points are reachable
// only from the supervisor binary and from test binaries via self-exec;
// they are documented-hidden binary entry points, never tool-plane API. The
// supervisor is an implementation detail of Bash execution ownership, so it
// is owned by the Bash module and only re-exported here under its binary
// entry-point path.
#[cfg(unix)]
#[doc(hidden)]
pub use bash::supervisor as bash_supervisor;

use crate::tools::background::ConversationBackgroundRegistry;
use crate::tools::executor::{ToolRegistry, ToolRegistryError};
use crate::tools::types::ToolInvocationPolicy;

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
    // The explicit composition of the native tool plane: every entry is a
    // tool-owned registration, and this list is the only place that knows
    // which native capabilities exist.
    let registrations = [
        background_task::registration(background),
        read::registration(policies.read),
        write::registration(policies.write),
        edit::registration(policies.edit),
        glob::registration(policies.glob),
        grep::registration(policies.grep),
        bash::registration(policies.bash),
    ];
    for NativeToolRegistration {
        definition,
        executor,
    } in registrations
    {
        registry.register(definition, executor)?;
    }
    Ok(())
}
