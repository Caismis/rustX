//! The local conversation runtime process (Issue #42).
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

mod authoring;
pub mod cli;
pub mod composition;
pub mod config;
mod diagnostics;
pub(crate) mod dispatcher;
mod initialization;
pub mod launch;
#[cfg(test)]
mod launch_tests;
pub(crate) mod live_inspection;
#[cfg(all(test, unix))]
mod preparation_e2e;
mod probes;
pub mod schemas;
pub mod serve;
pub mod session;
pub mod settings;
#[cfg(test)]
pub(crate) mod static_effects;
pub mod subagent_child;
pub(crate) mod subagent_resources;
pub mod supervisor;
mod workflow_inspection;
pub(crate) mod workflow_resources;

pub use cli::{ArgumentError, USAGE, parse_arguments};
pub use composition::{
    HeadlessConversationRuntime, LocalConversationCore, LocalConversationInspection,
    LocalConversationRuntime, LocalRuntimeDependencies, LocalRuntimeError, LocalSessionProduct,
    StartupSession,
};
pub use config::{
    CURRENT_RUNTIME_SCHEMA_VERSION, CurrentRuntimeConfig, CurrentRuntimeConfigError,
    McpServerDocument, McpTransportType, ModelTimeoutPolicyDocument, SubagentWorktreeDocument,
};
pub use launch::{
    HostEnvironment, LaunchLocations, LaunchRequest, ResolvedLaunch, TrustAction, resolve,
};
pub use serve::{ProcessOutcome, run_process, serve};
pub use session::{
    CatalogCommitError, HistoricalConversationSnapshot, SESSION_CATALOG_SCHEMA_VERSION,
    SESSION_LIST_PAGE_LIMIT, SESSION_NAME_LIMIT, SESSION_TREE_PAGE_LIMIT, SessionCatalog,
    SessionError, SessionId, SessionListPage, SessionNode, SessionNodeId, SessionNodeOrigin,
    SessionNodePage, SessionSnapshot, SessionSummary, SessionUserMessageBoundary,
    SessionUserMessageBoundaryPage,
};
pub use supervisor::{
    LocalSessionSupervisor, SessionSupervisorError, SessionSwitchResult, SessionTreeResult,
};

#[cfg(test)]
mod settings_e2e;

pub mod session_deletion;
