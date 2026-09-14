//! Shared local configuration, durable Session, and runtime owners.
//!
//! Ordinary `rustx` serves the local Runtime Client path described below.
//! `rustx app-server` composes these shared owners through [`crate::app_server`].
//!
//! This module owns everything between explicit startup configuration and
//! the Runtime Client endpoint a transport wraps:
//!
//! - [`config`] — the bounded explicit current runtime/project configuration;
//! - [`composition`] — the one Rust-side composition owner;
//! - [`cli`] — the bounded startup argument contract;
//! - [`serve`] — the process lifecycle over the Issue #38 stdio/JSONL
//!   transport.
//!
//! # Process output contract
//!
//! Normal runtime mode:
//!
//! ```text
//! before serving : stdout is empty; every diagnostic goes to stderr
//! while serving  : stdout is Runtime Client JSONL only
//! on failure     : stderr diagnostic, non-zero exit, zero bytes on stdout
//! ```
//!
//! The internal `rustx --subagent-child` mode ([`subagent_child`]) is the
//! an independent transport: fd 0 is the reliable subagent control IPC and
//! fd 1/stdout is the protocol-owned framed Activity observation IPC
//! (Issue #178), not human-readable output. Diagnostics stay on stderr in
//! runtime transport modes. Configuration subcommands instead own stdout for
//! their bounded human/JSON results and never create a runtime transport.
//!
//! `println!` is never used for diagnostics anywhere in the process.

pub(crate) mod agent_resources;
mod authoring;
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
mod managed_python_resources;
#[cfg(all(test, unix))]
mod preparation_e2e;
mod probes;
mod resource_directory;
pub mod schemas;
pub mod serve;
pub mod session;
pub mod session_controller;
pub mod session_runtime_manager;
pub mod settings;
#[cfg(test)]
pub(crate) mod static_effects;
pub mod subagent_child;
pub mod supervisor;
mod workflow_inspection;
pub(crate) mod workflow_resources;

pub use cli::{ArgumentError, USAGE, parse_arguments};
pub use composition::{
    HeadlessConversationRuntime, LocalConversationCore, LocalConversationInspection,
    LocalConversationRuntime, LocalRuntimeDependencies, LocalRuntimeError, LocalSessionClient,
    StartupSession,
};
pub use config::{
    AgentWorktreeDocument, CURRENT_RUNTIME_SCHEMA_VERSION, CurrentRuntimeConfig,
    CurrentRuntimeConfigError, McpServerDocument, McpTransportType, ModelTimeoutPolicyDocument,
};
pub use launch::{HostEnvironment, LaunchRequest, TrustAction, resolve};
pub use serve::{ProcessOutcome, run_process, serve};
pub use session::{
    CatalogCommitError, HistoricalConversationSnapshot, SESSION_CATALOG_SCHEMA_VERSION,
    SESSION_LIST_PAGE_LIMIT, SESSION_NAME_LIMIT, SESSION_TREE_PAGE_LIMIT, SessionCatalog,
    SessionError, SessionId, SessionListPage, SessionNode, SessionNodeId, SessionNodeOrigin,
    SessionNodePage, SessionSnapshot, SessionSummary, SessionUserMessageBoundary,
    SessionUserMessageBoundaryPage,
};
pub use supervisor::{
    LocalSessionAttachment, SessionAttachmentError, SessionTransitionResult, SessionTreeResult,
};

#[cfg(test)]
mod settings_e2e;

pub mod session_deletion;

pub use configuration::{
    AdmittedSessionConfig, SessionConfigInput, SessionLocations, UserConfigManager,
    UserConfigSources,
};

pub mod app_server_policy;
