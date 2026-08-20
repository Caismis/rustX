//! The runtime-owned tool execution contract and the validating registry.
//!
//! M1 defined the declarative tool data contracts in [`crate::tools::types`];
//! M3 added the runtime-owned execution contract; M5 replaces the
//! provisional `Tool` trait with the canonical [`ToolExecutor`] boundary that
//! native, MCP, and Python tools share this boundary. The agent loop
//! (M3) owns scheduling: it preflights every model-issued [`ToolCall`]
//! against the [`ToolRegistry`] (identity resolution, execution-policy
//! resolution, runtime metadata extraction, business argument validation)
//! and only then dispatches the stripped, validated invocation to the
//! executor. Provider adapters describe tool calls; they never execute
//! tools.
//!
//! The registry owns the definition/executor relationship: one
//! [`ToolDefinition`] is paired with one `Arc<dyn ToolExecutor>` at
//! registration. An executor object does not own its definition, so one
//! implementation object may serve
//! multiple registered definitions.
//!
//! There is no plugin framework, no dynamic loading, and no speculative
//! metadata subsystem: only the concrete contracts the current tool plane
//! needs.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::runtime::cancellation::ExecutionCancellation;
use crate::runtime::identity::{ConversationId, ToolExecutionId, ToolId};
use crate::tools::artifacts::ArtifactStore;
use crate::tools::environment::ToolEnvironment;
use crate::tools::managed_output::ManagedToolOutput;
use crate::tools::schema::{
    SchemaError, compile_model_definition, resolve_invocation_metadata,
    validate_business_arguments, validate_canonical_schema,
};
use crate::tools::types::{
    ModelToolDefinition, ToolCall, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionResult,
    ToolInvocation, ToolOrigin, ToolProgress,
};
use crate::tools::workspace::Workspace;

/// The canonical progress reporter boundary of one tool execution.
///
/// Executors report bounded progress facts through this narrow seam. For
/// foreground work the agent loop turns the facts into canonical
/// [`RuntimeEvent::ToolExecutionProgress`] events; for background work the
/// conversation background registry updates its latest progress snapshot and
/// emits the corresponding execution fact.
///
/// [`RuntimeEvent::ToolExecutionProgress`]: crate::events::types::RuntimeEvent::ToolExecutionProgress
pub trait ProgressReporter: Send + Sync {
    /// Reports one bounded progress notification.
    fn report(&self, progress: ToolProgress);
}

/// The runtime-owned execution context of one tool invocation.
///
/// The context provides the concrete resources the current contract needs —
/// conversation identity, execution identity when background, the runtime
/// cancellation signal, the workspace boundary, the progress reporter, the
/// artifact store, the managed tool-output store, and the explicit
/// authorized environment. Executor-
/// specific resources (process ids, MCP SDK types, Python runtime objects)
/// belong inside executor implementations and never appear here.
pub struct ToolExecutionContext<'a> {
    /// The owning conversation of the invocation.
    pub conversation_id: &'a ConversationId,
    /// The detached runtime execution identity, `None` for foreground work.
    /// No fake `ToolExecutionId` is invented for foreground calls.
    pub execution_id: Option<&'a ToolExecutionId>,
    /// The cancellation view of the execution: the runtime cancellation
    /// signal plus a **live** read of the owning authority's absorbing
    /// cause. Foreground executions view their attempt's `AgentCancellation`;
    /// background executions view their conversation-owned background record.
    ///
    /// The cause is read through this view at settlement time, never copied
    /// at start time: an execution that started before the cancellation race
    /// happened still reports the cause that actually won it.
    pub cancellation: ExecutionCancellation,
    /// The canonical workspace boundary every native filesystem tool and
    /// Bash operates against.
    pub workspace: &'a Workspace,
    /// The bounded progress reporter of the execution.
    pub progress: &'a dyn ProgressReporter,
    /// The conversation-owned artifact store for genuine semantic file
    /// artifacts (never for oversized textual output).
    pub artifacts: &'a ArtifactStore,
    /// The conversation-owned managed tool-output store: the read-only
    /// region of auxiliary textual output — lazy result spills of oversized
    /// foreground output, and the dispatch-allocated live-output channel of
    /// background executions.
    pub tool_output: &'a ManagedToolOutput,
    /// The explicit authorized tool environment.
    pub environment: &'a ToolEnvironment,
}

/// One executable tool.
///
/// Executors execute an already-resolved, already-validated
/// [`ToolInvocation`] and report the actual execution outcome: a failure is
/// a normalized [`ToolExecutionStatus::Failed`] result, never a fabricated
/// success. The runtime records the returned result verbatim.
///
/// An executor must settle after cancellation: when the [`ExecutionCancellation`]
/// in its context fires, a cancellable executor physically settles its
/// external work (for example by terminating an owned process group) and
/// returns a normalized cancelled result instead of abandoning work.
///
/// [`ToolExecutionStatus::Failed`]: crate::tools::types::ToolExecutionStatus::Failed
pub trait ToolExecutor: Send + Sync {
    /// Executes one canonical invocation.
    ///
    /// The returned future is owned by the executor: implementations clone
    /// or capture the data they need and never borrow past the call.
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult>;
}

/// A registration or resolution failure of the tool registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRegistryError {
    /// The `ToolId` is already registered.
    DuplicateToolId(ToolId),
    /// The model-facing name is already registered.
    DuplicateName(String),
    /// The tool identity or model-facing name is empty.
    InvalidIdentity(String),
    /// The canonical input schema is invalid or not a root object schema.
    InvalidSchema(String),
    /// The canonical input schema claims a reserved `__rustx_*` property.
    ReservedProperty(String),
    /// The declared policies are invalid for this tool.
    InvalidPolicy(String),
}

impl core::fmt::Display for ToolRegistryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicateToolId(id) => write!(f, "tool id {id} is already registered"),
            Self::DuplicateName(name) => write!(
                f,
                "model-facing tool name {name:?} is already registered; \
                 tool names must be unambiguous"
            ),
            Self::InvalidIdentity(message) => write!(f, "{message}"),
            Self::InvalidSchema(message) => write!(f, "invalid tool schema: {message}"),
            Self::ReservedProperty(_) | Self::InvalidPolicy(_) => {
                write!(f, "the tool registration violates a registry rule")
            }
        }
    }
}

impl std::error::Error for ToolRegistryError {}

/// A structural preflight failure of one model-issued tool call.
///
/// These are canonical contract violations of the model stream: the call
/// cannot be resolved unambiguously against the registry, so no
/// attempt-facing result slot exists for it.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPreflightError {
    /// Neither the call's tool id nor its name resolve to a registered tool.
    UnknownTool {
        /// The model-facing name the model called.
        name: String,
    },
    /// The call's tool id and model-facing name disagree.
    IdentityMismatch {
        /// The resolved tool id of the call.
        id: ToolId,
        /// The resolved model-facing name of the call.
        name: String,
    },
}

/// The preflight outcome of one model-issued tool call.
///
/// Both variants carry the canonical registry-resolved [`ToolId`] and
/// [`ToolOrigin`]: identity resolution succeeds before argument validation
/// can reject a call, so every settled result slot of a committed batch has a
/// stable typed identity available to the Agent Loop. Consumers therefore
/// never have to compare model-facing tool names to recognize a native
/// capability.
#[derive(Debug, Clone, PartialEq)]
pub enum PreflightOutcome {
    /// The call is ready to execute: the stripped, validated invocation, its
    /// scheduling policy, and its resolved origin.
    Ready(PreparedInvocation),
    /// The call is rejected as a normal failed result: the reserved runtime
    /// metadata is missing/invalid or the business arguments violate the
    /// canonical schema. The executor must not run.
    Rejected {
        /// The canonical registry-resolved tool identity of the call.
        tool_id: ToolId,
        /// The canonical registry-resolved origin of the tool.
        origin: ToolOrigin,
        /// The deterministic rejection reason.
        error: String,
    },
}

/// A fully preflighted invocation: the canonical invocation plus its
/// scheduling policy and resolved origin.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedInvocation {
    /// The stripped, validated canonical invocation.
    pub invocation: ToolInvocation,
    /// The tool's concurrency policy for batch scheduling.
    pub concurrency: ToolConcurrencyPolicy,
    /// The canonical registry-resolved origin of the tool.
    ///
    /// It is taken from the same resolved [`ToolDefinition`] that produced
    /// `invocation`, so no second stored identity can disagree with the
    /// registry.
    pub origin: ToolOrigin,
}

/// The immutable validating tool registry of one attempt's capability set.
///
/// The registry is a correctness boundary: it validates every registration
/// (duplicate ids, duplicate model-facing names, invalid identities,
/// invalid JSON Schema, reserved runtime-property collisions, invalid
/// policy combinations, and the fixed intrinsic policies of
/// `background_task`) and resolves model-issued calls unambiguously. There
/// is no ID-first/name-fallback behavior: a canonical call whose id and name
/// disagree is a contract violation.
#[derive(Default)]
pub struct ToolRegistry {
    entries: Vec<RegistryEntry>,
    by_id: HashMap<ToolId, usize>,
    by_name: HashMap<String, usize>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolRegistry")
            .field("definitions", &self.definitions())
            .finish()
    }
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        let mut clone = Self::new();
        for entry in &self.entries {
            clone
                .register(entry.definition.clone(), entry.executor.clone())
                .expect("a validated registry clones without registration errors");
        }
        clone
    }
}

/// One registered definition/executor pair.
struct RegistryEntry {
    definition: ToolDefinition,
    executor: Arc<dyn ToolExecutor>,
}

/// The name of the runtime intrinsic background inspection tool.
pub const BACKGROUND_TASK_TOOL_NAME: &str = "background_task";

impl ToolRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            by_id: HashMap::new(),
            by_name: HashMap::new(),
        }
    }

    /// Registers one tool definition with its executor.
    ///
    /// Registration validates the definition before insertion: duplicate
    /// `ToolId`s and duplicate model-facing names are rejected, empty
    /// identities are rejected, the canonical input schema must be a valid
    /// root object schema with no reserved `__rustx_*` property, and the
    /// runtime intrinsic `background_task` is fixed to
    /// `ForegroundOnly` + `Sequential` and may never be background-capable.
    ///
    /// # Errors
    ///
    /// Returns the specific [`ToolRegistryError`] of the first violation.
    pub fn register(
        &mut self,
        definition: ToolDefinition,
        executor: Arc<dyn ToolExecutor>,
    ) -> Result<(), ToolRegistryError> {
        if definition.id.as_str().is_empty() {
            return Err(ToolRegistryError::InvalidIdentity(format!(
                "tool {:?} must carry a non-empty ToolId",
                definition.name
            )));
        }
        if definition.name.is_empty() {
            return Err(ToolRegistryError::InvalidIdentity(format!(
                "tool {} must carry a non-empty model-facing name",
                definition.id
            )));
        }
        if self.by_id.contains_key(&definition.id) {
            return Err(ToolRegistryError::DuplicateToolId(definition.id.clone()));
        }
        if self.by_name.contains_key(&definition.name) {
            return Err(ToolRegistryError::DuplicateName(definition.name.clone()));
        }
        validate_canonical_schema(&definition.input_schema).map_err(|error| match error {
            SchemaError::InvalidSchema(message) | SchemaError::NotRootObjectSchema(message) => {
                ToolRegistryError::InvalidSchema(format!("{}: {message}", definition.name))
            }
            SchemaError::ReservedProperty(property) => {
                ToolRegistryError::ReservedProperty(property)
            }
            other => ToolRegistryError::InvalidSchema(other.to_string()),
        })?;
        if definition.name == BACKGROUND_TASK_TOOL_NAME
            && (definition.execution_policy
                != crate::tools::types::ToolExecutionPolicy::ForegroundOnly
                || definition.concurrency_policy != ToolConcurrencyPolicy::Sequential)
        {
            return Err(ToolRegistryError::InvalidPolicy(format!(
                "the runtime intrinsic {BACKGROUND_TASK_TOOL_NAME} is fixed to \
                 foreground-only sequential execution and may never be background-dispatchable"
            )));
        }
        self.by_id.insert(definition.id.clone(), self.entries.len());
        self.by_name
            .insert(definition.name.clone(), self.entries.len());
        self.entries.push(RegistryEntry {
            definition,
            executor,
        });
        Ok(())
    }

    /// Composes a new registry from this immutable base and additional
    /// definitions. The base entries retain their deterministic order and
    /// external callers must provide their entries in their canonical order.
    /// Registration remains the single validation and collision boundary.
    ///
    /// # Errors
    ///
    /// Returns a registry error if an added definition duplicates an id or
    /// model-facing name, or violates the canonical schema contract.
    pub fn compose(
        &self,
        additions: impl IntoIterator<Item = (ToolDefinition, Arc<dyn ToolExecutor>)>,
    ) -> Result<Self, ToolRegistryError> {
        let mut composed = self.clone();
        for (definition, executor) in additions {
            composed.register(definition, executor)?;
        }
        Ok(composed)
    }

    /// The canonical definitions of every registered tool in registration
    /// order.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.entries
            .iter()
            .map(|entry| entry.definition.clone())
            .collect()
    }

    /// The compiled model-facing definitions in registration order. These
    /// are what one model request receives; the canonical schemas are never
    /// sent.
    ///
    /// # Panics
    ///
    /// Panics only when a registered canonical schema would fail its own
    /// registration validation, which is impossible by construction.
    #[must_use]
    pub fn model_definitions(&self) -> Vec<ModelToolDefinition> {
        self.entries
            .iter()
            .map(|entry| {
                compile_model_definition(&entry.definition).expect(
                    "registered canonical schemas always compile into model-facing definitions",
                )
            })
            .collect()
    }

    /// The number of registered tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The registered model-facing names in registration order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.definition.name.as_str())
            .collect()
    }

    /// Preflights one model-issued tool call.
    ///
    /// # Errors
    ///
    /// Returns [`ToolPreflightError::UnknownTool`] when neither the call's
    /// id nor its name resolve to a registered tool and
    /// [`ToolPreflightError::IdentityMismatch`] when the call's id and name
    /// disagree.
    ///
    /// Invocation order:
    ///
    /// ```text
    /// resolve tool
    /// → extract/resolve rustX invocation metadata
    /// → strip rustX metadata
    /// → validate business arguments against the canonical schema
    /// → dispatch executor
    /// ```
    ///
    /// Identity resolution is unambiguous: the call's `ToolId` and
    /// model-facing name must resolve to the same registered tool. An
    /// unresolvable or inconsistent call is a structural
    /// [`ToolPreflightError`]; a missing/invalid reserved invocation field or
    /// a business schema violation is a normal [`PreflightOutcome::Rejected`]
    /// that never reaches an executor.
    pub fn preflight(&self, call: &ToolCall) -> Result<PreflightOutcome, ToolPreflightError> {
        let entry = self.resolve_entry(call)?;
        let (mode, stripped) =
            match resolve_invocation_metadata(entry.definition.execution_policy, &call.arguments) {
                Ok(value) => value,
                Err(error) => {
                    return Ok(PreflightOutcome::Rejected {
                        tool_id: entry.definition.id.clone(),
                        origin: entry.definition.origin.clone(),
                        error: error.to_string(),
                    });
                }
            };
        if let Err(error) = validate_business_arguments(&entry.definition.input_schema, &stripped) {
            return Ok(PreflightOutcome::Rejected {
                tool_id: entry.definition.id.clone(),
                origin: entry.definition.origin.clone(),
                error: error.to_string(),
            });
        }
        Ok(PreflightOutcome::Ready(PreparedInvocation {
            invocation: ToolInvocation {
                call_id: call.id.clone(),
                tool_id: entry.definition.id.clone(),
                tool_name: entry.definition.name.clone(),
                mode,
                arguments: stripped,
            },
            concurrency: entry.definition.concurrency_policy,
            origin: entry.definition.origin.clone(),
        }))
    }

    /// The executor of a preflighted tool.
    ///
    /// # Panics
    ///
    /// Panics only when the tool was never registered, which is impossible
    /// after a successful preflight.
    #[must_use]
    pub fn executor(&self, tool_id: &ToolId) -> Arc<dyn ToolExecutor> {
        let index = *self
            .by_id
            .get(tool_id)
            .expect("preflighted tools are registered");
        self.entries[index].executor.clone()
    }

    /// Resolves a canonical call to its registered entry, requiring the id
    /// and the model-facing name to agree.
    fn resolve_entry(&self, call: &ToolCall) -> Result<&RegistryEntry, ToolPreflightError> {
        let by_id = self.by_id.get(&call.tool_id).copied();
        let by_name = self.by_name.get(&call.name).copied();
        match (by_id, by_name) {
            (Some(id_index), Some(name_index)) if id_index == name_index => {
                Ok(&self.entries[id_index])
            }
            (Some(_), Some(_)) => Err(ToolPreflightError::IdentityMismatch {
                id: call.tool_id.clone(),
                name: call.name.clone(),
            }),
            (Some(index), None) | (None, Some(index)) => {
                let entry = &self.entries[index];
                let (id, name) = (entry.definition.id.clone(), entry.definition.name.clone());
                Err(ToolPreflightError::IdentityMismatch { id, name })
            }
            (None, None) => Err(ToolPreflightError::UnknownTool {
                name: call.name.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BACKGROUND_TASK_TOOL_NAME, PreflightOutcome, ToolPreflightError, ToolRegistry,
        ToolRegistryError,
    };
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::types::{
        ToolCall, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
        ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolOrigin, ToolProgress,
        ToolReplayPolicy,
    };
    use crate::tools::workspace::Workspace;
    use futures_util::FutureExt;
    use futures_util::future::{BoxFuture, ready};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn definition(
        id: &str,
        name: &str,
        execution: ToolExecutionPolicy,
        concurrency: ToolConcurrencyPolicy,
        schema: serde_json::Value,
    ) -> ToolDefinition {
        ToolDefinition {
            id: ToolId::new(id),
            name: name.to_owned(),
            description: String::new(),
            input_schema: schema,
            execution_policy: execution,
            concurrency_policy: concurrency,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        }
    }

    fn object_schema() -> serde_json::Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }

    /// An instant executor returning one fixed result.
    struct StubExecutor {
        result: ToolExecutionResult,
    }

    impl StubExecutor {
        fn success() -> Self {
            Self {
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                },
            }
        }
    }

    impl super::ToolExecutor for StubExecutor {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: super::ToolExecutionContext<'a>,
        ) -> BoxFuture<'a, ToolExecutionResult> {
            Box::pin(ready(self.result.clone()))
        }
    }

    fn call(tool_id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new(tool_id),
            name: name.to_owned(),
            arguments,
        }
    }

    fn register(
        registry: &mut ToolRegistry,
        definition: ToolDefinition,
    ) -> Result<(), ToolRegistryError> {
        registry.register(definition, std::sync::Arc::new(StubExecutor::success()))
    }

    /// Duplicate tool ids are rejected.
    #[test]
    fn duplicate_tool_id_is_rejected() {
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-a",
                "alpha",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                object_schema(),
            ),
        )
        .expect("first registration");
        let error = register(
            &mut registry,
            definition(
                "tool-a",
                "beta",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                object_schema(),
            ),
        )
        .expect_err("duplicate id");
        assert!(matches!(error, ToolRegistryError::DuplicateToolId(_)));
    }

    /// Duplicate model-facing names are rejected.
    #[test]
    fn duplicate_model_facing_name_is_rejected() {
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-a",
                "alpha",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                object_schema(),
            ),
        )
        .expect("first registration");
        let error = register(
            &mut registry,
            definition(
                "tool-b",
                "alpha",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                object_schema(),
            ),
        )
        .expect_err("duplicate name");
        assert!(matches!(error, ToolRegistryError::DuplicateName(_)));
    }

    /// Empty identities are rejected.
    #[test]
    fn empty_identities_are_rejected() {
        let mut registry = ToolRegistry::new();
        assert!(
            register(
                &mut registry,
                definition(
                    "",
                    "alpha",
                    ToolExecutionPolicy::ForegroundOnly,
                    ToolConcurrencyPolicy::Sequential,
                    object_schema()
                ),
            )
            .is_err()
        );
        assert!(
            register(
                &mut registry,
                definition(
                    "tool-a",
                    "",
                    ToolExecutionPolicy::ForegroundOnly,
                    ToolConcurrencyPolicy::Sequential,
                    object_schema()
                ),
            )
            .is_err()
        );
    }

    /// Invalid schemas and reserved properties are rejected at registration.
    #[test]
    fn invalid_schemas_and_reserved_properties_are_rejected() {
        let mut registry = ToolRegistry::new();
        assert!(
            register(
                &mut registry,
                definition(
                    "tool-a",
                    "alpha",
                    ToolExecutionPolicy::ForegroundOnly,
                    ToolConcurrencyPolicy::Sequential,
                    json!(42)
                ),
            )
            .is_err()
        );
        assert!(
            register(
                &mut registry,
                definition(
                    "tool-a",
                    "alpha",
                    ToolExecutionPolicy::ForegroundOnly,
                    ToolConcurrencyPolicy::Sequential,
                    json!({"type": "object", "properties": {"__rustx_secret": {"type": "string"}}})
                ),
            )
            .is_err()
        );
    }

    /// The runtime intrinsic `background_task` cannot be background-capable.
    #[test]
    fn background_task_cannot_be_background_capable() {
        let mut registry = ToolRegistry::new();
        let error = register(
            &mut registry,
            definition(
                BACKGROUND_TASK_TOOL_NAME,
                BACKGROUND_TASK_TOOL_NAME,
                ToolExecutionPolicy::BackgroundOnly,
                ToolConcurrencyPolicy::Sequential,
                object_schema(),
            ),
        )
        .expect_err("background-only intrinsic rejected");
        assert!(matches!(error, ToolRegistryError::InvalidPolicy(_)));
        let error = register(
            &mut registry,
            definition(
                BACKGROUND_TASK_TOOL_NAME,
                BACKGROUND_TASK_TOOL_NAME,
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Parallel,
                object_schema(),
            ),
        )
        .expect_err("parallel intrinsic rejected");
        assert!(matches!(error, ToolRegistryError::InvalidPolicy(_)));
        register(
            &mut registry,
            definition(
                BACKGROUND_TASK_TOOL_NAME,
                BACKGROUND_TASK_TOOL_NAME,
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                object_schema(),
            ),
        )
        .expect("fixed intrinsic policy is accepted");
    }

    /// A valid foreground model-selectable selection is extracted and
    /// stripped before dispatch.
    #[test]
    fn valid_foreground_selection_is_preflighted() {
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-sel",
                "sel",
                ToolExecutionPolicy::ModelSelectable,
                ToolConcurrencyPolicy::Sequential,
                json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"], "additionalProperties": false}),
            ),
        )
        .expect("register");
        let outcome = registry
            .preflight(&call(
                "tool-sel",
                "sel",
                json!({"__rustx_execution": "foreground", "path": "a.txt"}),
            ))
            .expect("preflight");
        let PreflightOutcome::Ready(prepared) = outcome else {
            panic!("expected ready");
        };
        assert_eq!(prepared.invocation.mode, ToolInvocationMode::Foreground);
        assert_eq!(prepared.invocation.arguments, json!({"path": "a.txt"}));
        assert!(
            !prepared
                .invocation
                .arguments
                .as_object()
                .is_some_and(|object| object.contains_key("__rustx_execution")),
            "the synthetic field never reaches the executor"
        );
    }

    /// A valid background selection is extracted and stripped.
    #[test]
    fn valid_background_selection_is_preflighted() {
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-sel",
                "sel",
                ToolExecutionPolicy::ModelSelectable,
                ToolConcurrencyPolicy::Sequential,
                json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"], "additionalProperties": false}),
            ),
        )
        .expect("register");
        let outcome = registry
            .preflight(&call(
                "tool-sel",
                "sel",
                json!({"__rustx_execution": "background", "command": "ls"}),
            ))
            .expect("preflight");
        let PreflightOutcome::Ready(prepared) = outcome else {
            panic!("expected ready");
        };
        assert_eq!(prepared.invocation.mode, ToolInvocationMode::Background);
        assert_eq!(prepared.invocation.arguments, json!({"command": "ls"}));
    }

    /// Missing or invalid model-selectable fields are rejected without an
    /// executor call.
    #[test]
    fn missing_or_invalid_model_selectable_field_is_rejected() {
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-sel",
                "sel",
                ToolExecutionPolicy::ModelSelectable,
                ToolConcurrencyPolicy::Sequential,
                json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"], "additionalProperties": false}),
            ),
        )
        .expect("register");
        let missing = registry
            .preflight(&call("tool-sel", "sel", json!({"path": "a.txt"})))
            .expect("preflight outcome");
        let PreflightOutcome::Rejected { error, .. } = missing else {
            panic!("expected rejection");
        };
        assert!(error.contains("__rustx_execution"));
        let invalid = registry
            .preflight(&call(
                "tool-sel",
                "sel",
                json!({"__rustx_execution": "sideways", "path": "a.txt"}),
            ))
            .expect("preflight outcome");
        assert!(matches!(invalid, PreflightOutcome::Rejected { .. }));
    }

    /// Business schema violations are rejected without an executor call.
    #[test]
    fn business_schema_violation_is_rejected() {
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-read",
                "read",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"], "additionalProperties": false}),
            ),
        )
        .expect("register");
        let outcome = registry
            .preflight(&call("tool-read", "read", json!({"path": 42})))
            .expect("preflight outcome");
        let PreflightOutcome::Rejected { error, .. } = outcome else {
            panic!("expected rejection");
        };
        assert!(!error.is_empty());
        let missing = registry
            .preflight(&call("tool-read", "read", json!({})))
            .expect("preflight outcome");
        assert!(matches!(missing, PreflightOutcome::Rejected { .. }));
    }

    /// ID/name disagreement is a structural contract violation, never an
    /// ID-first fallback.
    #[test]
    fn id_name_mismatch_fails_instead_of_falling_back() {
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-a",
                "alpha",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                object_schema(),
            ),
        )
        .expect("register");
        assert!(matches!(
            registry.preflight(&call("tool-a", "beta", json!({}))),
            Err(ToolPreflightError::IdentityMismatch { .. })
        ));
        assert!(matches!(
            registry.preflight(&call("tool-b", "alpha", json!({}))),
            Err(ToolPreflightError::IdentityMismatch { .. })
        ));
        assert!(matches!(
            registry.preflight(&call("nope", "nope", json!({}))),
            Err(ToolPreflightError::UnknownTool { .. })
        ));
    }

    /// Model definitions follow registration order deterministically.
    #[test]
    fn model_definitions_follow_registration_order() {
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-a",
                "alpha",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Parallel,
                object_schema(),
            ),
        )
        .expect("register a");
        register(
            &mut registry,
            definition(
                "tool-b",
                "beta",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                object_schema(),
            ),
        )
        .expect("register b");
        let names: Vec<String> = registry
            .model_definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    /// The canonical schema is unchanged by model compilation.
    #[test]
    fn canonical_schema_is_unchanged_after_compilation() {
        let schema = json!({"type": "object", "properties": {"path": {"type": "string"}}});
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-sel",
                "sel",
                ToolExecutionPolicy::ModelSelectable,
                ToolConcurrencyPolicy::Sequential,
                schema.clone(),
            ),
        )
        .expect("register");
        assert_eq!(registry.definitions()[0].input_schema, schema);
        let compiled = registry.model_definitions()[0].input_schema.clone();
        assert!(
            compiled["properties"]["__rustx_execution"].is_object(),
            "the compiled schema carries the synthetic field"
        );
        assert_eq!(
            registry.definitions()[0].input_schema,
            schema,
            "the canonical schema stays verbatim"
        );
    }

    /// Preflight rejects reserved invocation arguments for fixed policies.
    #[test]
    fn fixed_policies_reject_reserved_invocation_fields() {
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            definition(
                "tool-a",
                "alpha",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                object_schema(),
            ),
        )
        .expect("register");
        let outcome = registry
            .preflight(&call(
                "tool-a",
                "alpha",
                json!({"__rustx_execution": "foreground"}),
            ))
            .expect("preflight outcome");
        let PreflightOutcome::Rejected { error, .. } = outcome else {
            panic!("expected rejection");
        };
        assert!(error.contains("__rustx_"));
    }

    /// An execution context can be constructed for a registered tool; the
    /// executor receives the stripped invocation verbatim. This drives
    /// [`ToolExecutor::execute`] directly through the registry.
    ///
    #[test]
    fn executor_receives_no_synthetic_runtime_fields() {
        use super::{ToolExecutionContext, ToolExecutor};
        struct Capturing;
        impl super::ProgressReporter for Capturing {
            fn report(&self, _progress: ToolProgress) {}
        }
        struct CaptureExecutor {
            received: Arc<Mutex<Vec<serde_json::Value>>>,
        }
        impl ToolExecutor for CaptureExecutor {
            fn execute<'a>(
                &'a self,
                invocation: ToolInvocation,
                _context: ToolExecutionContext<'a>,
            ) -> BoxFuture<'a, ToolExecutionResult> {
                self.received
                    .lock()
                    .expect("lock")
                    .push(invocation.arguments.clone());
                Box::pin(ready(ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                }))
            }
        }
        let dir = std::env::temp_dir().join(format!(
            "rustx-reg-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker")
        ));
        let _ = std::fs::create_dir_all(&dir);
        let received = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        registry
            .register(
                definition(
                    "tool-sel",
                    "sel",
                    ToolExecutionPolicy::ModelSelectable,
                    ToolConcurrencyPolicy::Sequential,
                    json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"], "additionalProperties": false}),
                ),
                Arc::new(CaptureExecutor {
                    received: received.clone(),
                }),
            )
            .expect("register");
        let outcome = registry
            .preflight(&call(
                "tool-sel",
                "sel",
                json!({"__rustx_execution": "background", "path": "a.txt"}),
            ))
            .expect("preflight");
        let PreflightOutcome::Ready(prepared) = outcome else {
            panic!("expected ready");
        };
        let workspace = Workspace::new(&dir).expect("workspace");
        let artifacts = ArtifactStore::new(
            crate::runtime::identity::ConversationId::new("conv-1"),
            dir.join("artifacts"),
        )
        .expect("store");
        let tool_output = crate::tools::managed_output::ManagedToolOutput::new(
            crate::runtime::identity::ConversationId::new("conv-1"),
            dir.join("tool-output"),
        )
        .expect("managed tool output");
        let reporter = Capturing;
        let context = ToolExecutionContext {
            conversation_id: &ConversationId::new("conv-1"),
            execution_id: None,
            cancellation: crate::runtime::cancellation::ExecutionCancellation::detached(
                crate::runtime::cancellation::CancellationSignal::new(),
                crate::runtime::types::CancellationReason::UserRequested,
            ),
            workspace: &workspace,
            progress: &reporter,
            artifacts: &artifacts,
            tool_output: &tool_output,
            environment: &ToolEnvironment::new(),
        };
        let executor = registry.executor(&prepared.invocation.tool_id);
        let _result = executor
            .execute(prepared.invocation.clone(), context)
            .now_or_never()
            .expect("stub executors resolve immediately");
        assert_eq!(
            *received.lock().expect("lock"),
            vec![json!({"path": "a.txt"})],
            "the executor receives only the stripped business arguments"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
