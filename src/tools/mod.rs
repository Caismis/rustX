//! Canonical tool registry and executor contracts for native, MCP, and Python tools.
//!
//! The tool plane owns the canonical [`ToolDefinition`] contract, the
//! validating [`ToolRegistry`], the [`ToolExecutor`] boundary, the JSON
//! Schema validation and the model-facing schema compiler, the workspace
//! boundary, the artifact store, the explicit tool environment, and the
//! conversation-owned background registry. Native, MCP, and Python executor
//! implementations share exactly this contract; M5 implements the native
//! tools.

pub mod artifacts;
pub mod background;
pub mod environment;
pub mod executor;
pub mod limits;
pub mod native;
pub mod process_supervision;
pub mod runtime;
pub mod schema;
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
    BACKGROUND_TASK_TOOL_NAME, PreflightOutcome, PreparedInvocation, ProgressReporter,
    ToolExecutionContext, ToolExecutor, ToolPreflightError, ToolRegistry, ToolRegistryError,
};
pub use runtime::ConversationToolRuntime;
pub use schema::{
    EXECUTION_FIELD, EXECUTION_FIELD_VALUES, RUNTIME_PROPERTY_PREFIX, SchemaError,
    compile_model_definition, is_reserved_property, resolve_invocation_metadata,
    validate_business_arguments, validate_canonical_schema,
};
pub use types::{
    ModelToolDefinition, ToolCall, ToolCallStart, ToolConcurrencyPolicy, ToolDefinition,
    ToolExecutionPolicy, ToolExecutionResult, ToolExecutionStatus, ToolInvocation,
    ToolInvocationMode, ToolOrigin, ToolProgress, ToolReplayPolicy, ToolResultContent,
    TruncationState,
};
pub use workspace::{Workspace, WorkspaceError};
