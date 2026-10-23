//! Bounded byte delivery only. Semantic work belongs to `AppServerConnection`.
use std::{
    io,
    sync::{Arc, Mutex},
    task::Poll,
    time::Duration,
};

use futures_util::{Stream, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{connection::AppServerConnection, connection::Reply, delivery_access::Publication};

pub mod resources;
pub mod stdio;
pub mod websocket;

/// Maximum UTF-8 JSON bytes in either direction, excluding stdio LF.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Normal projection/read target leaves headroom for the complete envelope.
pub const NORMAL_RECORD_BYTES: usize = 786_432;
/// Maximum queued outbound records (plus one being written).
pub const OUTBOUND_MESSAGES: usize = 32;
/// Explicit queued encoded-byte budget; every record is size-checked before
/// insertion, so the existing queue proves both finite dimensions.
///
/// Extra encoded buffers are finite too: this serve task constructs at most
/// one candidate without suspending, the writer owns one record and one
/// selected acceptance copy, and WebSocket owns one Text frame plus its
/// `MAX_MESSAGE_BYTES + 1024` write buffer. Conservatively these add five
/// record bounds plus 1024 bytes. Pending subscription receipts carry one
/// pre-encoded supersession failure (valid 256-byte RPC id and fixed fields,
/// below 2 KiB), adding at most `OUTBOUND_MESSAGES * 2048` bytes. No blocked
/// sender keeps an additional encoded record and no second permit is needed.
pub const OUTBOUND_QUEUE_BYTES: usize = OUTBOUND_MESSAGES * MAX_MESSAGE_BYTES;
/// Maximum concurrently polled semantic requests per physical connection.
pub const IN_FLIGHT_REQUESTS: usize = 16;
/// Deadline for each physical write, including flush.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

fn failure(message: &'static str) -> io::Error {
    io::Error::other(message)
}

// A size-limited serializer does not first allocate an unbounded encoded String.
struct Record(Vec<u8>, usize);
impl io::Write for Record {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.1 - self.0.len() {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "outbound message exceeds limit",
            ));
        }
        let required = self.0.len() + bytes.len();
        if required > self.0.capacity() {
            let capacity = self
                .0
                .capacity()
                .max(8)
                .saturating_mul(2)
                .max(required)
                .min(self.1);
            self.0.reserve_exact(capacity - self.0.len());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One encoded, size-checked record and, for a delivery response, the owner
/// of its publication decision.
pub(crate) struct Outbound {
    record: String,
    publication: Option<(Publication, bool)>,
    registration: Option<super::registration::Publication>,
    association: Option<super::admission::Association>,
    retirement: Option<Arc<Mutex<bool>>>,
}

impl Outbound {
    pub(crate) fn notification(
        (notification, registration): (
            super::protocol::Notification,
            Option<super::registration::Publication>,
        ),
    ) -> io::Result<Self> {
        let encoded = if matches!(
            notification.notification,
            super::protocol::NotificationMethod::Auxiliary { .. }
        ) {
            serialize_record_limit(
                &notification,
                crate::runtime::auxiliary::delivery::RESULT_RECORD_BYTES_MAX,
            )
        } else {
            #[cfg(test)]
            {
                notification_fault::serialize(&notification)
            }
            #[cfg(not(test))]
            {
                serialize_record(&notification)
            }
        };
        match encoded {
            Ok(record) => Ok(Outbound {
                record,
                publication: None,
                registration,
                association: None,
                retirement: None,
            }),
            Err(error) if error.kind() == io::ErrorKind::FileTooLarge => {
                let (
                    super::protocol::NotificationMethod::Event {
                        registration_id,
                        target,
                        cursor,
                        ..
                    },
                    Some(registration),
                ) = (notification.notification, registration)
                else {
                    return Err(error);
                };
                let (registration, after_cursor) = registration.oversized();
                let notification = super::protocol::Notification {
                    jsonrpc: super::protocol::JsonRpcVersion::V2,
                    notification: super::protocol::NotificationMethod::ResyncRequired {
                        registration_id,
                        target,
                        reason: super::protocol::ResyncReason::RecordTooLarge,
                        checkpoint_required: true,
                        failed_at_cursor: Some(cursor),
                        after_cursor,
                        earliest_serviceable: cursor,
                    },
                };
                Ok(Outbound {
                    record: serialize_record(&notification)?,
                    publication: None,
                    registration: Some(registration),
                    association: None,
                    retirement: None,
                })
            }
            Err(error) => Err(error),
        }
    }

    /// The response is encoded (and size-checked) now; its publication is
    /// decided only when a transport accepts it.
    pub(crate) fn reply(mut reply: Reply) -> io::Result<Self> {
        let may_have_committed = reply.preserve_outcome();
        let limit = match &reply.response {
            super::protocol::Response::Success(success)
                if matches!(
                    success.result,
                    super::protocol::MethodResult::AuxiliarySubmitted { .. }
                        | super::protocol::MethodResult::AuxiliaryStatus { .. }
                        | super::protocol::MethodResult::AuxiliaryResult { .. }
                        | super::protocol::MethodResult::AuxiliarySubmissionLookup { .. }
                        | super::protocol::MethodResult::AuxiliaryCancelled { .. }
                        | super::protocol::MethodResult::AuxiliaryDiscarded { .. }
                ) =>
            {
                crate::runtime::auxiliary::delivery::RESULT_RECORD_BYTES_MAX
            }
            _ => MAX_MESSAGE_BYTES,
        };
        let (record, success) = match serialize_record_limit(&reply.response, limit) {
            Ok(record) => (
                record,
                matches!(reply.response, super::protocol::Response::Success(_)),
            ),
            Err(error) => {
                let id = match &reply.response {
                    super::protocol::Response::Success(response) => Some(response.id.clone()),
                    super::protocol::Response::Failure(response) => response.id.clone(),
                };
                let data = if may_have_committed {
                    super::protocol::ErrorData::OutcomeUnknown
                } else if error.kind() == io::ErrorKind::FileTooLarge {
                    super::protocol::ErrorData::ResponseTooLarge
                } else {
                    super::protocol::ErrorData::ResponseSerializationFailed
                };
                let response = super::rpc_error::failure(
                    id,
                    super::protocol::RpcError {
                        code: -32000,
                        message: "Response could not be encoded within the record budget".into(),
                        data: Some(data),
                    },
                );
                (serialize_record(&response)?, false)
            }
        };
        Ok(Self {
            record,
            publication: reply.publication.map(|publication| (publication, success)),
            association: reply.association,
            registration: reply.registration,
            retirement: None,
        })
    }

    pub(super) fn discarded(&self, cx: &std::task::Context<'_>) -> bool {
        self.retirement
            .as_ref()
            .is_some_and(|retired| *retired.lock().expect("carrier retirement"))
            || self
                .registration
                .as_ref()
                .is_some_and(|registration| registration.discarded(cx))
    }

    /// Hands this record to the transport. `accept` is the transport's one
    /// non-suspending acceptance of a record (the stdio pipe taking its first
    /// bytes, tungstenite taking its frame), returning `Pending` when it took
    /// nothing; the adapter calls this again when the transport can accept.
    /// For a delivery response this is the publication linearization point
    /// ([`Publication::poll_hand_off`]): which record is offered is decided
    /// in the same synchronous step, and fixed only once the transport
    /// accepted it. Returns the record as handed over and what `accept`
    /// returned.
    pub(crate) fn poll_hand_off<T>(
        &mut self,
        accept: impl FnOnce(&str) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<(String, Option<T>)>> {
        // Carrier teardown and physical acceptance share this finite fence.
        // Pending releases it immediately; a positive acceptance precedes any
        // concurrent teardown, whose caller then finishes the owned record.
        let retirement = self.retirement.clone();
        let retired = retirement
            .as_ref()
            .map(|value| value.lock().expect("carrier retirement"));
        if retired.as_deref().is_some_and(|value| *value) {
            drop(self.association.take());
            return Poll::Ready(Ok((String::new(), None)));
        }
        let Self {
            record,
            publication,
            registration,
            association,
            ..
        } = self;
        let outcome = if let Some(registration) = registration {
            match registration.hand_off(record, accept) {
                super::registration::Acceptance::Accepted(record, value) => {
                    Poll::Ready(Ok((record, Some(value))))
                }
                super::registration::Acceptance::Pending => Poll::Pending,
                super::registration::Acceptance::Discarded => {
                    Poll::Ready(Ok((String::new(), None)))
                }
                super::registration::Acceptance::Failed(error) => Poll::Ready(Err(error)),
            }
        } else if let Some((publication, success)) = publication {
            publication
                .poll_hand_off(record, *success, accept)
                .map_ok(|(replacement, accepted)| {
                    (
                        replacement.unwrap_or_else(|| std::mem::take(record)),
                        Some(accepted),
                    )
                })
        } else {
            accept(record).map_ok(|accepted| (std::mem::take(record), Some(accepted)))
        };
        if matches!(outcome, Poll::Ready(Ok(_))) {
            // This response's disposition is fixed at handoff. Later flush or
            // Stdio record completion is physical work, not another admission
            // lifetime. An ongoing read worker keeps its own reservation clone.
            drop(association.take());
        }
        outcome
    }
}

/// The writer's only source of records, in order. Each adapter hands a
/// record to its transport with [`Outbound::poll_hand_off`] only once every
/// earlier record has been fully written (stdio) or flushed to the socket
/// (WebSocket), so backpressure from an earlier record leaves later delivery
/// responses undecided.
pub(super) struct Outgoing {
    receiver: mpsc::Receiver<Outbound>,
    retirement: Arc<Mutex<bool>>,
    #[cfg(test)]
    enqueued: tokio::sync::watch::Receiver<bool>,
}
impl Outgoing {
    pub(super) async fn next(&mut self) -> Option<Outbound> {
        if *self.retirement.lock().expect("carrier retirement") {
            self.receiver.close();
            while self.receiver.try_recv().is_ok() {}
            return None;
        }
        let mut outbound = self.receiver.recv().await?;
        outbound.retirement = Some(Arc::clone(&self.retirement));
        Some(outbound)
    }
}

#[cfg(test)]
fn enqueue(sender: &mpsc::Sender<Outbound>, value: &impl Serialize) -> io::Result<()> {
    send(
        sender,
        Outbound {
            record: serialize_record(value)?,
            publication: None,
            association: None,
            registration: None,
            retirement: None,
        },
    )
}

#[derive(Clone)]
struct Outbox {
    sender: mpsc::Sender<Outbound>,
    #[cfg(test)]
    enqueued: tokio::sync::watch::Sender<bool>,
}
impl Outbox {
    fn new(sender: mpsc::Sender<Outbound>) -> Self {
        Self {
            sender,
            #[cfg(test)]
            enqueued: tokio::sync::watch::channel(false).0,
        }
    }
    #[cfg_attr(
        not(test),
        allow(
            clippy::unused_self,
            reason = "the queue owner carries the test-only exact enqueue watch"
        )
    )]
    fn send(&self, permit: mpsc::Permit<'_, Outbound>, outbound: Outbound) {
        permit.send(outbound);
        #[cfg(test)]
        self.enqueued.send_modify(|_| {});
    }
    #[cfg_attr(
        not(test),
        allow(
            clippy::unused_self,
            reason = "the queue owner signals its test-only post-close watch"
        )
    )]
    fn retire(&self, retirement: &Mutex<bool>, close: impl FnOnce()) {
        *retirement.lock().expect("carrier retirement") = true;
        close();
        #[cfg(test)]
        self.enqueued.send_replace(true);
    }
}

// Reserve before reading a native candidate. Losing a select turn drops this
// permit; no queue slot remains held while unrelated protocol work proceeds.
async fn enqueue_notification(sender: &Outbox, connection: &AppServerConnection) -> io::Result<()> {
    let permit = sender
        .sender
        .reserve()
        .await
        .map_err(|_| failure("outbound writer closed"))?;
    let candidate = connection.next_notification_candidate().await;
    sender.send(permit, Outbound::notification(candidate)?);
    Ok(())
}

// The admitted request keeps its original association and publication leases
// while waiting. It holds no encoded buffer or lock. This one absolute queue
// wait budget is separate from the writer's subsequent physical-write budget.
async fn enqueue_reply(sender: &Outbox, reply: Reply) -> io::Result<()> {
    let permit = tokio::time::timeout(WRITE_TIMEOUT, sender.sender.reserve())
        .await
        .map_err(|_| failure("outbound capacity deadline exceeded"))?
        .map_err(|_| failure("outbound writer closed"))?;
    sender.send(permit, Outbound::reply(reply)?);
    Ok(())
}

async fn resolve_and_enqueue(
    sender: Outbox,
    work: BoxFuture<'static, Option<Reply>>,
) -> io::Result<()> {
    if let Some(reply) = work.await {
        enqueue_reply(&sender, reply).await?;
    }
    Ok(())
}

#[cfg(test)]
fn send(sender: &mpsc::Sender<Outbound>, outbound: Outbound) -> io::Result<()> {
    sender
        .try_send(outbound)
        .map_err(|_| failure("outbound capacity exhausted"))
}

pub(super) fn serialize_record(value: &impl Serialize) -> io::Result<String> {
    serialize_record_limit(value, MAX_MESSAGE_BYTES)
}
fn serialize_record_limit(value: &impl Serialize, limit: usize) -> io::Result<String> {
    let mut record = Record(Vec::new(), limit);
    serde_json::to_writer(&mut record, value).map_err(|error| {
        let kind = if error.io_error_kind() == Some(io::ErrorKind::FileTooLarge)
            || error.to_string() == "native record exceeds limit"
        {
            io::ErrorKind::FileTooLarge
        } else {
            io::ErrorKind::InvalidData
        };
        io::Error::new(kind, error)
    })?;
    String::from_utf8(record.0).map_err(io::Error::other)
}

/// Fault only the disposable encoded candidate. Its native event and cut are
/// untouched; a smaller one-shot budget models an unexpected encoding growth.
#[cfg(test)]
pub(crate) mod notification_fault {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{
            Mutex, OnceLock,
            atomic::{AtomicBool, Ordering},
        },
    };
    struct Fault {
        limit: usize,
        fired: Arc<AtomicBool>,
    }
    fn faults() -> &'static Mutex<HashMap<String, Fault>> {
        static FAULTS: OnceLock<Mutex<HashMap<String, Fault>>> = OnceLock::new();
        FAULTS.get_or_init(Mutex::default)
    }
    pub(crate) struct Guard {
        registration: String,
        fired: Arc<AtomicBool>,
    }
    impl Guard {
        pub(crate) fn fired(&self) -> bool {
            self.fired.load(Ordering::SeqCst)
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            faults().lock().unwrap().remove(&self.registration);
        }
    }
    pub(crate) fn once(registration: &str, limit: usize) -> Guard {
        assert!(limit > 0 && limit < MAX_MESSAGE_BYTES);
        let fired = Arc::new(AtomicBool::new(false));
        assert!(
            faults()
                .lock()
                .unwrap()
                .insert(
                    registration.into(),
                    Fault {
                        limit,
                        fired: fired.clone()
                    }
                )
                .is_none()
        );
        Guard {
            registration: registration.into(),
            fired,
        }
    }
    struct Budget<'a> {
        inner: &'a mut Record,
        remaining: usize,
    }
    impl io::Write for Budget<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.remaining {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "notification fault budget",
                ));
            }
            let written = self.inner.write(bytes)?;
            self.remaining -= written;
            Ok(written)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }
    pub(super) fn serialize(
        notification: &super::super::protocol::Notification,
    ) -> io::Result<String> {
        use crate::runtime_client::RuntimeClientEvent;
        let super::super::protocol::NotificationMethod::Event {
            registration_id,
            event,
            ..
        } = &notification.notification
        else {
            return serialize_record(notification);
        };
        let canonical_user = event.facts().iter().any(|fact| match fact {
            RuntimeClientEvent::MessageCommitted { message, .. } => {
                matches!(message, crate::message::MessageBlock::User(user) if user.kind == crate::message::InboundKind::Message)
            }
            RuntimeClientEvent::MessageDescriptorCommitted { descriptor, .. } => {
                descriptor.role == "user" && descriptor.kind == "message"
            }
            _ => false,
        });
        if !canonical_user {
            return serialize_record(notification);
        }
        let Some(fault) = faults().lock().unwrap().remove(registration_id) else {
            return serialize_record(notification);
        };
        fault.fired.store(true, Ordering::SeqCst);
        let mut record = Record(Vec::new(), MAX_MESSAGE_BYTES);
        serde_json::to_writer(
            &mut Budget {
                inner: &mut record,
                remaining: fault.limit,
            },
            notification,
        )
        .map_err(|error| {
            io::Error::new(
                error.io_error_kind().unwrap_or(io::ErrorKind::InvalidData),
                error,
            )
        })?;
        String::from_utf8(record.0).map_err(io::Error::other)
    }
}

struct Detach(Arc<AppServerConnection>);
impl Drop for Detach {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Termination retires unaccepted delivery and finishes any physically accepted
/// record before releasing the writer. No transport task outlives this future.
/// Dropped response waiters do not cancel operations admitted by the runtime.
async fn serve<S, W, F>(
    connection: Arc<AppServerConnection>,
    incoming: S,
    writer: W,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    S: Stream<Item = io::Result<String>> + Send,
    W: FnOnce(Outgoing) -> F,
    F: Future<Output = io::Result<()>>,
{
    let _detach = Detach(connection.clone());
    let (sender, receiver) = mpsc::channel(OUTBOUND_MESSAGES);
    let outgoing = Outbox::new(sender);
    let retirement = Arc::new(Mutex::new(false));
    let write = writer(Outgoing {
        receiver,
        retirement: Arc::clone(&retirement),
        #[cfg(test)]
        enqueued: outgoing.enqueued.subscribe(),
    });
    tokio::pin!(incoming, write);
    let mut requests: FuturesUnordered<BoxFuture<'static, io::Result<()>>> =
        FuturesUnordered::new();
    // Invalid/unadmitted input may produce one reply outside the sixteen
    // admitted requests. Retain only that reply, then backpressure input.
    let mut immediate: Option<BoxFuture<'static, io::Result<()>>> = None;
    // Alternate ready observations with protocol progress. Completion priority
    // drains at most IN_FLIGHT_REQUESTS before input; new requests cannot appear
    // without input admission. Neither direction can starve the other.
    let mut observation_turn = false;
    let error = loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                outgoing.retire(&retirement, || {
                    if connection.server_draining() {
                        // Semantic owners are supervised separately. Only a
                        // physically accepted record survives carrier retirement.
                        connection.revoke_delivery_access();
                    } else {
                        connection.close();
                    }
                });
                drop(requests);
                drop(immediate);
                drop(outgoing);
                return write.await;
            },
            result = &mut write => return result,
            // Ready protocol replies formerly enqueued in the input arm.
            // Preserve that ordering before considering the next observation.
            result = async { immediate.as_mut().expect("pending immediate reply").await }, if immediate.is_some() => {
                immediate = None;
                if let Err(error) = result { break error; }
                observation_turn = true;
            }
            result = enqueue_notification(&outgoing, &connection), if observation_turn => {
                if let Err(error) = result { break error; }
                observation_turn = false;
            }
            response = requests.next(), if !requests.is_empty() => {
                if let Some(Err(error)) = response { break error; }
                observation_turn = true;
            }
            record = incoming.next(), if immediate.is_none() => {
                let Some(record) = record else {
                    // Input EOF is not an output I/O failure. A Stdio record
                    // whose first bytes were accepted must still finish its
                    // bytes and delimiter; retire unaccepted observations now.
                    outgoing.retire(&retirement, || connection.close());
                    drop(requests);
                    drop(immediate);
                    drop(outgoing);
                    return write.await;
                };
                let record = match record {
                    Ok(record) if record.len() <= MAX_MESSAGE_BYTES => record,
                    other => {
                        let error = other.err().unwrap_or_else(|| failure("inbound message exceeds limit"));
                        outgoing.retire(&retirement, || connection.close());
                        drop(requests);
                        drop(immediate);
                        drop(outgoing);
                        write.await?;
                        return Err(error);
                    }
                };
                match connection.prepare_json(&record) {
                    super::connection::Prepared::Ready(Some(reply)) => {
                        let sender = outgoing.clone();
                        immediate = Some(Box::pin(async move { enqueue_reply(&sender, *reply).await }));
                    },
                    super::connection::Prepared::Ready(None) => {},
                    super::connection::Prepared::Pending(work) => {
                        requests.push(Box::pin(resolve_and_enqueue(outgoing.clone(), work)));
                    },
                }
                observation_turn = true;
            }
            result = enqueue_notification(&outgoing, &connection), if !observation_turn => {
                if let Err(error) = result { break error; }
                observation_turn = false;
            }
        }
    };
    // Encoding/queue deadline failure retires the carrier just like input EOF. It is
    // not proof of a physical output failure: an already accepted record still
    // owns its exact remainder and delimiter, within the writer's deadline.
    outgoing.retire(&retirement, || {
        if connection.server_draining() {
            connection.revoke_delivery_access();
        } else {
            connection.close();
        }
    });
    drop(requests);
    drop(immediate);
    drop(outgoing);
    write.await?;
    Err(error)
}

/// Test-only view of the existing queue, used to establish exact saturation.
#[cfg(test)]
pub(crate) struct ControlledOutgoing(Outgoing);
#[cfg(test)]
impl ControlledOutgoing {
    pub(crate) fn retirement_watch(&self) -> RetirementWatch {
        RetirementWatch {
            changed: self.0.enqueued.clone(),
        }
    }
    pub(crate) async fn wait_full(&mut self) {
        loop {
            self.0.enqueued.borrow_and_update();
            if self.0.receiver.len() == OUTBOUND_MESSAGES {
                return;
            }
            self.0
                .enqueued
                .changed()
                .await
                .expect("live outbound queue");
        }
    }
    pub(crate) async fn next(&mut self) -> Option<Outbound> {
        self.0.next().await
    }
}

#[cfg(test)]
pub(crate) struct RetirementWatch {
    changed: tokio::sync::watch::Receiver<bool>,
}
#[cfg(test)]
impl RetirementWatch {
    pub(crate) async fn retired(&mut self) {
        self.changed
            .wait_for(|closed| *closed)
            .await
            .expect("live retirement owner");
    }
}

#[cfg(test)]
pub(crate) async fn serve_for_test<S, W, F>(
    connection: Arc<AppServerConnection>,
    incoming: S,
    writer: W,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    S: Stream<Item = io::Result<String>> + Send,
    W: FnOnce(ControlledOutgoing) -> F,
    F: Future<Output = io::Result<()>>,
{
    serve(
        connection,
        incoming,
        |outgoing| writer(ControlledOutgoing(outgoing)),
        shutdown,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::protocol::*;

    fn response(data: String) -> Response {
        Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("correlation".into()),
            result: MethodResult::ArtifactBytes { data },
        }))
    }

    #[test]
    fn t01_complete_envelope_exact_byte_limit_and_one_byte_overflow() {
        let overhead = serialize_record(&response(String::new())).unwrap().len();
        // Include multibyte UTF-8 and escaping in the actual complete envelope.
        let prefix = "é\n\"\\";
        let prefix_bytes = serde_json::to_string(prefix).unwrap().len() - 2;
        let content = format!(
            "{prefix}{}",
            "x".repeat(MAX_MESSAGE_BYTES - overhead - prefix_bytes)
        );
        assert_eq!(
            serialize_record(&response(content.clone())).unwrap().len(),
            MAX_MESSAGE_BYTES
        );
        assert_eq!(
            serialize_record(&response(format!("{content}x")))
                .unwrap_err()
                .kind(),
            io::ErrorKind::FileTooLarge
        );
    }

    #[test]
    fn t01_oversized_response_substitution_is_correlated_bounded_and_not_io_failure() {
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("invalid source encoding"))
            }
        }
        for may_have_committed in [false, true] {
            let outbound = Outbound::reply(Reply {
                response: response("\n".repeat(MAX_MESSAGE_BYTES)),
                publication: None,
                association: None,
                registration: None,
                may_have_committed,
            })
            .unwrap();
            assert!(outbound.record.len() < 1024);
            let Response::Failure(failure) = serde_json::from_str(&outbound.record).unwrap() else {
                panic!("bounded failure")
            };
            assert_eq!(failure.id, Some(RequestId::String("correlation".into())));
            assert_eq!(
                failure.error.data,
                Some(if may_have_committed {
                    ErrorData::OutcomeUnknown
                } else {
                    ErrorData::ResponseTooLarge
                })
            );
        }
        assert_eq!(
            serialize_record(&Invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn uncertain_failure_encoding_overflow_cannot_become_non_admission() {
        for data in [
            ErrorData::OutcomeUnknown,
            ErrorData::CommittedDurabilityUncertain,
        ] {
            let response = crate::app_server::rpc_error::failure(
                Some(RequestId::String("uncertain".into())),
                RpcError {
                    code: -32000,
                    message: "x".repeat(MAX_MESSAGE_BYTES),
                    data: Some(data),
                },
            );
            let outbound = Outbound::reply(Reply {
                response,
                publication: None,
                association: None,
                registration: None,
                may_have_committed: false,
            })
            .unwrap();
            let Response::Failure(failure) = serde_json::from_str(&outbound.record).unwrap() else {
                panic!("bounded uncertain failure")
            };
            assert!(outbound.record.len() < 1024);
            assert_eq!(failure.id, Some(RequestId::String("uncertain".into())));
            assert_eq!(failure.error.data, Some(ErrorData::OutcomeUnknown));
        }
    }

    #[test]
    fn t20_response_acceptance_releases_association_before_flush_and_fences_physical_cleanup() {
        let table = Arc::new(crate::app_server::admission::Associations::default());
        let id = RequestId::String("correlation".into());
        let owner = table.admit(&id, &Method::ServerInfo {}).unwrap();
        let physical = owner.clone();
        let mut outbound = Outbound::reply(Reply {
            response: response("bounded".into()),
            publication: None,
            registration: None,
            association: Some(owner),
            may_have_committed: false,
        })
        .unwrap();
        assert!(outbound.poll_hand_off::<()>(|_| Poll::Pending).is_pending());
        assert!(outbound.association.is_some());
        assert!(matches!(
            table.admit(&id, &Method::ServerInfo {}),
            Err(ErrorData::DuplicateRequestId)
        ));
        let Poll::Ready(Ok((record, Some(())))) = outbound.poll_hand_off(|_| Poll::Ready(Ok(())))
        else {
            panic!("bounded response was not accepted");
        };
        assert!(!record.is_empty());
        assert!(outbound.association.is_none());
        assert!(
            matches!(
                table.admit(&id, &Method::ServerInfo {}),
                Err(ErrorData::DuplicateRequestId)
            ),
            "physically ongoing work still owns capacity"
        );
        drop(physical);
        let successor = table.admit(&id, &Method::ServerInfo {}).unwrap();
        // The accepted record can remain alive through a pending physical flush.
        drop(outbound);
        assert!(
            matches!(
                table.admit(&id, &Method::ServerInfo {}),
                Err(ErrorData::DuplicateRequestId)
            ),
            "old record cleanup cannot erase its successor"
        );
        drop(successor);
        assert!(table.admit(&id, &Method::ServerInfo {}).is_ok());
    }
    #[test]
    fn t20_complete_record_candidate_capacity_obeys_the_physical_byte_budget() {
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "bounded-candidate",
            "result": { "body": "x".repeat(600_000), "items": (0..30_000).collect::<Vec<u32>>() },
        });
        let mut record = Record(Vec::new(), MAX_MESSAGE_BYTES);
        serde_json::to_writer(&mut record, &value).unwrap();
        assert!(record.0.len() < MAX_MESSAGE_BYTES);
        assert!(record.0.capacity() <= MAX_MESSAGE_BYTES);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&record.0).unwrap(),
            value
        );
        let rest = MAX_MESSAGE_BYTES - record.0.len();
        std::io::Write::write_all(&mut record, &vec![b' '; rest]).unwrap();
        let capacity = record.0.capacity();
        assert_eq!(record.0.len(), MAX_MESSAGE_BYTES);
        assert!(capacity <= MAX_MESSAGE_BYTES);
        assert!(std::io::Write::write_all(&mut record, b" ").is_err());
        assert_eq!(
            record.0.capacity(),
            capacity,
            "refused bytes allocate nothing"
        );
        assert_eq!(record.0.len(), MAX_MESSAGE_BYTES);
    }

    fn pending_reply() -> Reply {
        Reply {
            response: response("bounded".into()),
            publication: None,
            association: None,
            may_have_committed: false,
            registration: None,
        }
    }

    #[tokio::test]
    async fn transient_outbound_saturation_waits_for_capacity_without_encoding_another_record() {
        let (sender, mut receiver) = mpsc::channel(OUTBOUND_MESSAGES);
        for _ in 0..OUTBOUND_MESSAGES {
            enqueue(&sender, &0).unwrap();
        }
        let outgoing = Outbox::new(sender.clone());
        let pending = enqueue_reply(&outgoing, pending_reply());
        tokio::pin!(pending);
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(receiver.len(), OUTBOUND_MESSAGES);
        receiver.recv().await.unwrap();
        pending.await.unwrap();
        assert_eq!(receiver.len(), OUTBOUND_MESSAGES);
        for _ in 1..OUTBOUND_MESSAGES {
            assert_eq!(receiver.recv().await.unwrap().record, "0");
        }
        let record = receiver.recv().await.unwrap().record;
        let Response::Success(reply) = serde_json::from_str(&record).unwrap() else {
            panic!("the original correlated response must survive saturation");
        };
        assert_eq!(reply.id, RequestId::String("correlation".into()));
    }

    #[tokio::test(start_paused = true)]
    async fn sustained_outbound_saturation_keeps_one_absolute_wait_deadline() {
        let (sender, receiver) = mpsc::channel(OUTBOUND_MESSAGES);
        for _ in 0..OUTBOUND_MESSAGES {
            enqueue(&sender, &0).unwrap();
        }
        let outgoing = Outbox::new(sender.clone());
        let pending = enqueue_reply(&outgoing, pending_reply());
        tokio::pin!(pending);
        assert!(futures_util::poll!(&mut pending).is_pending());
        for _ in 0..9 {
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(futures_util::poll!(&mut pending).is_pending());
        }
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            pending.await.unwrap_err().to_string(),
            "outbound capacity deadline exceeded"
        );
        assert_eq!(receiver.len(), OUTBOUND_MESSAGES);
    }

    #[test]
    fn outbound_exact_capacity_and_encoded_size_fail_closed() {
        let (sender, mut receiver) = mpsc::channel(OUTBOUND_MESSAGES);
        for _ in 0..OUTBOUND_MESSAGES {
            enqueue(&sender, &0).unwrap();
        }
        assert!(enqueue(&sender, &0).is_err());
        assert_eq!(receiver.len(), OUTBOUND_MESSAGES);
        assert_eq!(receiver.try_recv().unwrap().record, "0");
        enqueue(&sender, &0).unwrap();
        let mut record = Record(Vec::new(), MAX_MESSAGE_BYTES);
        std::io::Write::write_all(&mut record, &vec![0; MAX_MESSAGE_BYTES]).unwrap();
        assert!(std::io::Write::write_all(&mut record, &[0]).is_err());
        assert_eq!(record.0.len(), MAX_MESSAGE_BYTES);
    }
}
