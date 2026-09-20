//! Test-only correlation driver shared by real process and socket tests.
use super::app_server_conformance::AppServerConformanceDriver;
use futures_util::{Stream, StreamExt, future::BoxFuture};
use rustx::app_server::protocol::{Notification, Request, Response};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot};

type Pending = Arc<Mutex<BTreeMap<String, oneshot::Sender<Response>>>>;
pub struct Driver {
    outgoing: mpsc::Sender<String>,
    pending: Pending,
    notifications: tokio::sync::Mutex<mpsc::Receiver<Notification>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Driver {
    pub fn new<S, W, F>(incoming: S, writer: W) -> Self
    where
        S: Stream<Item = String> + Send + 'static,
        W: FnOnce(mpsc::Receiver<String>) -> F,
        F: Future<Output = ()> + Send + 'static,
    {
        let (outgoing, receiver) = mpsc::channel(64);
        let (notifications, events) = mpsc::channel(256);
        let pending: Pending = Arc::default();
        let routing = pending.clone();
        let reader = tokio::spawn(async move {
            tokio::pin!(incoming);
            while let Some(record) = incoming.next().await {
                let value: serde_json::Value = serde_json::from_str(&record).unwrap();
                if let Some(id) = value.get("id") {
                    let sender = routing
                        .lock()
                        .unwrap()
                        .remove(&id.to_string())
                        .expect("correlated response");
                    let _ = sender.send(serde_json::from_str(&record).unwrap());
                } else {
                    notifications
                        .send(serde_json::from_str(&record).unwrap())
                        .await
                        .unwrap();
                }
            }
        });
        let writer = tokio::spawn(writer(receiver));
        Self {
            outgoing,
            pending,
            notifications: tokio::sync::Mutex::new(events),
            tasks: vec![reader, writer],
        }
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
impl AppServerConformanceDriver for Driver {
    fn request(&self, request: Request) -> BoxFuture<'_, Response> {
        Box::pin(async move {
            let (sender, receiver) = oneshot::channel();
            assert!(
                self.pending
                    .lock()
                    .unwrap()
                    .insert(serde_json::to_string(&request.id).unwrap(), sender)
                    .is_none()
            );
            self.outgoing
                .send(serde_json::to_string(&request).unwrap())
                .await
                .unwrap();
            receiver.await.unwrap()
        })
    }
    fn next_notification(&self) -> BoxFuture<'_, Notification> {
        Box::pin(async move { self.notifications.lock().await.recv().await.unwrap() })
    }
}

pub fn jsonl<R, W>(reader: R, mut writer: W) -> Driver
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let incoming =
        futures_util::stream::unfold(BufReader::new(reader).lines(), |mut lines| async move {
            lines.next_line().await.unwrap().map(|line| (line, lines))
        });
    Driver::new(incoming, |mut receiver| async move {
        while let Some(record) = receiver.recv().await {
            writer
                .write_all(format!("{record}\n").as_bytes())
                .await
                .unwrap();
            writer.flush().await.unwrap();
        }
    })
}

pub const TOKEN: &str = "test-token-000000000000000000000000000000000000000";
pub async fn socket(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("rustx.app-server.v13, rustx-token.{TOKEN}")
            .parse()
            .unwrap(),
    );
    let (socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(
        response.headers()["sec-websocket-protocol"],
        "rustx.app-server.v13"
    );
    socket
}
pub async fn websocket(url: &str) -> Driver {
    use futures_util::SinkExt;
    let (mut writer, reader) = socket(url).await.split();
    let incoming = reader.filter_map(|message| async move {
        use tokio_tungstenite::tungstenite::{Error, Message, error::ProtocolError};
        match message {
            Ok(Message::Text(text)) => Some(text.to_string()),
            Ok(_)
            | Err(
                Error::ConnectionClosed
                | Error::AlreadyClosed
                | Error::Protocol(ProtocolError::ResetWithoutClosingHandshake),
            ) => None,
            Err(error) => panic!("unexpected socket failure: {error}"),
        }
    });
    Driver::new(incoming, |mut receiver| async move {
        while let Some(record) = receiver.recv().await {
            writer.send(record.into()).await.unwrap();
        }
    })
}
