//! Bounded byte delivery only. Semantic work belongs to `AppServerConnection`.
use std::{io, sync::Arc, time::Duration};

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
/// Maximum queued outbound records (plus one being written).
pub const OUTBOUND_MESSAGES: usize = 32;
/// Explicit encoded-byte budget, excluding the single record being written.
/// Every queued record is size-checked before insertion, so the message queue
/// itself enforces both finite dimensions without a second permit system.
pub const OUTBOUND_QUEUE_BYTES: usize = OUTBOUND_MESSAGES * MAX_MESSAGE_BYTES;
/// Maximum concurrently polled semantic requests per physical connection.
pub const IN_FLIGHT_REQUESTS: usize = 16;
/// Deadline for each physical write, including flush.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

fn failure(message: &'static str) -> io::Error {
    io::Error::other(message)
}

// A size-limited serializer does not first allocate an unbounded encoded String.
struct Record(Vec<u8>);
impl io::Write for Record {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_MESSAGE_BYTES - self.0.len() {
            return Err(failure("outbound message exceeds limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One encoded, size-checked record and, for a delivery response, the owner
/// of its publication commit.
pub(super) struct Outbound {
    record: String,
    publication: Option<(Publication, bool)>,
}

/// The writer's only source of records. A delivery response is committed here,
/// after the writer is ready to transmit it and immediately before the
/// physical write: the publication linearization point. Revocation or
/// cancellation that wins before it replaces the record with the same id's
/// typed failure; after it, the record is on its way and is not retracted.
pub(super) struct Outgoing(mpsc::Receiver<Outbound>);
impl Outgoing {
    pub(super) async fn next(&mut self) -> Option<io::Result<String>> {
        let Outbound {
            record,
            publication,
        } = self.0.recv().await?;
        let Some((publication, success)) = publication else {
            return Some(Ok(record));
        };
        #[cfg(test)]
        publication.probe().before_publication.enter().await;
        Some(publication.publish_record(record, success))
    }
}

fn enqueue(sender: &mpsc::Sender<Outbound>, value: &impl Serialize) -> io::Result<()> {
    send(
        sender,
        Outbound {
            record: serialize_record(value)?,
            publication: None,
        },
    )
}

/// The response is encoded (and size-checked) now; its publication is
/// decided only by the writer.
fn enqueue_reply(sender: &mpsc::Sender<Outbound>, reply: Reply) -> io::Result<()> {
    let success = matches!(reply.response, super::protocol::Response::Success(_));
    send(
        sender,
        Outbound {
            record: serialize_record(&reply.response)?,
            publication: reply.publication.map(|publication| (publication, success)),
        },
    )
}

fn send(sender: &mpsc::Sender<Outbound>, outbound: Outbound) -> io::Result<()> {
    sender
        .try_send(outbound)
        .map_err(|_| failure("outbound capacity exhausted"))
}

pub(super) fn serialize_record(value: &impl Serialize) -> io::Result<String> {
    let mut record = Record(Vec::new());
    serde_json::to_writer(&mut record, value).map_err(io::Error::other)?;
    String::from_utf8(record.0).map_err(io::Error::other)
}

struct Detach(Arc<AppServerConnection>);
impl Drop for Detach {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// One termination path drops the writer/read futures and releases attachments.
/// No transport task outlives this future. Dropped semantic response waiters do
/// not cancel operations already admitted by the runtime owner.
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
    let (outgoing, receiver) = mpsc::channel(OUTBOUND_MESSAGES);
    let write = writer(Outgoing(receiver));
    tokio::pin!(incoming, write);
    let mut requests: FuturesUnordered<BoxFuture<'_, _>> = FuturesUnordered::new();
    // Alternate ready observations with protocol progress. Completion priority
    // drains at most IN_FLIGHT_REQUESTS before input; new requests cannot appear
    // without input admission. Neither direction can starve the other.
    let mut observation_turn = false;
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                if connection.server_draining() {
                    // The host has supervised semantic owners. Drain encoded
                    // records with the physical write deadline before close.
                    // Process shutdown revokes delivery publication first, as
                    // it does on the Product Host lane.
                    connection.revoke_delivery_access();
                    drop(requests);
                    drop(outgoing);
                    return write.await;
                }
                return Ok(());
            },
            result = &mut write => return result,
            notification = connection.next_notification(), if observation_turn => {
                enqueue(&outgoing, &notification)?;
                observation_turn = false;
            }
            response = requests.next(), if !requests.is_empty() => {
                if let Some(Some(reply)) = response { enqueue_reply(&outgoing, reply)?; }
                observation_turn = true;
            }
            record = incoming.next() => {
                let Some(record) = record else { return Ok(()); };
                let record = record?;
                if record.len() > MAX_MESSAGE_BYTES { return Err(failure("inbound message exceeds limit")); }
                if requests.len() == IN_FLIGHT_REQUESTS { return Err(failure("request capacity exhausted")); }
                let connection = connection.clone();
                requests.push(Box::pin(async move { connection.reply_json(&record).await }));
                observation_turn = true;
            }
            notification = connection.next_notification(), if !observation_turn => {
                enqueue(&outgoing, &notification)?;
                observation_turn = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut record = Record(Vec::new());
        std::io::Write::write_all(&mut record, &vec![0; MAX_MESSAGE_BYTES]).unwrap();
        assert!(std::io::Write::write_all(&mut record, &[0]).is_err());
        assert_eq!(record.0.len(), MAX_MESSAGE_BYTES);
    }
}
