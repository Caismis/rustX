//! Bounded byte delivery only. Semantic work belongs to `AppServerConnection`.
use std::{io, sync::Arc, time::Duration};

use futures_util::{Stream, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::connection::AppServerConnection;

pub mod stdio;
pub mod websocket;

/// Maximum UTF-8 JSON bytes in either direction, excluding stdio LF.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Maximum queued outbound records (plus one being written).
pub const OUTBOUND_MESSAGES: usize = 32;
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

fn enqueue(sender: &mpsc::Sender<String>, value: &impl Serialize) -> io::Result<()> {
    let mut record = Record(Vec::new());
    serde_json::to_writer(&mut record, value).map_err(io::Error::other)?;
    let record = String::from_utf8(record.0).map_err(io::Error::other)?;
    sender
        .try_send(record)
        .map_err(|_| failure("outbound capacity exhausted"))
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
    W: FnOnce(mpsc::Receiver<String>) -> F,
    F: Future<Output = io::Result<()>>,
{
    let _detach = Detach(connection.clone());
    let (outgoing, receiver) = mpsc::channel(OUTBOUND_MESSAGES);
    let write = writer(receiver);
    tokio::pin!(incoming, write);
    let mut requests: FuturesUnordered<BoxFuture<'_, _>> = FuturesUnordered::new();
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => return Ok(()),
            result = &mut write => return result,
            response = requests.next(), if !requests.is_empty() => {
                if let Some(Some(response)) = response { enqueue(&outgoing, &response)?; }
            }
            notification = connection.next_notification() => enqueue(&outgoing, &notification)?,
            record = incoming.next() => {
                let Some(record) = record else { return Ok(()); };
                let record = record?;
                if record.len() > MAX_MESSAGE_BYTES { return Err(failure("inbound message exceeds limit")); }
                if requests.len() == IN_FLIGHT_REQUESTS { return Err(failure("request capacity exhausted")); }
                let connection = connection.clone();
                requests.push(Box::pin(async move { connection.handle_json(&record).await }));
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
        receiver.try_recv().unwrap();
        enqueue(&sender, &0).unwrap();
        let mut record = Record(Vec::new());
        std::io::Write::write_all(&mut record, &vec![0; MAX_MESSAGE_BYTES]).unwrap();
        assert!(std::io::Write::write_all(&mut record, &[0]).is_err());
        assert_eq!(record.0.len(), MAX_MESSAGE_BYTES);
    }
}
