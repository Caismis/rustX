//! Test-only correlation driver shared by real process and socket tests.
use super::app_server_conformance::AppServerConformanceDriver;
use futures_util::{Stream, StreamExt, future::BoxFuture};
use rustx::app_server::protocol::{Notification, Request, Response};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot};

/// Why the client side of a transport ended: the first physical cause
/// observed, an orderly end of input, or a malformed record, retained
/// verbatim for diagnosis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportEnded(pub String);

/// Correlation state and the transport's terminal cause share one lock:
/// a request either registers before termination, and termination then
/// fails it, or observes termination and never registers. Nothing waits on
/// a transport that has already ended.
#[derive(Default)]
struct Correlation {
    pending: BTreeMap<String, oneshot::Sender<Response>>,
    ended: Option<TransportEnded>,
}
impl Correlation {
    /// Record the first terminal cause and fail every pending request with it.
    fn end(&mut self, cause: String) {
        self.ended.get_or_insert(TransportEnded(cause));
        // Dropping each sender completes its waiter with the recorded cause.
        self.pending.clear();
    }
}
type Shared = Arc<Mutex<Correlation>>;

pub struct Driver {
    outgoing: mpsc::Sender<String>,
    correlation: Shared,
    notifications: tokio::sync::Mutex<mpsc::Receiver<Notification>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Driver {
    /// `incoming` yields complete records or the transport's own failure;
    /// its end is an orderly end of input. `writer` drains outbound records
    /// and reports its own transport failure.
    pub fn new<S, W, F>(incoming: S, writer: W) -> Self
    where
        S: Stream<Item = Result<String, String>> + Send + 'static,
        W: FnOnce(mpsc::Receiver<String>) -> F,
        F: Future<Output = Result<(), String>> + Send + 'static,
    {
        let (outgoing, receiver) = mpsc::channel(64);
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
        let ending = correlation.clone();
        let writing = writer(receiver);
        let writer = tokio::spawn(async move {
            if let Err(error) = writing.await {
                ending.lock().unwrap().end(format!("write failed: {error}"));
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
    /// transport's terminal cause when it ends first.
    /// # Errors
    /// The transport ended before the response arrived.
    pub async fn try_request(&self, request: Request) -> Result<Response, TransportEnded> {
        let (sender, receiver) = oneshot::channel();
        {
            let mut correlation = self.correlation.lock().unwrap();
            if let Some(ended) = &correlation.ended {
                return Err(ended.clone());
            }
            assert!(
                correlation
                    .pending
                    .insert(serde_json::to_string(&request.id).unwrap(), sender)
                    .is_none(),
                "request ids are unique while pending"
            );
        }
        // A closed outbox means the writer ended; its cause is recorded.
        let _ = self
            .outgoing
            .send(serde_json::to_string(&request).unwrap())
            .await;
        receiver.await.map_err(|_| self.ended())
    }

    fn ended(&self) -> TransportEnded {
        self.correlation
            .lock()
            .unwrap()
            .ended
            .clone()
            .expect("a pending request fails only after the transport ended")
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
        let sender = correlation
            .lock()
            .unwrap()
            .pending
            .remove(&id.to_string())
            .ok_or_else(|| format!("uncorrelated response id {id}"))?;
        let _ = sender.send(response);
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

pub fn jsonl<R, W>(reader: R, mut writer: W) -> Driver
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use tokio::io::{AsyncWriteExt, BufReader};
    Driver::new(lines(BufReader::new(reader)), |mut receiver| async move {
        while let Some(record) = receiver.recv().await {
            writer
                .write_all(format!("{record}\n").as_bytes())
                .await
                .map_err(|error| error.to_string())?;
            writer.flush().await.map_err(|error| error.to_string())?;
        }
        Ok(())
    })
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
    try_socket(url, &format!("rustx.app-server.v50, rustx-token.{TOKEN}"))
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
        "rustx.app-server.v50",
        "the response selects only the public subprotocol, never a secret"
    );
    Ok(socket)
}
pub async fn websocket(url: &str) -> Driver {
    websocket_offering(url, &format!("rustx.app-server.v50, rustx-token.{TOKEN}")).await
}
pub async fn websocket_offering(url: &str, offered: &str) -> Driver {
    use futures_util::SinkExt;
    let (mut writer, reader) = try_socket(url, offered).await.unwrap().split();
    Driver::new(frames(reader), |mut receiver| async move {
        while let Some(record) = receiver.recv().await {
            writer
                .send(record.into())
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })
}

/// Text records of a WebSocket, ending at its close or first failure. A
/// completed close handshake and a peer that vanished without one (for
/// example a process that exited) both end input; any other failure, such as
/// a reset, is that end's recorded cause. Polling continues through a Close
/// frame so tungstenite completes the handshake.
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
    use tokio_tungstenite::tungstenite::{Error, Message, error::ProtocolError};
    futures_util::stream::unfold(Some(reader), |reader| async move {
        let mut reader = reader?;
        loop {
            return match reader.next().await? {
                Ok(Message::Text(text)) => Some((Ok(text.to_string()), Some(reader))),
                Err(
                    Error::ConnectionClosed
                    | Error::AlreadyClosed
                    | Error::Protocol(ProtocolError::ResetWithoutClosingHandshake),
                ) => None,
                Ok(_) => continue,
                Err(error) => Some((Err(error.to_string()), None)),
            };
        }
    })
}
