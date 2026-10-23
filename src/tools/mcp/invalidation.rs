//! The one `tools/list_changed` invalidation boundary.
//!
//! [`McpInvalidationState`] is the epoch state shared with the capability
//! coordinator. [`ToolListChangedSink`] is the single admission,
//! invalidation, and notification implementation that both
//! revision-specific protocol entry points feed:
//!
//! ```text
//! MCP 2026-07-28+  subscriptions/listen stream  -> subscription task --+
//!                                                                      +-> ToolListChangedSink
//! pre-2026         notifications/tools/list_changed -> client handler --+     admit (call gate + closed)
//!                                                                            invalidate (epoch + notify)
//! ```
//!
//! At most one entry point is installed per connection, and neither may
//! re-implement admission or invalidation.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rmcp::model::{ServerNotification, SubscriptionFilter};

use crate::runtime::identity::McpServerId;

use super::catalog_cache::{CatalogCacheKey, McpCatalogCache};
use super::runtime::McpServerRuntime;
use super::{McpError, bound_error};

/// The one `tools/list_changed` admission, invalidation, and notification
/// implementation of a connection generation.
///
/// Revisions from MCP 2026-07-28 deliver the notification on the
/// `subscriptions/listen` stream drained by the generation's owned
/// subscription task; earlier revisions have no `subscriptions/listen`, and
/// rmcp routes the bare `notifications/tools/list_changed` to the client
/// handler callback. Both entry points hand the notification to this sink,
/// so both observe the same call gate, the same close fence, the same
/// epoch, and the same waiter notification.
#[derive(Clone)]
pub(super) struct ToolListChangedSink {
    server_id: McpServerId,
    invalidation: Arc<McpInvalidationState>,
    change_notify: Arc<tokio::sync::Notify>,
    closed: Arc<AtomicBool>,
    call_gate: Arc<tokio::sync::RwLock<()>>,
}

/// One notification admitted against runtime close.
///
/// It holds the read side of the runtime's call gate, so the close write
/// barrier cannot be crossed while the notification is still being applied.
pub(super) struct AdmittedNotification<'a> {
    sink: &'a ToolListChangedSink,
    _call_gate: tokio::sync::RwLockReadGuard<'a, ()>,
}

impl ToolListChangedSink {
    /// Admits one inbound notification.
    ///
    /// Returns `None` once the runtime is closed: a callback that begins
    /// after close's write barrier observes `closed` and must not mutate
    /// invalidation state.
    pub(super) async fn admit(&self) -> Option<AdmittedNotification<'_>> {
        let call_gate = self.call_gate.read().await;
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        Some(AdmittedNotification {
            sink: self,
            _call_gate: call_gate,
        })
    }
}

impl AdmittedNotification<'_> {
    /// Applies one received `tools/list_changed`: the server's epoch
    /// advances exactly once through the shared invalidation guard, which
    /// serializes against preparation epoch snapshots and the commit's epoch
    /// validation and swap, and then every change waiter is woken.
    pub(super) fn invalidate(self) {
        let mut guard = self.sink.invalidation.lock();
        guard.advance(&self.sink.server_id);
        drop(guard);
        self.sink.change_notify.notify_waiters();
    }
}

/// The one shared `tools/list_changed` invalidation synchronization boundary
/// of a capability coordinator.
///
/// MCP invalidation epoch mutation (a received `tools/list_changed`
/// notification) and capability snapshot activation (preparation epoch
/// snapshot + commit epoch validation) share exactly one mutex, so they have
/// one synchronization order:
///
/// - if the notification wins first, the prepared candidate cannot commit
///   (its epoch is stale);
/// - if the commit wins first, the notification belongs to a future refresh
///   and can never retroactively invalidate the already-committed snapshot.
///
/// Lock ordering is explicit and documented: the capability commit lock
/// (the coordinator's `state` mutex) is always acquired **before** this
/// invalidation guard, and the notification path acquires **only** this
/// guard. No path ever takes the capability state lock while holding this
/// guard, so no cycle exists.
///
/// The type is `pub` only because the public `McpServerRuntime::connect`
/// entry point accepts it; it is an internal runtime coordination boundary,
/// not a stable application interface.
#[doc(hidden)]
pub struct McpInvalidationState {
    inner: std::sync::Mutex<InvalidationEntries>,
}

#[derive(Default)]
struct InvalidationEntries {
    epochs: BTreeMap<McpServerId, u64>,
    cache_keys: BTreeMap<McpServerId, CatalogCacheKey>,
}

impl McpInvalidationState {
    /// Creates an empty invalidation state.
    #[must_use]
    #[doc(hidden)]
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(InvalidationEntries::default()),
        }
    }

    /// Acquires the invalidation guard: the single shared boundary of epoch
    /// mutation and epoch validation.
    ///
    /// # Panics
    ///
    /// Panics only if the invalidation lock is poisoned.
    #[must_use]
    #[doc(hidden)]
    pub fn lock(&self) -> McpInvalidationGuard<'_> {
        McpInvalidationGuard {
            guard: self.inner.lock().expect("MCP invalidation lock poisoned"),
        }
    }

    /// The current invalidation epoch of one server.
    ///
    /// # Panics
    ///
    /// Panics only if the invalidation lock is poisoned.
    #[must_use]
    #[doc(hidden)]
    pub fn epoch(&self, server_id: &McpServerId) -> u64 {
        self.lock().epoch(server_id)
    }

    /// Register the effective source identity before lookup or connection.
    /// A configuration/credential change or an unknown new identity retires
    /// the previous hint under the same boundary as notification invalidation.
    pub(crate) fn attach_cache_key(&self, server_id: &McpServerId, key: Option<CatalogCacheKey>) {
        let mut guard = self.lock();
        if guard.guard.cache_keys.get(server_id) == key.as_ref() {
            return;
        }
        if let Some(previous) = guard.guard.cache_keys.remove(server_id) {
            McpCatalogCache::process().invalidate(&previous);
        }
        if let Some(key) = key {
            guard.guard.cache_keys.insert(server_id.clone(), key);
        }
    }
}

impl Default for McpInvalidationState {
    fn default() -> Self {
        Self::new()
    }
}

/// The held invalidation guard: epoch reads and mutations under it are
/// ordered against the capability commit's epoch validation and swap.
#[doc(hidden)]
pub struct McpInvalidationGuard<'a> {
    guard: std::sync::MutexGuard<'a, InvalidationEntries>,
}

impl McpInvalidationGuard<'_> {
    pub(crate) fn matches_cache_key(&self, server_id: &McpServerId, key: &CatalogCacheKey) -> bool {
        self.guard.cache_keys.get(server_id) == Some(key)
    }

    /// The current epoch of one server.
    #[must_use]
    #[doc(hidden)]
    pub fn epoch(&self, server_id: &McpServerId) -> u64 {
        self.guard.epochs.get(server_id).copied().unwrap_or(0)
    }

    /// Advances one server's invalidation epoch — the exact mutation the
    /// `tools/list_changed` notification performs.
    #[doc(hidden)]
    pub fn advance(&mut self, server_id: &McpServerId) {
        let current = self.epoch(server_id);
        self.guard.epochs.insert(server_id.clone(), current + 1);
        if let Some(key) = self.guard.cache_keys.get(server_id) {
            McpCatalogCache::process().invalidate(key);
        }
    }
}

impl McpServerRuntime {
    /// The invalidation sink of this generation, shared by whichever
    /// revision-appropriate entry point the connection installs.
    pub(super) fn tool_list_changed_sink(&self) -> ToolListChangedSink {
        ToolListChangedSink {
            server_id: self.server_id.clone(),
            invalidation: self.invalidation.clone(),
            change_notify: self.change_notify.clone(),
            closed: self.closed.clone(),
            call_gate: self.call_gate.clone(),
        }
    }

    /// Opens the MCP 2026-07-28 `subscriptions/listen` stream that carries
    /// `tools/list_changed` for inline-lifecycle peers: the modern entry
    /// point into [`ToolListChangedSink`].
    pub(super) async fn subscribe_tool_list_changed(self: &Arc<Self>) -> Result<(), McpError> {
        let mut subscription = self
            .peer
            .listen(SubscriptionFilter::builder().tools_list_changed().build())
            .await
            .map_err(|error| McpError::Discovery(bound_error(&error.to_string())))?;
        let sink = self.tool_list_changed_sink();
        let task = tokio::spawn(async move {
            while let Ok(Some(notification)) = subscription.next().await {
                let Some(admitted) = sink.admit().await else {
                    break;
                };
                if matches!(
                    notification,
                    ServerNotification::ToolListChangedNotification(_)
                ) {
                    admitted.invalidate();
                }
            }
        });
        *self.subscription.lock().await = Some(task);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures_util::FutureExt as _;

    use super::{McpInvalidationState, ToolListChangedSink};
    use crate::runtime::identity::McpServerId;

    fn sink(state: &Arc<McpInvalidationState>) -> (ToolListChangedSink, Arc<AtomicBool>) {
        let closed = Arc::new(AtomicBool::new(false));
        (
            ToolListChangedSink {
                server_id: McpServerId::new("fixture"),
                invalidation: Arc::clone(state),
                change_notify: Arc::new(tokio::sync::Notify::new()),
                closed: Arc::clone(&closed),
                call_gate: Arc::new(tokio::sync::RwLock::new(())),
            },
            closed,
        )
    }

    /// Both protocol entry points (the 2026-07-28 subscription task and the
    /// legacy client callback) share this exact sink, so this is the one
    /// admission/invalidation/notification outcome either can produce: an
    /// open runtime advances its server's epoch exactly once per
    /// notification and wakes its change waiters; another server's epoch is
    /// untouched.
    #[tokio::test]
    async fn an_admitted_notification_advances_one_epoch_once_and_wakes_waiters() {
        let state = Arc::new(McpInvalidationState::new());
        let (sink, _closed) = sink(&state);
        let notified = sink.change_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        sink.admit()
            .await
            .expect("open runtime admits")
            .invalidate();

        assert_eq!(state.epoch(&McpServerId::new("fixture")), 1);
        assert_eq!(state.epoch(&McpServerId::new("other")), 0);
        assert!(
            notified.now_or_never().is_some(),
            "the change waiter is woken"
        );

        sink.admit().await.expect("still open").invalidate();
        assert_eq!(state.epoch(&McpServerId::new("fixture")), 2);
    }

    /// After close the sink refuses admission, so neither entry point can
    /// mutate invalidation state or wake waiters, and the close write
    /// barrier waits for a notification that was admitted before it.
    #[tokio::test]
    async fn a_closed_runtime_refuses_admission_and_close_waits_for_an_admitted_one() {
        let state = Arc::new(McpInvalidationState::new());
        let (sink, closed) = sink(&state);

        let admitted = sink.admit().await.expect("open runtime admits");
        let barrier = Arc::clone(&sink.call_gate);
        let close_barrier = barrier.write();
        tokio::pin!(close_barrier);
        assert!(
            close_barrier.as_mut().now_or_never().is_none(),
            "close cannot cross the write barrier while a notification is admitted"
        );
        admitted.invalidate();
        drop(close_barrier.await);

        closed.store(true, Ordering::Release);
        assert!(
            sink.admit().await.is_none(),
            "a closed runtime refuses admission"
        );
        assert_eq!(state.epoch(&McpServerId::new("fixture")), 1);
    }
}
