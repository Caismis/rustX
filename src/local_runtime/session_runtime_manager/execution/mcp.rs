//! One stdio MCP session on its original admitted execution binding.

use rustx_execution_protocol::{
    mcp::{Reply, Request},
    process::{OperationId, ProcessOperationKey},
};
use std::sync::atomic::{AtomicU8, Ordering};

use super::{
    Arc, DataCapacity, ExecutionCancellation, Instant, LocalExecutionEndpoint,
    SessionExecutionScope, cancelled_before_dispatch, io,
};

pub(crate) struct StdioMcpBinding {
    endpoint: Arc<LocalExecutionEndpoint>,
    pub(crate) key: ProcessOperationKey,
    dispatch: Arc<OpenDispatch>,
}

#[derive(Default)]
struct OpenDispatch(AtomicU8);

impl OpenDispatch {
    const WAITING: u8 = 0;
    const MAY_HAVE_SENT: u8 = 1;
    const SEALED_UNUSED: u8 = 2;

    fn admit_open(&self) -> bool {
        matches!(
            self.0.compare_exchange(
                Self::WAITING,
                Self::MAY_HAVE_SENT,
                Ordering::AcqRel,
                Ordering::Acquire,
            ),
            Ok(_) | Err(Self::MAY_HAVE_SENT)
        )
    }

    fn seal_unused(&self) -> bool {
        matches!(
            self.0.compare_exchange(
                Self::WAITING,
                Self::SEALED_UNUSED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ),
            Ok(_) | Err(Self::SEALED_UNUSED)
        )
    }
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
        Ok(StdioMcpBinding {
            endpoint,
            key,
            dispatch: Arc::default(),
        })
    }
}

impl StdioMcpBinding {
    pub(crate) async fn exchange(
        &self,
        request: Request,
        deadline: Instant,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<Reply> {
        let opens_session = matches!(request, Request::Open { .. });
        let dispatch = self.dispatch.clone();
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
                            .with_active(|| {
                                if opens_session && !dispatch.admit_open() {
                                    return Err(cancelled_before_dispatch());
                                }
                                Ok(())
                            })
                            .ok_or_else(cancelled_before_dispatch)?
                    })
                })
            },
        )
        .await
    }

    /// Close may claim an unused identity before an Open reaches the wire.
    pub(crate) fn seal_unused(&self) -> bool {
        self.dispatch.seal_unused()
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

#[cfg(test)]
mod tests {
    use super::OpenDispatch;

    #[test]
    fn cleanup_seals_an_unsent_open_but_retains_a_possible_send() {
        let unsent = OpenDispatch::default();
        assert!(unsent.seal_unused());
        assert!(unsent.seal_unused());
        assert!(!unsent.admit_open());

        let dispatched = OpenDispatch::default();
        assert!(dispatched.admit_open());
        assert!(!dispatched.seal_unused());
        assert!(
            dispatched.admit_open(),
            "the same Open can recover its receipt"
        );
    }
}
