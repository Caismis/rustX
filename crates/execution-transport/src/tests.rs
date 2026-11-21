//! Unit tests for endpoint validation and the record channel over a real
//! loopback WebSocket handshake — never a UDS substitute.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use rustx_execution_protocol::transport::{MAX_RECORD_BYTES, encode_record};

use super::{ExecutionUrl, RecordChannel, accept, connect};
#[cfg(test)]
use crate::channel::ws_error_for_test;
use rustls_pki_types::pem::PemObject;

#[test]
fn endpoint_validation_accepts_loopback_ws_and_any_wss() {
    let ws = ExecutionUrl::validate("ws://127.0.0.1:9001/execution/v1").unwrap();
    assert!(!ws.is_secure());
    assert_eq!(ws.address().port(), 9001);
    let wss = ExecutionUrl::validate("wss://203.0.113.7/execution/v1").unwrap();
    assert!(wss.is_secure());
    assert_eq!(wss.address().port(), 443);
    let v6 = ExecutionUrl::validate("wss://[::1]:4443/execution/v1").unwrap();
    assert!(v6.is_secure());
}

#[test]
fn endpoint_validation_rejects_everything_insecure_or_ambiguous() {
    for url in [
        "http://127.0.0.1/execution/v1",
        "ws://127.0.0.1/other",
        "ws://127.0.0.1/execution/v1/",
        "ws://127.0.0.1/execution/v1?x=1",
        "ws://127.0.0.1/execution/v1#frag",
        "ws://127.0.0.1",
        "ws://user@127.0.0.1/execution/v1",
        "ws://localhost/execution/v1",
        "ws://execution.internal/execution/v1",
        "ws://192.0.2.1/execution/v1",
        "wss://not-an-ip.example/execution/v1",
        "wss://127.0.0.1:99999/execution/v1",
        "ws://127.0.0.1:9001/execution/v1/extra",
    ] {
        assert!(
            ExecutionUrl::validate(url).is_err(),
            "endpoint must be rejected: {url}"
        );
    }
}

#[test]
fn endpoint_validation_rejects_plaintext_nonloopback() {
    let error = ExecutionUrl::validate("ws://8.8.8.8/execution/v1").unwrap_err();
    assert!(error.contains("loopback"));
}

/// Pairs two channels over a real loopback `ws://` handshake and returns
/// them plus the server's revoke handle.
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
struct Probe {
    value: String,
}

fn ws_pair() -> (RecordChannel, RecordChannel) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        accept(stream, Instant::now() + Duration::from_secs(10)).unwrap()
    });
    let endpoint = ExecutionUrl::validate(&format!("ws://{address}/execution/v1")).unwrap();
    let client = connect(&endpoint, Instant::now() + Duration::from_secs(10), None).unwrap();
    (server.join().unwrap(), client)
}

#[test]
fn websocket_channel_carries_one_record_per_binary_message() {
    let (mut server, mut client) = ws_pair();
    let record = encode_record(&Probe {
        value: "héllo → snowman ☃".into(),
    })
    .unwrap();
    let deadline = Some(Instant::now() + Duration::from_secs(5));
    client.send(&record, deadline).unwrap();
    let echoed: Probe = server.receive(deadline).unwrap();
    assert_eq!(echoed.value, "héllo → snowman ☃");
    // The reply direction is symmetric.
    server.send(&record, deadline).unwrap();
    let back: Probe = client.receive(deadline).unwrap();
    assert_eq!(back.value, "héllo → snowman ☃");
    // Orderly teardown sends a real RFC 6455 Close — the only clean
    // optional EOF on WebSocket.
    server.close_graceful(deadline).unwrap();
    assert!(
        client
            .receive_optional::<Probe>(deadline)
            .unwrap()
            .is_none()
    );
}

#[test]
fn websocket_raw_eof_and_truncated_frame_are_never_clean_closes() {
    use std::io::Write;
    let deadline = Some(Instant::now() + Duration::from_secs(5));

    // A peer that sends a proper Close frame is the clean close.
    let (address, server) = ws_pair_with_raw_client();
    let mut raw = raw_ws_client(address);
    raw.close(None).unwrap();
    drop(raw);
    let mut server = server.join().unwrap();
    assert!(
        server
            .receive_optional::<Probe>(deadline)
            .unwrap()
            .is_none()
    );

    // A forced TCP disconnect without Close is an error, not EOF.
    let (address, server) = ws_pair_with_raw_client();
    let raw = raw_ws_client(address);
    drop(raw);
    let mut server = server.join().unwrap();
    assert_eq!(
        server
            .receive_optional::<Probe>(deadline)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::UnexpectedEof
    );

    // Half a binary frame then disconnect is a transport failure the
    // same way — truncation is never a clean close, and nothing
    // outstanding is retained.
    let (address, server) = ws_pair_with_raw_client();
    let mut raw = raw_ws_client(address);
    // FIN + opcode 0x2, 16-bit length 512, only 5 payload bytes.
    raw.get_mut()
        .write_all(&[0x82, 126, 0, 2, 0, 1, 2, 3, 4])
        .unwrap();
    raw.get_mut().flush().unwrap();
    drop(raw);
    let mut server = server.join().unwrap();
    // An error kind is expected; what matters is it is never Ok(None).
    assert!(server.receive_optional::<Probe>(deadline).is_err());
}

/// A raw RFC 6455 client over a plain blocking `TcpStream` — real frames,
/// no deadline adapter, used to drive malformed/off-contract input.
fn raw_ws_client(address: std::net::SocketAddr) -> tungstenite::WebSocket<std::net::TcpStream> {
    let stream = std::net::TcpStream::connect(address).unwrap();
    tungstenite::client::client_with_config(
        format!("ws://{address}/execution/v1").as_str(),
        stream,
        // The probe client is unbounded so it can send what the contract
        // forbids; the *server* config stays bound and must reject it.
        Some(
            tungstenite::protocol::WebSocketConfig::default()
                .max_message_size(None)
                .max_frame_size(None),
        ),
    )
    .unwrap()
    .0
}

fn ws_pair_with_raw_client() -> (std::net::SocketAddr, std::thread::JoinHandle<RecordChannel>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        accept(stream, Instant::now() + Duration::from_secs(10)).unwrap()
    });
    (address, server)
}

#[test]
fn websocket_channel_rejects_text_messages_before_deserialize() {
    let (address, server) = ws_pair_with_raw_client();
    let mut raw = raw_ws_client(address);
    let mut server = server.join().unwrap();
    tungstenite::WebSocket::send(&mut raw, tungstenite::Message::text("not a record")).unwrap();
    let error = server
        .receive::<serde_json::Value>(Some(Instant::now() + Duration::from_secs(5)))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn websocket_rejects_oversize_binary_before_deserialize() {
    let (address, server) = ws_pair_with_raw_client();
    let mut raw = raw_ws_client(address);
    let mut server = server.join().unwrap();
    let oversized = vec![b'x'; MAX_RECORD_BYTES + 1];
    tungstenite::WebSocket::send(&mut raw, tungstenite::Message::binary(oversized)).unwrap();
    let error = server
        .receive::<serde_json::Value>(Some(Instant::now() + Duration::from_secs(5)))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn websocket_deadline_expires_on_a_silent_peer() {
    let (mut server, mut client) = ws_pair();
    // No peer ever sends: the armed deadline must fire, not block forever.
    let deadline = Instant::now() + Duration::from_millis(150);
    let error = client
        .receive::<serde_json::Value>(Some(deadline))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    // Poll timeouts are millisecond-granularity; the expiry lands within
    // a few ms of the armed instant.
    assert!(Instant::now() + Duration::from_millis(10) >= deadline);
    // Sending a record still works — the channel itself is unharmed by the
    // peer's silence; the server receives it on its own deadline.
    let record = encode_record(&serde_json::json!({"ping": true})).unwrap();
    client
        .send(&record, Some(Instant::now() + Duration::from_secs(5)))
        .unwrap();
    let echoed: serde_json::Value = server
        .receive(Some(Instant::now() + Duration::from_secs(5)))
        .unwrap();
    assert_eq!(echoed["ping"], true);
}

#[test]
fn websocket_shutdown_handle_wakes_a_blocked_reader() {
    let (_server, mut client) = ws_pair();
    let handle = client.shutdown_handle().unwrap();
    let waiting = std::thread::spawn(move || {
        client
            .receive::<serde_json::Value>(None)
            .map(|_: serde_json::Value| ())
            .unwrap_err()
    });
    std::thread::sleep(Duration::from_millis(100));
    handle.shutdown();
    let error = waiting.join().unwrap();
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::InvalidData
        ),
        "shutdown must wake the blocked reader, got {error:?}"
    );
}

#[test]
fn unix_channel_roundtrips_the_same_records() {
    let (left, right) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut left = RecordChannel::unix(left);
    let mut right = RecordChannel::unix(right);
    let record = encode_record(&serde_json::json!({"kind": "ping"})).unwrap();
    left.send(&record, None).unwrap();
    let echoed: serde_json::Value = right.receive(None).unwrap();
    assert_eq!(echoed["kind"], "ping");
    drop(left);
    assert_eq!(
        right.receive_optional::<serde_json::Value>(None).unwrap(),
        None
    );
}

#[test]
fn unix_shutdown_handle_wakes_a_blocked_reader() {
    let (left, right) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut left = RecordChannel::unix(left);
    let _right = right;
    let handle = left.shutdown_handle().unwrap();
    let waiting = std::thread::spawn(move || {
        left.receive::<serde_json::Value>(None)
            .map(|_: serde_json::Value| ())
            .unwrap_err()
    });
    std::thread::sleep(Duration::from_millis(100));
    handle.shutdown();
    let _ = waiting.join();
}

#[test]
fn websocket_server_rejects_wrong_path_and_origin() {
    for (request, reason) in [
        (
            "GET /other HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            "wrong path",
        ),
        (
            "GET /execution/v1 HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nOrigin: https://example.test\r\n\r\n",
            "browser Origin",
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let _ = stream.set_nodelay(true);
            accept(stream, Instant::now() + Duration::from_secs(5))
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client.write_all(request.as_bytes()).unwrap();
        let mut reply = Vec::new();
        client
            .take(4096)
            .read_to_end(&mut reply)
            .unwrap_or_default();
        assert!(
            server.join().unwrap().is_err(),
            "server must reject upgrade: {reason}"
        );
    }
}

// ---- wss: real loopback TLS with a fixture CA and IP-SAN leaf ----

/// A fixture CA plus a leaf certificate for the given IP SANs, all PEM.
struct TlsFixture {
    ca: String,
    leaf: String,
    key: String,
}

fn tls_fixture(ips: &[std::net::IpAddr]) -> TlsFixture {
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();

    let mut leaf_params = rcgen::CertificateParams::new(Vec::new()).unwrap();
    leaf_params.subject_alt_names = ips
        .iter()
        .map(|ip| rcgen::SanType::IpAddress(*ip))
        .collect();
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf = leaf_params.signed_by(&leaf_key, &ca, &ca_key).unwrap();
    TlsFixture {
        ca: ca.pem(),
        leaf: leaf.pem(),
        key: leaf_key.serialize_pem(),
    }
}

/// A blocking TLS server side — the trusted ingress shape: TLS terminates
/// on the leaf+fixture CA, and the plaintext stream behind it is where the
/// real `tungstenite::accept` upgrade runs. Returns the first record
/// payload the client ever wrote, or the TLS/handshake error.
fn tls_server_side(
    listener: TcpListener,
    leaf_pem: &str,
    key_pem: &str,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    let cert = rustls_pki_types::CertificateDer::from_pem_slice(leaf_pem.as_bytes()).unwrap();
    let key = rustls_pki_types::PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept()?;
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(std::io::Error::other)?
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .map_err(std::io::Error::other)?;
        let mut connection = rustls::ServerConnection::new(std::sync::Arc::new(config))
            .map_err(std::io::Error::other)?;
        let mut transport = stream;
        while connection.is_handshaking() {
            connection
                .complete_io(&mut transport)
                .map_err(std::io::Error::other)?;
        }
        let tls = rustls::StreamOwned::new(connection, transport);
        let mut socket = tungstenite::accept(tls).map_err(|error| match error {
            tungstenite::HandshakeError::Failure(error) => ws_error_for_test(error),
            tungstenite::HandshakeError::Interrupted(_) => {
                std::io::Error::other("interrupted handshake")
            }
        })?;
        match socket.read().map_err(ws_error_for_test)? {
            tungstenite::Message::Binary(body) => Ok(body.to_vec()),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "expected a binary record",
            )),
        }
    })
}

#[test]
fn wss_connect_validates_against_a_private_ca_and_sends_after_tls() {
    let fixture = tls_fixture(&[std::net::IpAddr::from([127, 0, 0, 1])]);
    let private = tempfile::tempdir().unwrap();
    let ca_file = private.path().join("ca.pem");
    std::fs::write(&ca_file, &fixture.ca).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = tls_server_side(listener, &fixture.leaf, &fixture.key);

    let endpoint = ExecutionUrl::validate(&format!("wss://{address}/execution/v1")).unwrap();
    let mut client = connect(
        &endpoint,
        Instant::now() + Duration::from_secs(10),
        Some(&ca_file),
    )
    .unwrap();
    // The credential-bearing record flows only after TLS validation —
    // the server side sees it as the first message on the secure stream.
    let record = encode_record(&serde_json::json!({"credential": "secret-marker"})).unwrap();
    client
        .send(&record, Some(Instant::now() + Duration::from_secs(5)))
        .unwrap();
    let body = server.join().unwrap().unwrap();
    let received: serde_json::Value =
        rustx_execution_protocol::transport::decode_record_body(&body).unwrap();
    assert_eq!(received["credential"], "secret-marker");
}

#[test]
fn wss_wrong_ca_and_wrong_ip_san_fail_before_any_record() {
    let good = tls_fixture(&[std::net::IpAddr::from([127, 0, 0, 1])]);
    let wrong_ca = tls_fixture(&[std::net::IpAddr::from([127, 0, 0, 1])]);
    let wrong_san = tls_fixture(&[std::net::IpAddr::from([10, 0, 0, 9])]);
    let private = tempfile::tempdir().unwrap();
    let ca_file = private.path().join("ca.pem");
    std::fs::write(&ca_file, &good.ca).unwrap();

    // Certificate chains to a different CA: validation fails.
    for (name, ca, leaf, key) in [
        (
            "wrong CA",
            wrong_ca.ca.as_str(),
            good.leaf.as_str(),
            good.key.as_str(),
        ),
        (
            "wrong IP SAN",
            good.ca.as_str(),
            wrong_san.leaf.as_str(),
            wrong_san.key.as_str(),
        ),
    ] {
        let wrong_ca_file = private.path().join(format!("{name}.pem"));
        std::fs::write(&wrong_ca_file, ca).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = tls_server_side(listener, leaf, key);
        let endpoint = ExecutionUrl::validate(&format!("wss://{address}/execution/v1")).unwrap();
        let outcome = connect(
            &endpoint,
            Instant::now() + Duration::from_secs(10),
            Some(&wrong_ca_file),
        );
        assert!(outcome.is_err(), "{name}: TLS validation must fail");
        // No record — and therefore no credential — ever reached the
        // plaintext side: the server's first-read attempt fails inside
        // the TLS handshake.
        assert!(server.join().unwrap().is_err(), "{name}");
    }
}

#[test]
fn wss_without_a_matching_ca_file_fails() {
    let fixture = tls_fixture(&[std::net::IpAddr::from([127, 0, 0, 1])]);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = tls_server_side(listener, &fixture.leaf, &fixture.key);
    let endpoint = ExecutionUrl::validate(&format!("wss://{address}/execution/v1")).unwrap();
    // The WebPKI roots alone cannot trust a private fixture CA.
    assert!(connect(&endpoint, Instant::now() + Duration::from_secs(10), None).is_err());
    assert!(server.join().unwrap().is_err());
}

#[test]
fn bracketed_ipv6_urls_use_the_scheme_default_port() {
    let secure = ExecutionUrl::validate("wss://[::1]/execution/v1").unwrap();
    assert_eq!(secure.address(), "[::1]:443".parse().unwrap());
    let clear = ExecutionUrl::validate("ws://[::1]/execution/v1").unwrap();
    assert_eq!(clear.address(), "[::1]:80".parse().unwrap());
    assert!(ExecutionUrl::validate("wss://::1/execution/v1").is_err());
}

#[test]
fn trusted_loopback_tls_reaches_and_rejects_a_malformed_websocket_upgrade() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let tls = super::LoopbackTls::new(address.ip()).unwrap();
    let trust = crate::endpoint::client_config(None, Some(tls.certificate())).unwrap();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        tls.accept(stream, Instant::now() + Duration::from_secs(5))
            .err()
            .expect("HTTP Upgrade must fail after TLS")
    });
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut client = rustls::ClientConnection::new(
        std::sync::Arc::new(trust),
        rustls_pki_types::ServerName::IpAddress(address.ip().into()),
    )
    .unwrap();
    while client.is_handshaking() {
        client.complete_io(&mut stream).unwrap();
    }
    let mut stream = rustls::StreamOwned::new(client, stream);
    stream
        .write_all(b"GET /execution/v1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let failure = worker.join().unwrap();
    assert_eq!(failure.kind(), std::io::ErrorKind::InvalidData, "{failure}");
}
