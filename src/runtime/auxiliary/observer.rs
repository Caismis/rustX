//! Ephemeral attempt-separated text. Full/closed queues retire only that
//! observer. Retained results repair gaps without replaying inference.
use crate::runtime::identity::{AuxiliaryCallId, RequestId};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
pub const CHUNK_BYTES_MAX: usize = 1024;
pub const OBSERVER_RECORDS_MAX: usize = 16;
pub const CONVERSATION_OBSERVERS_MAX: usize = 16;
pub const PROCESS_OBSERVERS_MAX: usize = 128;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuxiliarySegment {
    Reset {
        call_id: AuxiliaryCallId,
        request_id: RequestId,
        attempt: u32,
    },
    Text {
        call_id: AuxiliaryCallId,
        request_id: RequestId,
        attempt: u32,
        start: u64,
        end: u64,
        text: String,
    },
}
struct Observer {
    sender: mpsc::Sender<AuxiliarySegment>,
    retired: AtomicBool,
    _capacity: OwnedSemaphorePermit,
}
pub(crate) struct AuxiliarySubscription {
    observer: Arc<Observer>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<AuxiliarySegment>>,
}
impl AuxiliarySubscription {
    pub(crate) async fn next(&self) -> AuxiliarySegment {
        let mut receiver = self.receiver.lock().await;
        if let Some(segment) = receiver.recv().await {
            return segment;
        }
        std::future::pending().await
    }
}
impl Drop for AuxiliarySubscription {
    fn drop(&mut self) {
        self.observer.retired.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub(crate) struct StreamOwner(Mutex<Vec<Weak<Observer>>>);
impl StreamOwner {
    pub(crate) fn subscribe(&self) -> Option<AuxiliarySubscription> {
        static PROCESS: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let permit = PROCESS
            .get_or_init(|| Arc::new(Semaphore::new(PROCESS_OBSERVERS_MAX)))
            .clone()
            .try_acquire_owned()
            .ok()?;
        let mut observers = self.0.lock().expect("auxiliary observers");
        observers.retain(|observer| observer.strong_count() > 0);
        if observers.len() >= CONVERSATION_OBSERVERS_MAX {
            return None;
        }
        let (sender, receiver) = mpsc::channel(OBSERVER_RECORDS_MAX);
        let observer = Arc::new(Observer {
            sender,
            retired: AtomicBool::new(false),
            _capacity: permit,
        });
        observers.push(Arc::downgrade(&observer));
        Some(AuxiliarySubscription {
            observer,
            receiver: tokio::sync::Mutex::new(receiver),
        })
    }
    pub(crate) fn publish(&self, segment: &AuxiliarySegment) {
        let mut observers = self.0.lock().expect("auxiliary observers");
        observers.retain(|observer| {
            let Some(observer) = observer.upgrade() else {
                return false;
            };
            if !observer.retired.load(Ordering::Acquire)
                && observer.sender.try_send(segment.clone()).is_err()
            {
                observer.retired.store(true, Ordering::Release);
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reset(attempt: u32) -> AuxiliarySegment {
        AuxiliarySegment::Reset {
            call_id: AuxiliaryCallId::generate(),
            request_id: RequestId::new(format!("request-{attempt}")),
            attempt,
        }
    }
    #[tokio::test]
    async fn full_queue_retires_observer_before_any_later_attempt_text() {
        let owner = StreamOwner::default();
        let subscriber = owner.subscribe().unwrap();
        for ordinal in 0..OBSERVER_RECORDS_MAX {
            owner.publish(&reset(u32::try_from(ordinal).unwrap()));
        }
        owner.publish(&reset(99));
        assert!(subscriber.observer.retired.load(Ordering::Acquire));
        for _ in 0..OBSERVER_RECORDS_MAX {
            let _ = subscriber.next().await;
        }
        owner.publish(&reset(100));
        assert!(subscriber.receiver.lock().await.try_recv().is_err());
        assert!(
            owner.subscribe().is_some(),
            "retiring an observer does not close the native stream owner"
        );
    }
    #[tokio::test]
    async fn tiny_records_and_subscribers_are_count_bounded() {
        let owner = StreamOwner::default();
        let mut subscriptions: Vec<_> = (0..CONVERSATION_OBSERVERS_MAX)
            .map(|_| owner.subscribe().unwrap())
            .collect();
        assert!(owner.subscribe().is_none());
        subscriptions.pop();
        assert!(owner.subscribe().is_some());
    }
}
