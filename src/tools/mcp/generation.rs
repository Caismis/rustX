//! Publication-owned MCP runtime generations, execution leases, and the
//! coordinator's retirement registry.
//!
//! A *runtime resource generation* ([`McpRuntimeGeneration`]) is the
//! capability-publication owner of one stable MCP connection. It is distinct
//! from a *transport connection generation* (see `connection`), which the
//! connection replaces without touching published capability knowledge.
//!
//! ```text
//! published capability generation
//!   -> McpRuntimeGeneration          (retired on drop / explicit retire)
//!        -> McpRuntimeBinding        (non-owning; held by executors)
//!             -> McpRuntimeLease     (one per admitted execution)
//! retired + zero leases -> one close task -> physical settlement evidence
//!   -> McpRuntimeRetirementRegistry  (reaps only proven settlements)
//! ```
//!
//! Lease zero is the only close trigger, and a generation whose physical
//! settlement is unproven is never reaped: its terminal failure is retained
//! as evidence for the owning runtime drain.

use std::sync::{Arc, Mutex, Weak};

use crate::runtime::identity::McpServerId;
use crate::runtime::types::LifecycleAdmission;

use super::connection;

/// The coordinator-owned retirement registry for MCP physical generations.
///
/// A generation is registered only after its publication/candidate owner has
/// retired. The registry contains no active pool: it is a bounded list of
/// close tasks that have not yet been reaped, plus authoritative terminal
/// failure evidence retained until the owning runtime drains it. A generation
/// whose physical settlement is unproven is never reaped.
pub(crate) struct McpRuntimeRetirementRegistry {
    inner: Arc<McpRuntimeRetirementRegistryInner>,
}

pub(crate) type McpRetirementFailureCallback = Arc<dyn Fn(String) + Send + Sync>;

struct McpRuntimeRetirementRegistryInner {
    entries: Mutex<Vec<Arc<McpRuntimeGenerationInner>>>,
    failures: Mutex<Vec<(McpServerId, String)>>,
    failure_callback: Mutex<Option<McpRetirementFailureCallback>>,
}

impl McpRuntimeRetirementRegistry {
    /// Creates the one retirement registry of a capability coordinator.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(McpRuntimeRetirementRegistryInner {
                entries: Mutex::new(Vec::new()),
                failures: Mutex::new(Vec::new()),
                failure_callback: Mutex::new(None),
            }),
        })
    }

    /// Installs the runtime-owned failure seam. The callback is invoked
    /// outside every registry mutex and therefore may begin runtime drain.
    pub(crate) fn install_failure_callback(&self, callback: &McpRetirementFailureCallback) {
        let existing = {
            let mut callback_slot = self
                .inner
                .failure_callback
                .lock()
                .expect("MCP retirement callback lock poisoned");
            *callback_slot = Some(callback.clone());
            self.inner
                .failures
                .lock()
                .expect("MCP retirement failure lock poisoned")
                .clone()
        };
        for (server_id, error) in existing {
            callback(format!("MCP server {server_id}: {error}"));
        }
    }

    fn register(&self, generation: Arc<McpRuntimeGenerationInner>) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .expect("MCP retirement lock poisoned");
        if !entries.iter().any(|entry| Arc::ptr_eq(entry, &generation)) {
            entries.push(generation);
        }
    }

    pub(crate) fn record_failure(&self, server_id: &McpServerId, error: &str) {
        let callback = {
            let callback_slot = self
                .inner
                .failure_callback
                .lock()
                .expect("MCP retirement callback lock poisoned");
            let mut failures = self
                .inner
                .failures
                .lock()
                .expect("MCP retirement failure lock poisoned");
            let failure = (server_id.clone(), error.to_owned());
            if failures.contains(&failure) {
                None
            } else {
                failures.push(failure);
                callback_slot.clone()
            }
        };
        if let Some(callback) = callback {
            callback(format!("MCP server {server_id}: {error}"));
        }
    }

    fn failure_diagnostics(&self) -> Vec<String> {
        let mut failures = self
            .inner
            .failures
            .lock()
            .expect("MCP retirement failure lock poisoned")
            .clone();
        failures.sort();
        failures
            .into_iter()
            .map(|(server_id, error)| format!("MCP server {server_id}: {error}"))
            .collect()
    }

    fn snapshot(&self) -> Vec<Arc<McpRuntimeGenerationInner>> {
        self.inner
            .entries
            .lock()
            .expect("MCP retirement lock poisoned")
            .clone()
    }

    #[cfg(all(test, feature = "mcp-fixture"))]
    pub(crate) fn pending_count(&self) -> usize {
        self.inner
            .entries
            .lock()
            .expect("MCP retirement lock poisoned")
            .len()
    }

    fn reap(&self) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .expect("MCP retirement lock poisoned");
        entries.retain(|entry| !entry.physical_settlement_proven());
    }

    /// Waits for retired generations that are already free of execution
    /// leases. This is used after successful reload to keep the retired list
    /// bounded without waiting for legitimate background owners.
    pub(crate) async fn settle_ready(&self) -> Result<(), Vec<String>> {
        for generation in self.snapshot() {
            if generation.can_close() {
                generation.wait_close_attempt().await;
            }
        }
        self.reap();
        let failures = self.failure_diagnostics();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }

    /// Waits for every retired generation to settle. Runtime drain calls this
    /// only after all attempt/background owners have settled.
    pub(crate) async fn settle_all(&self) -> Vec<String> {
        for generation in self.snapshot() {
            generation.wait_close_attempt().await;
        }
        self.reap();
        self.failure_diagnostics()
    }
}

/// The publication/candidate owner of one physical MCP runtime generation.
///
/// This owner is moved from a prepared capability candidate into the
/// coordinator's published generation state. Dropping it retires the
/// generation; the physical runtime closes only after every explicit
/// execution lease has settled.
pub(crate) struct McpRuntimeGeneration {
    inner: Arc<McpRuntimeGenerationInner>,
}

struct McpRuntimeGenerationInner {
    server_id: McpServerId,
    /// The stable connection owner of this server (Issue #205).
    ///
    /// A published capability generation owns one connection, and the
    /// connection owns whichever transport generation is currently
    /// authoritative. Executors bind to the connection, so a replaced
    /// transport is served immediately by every already-admitted tool
    /// without touching published capability knowledge.
    connection: Arc<connection::McpConnection>,
    retirement: Weak<McpRuntimeRetirementRegistryInner>,
    handle: tokio::runtime::Handle,
    lifecycle_admission: Mutex<Option<LifecycleAdmission>>,
    state: Mutex<McpRuntimeGenerationState>,
    close_finished: tokio::sync::Notify,
}

#[allow(clippy::struct_excessive_bools)]
struct McpRuntimeGenerationState {
    retired: bool,
    execution_leases: usize,
    close_started: bool,
    /// The complete close/retirement terminal outcome was published,
    /// whether successfully or with a terminal settlement error. This is not
    /// proof of physical settlement; that fact is represented separately
    /// below.
    close_attempt_finished: bool,
    /// True only when the owned physical MCP boundary was proven terminal.
    physical_settlement_proven: bool,
    /// Terminal evidence retained when physical settlement is unproven.
    terminal_failure: Option<String>,
}

/// A non-owning binding used by an MCP executor. It does not keep a physical
/// generation alive by itself; each execution acquires an explicit lease.
#[derive(Clone)]
pub(crate) struct McpRuntimeBinding {
    inner: Arc<McpRuntimeGenerationInner>,
}

/// One explicit physical-runtime execution lease.
pub(crate) struct McpRuntimeLease {
    inner: Arc<McpRuntimeGenerationInner>,
}

/// The leases captured by one detached background execution. Keeping this
/// value in the runner makes dispatch ownership explicit even while the
/// originating attempt and capability snapshot have settled.
#[derive(Default)]
pub(crate) struct McpRuntimeLeaseSet {
    #[allow(dead_code)]
    leases: Vec<McpRuntimeLease>,
}

/// The immutable physical-lease authority paired with one published
/// capability generation. Holding this authority does not itself retain an
/// execution lease; an admitted attempt or detached execution acquires one
/// from this exact generation rather than consulting mutable coordinator
/// state later.
#[derive(Clone, Default)]
pub(crate) struct McpRuntimeLeaseAuthority {
    bindings: Arc<[McpRuntimeBinding]>,
}

impl McpRuntimeLeaseAuthority {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn from_generations(generations: &[McpRuntimeGeneration]) -> Self {
        Self {
            bindings: generations
                .iter()
                .map(McpRuntimeGeneration::binding)
                .collect::<Vec<_>>()
                .into(),
        }
    }

    pub(crate) fn acquire(&self) -> Option<McpRuntimeLeaseSet> {
        let mut leases = Vec::with_capacity(self.bindings.len());
        for binding in self.bindings.iter() {
            leases.push(binding.acquire_lease()?);
        }
        Some(McpRuntimeLeaseSet { leases })
    }
}

impl std::fmt::Debug for McpRuntimeGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpRuntimeGeneration")
            .field("server_id", &self.inner.server_id)
            .field("retired", &self.is_retired())
            .field("execution_leases", &self.execution_lease_count())
            .finish()
    }
}

impl McpRuntimeGeneration {
    /// The publication owner of one MCP connection.
    ///
    /// The connection — not a single transport — is what a published
    /// capability generation owns. Retiring this owner closes the connection
    /// and every transport generation it ever established.
    pub(crate) fn from_connection(
        server_id: McpServerId,
        connection: Arc<connection::McpConnection>,
        lifecycle_admission: Option<LifecycleAdmission>,
        handle: tokio::runtime::Handle,
        retirement: &Arc<McpRuntimeRetirementRegistry>,
    ) -> Self {
        Self {
            inner: Arc::new(McpRuntimeGenerationInner {
                server_id,
                connection,
                retirement: Arc::downgrade(&retirement.inner),
                handle,
                lifecycle_admission: Mutex::new(lifecycle_admission),
                state: Mutex::new(McpRuntimeGenerationState {
                    retired: false,
                    execution_leases: 0,
                    close_started: false,
                    close_attempt_finished: false,
                    physical_settlement_proven: false,
                    terminal_failure: None,
                }),
                close_finished: tokio::sync::Notify::new(),
            }),
        }
    }

    /// The server identity this published generation serves.
    #[cfg(test)]
    pub(crate) fn server_id(&self) -> &McpServerId {
        &self.inner.server_id
    }

    /// The stable connection owner of this published generation.
    pub(crate) fn connection(&self) -> &Arc<connection::McpConnection> {
        &self.inner.connection
    }

    pub(crate) fn binding(&self) -> McpRuntimeBinding {
        McpRuntimeBinding {
            inner: self.inner.clone(),
        }
    }

    pub(crate) fn retire(&self) {
        self.inner.connection.retire_activation();
        let first_retirement = {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("MCP generation lock poisoned");
            if state.retired {
                false
            } else {
                state.retired = true;
                true
            }
        };
        if first_retirement && let Some(retirement) = self.inner.retirement.upgrade() {
            McpRuntimeRetirementRegistry { inner: retirement }.register(self.inner.clone());
        }
        self.inner.maybe_schedule_close();
    }

    pub(crate) async fn retire_and_close(self) -> Option<String> {
        self.retire();
        self.inner.wait_close_attempt().await;
        self.inner.terminal_failure()
    }

    fn is_retired(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("MCP generation lock poisoned")
            .retired
    }

    pub(crate) fn execution_lease_count(&self) -> usize {
        self.inner.execution_lease_count()
    }
}

impl Drop for McpRuntimeGeneration {
    fn drop(&mut self) {
        self.retire();
    }
}

impl McpRuntimeBinding {
    pub(crate) fn acquire_lease(&self) -> Option<McpRuntimeLease> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("MCP generation lock poisoned");
        if state.close_started || state.close_attempt_finished {
            return None;
        }
        state.execution_leases += 1;
        Some(McpRuntimeLease {
            inner: self.inner.clone(),
        })
    }

    pub(crate) fn connection(&self) -> &Arc<connection::McpConnection> {
        &self.inner.connection
    }
}

impl McpRuntimeLease {
    fn try_clone(&self) -> Option<Self> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("MCP generation lock poisoned");
        if state.close_started || state.close_attempt_finished {
            return None;
        }
        state.execution_leases += 1;
        Some(Self {
            inner: self.inner.clone(),
        })
    }
}

impl Drop for McpRuntimeLease {
    fn drop(&mut self) {
        {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("MCP generation lock poisoned");
            state.execution_leases = state
                .execution_leases
                .checked_sub(1)
                .expect("MCP execution lease count underflow");
        }
        self.inner.maybe_schedule_close();
    }
}

impl McpRuntimeLeaseSet {
    pub(crate) fn try_clone(&self) -> Option<Self> {
        let mut leases = Vec::with_capacity(self.leases.len());
        for lease in &self.leases {
            leases.push(lease.try_clone()?);
        }
        Some(Self { leases })
    }

    /// Whether one of these leases resolves to the given published
    /// transport (deterministic ownership tests only).
    #[cfg(all(test, feature = "mcp-fixture"))]
    pub(crate) fn contains_runtime(&self, runtime: &Arc<super::McpServerRuntime>) -> bool {
        self.leases.iter().any(|lease| {
            lease
                .inner
                .connection
                .published_runtime()
                .is_some_and(|published| Arc::ptr_eq(&published, runtime))
        })
    }
}

impl McpRuntimeGenerationInner {
    fn execution_lease_count(&self) -> usize {
        self.state
            .lock()
            .expect("MCP generation lock poisoned")
            .execution_leases
    }

    fn can_close(&self) -> bool {
        let state = self.state.lock().expect("MCP generation lock poisoned");
        state.retired && state.execution_leases == 0
    }

    fn close_attempt_finished(&self) -> bool {
        self.state
            .lock()
            .expect("MCP generation lock poisoned")
            .close_attempt_finished
    }

    fn physical_settlement_proven(&self) -> bool {
        self.state
            .lock()
            .expect("MCP generation lock poisoned")
            .physical_settlement_proven
    }

    fn terminal_failure(&self) -> Option<String> {
        self.state
            .lock()
            .expect("MCP generation lock poisoned")
            .terminal_failure
            .clone()
    }

    fn maybe_schedule_close(self: &Arc<Self>) {
        let admission = {
            let mut state = self.state.lock().expect("MCP generation lock poisoned");
            if !state.retired || state.execution_leases != 0 || state.close_started {
                return;
            }
            state.close_started = true;
            self.lifecycle_admission
                .lock()
                .expect("MCP lifecycle admission lock poisoned")
                .take()
        };
        let inner = self.clone();
        self.handle.spawn(async move {
            // Closing the connection drives every transport generation it
            // established — the current one and any retired-but-unclosed
            // predecessor — to the same physical settlement proof.
            let failures = inner.connection.close().await;
            let failure = if failures.is_empty() {
                None
            } else {
                Some(failures.join("; "))
            };
            {
                let mut state = inner.state.lock().expect("MCP generation lock poisoned");
                state.physical_settlement_proven = failure.is_none();
                state.terminal_failure.clone_from(&failure);
            }
            if let Some(retirement) = inner.retirement.upgrade() {
                let registry = McpRuntimeRetirementRegistry { inner: retirement };
                if let Some(failure) = failure {
                    // This hook is intentionally after McpServerRuntime::close
                    // returned and before the registry/callback publication.
                    // The completion signal below must remain later than this
                    // entire terminal-outcome publication sequence.
                    #[cfg(test)]
                    if let Some(runtime) = inner.connection.published_runtime() {
                        runtime.wait_before_retirement_failure_publication().await;
                    }
                    registry.record_failure(&inner.server_id, &failure);
                }
                registry.reap();
            }
            // The lifecycle admission is part of the physical owner's
            // terminal handoff. Release it only after generation state,
            // registry evidence, and the runtime fencing callback are all
            // published, then make completion observable as the final step.
            drop(admission);
            {
                let mut state = inner.state.lock().expect("MCP generation lock poisoned");
                // `close_attempt_finished` is deliberately the last
                // publication. `wait_close_attempt` therefore cannot observe
                // completion before the complete terminal outcome is visible.
                state.close_attempt_finished = true;
            }
            inner.close_finished.notify_waiters();
        });
    }

    /// Waits for the complete terminal outcome publication. The close task
    /// sets `close_attempt_finished` only after generation state, retirement
    /// failure evidence, callback fencing, and lifecycle-admission release
    /// have all completed; this is stronger than merely waiting for the
    /// underlying `close()` future to return.
    async fn wait_close_attempt(&self) {
        loop {
            // The waiter must join the list *before* the flag is read.
            // `Notify::notified()` does not register until it is polled or
            // explicitly enabled, and `notify_waiters` stores no permit, so
            // creating the future and only then checking leaves a window in
            // which the notification reaches nobody and this call waits
            // forever.
            let notified = self.close_finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.close_attempt_finished() {
                return;
            }
            notified.await;
        }
    }
}
