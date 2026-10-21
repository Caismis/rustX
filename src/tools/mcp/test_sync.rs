//! Deterministic MCP ownership synchronization seams used by the M9c
//! regressions. Test-only: no production path constructs these.

use std::sync::{Arc, Mutex};

/// A deterministic close seam for one retained MCP runtime: it records
/// that `close` was entered, optionally parks there, and optionally makes
/// the close report an unproven physical settlement.
#[derive(Debug, Default)]
pub(crate) struct CloseProbe {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    failure_publication_entered: tokio::sync::Notify,
    failure_publication_release: tokio::sync::Notify,
    state: Mutex<CloseState>,
}

#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
struct CloseState {
    entered: bool,
    parks: bool,
    released: bool,
    failure: Option<String>,
    pause_before_failure_publication: bool,
    failure_publication_entered: bool,
    failure_publication_released: bool,
}

impl CloseProbe {
    /// A probe whose close reports an unproven physical settlement.
    pub(crate) fn failing(diagnostic: &str) -> Self {
        Self {
            state: Mutex::new(CloseState {
                failure: Some(diagnostic.to_owned()),
                ..CloseState::default()
            }),
            ..Self::default()
        }
    }

    /// A failing close that parks after `McpServerRuntime::close()` has
    /// returned and before the retirement registry publishes the failure.
    /// This is the deterministic race seam for the generation completion
    /// happens-before contract.
    pub(crate) fn failing_before_failure_publication(diagnostic: &str) -> Self {
        Self {
            state: Mutex::new(CloseState {
                failure: Some(diagnostic.to_owned()),
                pause_before_failure_publication: true,
                ..CloseState::default()
            }),
            ..Self::default()
        }
    }

    /// A probe whose close parks until [`CloseProbe::release`].
    pub(crate) fn parking() -> Self {
        Self {
            state: Mutex::new(CloseState {
                parks: true,
                ..CloseState::default()
            }),
            ..Self::default()
        }
    }

    /// Whether `close` has been entered on the probed runtime.
    pub(crate) fn was_entered(&self) -> bool {
        self.state.lock().expect("close probe lock").entered
    }

    pub(crate) fn injected_failure(&self) -> Option<String> {
        self.state.lock().expect("close probe lock").failure.clone()
    }

    pub(crate) async fn enter(&self) {
        {
            let mut state = self.state.lock().expect("close probe lock");
            state.entered = true;
            if !state.parks || state.released {
                drop(state);
                self.entered.notify_waiters();
                return;
            }
        }
        self.entered.notify_waiters();
        loop {
            let released = self.release.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if self.state.lock().expect("close probe lock").released {
                return;
            }
            released.await;
        }
    }

    /// Waits until `close` has been entered on the probed runtime.
    pub(crate) async fn wait_entered(&self) {
        loop {
            let entered = self.entered.notified();
            tokio::pin!(entered);
            entered.as_mut().enable();
            if self.state.lock().expect("close probe lock").entered {
                return;
            }
            entered.await;
        }
    }

    /// Releases a parked close.
    pub(crate) fn release(&self) {
        self.state.lock().expect("close probe lock").released = true;
        self.release.notify_waiters();
    }

    /// Parks the generation close task before retirement failure
    /// publication when the specialized failing probe is armed.
    pub(crate) async fn wait_before_failure_publication(&self) {
        {
            let mut state = self.state.lock().expect("close probe lock");
            if !state.pause_before_failure_publication || state.failure_publication_released {
                return;
            }
            state.failure_publication_entered = true;
        }
        self.failure_publication_entered.notify_waiters();
        loop {
            let released = self.failure_publication_release.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if self
                .state
                .lock()
                .expect("close probe lock")
                .failure_publication_released
            {
                return;
            }
            released.await;
        }
    }

    /// Waits until the close task reaches the pre-publication park.
    pub(crate) async fn wait_before_failure_publication_entered(&self) {
        loop {
            let entered = self.failure_publication_entered.notified();
            tokio::pin!(entered);
            entered.as_mut().enable();
            if self
                .state
                .lock()
                .expect("close probe lock")
                .failure_publication_entered
            {
                return;
            }
            entered.await;
        }
    }

    /// Releases the pre-retirement-publication close park.
    pub(crate) fn release_before_failure_publication(&self) {
        self.state
            .lock()
            .expect("close probe lock")
            .failure_publication_released = true;
        self.failure_publication_release.notify_waiters();
    }
}

/// Parks one MCP stdio connect at the exact instant physical process
/// ownership exists and the handshake has not started.
#[derive(Debug, Default)]
pub(crate) struct ConnectOwnershipPause {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    state: Mutex<PauseState>,
}

#[derive(Debug, Default)]
struct PauseState {
    entered: bool,
    released: bool,
    supervisor_pid: Option<u32>,
}

impl ConnectOwnershipPause {
    pub(crate) fn record_process(&self, pid: Option<u32>) {
        self.state
            .lock()
            .expect("connect pause lock")
            .supervisor_pid = pid;
    }

    pub(crate) fn supervisor_pid(&self) -> Option<u32> {
        self.state
            .lock()
            .expect("connect pause lock")
            .supervisor_pid
    }

    /// Called by the connect owner: announces physical ownership and
    /// waits for the test to release it.
    pub(crate) async fn park(&self) {
        {
            let mut state = self.state.lock().expect("connect pause lock");
            state.entered = true;
            if state.released {
                return;
            }
        }
        self.entered.notify_waiters();
        loop {
            let released = self.release.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if self.state.lock().expect("connect pause lock").released {
                return;
            }
            released.await;
        }
    }

    /// Waits until physical process ownership provably exists.
    pub(crate) async fn wait_entered(&self) {
        loop {
            let entered = self.entered.notified();
            tokio::pin!(entered);
            entered.as_mut().enable();
            if self.state.lock().expect("connect pause lock").entered {
                return;
            }
            entered.await;
        }
    }

    /// Releases the parked connect owner.
    pub(crate) fn release(&self) {
        self.state.lock().expect("connect pause lock").released = true;
        self.release.notify_waiters();
    }
}

/// The process-global registry of one kind of installed MCP test probe.
///
/// # Why not a single `Option`
///
/// Tests in one Rust test binary run concurrently. A single slot makes
/// installation and removal *ownership bugs*: installing a second probe
/// silently replaces the first, and either guard's drop clears whichever
/// probe happens to be current — so one test can uninstall another's
/// synchronization state and never know.
///
/// Every probe is instead registered under its own identity token, and a
/// guard removes **only the registration it created**. Concurrent probes
/// coexist; lookup is by the scoped tool name a probe was installed for,
/// and every MCP fixture mints tool names that belong to exactly one
/// fixture instance, so a probe can never match another test's call.
struct ProbeRegistry<T> {
    installed: Mutex<Vec<(u64, Arc<T>)>>,
}

/// The identity of one probe registration. Monotone for the process, so
/// a stale guard can never name a live registration.
static NEXT_PROBE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl<T> ProbeRegistry<T> {
    const fn new() -> Self {
        Self {
            installed: Mutex::new(Vec::new()),
        }
    }

    /// Registers one probe and returns the identity that owns it.
    fn install(&self, probe: Arc<T>) -> u64 {
        let id = NEXT_PROBE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.installed
            .lock()
            .expect("MCP probe registry lock poisoned")
            .push((id, probe));
        id
    }

    /// Removes exactly the registration `id` names, and nothing else.
    fn remove(&self, id: u64) {
        self.installed
            .lock()
            .expect("MCP probe registry lock poisoned")
            .retain(|(installed, _)| *installed != id);
    }

    /// Every installed probe.
    ///
    /// The router's observations carry no tool name, so a fact they
    /// record is offered to every installed probe; each one keeps only
    /// what belongs to the requests it actually parked.
    fn all(&self) -> Vec<Arc<T>> {
        self.installed
            .lock()
            .expect("MCP probe registry lock poisoned")
            .iter()
            .map(|(_, probe)| Arc::clone(probe))
            .collect()
    }

    /// The installed probe matching `predicate`, when one is installed.
    fn find(&self, predicate: impl Fn(&T) -> bool) -> Option<Arc<T>> {
        self.installed
            .lock()
            .expect("MCP probe registry lock poisoned")
            .iter()
            .find(|(_, probe)| predicate(probe))
            .map(|(_, probe)| Arc::clone(probe))
    }
}

/// The deterministic proof seam for the MCP progress pre-subscription
/// race (Issue #205).
///
/// The race it makes observable is the one no capacity bound could ever
/// have been safe for: a server answering a request with genuine progress
/// **before** the dispatching executor has completed its subscription
/// registration. A parked call has crossed its effect frontier and has
/// not subscribed, which is exactly that window, held open deliberately
/// and for as many concurrent calls as a test wants.
///
/// Installation is scoped to one fixture-unique tool name, so a race
/// installed by one test can never park another test's calls even though
/// both run in the same binary.
pub(crate) struct ProgressSubscriptionRace {
    tool: String,
    state: Mutex<ProgressRaceState>,
    parked: tokio::sync::Notify,
    progressed: tokio::sync::Notify,
    answered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

/// One request, qualified by the connection generation that owns it.
///
/// rmcp mints request ids and progress tokens per peer, so two live MCP
/// connections in one test binary share both number spaces. A probe that
/// keyed facts on the bare id would attribute another connection's
/// request to its own — which is a flake, not a contract.
type ProgressRequestKey = (u64, rmcp::model::RequestId);
/// One progress token, qualified the same way.
type ProgressTokenKey = (u64, rmcp::model::ProgressToken);

#[derive(Default)]
struct ProgressRaceState {
    released: bool,
    /// The requests currently parked between their effect frontier and
    /// their progress subscription.
    parked: std::collections::HashSet<ProgressRequestKey>,
    /// Every request this race has ever parked, never cleared, so a
    /// released call's terminal facts are still attributable.
    seen: std::collections::HashSet<ProgressRequestKey>,
    /// What the outbound dispatch seam registered for each request.
    tokens: std::collections::HashMap<ProgressRequestKey, rmcp::model::ProgressToken>,
    /// The tokens whose progress the router observed while no
    /// subscriber existed for them.
    pre_subscription: std::collections::HashSet<ProgressTokenKey>,
    /// The requests whose pre-subscription evidence a subscription
    /// atomically claimed.
    claimed: std::collections::HashSet<ProgressRequestKey>,
    /// The requests the router reached a terminal forget point for.
    forgotten: std::collections::HashSet<ProgressRequestKey>,
    /// The requests the inbound seam observed a correlated answer for —
    /// or that the outbound seam refused, which answers them the same
    /// way.
    answered: std::collections::HashSet<ProgressRequestKey>,
}

static PROGRESS_RACES: ProbeRegistry<ProgressSubscriptionRace> = ProbeRegistry::new();

/// Uninstalls **only** the race its own installation created.
pub(crate) struct ProgressRaceGuard {
    id: u64,
}

impl Drop for ProgressRaceGuard {
    fn drop(&mut self) {
        PROGRESS_RACES.remove(self.id);
    }
}

impl ProgressSubscriptionRace {
    /// Installs a race that parks every dispatch of `tool` between its
    /// effect frontier and its progress subscription.
    pub(crate) fn install(tool: &str) -> (Arc<Self>, ProgressRaceGuard) {
        let race = Arc::new(Self {
            tool: tool.to_owned(),
            state: Mutex::new(ProgressRaceState::default()),
            parked: tokio::sync::Notify::new(),
            progressed: tokio::sync::Notify::new(),
            answered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let id = PROGRESS_RACES.install(Arc::clone(&race));
        (race, ProgressRaceGuard { id })
    }

    /// Resolves once at least `count` calls are parked in the
    /// pre-subscription window at the same time.
    pub(crate) async fn wait_parked(&self, count: usize) {
        loop {
            let parked = self.parked.notified();
            tokio::pin!(parked);
            parked.as_mut().enable();
            if self.state.lock().expect("progress race lock").parked.len() >= count {
                return;
            }
            parked.await;
        }
    }

    /// Resolves once every one of `count` parked requests has had
    /// genuine remote progress delivered for its own token *while it was
    /// still parked* — that is, once progress has provably beaten all
    /// `count` subscription registrations.
    pub(crate) async fn wait_pre_subscription_progress(&self, count: usize) {
        loop {
            let progressed = self.progressed.notified();
            tokio::pin!(progressed);
            progressed.as_mut().enable();
            if self.parked_requests() >= count
                && self.every_parked_request_has_pre_subscription_progress()
            {
                return;
            }
            progressed.await;
        }
    }

    /// Whether every currently parked request has already had genuine
    /// remote progress delivered for its own token — that is, whether
    /// progress provably beat every one of those subscriptions.
    pub(crate) fn every_parked_request_has_pre_subscription_progress(&self) -> bool {
        let state = self.state.lock().expect("progress race lock");
        state.parked.iter().all(|key| {
            state
                .tokens
                .get(key)
                .is_some_and(|token| state.pre_subscription.contains(&(key.0, token.clone())))
        })
    }

    /// Resolves once the inbound seam has observed a correlated answer
    /// for at least `count` of the requests this race parked.
    ///
    /// This is the ordering point the Issue #205 retention regression
    /// needs: it is the exact instant at which the old implementation
    /// deleted a parked call's evidence.
    pub(crate) async fn wait_answered(&self, count: usize) {
        loop {
            let answered = self.answered.notified();
            tokio::pin!(answered);
            answered.as_mut().enable();
            if self.answered_requests() >= count {
                return;
            }
            answered.await;
        }
    }

    /// How many parked requests the peer has answered.
    pub(crate) fn answered_requests(&self) -> usize {
        let state = self.state.lock().expect("progress race lock");
        state
            .parked
            .iter()
            .filter(|key| state.answered.contains(*key))
            .count()
    }

    /// Whether every currently parked request still **owns** the
    /// pre-subscription progress the router observed for it: the
    /// occurrence was recorded, and neither a subscription claimed it
    /// nor a forget point discarded it.
    ///
    /// A parked call cannot subscribe and cannot relinquish, so this is
    /// a settled fact rather than a sampled one.
    pub(crate) fn every_parked_request_retains_its_progress(&self) -> bool {
        let state = self.state.lock().expect("progress race lock");
        !state.parked.is_empty()
            && state.parked.iter().all(|key| {
                state.tokens.get(key).is_some_and(|token| {
                    state.pre_subscription.contains(&(key.0, token.clone()))
                        && !state.claimed.contains(key)
                        && !state.forgotten.contains(key)
                })
            })
    }

    /// Whether every request this race ever parked has claimed its
    /// buffered evidence and then reached the router's terminal forget
    /// point, leaving no progress state behind for it.
    pub(crate) fn every_parked_request_reached_its_forget_point(&self) -> bool {
        let state = self.state.lock().expect("progress race lock");
        !state.seen.is_empty()
            && state
                .seen
                .iter()
                .all(|key| state.claimed.contains(key) && state.forgotten.contains(key))
    }

    /// How many parked requests there are right now.
    pub(crate) fn parked_requests(&self) -> usize {
        self.state.lock().expect("progress race lock").parked.len()
    }

    /// Releases every parked call, and every call that parks later.
    pub(crate) fn release(&self) {
        self.state.lock().expect("progress race lock").released = true;
        self.release.notify_waiters();
    }
}

/// Parks one outbound tool invocation **after** `Transport::send`'s
/// synchronous prologue has taken its dispatch ownership and **before**
/// the inner transport send can be polled (Issue #205).
///
/// That window is exactly "this request has an outbound participant that
/// owns it and has not reached the transport", so a cancellation landing
/// while a call is parked here is provably pre-dispatch — and the
/// settlement of that cancellation is provably unable to complete until
/// the parked participant observes the termination and refuses.
///
/// It also records what each parked participant then *decided*, which is
/// the rendezvous a regression needs: "the outbound participant refused
/// this request" is a fact the test waits for, never one it assumes from
/// having released a pause.
pub(crate) struct OutboundDispatchPause {
    tool: String,
    state: Mutex<OutboundPauseState>,
    entered: tokio::sync::Notify,
    decided: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[derive(Default)]
struct OutboundPauseState {
    released: bool,
    /// The requests whose MCP settlement plane has finished everything
    /// except the local-ownership proof.
    awaiting_local_proof: std::collections::HashSet<rmcp::model::RequestId>,
    /// The requests currently parked at the outbound seam.
    parked: std::collections::HashSet<rmcp::model::RequestId>,
    /// Each parked request's own termination token, captured when it
    /// parked. A cancellation token is **level-triggered**, so awaiting
    /// one is a proof that cannot be missed however the two tasks
    /// interleave — which is what makes "the invocation has terminated"
    /// a fact a test waits for rather than one it assumes from having
    /// advanced a clock.
    terminations:
        std::collections::HashMap<rmcp::model::RequestId, tokio_util::sync::CancellationToken>,
    /// The requests whose outbound participant refused to dispatch.
    refused: std::collections::HashSet<rmcp::model::RequestId>,
    /// The requests whose outbound participant handed the message to the
    /// inner transport.
    dispatched: std::collections::HashSet<rmcp::model::RequestId>,
}

static OUTBOUND_PAUSES: ProbeRegistry<OutboundDispatchPause> = ProbeRegistry::new();

/// Uninstalls **only** the pause its own installation created.
pub(crate) struct OutboundDispatchPauseGuard {
    id: u64,
}

impl Drop for OutboundDispatchPauseGuard {
    fn drop(&mut self) {
        OUTBOUND_PAUSES.remove(self.id);
    }
}

impl OutboundDispatchPause {
    /// Installs a pause that parks every outbound dispatch of `tool`.
    pub(crate) fn install(tool: &str) -> (Arc<Self>, OutboundDispatchPauseGuard) {
        let pause = Arc::new(Self {
            tool: tool.to_owned(),
            state: Mutex::new(OutboundPauseState::default()),
            entered: tokio::sync::Notify::new(),
            decided: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let id = OUTBOUND_PAUSES.install(Arc::clone(&pause));
        (pause, OutboundDispatchPauseGuard { id })
    }

    /// Resolves once at least `count` outbound dispatches of the paused
    /// tool are parked at the seam at the same time.
    pub(crate) async fn wait_parked(&self, count: usize) {
        loop {
            let entered = self.entered.notified();
            tokio::pin!(entered);
            entered.as_mut().enable();
            if self.parked_requests() >= count {
                return;
            }
            entered.await;
        }
    }

    /// How many outbound dispatches are parked at the seam right now.
    pub(crate) fn parked_requests(&self) -> usize {
        self.state
            .lock()
            .expect("outbound dispatch pause lock")
            .parked
            .len()
    }

    /// Resolves once at least `count` parked requests have had their
    /// invocation terminated.
    ///
    /// This is the ordering point a pre-dispatch regression needs before
    /// it releases anything: advancing a clock only *causes* a
    /// cancellation, and the participant must observe a termination that
    /// has already been applied, not one that is on its way.
    pub(crate) async fn wait_terminated(&self, count: usize) {
        loop {
            let tokens: Vec<_> = self
                .state
                .lock()
                .expect("outbound dispatch pause lock")
                .terminations
                .values()
                .cloned()
                .collect();
            if tokens.iter().filter(|token| token.is_cancelled()).count() >= count {
                return;
            }
            let Some(pending) = tokens.into_iter().find(|token| !token.is_cancelled()) else {
                // Fewer parked requests than `count` exist yet; wait for
                // the next one to park.
                let entered = self.entered.notified();
                tokio::pin!(entered);
                entered.as_mut().enable();
                if self.parked_requests() >= count {
                    continue;
                }
                entered.await;
                continue;
            };
            pending.cancelled().await;
        }
    }

    /// Resolves once at least `count` of this pause's requests have
    /// reached the point where the MCP settlement plane has nothing left
    /// to do but await the local-ownership proof.
    ///
    /// This is the discriminating ordering point of the whole
    /// regression. Everything before it — the best-effort
    /// `notifications/cancelled` send and the response-channel
    /// arbitration — involves real network work whose duration bounds
    /// nothing; everything after it is a single `await` on the release
    /// proof and the construction of a result. So a settlement that is
    /// still pending *after* this fact is pending because a local
    /// participant has not become terminal, and an implementation that
    /// treated the pre-dispatch phase as "nothing pending" settles
    /// within a poll or two of it.
    pub(crate) async fn wait_awaiting_local_proof(&self, count: usize) {
        loop {
            let decided = self.decided.notified();
            tokio::pin!(decided);
            decided.as_mut().enable();
            if self.requests_awaiting_local_proof() >= count {
                return;
            }
            decided.await;
        }
    }

    /// How many of this pause's requests are awaiting only the
    /// local-ownership proof.
    pub(crate) fn requests_awaiting_local_proof(&self) -> usize {
        self.state
            .lock()
            .expect("outbound dispatch pause lock")
            .awaiting_local_proof
            .len()
    }

    /// Resolves once at least `count` outbound participants have made
    /// their dispatch decision — refused, or handed the message on.
    ///
    /// This is the rendezvous that makes a pre-dispatch regression a
    /// proof: releasing a pause only lets the participant run, and a
    /// test that asserts immediately afterwards is asserting about a
    /// continuation that may not have been scheduled yet.
    pub(crate) async fn wait_decided(&self, count: usize) {
        loop {
            let decided = self.decided.notified();
            tokio::pin!(decided);
            decided.as_mut().enable();
            if self.decisions() >= count {
                return;
            }
            decided.await;
        }
    }

    /// How many outbound participants have decided.
    pub(crate) fn decisions(&self) -> usize {
        let state = self.state.lock().expect("outbound dispatch pause lock");
        state.refused.len() + state.dispatched.len()
    }

    /// How many outbound participants refused to dispatch their request.
    pub(crate) fn refusals(&self) -> usize {
        self.state
            .lock()
            .expect("outbound dispatch pause lock")
            .refused
            .len()
    }

    /// How many outbound participants handed their request to the inner
    /// transport.
    pub(crate) fn dispatches(&self) -> usize {
        self.state
            .lock()
            .expect("outbound dispatch pause lock")
            .dispatched
            .len()
    }

    /// Releases every parked dispatch.
    pub(crate) fn release(&self) {
        self.state
            .lock()
            .expect("outbound dispatch pause lock")
            .released = true;
        self.release.notify_waiters();
    }
}

/// Parks one owned outbound tool invocation, when a pause is installed
/// for its tool.
pub(crate) async fn park_before_outbound_dispatch(
    tool: &str,
    id: &rmcp::model::RequestId,
    terminate: &tokio_util::sync::CancellationToken,
) {
    let Some(pause) = OUTBOUND_PAUSES.find(|pause| pause.tool == tool) else {
        return;
    };
    {
        let mut state = pause.state.lock().expect("outbound dispatch pause lock");
        if state.released {
            return;
        }
        state.parked.insert(id.clone());
        state.terminations.insert(id.clone(), terminate.clone());
    }
    pause.entered.notify_waiters();
    loop {
        let released = pause.release.notified();
        tokio::pin!(released);
        released.as_mut().enable();
        if pause
            .state
            .lock()
            .expect("outbound dispatch pause lock")
            .released
        {
            break;
        }
        released.await;
    }
    pause
        .state
        .lock()
        .expect("outbound dispatch pause lock")
        .parked
        .remove(id);
}

/// Records that one request's MCP settlement plane has completed
/// everything except awaiting the local-ownership proof.
///
/// Offered to whichever installed pause already knows this request — the
/// settlement plane has no tool name at that point, and a pause records
/// the termination token of every request it parked.
pub(crate) fn note_awaiting_local_proof(id: &rmcp::model::RequestId) {
    for pause in OUTBOUND_PAUSES.all() {
        let known = {
            let mut state = pause.state.lock().expect("outbound dispatch pause lock");
            if state.terminations.contains_key(id) {
                state.awaiting_local_proof.insert(id.clone());
                true
            } else {
                false
            }
        };
        if known {
            pause.decided.notify_waiters();
        }
    }
}

/// Records what one outbound participant decided at the seam.
pub(crate) fn note_outbound_dispatch(tool: &str, id: &rmcp::model::RequestId, refused: bool) {
    let Some(pause) = OUTBOUND_PAUSES.find(|pause| pause.tool == tool) else {
        return;
    };
    {
        let mut state = pause.state.lock().expect("outbound dispatch pause lock");
        if refused {
            state.refused.insert(id.clone());
        } else {
            state.dispatched.insert(id.clone());
        }
    }
    pause.decided.notify_waiters();
}

/// Records what the outbound dispatch seam registered for one request.
///
/// Every router fact is qualified by the identity of the connection
/// generation's router that produced it, so a race installed by one test
/// never attributes another live connection's identically numbered
/// request to a call of its own.
pub(crate) fn note_progress_admission(
    scope: u64,
    id: &rmcp::model::RequestId,
    token: &rmcp::model::ProgressToken,
) {
    for race in PROGRESS_RACES.all() {
        race.state
            .lock()
            .expect("progress race lock")
            .tokens
            .insert((scope, id.clone()), token.clone());
    }
}

/// Records that the router observed progress for a token whose
/// dispatching call had not subscribed yet.
pub(crate) fn note_pre_subscription_progress(scope: u64, token: &rmcp::model::ProgressToken) {
    for race in PROGRESS_RACES.all() {
        race.state
            .lock()
            .expect("progress race lock")
            .pre_subscription
            .insert((scope, token.clone()));
        race.progressed.notify_waiters();
    }
}

/// Records that one subscription atomically claimed the
/// pre-subscription evidence the router held for its request.
pub(crate) fn note_progress_claimed(scope: u64, id: &rmcp::model::RequestId) {
    for race in PROGRESS_RACES.all() {
        race.state
            .lock()
            .expect("progress race lock")
            .claimed
            .insert((scope, id.clone()));
    }
}

/// Records that one request reached the router's terminal forget point:
/// both its remote answer and its caller's progress ownership are over.
pub(crate) fn note_progress_forgotten(scope: u64, id: &rmcp::model::RequestId) {
    for race in PROGRESS_RACES.all() {
        race.state
            .lock()
            .expect("progress race lock")
            .forgotten
            .insert((scope, id.clone()));
    }
}

/// Records that no correlated answer can arrive for one request any
/// more, which is what the inbound seam observes on a response and the
/// outbound seam records on a refusal.
pub(crate) fn note_remote_terminal(scope: u64, id: &rmcp::model::RequestId) {
    for race in PROGRESS_RACES.all() {
        race.state
            .lock()
            .expect("progress race lock")
            .answered
            .insert((scope, id.clone()));
        race.answered.notify_waiters();
    }
}

/// Freeze the completing poll before arbitration until its correlated
/// response is queued. This test-only scheduling hook does not consume or
/// classify the response; the existing Issue #205 select remains authoritative.
pub(crate) async fn park_task_arbitration(
    server: &str,
    handle: &rmcp::service::RequestHandle<rmcp::RoleClient>,
) {
    const METHOD: &str = "tasks/get:correlated";
    let Some(barrier) = TASK_REQUEST_BARRIERS.find(|b| b.server == server && b.method == METHOD)
    else {
        return;
    };
    park_before_task_request(server, METHOD, "in-flight").await;
    if barrier.arrivals().len() >= barrier.first_held {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while handle.rx.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the gated poll queues its correlated response");
    }
}

/// The deterministic MRTR continuation-dispatch barrier (Issue #242).
///
/// It holds one invocation at the exact point between "the typed human
/// response has been accepted and mapped to `inputResponses`" and "the
/// continuation dispatch frontier". A regression parks there, decides the
/// cancellation race deliberately, and then asserts on the number of
/// physical `tools/call` rounds the server actually saw — no sleeps, and
/// no reliance on scheduler timing.
pub(crate) struct MrtrContinuationBarrier {
    tool: String,
    state: Mutex<MrtrBarrierState>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[derive(Default)]
struct MrtrBarrierState {
    released: bool,
    /// How many continuations have reached the barrier, and how many
    /// input requests each of them answered.
    arrivals: Vec<usize>,
}

static MRTR_BARRIERS: ProbeRegistry<MrtrContinuationBarrier> = ProbeRegistry::new();

/// Uninstalls **only** the barrier its own installation created.
pub(crate) struct MrtrBarrierGuard {
    id: u64,
}

impl Drop for MrtrBarrierGuard {
    fn drop(&mut self) {
        MRTR_BARRIERS.remove(self.id);
    }
}

impl MrtrContinuationBarrier {
    /// Installs a barrier that parks every MRTR continuation of `tool`.
    pub(crate) fn install(tool: &str) -> (Arc<Self>, MrtrBarrierGuard) {
        let barrier = Arc::new(Self {
            tool: tool.to_owned(),
            state: Mutex::new(MrtrBarrierState::default()),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let id = MRTR_BARRIERS.install(Arc::clone(&barrier));
        (barrier, MrtrBarrierGuard { id })
    }

    /// Resolves once at least `count` continuations have arrived.
    pub(crate) async fn wait_arrived(&self, count: usize) {
        loop {
            let entered = self.entered.notified();
            tokio::pin!(entered);
            entered.as_mut().enable();
            if self.arrivals() >= count {
                return;
            }
            entered.await;
        }
    }

    /// How many continuations have reached the barrier.
    pub(crate) fn arrivals(&self) -> usize {
        self.state.lock().expect("MRTR barrier lock").arrivals.len()
    }

    /// How many input requests each arrived continuation answered.
    pub(crate) fn answered_requests(&self) -> Vec<usize> {
        self.state
            .lock()
            .expect("MRTR barrier lock")
            .arrivals
            .clone()
    }

    /// Releases every parked continuation.
    pub(crate) fn release(&self) {
        self.state.lock().expect("MRTR barrier lock").released = true;
        self.release.notify_waiters();
    }
}

/// Parks one MRTR continuation before its dispatch frontier, when a
/// barrier is installed for its tool.
pub(crate) async fn park_before_mrtr_continuation(tool: &str, answered_requests: usize) {
    let Some(barrier) = MRTR_BARRIERS.find(|barrier| barrier.tool == tool) else {
        return;
    };
    {
        let mut state = barrier.state.lock().expect("MRTR barrier lock");
        state.arrivals.push(answered_requests);
        if state.released {
            drop(state);
            barrier.entered.notify_waiters();
            return;
        }
    }
    barrier.entered.notify_waiters();
    loop {
        let released = barrier.release.notified();
        tokio::pin!(released);
        released.as_mut().enable();
        if barrier.state.lock().expect("MRTR barrier lock").released {
            return;
        }
        released.await;
    }
}

/// The deterministic linearization point of every SEP-2663 task-request
/// dispatch frontier (Issue #243).
///
/// A barrier is installed for one `(server, method)` pair and parks the
/// invocation immediately **before** that request is dispatched:
///
/// - `tasks/get` — after the poll wait, before the poll exists;
/// - `tasks/update` — after the typed human response has been accepted,
///   mapped to `inputResponses`, and recorded as answered, and before the
///   update exists;
/// - `tasks/cancel` — before the cooperative cancellation exists.
///
/// A regression parks there, decides the cancellation race deliberately,
/// and then asserts on what the server actually saw — no sleeps, and no
/// reliance on scheduler timing.
pub(crate) struct TaskRequestBarrier {
    server: String,
    method: &'static str,
    /// The 1-based arrival from which requests are held. Earlier
    /// arrivals are counted and pass straight through, so a regression
    /// can let a known number of requests complete and then hold exactly
    /// the next one at its frontier.
    first_held: usize,
    state: Mutex<TaskUpdateBarrierState>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[derive(Default)]
struct TaskUpdateBarrierState {
    released: bool,
    /// The task id of every request that reached the barrier.
    arrivals: Vec<String>,
}

static TASK_REQUEST_BARRIERS: ProbeRegistry<TaskRequestBarrier> = ProbeRegistry::new();

/// Uninstalls **only** the barrier its own installation created.
pub(crate) struct TaskRequestBarrierGuard {
    id: u64,
}

impl Drop for TaskRequestBarrierGuard {
    fn drop(&mut self) {
        TASK_REQUEST_BARRIERS.remove(self.id);
    }
}

impl TaskRequestBarrier {
    /// Installs a barrier that parks every `method` request of `server`.
    pub(crate) fn install(
        server: &str,
        method: &'static str,
    ) -> (Arc<Self>, TaskRequestBarrierGuard) {
        Self::install_from(server, method, 1)
    }

    /// Installs a barrier that lets the first `first_held - 1` requests
    /// through and holds every later one at its dispatch frontier.
    pub(crate) fn install_from(
        server: &str,
        method: &'static str,
        first_held: usize,
    ) -> (Arc<Self>, TaskRequestBarrierGuard) {
        let barrier = Arc::new(Self {
            server: server.to_owned(),
            method,
            first_held,
            state: Mutex::new(TaskUpdateBarrierState::default()),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let id = TASK_REQUEST_BARRIERS.install(Arc::clone(&barrier));
        (barrier, TaskRequestBarrierGuard { id })
    }

    /// Resolves once at least `count` requests have arrived.
    pub(crate) async fn wait_arrived(&self, count: usize) {
        loop {
            let entered = self.entered.notified();
            tokio::pin!(entered);
            entered.as_mut().enable();
            if self.arrivals().len() >= count {
                return;
            }
            entered.await;
        }
    }

    /// The task id of every request that reached the barrier.
    pub(crate) fn arrivals(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("task update barrier lock")
            .arrivals
            .clone()
    }

    /// Releases every parked request.
    pub(crate) fn release(&self) {
        self.state
            .lock()
            .expect("task update barrier lock")
            .released = true;
        self.release.notify_waiters();
    }
}

/// Parks one SEP-2663 task request before its dispatch frontier, when a
/// barrier is installed for its server and method.
pub(crate) async fn park_before_task_request(server: &str, method: &str, task_id: &str) {
    let Some(barrier) =
        TASK_REQUEST_BARRIERS.find(|barrier| barrier.server == server && barrier.method == method)
    else {
        return;
    };
    {
        let mut state = barrier.state.lock().expect("task update barrier lock");
        state.arrivals.push(task_id.to_owned());
        let held = state.arrivals.len() >= barrier.first_held;
        if state.released || !held {
            drop(state);
            barrier.entered.notify_waiters();
            return;
        }
    }
    barrier.entered.notify_waiters();
    loop {
        let released = barrier.release.notified();
        tokio::pin!(released);
        released.as_mut().enable();
        if barrier
            .state
            .lock()
            .expect("task update barrier lock")
            .released
        {
            return;
        }
        released.await;
    }
}

/// Parks one dispatched call between its effect frontier and its
/// progress subscription, when a race is installed for its tool.
pub(crate) async fn park_before_progress_subscription(
    tool: &str,
    id: &rmcp::model::RequestId,
    scope: u64,
) {
    let Some(race) = PROGRESS_RACES.find(|race| race.tool == tool) else {
        return;
    };
    let key = (scope, id.clone());
    {
        let mut state = race.state.lock().expect("progress race lock");
        if state.released {
            return;
        }
        state.parked.insert(key.clone());
        state.seen.insert(key.clone());
    }
    race.parked.notify_waiters();
    loop {
        let released = race.release.notified();
        tokio::pin!(released);
        released.as_mut().enable();
        if race.state.lock().expect("progress race lock").released {
            break;
        }
        released.await;
    }
    race.state
        .lock()
        .expect("progress race lock")
        .parked
        .remove(&key);
}
