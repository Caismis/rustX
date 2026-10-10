//! WebSocket admission and framing. Each admitted socket gets its own endpoint.
use super::{MAX_MESSAGE_BYTES, Outbound, WRITE_TIMEOUT, failure};
use crate::{app_server::connection::AppServerConnection, app_server::host::AppServerHost};
use futures_util::{SinkExt, StreamExt};
use std::io::Write as _;
use std::{
    io,
    sync::{Arc, Mutex, MutexGuard},
    task::{Context, Poll, ready},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    task::JoinSet,
};
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async_with_config,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
        protocol::WebSocketConfig,
    },
};
use tokio_util::sync::CancellationToken;

/// Includes sockets still completing admission; excess sockets are dropped.
#[cfg(test)]
pub const MAX_CLIENTS: usize = 32;
/// Incomplete/authentication handshakes cannot retain slots indefinitely.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Browser clients offer this protocol plus `rustx-token.<dedicated token>`.
pub const SUBPROTOCOL: &str = "rustx.app-server.v42";

/// Dedicated transport credential. Deliberately has no Debug/Serialize.
#[derive(Clone)]
pub struct Credential(String);
impl Credential {
    pub(crate) fn offered(&self, offered: &[&str], prefix: &str) -> bool {
        let expected = format!("{prefix}{}", self.0);
        offered.iter().any(|value| {
            value.len() == expected.len()
                && value
                    .bytes()
                    .zip(expected.bytes())
                    .fold(0u8, |different, (a, b)| different | (a ^ b))
                    == 0
        })
    }
    pub(crate) fn same_secret(&self, other: &Self) -> bool {
        self.0 == other.0
    }
    /// Require a bounded URL/header-safe secret (at least 256 random bits when generated).
    /// # Errors
    /// Rejects empty, short, long or non-token characters.
    pub fn new(value: String) -> io::Result<Self> {
        if !(43..=128).contains(&value.len())
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(failure(
                "transport token must contain 43..128 base64url characters",
            ));
        }
        Ok(Self(value))
    }
}

/// One WebSocket stream, shared by a connection's reader and its writer in
/// place of `SplitStream`/`SplitSink`.
///
/// `SplitSink::start_send` only parks a frame in the sink's own slot, which a
/// later flush forwards to tungstenite once it obtains the shared lock; a
/// frame in that slot is neither undecided nor accepted. Here the writer has
/// no slot. Each side holds the stream for one non-suspending poll, and the
/// writer hands a record to tungstenite inside its own locked poll
/// ([`Self::poll_hand_off`]): the record is either still the writer's
/// undecided [`Outbound`], or tungstenite has taken its frame.
pub(crate) struct Socket<S> {
    stream: Mutex<WebSocketStream<S>>,
    /// Set when one side found the stream held by the other and waited.
    #[cfg(test)]
    pub(crate) waited: tokio::sync::watch::Sender<bool>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Socket<S> {
    pub(crate) fn new(stream: WebSocketStream<S>) -> Self {
        Self {
            stream: Mutex::new(stream),
            #[cfg(test)]
            waited: tokio::sync::watch::channel(false).0,
        }
    }

    fn lock(&self) -> MutexGuard<'_, WebSocketStream<S>> {
        #[cfg(test)]
        if self.stream.try_lock().is_err() {
            self.waited.send_replace(true);
        }
        self.stream.lock().expect("WebSocket stream")
    }

    pub(crate) fn poll_next(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Message, tokio_tungstenite::tungstenite::Error>>> {
        self.lock().poll_next_unpin(cx)
    }

    pub(crate) async fn next(
        &self,
    ) -> Option<Result<Message, tokio_tungstenite::tungstenite::Error>> {
        std::future::poll_fn(|cx| self.poll_next(cx)).await
    }

    /// Hands `outbound` to tungstenite, returning the record as handed over.
    ///
    /// Only once every earlier frame, and any pong the reader queued, has
    /// gone to the socket and tungstenite is ready is the record decided and
    /// handed over, by tungstenite's synchronous `start_send` under the same
    /// lock. That acceptance is the WebSocket publication point. tungstenite
    /// writes the frame to the socket at once (its write buffer size is 0);
    /// what the socket does not take stays in tungstenite's buffer, owned by
    /// the transport, and is never retracted.
    pub(crate) fn poll_hand_off(
        &self,
        cx: &mut Context<'_>,
        outbound: &mut Outbound,
    ) -> Poll<io::Result<String>> {
        let mut stream = self.lock();
        ready!(stream.poll_flush_unpin(cx)).map_err(io::Error::other)?;
        ready!(stream.poll_ready_unpin(cx)).map_err(io::Error::other)?;
        outbound
            .poll_hand_off(|record| {
                Poll::Ready(
                    stream
                        .start_send_unpin(Message::Text(record.into()))
                        .map_err(io::Error::other),
                )
            })
            .map_ok(|(record, ())| record)
    }

    pub(crate) async fn flush(&self) -> io::Result<()> {
        std::future::poll_fn(|cx| self.lock().poll_flush_unpin(cx))
            .await
            .map_err(io::Error::other)
    }

    /// Hands one record over and flushes it.
    pub(crate) async fn send(&self, mut outbound: Outbound) -> io::Result<()> {
        std::future::poll_fn(|cx| self.poll_hand_off(cx, &mut outbound)).await?;
        self.flush().await
    }

    pub(crate) async fn close(&self) -> io::Result<()> {
        std::future::poll_fn(|cx| self.lock().poll_close_unpin(cx))
            .await
            .map_err(io::Error::other)
    }
}

/// Serve until the process owner requests shutdown. Individual client failures
/// cannot stop the listener. All connection tasks are settled before returning.
/// # Errors
/// Returns listener accept failures after settling existing connections.
pub async fn serve(
    listener: TcpListener,
    host: AppServerHost,
    credential: Credential,
    shutdown: CancellationToken,
) -> io::Result<()> {
    serve_listener(
        listener,
        host,
        credential,
        shutdown,
        #[cfg(test)]
        None,
    )
    .await
}

// The optional test observer reports slots only after the listener has reaped
// tasks. It never participates in production admission or semantic ownership.
pub(crate) async fn serve_listener(
    listener: TcpListener,
    host: AppServerHost,
    credential: Credential,
    shutdown: CancellationToken,
    #[cfg(test)] slots: Option<tokio::sync::watch::Sender<usize>>,
) -> io::Result<()> {
    host.archives().serve_remote();
    host.uploads().serve_remote();
    let mut clients = JoinSet::new();
    let stop = shutdown.child_token();
    let result = loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break Ok(()),
            settled = clients.join_next(), if !clients.is_empty() => {
                if let Some(Err(error)) = settled {
                    let _ = writeln!(io::stderr(), "rustx app-server: connection task failed: {error}");
                }
                #[cfg(test)]
                if let Some(slots) = &slots { slots.send_replace(clients.len()); }
            },
            accepted = listener.accept() => {
                let (socket, _) = match accepted { Ok(value) => value, Err(error) => break Err(error) };
                let Some(lease) = host.admit_connection(true) else {
                    // A rejected socket never enters JSON-RPC or a task queue.
                    let _ = socket.try_write(b"HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
                    continue;
                };
                let host = host.clone();
                let credential = credential.clone();
                let stop = stop.clone();
                clients.spawn(async move {
                    let _lease = lease;
                    let _ = crate::app_server::archive_download::dispatch(socket, host, credential, stop).await;
                });
                #[cfg(test)]
                if let Some(slots) = &slots { slots.send_replace(clients.len()); }
            }
        }
    };
    stop.cancel();
    while clients.join_next().await.is_some() {}
    result
}

/// What one authenticated handshake admitted. Only transport credentials
/// produce these; nothing in a later JSON payload can.
enum Admission {
    /// The private one-read Product Host lane and its native authority.
    ProductHost(CancellationToken),
    /// The ordinary lane, with delivery access when its separate credential was offered.
    Ordinary(Option<CancellationToken>),
}

fn admit(host: &AppServerHost, credential: &Credential, request: &Request) -> Option<Admission> {
    let offered: Vec<_> = request
        .headers()
        .get_all("sec-websocket-protocol")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(',').map(str::trim))
        .collect();
    if request.uri().query().is_some() {
        return None;
    }
    if request.uri().path() == crate::app_server::product_host::PATH {
        return host
            .authenticate_product_host(&offered)
            .map(Admission::ProductHost);
    }
    if request.uri().path() != "/"
        || !offered.contains(&SUBPROTOCOL)
        || !credential.offered(&offered, "rustx-token.")
    {
        return None;
    }
    // Delivery access is additive to, never a substitute for, the ordinary
    // transport credential. A wrong one fails closed instead of downgrading.
    host.authenticate_delivery_access(&offered)
        .ok()
        .map(Admission::Ordinary)
}

pub(crate) async fn connection<S>(
    socket: S,
    host: AppServerHost,
    credential: Credential,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_MESSAGE_BYTES + 1024);
    let admission = Arc::new(std::sync::Mutex::new(None));
    let admitted = admission.clone();
    let handshake_host = host.clone();
    #[allow(clippy::result_large_err)]
    // tungstenite requires this concrete handshake response type.
    let callback = move |request: &Request, mut response: Response| {
        let Some(admission) = admit(&handshake_host, &credential, request) else {
            return Err(http::Response::builder()
                .status(401)
                .body(Some("Unauthorized".into()))
                .expect("constant response"));
        };
        let selected = match admission {
            Admission::ProductHost(_) => crate::app_server::product_host::SUBPROTOCOL,
            Admission::Ordinary(_) => SUBPROTOCOL,
        };
        *admitted.lock().expect("handshake admission") = Some(admission);
        response.headers_mut().insert(
            "sec-websocket-protocol",
            http::HeaderValue::from_static(selected),
        );
        Ok(response)
    };
    let socket = tokio::select! {
        () = shutdown.cancelled() => return Ok(()),
        socket = async { tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        accept_hdr_async_with_config(socket, callback, Some(config)),
    )
    .await
    .map_err(|_| failure("WebSocket handshake deadline exceeded"))?
    .map_err(io::Error::other) } => socket?,
    };
    let admitted = admission.lock().expect("handshake admission").take();
    let delivery = match admitted {
        Some(Admission::ProductHost(authority)) => {
            return crate::app_server::product_host::serve(socket, host, authority, shutdown).await;
        }
        Some(Admission::Ordinary(delivery)) => delivery,
        None => return Err(failure("WebSocket admission missing")),
    };
    let socket = Socket::new(socket);
    let socket = &socket;
    let incoming = futures_util::stream::poll_fn(|cx| socket.poll_next(cx))
        .take_while(|message| std::future::ready(!matches!(message, Ok(Message::Close(_)))))
        .filter_map(|message| async move {
            match message {
                Ok(Message::Text(text)) => Some(Ok(text.to_string())),
                Ok(Message::Binary(_)) => Some(Err(failure("binary protocol message rejected"))),
                Ok(_) => None,
                Err(error) => Some(Err(io::Error::other(error))),
            }
        });
    let endpoint = Arc::new(match delivery {
        Some(authorization) => AppServerConnection::with_delivery_access(host, authorization),
        None => AppServerConnection::new(host),
    });
    let result = super::serve(
        endpoint.clone(),
        incoming,
        |mut receiver| async move {
            while let Some(outbound) = receiver.next().await {
                tokio::time::timeout(WRITE_TIMEOUT, socket.send(outbound))
                    .await
                    .map_err(|_| failure("WebSocket write deadline exceeded"))??;
            }
            tokio::time::timeout(WRITE_TIMEOUT, socket.close())
                .await
                .map_err(|_| failure("WebSocket close deadline exceeded"))?
        },
        shutdown,
    )
    .await;
    if let Err(error) = &result {
        endpoint.transport_failure();
        // This listener survives an individual connection failure. Report
        // framing, capacity and write failures where the dev launcher already
        // captures diagnostics; never print the credential or RPC payload.
        let _ = writeln!(
            io::stderr(),
            "rustx app-server: authenticated WebSocket connection failed: {error}"
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_is_bounded_and_header_safe() {
        for length in [43, 128] {
            assert!(Credential::new("x".repeat(length)).is_ok());
        }
        for value in [
            String::new(),
            "x".repeat(42),
            "x".repeat(129),
            format!("{}\n", "x".repeat(43)),
            format!("{}=", "x".repeat(43)),
        ] {
            assert!(Credential::new(value).is_err());
        }
    }
}
