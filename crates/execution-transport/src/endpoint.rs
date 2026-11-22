//! Strict execution-endpoint validation and rustls client configuration.
//!
//! Validation is pure policy: it runs before any socket is opened and
//! before any credential is transmitted, and it returns plain `String`
//! diagnostics (never record bytes, never URL contents beyond the caller
//! already holds).

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;

use rustls_pki_types::ServerName;
use rustls_pki_types::pem::PemObject;

use crate::EXECUTION_PATH;

/// A validated `BootstrapEndpoint::WebSocket` URL.
///
/// Supported forms are deliberately narrow — `ws://IP[:PORT]` only for a
/// literal loopback IP, `wss://IP[:PORT]` for any literal IP, path always
/// `/execution/v2`, no userinfo, query, or fragment. Hostnames are
/// unsupported: a numeric IP keeps resolution inside the bounded connect
/// budget and pairs with an IP-SAN certificate.
#[derive(Debug)]
pub struct ExecutionUrl {
    /// The exact provisioned URL, sent verbatim as the upgrade request.
    pub(crate) request_target: String,
    /// The numeric address the socket connects to.
    pub(crate) address: SocketAddr,
    /// `wss` — TLS terminates inside the client before any record flows.
    pub(crate) secure: bool,
    /// The TLS server name: the literal IP, matched against an IP SAN.
    pub(crate) server_name: ServerName<'static>,
}

impl ExecutionUrl {
    /// Whether this endpoint requires TLS before credentials may flow.
    #[must_use]
    pub fn is_secure(&self) -> bool {
        self.secure
    }

    /// The numeric socket address of the listener.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

fn invalid(reason: &str) -> String {
    format!("invalid execution endpoint: {reason}")
}

impl ExecutionUrl {
    /// Validates a bootstrap-supplied WebSocket execution URL against the
    /// endpoint policy above. Runs before any connect or credential
    /// transmission; rejects insecure or ambiguous endpoints outright.
    ///
    /// # Errors
    ///
    /// Returns a violation description whenever the URL is not a strict
    /// `ws`/`wss` execution endpoint — before any socket is opened.
    pub fn validate(url: &str) -> Result<ExecutionUrl, String> {
        if url.is_empty() || url.len() > 512 || !url.is_ascii() {
            return Err(invalid("the endpoint must be short ASCII"));
        }
        let (secure, rest) = if let Some(rest) = url.strip_prefix("ws://") {
            (false, rest)
        } else if let Some(rest) = url.strip_prefix("wss://") {
            (true, rest)
        } else {
            return Err(invalid("the endpoint must be a ws:// or wss:// URL"));
        };
        let Some((authority, path)) = rest.split_once('/') else {
            return Err(invalid("the path must be /execution/v2"));
        };
        // Exact match also rejects `?query` and `#fragment` — both would
        // leave bytes after the path.
        if path != EXECUTION_PATH.strip_prefix('/').unwrap_or_default() {
            return Err(invalid("the path must be /execution/v2"));
        }
        if authority.is_empty() {
            return Err(invalid("a numeric host is required"));
        }
        if authority.contains('@') {
            return Err(invalid("userinfo is never permitted"));
        }
        // Literal numeric IP only — a hostname would need blocking DNS
        // inside the bounded connect budget and ambiguously resolve
        // across families.
        let address = authority
            .parse::<SocketAddr>()
            .or_else(|_| {
                authority
                    .strip_prefix('[')
                    .and_then(|host| host.strip_suffix(']'))
                    .map_or_else(
                        || authority.parse::<std::net::Ipv4Addr>().map(IpAddr::V4),
                        |host| host.parse::<std::net::Ipv6Addr>().map(IpAddr::V6),
                    )
                    .map(|ip| SocketAddr::new(ip, if secure { 443 } else { 80 }))
            })
            .map_err(|_| invalid("the host must be a numeric IP address"))?;
        if !secure && !address.ip().is_loopback() {
            return Err(invalid(
                "cleartext ws:// is permitted only for a literal loopback IP",
            ));
        }
        let server_name = ServerName::try_from(address.ip().to_string())
            .map_err(|_| invalid("the IP address cannot be a TLS server name"))?;
        Ok(ExecutionUrl {
            request_target: url.to_owned(),
            address,
            secure,
            server_name,
        })
    }
}

/// Builds the client `ClientConfig` for `wss`: the standard `WebPKI` roots
/// plus any operator-provisioned private anchors from `ca_file` (PEM).
/// There is deliberately no way to disable certificate or hostname
/// verification — a private deployment adds trust, never removes it.
pub(crate) fn client_config(
    ca_file: Option<&Path>,
    provisioned_cert: Option<&[u8]>,
) -> io::Result<rustls::ClientConfig> {
    const MAX_CA_BUNDLE_BYTES: u64 = 8 * 1024 * 1024;
    let mut roots = rustls::RootCertStore::empty();
    if let Some(cert) = provisioned_cert {
        roots
            .add(rustls_pki_types::CertificateDer::from(cert))
            .map_err(io::Error::other)?;
    } else {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    if let Some(path) = ca_file {
        // The operator bundle must be a finite regular file before we
        // open it: a FIFO would block the read and a symlink is refused
        // outright — the caller loaded this policy locally, never from a
        // remotely supplied path.
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| io::Error::other(format!("execution CA bundle: {error}")))?;
        if !metadata.file_type().is_file() {
            return Err(io::Error::other(
                "execution CA bundle must be a regular file",
            ));
        }
        if metadata.len() > MAX_CA_BUNDLE_BYTES {
            return Err(io::Error::other("execution CA bundle is oversized"));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(path)
            .map_err(|error| io::Error::other(format!("execution CA bundle: {error}")))?;
        if !file
            .metadata()
            .map_err(|error| io::Error::other(format!("execution CA bundle: {error}")))?
            .file_type()
            .is_file()
        {
            return Err(io::Error::other(
                "execution CA bundle must be a regular file",
            ));
        }
        let mut added = false;
        for certificate in rustls_pki_types::CertificateDer::pem_reader_iter(file) {
            roots
                .add(
                    certificate.map_err(|error| {
                        io::Error::other(format!("execution CA bundle: {error}"))
                    })?,
                )
                .map_err(|error| {
                    io::Error::other(format!("execution CA bundle rejected: {error}"))
                })?;
            added = true;
        }
        if !added {
            return Err(io::Error::other(
                "execution CA bundle contains no certificates",
            ));
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_root_certificates(roots)
        .with_no_client_auth())
}
