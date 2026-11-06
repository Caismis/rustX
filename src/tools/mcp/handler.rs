//! The rmcp client handler of one connection generation: client info,
//! inbound progress delivery, and the legacy `tools/list_changed` entry
//! point.

use std::sync::Arc;

use rmcp::ClientHandler;
use rmcp::model::{ClientCapabilities, ClientInfo, Implementation, ProgressNotificationParam};
use rmcp::service::RoleClient;

use super::invalidation::ToolListChangedSink;
use super::legacy_handshake_version;
use super::progress::McpProgressRouter;

#[derive(Clone)]
pub(super) struct McpClientHandler {
    info: ClientInfo,
    pub(super) progress: Arc<McpProgressRouter>,
    /// The legacy `tools/list_changed` entry point's sink: installed at most
    /// once, and only for a legacy-revision connection.
    tool_list_changed: Arc<std::sync::OnceLock<ToolListChangedSink>>,
}

impl McpClientHandler {
    pub(super) fn new() -> Self {
        Self {
            info: ClientInfo::new(
                ClientCapabilities::default(),
                Implementation::new("rustx", env!("CARGO_PKG_VERSION")),
            )
            // Only the legacy `initialize` handshake reads this field; the
            // inline lifecycle carries the negotiated candidate in each
            // request's `_meta`. Keeping it equal to the lifecycle's
            // `legacy_version` keeps the two paths from disagreeing.
            .with_protocol_version(legacy_handshake_version()),
            progress: Arc::new(McpProgressRouter::default()),
            tool_list_changed: Arc::new(std::sync::OnceLock::new()),
        }
    }

    pub(super) fn install_tool_list_changed_sink(&self, sink: ToolListChangedSink) {
        assert!(
            self.tool_list_changed.set(sink).is_ok(),
            "an MCP connection installs its invalidation sink exactly once"
        );
    }
}

impl ClientHandler for McpClientHandler {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.progress.deliver(params);
    }

    async fn on_tool_list_changed(&self, _context: rmcp::service::NotificationContext<RoleClient>) {
        let Some(sink) = self.tool_list_changed.get() else {
            return;
        };
        // The legacy entry point into the one shared sink: the same
        // admission and the same invalidation the subscription task applies.
        if let Some(admitted) = sink.admit().await {
            admitted.invalidate();
        }
    }

    fn get_info(&self) -> ClientInfo {
        self.info.clone()
    }
}
