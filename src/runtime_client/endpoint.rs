//! The typed request endpoint of one native Runtime Client host.
//!
//! This is not a client protocol boundary: clients use
//! [`crate::app_server::connection::AppServerConnection`], which multiplexes
//! Sessions and directly reuses native attachment/projection operations.
//! In production the endpoint is served read-only over the child-owned live
//! inspection socket; in-process callers drive a host's typed requests
//! through it.
//!
//! [`RuntimeClientEndpoint`] accepts *every* Runtime Client request —
//! including `initialize` — and returns the correlated response, so protocol
//! semantics live here and nowhere else.
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
//! - attachment admission — a normal endpoint admits the one control
//!   attachment, while an inspection endpoint admits a read-only attachment;
//!   a second control `initialize` returns
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
//! No transport, framing, or I/O lives here: this module is semantic
//! dispatch over the host. Ordinary requests are synchronous; `shutdown` and
//! the other awaiting requests use the async entry point because their
//! responses may mean runtime quiescence rather than cancellation-request
//! acceptance.

use std::sync::{Arc, Mutex, Weak};

use super::attachment::RuntimeAttachment;
use super::host::{EventDelivery, EventSubscription, RuntimeClientHost};
use super::types::{RequestId, RuntimeClientError, RuntimeClientRequest, RuntimeClientResponse};

/// The transport-neutral semantic endpoint of one Runtime Client
/// connection.
///
/// Ordinary requests are handled synchronously and serialized against each
/// other. `shutdown` and the other awaiting requests are handled through
/// [`Self::handle_request_async`] so their responses can await semantic
/// quiescence without holding the attachment lock.
pub struct RuntimeClientEndpoint {
    /// The runtime this endpoint speaks for.
    host: Weak<super::host::ClientInner>,
    /// Whether this endpoint is a read-only observation attachment. The
    /// host still owns the projection; this flag only selects attachment
    /// admission and fences control requests at the attachment boundary.
    read_only: bool,
    /// The admitted attachment, once `initialize` succeeded.
    attachment: Arc<Mutex<EndpointAttachment>>,
    /// Ordinary dispatch stays serialized while retirement can reach the
    /// attachment without waiting on an admitted physical Source read.
    dispatch: Mutex<()>,
}

#[derive(Default)]
struct EndpointAttachment {
    retired: bool,
    attachment: Option<Arc<RuntimeAttachment>>,
}

/// A listener's weak retirement capability for one exact connection endpoint.
/// It retains neither the endpoint nor the runtime's physical storage owner.
pub(crate) struct EndpointRetirement(Weak<Mutex<EndpointAttachment>>);
impl EndpointRetirement {
    pub(crate) fn is_alive(&self) -> bool {
        self.0.strong_count() != 0
    }

    pub(crate) fn retire(&self) {
        let Some(state) = self.0.upgrade() else {
            return;
        };
        let attachment = {
            let mut slot = state.lock().expect("runtime client endpoint lock poisoned");
            slot.retired = true;
            slot.attachment.take()
        };
        // Detach acquires the Host lock and wakes its exact subscriber. Never
        // hold the endpoint slot across this native retirement boundary.
        if let Some(attachment) = attachment {
            attachment.detach();
        }
    }
}

impl core::fmt::Debug for RuntimeClientEndpoint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RuntimeClientEndpoint")
            .field("resident", &self.host.strong_count())
            .finish_non_exhaustive()
    }
}

impl RuntimeClientEndpoint {
    /// Creates an unattached endpoint over one runtime.
    #[must_use]
    pub fn new(host: &RuntimeClientHost) -> Self {
        Self::new_with_mode(host, false)
    }

    /// Creates an endpoint whose attachment can only observe the host's
    /// projection. It may coexist with the host's control attachment and with
    /// other read-only endpoints.
    #[must_use]
    pub(crate) fn new_read_only(host: &RuntimeClientHost) -> Self {
        Self::new_with_mode(host, true)
    }

    fn new_with_mode(host: &RuntimeClientHost, read_only: bool) -> Self {
        Self {
            host: Arc::downgrade(&host.inner),
            read_only,
            attachment: Arc::new(Mutex::new(EndpointAttachment::default())),
            dispatch: Mutex::new(()),
        }
    }

    pub(crate) fn retirement(&self) -> EndpointRetirement {
        EndpointRetirement(Arc::downgrade(&self.attachment))
    }

    /// Handles one Runtime Client protocol request and returns its
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
        let _dispatch = self
            .dispatch
            .lock()
            .expect("runtime client dispatch lock poisoned");
        let mut slot = self
            .attachment
            .lock()
            .expect("runtime client endpoint lock poisoned");
        if slot.retired {
            return error(id, RuntimeClientError::NotAttached);
        }
        match request {
            RuntimeClientRequest::Initialize {
                protocol_version, ..
            } => {
                if slot.attachment.is_some() {
                    return error(
                        id,
                        RuntimeClientError::InvalidRequest {
                            message: "the attachment is already initialized".to_owned(),
                        },
                    );
                }
                // Version negotiation, admission, identity allocation, and
                // the bounded identity receipt are all the host's, not
                // the transport's.
                let Some(host) = self.host.upgrade() else {
                    return error(id, RuntimeClientError::NotAttached);
                };
                let attached = if self.read_only {
                    host.attach_read_only(protocol_version)
                } else {
                    host.attach(protocol_version)
                };
                match attached {
                    Ok((attachment, result)) => {
                        slot.attachment = Some(Arc::new(attachment));
                        ok(id, result)
                    }
                    Err(error_value) => error(id, error_value),
                }
            }
            other => {
                let Some(attachment) = slot.attachment.clone() else {
                    return error(id, RuntimeClientError::NotAttached);
                };
                drop(slot);
                let detaching = matches!(other, RuntimeClientRequest::Detach { .. });
                let response = attachment.handle_request(other);
                if detaching && response.error.is_none() {
                    // The attachment released itself; returning the slot to
                    // the unattached state lets the same connection
                    // re-initialize into a fresh attachment identity.
                    let released = self
                        .attachment
                        .lock()
                        .expect("runtime client endpoint lock poisoned")
                        .attachment
                        .take();
                    drop(released);
                }
                response
            }
        }
    }

    /// Handles one request through the async semantic path. This is the
    /// complete transport entry point when a request can await runtime-owned
    /// settlement, including `shutdown`.
    ///
    /// # Panics
    ///
    /// Panics only if the endpoint attachment lock is poisoned, which would
    /// mean a previous operation panicked while holding the lock.
    pub async fn handle_request_async(
        &self,
        request: RuntimeClientRequest,
    ) -> RuntimeClientResponse {
        // Checkpoint recovery may await the existing Source worker. Never
        // block the socket executor (including its IO/timer driver) on it.
        let checkpoint_read = matches!(request, RuntimeClientRequest::CheckpointGet { .. });
        if !request.requires_async() && !checkpoint_read {
            return self.handle_request(request);
        }
        let id = request.id();
        let attachment = self
            .attachment
            .lock()
            .expect("runtime client endpoint lock poisoned")
            .attachment
            .clone();
        let Some(attachment) = attachment else {
            return error(id, RuntimeClientError::NotAttached);
        };
        if checkpoint_read {
            // The admitted socket task awaits the physical blocking call;
            // inspection shutdown retains its JoinSet slot through this join.
            return tokio::task::spawn_blocking(move || attachment.handle_request(request))
                .await
                .unwrap_or_else(|_| {
                    error(
                        id,
                        RuntimeClientError::ObservationUnavailable {
                            message: "native checkpoint worker did not complete".into(),
                        },
                    )
                });
        }
        attachment.handle_request_async(request).await
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
            .attachment
            .as_ref()
            .and_then(|attachment| attachment.subscription())
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
            .attachment
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

#[cfg(test)]
mod retirement_tests;
