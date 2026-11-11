//! Fixed execution-permission modes, resource overrides, and the built-in
//! permission matrix (Issue #544).
//!
//! Exactly three built-in modes (`plan`, `automatic`, `full_access`) decide
//! whether a prepared invocation may execute. They never change tool
//! availability or model-facing schemas. Resource-local three-mode overrides
//! and the unified elevation / Auxiliary review path are owned here; domain
//! authority (Goal budgets, Scheduler revisions, MCP host limits) remains
//! with those owners.

mod bash;
mod classify;
mod decision;
mod matrix;
mod mode;
mod resource;
pub(crate) mod review;

pub use bash::{BashReadonlyAnalysis, BashReadonlyClass, analyze_bash_readonly};
pub use classify::{ClassificationHints, classify_tool_kind, is_workspace_plan_file};
pub use decision::{
    ElevationReason, PermissionDecision, PermissionDecisionKind, PermissionFacts,
    evaluate_permission,
};
pub use matrix::{BuiltinPermissionAction, BuiltinToolKind, builtin_action_for};
pub use mode::ExecutionPermissionMode;
pub use resource::{
    ResourcePermissionSet, ResourcePermissionValue, resolve_resource_permission,
};
pub use review::{PermissionReviewVerdict, accept_permission_review};
