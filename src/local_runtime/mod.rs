//! Shared local configuration, durable Session, and runtime owners.
//!
//! `rustx app-server` composes these owners through [`crate::app_server`],
//! the only external product control protocol:
//!
//! - [`config`] — the bounded explicit current runtime/project configuration;
//! - [`composition`] — the one Rust-side composition owner;
//! - [`session_controller`] / [`session_runtime_manager`] — durable Session
//!   authority and process-local runtime residency;
//! - [`cli`] — the bounded public argument contract;
//! - [`process`] — the single process entry and its stream ownership.
//!
//! # Process output contract
//!
//! Help and argument diagnostics go to stderr before any effect; a bare
//! `rustx` prints help. App Server stdout is protocol-only. Configuration
//! subcommands own stdout for their bounded human/JSON results and never
//! create a runtime.
//!
//! The internal `rustx --subagent-child` mode ([`subagent_child`]) is an
//! independent transport: fd 0 is the reliable subagent control IPC and
//! fd 1/stdout is the protocol-owned framed Activity observation IPC
//! (Issue #178), not human-readable output. Its read-only live inspection
//! endpoint (`live_inspection`) is child-local routing state.
//!
//! `println!` is never used for diagnostics anywhere in the process.

pub(crate) mod agent_resources;
pub mod authoring;
pub mod cli;
pub mod composition;
pub mod config;
pub mod configuration;
mod diagnostics;
pub(crate) mod dispatcher;
mod initialization;
pub mod launch;
#[cfg(test)]
mod launch_tests;
pub(crate) mod live_inspection;
pub mod managed_python_resources;
pub mod mcp_resources;
#[cfg(all(test, unix))]
mod preparation_e2e;
mod probes;
pub mod process;
mod resource_directory;
pub mod schemas;
pub mod session;
pub mod session_controller;
pub(crate) mod session_display_projection;
pub mod session_runtime_manager;
pub mod settings;
#[cfg(test)]
pub(crate) mod static_effects;
pub mod subagent_child;
mod workflow_inspection;
pub(crate) mod workflow_resources;

pub use cli::ArgumentError;
pub use composition::{
    HeadlessConversationRuntime, LocalConversationCore, LocalConversationRuntime,
    LocalRuntimeDependencies, LocalRuntimeError, StartupSession,
};
pub use config::{
    AgentWorktreeDocument, CURRENT_RUNTIME_SCHEMA_VERSION, CurrentRuntimeConfig,
    CurrentRuntimeConfigError, McpServerDocument, McpTransportType, ModelTimeoutPolicyDocument,
};
pub use launch::{HostEnvironment, LaunchRequest, resolve};
pub use process::run_process;
pub use session::{
    CatalogCommitError, LineageReadCut, SESSION_CATALOG_SCHEMA_VERSION, SESSION_LIST_PAGE_LIMIT,
    SESSION_NAME_LIMIT, SESSION_TREE_PAGE_LIMIT, SessionCatalog, SessionError, SessionId,
    SessionListPage, SessionNode, SessionNodeId, SessionNodeOrigin, SessionNodePage,
    SessionSnapshot, SessionSummary, SessionUserMessageBoundary, SessionUserMessageBoundaryPage,
};
pub use session_controller::SessionTransitionResult;

#[cfg(test)]
mod settings_e2e;

pub mod session_deletion;
pub mod session_ownership;

pub use configuration::{
    AdmittedSessionConfig, SessionConfigInput, SessionLocations, UserConfigManager,
    UserConfigSources,
};

pub mod app_server_policy;

#[cfg(test)]
mod mcp_tests;
