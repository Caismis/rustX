//! Runtime-owned monotonic time.
//!
//! Monotonic time is for elapsed-time policies only. It is deliberately
//! separate from [`crate::runtime::types::RuntimeClock`], which supplies UTC
//! timestamps for durable facts and must never be used for deadlines.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::sync::watch;

/// A runtime-owned monotonic clock for bounded elapsed-time policies.
pub trait MonotonicClock: Send + Sync + fmt::Debug {
    /// Monotonic milliseconds since an arbitrary fixed origin.
    fn now_millis(&self) -> u64;

    /// Wakes at an absolute deadline in this clock's same monotonic domain.
    fn wait_until_millis(&self, deadline_millis: u64) -> BoxFuture<'static, ()>;
}

/// The production monotonic clock.
#[derive(Debug)]
pub struct SystemMonotonicClock {
    origin: tokio::time::Instant,
}

impl SystemMonotonicClock {
    /// Creates a clock whose origin is now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl Default for SystemMonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotonicClock for SystemMonotonicClock {
    fn now_millis(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn wait_until_millis(&self, deadline_millis: u64) -> BoxFuture<'static, ()> {
        let remaining = deadline_millis.saturating_sub(self.now_millis());
        Box::pin(async move {
            if remaining > 0 {
                tokio::time::sleep(Duration::from_millis(remaining)).await;
            }
        })
    }
}

/// A manually advanced monotonic clock for deterministic policy tests.
#[derive(Debug)]
pub struct ManualMonotonicClock {
    millis: Arc<AtomicU64>,
    wake: watch::Sender<u64>,
}

impl Default for ManualMonotonicClock {
    fn default() -> Self {
        let (wake, _receiver) = watch::channel(0);
        Self {
            millis: Arc::new(AtomicU64::new(0)),
            wake,
        }
    }
}

impl ManualMonotonicClock {
    /// Creates a clock parked at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances the clock by `millis`.
    pub fn advance(&self, millis: u64) {
        let mut current = self.millis.load(Ordering::SeqCst);
        loop {
            let next = current.saturating_add(millis);
            match self
                .millis
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => {
                    self.wake.send_replace(next);
                    break;
                }
                Err(observed) => current = observed,
            }
        }
    }
}

impl MonotonicClock for ManualMonotonicClock {
    fn now_millis(&self) -> u64 {
        self.millis.load(Ordering::SeqCst)
    }

    fn wait_until_millis(&self, deadline_millis: u64) -> BoxFuture<'static, ()> {
        let millis = Arc::clone(&self.millis);
        let mut wake = self.wake.subscribe();
        Box::pin(async move {
            loop {
                if millis.load(Ordering::SeqCst) >= deadline_millis {
                    return;
                }
                if wake.changed().await.is_err() {
                    return;
                }
            }
        })
    }
}
