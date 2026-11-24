//! Typed relay between a staged child Server and its Session's Server owner.
//! The inherited socket pair is only a transport. The parent still makes
//! every physical dispatch on its single authenticated Client connection.

use super::{ControlHandle, ControlShared, ControlWorker, LocalConnection};
use rustx_execution_protocol::transport::{CONTROL_COMMAND_TIMEOUT, encode_record};
use rustx_execution_protocol::{
    Admission, Capability, ClientReply, ClientRequest, ConnectionId, ControlReply,
};
use rustx_execution_transport::RecordChannel;
use std::collections::BTreeSet;
use std::io;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

/// Tracks whether a child-side connection could have created retained work
/// on the parent relay. The no-work proof seals future effectful sends.
#[derive(Clone, Default)]
pub struct DelegatedWork(Arc<AtomicU8>);

impl DelegatedWork {
    const OPEN: u8 = 0;
    const DISPATCHED: u8 = 1;
    const SEALED_UNUSED: u8 = 2;

    /// Seals a child that never dispatched retained work.
    #[must_use]
    pub fn seal_unused(&self) -> bool {
        self.0
            .compare_exchange(
                Self::OPEN,
                Self::SEALED_UNUSED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(super) fn admit(&self, request: &ClientRequest) -> io::Result<()> {
        // Only the relay's stateless reads can bypass this cut. A lost reply
        // or a failed send after admission cannot erase potential work.
        if matches!(
            request,
            ClientRequest::ReadLines { .. }
                | ClientRequest::ReadDocument { .. }
                | ClientRequest::ReadRange { .. }
                | ClientRequest::Search { .. }
                | ClientRequest::GetMetadata { .. }
                | ClientRequest::ReadDirectory { .. }
                | ClientRequest::ReadBashOutput { .. }
                | ClientRequest::SearchBashOutput { .. }
        ) {
            return Ok(());
        }
        match self.0.compare_exchange(
            Self::OPEN,
            Self::DISPATCHED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(Self::DISPATCHED) => Ok(()),
            Err(_) => Err(io::ErrorKind::NotConnected.into()),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataRequest {
    pub request: ClientRequest,
    pub remaining_ms: u32,
}

/// Separate wire return time from the physical dispatch budget.
pub const DATA_REPLY_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(serde::Serialize, serde::Deserialize)]
pub enum DataReply {
    Client(ClientReply),
    Rejected(Refusal),
    Unknown(Refusal),
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum ControlReceipt {
    Client(ControlReply),
    Rejected(Refusal),
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum Refusal {
    Busy,
    Denied,
    Unavailable,
    Timeout,
    Lost,
}

impl From<io::Error> for Refusal {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::WouldBlock => Self::Busy,
            io::ErrorKind::PermissionDenied => Self::Denied,
            io::ErrorKind::Unsupported | io::ErrorKind::NotConnected => Self::Unavailable,
            io::ErrorKind::TimedOut => Self::Timeout,
            _ => Self::Lost,
        }
    }
}

impl From<Refusal> for io::Error {
    fn from(error: Refusal) -> Self {
        match error {
            Refusal::Busy => io::ErrorKind::WouldBlock,
            Refusal::Denied => io::ErrorKind::PermissionDenied,
            Refusal::Unavailable => io::ErrorKind::NotConnected,
            Refusal::Timeout => io::ErrorKind::TimedOut,
            Refusal::Lost => io::ErrorKind::BrokenPipe,
        }
        .into()
    }
}

impl LocalConnection {
    /// Construct the child-side projection of its parent's exact Client.
    ///
    /// # Errors
    ///
    /// Returns an error when an inherited transport cannot be retained.
    pub fn delegated(
        admission: Admission,
        connection: ConnectionId,
        capabilities: BTreeSet<Capability>,
        data: UnixStream,
        control: UnixStream,
    ) -> io::Result<Self> {
        let channel = RecordChannel::unix(data);
        let shutdown = channel.shutdown_handle()?;
        let mut control = RecordChannel::unix(control);
        let shared = Arc::new(ControlShared::new());
        shared.arm_stop(control.shutdown_handle()?);
        let handle = ControlHandle {
            shared: shared.clone(),
        };
        let join = std::thread::Builder::new()
            .name("subagent-execution-control".into())
            .spawn(move || {
                while let Some(request) = shared.wait_next_command() {
                    let deadline = Some(Instant::now() + CONTROL_COMMAND_TIMEOUT);
                    let receipt = encode_record(&request)
                        .and_then(|encoded| control.send(&encoded, deadline))
                        .and_then(|()| control.receive::<ControlReceipt>(deadline));
                    match receipt {
                        Ok(ControlReceipt::Client(reply)) => {
                            if !shared.resolve(Some(reply), |_| false) {
                                break;
                            }
                        }
                        Ok(ControlReceipt::Rejected(error)) => shared.reject(error.into()),
                        _ => break,
                    }
                }
                shared.terminate(io::ErrorKind::BrokenPipe.into());
                shutdown.shutdown();
            })?;
        Ok(Self {
            owner: None,
            delegated_work: Some(DelegatedWork::default()),
            channel,
            control: Some(ControlWorker { handle, join }),
            usable: true,
            closed: false,
            capabilities,
            connection,
            admission,
        })
    }

    /// Shared dispatch witness for the delegated child epilogue.
    #[must_use]
    pub fn delegated_work(&self) -> Option<DelegatedWork> {
        self.delegated_work.clone()
    }

    /// The negotiated physical capabilities. A child can only receive a
    /// subset chosen by the parent from these capabilities.
    #[must_use]
    pub fn capabilities(&self) -> &BTreeSet<Capability> {
        &self.capabilities
    }

    /// Forwards one request the child relay has already authorized.
    ///
    /// # Errors
    ///
    /// Returns any dispatch gate or transport error. The normal binding
    /// send gate runs against the authenticated physical connection.
    pub fn forward_delegated(
        &mut self,
        request: &ClientRequest,
        deadline: Instant,
        expected_commit_size: Option<u64>,
        permit: impl Fn() -> io::Result<()>,
    ) -> io::Result<ClientReply> {
        if let ClientRequest::CommitWrite { ticket } = request {
            let size = expected_commit_size.ok_or(io::ErrorKind::InvalidInput)?;
            return match self.commit_write(*ticket, size, Some(deadline), &permit, &|| ()) {
                Ok(metadata) => Ok(ClientReply::Written {
                    ticket: *ticket,
                    metadata,
                }),
                Err(error) => match error.get_ref() {
                    Some(cause) if cause.is::<super::WriteOutcomeUnknown>() => {
                        Ok(ClientReply::CommitUnknown { ticket: *ticket })
                    }
                    Some(cause) if cause.is::<super::CommitCancelled>() => {
                        Ok(ClientReply::WriteCancelled { ticket: *ticket })
                    }
                    Some(cause)
                        if let Some(physical) = cause.downcast_ref::<super::PhysicalError>() =>
                    {
                        Ok(ClientReply::Error { kind: physical.0 })
                    }
                    _ => Err(error),
                },
            };
        }
        if let ClientRequest::Process(request) = request {
            return self
                .process_admitted(request, deadline, permit)
                .map(ClientReply::Process);
        }
        if let ClientRequest::Mcp(request) = request {
            let key = request.key().clone();
            let reply = self.mcp_by(request.clone(), deadline, permit)?;
            return Ok(ClientReply::Mcp(rustx_execution_protocol::mcp::Response {
                key,
                reply,
            }));
        }
        let reply = self.exchange_admitted(request, Some(deadline), permit, || ())?;
        if matches!(reply, ClientReply::Error { .. }) {
            return Ok(reply);
        }
        match request {
            ClientRequest::PrepareWrite { expectation, .. }
            | ClientRequest::BeginWrite { expectation, .. } => {
                // The child still validates and tracks the chunk stream. The
                // parent must verify the ticket it will use for cleanup.
                self.verify_write_prepared(reply.clone(), expectation)?;
            }
            ClientRequest::PrepareEdit {
                expectation,
                replacements,
                ..
            } => {
                if let Err(error) =
                    self.valid_edit_prepared(reply.clone(), expectation, replacements)
                    && let Err(error) = error.downcast::<super::RefusedEdit>()
                {
                    return Err(error);
                }
            }
            _ => {}
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustx_execution_protocol::process::{
        OperationId, ProcessOperationKey, ProcessRequest, ServerIncarnation,
    };
    use rustx_execution_protocol::{
        ClientInstanceId, DecimalU64, EnvironmentId, FileStamp, Generation, PROTOCOL_VERSION,
        SessionId,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn unused_delegation_is_sealed_against_late_effectful_dispatch() {
        let query = ClientRequest::GetMetadata {
            relative_path: PathBuf::from("example"),
        };
        let start = ClientRequest::Process(ProcessRequest::Start {
            key: ProcessOperationKey {
                incarnation: ServerIncarnation::generate(),
                session: SessionId::new("session"),
                instance: ClientInstanceId::new("client"),
                generation: Generation::try_from(1u64).unwrap(),
                operation: OperationId::new(),
            },
        });

        let unused = DelegatedWork::default();
        let sender = unused.clone();
        sender.admit(&query).unwrap();
        assert!(unused.seal_unused());
        assert_eq!(
            sender.admit(&start).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );

        let dispatched = DelegatedWork::default();
        dispatched.clone().admit(&start).unwrap();
        assert!(!dispatched.seal_unused());
    }

    #[test]
    fn delegated_write_preserves_the_commit_frame_and_unknown_cut() {
        for lose_receipt in [false, true] {
            let (data, peer) = UnixStream::pair().unwrap();
            let (control, _control_peer) = UnixStream::pair().unwrap();
            let ticket = DecimalU64::new(42);
            let stamp = FileStamp {
                size: DecimalU64::new(3),
                revision: "a".repeat(64),
                identity: "4:12".to_owned(),
                modified_at_ms: None,
            };
            let witness = stamp.clone();
            let physical = std::thread::spawn(move || {
                let mut peer = RecordChannel::unix(peer);
                let first: DataRequest = peer.receive(None).unwrap();
                assert!(matches!(
                    first.request,
                    ClientRequest::PrepareWrite { relative_path, .. } if relative_path == Path::new("witness")
                ));
                peer.send(
                    &encode_record(&DataReply::Client(ClientReply::WritePrepared {
                        ticket,
                        original: None,
                    }))
                    .unwrap(),
                    None,
                )
                .unwrap();
                let second: DataRequest = peer.receive(None).unwrap();
                assert_eq!(second.request, ClientRequest::CommitWrite { ticket });
                if !lose_receipt {
                    peer.send(
                        &encode_record(&DataReply::Client(ClientReply::Written {
                            ticket,
                            metadata: witness,
                        }))
                        .unwrap(),
                        None,
                    )
                    .unwrap();
                    assert!(
                        peer.receive_optional::<DataRequest>(None)
                            .unwrap()
                            .is_none()
                    );
                }
            });
            let mut connection = LocalConnection::delegated(
                Admission {
                    version: PROTOCOL_VERSION,
                    server_incarnation: ServerIncarnation::generate(),
                    session: SessionId::new("session"),
                    environment: EnvironmentId::new("env"),
                    instance: ClientInstanceId::new("client"),
                    generation: Generation::new(std::num::NonZeroU64::MIN),
                    workspace: PathBuf::from("/workspace"),
                },
                ConnectionId::new("live"),
                BTreeSet::from([Capability::ConditionalWrite]),
                data,
                control,
            )
            .unwrap();
            let result = connection.write_file(Path::new("witness"), b"abc", None, |_| Ok(()));
            if lose_receipt {
                let error = result.unwrap_err();
                assert!(
                    error
                        .get_ref()
                        .and_then(|cause| cause.downcast_ref::<crate::WriteOutcomeUnknown>())
                        .is_some()
                );
            } else {
                assert_eq!(result.unwrap(), stamp);
            }
            drop(connection);
            physical.join().unwrap();
        }
    }
}
