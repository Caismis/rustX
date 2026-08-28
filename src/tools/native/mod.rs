//! The native tool plane: runtime-intrinsic and native tools implemented as
//! normal [`ToolDefinition`] + [`ToolExecutor`] registrations.
//!
//! Read, Write, Edit, Glob, Grep, and Bash are all ordinary registrations;
//! their executor implementations and their execution-ownership and approval
//! policies are independent, so each tool is configured with its own
//! [`ToolInvocationPolicy`] (`ForegroundOnly`, `BackgroundOnly`, or
//! `ModelSelectable` execution, `Sequential`/`Parallel` concurrency, and
//! `Never`/`Always` approval through the concrete bounded
//! [`NativeToolPolicies`] configuration. The only intentionally fixed
//! policy is the runtime intrinsic `background_task` (foreground-only,
//! sequential, approval-never), and `ask_user` and `todo` are likewise
//! fixed to foreground-only, sequential, approval-never: one is the native
//! Questionnaire capability itself, and the other mutates conversation-owned
//! task state that two concurrent calls would race on.
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
//! object of a definition, executor, and optional tool-owned normalizer. This
//! module only *composes* the
//! known native tools — the composition is explicit and deterministic, with
//! no discovery, no plugin loading, and no generic tool factory.
//!
//! The registration object is an implementation detail: the public tool-plane
//! API stays [`ToolDefinition`], [`ToolExecutor`], [`ToolRegistry`], and
//! [`ToolExecutionResult`].
//!
//! [`ToolRegistry`]: crate::tools::executor::ToolRegistry
//! [`ToolExecutionResult`]: crate::tools::types::ToolExecutionResult
//!
//! [`ToolDefinition`]: crate::tools::types::ToolDefinition
//! [`ToolExecutor`]: crate::tools::executor::ToolExecutor

mod ask_user;
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
mod subagent;
mod support;
mod todo;
mod write;

#[cfg(test)]
pub(crate) use bash::{BashTestControl, BashTool};
#[cfg(test)]
pub(crate) use grep::GrepTool;
#[cfg(test)]
pub(crate) use read::ReadTool;

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
use crate::tools::types::{ToolCall, ToolInvocationPolicy};

/// The normalized file-operation fact of one native file tool call recorded
/// in canonical conversation history (Issue #140).
///
/// This is the rustX-owned extraction boundary between the native file tools
/// and context compaction: the tool modules own the decoding that identifies
/// the path, and compaction aggregates these normalized facts without
/// learning tool argument formats. Classification is by the canonical
/// [`ToolCall::tool_id`], never by the model-facing name, so an unrelated
/// foreign tool named `read` can never contribute a file fact. The fact is
/// the call itself: whether the call later failed, was denied, or was
/// cancelled does not rewrite what the retired conversation asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeFileOperation {
    /// A native `read(path)` call.
    Read {
        /// The path argument of the call.
        path: String,
    },
    /// A native `edit(path)` or `write(path)` call.
    Modified {
        /// The path argument of the call.
        path: String,
    },
}

/// Classifies one canonical tool call as a native file operation, decoding
/// the path through the owning tool module.
///
/// Returns `None` for any non-file native tool, any non-native tool, and any
/// native file call whose recorded arguments do not identify a path.
pub(crate) fn native_file_operation(call: &ToolCall) -> Option<NativeFileOperation> {
    let tool_id = call.tool_id.as_str();
    if tool_id == read::TOOL_ID {
        read::operation_path(&call.arguments).map(|path| NativeFileOperation::Read { path })
    } else if tool_id == edit::TOOL_ID {
        edit::operation_path(&call.arguments).map(|path| NativeFileOperation::Modified { path })
    } else if tool_id == write::TOOL_ID {
        write::operation_path(&call.arguments).map(|path| NativeFileOperation::Modified { path })
    } else {
        None
    }
}

/// The conversation-owned resources native tools need beyond their
/// execution context.
#[derive(Clone)]
pub struct NativeToolResources {
    /// The conversation background registry used by the `background_task`
    /// intrinsic.
    pub background: ConversationBackgroundRegistry,
    /// The conversation subagent registry used by the `subagent` intrinsic
    /// (Issue #60). `None` — for example inside a subagent child itself —
    /// means the intrinsic is not registered at all, so recursive
    /// delegation is absent by construction.
    pub subagents: Option<crate::runtime::subagent::SubagentRegistry>,
}

/// The concrete, bounded per-tool policy configuration of the six ordinary
/// configurable native tools. `ask_user`, `background_task`, and `todo` own
/// fixed policies and are not configurable through this table.
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
    // The explicit composition of the native tool plane: every entry is a
    // tool-owned registration, and this list is the only place that knows
    // which native capabilities exist.
    for registration in native_tool_registrations(resources, policies) {
        let NativeToolRegistration {
            definition,
            executor,
            normalizer,
            mandatory,
        } = registration;
        registry.register_with_activation_metadata(definition, executor, normalizer, mandatory)?;
    }
    Ok(())
}

/// Builds every native Tool registration without activating any of them.
///
/// The capability coordinator uses this internal seam to keep all native
/// tools available while applying the current startup activation policy.
pub(crate) fn native_tool_registrations(
    resources: NativeToolResources,
    policies: NativeToolPolicies,
) -> Vec<NativeToolRegistration> {
    let NativeToolResources {
        background,
        subagents,
    } = resources;
    let mut registrations = vec![
        background_task::registration(background),
        ask_user::registration(),
        read::registration(policies.read),
        write::registration(policies.write),
        edit::registration(policies.edit),
        glob::registration(policies.glob),
        grep::registration(policies.grep),
        bash::registration(policies.bash),
        todo::registration(),
    ];
    // The `subagent` intrinsic exists only in a runtime that owns a
    // subagent registry (never inside a child runtime).
    if let Some(subagents) = subagents {
        registrations.push(subagent::registration(subagents));
    }
    registrations
}

/// Registers exactly the deny-by-construction capability set of a subagent
/// child (Issue #60).
///
/// The v1 `explore` profile is read-only: the child's `ToolRegistry`
/// contains `Read`, `Glob`, and `Grep` and nothing else. There is no
/// `subagent` tool in the child, so recursive delegation is structurally
/// absent; there is no Write/Edit/Bash, no background execution, and no
/// MCP/Python capability in the child composition path at all.
///
/// # Errors
///
/// Returns the specific [`ToolRegistryError`] of the first registration
/// violation.
pub fn register_subagent_child_tools(
    registry: &mut ToolRegistry,
    policies: NativeToolPolicies,
) -> Result<(), ToolRegistryError> {
    let registrations = [
        read::registration(policies.read),
        glob::registration(policies.glob),
        grep::registration(policies.grep),
    ];
    for NativeToolRegistration {
        definition,
        executor,
        normalizer,
        mandatory,
    } in registrations
    {
        registry.register_with_activation_metadata(definition, executor, normalizer, mandatory)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{NativeToolPolicies, register_subagent_child_tools};
    use crate::tools::executor::ToolRegistry;

    #[test]
    fn subagent_child_registry_is_exactly_the_explore_profile() {
        let mut registry = ToolRegistry::new();
        register_subagent_child_tools(&mut registry, NativeToolPolicies::default())
            .expect("explore tools register");
        assert_eq!(registry.names(), vec!["read", "glob", "grep"]);
        assert_eq!(registry.len(), 3);
        assert!(
            registry
                .registrations()
                .iter()
                .find(|registration| registration.definition.name == "read")
                .is_some_and(|registration| registration.mandatory),
            "native Read is marked mandatory by the native composition"
        );
        assert!(
            registry
                .definitions()
                .iter()
                .all(|definition| definition.origin == crate::tools::types::ToolOrigin::Builtin)
        );
    }
}
