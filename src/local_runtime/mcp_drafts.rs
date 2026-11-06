//! Ephemeral selected-Workspace MCP ownership. One connection owns one current
//! unsent draft; a single supervisor coalesces replacements while old physical
//! owners settle. A claim removes the slot under that same lock, once.
use super::composition::{
    McpInitialSubmission, SubmissionIntent, admitted_source_demand, captured_mcp_bindings,
};
use super::configuration::{AdmittedSessionConfig, UserConfigManager};
use crate::capabilities::{McpPreparation, McpPreparationState, ToolSourceId};
use crate::runtime::cancellation::CancellationSignal;
use crate::tools::mcp::McpServerBindings;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{Notify, Semaphore, watch};

/// An opaque draft identity and monotonically increasing selection generation.
/// Native connection identity supplies tab and endpoint isolation.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct McpDraftTarget {
    pub draft_id: String,
    pub generation: u32,
    pub host_authority: String,
}
impl McpDraftTarget {
    pub(crate) fn validate(&self) -> bool {
        !self.draft_id.is_empty()
            && self.draft_id.len() <= 128
            && self.generation > 0
            && !self.host_authority.is_empty()
            && self.host_authority.len() <= 256
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Inputs {
    workspace: PathBuf,
    workspace_identity: (u64, u64),
    bindings: McpServerBindings,
}
impl Inputs {
    fn capture(paths: &AdmittedSessionConfig) -> Result<Self, String> {
        paths.validate_resource_authority()?;
        let demand = admitted_source_demand(
            &paths.config,
            &paths.subagents,
            &paths.workflows,
            paths.managed_python.clone(),
        );
        let mut bindings = captured_mcp_bindings(&paths.config, &paths.credentials)
            .map_err(|error| error.to_string())?;
        bindings.retain(|id, _| demand.sources.contains(&ToolSourceId::Mcp(id.clone())));
        let physical = std::fs::metadata(&paths.workspace).map_err(|error| error.to_string())?;
        Ok(Self {
            workspace: paths.workspace.clone(),
            workspace_identity: (physical.dev(), physical.ino()),
            bindings,
        })
    }
}

// Thirty-two live/initializing/retiring draft sources per App Server. Whole
// batches reserve their finite source count, so ready sources never deadlock a
// batch waiting for another slot. Oversized drafts use normal submission startup.
const PREWARM_SOURCES: usize = 32;
#[derive(Debug)]
pub(crate) struct McpDraftRegistry {
    capacity: Arc<Semaphore>,
    scopes: Mutex<Vec<Weak<Scope>>>,
    draining: AtomicBool,
    failures: Arc<Mutex<Vec<String>>>,
    cleanup: watch::Sender<usize>,
}
impl Default for McpDraftRegistry {
    fn default() -> Self {
        Self {
            capacity: Arc::new(Semaphore::new(PREWARM_SOURCES)),
            scopes: Mutex::default(),
            draining: AtomicBool::new(false),
            failures: Arc::default(),
            cleanup: watch::channel(0).0,
        }
    }
}
/// Operational preparation facts only; a prepared batch grants no executable
/// capability authority until the Conversation's ordinary candidate commits.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct McpDraftDiagnostics {
    pub pending: usize,
    pub prepared: usize,
    pub failed: usize,
    pub cancelling: usize,
    pub source_capacity_used: usize,
    pub source_capacity_limit: usize,
    pub cleanup_failures: Vec<String>,
}
impl McpDraftRegistry {
    #[cfg(test)]
    pub(crate) async fn reserve_all_for_test(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.capacity
            .clone()
            .acquire_many_owned(u32::try_from(PREWARM_SOURCES).unwrap())
            .await
            .unwrap()
    }
    pub(crate) fn diagnostics(&self) -> McpDraftDiagnostics {
        let mut result = McpDraftDiagnostics {
            source_capacity_used: PREWARM_SOURCES - self.capacity.available_permits(),
            source_capacity_limit: PREWARM_SOURCES,
            cleanup_failures: self.failures.lock().expect("draft failures").clone(),
            ..Default::default()
        };
        for scope in self
            .scopes
            .lock()
            .expect("draft scopes")
            .iter()
            .filter_map(Weak::upgrade)
        {
            let control = scope.control.lock().expect("draft control");
            let mut seen = Vec::new();
            for slot in [&control.target, &control.current, &control.reserved]
                .into_iter()
                .flatten()
            {
                if seen.iter().any(|other| Arc::ptr_eq(other, slot)) {
                    continue;
                }
                seen.push(slot.clone());
                if slot.cancellation.is_cancelled() {
                    result.cancelling += 1;
                    continue;
                }
                match slot.result.lock().expect("draft result").as_ref() {
                    Some(Ok((_, Some(preparation)))) => match preparation.state() {
                        McpPreparationState::Pending => result.pending += 1,
                        McpPreparationState::Prepared => result.prepared += 1,
                        McpPreparationState::Failed => result.failed += 1,
                    },
                    Some(Err(_)) => result.failed += 1,
                    None if !*slot.transferred.borrow() => result.pending += 1,
                    Some(Ok((_, None))) | None => {}
                }
            }
        }
        result
    }
    pub(crate) fn open(
        self: &Arc<Self>,
        configuration: UserConfigManager,
        credentials: crate::credentials::CredentialSnapshot,
    ) -> McpDraftScope {
        let scope = Arc::new(Scope {
            control: Mutex::new(Control::default()),
            changed: Notify::new(),
            configuration,
            credentials,
            registry: self.clone(),
        });
        let mut scopes = self.scopes.lock().expect("draft scopes");
        scopes.retain(|scope| scope.strong_count() > 0);
        scopes.push(Arc::downgrade(&scope));
        McpDraftScope(Arc::new(ScopeLease(scope)))
    }
    pub(crate) fn begin_drain(&self) {
        self.draining.store(true, Ordering::Release);
        for scope in self
            .scopes
            .lock()
            .expect("draft scopes")
            .iter()
            .filter_map(Weak::upgrade)
        {
            scope.close();
        }
    }
    pub(crate) async fn settle(&self) -> Vec<String> {
        self.begin_drain();
        let scopes: Vec<_> = self
            .scopes
            .lock()
            .expect("draft scopes")
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for scope in scopes {
            loop {
                let changed = scope.changed.notified();
                if !scope.control.lock().expect("draft control").running {
                    break;
                }
                changed.await;
            }
        }
        self.cleanup
            .subscribe()
            .wait_for(|count| *count == 0)
            .await
            .expect("draft cleanup owner");
        self.failures.lock().expect("draft failures").clone()
    }
    pub(crate) fn is_settled(&self) -> bool {
        self.capacity.available_permits() == PREWARM_SOURCES
            && *self.cleanup.borrow() == 0
            && self.failures.lock().expect("draft failures").is_empty()
            && self
                .scopes
                .lock()
                .expect("draft scopes")
                .iter()
                .filter_map(Weak::upgrade)
                .all(|scope| !scope.control.lock().expect("draft control").running)
    }
}

#[derive(Clone)]
pub(crate) struct McpDraftScope(Arc<ScopeLease>);
struct ScopeLease(Arc<Scope>);
impl Drop for ScopeLease {
    fn drop(&mut self) {
        self.0.close();
    }
}
impl std::fmt::Debug for McpDraftScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpDraftScope").finish_non_exhaustive()
    }
}
struct Scope {
    control: Mutex<Control>,
    changed: Notify,
    configuration: UserConfigManager,
    credentials: crate::credentials::CredentialSnapshot,
    registry: Arc<McpDraftRegistry>,
}
#[derive(Default)]
struct Control {
    closed: bool,
    running: bool,
    target: Option<Arc<Slot>>,
    current: Option<Arc<Slot>>,
    reserved: Option<Arc<Slot>>,
    // Retained after cancellation/claim: a delayed earlier request cannot
    // recreate a consumed or superseded generation on this connection.
    latest: Option<McpDraftTarget>,
}
type PreparationResult = Result<(Inputs, Option<McpPreparation>), String>;
struct Slot {
    target: McpDraftTarget,
    cwd: PathBuf,
    startup: McpInitialSubmission,
    cancellation: CancellationSignal,
    started: AtomicBool,
    claimed: AtomicBool,
    transferred: watch::Sender<bool>,
    scope: Weak<Scope>,
    complete: watch::Sender<bool>,
    result: Mutex<Option<PreparationResult>>,
    registry: Weak<McpDraftRegistry>,
    #[cfg(test)]
    capacity_waiting: watch::Sender<bool>,
    #[cfg(test)]
    consume_waiting: watch::Sender<bool>,
}
impl Slot {
    fn cancel(&self) {
        self.cancellation.cancel();
        if let Some(Ok((_, Some(preparation)))) = self.result.lock().expect("draft result").as_ref()
        {
            preparation.cancel();
        }
    }
    fn release_reservation(&self) {
        if let Some(scope) = self.scope.upgrade() {
            let mut control = scope.control.lock().expect("draft control");
            if control
                .reserved
                .as_ref()
                .is_some_and(|slot| std::ptr::eq(slot.as_ref(), self))
            {
                control.reserved = None;
            }
        }
    }
    fn transfer(&self) -> bool {
        let Some(scope) = self.scope.upgrade() else {
            return false;
        };
        let mut control = scope.control.lock().expect("draft control");
        if control.closed
            || self.cancellation.is_cancelled()
            || !control
                .reserved
                .as_ref()
                .is_some_and(|slot| std::ptr::eq(slot.as_ref(), self))
        {
            return false;
        }
        control.reserved = None;
        self.transferred.send_replace(true);
        true
    }
    async fn take(&self) -> Result<(Inputs, Option<McpPreparation>), String> {
        self.complete
            .subscribe()
            .wait_for(|complete| *complete)
            .await
            .expect("draft owner completion");
        self.result
            .lock()
            .expect("draft result")
            .take()
            .unwrap_or_else(|| Err("draft resources were already consumed".into()))
    }
    async fn settle(&self) -> Vec<String> {
        self.cancel();
        match self.take().await {
            Ok((_, Some(preparation))) => preparation.settle().await,
            _ => Vec::new(),
        }
    }
}
impl McpDraftScope {
    #[cfg(test)]
    pub(crate) fn consume_wait_for_test(&self) -> watch::Receiver<bool> {
        self.0
            .0
            .control
            .lock()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .consume_waiting
            .subscribe()
    }
    #[cfg(test)]
    pub(crate) fn capacity_wait_for_test(&self) -> watch::Receiver<bool> {
        self.0
            .0
            .control
            .lock()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .capacity_waiting
            .subscribe()
    }
    #[cfg(test)]
    pub(crate) fn transfer_for_test(&self) -> watch::Receiver<bool> {
        self.0
            .0
            .control
            .lock()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .transferred
            .subscribe()
    }
    #[cfg(test)]
    pub(crate) async fn prepared_for_test(&self) -> McpInitialSubmission {
        let slot = self
            .0
            .0
            .control
            .lock()
            .expect("draft control")
            .target
            .clone()
            .expect("selected draft");
        slot.complete
            .subscribe()
            .wait_for(|complete| *complete)
            .await
            .unwrap();
        let completion = slot
            .result
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .and_then(|(_, preparation)| preparation.as_ref().map(McpPreparation::completion));
        if let Some(mut completion) = completion {
            completion
                .wait_for(|complete| *complete != McpPreparationState::Pending)
                .await
                .unwrap();
        }
        slot.startup.clone()
    }
    pub(crate) fn select(&self, target: McpDraftTarget, cwd: PathBuf) -> Result<(), String> {
        if !target.validate() || !cwd.is_absolute() {
            return Err("invalid draft selection".into());
        }
        let scope = &self.0.0;
        let mut control = scope.control.lock().expect("draft control");
        if control.closed || scope.registry.draining.load(Ordering::Acquire) {
            return Err("draft owner is closed".into());
        }
        if control.reserved.is_some() {
            return Err("first submission owns this draft selection".into());
        }
        if let Some(latest) = &control.latest
            && latest.generation >= target.generation
        {
            return if latest == &target
                && control.target.as_ref().is_some_and(|slot| slot.cwd == cwd)
            {
                Ok(())
            } else {
                Err("draft selection was superseded or consumed".into())
            };
        }
        if let Some(previous) = control.target.take() {
            previous.cancel();
        }
        control.latest = Some(target.clone());
        control.target = Some(Arc::new(Slot {
            target,
            cwd,
            startup: McpInitialSubmission::default(),
            cancellation: CancellationSignal::new(),
            started: AtomicBool::new(false),
            claimed: AtomicBool::new(false),
            transferred: watch::channel(false).0,
            scope: Arc::downgrade(scope),
            complete: watch::channel(false).0,
            result: Mutex::new(None),
            registry: Arc::downgrade(&scope.registry),
            #[cfg(test)]
            capacity_waiting: watch::channel(false).0,
            #[cfg(test)]
            consume_waiting: watch::channel(false).0,
        }));
        scope.start_supervisor(&mut control);
        Ok(())
    }
    pub(crate) fn cancel(&self, target: &McpDraftTarget) {
        let scope = &self.0.0;
        let mut control = scope.control.lock().expect("draft control");
        if control
            .reserved
            .as_ref()
            .is_some_and(|slot| slot.target == *target)
            && let Some(slot) = control.reserved.take()
        {
            slot.cancel();
        }
        if control
            .target
            .as_ref()
            .is_some_and(|slot| slot.target == *target)
        {
            if let Some(slot) = control.target.take() {
                slot.cancel();
            }
            scope.start_supervisor(&mut control);
        }
    }
    /// The sole cancel/claim/replace linearization boundary. A stale cancel no
    /// longer has a route to this slot, its cancellation root or physical owners.
    pub(crate) fn claim(&self, target: &McpDraftTarget, receipt: SubmissionIntent) -> DraftClaim {
        let scope = &self.0.0;
        let mut control = scope.control.lock().expect("draft control");
        let slot = if !control.closed
            && control
                .target
                .as_ref()
                .is_some_and(|slot| slot.target == *target && !slot.cancellation.is_cancelled())
        {
            let slot = control.target.take().expect("matched draft");
            slot.claimed.store(true, Ordering::Release);
            control.reserved = Some(slot.clone());
            Some(slot)
        } else {
            None
        };
        if control
            .latest
            .as_ref()
            .is_none_or(|latest| latest.generation < target.generation)
        {
            control.latest = Some(target.clone());
        }
        let startup = slot
            .as_ref()
            .map_or_else(McpInitialSubmission::default, |slot| slot.startup.clone());
        // Receipt is captured by the connection before any asynchronous create,
        // configuration, capacity, attachment or claim readiness wait.
        startup.record(receipt).expect("policy not yet bound");
        if let Some(slot) = &slot {
            scope.start_slot(slot.clone());
        }
        DraftClaim { slot, startup }
    }
    pub(crate) fn close(&self) {
        self.0.0.close();
    }
}
impl Scope {
    fn start_supervisor(self: &Arc<Self>, control: &mut Control) {
        if control.running {
            return;
        }
        control.running = true;
        let scope = self.clone();
        tokio::spawn(async move {
            scope.supervise().await;
        });
    }
    fn close(self: &Arc<Self>) {
        let mut control = self.control.lock().expect("draft control");
        if control.closed {
            return;
        }
        control.closed = true;
        if let Some(slot) = control.reserved.take() {
            slot.cancel();
        }
        if let Some(slot) = control.target.take() {
            slot.cancel();
        }
        if control.current.is_some() {
            self.start_supervisor(&mut control);
        }
    }
    fn start_slot(self: &Arc<Self>, slot: Arc<Slot>) {
        if slot.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let scope = self.clone();
        tokio::spawn(async move {
            let result = scope.prepare(&slot).await;
            let mut result_slot = slot.result.lock().expect("draft result");
            if slot.cancellation.is_cancelled()
                && let Ok((_, Some(preparation))) = &result
            {
                preparation.cancel();
            }
            *result_slot = Some(result);
            drop(result_slot);
            slot.complete.send_replace(true);
        });
    }
    async fn supervise(self: Arc<Self>) {
        loop {
            let (retire, start) = {
                let mut control = self.control.lock().expect("draft control");
                let current = control.current.take();
                match current {
                    Some(slot) if slot.claimed.load(Ordering::Acquire) => (None, None),
                    Some(slot)
                        if control
                            .target
                            .as_ref()
                            .is_some_and(|target| Arc::ptr_eq(target, &slot)) =>
                    {
                        control.current = Some(slot);
                        control.running = false;
                        self.changed.notify_waiters();
                        return;
                    }
                    Some(slot) => (Some(slot), None),
                    None => {
                        let start = control.target.clone();
                        if start.is_none() {
                            control.running = false;
                            self.changed.notify_waiters();
                            return;
                        }
                        control.current.clone_from(&start);
                        (None, start)
                    }
                }
            };
            if let Some(slot) = retire {
                // A latest target that was replaced before its turn has no task.
                self.start_slot(slot.clone());
                let failures = slot.settle().await;
                if !failures.is_empty() {
                    self.registry
                        .failures
                        .lock()
                        .expect("draft failures")
                        .extend(failures);
                    self.close();
                }
            }
            if let Some(slot) = start {
                self.start_slot(slot);
            }
        }
    }
    async fn prepare(&self, slot: &Slot) -> Result<(Inputs, Option<McpPreparation>), String> {
        if slot.cancellation.is_cancelled() {
            return Err("draft cancelled".into());
        }
        let configuration = self.configuration.clone();
        let credentials = self.credentials.clone();
        let cwd = slot.cwd.clone();
        let paths = tokio::task::spawn_blocking(move || {
            configuration
                .resolve_session(&super::configuration::SessionConfigInput { cwd, model: None })
                .map_err(|error| error.to_string())?
                .admit(|| credentials)
        })
        .await
        .map_err(|error| error.to_string())??;
        let inputs = Inputs::capture(&paths)?;
        if inputs.bindings.is_empty() {
            return Ok((inputs, None));
        }
        let count = u32::try_from(inputs.bindings.len())
            .map_err(|_| "MCP preparation capacity exceeded")?;
        if inputs.bindings.len() > PREWARM_SOURCES {
            return Err("MCP preparation capacity exceeded".into());
        }
        let deadline = slot.startup.deadline();
        #[cfg(test)]
        slot.capacity_waiting.send_replace(true);
        let capacity = tokio::select! {
            biased;
            () = slot.cancellation.cancelled() => return Err("draft cancelled".into()),
            () = deadline.expired() => return Err("MCP preparation capacity wait timed out".into()),
            capacity = self.registry.capacity.clone().acquire_many_owned(count) => capacity.map_err(|error| error.to_string())?,
        };
        if Inputs::capture(&paths)? != inputs {
            return Err("Workspace changed while MCP preparation awaited capacity".into());
        }
        let workspace = crate::tools::workspace::Workspace::new(&inputs.workspace)
            .map_err(|error| error.to_string())?;
        self.registry.cleanup.send_modify(|count| *count += 1);
        let registry = self.registry.clone();
        let settled = Arc::new(move |failures| {
            registry
                .failures
                .lock()
                .expect("draft failures")
                .extend(failures);
            registry.cleanup.send_modify(|count| *count -= 1);
        });
        let preparation = McpPreparation::start(
            workspace,
            inputs.bindings.clone(),
            deadline,
            capacity,
            slot.cancellation.clone(),
            settled,
        );
        Ok((inputs, Some(preparation)))
    }
}

/// This guard belongs to creation, then the loading flight. It is never reusable.
#[derive(Debug)]
pub(crate) struct DraftClaim {
    slot: Option<Arc<Slot>>,
    pub(crate) startup: McpInitialSubmission,
}
impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpDraftSlot").finish_non_exhaustive()
    }
}
impl DraftClaim {
    pub(crate) fn into_startup(mut self) -> McpInitialSubmission {
        std::mem::take(&mut self.startup).with_claim(self)
    }
    pub(crate) async fn consume(
        mut self,
        paths: &AdmittedSessionConfig,
        cancellation: &CancellationSignal,
    ) -> Result<Option<McpPreparation>, String> {
        let Some(slot) = self.slot.take() else {
            return Ok(None);
        };
        #[cfg(test)]
        slot.consume_waiting.send_replace(true);
        let prepared = tokio::select! {
            outcome = slot.take() => outcome,
            () = cancellation.cancelled() => { slot.cancel(); slot.take().await },
        };
        match (Inputs::capture(paths), prepared) {
            (Ok(current), Ok((captured, preparation)))
                if current == captured && slot.transfer() =>
            {
                Ok(preparation)
            }
            (_, Ok((_, Some(preparation)))) => {
                slot.release_reservation();
                let failures = preparation.settle().await;
                if failures.is_empty() {
                    Ok(None)
                } else {
                    Err(failures.join("; "))
                }
            }
            (Err(error), _) => {
                slot.release_reservation();
                Err(error)
            }
            _ => {
                slot.release_reservation();
                Ok(None)
            }
        }
    }
}
struct CleanupCount(watch::Sender<usize>);
impl Drop for CleanupCount {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
    }
}
impl Drop for DraftClaim {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            if let Some(scope) = slot.scope.upgrade() {
                let mut control = scope.control.lock().expect("draft control");
                if control
                    .reserved
                    .as_ref()
                    .is_some_and(|reserved| Arc::ptr_eq(reserved, &slot))
                {
                    control.reserved = None;
                }
            }
            slot.cancel();
            let registry = slot.registry.upgrade();
            let count = registry.as_ref().map(|registry| {
                registry.cleanup.send_modify(|count| *count += 1);
                CleanupCount(registry.cleanup.clone())
            });
            tokio::spawn(async move {
                let _count = count;
                let failures = slot.settle().await;
                if let Some(registry) = registry {
                    registry
                        .failures
                        .lock()
                        .expect("draft failures")
                        .extend(failures);
                }
            });
        }
    }
}
