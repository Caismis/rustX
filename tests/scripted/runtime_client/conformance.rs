//! Issue #38/#130/#136: transport-independent Runtime Client conformance.
//!
//! Every scenario below runs unchanged through two framings:
//!
//! - `direct` — typed requests straight into `RuntimeClientEndpoint`, the
//!   semantic reference;
//! - `stdio-jsonl` — the same typed requests encoded as JSONL records,
//!   driven through a real `serve_stdio_jsonl_with_io` session over
//!   in-memory byte pipes, and decoded back.
//!
//! A transport is correct exactly when the two are indistinguishable here.
//! Issue #36 adds a `WebSocketDriverFactory` and one more generated test per
//! scenario; no scenario body changes, because no scenario names a framing,
//! a byte, or a transport error.
//!
//! Byte-level framing, record limits, stdout purity, EOF/broken-pipe
//! lifecycle, and backpressure are transport-specific and live in
//! `stdio_transport.rs` (same directory) instead.

use super::super::support::runtime_client_conformance as conformance;

/// Generates one test per scenario per transport.
macro_rules! conformance_scenarios {
    ($($scenario:ident),* $(,)?) => {
        $(
            mod $scenario {
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn direct() {
                    super::conformance::$scenario(&super::conformance::DirectDriverFactory).await;
                }

                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn stdio_jsonl() {
                    super::conformance::$scenario(&super::conformance::StdioJsonlDriverFactory)
                        .await;
                }
            }
        )*
    };
}

conformance_scenarios!(
    // Protocol / session
    initialize_admits_the_attachment,
    unsupported_protocol_version_is_typed,
    responses_correlate_request_ids,
    a_second_attachment_is_rejected,
    detach_then_reinitialize,
    shutdown_is_not_detach,
    // Conversation / attempt lifecycle
    submission_acceptance_is_not_settlement,
    inbound_batches_and_drains,
    cancellation_is_acceptance_not_settlement,
    // Tools
    before_start_cancellation_repairs_runtime_client,
    foreground_tool_lifecycle,
    parallel_tools_keep_independent_identities,
    background_execution_lifecycle,
    // Projections
    agent_status_is_runtime_owned,
    capability_projection_is_deterministic,
    capability_projection_covers_python_origins,
    snapshot_and_cursor_linearize,
    resync_required_and_snapshot_repair,
);

/// The MCP capability projection, driven through both transports.
///
/// Written out rather than generated because the MCP fixture re-runs this
/// binary as its own server child and therefore needs each test's exact
/// name.
#[cfg(all(unix, feature = "mcp-fixture"))]
mod capability_projection_covers_mcp_origins {
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn direct() {
        super::conformance::capability_projection_covers_mcp_origins(
            &super::conformance::DirectDriverFactory,
            "scripted_suites::runtime_client::conformance::capability_projection_covers_mcp_origins::direct",
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stdio_jsonl() {
        super::conformance::capability_projection_covers_mcp_origins(
            &super::conformance::StdioJsonlDriverFactory,
            "scripted_suites::runtime_client::conformance::capability_projection_covers_mcp_origins::stdio_jsonl",
        )
        .await;
    }
}
