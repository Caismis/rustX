//! The `subagent` runtime intrinsic (Issue #60, renamed to named
//! attempt-scoped definitions by Issue #144).
//!
//! The model-facing surface of the native async one-shot subagent plane:
//!
//! ```json
//! {
//!   "agent": "explore",
//!   "task": "...",
//!   "context": "..."   // optional, bounded
//! }
//! ```
//!
//! The obsolete `profile` field is gone and is not accepted: the schema
//! denies unknown fields, so the old contract fails deterministically rather
//! than being silently reinterpreted.
//!
//! The call returns **immediately after the ownership commit** with a
//! running execution handle — the child runtime works asynchronously and its
//! bounded final answer arrives later as an ordinary inbound turn from the
//! child agent. There is no wait/poll mode and no result channel outside the
//! conversation's own message bus. The returned execution handle (Issue
//! #162) is the canonical continuation affordance: pass it to the
//! `execution` intrinsic to inspect or cancel the child.
//!
//! # Where the authority comes from
//!
//! The registered executor is long-lived, but the *authority* it resolves
//! against is not: each invocation receives the immutable
//! `RuntimeResourceSnapshot` owned by the invoking `AgentExecution` through
//! the crate-private [`ToolExecutionContext`] seam. The executor therefore
//! never reads mutable runtime-current resources, and never derives child
//! capabilities from the parent model's active `ToolRegistry`.
//!
//! The model chooses only *which named agent* runs. Model, tools, Skills,
//! project instructions, and workspace policy belong to the named
//! definition and are deliberately not per-call arguments.
//!
//! The executor stays a thin adapter over the conversation-owned
//! [`SubagentRegistry`]: input validation, attempt-scoped resolution, the
//! two-stage prepare/commit boundary, and the cancellation-race outcome
//! mapping. All lifecycle, durability, and supervision semantics live in
//! the registry; all configuration semantics live in the catalog/resolver.

use futures_util::future::BoxFuture;

use crate::runtime::subagent::catalog::{SubagentCatalog, SubagentName};
use crate::runtime::subagent::resolver::render_agent_routing;
use crate::runtime::subagent::{
    SubagentAccepted, SubagentRegistry, SubagentStartError, SubagentStartOutcome,
    SubagentStartSpec, SubagentTerminalMode,
};
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, input_schema};
use crate::tools::types::{
    ToolCancellationPhase, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
};

use super::input::decode;
use super::support::{failed_result, success_json};

/// The deterministic model-facing creation result of an accepted subagent
/// start (Issue #162).
///
/// The result returns the typed execution handle (`kind` `subagent` plus
/// the subagent id) as the canonical continuation affordance — the same
/// handle the `execution` intrinsic accepts — alongside the running state
/// and the frozen child identity. The final child answer is **not** part of
/// this result: it arrives later, exactly once, through the canonical
/// inbound message path.
pub(crate) fn accepted_result(accepted: SubagentAccepted) -> ToolExecutionResult {
    let mut result = accepted.result;
    result["execution"] = serde_json::to_value(crate::tools::execution::ExecutionHandle::subagent(
        &accepted.subagent_id,
    ))
    .expect("execution handles serialize");
    result["child_agent_id"] = serde_json::Value::String(accepted.child_agent_id.to_string());
    result["agent"] = serde_json::Value::String(accepted.agent);
    result["definition_digest"] = serde_json::Value::String(accepted.definition_digest);
    success_json(result)
}

/// The model-facing projection of a failed subagent start.
///
/// This is the boundary at which an internal runtime failure becomes
/// model-visible tool output, so it — and not the workspace/Git layer — owns
/// the public configuration spelling and the actionable remediation. The
/// workspace manager reports the typed execution fact
/// (`WorkspaceAcquireError::DirtyParent`), the registry preserves its
/// semantic identity ([`SubagentStartError::WorkspaceDirtyParent`]), and this
/// function renders what the model can actually act on.
///
/// Every other start failure keeps its own bounded runtime diagnostic: this
/// is one named translation, not a general error-presentation framework.
fn start_failure_result(error: &SubagentStartError) -> ToolExecutionResult {
    match error {
        // Issue #188. The opt-out is described exactly: it runs the child
        // from the committed HEAD snapshot and ignores the parent's local
        // dirty bytes. No stash, commit, copy, or patch is implied, and no
        // Git command detail is exposed.
        SubagentStartError::WorkspaceDirtyParent { .. } => failed_result(
            "the isolated subagent was not started because the parent workspace has \
             uncommitted changes. Commit or clean those changes, or explicitly set \
             \"requireCleanParent\": false for this subagent to run from the committed \
             HEAD snapshot while intentionally ignoring the local changes.",
        ),
        other => failed_result(other.to_string()),
    }
}

/// The canonical model-facing name of the intrinsic.
pub const SUBAGENT_TOOL_NAME: &str = "subagent";

/// The tool-owned registration of the `subagent` runtime intrinsic.
///
/// The intrinsic owns its own fixed policies (foreground-only, sequential):
/// the async boundary is the child runtime, not the tool execution.
#[must_use]
pub(super) fn registration(
    subagents: SubagentRegistry,
    catalog: &SubagentCatalog,
) -> Option<NativeToolRegistration> {
    definition(catalog).map(|definition| {
        NativeToolRegistration::new(
            definition,
            std::sync::Arc::new(SubagentExecutor { subagents }),
        )
    })
}

/// The canonical schema of the `subagent` intrinsic when this generation has
/// at least one satisfiable route.
///
/// An empty catalog produces no definition: capability composition must never
/// expose a model-facing Tool which every possible invocation is guaranteed
/// to reject.
fn definition(catalog: &SubagentCatalog) -> Option<ToolDefinition> {
    if catalog.is_empty() {
        return None;
    }
    Some(ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-subagent"),
        name: SUBAGENT_TOOL_NAME.to_owned(),
        description: format!(
            "Delegate a bounded task to a one-shot child agent runtime. The child runs \
             asynchronously in its own isolated conversation and process; this call returns \
             as soon as the child is durably started, together with the execution handle you \
             can pass to the execution tool to inspect or cancel the child. The child's final \
             answer arrives later as a new message from the child agent. Each named agent has \
             its own fixed instructions, model, capabilities, and Skills, which this call \
             cannot override.\n\n{}",
            render_agent_routing(catalog)
        ),
        input_schema: input_schema::<SubagentInput>(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    })
}

/// The typed model-facing input contract of the `subagent` intrinsic.
///
/// `deny_unknown_fields` is the whole compatibility posture: the obsolete
/// `profile` field is not a recognized argument and never becomes one.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SubagentInput {
    /// The named agent to run, from this runtime's admitted catalog.
    pub agent: String,
    /// The delegated task, in natural language.
    pub task: String,
    /// An explicit bounded context package for the child.
    #[serde(default)]
    pub context: Option<String>,
}

impl SubagentInput {
    /// Deserializes one `subagent` invocation.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        decode(SUBAGENT_TOOL_NAME, arguments)
    }
}

/// The executor of the `subagent` intrinsic.
struct SubagentExecutor {
    subagents: SubagentRegistry,
}

impl ToolExecutor for SubagentExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move {
            let input = match SubagentInput::parse(&invocation.arguments) {
                Ok(input) => input,
                Err(error) => return failed_result(error),
            };
            // Without an attempt-scoped view there is no generation to
            // resolve against, and guessing one would be exactly the
            // tearing this seam exists to prevent.
            let Some(subagent_context) = context.subagent_context() else {
                return failed_result(
                    "the subagent capability is not available outside an agent attempt",
                );
            };
            let agent = match SubagentName::parse(&input.agent) {
                Ok(agent) => agent,
                Err(error) => {
                    return failed_result(format!(
                        "invalid subagent name {:?}: {error}. {}",
                        input.agent,
                        subagent_context.routing_description()
                    ));
                }
            };
            let resolved = match subagent_context.resolve(&agent) {
                Ok(resolved) => resolved,
                Err(error) => return failed_result(error.to_string()),
            };
            let spec = SubagentStartSpec {
                resolved,
                approval_mode: subagent_context.approval_mode(),
                task: input.task,
                context: input.context,
                tool_call_id: invocation.call_id.clone(),
                terminal: SubagentTerminalMode::Normal,
            };
            // One attempt-derived cancellation authority owns the whole
            // pre-commit lifecycle: preparation (identity, spawn, startup
            // handshake, child external capability composition) AND the
            // commit decision. There is no second, unrelated cancellation
            // model for the same staged lifecycle.
            let child_cancellation = context.cancellation.child_signal();
            let prepared = match self.subagents.prepare(&spec, &child_cancellation).await {
                Ok(prepared) => prepared,
                Err(SubagentStartError::Cancelled) => {
                    // The attempt cancellation won while the child was
                    // still staging: nothing was published, every staged
                    // physical resource settled before the error returned,
                    // and the tool result is the absorbing cancellation
                    // outcome.
                    return ToolExecutionResult {
                        status: ToolExecutionStatus::Cancelled {
                            reason: context.cancellation.reason(),
                            phase: ToolCancellationPhase::DuringExecution,
                        },
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        managed_output: None,
                    };
                }
                Err(error) => return start_failure_result(&error),
            };
            match self.subagents.commit(prepared, &child_cancellation).await {
                Ok(SubagentStartOutcome::Accepted(accepted)) => accepted_result(accepted),
                // The attempt cancellation won the race against the
                // ownership commit: nothing was published, the staged child
                // is already torn down, and the tool result is the
                // absorbing cancellation outcome.
                Ok(SubagentStartOutcome::RolledBack) => ToolExecutionResult {
                    status: ToolExecutionStatus::Cancelled {
                        reason: context.cancellation.reason(),
                        phase: ToolCancellationPhase::DuringExecution,
                    },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
                Err(SubagentStartError::ConversationInactive) => {
                    failed_result("the conversation is shutting down")
                }
                Err(error) => start_failure_result(&error),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{SUBAGENT_TOOL_NAME, SubagentExecutor, SubagentInput, ToolInvocation, definition};
    use crate::runtime::subagent::SubagentWorkspacePolicy;
    use crate::runtime::subagent::catalog::{
        SubagentCatalog, SubagentDefinition, SubagentName, SubagentProjectInstructionPolicy,
    };
    use crate::tools::types::{ToolExecutionStatus, ToolResultContent};

    fn catalog() -> SubagentCatalog {
        SubagentCatalog::new([
            SubagentDefinition::new(
                SubagentName::parse("research").expect("name"),
                "Deep multi-source research.".to_owned(),
                "instructions".to_owned(),
                std::path::PathBuf::from("/w/research.md"),
                None,
                Vec::new(),
                Vec::new(),
                SubagentProjectInstructionPolicy {
                    inherit: true,
                    files: Vec::new(),
                },
                SubagentWorkspacePolicy::SharedWorkspace,
            )
            .expect("definition"),
            SubagentDefinition::new(
                SubagentName::parse("explore").expect("name"),
                "Read-only repository exploration.".to_owned(),
                "instructions".to_owned(),
                std::path::PathBuf::from("/w/explore.md"),
                None,
                Vec::new(),
                Vec::new(),
                SubagentProjectInstructionPolicy {
                    inherit: true,
                    files: Vec::new(),
                },
                SubagentWorkspacePolicy::SharedWorkspace,
            )
            .expect("definition"),
        ])
        .expect("catalog")
    }

    #[test]
    fn the_input_contract_accepts_agent_and_rejects_the_obsolete_profile_field() {
        let accepted = SubagentInput::parse(&serde_json::json!({
            "agent": "explore",
            "task": "inspect the tool plane",
        }))
        .expect("the named-agent contract is accepted");
        assert_eq!(accepted.agent, "explore");
        assert_eq!(accepted.context, None);

        let rejected = SubagentInput::parse(&serde_json::json!({
            "profile": "explore",
            "task": "inspect the tool plane",
        }))
        .expect_err("the obsolete profile contract is rejected");
        assert!(
            rejected.contains("profile") || rejected.contains("agent"),
            "the rejection names the contract violation: {rejected}"
        );

        // Not even alongside the new field: there is exactly one contract.
        assert!(
            SubagentInput::parse(&serde_json::json!({
                "agent": "explore",
                "profile": "explore",
                "task": "inspect",
            }))
            .is_err()
        );
    }

    #[test]
    fn the_accepted_creation_result_returns_a_typed_execution_handle() {
        use crate::runtime::subagent::SubagentAccepted;
        let accepted = SubagentAccepted {
            subagent_id: crate::runtime::identity::SubagentId::new("conversation-1-subagent-2"),
            child_agent_id: crate::runtime::identity::AgentId::new("agent-child"),
            child_conversation_id: crate::runtime::identity::ConversationId::new(
                "conversation-1-subagent-2",
            ),
            agent: "explore".to_owned(),
            definition_digest: "sha256:d1".to_owned(),
            result: serde_json::json!({
                "state": "running",
                "note": "The child runtime is running asynchronously. Its answer arrives \
                         as a new turn from the child agent; do not retry or poll for it."
            }),
        };
        let result = super::accepted_result(accepted);
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let value = match &result.content[0] {
            ToolResultContent::Json { value } => value.clone(),
            other => panic!("expected JSON, got {other:?}"),
        };
        assert_eq!(
            value["execution"],
            serde_json::json!({"kind": "subagent", "id": "conversation-1-subagent-2"}),
            "the creation result returns the typed execution handle"
        );
        assert_eq!(value["state"], "running");
        assert_eq!(value["agent"], "explore");
        assert_eq!(value["definition_digest"], "sha256:d1");
        assert_eq!(value["child_agent_id"], "agent-child");
        assert!(
            value.get("subagent_id").is_none(),
            "the bare id is replaced by the tagged handle"
        );
        assert!(
            value.get("status").is_none(),
            "the status spelling is replaced by the state vocabulary"
        );
    }

    #[test]
    fn per_call_capability_overrides_are_not_representable() {
        for field in [
            "model",
            "tools",
            "skills",
            "instructions",
            "agents_md",
            "workspace",
            "worktree",
        ] {
            assert!(
                SubagentInput::parse(&serde_json::json!({
                    "agent": "explore",
                    "task": "inspect",
                    field: serde_json::json!("anything"),
                }))
                .is_err(),
                "{field} must not be a per-call argument"
            );
        }
    }

    /// The real collaborators the `subagent` intrinsic needs to reach
    /// workspace acquisition against a genuinely dirty Git parent.
    ///
    /// Everything the execution context borrows lives here so the context
    /// itself can be built inside the test that drives the tool.
    #[cfg(unix)]
    struct DirtyParentPlane {
        _dir: tempfile::TempDir,
        conversation_id: crate::runtime::identity::ConversationId,
        subagents: crate::runtime::subagent::SubagentRegistry,
        subagent_context: crate::runtime::subagent::AttemptSubagentContext,
        workspace: crate::tools::workspace::Workspace,
        artifacts: crate::tools::artifacts::ArtifactStore,
        tool_output: crate::tools::managed_output::ManagedToolOutput,
        environment: crate::tools::environment::ToolEnvironment,
        runtime_root: std::path::PathBuf,
    }

    #[cfg(unix)]
    const DIRTY_PARENT_MODELS: &str = r#"{
      "providers": {
        "local": {
          "baseUrl": "http://127.0.0.1:9/v1",
          "apiKey": "test-only-secret",
          "models": [{
            "id": "model",
            "protocol": "openai_chat_completions",
            "contextWindow": 128000,
            "maxOutputTokens": 512,
            "capabilities": {
              "inputModalities": ["text"],
              "outputModalities": ["text"],
              "toolCalls": true,
              "reasoning": false
            },
            "compat": {"chatReasoningReplay": "omit"}
          }]
        }
      }
    }"#;

    #[cfg(unix)]
    fn git(cwd: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env("GIT_AUTHOR_NAME", "rustX tests")
            .env("GIT_AUTHOR_EMAIL", "rustx-tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "rustX tests")
            .env("GIT_COMMITTER_EMAIL", "rustx-tests@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Builds a conversation whose only admitted agent asks for an isolated
    /// worktree under the Issue #188 default (strict clean parent), over a
    /// parent repository made dirty by an ordinary tracked modification.
    #[cfg(unix)]
    #[allow(clippy::too_many_lines)] // one cohesive real-collaborator fixture
    fn dirty_parent_plane() -> DirtyParentPlane {
        use std::collections::BTreeSet;
        use std::sync::Arc;

        use crate::capabilities::CapabilitySnapshot;
        use crate::context::SessionContextPolicy;
        use crate::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
        use crate::model::invocation::ModelBindingRegistry;
        use crate::model::session::SessionModelConfig;
        use crate::runtime::identity::{AgentId, CapabilityRevision, ConversationId};
        use crate::runtime::inbound::ConversationInboundMailbox;
        use crate::runtime::subagent::{
            AttemptSubagentContext, SubagentRegistry, SubagentRegistryConfig, SubagentSpawnPlan,
            SubagentWorkspaceManager,
        };
        use crate::runtime::types::{ApprovalMode, SystemClock};
        use crate::skills::SkillSnapshot;
        use crate::tools::environment::ToolEnvironment;
        use crate::tools::mcp::McpRuntimeLeaseAuthority;

        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        let runtime_root = dir.path().join("runtime");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        std::fs::create_dir_all(&runtime_root).expect("runtime root");

        git(&workspace_root, &["init"]);
        std::fs::write(workspace_root.join("tracked.txt"), "committed\n").expect("tracked file");
        git(&workspace_root, &["add", "tracked.txt"]);
        git(&workspace_root, &["commit", "-m", "initial"]);
        std::fs::write(workspace_root.join("tracked.txt"), "dirty parent\n").expect("dirty file");

        let conversation_id = ConversationId::new("conv-dirty-parent");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
                .expect("in-memory store"),
        );
        let subagents = SubagentRegistry::new(SubagentRegistryConfig {
            conversation_id: conversation_id.clone(),
            agent_id: AgentId::new("agent-parent"),
            mailbox: ConversationInboundMailbox::over_store(store),
            clock: Arc::new(SystemClock),
            spawn: SubagentSpawnPlan {
                program: std::path::PathBuf::from("/nonexistent/rustx"),
                runtime_root: runtime_root.clone(),
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                agent_status: crate::context::AgentStatusConfig::default(),
                context: SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
            },
            workspace: SubagentWorkspaceManager::new(&workspace_root, &runtime_root),
            max_active: 4,
        });

        let isolated = SubagentName::parse("isolated").expect("name");
        let model = ModelRef::parse("local/model").expect("model");
        let definition = SubagentDefinition::new(
            isolated.clone(),
            "Isolated worktree agent.".to_owned(),
            "instructions".to_owned(),
            workspace_root.join("isolated.md"),
            Some(model.clone()),
            Vec::new(),
            Vec::new(),
            SubagentProjectInstructionPolicy {
                inherit: false,
                files: Vec::new(),
            },
            SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            },
        )
        .expect("definition");

        let models = ModelBindingRegistry::new(
            ModelCatalog::from_jsonc_slice(DIRTY_PARENT_MODELS.as_bytes())
                .expect("model catalog")
                .resolve(&MapCredentialEnvironment::default())
                .expect("model resolution"),
        )
        .expect("model bindings");
        let capabilities = Arc::new(CapabilitySnapshot::new(
            conversation_id.clone(),
            workspace_root.clone(),
            CapabilityRevision::new(1),
            Arc::new(crate::tools::executor::ToolRegistry::new()),
            Arc::new(crate::capabilities::AvailableToolCatalog::default()),
            Arc::new(SkillSnapshot::new(Vec::new())),
            None,
            None,
            ToolEnvironment::new(),
            Arc::new(McpRuntimeLeaseAuthority::empty()),
            Arc::new(std::collections::BTreeMap::new()),
        ));
        let resources = Arc::new(
            crate::runtime::RuntimeResourceSnapshot::new(
                crate::runtime::identity::RuntimeResourceRevision::new(1),
                Vec::new(),
                None,
                crate::context::ContextAssembly::new(),
                capabilities,
            )
            .with_subagent_catalog(SubagentCatalog::new([definition]).expect("catalog"))
            .with_subagent_admissions(BTreeSet::from([isolated]), BTreeSet::new()),
        );

        DirtyParentPlane {
            subagent_context: AttemptSubagentContext::new(
                resources,
                SessionModelConfig::of(model),
                models,
                ApprovalMode::Policy,
            ),
            workspace: crate::tools::workspace::Workspace::new(&workspace_root).expect("workspace"),
            artifacts: crate::tools::artifacts::ArtifactStore::new(
                conversation_id.clone(),
                dir.path().join("artifacts"),
            )
            .expect("artifact store"),
            tool_output: crate::tools::managed_output::ManagedToolOutput::new(
                conversation_id.clone(),
                dir.path().join("tool-output"),
            )
            .expect("managed tool output"),
            environment: ToolEnvironment::new(),
            conversation_id,
            subagents,
            runtime_root,
            _dir: dir,
        }
    }

    /// Issue #188 — the model-facing regression.
    ///
    /// This drives the real `subagent` intrinsic against a real registry and
    /// a real dirty Git parent, so the assertion is made on the actual
    /// `ToolExecutionResult` the model would see, not on any lower layer's
    /// `Display`. The chain under test is:
    ///
    /// ```text
    /// WorkspaceAcquireError::DirtyParent            (typed execution fact)
    ///   -> SubagentStartError::WorkspaceDirtyParent (typed identity kept)
    ///   -> ToolExecutionResult                      (actionable remediation)
    /// ```
    #[cfg(unix)]
    #[tokio::test]
    async fn a_dirty_parent_renders_actionable_guidance_at_the_model_facing_boundary() {
        use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};

        struct NoProgress;
        impl ProgressReporter for NoProgress {
            fn report(&self, _progress: crate::tools::types::ToolProgress) {}
        }

        let plane = dirty_parent_plane();
        let progress = NoProgress;
        let context = ToolExecutionContext::new(
            &plane.conversation_id,
            None,
            crate::runtime::cancellation::ExecutionCancellation::detached(
                crate::runtime::cancellation::CancellationSignal::new(),
                crate::runtime::types::CancellationReason::UserRequested,
            ),
            &plane.workspace,
            &progress,
            &plane.artifacts,
            &plane.tool_output,
            &plane.environment,
        )
        .with_subagent_context(plane.subagent_context.clone());

        let executor = SubagentExecutor {
            subagents: plane.subagents.clone(),
        };
        let result = executor
            .execute(
                ToolInvocation {
                    call_id: crate::runtime::identity::ToolCallId::new("call-1"),
                    tool_id: crate::runtime::identity::ToolId::new("tool-subagent"),
                    tool_name: SUBAGENT_TOOL_NAME.to_owned(),
                    mode: crate::tools::types::ToolInvocationMode::Foreground,
                    arguments: serde_json::json!({
                        "agent": "isolated",
                        "task": "inspect the isolated workspace",
                    }),
                },
                context,
            )
            .await;

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("the dirty parent must fail the call: {:?}", result.status);
        };
        // The actionable contract the model receives.
        for required in [
            "isolated subagent was not started",
            "uncommitted changes",
            "Commit or clean",
            "requireCleanParent",
            "committed HEAD",
            "ignoring the local changes",
        ] {
            assert!(
                error.contains(required),
                "the model-facing result must state {required:?}: {error}"
            );
        }
        // And nothing the model cannot act on: no Git plumbing, no raw
        // command output, and no implication that the runtime will move the
        // parent's changes for it.
        for leaked in ["rev-parse", "porcelain", "git ", "stash", "auto-commit"] {
            assert!(
                !error.contains(leaked),
                "the model-facing result must not expose {leaked:?}: {error}"
            );
        }
        assert!(
            !plane.runtime_root.join("worktrees").exists(),
            "the rejection happens before any physical worktree is created"
        );
    }

    #[test]
    fn the_tool_description_is_derived_from_the_admitted_catalog() {
        let described = definition(&catalog()).expect("a non-empty catalog has a route");
        assert!(
            described
                .description
                .contains("- explore: Read-only repository exploration.")
        );
        assert!(
            described
                .description
                .contains("- research: Deep multi-source research.")
        );
        assert!(
            !described.description.contains("profile"),
            "no hard-coded profile prose survives: {}",
            described.description
        );
        assert!(
            definition(&SubagentCatalog::empty()).is_none(),
            "an unsatisfiable subagent tool never enters the model-facing capability set"
        );

        // The schema stays small: exactly agent/task/context.
        let properties = described.input_schema["properties"]
            .as_object()
            .expect("object schema");
        let mut names = properties.keys().cloned().collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec!["agent", "context", "task"]);
    }
}
