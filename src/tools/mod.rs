//! Canonical tool registry and executor contracts for native, MCP, and Python tools.
//!
//! The tool plane owns the canonical [`ToolDefinition`] contract, the
//! validating [`ToolRegistry`], the [`ToolExecutor`] boundary, the JSON
//! Schema validation and the model-facing schema compiler, the workspace
//! boundary, the managed tool-output store, the artifact store, the explicit
//! tool environment, the conversation-owned background registry, and the
//! conversation-owned task list. Native,
//! MCP, and Python executor implementations share exactly this contract.
//! Native Read/Write/Edit/Grep/Glob use the workspace root as their cwd and
//! accept ordinary absolute host paths. The locator remains the runtime's
//! read-only resolver for advertised managed-output paths; `ManagedToolOutput`
//! owns the separate model-mutation guard.

pub mod artifacts;
pub mod background;
pub mod environment;
pub mod executor;
pub mod limits;
pub mod locator;
pub mod managed_output;
pub mod mcp;
pub mod native;
pub(crate) mod output;
pub mod python;
pub mod runtime;
pub mod schema;
pub mod todo;
pub mod types;
pub mod workspace;

pub use native::{NativeToolPolicies, NativeToolResources, register_native_tools};

pub use artifacts::{ArtifactError, ArtifactStore, ArtifactWriter};
pub use background::{
    BackgroundDispatchOutcome, BackgroundExecutionSnapshot, BackgroundLifecycle,
    ConversationBackgroundRegistry,
};
pub use environment::{ToolEnvironment, ToolEnvironmentError};
pub use executor::{
    ASK_USER_TOOL_NAME, BACKGROUND_TASK_TOOL_NAME, PreflightOutcome, PreparedInvocation,
    ProgressReporter, ToolExecutionContext, ToolExecutor, ToolPreflightError, ToolRegistry,
    ToolRegistryError,
};
pub use locator::LocatorError;
pub use managed_output::{BackgroundOutput, ManagedOutputError, ManagedToolOutput, ResultSpill};
pub use runtime::ConversationToolRuntime;
pub use schema::{
    DECORATABLE_ROOT_INSTANCE_KEYWORDS, EXECUTION_MODE_DESCRIPTION,
    EXECUTION_MODE_DESCRIPTION_REMINDER, EXECUTION_MODE_FIELD, EXECUTION_MODE_VALUES,
    ExecutionModeClaim, REFERENCE_APPLICATOR_KEYWORDS, ROOT_ANNOTATION_KEYWORDS,
    RUNTIME_PROPERTY_PREFIX, SchemaError, compile_model_definition, is_decoratable_root_keyword,
    is_reserved_property, resolve_invocation_metadata, validate_business_arguments,
    validate_canonical_schema, validate_execution_metadata_contract,
};
// The task list is split at the authority boundary rather than exported as
// one surface. What a consumer may hold is the *derived* list and the rules
// for reading one out of canonical history; what mutates or settles it is
// the Agent Loop's, because settlement is the claim that the Ledger already
// carries the list being installed, and nothing outside the loop can make
// that claim truthfully.
pub use todo::{
    TODO_TOOL_ID, TodoRebuildError, TodoSnapshot, TodoSnapshotError, TodoStatus, TodoTask,
    forbidden_control, metadata_control, published_snapshot,
};

pub use types::{
    ManagedOutputContinuation, ModelToolDefinition, ToolApprovalPolicy, ToolCall, ToolCallStart,
    ToolCancellationPhase, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode,
    ToolInvocationPolicy, ToolOrigin, ToolProgress, ToolReplayPolicy, ToolResultContent,
    TruncationState,
};
pub use workspace::{Workspace, WorkspaceError};
