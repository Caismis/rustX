//! Native, provider-independent scoped Workflow programs (Issues #83/#217).
//!
//! YAML is only the serialization format at this boundary. The loader turns a
//! configured workflow file into [`WorkflowDefinition`], the compiler checks
//! the finite graph and every explicit value reference, and the immutable
//! [`WorkflowProgram`] is the only representation execution code consumes.
//! Dynamic ownership and orchestration state belong to `WorkflowRuntime`;
//! child model execution remains in the native subagent runtime.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::durable::ConversationStore;
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
#[cfg(test)]
use crate::runtime::identity::SubagentId;
use crate::runtime::identity::{EventId, ToolCallId};
pub use crate::tools::executor::WORKFLOW_OUTPUT_TOOL_NAME;
pub use rustx_types::runtime::workflow::WorkflowPort;

use super::subagent::SubagentName;

mod admission;
pub mod authoring;
mod code;
pub use authoring::{WorkflowModelPolicy, WorkflowToolSelector};
pub use code::WorkflowCodeLanguage;

/// Internal entry point of the resource-bounded Template renderer binary.
#[doc(hidden)]
#[must_use]
pub fn run_template_renderer() -> i32 {
    transform::run_worker()
}
mod defaults;
pub(crate) use defaults::apply_schema_defaults;
mod execution;
mod flow;
mod transform;
pub use transform::{
    WorkflowAggregateGroup, WorkflowListFilter, WorkflowListOperator, WorkflowListSort,
    WorkflowSortOrder,
};
pub mod inspection;
pub mod read_model;
mod references;
mod review;
pub mod scheduled;
pub mod snapshot;
pub use review::{MAX_REVIEW_ACTIONS, REVIEW_TIMEOUT_PORT, WorkflowReviewAction};
pub use snapshot::{
    WORKFLOW_SNAPSHOT_CHUNK_BYTES, WorkflowRunDefinitionSnapshot, WorkflowRunSnapshotChunk,
    WorkflowRunSnapshotHeader,
};
mod expressions;
pub(crate) mod model;
pub(crate) mod tool;
use expressions::{
    evaluate_predicate, evaluate_value, valid_local_key, validate_predicate, value_schema,
};
pub use tool::{WorkflowToolChoice, WorkflowToolResult, selectable_builtin_tools};

pub use rustx_types::runtime::workflow::{
    LOOP_NODE_MAX_COUNT, MAX_BLOCK_DEPTH, MAX_LOCAL_BYTES, MAX_MODEL_MESSAGE_BYTES,
    MAX_MODEL_MESSAGE_PARTS, MAX_MODEL_MESSAGES, MAX_PARALLEL_LIMIT, MAX_REFERENCE_COMPONENTS,
    MAX_RUN_ID_COMPONENT_BYTES, MAX_VALUE_BYTES, MAX_VALUE_DEPTH, MAX_WORKFLOW_AGENTS,
    MAX_WORKFLOW_BYTES, MAX_WORKFLOW_DEFINITIONS, MAX_WORKFLOW_DIAGNOSTIC_BYTES,
    MAX_WORKFLOW_MODEL_CALLS, MAX_WORKFLOW_NODES, MAX_WORKFLOW_STEPS, WORKFLOW_TOOL_ID_PREFIX,
    WorkflowBlockInstance, WorkflowDefinitionPath, WorkflowExecutionOutcome, WorkflowId,
    WorkflowIdError, WorkflowLimit, WorkflowLoopExit, WorkflowNodeInstance, WorkflowRunId,
    WorkflowRunOrigin, WorkflowToolIdentity,
};
#[cfg(test)]
pub(crate) use rustx_types::runtime::workflow::{test_body, test_instance};
pub use rustx_workflow::definition::*;
pub use rustx_workflow::nav::*;

/// A compiled, immutable executable workflow.
#[derive(Debug, Clone)]
pub struct WorkflowProgram {
    digest: String,
    id: WorkflowId,
    description: String,
    block: WorkflowBlockProgram,
    execution_bound: usize,
    agent_bound: usize,
    model_call_bound: usize,
    retained_bound: usize,
    tools: BTreeSet<crate::capabilities::selection::ExactToolSelector>,
    timeout_ms: u64,
    frozen_tools: BTreeMap<
        crate::capabilities::selection::ExactToolSelector,
        crate::tools::types::ToolDefinition,
    >,
    permissions: Option<crate::runtime::permission::ResourcePermissionSet>,
    snapshot: Arc<WorkflowRunDefinitionSnapshot>,
}

/// Immutable compiled graph of the root or of a body; a body's path is not empty.
#[derive(Debug, Clone)]
pub struct WorkflowBlockProgram {
    path: Vec<String>,
    input_schema: Value,
    output_schema: Value,
    entry: String,
    nodes: BTreeMap<String, WorkflowNodeProgram>,
    outgoing: BTreeMap<String, Vec<WorkflowEdgeProgram>>,
}

impl WorkflowProgram {
    /// The admitted total wall-clock budget. Every step consumes this budget.
    #[must_use]
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// Immutable definition + same-revision presentation snapshot captured when
    /// this program was compiled/admitted.
    #[must_use]
    pub fn run_snapshot(&self) -> &Arc<WorkflowRunDefinitionSnapshot> {
        &self.snapshot
    }

    /// The configured identity.
    #[must_use]
    pub fn id(&self) -> &WorkflowId {
        &self.id
    }

    /// Bounded immutable identity, with no executable state.
    #[must_use]
    pub fn tool_identity(&self) -> WorkflowToolIdentity {
        WorkflowToolIdentity {
            workflow_id: self.id.clone(),
            program_digest: self.digest.clone(),
        }
    }

    /// The model-facing description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The frozen workflow Tool resource permission object, when authored.
    #[must_use]
    pub(crate) const fn permissions(
        &self,
    ) -> Option<crate::runtime::permission::ResourcePermissionSet> {
        self.permissions
    }

    /// The immutable input schema.
    #[must_use]
    pub fn input_schema(&self) -> &Value {
        &self.block.input_schema
    }

    /// The immutable output schema.
    #[must_use]
    pub fn output_schema(&self) -> &Value {
        &self.block.output_schema
    }

    /// The explicit entry node.
    #[must_use]
    pub fn entry(&self) -> &str {
        &self.block.entry
    }

    /// The compiled nodes in deterministic id order.
    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<String, WorkflowNodeProgram> {
        &self.block.nodes
    }

    /// The leaf Tool definitions its direct Tool nodes admitted.
    pub(crate) fn frozen_tools(
        &self,
    ) -> impl Iterator<Item = &crate::tools::types::ToolDefinition> {
        self.frozen_tools.values()
    }

    pub(crate) fn agent_nodes(&self) -> Vec<&WorkflowAgentProgram> {
        fn walk<'a>(block: &'a WorkflowBlockProgram, found: &mut Vec<&'a WorkflowAgentProgram>) {
            for node in block.nodes.values() {
                match node {
                    WorkflowNodeProgram::Agent(agent) => found.push(agent),
                    WorkflowNodeProgram::Loop { body, .. }
                    | WorkflowNodeProgram::Iteration { body, .. } => walk(body, found),
                    _ => {}
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.block, &mut found);
        found
    }

    /// The compiled outgoing edges of a node.
    #[must_use]
    pub fn outgoing(&self, node: &str) -> &[WorkflowEdgeProgram] {
        self.block.outgoing.get(node).map_or(&[], Vec::as_slice)
    }
}

/// A compiled node whose references and schemas have been admitted.
#[derive(Debug, Clone)]
pub enum WorkflowNodeProgram {
    Start,
    Template {
        variables: BTreeMap<String, WorkflowValue>,
        template: String,
    },
    List {
        input: WorkflowValue,
        filter: Option<WorkflowListFilter>,
        sort: Option<WorkflowListSort>,
        limit: Option<u32>,
        extract: Option<WorkflowValue>,
        output_schema: Value,
    },
    Aggregate {
        grouped: bool,
        groups: Vec<WorkflowAggregateGroup>,
        output_schema: Value,
    },
    /// One immutable, tool-free Auxiliary Call template.
    Model(WorkflowModelProgram),
    Loop {
        loop_variables: Vec<WorkflowLoopVariable>,
        break_condition: Option<WorkflowPredicate>,
        loop_count: u32,
        body: Box<WorkflowBlockProgram>,
        output_schema: Value,
    },
    Iteration {
        iterator: WorkflowValue,
        output: WorkflowValue,
        /// Rounds run at once; one when sequential.
        parallel: u32,
        error_handle_mode: WorkflowIterationErrorMode,
        flatten_output: bool,
        body: Box<WorkflowBlockProgram>,
        output_schema: Value,
    },
    LoopEnd,
    Assigner {
        items: Vec<WorkflowAssignment>,
        /// Each written variable's type.
        types: Vec<WorkflowVarType>,
    },
    Review {
        content: Vec<WorkflowValue>,
        actions: Vec<WorkflowReviewAction>,
        timeout_ms: u64,
    },
    Tool {
        selector: crate::capabilities::selection::ExactToolSelector,
        arguments: tool::WorkflowToolArguments,
        result: tool::WorkflowToolProjection,
        retry: Option<WorkflowRetry>,
        on_error: WorkflowValueErrorStrategy,
    },
    /// One admitted `AgentRun` template.
    Agent(WorkflowAgentProgram),
    /// The first matching IF / ELIF case, or ELSE.
    Branch {
        cases: Vec<WorkflowBranchCase>,
    },
    /// The terminal Return operation.
    Return {
        output: WorkflowValue,
    },
}

/// Static Model intent; actual invocation resolution uses frozen Attempt resources.
#[derive(Debug, Clone)]
pub struct WorkflowModelProgram {
    /// Classifier exits; ordinary Model calls have none.
    pub classes: Vec<WorkflowClassifierClass>,
    pub model: crate::model::session::DirectModelPolicy,
    pub messages: Vec<WorkflowModelMessage>,
    pub output: WorkflowModelOutput,
    pub retry: Option<WorkflowRetry>,
    pub on_error: WorkflowModelErrorStrategy,
}

/// A compiled `AgentRun` template.
#[derive(Debug, Clone)]
pub struct WorkflowAgentProgram {
    pub(crate) resolved: Option<Arc<crate::runtime::subagent::resolver::FrozenAgentComposition>>,
    /// The admitted native profile.
    pub profile: SubagentName,
    /// The task's fixed text and references.
    pub task: Vec<WorkflowValue>,
    /// Frozen output contract.
    pub output: WorkflowAgentOutput,
    /// The exits the child chooses among; empty for one `next` exit.
    pub routes: Vec<WorkflowAgentRoute>,
    /// Frozen Agent-entry resource permission object.
    pub resource_permissions: Option<crate::runtime::permission::ResourcePermissionSet>,
    pub on_error: WorkflowValueErrorStrategy,
}

impl WorkflowAgentProgram {
    /// The schema of the child's `workflow_output` terminal, when it has one.
    #[must_use]
    pub fn terminal_schema(&self) -> Option<Value> {
        agent_terminal_schema(&self.output, &self.routes)
    }

    /// The node's value contract on its success exits.
    #[must_use]
    pub fn value_schema(&self) -> Value {
        agent_value_schema(&self.output, &self.routes)
    }

    /// The node value of one settled child: its reply text, or its
    /// committed value.
    fn value(&self, settled: Value) -> Option<Value> {
        match (self.terminal_schema(), settled) {
            (None, Value::String(text)) => Some(serde_json::json!({"text": text})),
            (Some(_), value) if value.is_object() => Some(value),
            _ => None,
        }
    }

    /// The route a validated routed value chose.
    pub(super) fn chosen_route(value: &Value) -> WorkflowPort {
        WorkflowPort::case(value[AGENT_ROUTE_FIELD].as_str().unwrap_or_default())
    }
}

/// A compiled edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowEdgeProgram {
    /// Destination id.
    pub to: String,
    /// The port used by this edge.
    pub port: WorkflowPort,
}

mod catalog;
mod compiler;
pub use catalog::{
    WorkflowAdmission, WorkflowAdmissionDiagnostic, WorkflowCatalog, WorkflowCatalogError,
    WorkflowDependencyFailure, WorkflowEntry,
};
use compiler::{
    SchemaMap, SchemaProofStep, control_flow, finite_schema_values, is_reserved_port,
    schema_forbids_property, schema_type, schemas_compatible, schemas_compatible_with,
    validate_agent_output, validate_model_output, validate_model_policy, validate_prompt,
    validate_root_schema, validate_tool_arguments_root, validate_workflow_schema,
};
pub use rustx_workflow::compile_error::WorkflowCompileError;

/// The result of the reserved `workflow_output` terminal protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowOutputSubmission {
    /// The value passed the frozen Agent contract and was committed.
    Committed,
    /// The value was rejected without changing terminal state.
    Invalid(String),
    /// A valid output arrived after output or cancellation had already won.
    Stale,
}

/// The one terminal-output authority of a Workflow-owned `AgentRun`.
///
/// This is deliberately not a [`ToolExecutor`](crate::tools::executor::ToolExecutor). The model may see a
/// tool-shaped `workflow_output` declaration, but the call is consumed by
/// the Agent Loop before ordinary Tool Plane preflight or dispatch.
pub trait WorkflowOutputTerminal: Send + Sync {
    /// The frozen Agent output schema shown to the child model.
    fn output_schema(&self) -> Value;
    /// Attempts the validate-and-commit transition.
    fn submit(&self, value: Value) -> WorkflowOutputSubmission;
    /// Attempts the cancellation transition. A committed output cannot be
    /// rewritten by a later cancellation.
    fn cancel(&self, reason: crate::runtime::types::CancellationReason) -> bool;
}

#[derive(Debug)]
enum WorkflowOutputState {
    Pending,
    Committed(Value),
    Cancelled(crate::runtime::types::CancellationReason),
}

/// How one Workflow-owned child `AgentRun` settles: its final reply is the
/// result, or its committed `workflow_output` value is.
#[derive(Clone)]
pub(crate) enum WorkflowChildTerminal {
    Text,
    Output(Arc<WorkflowOutputLatch>),
}

impl WorkflowChildTerminal {
    pub(crate) fn latch(&self) -> Option<&Arc<WorkflowOutputLatch>> {
        match self {
            Self::Text => None,
            Self::Output(latch) => Some(latch),
        }
    }
}

/// A thread-safe, frozen-schema terminal latch used by one child `AgentRun`.
pub struct WorkflowOutputLatch {
    schema: Value,
    validator: jsonschema::Validator,
    state: std::sync::Mutex<WorkflowOutputState>,
}

impl fmt::Debug for WorkflowOutputLatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkflowOutputLatch")
            .field("schema", &self.schema)
            .field(
                "state",
                &self
                    .state
                    .lock()
                    .map_or_else(|_| "poisoned".to_owned(), |state| format!("{state:?}")),
            )
            .finish_non_exhaustive()
    }
}

impl WorkflowOutputLatch {
    /// Creates a latch over one already compiler-validated Agent schema.
    ///
    /// # Errors
    ///
    /// Returns an error if the supplied schema cannot be compiled by the
    /// Workflow v1 compiler and JSON Schema validator.
    pub fn new(schema: Value) -> Result<Self, String> {
        validate_root_schema(&schema, "Workflow Agent output")
            .map_err(|error| error.to_string())?;
        let validator = jsonschema::Validator::new(&schema)
            .map_err(|error| format!("invalid workflow Agent output schema: {error}"))?;
        Ok(Self {
            schema,
            validator,
            state: std::sync::Mutex::new(WorkflowOutputState::Pending),
        })
    }

    /// Returns the committed value, if output won the terminal race.
    ///
    /// # Panics
    ///
    /// Panics if the latch mutex is poisoned by a prior panic while holding
    /// it; a poisoned latch cannot safely establish terminal authority.
    #[must_use]
    pub fn committed_value(&self) -> Option<Value> {
        let state = self.state.lock().expect("workflow output latch lock");
        match &*state {
            WorkflowOutputState::Committed(value) => Some(value.clone()),
            WorkflowOutputState::Pending => None,
            WorkflowOutputState::Cancelled(reason) => {
                let _ = reason;
                None
            }
        }
    }
}

impl WorkflowOutputTerminal for WorkflowOutputLatch {
    fn output_schema(&self) -> Value {
        self.schema.clone()
    }

    fn submit(&self, value: Value) -> WorkflowOutputSubmission {
        let mut state = self.state.lock().expect("workflow output latch lock");
        if !matches!(*state, WorkflowOutputState::Pending) {
            return WorkflowOutputSubmission::Stale;
        }
        let serialized_size = match serde_json::to_vec(&value) {
            Ok(serialized) => serialized.len(),
            Err(_) => {
                return WorkflowOutputSubmission::Invalid(
                    "workflow_output must be JSON-serializable".to_owned(),
                );
            }
        };
        if serialized_size > crate::runtime::subagent::MAX_RESULT_CONTENT_BYTES {
            return WorkflowOutputSubmission::Invalid(
                "workflow_output exceeds the bounded value size".to_owned(),
            );
        }
        if !self.validator.is_valid(&value) {
            return WorkflowOutputSubmission::Invalid(
                "workflow_output does not satisfy the frozen Agent output schema".to_owned(),
            );
        }
        *state = WorkflowOutputState::Committed(value);
        WorkflowOutputSubmission::Committed
    }

    fn cancel(&self, reason: crate::runtime::types::CancellationReason) -> bool {
        let mut state = self.state.lock().expect("workflow output latch lock");
        if !matches!(*state, WorkflowOutputState::Pending) {
            return false;
        }
        *state = WorkflowOutputState::Cancelled(reason);
        true
    }
}

/// The terminal state of a dynamic workflow run.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowTerminalState {
    /// One validated workflow output was committed.
    Completed(Value),
    /// The workflow failed before producing a result.
    Failed(WorkflowRunError),
    /// Cancellation won the workflow terminal race.
    Cancelled(crate::runtime::types::CancellationReason),
}

/// Dynamic execution state and ownership of one immutable program run.
///
/// A run owns only workflow-local values, deterministic control-flow
/// progression, admitted child identities, and terminal settlement. It does
/// not own model execution, tool execution, workspaces, or a second
/// scheduler.
pub struct WorkflowRun {
    program: Arc<WorkflowProgram>,
    model_invocations: BTreeMap<Vec<String>, crate::model::ResolvedModelInvocation>,
    run_id: WorkflowRunId,
    budgets: std::sync::Mutex<execution::RunBudgets>,
    effects: Option<Arc<scheduled::effects::ScheduledEffects>>,
    terminal: Option<WorkflowTerminalState>,
    tools: BTreeMap<
        crate::capabilities::selection::ExactToolSelector,
        crate::tools::types::ToolDefinition,
    >,
}

impl fmt::Debug for WorkflowRun {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkflowRun")
            .field("program", &self.program.id())
            .field("run_id", &self.run_id)
            .field("budgets", &self.budgets)
            .field("terminal", &self.terminal)
            .field("tools", &self.tools)
            .finish_non_exhaustive()
    }
}

impl WorkflowRun {
    fn new(program: Arc<WorkflowProgram>, run_id: WorkflowRunId) -> Self {
        let budgets = execution::RunBudgets::reserved(program.retained_bound);
        Self {
            program,
            model_invocations: BTreeMap::new(),
            run_id,
            budgets: std::sync::Mutex::new(budgets),
            effects: None,
            terminal: None,
            tools: BTreeMap::new(),
        }
    }

    /// The immutable program snapshot this run owns.
    #[must_use]
    pub fn program(&self) -> &Arc<WorkflowProgram> {
        &self.program
    }

    /// The terminal settlement, once committed.
    #[must_use]
    pub fn terminal(&self) -> Option<&WorkflowTerminalState> {
        self.terminal.as_ref()
    }

    fn settle(&mut self, terminal: WorkflowTerminalState) -> Result<(), WorkflowRunError> {
        if self.terminal.is_some() {
            return Err(WorkflowRunError::TerminalAlreadySettled);
        }
        self.terminal = Some(terminal);
        Ok(())
    }
}

/// The native Workflow orchestrator over the existing `SubagentRegistry`.
#[derive(Clone)]
pub struct WorkflowRuntime {
    read_model: read_model::WorkflowReadModel,
    subagents: crate::runtime::subagent::SubagentRegistry,
    /// The existing conversation Event Journal. Workflow lifecycle facts are
    /// best-effort observability here; the journal never becomes the
    /// `WorkflowRun` state authority.
    event_store: Arc<dyn ConversationStore>,
    next_run: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(test)]
    node_frontier: Arc<std::sync::Mutex<Option<execution::NodeFrontierHook>>>,
    #[cfg(test)]
    iteration_frontier: Arc<std::sync::Mutex<Option<execution::IterationFrontierHook>>>,
    #[cfg(test)]
    observations: tokio::sync::watch::Sender<Vec<RuntimeEvent>>,
}

impl fmt::Debug for WorkflowRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkflowRuntime")
            .field("subagents", &"native SubagentRegistry")
            .field("event_store", &"conversation Event Journal")
            .finish()
    }
}

/// Derives a stable, conversation-local identity for a Workflow fact.
///
/// Workflow events are best-effort observability evidence rather than
/// execution authority. The complete event payload includes the run and node
/// identities, so hashing it prevents distinct facts from colliding while
/// keeping the event ID bounded.
fn workflow_event_id(event: &RuntimeEvent) -> EventId {
    let encoded = serde_json::to_vec(event).expect("Workflow runtime events are serializable");
    let digest = Sha256::digest(encoded);
    EventId::new(format!("workflow-event:{digest:x}"))
}

impl WorkflowRuntime {
    /// Creates the workflow orchestrator over one native child registry.
    #[must_use]
    pub fn new(
        subagents: crate::runtime::subagent::SubagentRegistry,
        event_store: Arc<dyn ConversationStore>,
        read_model: read_model::WorkflowReadModel,
    ) -> Self {
        Self {
            read_model,
            subagents,
            event_store,
            next_run: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            #[cfg(test)]
            node_frontier: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            iteration_frontier: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            observations: tokio::sync::watch::Sender::new(Vec::new()),
        }
    }

    #[cfg(test)]
    async fn run_test_foreground(
        &self,
        program: Arc<WorkflowProgram>,
        run_id: ToolCallId,
        context: crate::runtime::subagent::NativeExecutionContext,
        input: Value,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let context = context.with_test_workflow(&program);
        self.run_foreground(program.id().clone(), run_id, context, input, cancellation)
            .await
    }

    /// Executes one immutable foreground Workflow program.
    ///
    /// The returned value is suitable for the parent Workflow `ToolResult`.
    /// Intermediate values and child transcripts remain inside this run and
    /// never enter the parent canonical conversation.
    ///
    /// # Errors
    ///
    /// Returns a [`WorkflowRunError`] when input, child execution, control
    /// flow, output validation, or cancellation prevents successful
    /// settlement.
    #[allow(clippy::too_many_lines)] // One admission and terminal settlement protocol.
    pub async fn run_foreground(
        &self,
        workflow: WorkflowId,
        run_id: ToolCallId,
        context: crate::runtime::subagent::NativeExecutionContext,
        input: Value,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let program = context
            .resources()
            .workflows()
            .executable(&workflow)?
            .clone();
        let attempt_id = context.attempt_id().ok_or_else(|| {
            WorkflowRunError::InvocationAuthority(
                "foreground Workflow requires a Tool Attempt".into(),
            )
        })?;
        if attempt_id.as_str().is_empty() || attempt_id.as_str().len() > MAX_RUN_ID_COMPONENT_BYTES
        {
            return Err(WorkflowRunError::InvalidInput(
                "native run identity exceeds its bound".into(),
            ));
        }
        let invocation = self
            .next_run
            .try_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| n.checked_add(1),
            )
            .map_err(|_| {
                WorkflowRunError::InvalidProgram("Workflow invocation identity exhausted".into())
            })?;
        let native_id = WorkflowRunId {
            conversation_id: self.event_store.conversation_id().clone(),
            origin: attempt_id.clone().into(),
            invocation,
        };
        self.run_native(
            program,
            native_id,
            Some(run_id),
            context,
            input,
            cancellation,
            None,
        )
        .await
    }

    pub(crate) async fn run_scheduled(
        &self,
        claim: scheduled::lifecycle::ClaimedScheduledWorkflow,
        prepared: scheduled::PreparedScheduledWorkflow,
        context: crate::runtime::subagent::NativeExecutionContext,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        if context.origin() != &claim.start.head.run_id.origin
            || prepared.contract().binding != claim.start.head.binding
        {
            return Err(WorkflowRunError::InvocationAuthority(
                "scheduled origin or binding mismatch".into(),
            ));
        }
        let input =
            claim.start.arguments.clone().ok_or_else(|| {
                WorkflowRunError::InvalidInput("scheduled arguments missing".into())
            })?;
        let run_id = claim.start.head.run_id.clone();
        let tracker = std::sync::Arc::new(scheduled::effects::ScheduledEffects::new(
            self.event_store.clone(),
            claim,
        ));
        self.run_native(
            prepared.program().clone(),
            run_id,
            None,
            context,
            input,
            cancellation,
            Some(tracker),
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_native(
        &self,
        program: Arc<WorkflowProgram>,
        native_id: WorkflowRunId,
        tool_call_id: Option<ToolCallId>,
        context: crate::runtime::subagent::NativeExecutionContext,
        input: Value,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
        effects: Option<Arc<scheduled::effects::ScheduledEffects>>,
    ) -> Result<Value, WorkflowRunError> {
        #[cfg(test)]
        crate::local_runtime::static_effects::observe(
            crate::local_runtime::static_effects::Effect::WorkflowRun,
        );
        let model_invocations = model::freeze_models(&program.block, &context)?;
        let mut run = WorkflowRun::new(program.clone(), native_id);
        run.effects = effects;
        run.model_invocations = model_invocations;
        self.read_model.register(read_model::WorkflowRunView {
            id: run.run_id.clone(),
            workflow_id: program.id.clone(),
            program_digest: program.digest.clone(),
            resource_revision: context.resources().revision(),
            tool_call_id: tool_call_id.clone(),
            state: read_model::WorkflowState::Running,
            instances: Vec::new(),
            omitted_instances: 0,
            steps_consumed: 0,
            steps_max: program.execution_bound,
            agents_consumed: 0,
            model_calls_consumed: 0,
        });
        self.emit_workflow_start_observability(
            RuntimeEvent::WorkflowStarted {
                tool_call_id,
                workflow_id: program.id().clone(),
                run_id: run.run_id.clone(),
            },
            program.run_snapshot(),
        );
        let execution = {
            let id = run.run_id.clone();
            let execute = async {
                run.tools = program.frozen_tools.clone();
                // The run executes against the invoking Session's execution
                // binding: there is no run-owned workspace to acquire, borrow
                // or settle. Every node settles its own native work inline.
                let execution =
                    match execution::validate_commit(&program.block.input_schema, &input) {
                        Ok(()) => self
                            .execute_block(
                                &run,
                                &program.block,
                                &context,
                                input,
                                vec![0],
                                &cancellation,
                            )
                            .await
                            .map(|output| output.value),
                        Err(error) => Err(error),
                    };
                self.read_model.update(&run.run_id, |view| {
                    if !matches!(view.state, read_model::WorkflowState::Draining) {
                        view.state = read_model::WorkflowState::Waiting {
                            reason: read_model::WorkflowWait::Settlement,
                        };
                    }
                });
                execution
            };
            tokio::pin!(execute);
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    self.read_model.draining(&id);
                    execute.await
                }
                result = &mut execute => result,
            }
        };
        // Run terminal frontier. The shared block executor has already
        // validated its result and settled all owned native work. No await
        // separates this cancellation observation from the unique run commit.
        context.check_workflow_deadline(&cancellation);
        let execution = match execution {
            Ok(_) if cancellation.is_cancelled() => {
                Err(WorkflowRunError::from_cancellation(&cancellation))
            }
            result => result,
        };
        match execution {
            Ok(value) => {
                run.settle(WorkflowTerminalState::Completed(value.clone()))?;
                self.read_model.update(&run.run_id, |view| {
                    view.state = read_model::WorkflowState::Settled {
                        outcome: WorkflowExecutionOutcome::Completed,
                    }
                });
                self.emit_observability(
                    &run,
                    RuntimeEvent::WorkflowCompleted {
                        workflow_id: program.id().clone(),
                        run_id: run.run_id.clone(),
                    },
                );
                // The semantic output is the validated output of all active Returns.
                Ok(value)
            }
            Err(error) => {
                let terminal = match &error {
                    WorkflowRunError::Cancelled(reason) => {
                        WorkflowTerminalState::Cancelled(*reason)
                    }
                    _ => WorkflowTerminalState::Failed(error.clone()),
                };
                run.settle(terminal)?;
                self.read_model.update(&run.run_id, |view| {
                    view.state = read_model::WorkflowState::Settled {
                        outcome: execution::outcome::<()>(&Err(error.clone())),
                    }
                });
                match &error {
                    WorkflowRunError::Cancelled(reason) => self.emit_observability(
                        &run,
                        RuntimeEvent::WorkflowCancelled {
                            workflow_id: program.id().clone(),
                            run_id: run.run_id.clone(),
                            reason: *reason,
                        },
                    ),
                    _ => self.emit_observability(
                        &run,
                        RuntimeEvent::WorkflowFailed {
                            workflow_id: program.id().clone(),
                            run_id: run.run_id.clone(),
                            diagnostic: bound_workflow_text(error.to_string()),
                            status: error.execution_status(),
                        },
                    ),
                }
                Err(error)
            }
        }
    }

    fn build_observability_envelope(&self, event: RuntimeEvent) -> RuntimeEventEnvelope {
        #[cfg(test)]
        self.observations
            .send_modify(|events| events.push(event.clone()));
        let event_id = workflow_event_id(&event);
        RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id,
            sequence: 0,
            conversation_id: self.event_store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: Utc::now(),
            event,
        }
    }

    fn emit_workflow_start_observability(
        &self,
        event: RuntimeEvent,
        snapshot: &WorkflowRunDefinitionSnapshot,
    ) {
        let envelope = self.build_observability_envelope(event);
        if let Ok(committed) = self.event_store.publish_workflow_start(envelope, snapshot) {
            self.read_model.publish_committed(committed.sequence);
        }
    }

    /// Appends one bounded best-effort observability fact to the conversation's
    /// existing Event Journal. A failure is intentionally ignored: ordinary
    /// Workflow lifecycle/join events never decide control flow or terminal
    /// state, which remain owned by this `WorkflowRun` and its native child
    /// registry. The successful child value and native terminal lifecycle fact
    /// use the separate durable compound transition in `SubagentRegistry`.
    fn emit_observability(&self, _run: &WorkflowRun, event: RuntimeEvent) {
        let envelope = self.build_observability_envelope(event);
        if let Ok(committed) = self.event_store.append_event(envelope) {
            // Every committed native progress fact releases its preceding
            // read-model revisions. Waiting for the outer run's terminal fact
            // would hide in-flight child/check state behind its own execution.
            self.read_model.publish_committed(committed.sequence);
        }
    }

    #[allow(clippy::too_many_arguments)] // the explicit child admission boundary
    #[allow(clippy::too_many_lines)] // the single native child admission path
    async fn admit_agent(
        &self,
        run: &WorkflowRun,
        context: &crate::runtime::subagent::NativeExecutionContext,
        input: &Value,
        values: &imbl::OrdMap<String, Value>,
        node_id: &WorkflowNodeInstance,
        agent: &WorkflowAgentProgram,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<crate::runtime::identity::SubagentId, WorkflowRunError> {
        let resolved = context
            .bind_workflow_agent(
                agent
                    .resolved
                    .as_ref()
                    .expect("admission froze every Agent"),
            )
            .map_err(|error| WorkflowRunError::ChildStart {
                node: node_id.to_string(),
                detail: bound_workflow_diagnostic(error.to_string()),
            })?;
        let task = model::render_parts(&agent.task, |part| evaluate_value(part, input, values))?;
        let spec = crate::runtime::subagent::SubagentStartSpec {
            authority: crate::runtime::subagent::DurableAgentAuthority {
                title: node_id.node.clone().into(),
                execution_policy: context.execution_policy(),
                resolved,
            },
            admission: crate::runtime::subagent::ActivationAdmission {
                attachments: Vec::new(),
                task,
                context: None,
                origin: crate::runtime::subagent::AgentActivationOrigin::Workflow {
                    node_id: Box::new(node_id.clone()),
                },
                terminal: crate::runtime::subagent::SubagentTerminalMode::WorkflowOutput {
                    output_schema: agent.terminal_schema(),
                    workflow_id: run.program.id().clone(),
                    run_id: run.run_id.clone(),
                    node_id: Box::new(node_id.clone()),
                },
                approval_mode: context.approval_mode(),
            },
        };
        let child_cancellation = cancellation.child_signal();
        let services = context.native.as_ref().ok_or_else(|| {
            WorkflowRunError::InvocationAuthority(
                "attempt has no native invocation services".into(),
            )
        })?;
        let approval_mode = context.approval_mode();
        // Node-authored permissions win; otherwise inherit the Named Agent
        // profile's frozen permissions. Omitting the node field must not erase
        // an explicit profile Deny/Ask.
        let effective_permissions = admission::effective_agent_entry_resource_permissions(
            agent.resource_permissions,
            context
                .resources()
                .subagents()
                .get(&agent.profile)
                .and_then(|definition| definition.profile().permissions),
        );
        let resource_permission = effective_permissions.map_or(
            crate::runtime::permission::ResourcePermissionValue::Default,
            |set| set.for_mode(approval_mode),
        );
        let tool_id =
            crate::runtime::identity::ToolId::new(format!("workflow-agent-{}", agent.profile));
        let origin = crate::tools::types::ToolOrigin::Builtin;
        let invocation_id = crate::tools::types::ToolInvocationId::Workflow {
            node: Box::new(node_id.clone()),
            retry: 0,
        };
        let arguments =
            admission::agent_entry_authorize_arguments(&agent.profile, &spec.admission.task);
        let workspace_root = services.runtime.workspace().root();
        let view = crate::agent::PreToolView {
            task: services.task.as_deref(),
            conversation_id: &run.run_id.conversation_id,
            execution: services.execution.clone(),
            invocation_id: &invocation_id,
            tool_id: &tool_id,
            tool_name: crate::tools::native::subagent::SUBAGENT_TOOL_NAME,
            origin: &origin,
            mode: crate::tools::types::ToolInvocationMode::Foreground,
            arguments: &arguments,
            audit_arguments: &arguments,
            elevation_reason: None,
            workspace_root: Some(workspace_root),
            mutation_target: None,
            resource_permission,
        };
        self.read_model.node(node_id, |view| {
            view.state = read_model::WorkflowState::Waiting {
                reason: read_model::WorkflowWait::Approval,
            }
        });
        if let Some(result) =
            crate::tools::invocation::authorize(&services.lifecycle, &view, cancellation)
                .await
                .map_err(|error| WorkflowRunError::InvocationAuthority(error.to_string()))?
        {
            let detail = match result.status {
                crate::tools::types::ToolExecutionStatus::Denied { reason } => reason,
                other => format!("agent entry blocked: {other:?}"),
            };
            return Err(WorkflowRunError::ChildStart {
                node: node_id.to_string(),
                detail: bound_workflow_diagnostic(detail),
            });
        }
        self.read_model.node(node_id, |view| {
            view.state = read_model::WorkflowState::Waiting {
                reason: read_model::WorkflowWait::Capacity,
            }
        });
        // The finite Workflow-owned AgentRun shares the native staged child
        // supervision path and the invoking Session binding. Its terminal mode
        // keeps it out of the ordinary durable Agent resume and mailbox API.
        let prepared = self
            .subagents
            .prepare(&spec, &child_cancellation)
            .await
            .map_err(|error| {
                if !error.rollback_is_proven()
                    && let Some(effects) = &run.effects
                {
                    effects.unproven(node_id, "native Agent rollback has no physical proof");
                }
                match error {
                    crate::runtime::subagent::SubagentStartError::Cancelled => {
                        WorkflowRunError::from_cancellation(cancellation)
                    }
                    error => WorkflowRunError::ChildStart {
                        node: node_id.to_string(),
                        detail: bound_workflow_diagnostic(error.to_string()),
                    },
                }
            })?;
        let accepted = self
            .subagents
            .commit_waiting(prepared, &child_cancellation)
            .await
            .map_err(|error| {
                if !error.rollback_is_proven()
                    && let Some(effects) = &run.effects
                {
                    effects.unproven(node_id, "native Agent rollback has no physical proof");
                }
                match error {
                    crate::runtime::subagent::SubagentStartError::Cancelled => {
                        WorkflowRunError::from_cancellation(cancellation)
                    }
                    error => WorkflowRunError::ChildStart {
                        node: node_id.to_string(),
                        detail: bound_workflow_diagnostic(error.to_string()),
                    },
                }
            })?;
        let crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) = accepted else {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        };
        if let Some(effects) = &run.effects
            && let Err(error) = effects.owner(
                node_id,
                scheduled::lifecycle::ScheduledEffectOwner::Agent {
                    activation: accepted.subagent_id.clone(),
                },
            )
        {
            let _ = self.subagents.cancel(
                &accepted.subagent_id,
                crate::runtime::types::CancellationReason::ParentCancelled,
            );
            let settled = self
                .subagents
                .wait_until_settled(&accepted.subagent_id)
                .await;
            if !settled.is_some_and(|snapshot| snapshot.is_settled()) {
                effects.unproven(
                    node_id,
                    "Agent owner association failed and native cleanup remains unproven",
                );
            }
            return Err(error);
        }
        self.read_model.node(node_id, |view| {
            view.child = Some(accepted.subagent_id.clone());
            view.child_agent_id = Some(accepted.child_agent_id.clone());
            view.state = read_model::WorkflowState::Waiting {
                reason: read_model::WorkflowWait::Agent,
            };
        });
        self.emit_observability(
            run,
            RuntimeEvent::WorkflowAgentAdmitted {
                workflow_id: run.program.id().clone(),
                run_id: run.run_id.clone(),
                node_id: node_id.clone(),
                subagent_id: accepted.subagent_id.clone(),
                profile: agent.profile.clone(),
            },
        );
        Ok(accepted.subagent_id)
    }

    async fn settle_agent(
        &self,
        run: &WorkflowRun,
        subagent_id: crate::runtime::identity::SubagentId,
        node_id: &WorkflowNodeInstance,
        agent: &WorkflowAgentProgram,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let mut wait = Box::pin(self.subagents.wait_until_settled(&subagent_id));
        let snapshot = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                // The child owns a separate attempt. Parent deadline intent is
                // ParentCancelled there, never a fabricated user request; this
                // Workflow retains the exact deadline in its typed run outcome.
                let reason = match cancellation.native_cause() {
                    crate::tools::deadline::ToolCancellationCause::Attempt(reason) => reason,
                    crate::tools::deadline::ToolCancellationCause::Deadline(_) => crate::runtime::types::CancellationReason::ParentCancelled,
                };
                let _ = self.subagents.cancel(&subagent_id, reason);
                let snapshot = (&mut wait).await;
                // The native child settlement is the cross-process
                // observation of the workflow_output latch. If that
                // success committed before cancellation, it remains the
                // winner even when this waiter observed cancellation first.
                if let Some(snapshot) = snapshot {
                    return self.settled_agent_value(
                            run, snapshot,
                            &subagent_id,
                            node_id,
                            agent,
                            cancellation,
                        );
                }
                return Err(WorkflowRunError::from_cancellation(cancellation));
            }
            snapshot = &mut wait => snapshot,
        };
        let snapshot = snapshot.ok_or_else(|| WorkflowRunError::ChildOutcome {
            node: node_id.to_string(),
            status: crate::tools::types::ToolExecutionStatus::OutcomeUnknown {
                detail: "the native SubagentRegistry lost the child record".to_owned(),
            },
        })?;
        self.settled_agent_value(run, snapshot, &subagent_id, node_id, agent, cancellation)
    }

    fn settled_agent_value(
        &self,
        run: &WorkflowRun,
        snapshot: crate::runtime::subagent::SubagentSnapshot,
        subagent_id: &crate::runtime::identity::SubagentId,
        node_id: &WorkflowNodeInstance,
        agent: &WorkflowAgentProgram,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        use crate::runtime::subagent::{SubagentPhysicalSettlement, SubagentPublication};
        use crate::tools::types::{ToolCancellationPhase, ToolExecutionStatus as Status};
        if !snapshot.is_settled()
            && let Some(effects) = &run.effects
        {
            effects.unproven(
                node_id,
                "native Agent publication or physical proof is pending",
            );
        }
        let settlement_failure = match snapshot.settlement.publication {
            SubagentPublication::Pending => {
                Some(WorkflowChildSettlementFailure::PublicationPending)
            }
            SubagentPublication::Abandoned => {
                Some(WorkflowChildSettlementFailure::PublicationAbandoned)
            }
            SubagentPublication::Committed if !snapshot.state.is_terminal() => {
                Some(WorkflowChildSettlementFailure::LogicalTerminalMissing)
            }
            SubagentPublication::Committed
                if snapshot.settlement.physical == SubagentPhysicalSettlement::Unproven =>
            {
                Some(WorkflowChildSettlementFailure::PhysicalContainmentUnproven)
            }
            SubagentPublication::Committed => None,
        };
        if let Some(failure) = settlement_failure {
            return Err(WorkflowRunError::ChildSettlement {
                node: node_id.to_string(),
                failure,
            });
        }
        match snapshot.state {
            crate::runtime::subagent::SubagentState::Succeeded => {
                // The committed output value is the live Workflow result
                // channel owned by the registry (Issue #178): the
                // observation snapshot deliberately never carries it.
                let value = self
                    .subagents
                    .take_workflow_agent_output(subagent_id, node_id)
                    .ok_or_else(|| WorkflowRunError::ChildFailed {
                        node: node_id.to_string(),
                        detail: "workflow Agent completed without committed output".to_owned(),
                    })?;
                // This validation is not redundant with
                // `validate_workflow_candidate` in the registry: the registry
                // revalidates the untrusted cross-process child wire frame
                // before its durable commit, while this validates the run's
                // result against the node's own frozen output authority at
                // consumption time. The two checks guard different trust
                // boundaries; both stay.
                if let Some(schema) = agent.terminal_schema() {
                    let validator = jsonschema::Validator::new(&schema).map_err(|error| {
                        WorkflowRunError::ChildFailed {
                            node: node_id.to_string(),
                            detail: format!("workflow Agent output schema became invalid: {error}"),
                        }
                    })?;
                    if !validator.is_valid(&value) {
                        return Err(WorkflowRunError::ChildFailed {
                            node: node_id.to_string(),
                            detail: "workflow Agent output violated its frozen output schema"
                                .to_owned(),
                        });
                    }
                }
                let value = agent
                    .value(value)
                    .ok_or_else(|| WorkflowRunError::ChildFailed {
                        node: node_id.to_string(),
                        detail: "workflow Agent output does not match its output mode".to_owned(),
                    })?;
                // The native SubagentRegistry already committed the value
                // fact atomically with the child's terminal lifecycle fact.
                // Do not append a second observability event here: the
                // WorkflowRun consumes that durable handoff but is not a
                // second Event Journal authority.
                Ok(value)
            }
            crate::runtime::subagent::SubagentState::Cancelled => {
                // The registry's first committed cause is immutable. Only
                // inherited parent cancellation may project the parent's
                // richer deadline cause; an unrelated later outer cancellation
                // cannot replace a child's own committed cause.
                let status = match snapshot.cancel_reason {
                    Some(crate::runtime::types::CancellationReason::SubagentExecutionDeadlineExceeded) => Status::TimedOut,
                    Some(crate::runtime::types::CancellationReason::ParentCancelled)
                        if cancellation.is_cancelled() => cancellation.native_status(ToolCancellationPhase::DuringExecution),
                    Some(reason) => Status::Cancelled { reason, phase: ToolCancellationPhase::DuringExecution },
                    None => Status::OutcomeUnknown { detail: "native child cancellation has no committed cause".into() },
                };
                Err(WorkflowRunError::ChildOutcome {
                    node: node_id.to_string(),
                    status,
                })
            }
            crate::runtime::subagent::SubagentState::Interrupted => {
                Err(WorkflowRunError::ChildOutcome {
                    node: node_id.to_string(),
                    status: Status::OutcomeUnknown {
                        detail: bound_workflow_diagnostic(snapshot.detail.unwrap_or_else(|| {
                            "native child interrupted without a valid outcome".into()
                        })),
                    },
                })
            }
            state => Err(WorkflowRunError::ChildFailed {
                node: node_id.to_string(),
                detail: bound_workflow_diagnostic(
                    snapshot
                        .detail
                        .unwrap_or_else(|| format!("native child settled as {state:?}").into()),
                ),
            }),
        }
    }
}

/// Which independent native child settlement obligation is unproven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowChildSettlementFailure {
    LogicalTerminalMissing,
    PublicationPending,
    PublicationAbandoned,
    PhysicalContainmentUnproven,
}

impl fmt::Display for WorkflowChildSettlementFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::LogicalTerminalMissing => "native child logical terminal did not commit",
            Self::PublicationPending => "native child terminal publication has not committed",
            Self::PublicationAbandoned => {
                "native child terminal publication was abandoned after durability failure"
            }
            Self::PhysicalContainmentUnproven => {
                "native child terminal publication committed but physical containment is unproven"
            }
        })
    }
}

/// A workflow execution error. Execution failures remain failures; they are
/// never converted into workflow-local values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRunError {
    Disabled {
        workflow: WorkflowId,
        diagnostics: Vec<WorkflowAdmissionDiagnostic>,
    },
    LimitExceeded(WorkflowLimit),
    ChildOutcome {
        node: String,
        status: crate::tools::types::ToolExecutionStatus,
    },
    ChildSettlement {
        node: String,
        failure: WorkflowChildSettlementFailure,
    },
    CapabilityNotAdmitted(String),
    IneligibleCapability(String),
    IdentityChanged(String),
    InvocationAuthority(String),
    /// A native leaf settled without successful business output.
    ToolFailed {
        node: String,
        status: crate::tools::types::ToolExecutionStatus,
    },
    /// An Auxiliary Model Call failed without committing a local value.
    ModelFailed {
        node: String,
        status: crate::tools::types::ToolExecutionStatus,
    },
    /// Input did not satisfy the frozen workflow schema.
    InvalidInput(String),
    /// The Return value did not satisfy the frozen workflow schema.
    InvalidOutput(String),
    /// The immutable program was internally inconsistent.
    InvalidProgram(String),
    /// A producer has not committed a value in this scope.
    UncommittedReference(Vec<String>),
    /// A value could not be constructed or resolved at runtime.
    InvalidValue(String),
    /// A child could not be admitted.
    ChildStart {
        node: String,
        detail: String,
    },
    /// A child settled unsuccessfully.
    ChildFailed {
        node: String,
        detail: String,
    },
    /// Multiple admitted nodes failed, in node ID order.
    NodesFailed {
        failures: BTreeMap<String, WorkflowRunError>,
    },
    /// Cancellation won terminal settlement.
    Cancelled(crate::runtime::types::CancellationReason),
    /// An ancestor native deadline interrupted this scope.
    Deadline(crate::tools::deadline::ToolDeadlineKind),
    /// A terminal transition was attempted twice.
    TerminalAlreadySettled,
}

impl WorkflowRunError {
    fn from_cancellation(
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Self {
        match cancellation.native_cause() {
            crate::tools::deadline::ToolCancellationCause::Attempt(reason) => {
                Self::Cancelled(reason)
            }
            crate::tools::deadline::ToolCancellationCause::Deadline(kind) => Self::Deadline(kind),
        }
    }
    /// Preserve native execution certainty at every composite boundary.
    #[must_use]
    pub fn execution_status(&self) -> crate::tools::types::ToolExecutionStatus {
        use crate::tools::types::{ToolCancellationPhase, ToolExecutionStatus as Status};
        match self {
            Self::ToolFailed { status, .. }
            | Self::ModelFailed { status, .. }
            | Self::ChildOutcome { status, .. } => status.clone(),
            Self::ChildSettlement { failure, .. } => Status::OutcomeUnknown {
                detail: failure.to_string(),
            },
            Self::Deadline(_) => Status::TimedOut,
            Self::Cancelled(reason) => Status::Cancelled {
                reason: *reason,
                phase: ToolCancellationPhase::DuringExecution,
            },
            Self::NodesFailed { failures, .. } => {
                let statuses = failures
                    .values()
                    .map(Self::execution_status)
                    .collect::<Vec<_>>();
                statuses
                    .iter()
                    .find(|s| matches!(s, Status::OutcomeUnknown { .. }))
                    .or_else(|| statuses.first())
                    .cloned()
                    .unwrap_or_else(|| Status::Failed {
                        error: self.to_string(),
                    })
            }
            _ => Status::Failed {
                error: self.to_string(),
            },
        }
    }
    /// Whether this error represents the native cancellation terminal.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self.execution_status(),
            crate::tools::types::ToolExecutionStatus::Cancelled { .. }
        )
    }
}

impl fmt::Display for WorkflowRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled {
                workflow,
                diagnostics,
            } => write!(
                formatter,
                "Workflow {workflow} is disabled: {}",
                serde_json::to_string(diagnostics).expect("typed diagnostics serialize")
            ),
            Self::LimitExceeded(limit) => {
                write!(formatter, "Workflow run {limit:?} limit exceeded")
            }
            Self::ChildOutcome { node, status } => {
                write!(formatter, "Workflow Agent {node:?}: {status:?}")
            }
            Self::ChildSettlement { node, failure } => {
                write!(formatter, "Workflow Agent {node:?}: {failure}")
            }
            Self::ToolFailed { node, status } => {
                write!(formatter, "Workflow Tool {node:?}: {status:?}")
            }
            Self::ModelFailed { node, status } => {
                write!(formatter, "Workflow Model {node:?}: {status:?}")
            }
            Self::CapabilityNotAdmitted(detail) => {
                write!(formatter, "capability not admitted: {detail}")
            }
            Self::IneligibleCapability(detail) => {
                write!(formatter, "ineligible Workflow leaf: {detail}")
            }
            Self::IdentityChanged(detail) => {
                write!(formatter, "frozen capability identity changed: {detail}")
            }
            Self::InvocationAuthority(detail) => {
                write!(formatter, "native invocation authority failed: {detail}")
            }
            Self::InvalidInput(detail) => write!(formatter, "invalid workflow input: {detail}"),
            Self::InvalidOutput(detail) => write!(formatter, "invalid workflow output: {detail}"),
            Self::InvalidProgram(detail) => write!(formatter, "invalid workflow program: {detail}"),
            Self::UncommittedReference(path) => write!(formatter, "uncommitted reference {path:?}"),
            Self::InvalidValue(detail) => write!(formatter, "invalid workflow value: {detail}"),
            Self::ChildStart { node, detail } => {
                write!(
                    formatter,
                    "Workflow Agent {node:?} could not start: {detail}"
                )
            }
            Self::ChildFailed { node, detail } => {
                write!(formatter, "Workflow Agent {node:?} failed: {detail}")
            }
            Self::NodesFailed { failures } => {
                write!(formatter, "Workflow nodes failed: ")?;
                for (key, failure) in failures {
                    write!(formatter, "{key}: {failure}; ")?;
                }
                Ok(())
            }
            Self::Cancelled(reason) => write!(formatter, "workflow cancelled: {reason:?}"),
            Self::Deadline(kind) => write!(
                formatter,
                "workflow interrupted by ancestor {kind:?} deadline"
            ),
            Self::TerminalAlreadySettled => {
                formatter.write_str("workflow terminal state was already settled")
            }
        }
    }
}

impl std::error::Error for WorkflowRunError {}

fn bound_workflow_text(value: String) -> String {
    crate::runtime::subagent::bound_utf8(value, crate::runtime::subagent::MAX_RESULT_CONTENT_BYTES)
}

fn bound_workflow_diagnostic(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    let mut end = value.len().min(MAX_WORKFLOW_DIAGNOSTIC_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests;
