//! The shared record channel: one `EncodedRecord` per Unix frame or one
//! per WebSocket binary message, under a caller-armable absolute deadline.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use rustx_execution_protocol::MAX_ENCODED_FRAME_BYTES;
use rustx_execution_protocol::transport::{
    DeadlineStream, EncodedRecord, MAX_RECORD_BYTES, decode_record_body, read_optional_record,
    read_record,
};
use serde::de::DeserializeOwned;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::{StatusCode, header};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Error, HandshakeError, Message, WebSocket};

use crate::EXECUTION_PATH;
use crate::deadline::{ArmedDeadline, DeadlineTcp, arm, armed_deadline, remaining};
use crate::endpoint::{self, ExecutionUrl};

/// Any byte stream a WebSocket can run over: the deadline TCP adapter for
/// plaintext `ws`, or the rustls `StreamOwned` wrapping it for `wss`.
trait IoStream: Read + Write {}
impl<T: Read + Write> IoStream for T {}

fn websocket_config() -> WebSocketConfig {
    // One record per binary message, with RFC overhead bounded by the
    // same protocol limits the Unix framing enforces.
    WebSocketConfig::default()
        .max_message_size(Some(MAX_RECORD_BYTES))
        .max_frame_size(Some(MAX_RECORD_BYTES))
        .read_buffer_size(8192)
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_ENCODED_FRAME_BYTES + 32)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "execution record deadline expired")
}

/// Probes the armed absolute deadline without doing I/O. Buffered input
/// — an already-received frame or a queued Pong — must not extend a peer
/// past its budget, so every send/receive/decode leg checks explicitly.
fn check(deadline: &ArmedDeadline) -> io::Result<()> {
    remaining(deadline).map(|_| ())
}

/// Maps a transport-level error to `io::Error`. Diagnostics never carry
/// peer-controlled record bytes.
#[cfg(test)]
pub(crate) fn ws_error_for_test(error: Error) -> io::Error {
    ws_error(error)
}

fn ws_error(error: Error) -> io::Error {
    match error {
        Error::Io(error) => error,
        // A closed connection, a completed close handshake, or a raw
        // transport reset without a WS close frame are all the channel's
        // EOF — exactly what `shutdown(Both)` produces on the Unix byte
        // stream.
        Error::ConnectionClosed
        | Error::AlreadyClosed
        | Error::Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake) => {
            io::Error::new(io::ErrorKind::UnexpectedEof, "execution channel closed")
        }
        Error::Capacity(_) => invalid("execution record exceeds the protocol bound"),
        // Every other protocol violation maps to one static code: a
        // tungstenite diagnostic can echo peer-controlled HTTP upgrade
        // text or frame bytes and must never leak into our errors.
        _ => invalid("execution channel protocol error"),
    }
}

fn handshake_error<Mid: tungstenite::handshake::HandshakeRole>(
    error: HandshakeError<Mid>,
) -> io::Error {
    match error {
        HandshakeError::Failure(error) => ws_error(error),
        // DeadlineTcp never returns WouldBlock, so an interrupted
        // handshake cannot occur; treat it as a transport failure.
        HandshakeError::Interrupted(_) => io::Error::other("interrupted WebSocket handshake"),
    }
}

/// A duplicated transport handle that exists only to interrupt a blocked
/// peer: shutting the raw socket down wakes any in-flight read or write
/// without touching the channel's writer state. It is the transport-
/// agnostic form of the Unix revocation duplicate.
pub struct ChannelShutdown {
    unix: Option<UnixStream>,
    tcp: Option<TcpStream>,
}

impl ChannelShutdown {
    /// Shuts both directions of the duplicated handle down. Errors are
    /// intentionally discarded: an already-closed peer still wakes.
    pub fn shutdown(&self) {
        if let Some(stream) = &self.unix {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(stream) = &self.tcp {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    /// Polls the duplicated handle once without waiting. Readable bytes,
    /// peer shutdown and descriptor errors can all invalidate a pending lane.
    ///
    /// # Errors
    ///
    /// Returns the poll error if readiness cannot be checked.
    pub fn readable_now(&self) -> io::Result<bool> {
        let fd = self
            .as_fd()
            .ok_or_else(|| io::Error::other("shutdown handle holds no stream"))?;
        let mut fds = [nix::poll::PollFd::new(fd, nix::poll::PollFlags::POLLIN)];
        nix::poll::poll(&mut fds, nix::poll::PollTimeout::ZERO)?;
        Ok(fds[0].revents().is_some_and(|events| !events.is_empty()))
    }

    /// The raw transport descriptor behind the handle — read-only
    /// access for socket options and peeks (`MSG_PEEK`,
    /// `setsockopt(2)`): a duplicated descriptor configures and
    /// observes the shared socket. The handle itself never reads or
    /// writes.
    #[must_use]
    pub fn as_fd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        use std::os::fd::AsFd;
        self.unix
            .as_ref()
            .map(AsFd::as_fd)
            .or_else(|| self.tcp.as_ref().map(AsFd::as_fd))
    }
}

/// One typed execution record channel.
///
/// On Unix it is the protocol's proven length-prefixed byte stream. On
/// WebSocket it carries exactly one `EncodedRecord` body per binary
/// message — the RFC 6455 envelope is transport framing only and never
/// nests a UDS length prefix. Identical record bytes and identical record
/// limits (`MAX_RECORD_BYTES`) apply on both transports.
pub enum RecordChannel {
    /// The Unix byte-stream framing owned by `rustx-execution-protocol`.
    Unix(UnixStream),
    /// One `EncodedRecord` per binary WebSocket message.
    WebSocket(Box<WebSocketChannel>),
}

/// The WebSocket half of [`RecordChannel`] — construction is crate-only;
/// callers interact through the channel API.
pub struct WebSocketChannel {
    socket: WebSocket<Box<dyn IoStream + Send>>,
    /// A duplicate of the plaintext TCP peer — revocation shuts this down
    /// to wake a blocked read/write even while TLS state lives inside the
    /// WebSocket.
    tcp: TcpStream,
    deadline: ArmedDeadline,
}

impl RecordChannel {
    /// Wraps an authenticated Unix peer.
    #[must_use]
    pub fn unix(stream: UnixStream) -> Self {
        Self::Unix(stream)
    }

    /// Sends one encoded record under `deadline` (`None` = unbounded
    /// blocking). The record was already bounded at encode time; this
    /// writes exactly one frame/message and never splits it.
    ///
    /// # Errors
    ///
    /// Returns any transport or deadline failure; the caller retires the
    /// channel after the request gate.
    pub fn send(&mut self, record: &EncodedRecord, deadline: Option<Instant>) -> io::Result<()> {
        match self {
            Self::Unix(stream) => match deadline {
                Some(deadline) => {
                    let mut bounded = DeadlineStream::new(stream, deadline)?;
                    record.write_to(&mut bounded)?;
                    bounded.finish()
                }
                None => record.write_to(stream),
            },
            Self::WebSocket(channel) => {
                arm(&channel.deadline, deadline);
                check(&channel.deadline)?;
                channel
                    .socket
                    .send(Message::Binary(Bytes::copy_from_slice(record.as_bytes())))
                    .map_err(ws_error)?;
                check(&channel.deadline)
            }
        }
    }

    /// Receives and decodes the next record under `deadline`. A clean
    /// transport close reads as `UnexpectedEof`, matching the Unix byte
    /// stream. Malformed, text, truncated, or oversized input is
    /// `InvalidData` — the caller retires the channel.
    ///
    /// # Errors
    ///
    /// Returns `UnexpectedEof` on a clean close, `TimedOut` on deadline,
    /// or `InvalidData` on malformed input.
    pub fn receive<T: DeserializeOwned>(&mut self, deadline: Option<Instant>) -> io::Result<T> {
        match self {
            Self::Unix(stream) => match deadline {
                Some(deadline) => {
                    let mut bounded = DeadlineStream::new(stream, deadline)?;
                    let record = read_record(&mut bounded)?;
                    bounded.finish()?;
                    Ok(record)
                }
                None => read_record(stream),
            },
            Self::WebSocket(channel) => {
                arm(&channel.deadline, deadline);
                check(&channel.deadline)?;
                match channel.read_message()? {
                    Some(body) => {
                        check(&channel.deadline)?;
                        decode_record_body(&body)
                    }
                    None => Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "execution channel closed",
                    )),
                }
            }
        }
    }

    /// Like [`Self::receive`] but returns `None` on a clean close before
    /// any message — the client loop's termination signal. On WebSocket
    /// `None` requires a real RFC 6455 Close: a raw TCP EOF or reset —
    /// including one mid-frame — is an `UnexpectedEof` error, because a
    /// forced disconnect is not proof the peer finished cleanly.
    ///
    /// # Errors
    ///
    /// Returns `TimedOut` on deadline, `UnexpectedEof` on a raw
    /// transport close without the Close frame, or `InvalidData` on
    /// malformed input; a graceful close is `Ok(None)`.
    pub fn receive_optional<T: DeserializeOwned>(
        &mut self,
        deadline: Option<Instant>,
    ) -> io::Result<Option<T>> {
        match self {
            Self::Unix(stream) => match deadline {
                Some(deadline) => {
                    let mut bounded = DeadlineStream::new(stream, deadline)?;
                    let record = read_optional_record(&mut bounded)?;
                    bounded.finish()?;
                    Ok(record)
                }
                None => read_optional_record(stream),
            },
            Self::WebSocket(channel) => {
                arm(&channel.deadline, deadline);
                check(&channel.deadline)?;
                match channel.read_message()? {
                    Some(body) => {
                        check(&channel.deadline)?;
                        decode_record_body(&body).map(Some)
                    }
                    None => Ok(None),
                }
            }
        }
    }

    /// Duplicates the raw transport for revocation, independent of any
    /// writer lock: a `shutdown` on the returned handle interrupts a peer
    /// blocked in read or write.
    ///
    /// # Errors
    ///
    /// Fails if the raw transport cannot be duplicated.
    pub fn shutdown_handle(&self) -> io::Result<ChannelShutdown> {
        match self {
            Self::Unix(stream) => Ok(ChannelShutdown {
                unix: Some(stream.try_clone()?),
                tcp: None,
            }),
            Self::WebSocket(channel) => Ok(ChannelShutdown {
                unix: None,
                tcp: Some(channel.tcp.try_clone()?),
            }),
        }
    }

    /// Shuts the transport down in both directions.
    pub fn shutdown(&mut self) {
        match self {
            Self::Unix(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            Self::WebSocket(channel) => {
                let _ = channel.tcp.shutdown(Shutdown::Both);
            }
        }
    }

    /// Orderly teardown under `deadline`: a WebSocket peer receives the
    /// RFC 6455 Close frame before the transport drops, so its
    /// `receive_optional` reads a real clean close rather than a raw
    /// TCP EOF. Revocation and protocol retirement deliberately do not
    /// take this path — a forced shutdown carries no "finished cleanly"
    /// claim. The Unix byte stream's `shutdown` already delivers the
    /// identical clean EOF.
    ///
    /// # Errors
    ///
    /// Returns the write/flush failure; the raw transport is still shut
    /// down so the peer never waits on a Close that will not arrive.
    pub fn close_graceful(&mut self, deadline: Option<Instant>) -> io::Result<()> {
        match self {
            Self::Unix(stream) => {
                stream.shutdown(Shutdown::Both)?;
                Ok(())
            }
            Self::WebSocket(channel) => {
                arm(&channel.deadline, deadline);
                let result = (|| {
                    check(&channel.deadline)?;
                    channel.socket.close(None).map_err(ws_error)?;
                    channel.socket.flush().map_err(ws_error)?;
                    check(&channel.deadline)
                })();
                let _ = channel.tcp.shutdown(Shutdown::Both);
                result
            }
        }
    }
}

impl WebSocketChannel {
    /// Reads messages until a data-bearing or closing one arrives. RFC
    /// control frames are transport-level only: tungstenite queues the
    /// mandated Pong/Close reply automatically, so flushing here keeps the
    /// answer inside this call's armed deadline; a control flood burns the
    /// same absolute budget as record traffic. The armed deadline is
    /// probed on every loop so already-buffered frames cannot slip past
    /// it without an I/O check.
    fn read_message(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            check(&self.deadline)?;
            match self.socket.read() {
                Ok(Message::Binary(body)) => {
                    if body.len() > MAX_RECORD_BYTES {
                        return Err(invalid("execution record exceeds the protocol bound"));
                    }
                    return Ok(Some(body.to_vec()));
                }
                Ok(Message::Close(_)) => return Ok(None),
                Ok(Message::Text(_)) => {
                    return Err(invalid("execution channel carries binary records only"));
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                    self.socket.flush().map_err(ws_error)?;
                }
                Err(error) => return Err(ws_error(error)),
            }
        }
    }
}

/// Client-side `WebSocket` connect to a validated endpoint under one
/// absolute deadline: bounded TCP connect, then — for `wss` — the rustls
/// handshake completes before the HTTP upgrade, so the Hello credential
/// can never flow before certificate and hostname validation succeed.
/// `ca_file` is the operator-provisioned private CA bundle (PEM); `None`
/// uses the `WebPKI` roots alone.
///
/// # Errors
///
/// Returns any TCP, TLS, or upgrade failure — no credential bytes are
/// ever transmitted before TLS validation completes.
pub fn connect(
    endpoint: &ExecutionUrl,
    deadline: Instant,
    ca_file: Option<&Path>,
) -> io::Result<RecordChannel> {
    connect_with_trust(endpoint, deadline, ca_file, None)
}

/// Connects with only the certificate supplied by the owner-only bootstrap.
///
/// # Errors
///
/// Returns a certificate, TCP, TLS, or upgrade error before sending any record.
pub fn connect_provisioned(
    endpoint: &ExecutionUrl,
    deadline: Instant,
    certificate_der: &[u8],
) -> io::Result<RecordChannel> {
    if !endpoint.secure || !endpoint.address.ip().is_loopback() {
        return Err(invalid("provisioned TLS requires a loopback wss endpoint"));
    }
    connect_with_trust(endpoint, deadline, None, Some(certificate_der))
}

fn connect_with_trust(
    endpoint: &ExecutionUrl,
    deadline: Instant,
    ca_file: Option<&Path>,
    certificate_der: Option<&[u8]>,
) -> io::Result<RecordChannel> {
    let budget = deadline.saturating_duration_since(Instant::now());
    if budget.is_zero() {
        return Err(timed_out());
    }
    // The operator CA bundle is validated and loaded before any socket
    // is opened: a bad bundle fails without touching the network.
    let config = endpoint
        .secure
        .then(|| endpoint::client_config(ca_file, certificate_der))
        .transpose()?;
    let stream = TcpStream::connect_timeout(&endpoint.address, budget)?;
    if Instant::now() >= deadline {
        return Err(timed_out());
    }
    let tcp = stream.try_clone()?;
    let armed = armed_deadline();
    arm(&armed, Some(deadline));
    let deadline_tcp = DeadlineTcp::new(stream, armed.clone())?;
    let socket = if endpoint.secure {
        // `secure` endpoints always build a config at URL validation.
        let Some(config) = config else {
            return Err(invalid("wss endpoint requires a TLS config"));
        };
        let mut connection =
            rustls::ClientConnection::new(Arc::new(config), endpoint.server_name.clone())
                .map_err(io::Error::other)?;
        let mut transport = deadline_tcp;
        // complete_io drives the handshake; every read/write shares the
        // armed deadline, so a stalled peer cannot outlive it.
        while connection.is_handshaking() {
            connection.complete_io(&mut transport)?;
        }
        upgrade(endpoint, rustls::StreamOwned::new(connection, transport))?
    } else {
        upgrade(endpoint, deadline_tcp)?
    };
    check(&armed)?;
    Ok(RecordChannel::WebSocket(Box::new(WebSocketChannel {
        socket,
        tcp,
        deadline: armed,
    })))
}

fn upgrade<S: IoStream + Send + 'static>(
    endpoint: &ExecutionUrl,
    stream: S,
) -> io::Result<WebSocket<Box<dyn IoStream + Send>>> {
    let boxed: Box<dyn IoStream + Send> = Box::new(stream);
    tungstenite::client::client_with_config(
        endpoint.request_target.as_str(),
        boxed,
        Some(websocket_config()),
    )
    .map(|(socket, _response)| socket)
    .map_err(handshake_error)
}

/// Server-side upgrade of an accepted TCP peer under one absolute
/// deadline. The request must target exactly `/execution/v9` and must not
/// carry a browser `Origin` header — this channel speaks only to the
/// authenticated Execution Client, never to a browser context.
///
/// # Errors
///
/// Returns the handshake failure when the upgrade is rejected or the
/// deadline expires; the binding reservation is untouched.
pub fn accept(stream: TcpStream, deadline: Instant) -> io::Result<RecordChannel> {
    accept_with_tls(stream, deadline, None)
}

/// The ephemeral TLS identity of a directly connected loopback binding.
pub struct LoopbackTls {
    server: Arc<rustls::ServerConfig>,
    certificate: Vec<u8>,
}

impl LoopbackTls {
    /// Creates a fresh key and a certificate for this listener's literal IP.
    ///
    /// # Errors
    ///
    /// Returns an error for non-loopback addresses or a certificate setup failure.
    pub fn new(ip: std::net::IpAddr) -> io::Result<Self> {
        if !ip.is_loopback() {
            return Err(invalid("provisioned TLS requires a loopback address"));
        }
        let rcgen::CertifiedKey { cert, key_pair } =
            rcgen::generate_simple_self_signed(vec![ip.to_string()]).map_err(io::Error::other)?;
        let server = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            rustls_pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()).into(),
        )
        .map_err(io::Error::other)?;
        Ok(Self {
            server: Arc::new(server),
            certificate: cert.der().to_vec(),
        })
    }

    /// Returns the public certificate for the owner-only bootstrap file.
    #[must_use]
    pub fn certificate(&self) -> &[u8] {
        &self.certificate
    }

    /// Completes TLS and the WebSocket upgrade under the caller's deadline.
    ///
    /// # Errors
    ///
    /// Returns the TLS or upgrade failure without entering protocol admission.
    pub fn accept(&self, stream: TcpStream, deadline: Instant) -> io::Result<RecordChannel> {
        accept_with_tls(stream, deadline, Some(Arc::clone(&self.server)))
    }
}

#[allow(clippy::result_large_err)]
fn accept_with_tls(
    stream: TcpStream,
    deadline: Instant,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> io::Result<RecordChannel> {
    let tcp = stream.try_clone()?;
    let armed = armed_deadline();
    arm(&armed, Some(deadline));
    let mut deadline_tcp = DeadlineTcp::new(stream, armed.clone())?;
    let transport: Box<dyn IoStream + Send> = if let Some(tls) = tls {
        let mut connection = rustls::ServerConnection::new(tls).map_err(io::Error::other)?;
        while connection.is_handshaking() {
            connection
                .complete_io(&mut deadline_tcp)
                .map_err(io::Error::other)?;
        }
        Box::new(rustls::StreamOwned::new(connection, deadline_tcp))
    } else {
        Box::new(deadline_tcp)
    };
    let socket = tungstenite::accept_hdr_with_config(
        transport,
        |request: &Request, response: Response| {
            let rejected = |status| {
                let mut reply = tungstenite::http::Response::<Option<String>>::new(None);
                *reply.status_mut() = status;
                ErrorResponse::from(reply)
            };
            // Origin-form only: an absolute-form request-target carries a
            // scheme/authority and is refused outright, and path+query
            // must be exactly `/execution/v9` — a query or fragment is
            // never tolerated.
            let exact_path = request.uri().scheme().is_none()
                && request.uri().authority().is_none()
                && request
                    .uri()
                    .path_and_query()
                    .map(tungstenite::http::uri::PathAndQuery::as_str)
                    == Some(EXECUTION_PATH);
            if !exact_path {
                return Err(rejected(StatusCode::NOT_FOUND));
            }
            if request.headers().contains_key(header::ORIGIN) {
                return Err(rejected(StatusCode::FORBIDDEN));
            }
            Ok(response)
        },
        Some(websocket_config()),
    )
    .map_err(handshake_error)?;
    Ok(RecordChannel::WebSocket(Box::new(WebSocketChannel {
        socket,
        tcp,
        deadline: armed,
    })))
}
