//! WebSocket admission and framing. Each admitted socket gets its own endpoint.
use super::{MAX_MESSAGE_BYTES, WRITE_TIMEOUT, failure};
use crate::{app_server::connection::AppServerConnection, app_server::host::AppServerHost};
use futures_util::{SinkExt, StreamExt};
use std::{io, sync::Arc, time::Duration};
use tokio::{net::TcpListener, task::JoinSet};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
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
pub const SUBPROTOCOL: &str = "rustx.app-server.v25";

/// Dedicated transport credential. Deliberately has no Debug/Serialize.
#[derive(Clone)]
pub struct Credential(String);
impl Credential {
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
    let mut clients = JoinSet::new();
    let stop = shutdown.child_token();
    let result = loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break Ok(()),
            _ = clients.join_next(), if !clients.is_empty() => {
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
    #[allow(clippy::result_large_err)]
    // tungstenite requires this concrete handshake response type.
    let callback = move |request: &Request, mut response: Response| {
        let offered: Vec<_> = request
            .headers()
            .get_all("sec-websocket-protocol")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(',').map(str::trim))
            .collect();
        let token = format!("rustx-token.{}", credential.0);
        if request.uri().path() != "/"
            || request.uri().query().is_some()
            || !offered.contains(&SUBPROTOCOL)
            || !offered.contains(&token.as_str())
        {
            return Err(http::Response::builder()
                .status(401)
                .body(Some("Unauthorized".into()))
                .expect("constant response"));
        }
        response.headers_mut().insert(
            "sec-websocket-protocol",
            http::HeaderValue::from_static(SUBPROTOCOL),
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
    let (mut writer, reader) = socket.split();
    let incoming = reader
        .take_while(|message| std::future::ready(!matches!(message, Ok(Message::Close(_)))))
        .filter_map(|message| async move {
            match message {
                Ok(Message::Text(text)) => Some(Ok(text.to_string())),
                Ok(Message::Binary(_)) => Some(Err(failure("binary protocol message rejected"))),
                Ok(_) => None,
                Err(error) => Some(Err(io::Error::other(error))),
            }
        });
    let endpoint = Arc::new(AppServerConnection::new(host));
    let result = super::serve(
        endpoint.clone(),
        incoming,
        |mut receiver| async move {
            while let Some(record) = receiver.recv().await {
                tokio::time::timeout(WRITE_TIMEOUT, writer.send(Message::Text(record.into())))
                    .await
                    .map_err(|_| failure("WebSocket write deadline exceeded"))?
                    .map_err(io::Error::other)?;
            }
            tokio::time::timeout(WRITE_TIMEOUT, writer.close())
                .await
                .map_err(|_| failure("WebSocket close deadline exceeded"))?
                .map_err(io::Error::other)
        },
        shutdown,
    )
    .await;
    if result.is_err() {
        endpoint.transport_failure();
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
