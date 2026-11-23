//! One stdio MCP session on its original admitted execution binding.

use rustx_execution_protocol::{
    mcp::{Reply, Request},
    process::{OperationId, ProcessOperationKey},
};

use super::{
    Arc, DataCapacity, ExecutionCancellation, Instant, LocalExecutionEndpoint,
    SessionExecutionScope, cancelled_before_dispatch, io,
};

pub(crate) struct StdioMcpBinding {
    endpoint: Arc<LocalExecutionEndpoint>,
    pub(crate) key: ProcessOperationKey,
}

impl SessionExecutionScope {
    pub(crate) fn bind_stdio_mcp(
        &self,
        workspace: &std::path::Path,
    ) -> io::Result<StdioMcpBinding> {
        let endpoint = self.endpoint()?;
        if endpoint.workspace() != workspace {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        if !endpoint.dispatch_open() {
            return Err(io::ErrorKind::NotConnected.into());
        }
        if endpoint.stdio_mcp.get() != Some(&true) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the admitted Client does not offer stdio MCP",
            ));
        }
        let admission = endpoint
            .admission()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected))?;
        let key = ProcessOperationKey {
            incarnation: admission.server_incarnation,
            session: admission.session.clone(),
            instance: admission.instance.clone(),
            generation: admission.generation,
            operation: OperationId::new(),
        };
        Ok(StdioMcpBinding { endpoint, key })
    }
}

impl StdioMcpBinding {
    pub(crate) async fn exchange(
        &self,
        request: Request,
        deadline: Instant,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<Reply> {
        SessionExecutionScope::dispatch_data_on(
            self.endpoint.clone(),
            cancellation,
            DataCapacity::WaitForSlot,
            deadline,
            move |endpoint, data, writer, deadline, cancellation| {
                let _writer = writer;
                endpoint.with_connection(Some(data), true, |connection| {
                    connection.mcp_by(request, deadline, || {
                        cancellation
                            .with_active(|| ())
                            .ok_or_else(cancelled_before_dispatch)
                    })
                })
            },
        )
        .await
    }

    pub(crate) async fn settle_binding(&self) -> io::Result<()> {
        if matches!(
            self.endpoint.authority,
            super::EndpointAuthority::Delegated { .. }
        ) {
            self.abandon();
            return Err(io::Error::other(
                "delegated stdio settlement requires the parent relay",
            ));
        }
        self.endpoint.clone().settle().await
    }

    pub(crate) fn abandon(&self) {
        self.endpoint.cut_pair_if_open();
    }
}
