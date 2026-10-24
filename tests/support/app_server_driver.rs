//! Test-only correlation driver shared by real process and socket tests.
use super::app_server_conformance::AppServerConformanceDriver;
use futures_util::{Sink, SinkExt, Stream, StreamExt, future::BoxFuture};
use rustx::app_server::protocol::{Notification, Request, Response};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot};

/// Why a request cannot complete: the first physical cause observed, an
/// orderly end of input or of output, or a malformed record, retained
/// verbatim for diagnosis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportEnded(pub String);

type Outcome = Result<Response, TransportEnded>;

/// One registered request: its waiter, and whether its record has been
/// physically accepted by the transport.
struct Pending {
    waiter: oneshot::Sender<Outcome>,
    sent: bool,
}

/// The single authority over every request and both transport frontiers.
///
/// - `output` closes first or alone: no record is accepted after it, so new
///   requests are refused and unsent ones fail with its cause, while sent
///   ones still await their responses.
/// - `input` closes the whole transport: no response can arrive, so every
///   pending request fails with the first cause recorded.
///
/// A request either registers before a frontier and is then failed by it,
/// or observes the frontier and never registers. Nothing waits on a
/// transport that can no longer complete it.
#[derive(Default)]
struct Correlation {
    pending: BTreeMap<String, Pending>,
    output: Option<TransportEnded>,
    input: Option<TransportEnded>,
}
impl Correlation {
    /// Output ended: fail every request whose record was not accepted.
    fn end_output(&mut self, cause: String) {
        let cause = self.output.get_or_insert(TransportEnded(cause)).clone();
        let unsent: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| !pending.sent)
            .map(|(id, _)| id.clone())
            .collect();
        for id in unsent {
            let pending = self.pending.remove(&id).unwrap();
            let _ = pending.waiter.send(Err(cause.clone()));
        }
    }
    /// The transport ended: no request can complete any longer.
    fn end(&mut self, cause: String) {
        self.output.get_or_insert(TransportEnded(cause.clone()));
        let cause = self.input.get_or_insert(TransportEnded(cause)).clone();
        for (_, pending) in std::mem::take(&mut self.pending) {
            let _ = pending.waiter.send(Err(cause.clone()));
        }
    }
}
type Shared = Arc<Mutex<Correlation>>;

/// What the writer is asked to do, in admission order.
enum Outbound {
    Record {
        id: String,
        record: String,
    },
    /// Close output after every record admitted before it.
    Close,
}

pub struct Driver {
    outgoing: mpsc::Sender<Outbound>,
    correlation: Shared,
    notifications: tokio::sync::Mutex<mpsc::Receiver<Notification>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Driver {
    /// `incoming` yields complete records or the transport's own failure;
    /// its end is an orderly end of input. `sink` accepts one record per
    /// send; a record is sent once that send completes.
    pub fn new<S, K>(incoming: S, sink: K) -> Self
    where
        S: Stream<Item = Result<String, String>> + Send + 'static,
        K: Sink<String, Error = String> + Send + 'static,
    {
        Self::with_capacity(incoming, sink, 64)
    }

    fn with_capacity<S, K>(incoming: S, sink: K, capacity: usize) -> Self
    where
        S: Stream<Item = Result<String, String>> + Send + 'static,
        K: Sink<String, Error = String> + Send + 'static,
    {
        let (outgoing, receiver) = mpsc::channel(capacity);
        let (notifications, events) = mpsc::channel(256);
        let correlation: Shared = Arc::default();
        let routing = correlation.clone();
        let reader = tokio::spawn(async move {
            tokio::pin!(incoming);
            let cause = loop {
                let record = match incoming.next().await {
                    Some(Ok(record)) => record,
                    Some(Err(error)) => break error,
                    None => break "end of input".into(),
                };
                if let Err(error) = route(&routing, &notifications, record).await {
                    break error;
                }
            };
            routing.lock().unwrap().end(cause);
        });
        let writing = correlation.clone();
        let writer = tokio::spawn(async move {
            match write(&writing, receiver, sink).await {
                Ok(()) => writing.lock().unwrap().end_output("end of output".into()),
                Err(error) => writing
                    .lock()
                    .unwrap()
                    .end(format!("write failed: {error}")),
            }
        });
        Self {
            outgoing,
            correlation,
            notifications: tokio::sync::Mutex::new(events),
            tasks: vec![reader, writer],
        }
    }

    /// Send `request` and wait for its correlated response, or for the
    /// cause that made it impossible: output closing before its record was
    /// accepted, or the transport ending before its response arrived. Both
    /// settle a request still waiting for outbound admission.
    /// # Errors
    /// The transport can no longer complete the request.
    pub async fn try_request(&self, request: Request) -> Outcome {
        let id = serde_json::to_string(&request.id).unwrap();
        let (waiter, mut outcome) = oneshot::channel();
        {
            let mut correlation = self.correlation.lock().unwrap();
            if let Some(closed) = &correlation.output {
                return Err(closed.clone());
            }
            let pending = Pending {
                waiter,
                sent: false,
            };
            assert!(
                correlation.pending.insert(id.clone(), pending).is_none(),
                "request ids are unique while pending"
            );
        }
        let record = serde_json::to_string(&request).unwrap();
        tokio::select! {
            biased;
            settled = &mut outcome => return settled.expect("every frontier settles its requests"),
            // A closed outbox means the writer ended; its frontier settles
            // this request.
            _ = self.outgoing.send(Outbound::Record { id, record }) => {}
        }
        outcome.await.expect("every frontier settles its requests")
    }

    /// Close output after every record already admitted, as stdio EOF does.
    /// Sent requests still receive their responses; later requests are
    /// refused with the output's end.
    pub async fn close_output(&self) {
        let _ = self.outgoing.send(Outbound::Close).await;
    }

    fn ended(&self) -> TransportEnded {
        self.correlation
            .lock()
            .unwrap()
            .input
            .clone()
            .expect("notifications end only after the transport ended")
    }

    pub async fn close(mut self) {
        for task in &self.tasks {
            task.abort();
        }
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Write admitted records in order, marking each sent once the sink accepts
/// it, until output is closed. A request that settled while queued (its
/// transport ended) is never written.
async fn write<K>(
    correlation: &Shared,
    mut receiver: mpsc::Receiver<Outbound>,
    sink: K,
) -> Result<(), String>
where
    K: Sink<String, Error = String>,
{
    tokio::pin!(sink);
    while let Some(Outbound::Record { id, record }) = receiver.recv().await {
        if !correlation.lock().unwrap().pending.contains_key(&id) {
            continue;
        }
        sink.send(record).await?;
        if let Some(pending) = correlation.lock().unwrap().pending.get_mut(&id) {
            pending.sent = true;
        }
    }
    sink.close().await
}

/// Deliver one complete record to its waiter or the notification queue.
async fn route(
    correlation: &Shared,
    notifications: &mpsc::Sender<Notification>,
    record: String,
) -> Result<(), String> {
    if record.len() > rustx::app_server::transport::MAX_MESSAGE_BYTES {
        return Err("complete physical App Server record exceeds the encoded byte limit".into());
    }
    let value: serde_json::Value =
        serde_json::from_str(&record).map_err(|error| format!("malformed record: {error}"))?;
    if let Some(id) = value.get("id") {
        let response = serde_json::from_str(&record)
            .map_err(|error| format!("malformed response: {error}"))?;
        let pending = correlation
            .lock()
            .unwrap()
            .pending
            .remove(&id.to_string())
            .ok_or_else(|| format!("uncorrelated response id {id}"))?;
        let _ = pending.waiter.send(Ok(response));
    } else {
        let notification = serde_json::from_str(&record)
            .map_err(|error| format!("malformed notification: {error}"))?;
        // A dropped notification consumer is not a transport failure.
        let _ = notifications.send(notification).await;
    }
    Ok(())
}

impl AppServerConformanceDriver for Driver {
    fn request(&self, request: Request) -> BoxFuture<'_, Response> {
        Box::pin(async move {
            let id = request.id.clone();
            self.try_request(request).await.unwrap_or_else(|ended| {
                panic!("request {id:?} unanswered: transport ended: {}", ended.0)
            })
        })
    }
    fn next_notification(&self) -> BoxFuture<'_, Notification> {
        Box::pin(async move {
            match self.notifications.lock().await.recv().await {
                Some(notification) => notification,
                None => panic!(
                    "no further notification: transport ended: {}",
                    self.ended().0
                ),
            }
        })
    }
}

pub fn jsonl<R, W>(reader: R, writer: W) -> Driver
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Send + 'static,
{
    Driver::new(lines(tokio::io::BufReader::new(reader)), jsonl_sink(writer))
}

/// One newline-terminated record per send, flushed before the send completes.
pub fn jsonl_sink<W>(writer: W) -> impl Sink<String, Error = String> + Send + 'static
where
    W: tokio::io::AsyncWrite + Send + 'static,
{
    let lines = tokio_util::codec::FramedWrite::new(writer, tokio_util::codec::LinesCodec::new());
    SinkExt::<String>::sink_map_err(lines, |error| error.to_string())
}

/// JSONL records of `reader`, ending after its first read failure.
pub fn lines<R>(reader: R) -> impl Stream<Item = Result<String, String>> + Send + 'static
where
    R: tokio::io::AsyncBufRead + Unpin + Send + 'static,
{
    use tokio::io::AsyncBufReadExt;
    futures_util::stream::unfold(Some(reader.lines()), |lines| async move {
        let mut lines = lines?;
        match lines.next_line().await {
            Ok(Some(line)) => Some((Ok(line), Some(lines))),
            Ok(None) => None,
            Err(error) => Some((Err(error.to_string()), None)),
        }
    })
}

pub const TOKEN: &str = "test-token-000000000000000000000000000000000000000";
pub async fn socket(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    try_socket(url, &format!("rustx.app-server.v57, rustx-token.{TOKEN}"))
        .await
        .unwrap()
}
/// Offer exact subprotocol values; a refused handshake is an `Err`.
pub async fn try_socket(
    url: &str,
    offered: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Error,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", offered.parse().unwrap());
    let (socket, response) = tokio_tungstenite::connect_async(request).await?;
    assert_eq!(
        response.headers()["sec-websocket-protocol"],
        "rustx.app-server.v57",
        "the response selects only the public subprotocol, never a secret"
    );
    Ok(socket)
}
pub async fn websocket(url: &str) -> Driver {
    websocket_offering(url, &format!("rustx.app-server.v57, rustx-token.{TOKEN}")).await
}
pub async fn websocket_offering(url: &str, offered: &str) -> Driver {
    let (writer, reader) = try_socket(url, offered).await.unwrap().split();
    Driver::new(frames(reader), text_sink(writer))
}

/// One WebSocket text message per record.
pub fn text_sink<K>(sink: K) -> impl Sink<String, Error = String> + Send + 'static
where
    K: Sink<tokio_tungstenite::tungstenite::Message, Error = tokio_tungstenite::tungstenite::Error>
        + Send
        + 'static,
{
    sink.sink_map_err(|error| error.to_string())
        .with(|record: String| std::future::ready(Ok(record.into())))
}

/// Text records of a WebSocket, ending at its close or first failure. Only a
/// completed close handshake is an orderly end of input; a peer that
/// vanished without one, or any other failure, is that end's recorded cause.
/// Polling continues through a Close frame so tungstenite completes the
/// handshake.
pub fn frames<S>(reader: S) -> impl Stream<Item = Result<String, String>> + Send + 'static
where
    S: Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Send
        + Unpin
        + 'static,
{
    use tokio_tungstenite::tungstenite::{Error, Message};
    futures_util::stream::unfold(Some(reader), |reader| async move {
        let mut reader = reader?;
        loop {
            return match reader.next().await? {
                Ok(Message::Text(text)) => Some((Ok(text.to_string()), Some(reader))),
                Err(Error::ConnectionClosed | Error::AlreadyClosed) => None,
                Ok(_) => continue,
                Err(error) => Some((Err(error.to_string()), None)),
            };
        }
    })
}
