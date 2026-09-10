//! The in-crate boundary conformance suites, compiled into the crate's own
//! test build.
//!
//! # Semantic class
//!
//! Every suite here is a **boundary conformance test**: the invariant under
//! test *is* a real operating-system or runtime boundary — process spawn and
//! death, child-process supervision, real stdio/IPC to a spawned fixture
//! server, shell background execution. None of that can be proven by a
//! scripted in-memory seam, so these suites deliberately drive real
//! processes, real signal semantics, and real transports.
//!
//! # Why they compile in-crate
//!
//! Like [`super::scripted_suites`], these suites need the crate's
//! `#[cfg(test)] pub(crate)` seams (scripted `ModelAdapter` /
//! `ContextSummarizer` fixtures, staged-child registry overrides, the
//! process-death child entry point) that must never become published API.
//! They therefore share the lib test binary with the deterministic contract
//! suites — but compilation placement is not the semantic class. The stable
//! `boundary_suites::` name prefix is what identifies these tests as
//! boundary conformance, and CI selects them by that prefix
//! (`cargo test --lib -- boundary_suites::`).
//!
//! # Platform sensitivity
//!
//! Every suite here exercises semantics where operating systems genuinely
//! differ (process groups, SIGKILL, unix sockets, shell supervision), so the
//! `boundary_suites::` prefix also runs on the macOS CI job.
//!
//! # Domain ownership
//!
//! - [`durable`] — real process-death conformance over the durable
//!   authority: SIGKILL at deterministic gates, then recovery.
//! - [`background`] — real bash background execution and supervision
//!   (spill, terminal inbound) through the actual supervisor binary.
//! - [`subagent`] — the child-process boundary with real staged children:
//!   control socket, registry lifecycle, terminal notice, cancellation/drain
//!   crossing the boundary.
//! - [`runtime_client`] — capability projection over real MCP stdio child
//!   servers: the configured-server fixture (this binary re-executed in
//!   fixture mode) and the managed Python package projection (a real,
//!   network-bound `uv` build serving a real `FastMCP` child).
//! - [`mcp_mrtr`] — MCP multi-round-trip (SEP-2322) execution against a real
//!   MCP `2026-07-28` stdio child: one invocation, N bounded rounds, the
//!   runtime-owned Interaction between them, and the cancellation frontiers
//!   around each dispatch.
//! - [`mcp_mrtr_managed`] — the same contract end to end against a real
//!   managed `FastMCP` 4 child built by a real, network-bound `uv`.
//! - [`mcp_tasks`] — the MCP Tasks extension (SEP-2663) as an adapter-local
//!   remote sub-lifecycle of one already-admitted `ToolInvocation`: capability
//!   advertisement, the poll loop, in-task Interaction reuse, the
//!   eventually-consistent update deduplication, and the cancellation
//!   frontiers around every task request.
//! - [`mcp_tasks_managed`] — the same contract end to end against a real
//!   managed `FastMCP` 4 child whose own `requirements.txt` declares the
//!   SEP-2663 `fastmcp-tasks` extension.
//! - [`tool_deadline`] — the generic Issue #204 hard deadline bounding a
//!   real foreground Bash process: process-group kill and reap are the
//!   physical settlement behind the proven `TimedOut`.
//!
//! See `tests/README.md` for the full test architecture.

#![allow(clippy::too_many_lines)] // scenario bodies are deliberately linear

/// The fixtures shared with the integration-test binaries and the scripted
/// contract suites (one compilation, re-exported from `scripted_suites`).
pub(crate) use crate::scripted_suites::common;
/// The `cfg(test)`-seam fixtures (scripted adapters, conformance drivers),
/// shared with the scripted contract suites.
pub(crate) use crate::scripted_suites::support;

mod background;
mod durable;
mod managed_selection;
#[cfg(feature = "mcp-fixture")]
mod mcp_mrtr;
mod mcp_mrtr_managed;
mod mcp_recovery;
#[cfg(feature = "mcp-fixture")]
mod mcp_tasks;
mod mcp_tasks_managed;
mod runtime_client;
mod subagent;
mod tool_deadline;
