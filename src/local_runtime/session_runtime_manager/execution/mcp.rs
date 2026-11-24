//! One stdio MCP session on its original admitted execution binding.

use rustx_execution_protocol::{
    mcp::{Reply, Request},
    process::{OperationId, ProcessOperationKey},
};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAttempt {
    First,
    Subsequent,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenOutcome {
    RefusedBeforeClient,
    DispatchedOrUnknown,
}

struct OpenAttemptGuard<'a> {
    dispatch: &'a OpenDispatch,
    attempt: std::cell::Cell<Option<OpenAttempt>>,
}

impl<'a> OpenAttemptGuard<'a> {
    fn new(dispatch: &'a OpenDispatch) -> Self {
        Self {
            dispatch,
            attempt: std::cell::Cell::new(None),
        }
    }

    fn record(&self, attempt: OpenAttempt) {
        self.attempt.set(Some(attempt));
    }

    fn finish(&self, outcome: OpenOutcome) {
        if let Some(attempt) = self.attempt.take() {
            self.dispatch.finish_open(attempt, outcome);
        }
    }
}

impl Drop for OpenAttemptGuard<'_> {
    fn drop(&mut self) {
        self.finish(OpenOutcome::DispatchedOrUnknown);
    }
}

fn is_unsent_mcp_exchange(error: &io::Error) -> bool {
    error.get_ref().is_some_and(
        <dyn std::error::Error + Send + Sync + 'static>::is::<
            rustx_execution_server::UnsentMcpExchange,
        >,
    )
}

impl OpenDispatch {
    const WAITING: u8 = 0;
    const FIRST_OPEN_IN_FLIGHT: u8 = 1;
    const MAY_HAVE_SENT: u8 = 2;
    const SEALED_UNUSED: u8 = 3;

    fn admit_open(&self) -> Option<OpenAttempt> {
        let mut state = self.0.load(Ordering::Acquire);
        loop {
            match state {
                Self::WAITING => match self.0.compare_exchange_weak(
                    Self::WAITING,
                    Self::FIRST_OPEN_IN_FLIGHT,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return Some(OpenAttempt::First),
                    Err(actual) => state = actual,
                },
                Self::FIRST_OPEN_IN_FLIGHT => match self.0.compare_exchange_weak(
                    Self::FIRST_OPEN_IN_FLIGHT,
                    Self::MAY_HAVE_SENT,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return Some(OpenAttempt::Subsequent),
                    Err(actual) => state = actual,
                },
                Self::MAY_HAVE_SENT => return Some(OpenAttempt::Subsequent),
                Self::SEALED_UNUSED => return None,
                _ => unreachable!("invalid OpenDispatch state"),
            }
        }
    }

    fn finish_open(&self, attempt: OpenAttempt, outcome: OpenOutcome) {
        if attempt != OpenAttempt::First {
            return;
        }
        let next = match outcome {
            OpenOutcome::RefusedBeforeClient => Self::WAITING,
            OpenOutcome::DispatchedOrUnknown => Self::MAY_HAVE_SENT,
        };
        let _ = self.0.compare_exchange(
            Self::FIRST_OPEN_IN_FLIGHT,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
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

pub(crate) enum ReceiveExchange {
    Reply(Reply),
    Deferred,
    Failed(io::Error),
}

impl StdioMcpBinding {
    pub(crate) async fn receive(
        &self,
        sequence: u64,
        offset: u64,
        deadline: Instant,
        cancellation: &ExecutionCancellation,
    ) -> ReceiveExchange {
        let attempted = Arc::new(AtomicBool::new(false));
        let request = Request::Receive {
            key: self.key.clone(),
            sequence: sequence.into(),
            offset: offset.into(),
        };
        match self
            .exchange_inner(request, deadline, cancellation, Some(attempted.clone()))
            .await
        {
            Ok(reply) => ReceiveExchange::Reply(reply),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) && (!attempted.load(Ordering::Acquire) || is_unsent_mcp_exchange(&error)) =>
            {
                ReceiveExchange::Deferred
            }
            Err(error) => ReceiveExchange::Failed(error),
        }
    }

    pub(crate) async fn exchange(
        &self,
        request: Request,
        deadline: Instant,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<Reply> {
        self.exchange_inner(request, deadline, cancellation, None)
            .await
    }

    async fn exchange_inner(
        &self,
        request: Request,
        deadline: Instant,
        cancellation: &ExecutionCancellation,
        attempted: Option<Arc<AtomicBool>>,
    ) -> io::Result<Reply> {
        let opens_session = matches!(request, Request::Open { .. });
        let dispatch = self.dispatch.clone();
        SessionExecutionScope::dispatch_data_on(
            self.endpoint.clone(),
            cancellation,
            DataCapacity::WaitForSlot,
            deadline,
            move |endpoint, data, writer, deadline, cancellation| {
                let _permits = (data, writer);
                let mut request = Some(request);
                tokio::runtime::Handle::current().block_on(endpoint.wait_for_connection(
                    deadline,
                    |connection| {
                        if let Some(attempted) = &attempted {
                            attempted.store(true, Ordering::Release);
                        }
                        let request = request.take().expect("dispatch once on the current pair");
                        let guard = OpenAttemptGuard::new(&dispatch);
                        // The outer result belongs to obtaining the connection.
                        // Treat the exchange result independently of contention.
                        let result = connection.mcp_by(request, deadline, || {
                            if Instant::now() >= deadline {
                                return Err(io::ErrorKind::TimedOut.into());
                            }
                            cancellation
                                .with_active(|| {
                                    if opens_session {
                                        let attempt = dispatch
                                            .admit_open()
                                            .ok_or_else(cancelled_before_dispatch)?;
                                        guard.record(attempt);
                                    }
                                    Ok(())
                                })
                                .ok_or_else(cancelled_before_dispatch)?
                        });
                        let outcome = match &result {
                            Err(error) if is_unsent_mcp_exchange(error) => {
                                OpenOutcome::RefusedBeforeClient
                            }
                            _ => OpenOutcome::DispatchedOrUnknown,
                        };
                        guard.finish(outcome);
                        Ok(result)
                    },
                ))?
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
    use super::{OpenAttempt, OpenDispatch, OpenOutcome};

    #[test]
    fn cleanup_seals_an_unsent_open_but_retains_a_possible_send() {
        let unsent = OpenDispatch::default();
        assert!(unsent.seal_unused());
        assert!(unsent.seal_unused());
        assert!(unsent.admit_open().is_none());

        let refused = OpenDispatch::default();
        assert_eq!(refused.admit_open(), Some(OpenAttempt::First));
        assert!(
            !refused.seal_unused(),
            "in-flight initial Open cannot be sealed unused before its outcome"
        );
        refused.finish_open(OpenAttempt::First, OpenOutcome::RefusedBeforeClient);
        assert!(
            refused.seal_unused(),
            "pre-dispatch refusal of the first Open restores seal_unused"
        );
        assert!(refused.admit_open().is_none());

        let dispatched = OpenDispatch::default();
        assert_eq!(dispatched.admit_open(), Some(OpenAttempt::First));
        dispatched.finish_open(OpenAttempt::First, OpenOutcome::DispatchedOrUnknown);
        assert!(!dispatched.seal_unused());
        assert_eq!(
            dispatched.admit_open(),
            Some(OpenAttempt::Subsequent),
            "the same Open can recover its receipt"
        );
        dispatched.finish_open(OpenAttempt::Subsequent, OpenOutcome::RefusedBeforeClient);
        assert!(
            !dispatched.seal_unused(),
            "a later pre-dispatch refusal never cancels an earlier possible Open"
        );

        let overlapped = OpenDispatch::default();
        assert_eq!(overlapped.admit_open(), Some(OpenAttempt::First));
        assert_eq!(overlapped.admit_open(), Some(OpenAttempt::Subsequent));
        overlapped.finish_open(OpenAttempt::First, OpenOutcome::RefusedBeforeClient);
        assert!(
            !overlapped.seal_unused(),
            "an overlapped Open forbids rolling back the first attempt"
        );
    }
}
