//! Connection-local registration acceptance. This is delivery coordination,
//! never a second cursor or history owner.
//!
//! Acceptance lock order: WebSocket Socket (outer) -> carrier retirement fence
//! -> route table -> registration slot -> native attachment/Projection.
//! Readiness and Flush complete outside the carrier/registration boundary.
//! Replacement acquires neither Socket nor the carrier fence. Teardown releases
//! the carrier fence before closing routes. No await occurs in a slot.
use super::protocol::RequestId;
use crate::runtime_client::{host::EventSubscription, types::RuntimeClientCursor};
use std::{
    io,
    sync::{Arc, Mutex},
    task::Poll,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    PendingReceipt,
    Following,
    RecoveryRequired,
    Retired,
}
struct Current {
    id: String,
    state: State,
    native: EventSubscription,
    accepted: RuntimeClientCursor,
}
#[derive(Default)]
pub(crate) struct Slot(
    Mutex<Option<Current>>,
    tokio::sync::Notify,
    futures_util::task::AtomicWaker,
    #[cfg(test)] Mutex<Option<tokio::sync::watch::Sender<bool>>>,
);
#[derive(Clone)]
pub(crate) struct Registration {
    slot: Arc<Slot>,
    pub(crate) id: String,
}
pub(crate) enum Kind {
    Receipt { superseded: String },
    Event { cursor: RuntimeClientCursor },
    Resync,
}
pub(crate) struct Publication {
    registration: Registration,
    kind: Kind,
}
/// Pending proves the callback took no bytes; Discarded advances the queue.
pub(crate) enum Acceptance<T> {
    Accepted(String, T),
    Pending,
    Discarded,
    Failed(io::Error),
}
impl Slot {
    #[cfg(test)]
    pub(crate) fn accepted_cursor(&self) -> Option<RuntimeClientCursor> {
        self.0
            .lock()
            .expect("registration")
            .as_ref()
            .map(|current| current.accepted)
    }
    #[cfg(test)]
    pub(crate) fn install_wait_probe(&self) -> tokio::sync::watch::Receiver<bool> {
        let (send, receive) = tokio::sync::watch::channel(false);
        *self.3.lock().expect("registration probe") = Some(send);
        receive
    }
    pub(crate) fn replace(
        self: &Arc<Self>,
        install: impl FnOnce() -> Result<EventSubscription, super::protocol::RpcError>,
        after: RuntimeClientCursor,
    ) -> Result<Registration, super::protocol::RpcError> {
        #[cfg(test)]
        if self.0.try_lock().is_err()
            && let Some(probe) = self.3.lock().expect("registration test probe").as_ref()
        {
            probe.send_replace(true);
        }
        let mut current = self.0.lock().expect("registration");
        // Native replacement and App Server identity installation share this
        // boundary with final physical acceptance; native never calls back.
        let native = install()?;
        if let Some(old) = current.as_mut() {
            old.state = State::Retired;
        }
        let id = uuid::Uuid::new_v4().to_string();
        *current = Some(Current {
            id: id.clone(),
            state: State::PendingReceipt,
            native,
            accepted: after,
        });
        self.1.notify_waiters();
        self.2.wake();
        Ok(Registration {
            slot: self.clone(),
            id,
        })
    }
    pub(crate) fn following(self: &Arc<Self>) -> Option<(Registration, EventSubscription)> {
        let current = self.0.lock().expect("registration");
        let current = current
            .as_ref()
            .filter(|value| value.state == State::Following)?;
        Some((
            Registration {
                slot: self.clone(),
                id: current.id.clone(),
            },
            current.native.clone(),
        ))
    }
    pub(crate) async fn wait_following(self: &Arc<Self>) -> (Registration, EventSubscription) {
        loop {
            let wake = self.1.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            if let Some(value) = self.following() {
                return value;
            }
            wake.await;
        }
    }
    pub(crate) fn receipt_for(
        self: &Arc<Self>,
        registration_id: &str,
        id: &RequestId,
    ) -> io::Result<Publication> {
        Registration {
            slot: self.clone(),
            id: registration_id.to_owned(),
        }
        .receipt(id)
    }
    pub(crate) fn retire(&self) {
        if let Some(current) = self.0.lock().expect("registration").as_mut() {
            current.state = State::Retired;
        }
        self.1.notify_waiters();
        self.2.wake();
    }
}
impl Registration {
    pub(crate) fn receipt(self, id: &RequestId) -> io::Result<Publication> {
        let superseded = super::transport::serialize_record(&super::rpc_error::failure(
            Some(id.clone()),
            super::rpc_error::domain(super::protocol::ErrorData::SubscriptionSuperseded),
        ))?;
        Ok(Publication {
            registration: self,
            kind: Kind::Receipt { superseded },
        })
    }
    pub(crate) fn event(self, cursor: RuntimeClientCursor) -> Publication {
        Publication {
            registration: self,
            kind: Kind::Event { cursor },
        }
    }
    pub(crate) fn recovery(self) -> (Publication, RuntimeClientCursor) {
        let mut slot = self.slot.0.lock().expect("registration");
        let after = if let Some(current) = slot.as_mut().filter(|current| current.id == self.id) {
            if current.state == State::Following {
                current.state = State::RecoveryRequired;
            }
            current.accepted
        } else {
            RuntimeClientCursor::new(0)
        };
        drop(slot);
        self.slot.2.wake();
        (
            Publication {
                registration: self,
                kind: Kind::Resync,
            },
            after,
        )
    }
}
impl Publication {
    pub(crate) fn oversized(self) -> (Self, RuntimeClientCursor) {
        // The accepted frontier is captured at the same boundary that fences
        // all still-unaccepted ordinary events for this exact registration.
        self.registration.recovery()
    }

    pub(crate) fn discarded(&self, cx: &std::task::Context<'_>) -> bool {
        self.registration.slot.2.register(cx.waker());
        let slot = self.registration.slot.0.lock().expect("registration");
        let current = slot
            .as_ref()
            .filter(|current| current.id == self.registration.id);
        match (&self.kind, current) {
            (Kind::Receipt { .. }, _) => false,
            (Kind::Event { .. }, Some(current)) => current.state != State::Following,
            (Kind::Resync, Some(current)) => current.state != State::RecoveryRequired,
            _ => true,
        }
    }

    pub(crate) fn hand_off<T>(
        &self,
        record: &str,
        accept: impl FnOnce(&str) -> Poll<io::Result<T>>,
    ) -> Acceptance<T> {
        let mut slot = self.registration.slot.0.lock().expect("registration");
        let current = slot
            .as_mut()
            .filter(|current| current.id == self.registration.id);
        let selected = match (&self.kind, current.as_ref()) {
            (Kind::Receipt { .. }, Some(current)) if current.state == State::PendingReceipt => {
                record
            }
            (Kind::Receipt { superseded }, _) => superseded,
            (Kind::Event { .. }, Some(current)) if current.state == State::Following => record,
            (Kind::Resync, Some(current)) if current.state == State::RecoveryRequired => record,
            _ => return Acceptance::Discarded,
        };
        match accept(selected) {
            Poll::Pending => Acceptance::Pending,
            Poll::Ready(Err(error)) => Acceptance::Failed(error),
            Poll::Ready(Ok(value)) => {
                if let Some(current) = current {
                    match self.kind {
                        Kind::Receipt { .. } if current.state == State::PendingReceipt => {
                            current.state = State::Following;
                        }
                        Kind::Event { cursor } => current.accepted = cursor,
                        _ => {}
                    }
                }
                self.registration.slot.1.notify_waiters();
                Acceptance::Accepted(selected.to_owned(), value)
            }
        }
    }
}
