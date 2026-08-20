//! The local conversation runtime process (Issue #42).
//!
//! This module owns everything between explicit startup configuration and
//! the Runtime Client endpoint a transport wraps:
//!
//! - [`config`] — the bounded explicit bootstrap conversation configuration;
//! - [`composition`] — the one Rust-side composition owner;
//! - [`cli`] — the bounded startup argument contract;
//! - [`serve`] — the process lifecycle over the Issue #38 stdio/JSONL
//!   transport.
//!
//! # Process output contract
//!
//! ```text
//! before serving : stdout is empty; every diagnostic goes to stderr
//! while serving  : stdout is Runtime Client JSONL only
//! on failure     : stderr diagnostic, non-zero exit, zero bytes on stdout
//! ```
//!
//! `println!` is never used for diagnostics anywhere in the process.

pub mod cli;
pub mod composition;
pub mod config;
pub mod serve;
pub mod session;
pub mod subagent_child;
pub mod supervisor;

pub use cli::{ArgumentError, USAGE, parse_arguments};
pub use composition::{
    HeadlessConversationRuntime, LocalConversationCore, LocalConversationRuntime,
    LocalRuntimeDependencies, LocalRuntimeError, LocalRuntimePaths, LocalSessionProduct,
};
pub use config::{
    LOCAL_CONVERSATION_SCHEMA_VERSION, LocalConversationConfig, LocalConversationConfigError,
    McpServerDocument, McpTransportType,
};
pub use serve::{ProcessOutcome, run_process, serve};
pub use session::{
    HistoricalConversationSnapshot, SESSION_CATALOG_SCHEMA_VERSION, SessionCatalog, SessionError,
    SessionId, SessionNode, SessionNodeId, SessionNodeOrigin, SessionSnapshot, SessionSummary,
    SessionUserMessageBoundary,
};
pub use supervisor::{
    LocalSessionSupervisor, SessionSupervisorError, SessionSwitchResult, SessionTreeResult,
};
