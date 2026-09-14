//! Physical connection accounting; leases are the authority, snapshots are reads.
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

#[derive(Debug, Default)]
pub struct TransportResources {
    websocket: AtomicUsize,
    stdio: AtomicUsize,
    refusals: AtomicU64,
    failures: AtomicU64,
}
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct TransportDiagnostics {
    pub websocket_connections: usize,
    pub stdio_connections: usize,
    pub connection_refusals: u64,
    pub delivery_failures: u64,
    pub max_message_bytes: usize,
    pub outbound_queue_messages: usize,
    pub outbound_queue_bytes: usize,
    pub in_flight_requests: usize,
    pub write_deadline_ms: u64,
}
#[derive(Debug)]
pub struct ConnectionLease {
    owner: Arc<TransportResources>,
    websocket: bool,
}
impl Drop for ConnectionLease {
    fn drop(&mut self) {
        let count = if self.websocket {
            &self.owner.websocket
        } else {
            &self.owner.stdio
        };
        count.fetch_sub(1, Ordering::Relaxed);
    }
}
impl TransportResources {
    pub(crate) fn reserve(
        self: &Arc<Self>,
        websocket: bool,
        limit: usize,
    ) -> Option<ConnectionLease> {
        let count = if websocket {
            &self.websocket
        } else {
            &self.stdio
        };
        if count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < limit).then_some(n + 1)
            })
            .is_err()
        {
            self.refuse();
            return None;
        }
        Some(ConnectionLease {
            owner: self.clone(),
            websocket,
        })
    }
    pub(crate) fn has_connections(&self) -> bool {
        self.websocket.load(Ordering::Relaxed) != 0 || self.stdio.load(Ordering::Relaxed) != 0
    }
    pub(crate) fn refuse(&self) {
        self.refusals.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn delivery_failed(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }
    #[must_use]
    pub fn snapshot(&self) -> TransportDiagnostics {
        TransportDiagnostics {
            websocket_connections: self.websocket.load(Ordering::Relaxed),
            stdio_connections: self.stdio.load(Ordering::Relaxed),
            connection_refusals: self.refusals.load(Ordering::Relaxed),
            delivery_failures: self.failures.load(Ordering::Relaxed),
            max_message_bytes: super::MAX_MESSAGE_BYTES,
            outbound_queue_messages: super::OUTBOUND_MESSAGES,
            outbound_queue_bytes: super::OUTBOUND_QUEUE_BYTES,
            in_flight_requests: super::IN_FLIGHT_REQUESTS,
            write_deadline_ms: 10_000,
        }
    }
}
