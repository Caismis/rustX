//! The default credential-free HTTP pool is shared across provider adapters.
//! Endpoints, credentials and request policy remain on each adapter/request.
use std::sync::LazyLock;

pub(super) fn default_client() -> reqwest::Client {
    // Client clones share the pool and TLS configuration. Keep construction
    // lazy: metadata-only commands must not initialize provider transport.
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);
    CLIENT.clone()
}
