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
//! policy is the runtime intrinsic `execution` (foreground-only,
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
mod bash;
mod edit;
pub(crate) mod execution;
mod glob;
mod grep;
mod input;
mod read;
mod registration;
// The private native-search substrate shared by Glob and Grep. It is not a
// tool: it is never registered, never reaches the model, and exists only
// because Glob and Grep must observe one filesystem universe.
mod search;
pub(crate) mod subagent;
mod support;
mod todo;
mod workflow;
mod write;

#[cfg(test)]
pub(crate) use bash::{BashTestControl, BashTool};
#[cfg(test)]
pub(crate) use grep::GrepTool;
#[cfg(test)]
pub(crate) use read::ReadTool;

use registration::NativeToolRegistration;

pub use subagent::SUBAGENT_TOOL_NAME;

// The per-invocation Bash supervisor process entry points are reachable
// only from the supervisor binary and from test binaries via self-exec;
// they are documented-hidden binary entry points, never tool-plane API. The
// supervisor is an implementation detail of Bash execution ownership, so it
// is owned by the Bash module and only re-exported here under its binary
// entry-point path.
#[cfg(unix)]
#[doc(hidden)]
pub use bash::supervisor as bash_supervisor;

use crate::runtime::workflow::{WorkflowCatalog, WorkflowRuntime};
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
    /// The conversation background registry used by the `execution`
    /// intrinsic.
    pub background: ConversationBackgroundRegistry,
    /// The conversation subagent registry used by the `subagent` intrinsic
    /// (Issue #60). Registration also requires a non-empty catalog below;
    /// `None` — for example inside a subagent child itself — means the
    /// intrinsic is not registered at all, so recursive delegation is absent
    /// by construction.
    pub subagents: Option<crate::runtime::subagent::SubagentRegistry>,
    /// The named-subagent catalog of the resource generation this
    /// registration set belongs to (Issue #144).
    ///
    /// It controls whether the intrinsic exists and, when non-empty, supplies
    /// its model-facing routing description. The description a generation
    /// publishes therefore always names exactly the agents that generation
    /// admits, and an empty generation publishes no unsatisfiable Tool.
    pub subagent_catalog: crate::runtime::subagent::SubagentCatalog,
}

/// The concrete, bounded per-tool policy configuration of the six ordinary
/// configurable native tools. `ask_user`, `execution`, and `todo` own
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
/// `execution` is intentionally outside this configurable set and
/// stays fixed to foreground-only sequential execution, which the registry
/// enforces regardless of the configured policies.
///
/// # Errors
///
/// Returns the specific [`ToolRegistryError`] of the first registration
/// violation; the fixed intrinsic policy of `execution` is enforced by
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

/// Registers the concrete model-facing Workflow Tools of one immutable
/// catalog generation.
///
/// Workflow admission is intentionally a separate composition step from the
/// ordinary native plane: the catalog has already compiled and validated the
/// programs, and each registration captures one immutable program snapshot.
/// A reload therefore builds a new set off-side and a running invocation can
/// never observe a later catalog generation.
pub(crate) fn register_workflow_tools(
    registry: &mut ToolRegistry,
    runtime: &WorkflowRuntime,
    catalog: &WorkflowCatalog,
) -> Result<(), ToolRegistryError> {
    for registration in workflow::registrations(runtime, catalog) {
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
        subagent_catalog,
    } = resources;
    let mut registrations = vec![
        execution::registration(background, subagents.clone()),
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
    // subagent registry (never inside a child runtime) and whose frozen
    // resource generation admits at least one named agent. An empty catalog
    // has no satisfiable invocation, so publishing a Tool definition for it
    // would make the model-facing capability set untruthful.
    if let Some(subagents) = subagents
        && let Some(registration) = subagent::registration(subagents, &subagent_catalog)
    {
        registrations.push(registration);
    }
    registrations
}

/// The one explicit native-name -> implementation composition of the
/// subagent child plane (Issue #144).
///
/// It is a bounded `match` over the native capabilities a child may run —
/// deliberately not a factory, plugin loader, strategy registry, or
/// reflective lookup. Three capabilities are structurally absent from it and
/// therefore unregistrable in a child however a definition was written:
/// `subagent` (recursive delegation), and `ask_user` and `execution`
/// (a headless child holds no Runtime Client questionnaire authority and no
/// conversation-owned detached execution plane of its own).
///
/// The requested `policy` is the parent-frozen one, so the returned
/// registration's definition is built from the frozen policy rather than
/// from a default table. `todo` owns a fixed policy of its own; a frozen
/// definition that disagrees with it simply fails the identity check below
/// instead of being quietly rewritten.
fn subagent_child_registration(
    name: &str,
    policy: ToolInvocationPolicy,
) -> Option<NativeToolRegistration> {
    Some(match name {
        "read" => read::registration(policy),
        "write" => write::registration(policy),
        "edit" => edit::registration(policy),
        "glob" => glob::registration(policy),
        "grep" => grep::registration(policy),
        "bash" => bash::registration(policy),
        "todo" => todo::registration(),
        _ => return None,
    })
}

/// The canonical child-plane definition of one native capability under one
/// invocation policy, for in-crate tests that need to build a frozen
/// specification the child plane can actually materialize.
#[cfg(test)]
#[must_use]
pub(crate) fn subagent_child_definition(
    name: &str,
    policy: ToolInvocationPolicy,
) -> Option<crate::tools::types::ToolDefinition> {
    subagent_child_registration(name, policy).map(|registration| registration.definition)
}

/// Registers exactly the Builtin capability set one named subagent
/// definition resolved to (Issue #144).
///
/// # The definition is the contract, not the name
///
/// `frozen` is the sequence of exact [`ToolDefinition`]s the invoking
/// generation admitted and the parent resolution froze. Each one is
/// registered **verbatim**: its identity, description, input schema, replay
/// policy, origin, and all three invocation-policy axes are the ones the
/// parent authorized, never a locally rebuilt default. A generation that
/// admits `grep` as model-selectable, parallel, and approval-required must
/// give the child that `grep` — quietly substituting a foreground-only,
/// sequential, approval-never `grep` would hand the child different
/// semantics under the same name.
///
/// Materialization is therefore checked, not assumed: the child plane
/// reconstructs the native implementation for the frozen name under the
/// frozen policy and compares the reconstructed definition against the
/// frozen one. A mismatch — a capability this build implements differently,
/// or a fixed-policy tool whose frozen policy disagrees — fails closed
/// rather than registering something the parent did not authorize.
///
/// The set is deny-by-construction in both directions:
///
/// - the `subagent` intrinsic is never registrable here, so recursive
///   delegation is structurally absent even if a definition somehow named
///   it (definition admission already rejects that selector);
/// - `ask_user` is likewise absent, because a headless child has no Runtime
///   Client questionnaire authority to satisfy it;
/// - no registration is marked mandatory, so the child's active set equals
///   its authorized set exactly rather than gaining an implicit Read.
///
/// # Errors
///
/// Returns [`ToolRegistryError::InvalidIdentity`] when the frozen set names
/// a capability the child plane does not implement or when the child cannot
/// faithfully materialize a frozen definition, and the specific
/// [`ToolRegistryError`] of the first registration violation otherwise.
///
/// [`ToolDefinition`]: crate::tools::types::ToolDefinition
pub fn register_subagent_child_tools(
    registry: &mut ToolRegistry,
    frozen: &[crate::tools::types::ToolDefinition],
) -> Result<(), ToolRegistryError> {
    for definition in frozen {
        let policy = ToolInvocationPolicy::new(
            definition.execution_policy,
            definition.concurrency_policy,
            definition.approval_policy,
        );
        let Some(registration) = subagent_child_registration(&definition.name, policy) else {
            return Err(ToolRegistryError::InvalidIdentity(format!(
                "the subagent child plane does not implement the built-in capability {:?}",
                definition.name
            )));
        };
        let NativeToolRegistration {
            definition: reconstructed,
            executor,
            normalizer,
            ..
        } = registration;
        if reconstructed != *definition {
            return Err(ToolRegistryError::InvalidIdentity(format!(
                "the subagent child plane cannot faithfully materialize the frozen \
                 definition of {:?}: this build implements {reconstructed:?}",
                definition.name
            )));
        }
        // The exact frozen definition is what the child registers. The
        // child's active set is exactly its authorized set: nothing is
        // mandatory, so no capability is force-activated beside the ones the
        // definition selected.
        registry.register_with_activation_metadata(
            definition.clone(),
            executor,
            normalizer,
            false,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{register_subagent_child_tools, subagent_child_definition};
    use crate::tools::executor::ToolRegistry;
    use crate::tools::types::{
        ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
        ToolInvocationPolicy,
    };

    fn frozen(names: &[&str], policy: ToolInvocationPolicy) -> Vec<ToolDefinition> {
        names
            .iter()
            .map(|name| {
                subagent_child_definition(name, policy)
                    .unwrap_or_else(|| panic!("{name} is a child-plane capability"))
            })
            .collect()
    }

    #[test]
    fn the_child_registry_is_exactly_the_resolved_selection() {
        let mut registry = ToolRegistry::new();
        register_subagent_child_tools(
            &mut registry,
            &frozen(&["read", "glob", "grep"], ToolInvocationPolicy::default()),
        )
        .expect("selected tools register");
        assert_eq!(registry.names(), vec!["read", "glob", "grep"]);
        assert_eq!(registry.len(), 3);
        assert!(
            registry
                .registrations()
                .iter()
                .all(|registration| !registration.mandatory),
            "a child activates exactly its authorized set, with nothing forced in"
        );
        assert!(
            registry
                .definitions()
                .iter()
                .all(|definition| definition.origin == crate::tools::types::ToolOrigin::Builtin)
        );

        let mut narrower = ToolRegistry::new();
        register_subagent_child_tools(
            &mut narrower,
            &frozen(&["grep"], ToolInvocationPolicy::default()),
        )
        .expect("a narrower selection registers");
        assert_eq!(narrower.names(), vec!["grep"]);
    }

    /// Child-unsafe and recursive capabilities are structurally
    /// unregistrable in a child, independently of definition admission.
    #[test]
    fn child_unsafe_capabilities_are_structurally_absent() {
        for name in ["subagent", "ask_user", "execution"] {
            assert!(
                subagent_child_definition(name, ToolInvocationPolicy::default()).is_none(),
                "{name} has no child-plane implementation at all"
            );
            let mut registry = ToolRegistry::new();
            let definition = ToolDefinition {
                name: name.to_owned(),
                ..subagent_child_definition("read", ToolInvocationPolicy::default())
                    .expect("read exists")
            };
            assert!(
                register_subagent_child_tools(&mut registry, &[definition]).is_err(),
                "{name} must not be registrable in a subagent child"
            );
        }
    }

    /// Issue #144 blocker 2: the child registers the **exact** parent-frozen
    /// definition, including a non-default policy on every axis. Rebuilding
    /// from a default policy table would silently give the child different
    /// semantics under the same tool name.
    #[test]
    fn a_non_default_frozen_policy_survives_child_registration_exactly() {
        let policy = ToolInvocationPolicy::new(
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Parallel,
            ToolApprovalPolicy::Always,
        );
        let frozen_definitions = frozen(&["grep"], policy);
        let frozen_grep = &frozen_definitions[0];
        assert_ne!(
            *frozen_grep,
            subagent_child_definition("grep", ToolInvocationPolicy::default())
                .expect("grep exists"),
            "the fixture must actually differ from the default-policy definition"
        );

        let mut registry = ToolRegistry::new();
        register_subagent_child_tools(&mut registry, &frozen_definitions)
            .expect("the frozen definition registers");
        let registered = registry
            .definitions()
            .into_iter()
            .find(|definition| definition.name == "grep")
            .expect("grep is registered")
            .clone();
        assert_eq!(
            registered, *frozen_grep,
            "the child registry holds the whole frozen definition, not just its name"
        );
        assert_eq!(
            registered.execution_policy,
            ToolExecutionPolicy::ModelSelectable
        );
        assert_eq!(
            registered.concurrency_policy,
            ToolConcurrencyPolicy::Parallel
        );
        assert_eq!(registered.approval_policy, ToolApprovalPolicy::Always);
    }

    /// A frozen definition this build cannot reproduce faithfully fails
    /// closed rather than registering something the parent never authorized.
    #[test]
    fn an_unmaterializable_frozen_definition_fails_closed() {
        let mut tampered = subagent_child_definition("read", ToolInvocationPolicy::default())
            .expect("read exists");
        tampered.description = "a description this build does not own".to_owned();
        let mut registry = ToolRegistry::new();
        let error = register_subagent_child_tools(&mut registry, &[tampered])
            .expect_err("an unfaithful materialization is refused");
        assert!(
            format!("{error:?}").contains("faithfully materialize"),
            "the refusal names the contract it defends: {error:?}"
        );
        assert_eq!(registry.len(), 0);
    }

    /// `todo` owns a fixed policy; a frozen definition that disagrees with
    /// it is refused instead of being quietly rewritten.
    #[test]
    fn a_fixed_policy_tool_refuses_a_conflicting_frozen_policy() {
        let mut conflicting = subagent_child_definition("todo", ToolInvocationPolicy::default())
            .expect("todo exists");
        assert_eq!(conflicting.approval_policy, ToolApprovalPolicy::Never);
        conflicting.approval_policy = ToolApprovalPolicy::Always;
        let mut registry = ToolRegistry::new();
        assert!(
            register_subagent_child_tools(&mut registry, &[conflicting]).is_err(),
            "todo's fixed policy is never silently overwritten by a frozen one"
        );
    }
}
