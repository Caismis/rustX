//! The Runtime Client semantic endpoint: the transport-neutral protocol
//! entry point of Runtime Client Protocol v1.
//!
//! [`RuntimeClientEndpoint`] is the boundary every transport wraps. It
//! accepts *every* v1 request — including `initialize` — and returns the
//! correlated response, so protocol semantics live here and nowhere else.
//!
//! # What a transport is
//!
//! A transport is a framing adapter. Issue #38 (stdio/JSONL) reduces to:
//!
//! ```text
//! read one JSONL line
//!   -> serde_json::from_str::<RuntimeClientRequest>
//!   -> RuntimeClientEndpoint::handle_request
//!   -> serde_json::to_string::<RuntimeClientResponse>
//!   -> write one JSONL line
//!
//! and, concurrently:
//!
//! RuntimeClientEndpoint::next_event
//!   -> serde_json::to_string::<RuntimeClientProtocolEvent>
//!   -> write one JSONL line
//! ```
//!
//! Nothing in that pipeline is semantic. In particular a transport does
//! **not** implement, and cannot observe the need to implement:
//!
//! - protocol version negotiation — `initialize` performs it here and
//!   returns [`RuntimeClientError::UnsupportedProtocolVersion`];
//! - single-active-attachment admission — the host admits at most one, and
//!   a second `initialize` returns
//!   [`RuntimeClientError::AttachmentInUse`] without evicting the first;
//! - [`AttachmentId`](super::types::AttachmentId) creation — the runtime
//!   allocates the identity and returns it in the `initialized` result;
//!   the endpoint never invents one;
//! - attachment replacement/rejection semantics — rejection is the
//!   deterministic outcome, and release happens only through `detach` or
//!   the RAII drop of the endpoint;
//! - any out-of-band attach operation — `initialize` is sufficient to
//!   establish the active attachment.
//!
//! # Lifecycle
//!
//! One endpoint models one client connection. It starts unattached; the
//! first successful `initialize` admits the attachment and stores it. Every
//! non-`initialize` request before that is
//! [`RuntimeClientError::NotAttached`]. A successful `detach` releases the
//! attachment and returns the endpoint to the unattached state, so a
//! reconnecting client on the same connection re-initializes and receives a
//! fresh attachment identity. Dropping the endpoint detaches.
//!
//! No transport, framing, or I/O lives here: this module is synchronous
//! semantics over the host.

use std::sync::Mutex;

use super::attachment::RuntimeAttachment;
use super::host::{EventDelivery, EventSubscription, RuntimeClientHost};
use super::types::{RequestId, RuntimeClientError, RuntimeClientRequest, RuntimeClientResponse};

/// The transport-neutral semantic endpoint of one Runtime Client
/// connection.
///
/// Requests are handled synchronously and are serialized against each
/// other, so `initialize`, `detach`, and every other method observe a
/// consistent attachment state without the caller coordinating anything.
pub struct RuntimeClientEndpoint {
    /// The runtime this endpoint speaks for.
    host: RuntimeClientHost,
    /// The admitted attachment, once `initialize` succeeded.
    attachment: Mutex<Option<RuntimeAttachment>>,
}

impl core::fmt::Debug for RuntimeClientEndpoint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RuntimeClientEndpoint")
            .field("conversation_id", self.host.conversation_id())
            .finish_non_exhaustive()
    }
}

impl RuntimeClientEndpoint {
    /// Creates an unattached endpoint over one runtime.
    #[must_use]
    pub fn new(host: RuntimeClientHost) -> Self {
        Self {
            host,
            attachment: Mutex::new(None),
        }
    }

    /// Handles one Runtime Client Protocol v1 request and returns its
    /// correlated response.
    ///
    /// The request id is echoed exactly, so responses correlate under
    /// pipelining. This is the complete semantic surface: a transport needs
    /// no other entry point.
    ///
    /// # Panics
    ///
    /// Panics only if the endpoint attachment lock is poisoned, which would
    /// mean a previous operation panicked while holding the lock.
    pub fn handle_request(&self, request: RuntimeClientRequest) -> RuntimeClientResponse {
        let id = request.id();
        let mut slot = self
            .attachment
            .lock()
            .expect("runtime client endpoint lock poisoned");
        match request {
            RuntimeClientRequest::Initialize {
                protocol_version, ..
            } => {
                if slot.is_some() {
                    return error(
                        id,
                        RuntimeClientError::InvalidRequest {
                            message: "the attachment is already initialized".to_owned(),
                        },
                    );
                }
                // Version negotiation, admission, identity allocation, and
                // the linearized initial snapshot are all the host's, not
                // the transport's.
                match self.host.attach(protocol_version) {
                    Ok((attachment, result)) => {
                        *slot = Some(attachment);
                        ok(id, result)
                    }
                    Err(error_value) => error(id, error_value),
                }
            }
            other => {
                let Some(attachment) = slot.as_ref() else {
                    return error(id, RuntimeClientError::NotAttached);
                };
                let detaching = matches!(other, RuntimeClientRequest::Detach { .. });
                let response = attachment.handle_request(other);
                if detaching && response.error.is_none() {
                    // The attachment released itself; returning the slot to
                    // the unattached state lets the same connection
                    // re-initialize into a fresh attachment identity.
                    let released = slot.take();
                    drop(slot);
                    drop(released);
                }
                response
            }
        }
    }

    /// The delivery handle of the active subscription, when the client
    /// subscribed.
    ///
    /// # Panics
    ///
    /// Panics only if the endpoint attachment lock is poisoned.
    #[must_use]
    pub fn subscription(&self) -> Option<EventSubscription> {
        self.attachment
            .lock()
            .expect("runtime client endpoint lock poisoned")
            .as_ref()
            .and_then(RuntimeAttachment::subscription)
    }

    /// Waits for the next notification to frame.
    ///
    /// Returns [`EventDelivery::Closed`] when the endpoint is unattached or
    /// has no active subscription. No lock is held across the await.
    pub async fn next_event(&self) -> EventDelivery {
        let Some(subscription) = self.subscription() else {
            return EventDelivery::Closed;
        };
        subscription.next().await
    }

    /// Polls for the next notification to frame without waiting.
    #[must_use]
    pub fn try_next_event(&self) -> EventDelivery {
        match self.subscription() {
            Some(subscription) => subscription.try_next(),
            None => EventDelivery::Closed,
        }
    }

    /// The attachment identity of this endpoint, when initialized.
    ///
    /// # Panics
    ///
    /// Panics only if the endpoint attachment lock is poisoned.
    #[must_use]
    pub fn attachment_id(&self) -> Option<super::types::AttachmentId> {
        self.attachment
            .lock()
            .expect("runtime client endpoint lock poisoned")
            .as_ref()
            .map(|attachment| attachment.attachment_id().clone())
    }
}

/// Builds the correlated response of a successful request.
fn ok(id: RequestId, result: super::types::RuntimeClientResult) -> RuntimeClientResponse {
    RuntimeClientResponse {
        id,
        result: Some(result),
        error: None,
    }
}

/// Builds the correlated response of a failed request.
fn error(id: RequestId, error: RuntimeClientError) -> RuntimeClientResponse {
    RuntimeClientResponse {
        id,
        result: None,
        error: Some(error),
    }
}
