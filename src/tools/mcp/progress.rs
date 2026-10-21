//! Remote progress state, routing, subscriptions, and caller leases
//! (Issue #205).
//!
//! The router keeps the caller, remote, and outbound-admission dimensions of
//! a dispatched request's progress separate, retains payload-free
//! tombstones only while a not-yet-consumed admission could resurrect caller
//! ownership, and forgets a request at its own terminal forget point.

use std::sync::{Arc, Mutex};

use rmcp::model::ProgressNotificationParam;

use crate::tools::executor::ToolExecutionContext;
use crate::tools::limits::bound_tool_progress;

#[cfg(test)]
use super::test_sync;

/// The delivery capacity of one subscribed token.
///
/// Bounding a subscriber's queue can never erase a liveness *occurrence*: a
/// full queue is a queue whose subscriber already holds this many
/// undelivered proofs that the remote is alive.
const PROGRESS_SUBSCRIPTION_CAPACITY: usize = 16;

/// Routes remote progress notifications to the in-flight `tools/call` that
/// owns their token (Issue #205).
///
/// # Two classes of progress token, and only one of them may be discarded
///
/// MCP progress is not telemetry. It is the idle-liveness evidence the
/// generic Agent Loop watchdog consumes, so discarding it can turn genuine
/// remote progress into a *false* idle timeout. But the set of tokens that
/// can appear on the wire is peer-controlled, so something must be bounded.
/// The router therefore separates the two completely:
///
/// ```text
/// known live request token                 unknown / unsolicited token
///   minted by rmcp for a rustX request       any other progress token a
///   and registered by the outbound           peer chooses to emit
///   dispatch seam before the request
///   can reach the server
///
///   O(1) state per token                     no per-token state at all
///   never evicted by capacity                counted, then dropped
///   removed at the request's own
///   terminal forget point
/// ```
///
/// Unsolicited progress occupies **no storage**, so peer-controlled traffic
/// has no capacity to consume and therefore nothing of a live request's to
/// displace. There is no shared queue and no eviction policy between the two
/// classes, because they do not share a container.
///
/// # How a token becomes known, and why there is no unowned window
///
/// rmcp mints a request's progress token *inside*
/// `Peer::send_cancellable_request`, so the dispatching call cannot know it
/// beforehand and can only subscribe after the request was enqueued. A
/// server that answers with a progress notification immediately therefore
/// races that subscription. Bounding a speculative pre-subscription cache
/// cannot fix that: nothing bounds how many admitted requests are inside
/// that window at once, so any capacity there can evict a legitimate live
/// request's only liveness evidence.
///
/// The window is closed by ownership instead of by capacity.
/// [`dispatch::McpDispatchSeam`] is the transport rustX hands to rmcp, and
/// it registers a request's `(RequestId, ProgressToken)` pair
/// **synchronously inside `Transport::send`**, before the message is handed
/// to the wire. That registration therefore *happens-before* the server can
/// receive the request, which happens-before the server can emit any
/// progress for its token, which happens-before that notification can reach
/// this router. The linearization point is causal, not a timing hope:
///
/// ```text
/// send_cancellable_request -> Ok        the request exists
///   executor -> lease(id, token)           <-- caller ownership begins here
///   Transport::send  -> admit(id, token)   <-- the token becomes known here
///     bytes on the wire
///       server receives the request
///         server emits progress for the token
///           router.deliver(..)                <-- always after admit
/// executor        -> lease.subscribe()        <-- may be anywhere after Ok
/// ```
///
/// `subscribe` may run before or after `admit`; both create the same entry,
/// so neither order can lose evidence.
///
/// # Two ownership dimensions, and why neither stands in for the other
///
/// A dispatched request's progress state answers to two facts that are
/// ordered by nothing:
///
/// ```text
/// remote ownership                    caller ownership
///   Open      a correlated response     AwaitingSubscription  the call owns
///             for this request may                            progress and
///             still arrive                                    has not
///                                                             claimed
///   Terminal  the peer answered it,                           delivery yet
///             or rustX refused to       Subscribed            the call owns
///             dispatch it, so no                              delivery
///             correlated response                             directly
///             can arrive any more       Relinquished          the call is
///                                                             gone
/// ```
///
/// > **A correlated response ends remote execution. It does not end the
/// > dispatching caller's progress-consumer ownership.**
///
/// That is the Issue #205 review finding stated as a rule. The peer may
/// answer while the executor is still between
/// `send_cancellable_request -> Ok` and its subscription, and treating that
/// response as proof that no caller remains deleted exactly the evidence
/// this router exists to keep: genuine progress observed for a live
/// `ToolCall`, discarded before that call could claim it, after which the
/// biased response arm reported a terminal result with the liveness
/// occurrence silently gone.
///
/// Admission consumption is a third, independent fact: `admit` consumes
/// the once-per-request outbound seam even when the caller is already gone.
/// The forget rule is:
///
/// ```text
/// caller                 admission   remote       action
/// AwaitingSubscription   either      either       retain evidence
/// Subscribed             either      either       retain delivery
/// Relinquished           pending     Open         payload-free tombstone
/// Relinquished           pending     Terminal     forget
/// Relinquished           consumed    either       forget
/// ```
///
/// > **A relinquished progress record survives only while needed to prevent
/// > a not-yet-consumed outbound admission from resurrecting caller ownership.**
///
/// `Transport::send` can run after the executor returned. Relinquishment
/// before that seam leaves a payload-free tombstone with no delivery index.
/// Late admission consumes that tombstone without creating caller ownership.
/// Once admission has happened, relinquishment forgets immediately, even if
/// the server never answers. Remote execution uncertainty belongs to the
/// canonical `OutcomeUnknown` Tool result, not to progress-router ownership.
/// Remote terminality also makes a pre-admission tombstone unnecessary.
///
/// # Who owns each transition
///
/// ```text
/// admit(id, token)             the outbound dispatch seam, in
///                              Transport::send's synchronous prologue
/// deliver(notification)        the inbound progress notification
/// lease(id, token).subscribe() the dispatching call, claiming buffered
///                              evidence atomically
/// drop(McpProgressLease)       the dispatching call relinquishing progress
///                              ownership, on *every* exit path
/// mark_remote_terminal(id)     the inbound seam on a correlated response,
///                              and the outbound seam on a refused dispatch
/// ```
///
/// [`McpProgressLease`] is the whole caller side: it is taken the instant
/// the request id exists and dropped when the executor's call frame ends,
/// so cancellation, a deadline, transport loss, protocol corruption, a send
/// failure and ordinary completion all relinquish through the same RAII
/// owner rather than through per-branch cleanup.
///
/// # The invariant
///
/// > Every admitted MCP request that rustX can identify as belonging to a
/// > live local `ToolCall` retains at least one liveness occurrence once
/// > genuine remote progress for that request has been observed, until that
/// > `ToolCall` consumes or relinquishes that liveness state.
///
/// Payload detail is explicitly *not* guaranteed, and two places coalesce:
///
/// - an admitted token with no subscriber yet keeps **one** entry: the
///   latest payload plus an occurrence count. Repeated notifications
///   collapse onto it, so the state is O(1) per live request and no
///   occurrence is lost;
/// - a subscribed token's delivery queue is bounded by
///   [`PROGRESS_SUBSCRIPTION_CAPACITY`] and drops on full, which cannot
///   erase an occurrence because the queue is full precisely when that many
///   undelivered proofs are already in the subscriber's hands.
///
/// Nothing is ever fabricated: the router only reorders and coalesces
/// delivery of notifications the peer genuinely sent.
///
/// # Boundedness
///
/// State is O(live callers + unresolved outbound admissions), with one
/// coalesced payload or bounded delivery queue per live caller and one
/// payload-free tombstone per relinquished request still awaiting admission.
/// Unknown progress creates no per-token state. Repeated admitted calls whose
/// servers never answer leave no history after caller relinquishment. The
/// whole router belongs to one connection generation and dies with it.
#[cfg_attr(not(test), derive(Default))]
pub(super) struct McpProgressRouter {
    pub(super) state: Mutex<ProgressRouterState>,
    /// Identifies this connection generation's router to test probes.
    ///
    /// rmcp mints request ids and progress tokens per peer, so two live
    /// connections in one test binary share both number spaces; a probe that
    /// keyed facts on the bare id would attribute one connection's request
    /// to another's call.
    #[cfg(test)]
    scope: u64,
}

#[cfg(test)]
impl Default for McpProgressRouter {
    fn default() -> Self {
        static NEXT_ROUTER_SCOPE: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(1);
        Self {
            state: Mutex::default(),
            scope: NEXT_ROUTER_SCOPE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
pub(super) struct ProgressRouterState {
    /// One entry per request that has been admitted or leased and has not
    /// reached its terminal forget point. **No capacity eviction.**
    pub(super) requests: std::collections::HashMap<rmcp::model::RequestId, RequestProgress>,
    /// The delivery index. A token appears here exactly while some caller
    /// can still consume progress for it, which is what makes a relinquished
    /// request's later notifications unsolicited rather than stored.
    owners: std::collections::HashMap<rmcp::model::ProgressToken, rmcp::model::RequestId>,
    /// Unsolicited peer progress: counted, never stored.
    unsolicited: UnsolicitedProgress,
}

/// What the router retains about progress for tokens no rustX caller owns.
///
/// Deliberately O(1) and payload-free. An unknown token is peer-controlled,
/// so it gets a counter and the most recent token value for diagnostics —
/// never storage a flood could grow and never storage a live request's
/// evidence has to compete for.
#[derive(Default)]
struct UnsolicitedProgress {
    dropped: u64,
    last: Option<rmcp::model::ProgressToken>,
}

/// Caller, remote, and outbound-admission state of one request.
pub(super) struct RequestProgress {
    /// The token rmcp minted for this request.
    token: rmcp::model::ProgressToken,
    caller: CallerOwnership,
    pub(super) remote: RemoteOwnership,
    /// Whether the once-per-request outbound admission has reached the router.
    /// A relinquished request only needs a tombstone while this is false.
    pub(super) admission_consumed: bool,
}

/// Where the dispatching call's progress-consumer ownership stands.
enum CallerOwnership {
    /// The dispatching call owns this request's progress and has not claimed
    /// delivery yet. O(1) coalesced evidence waits here for it — including
    /// across a correlated response, which says nothing about this
    /// dimension.
    AwaitingSubscription(PendingProgress),
    /// The dispatching call owns delivery directly.
    Subscribed(ProgressSubscriber),
    /// The dispatching call is gone. Nothing is deliverable any more, and
    /// the record exists only while a pending outbound admission could
    /// otherwise resurrect an owner.
    Relinquished,
}

/// Whether a correlated response for this request can still arrive.
#[derive(PartialEq, Eq)]
pub(super) enum RemoteOwnership {
    /// The peer has neither answered the request nor been refused it.
    Open,
    /// The peer answered this request, or rustX refused to dispatch it. No
    /// correlated response can ever arrive for it again.
    Terminal,
}

type ProgressSubscriber = tokio::sync::mpsc::Sender<ProgressNotificationParam>;

/// The coalesced pre-subscription evidence of one known live token.
#[derive(Default)]
struct PendingProgress {
    /// The most recent payload the peer sent for this token, when any.
    latest: Option<ProgressNotificationParam>,
    /// How many notifications this entry represents. Only its non-zeroness
    /// is load-bearing — it is the liveness occurrence — but retaining the
    /// count keeps the coalescing honest in diagnostics.
    occurrences: u32,
}

impl McpProgressRouter {
    /// Registers one dispatched request's progress token as **known and
    /// live**, before the request can reach the server.
    ///
    /// Called from [`dispatch::McpDispatchSeam`] inside `Transport::send`.
    /// While the caller remains live, the token is known and can never be
    /// evicted by peer-controlled traffic.
    ///
    /// Admission consumes the outbound seam without replacing live evidence.
    /// A pending relinquished tombstone is consumed and forgotten. The
    /// call's own lease may already have subscribed, and — because this runs
    /// in rmcp's service loop rather than in the caller's task — it may
    /// already have relinquished; in the second case re-creating an owned
    /// entry would leave state with no caller left to forget it.
    pub(super) fn admit(&self, id: &rmcp::model::RequestId, token: &rmcp::model::ProgressToken) {
        let mut state = self
            .state
            .lock()
            .expect("MCP progress router lock poisoned");
        if let Some(entry) = state.requests.get_mut(id) {
            entry.admission_consumed = true;
            if matches!(entry.caller, CallerOwnership::Relinquished) {
                state.requests.remove(id);
                #[cfg(test)]
                test_sync::note_progress_forgotten(self.scope, id);
            }
        } else {
            state.requests.insert(
                id.clone(),
                RequestProgress {
                    token: token.clone(),
                    caller: CallerOwnership::AwaitingSubscription(PendingProgress::default()),
                    remote: RemoteOwnership::Open,
                    admission_consumed: true,
                },
            );
            state.owners.insert(token.clone(), id.clone());
        }
        #[cfg(test)]
        test_sync::note_progress_admission(self.scope, id, token);
    }

    /// Delivers one remote progress notification.
    pub(super) fn deliver(&self, notification: ProgressNotificationParam) {
        let mut state = self
            .state
            .lock()
            .expect("MCP progress router lock poisoned");
        let token = notification.progress_token.clone();
        // No caller owns this token: either the peer invented it, or the
        // request that owned it has relinquished. Either way no `ToolCall`
        // will ever consume it, so it gets a counter, not storage.
        let owner = state.owners.get(&token).cloned();
        let live = owner.and_then(|id| state.requests.get_mut(&id));
        let Some(live) = live else {
            state.unsolicited.dropped = state.unsolicited.dropped.saturating_add(1);
            state.unsolicited.last = Some(token);
            return;
        };
        match &mut live.caller {
            CallerOwnership::Subscribed(sender) => {
                // Bounded, drop-on-full. This cannot erase a liveness
                // occurrence: a full queue is a queue whose subscriber
                // already holds `PROGRESS_SUBSCRIPTION_CAPACITY` undelivered
                // proofs that the remote is alive.
                let _ = sender.try_send(notification);
            }
            CallerOwnership::AwaitingSubscription(pending) => {
                pending.latest = Some(notification);
                pending.occurrences = pending.occurrences.saturating_add(1);
                #[cfg(test)]
                test_sync::note_pre_subscription_progress(self.scope, &token);
            }
            // Unreachable: relinquishing removes the delivery index, so no
            // owner is found for such a token above. Counted rather than
            // stored, which is the fail-safe direction.
            CallerOwnership::Relinquished => {
                state.unsolicited.dropped = state.unsolicited.dropped.saturating_add(1);
                state.unsolicited.last = Some(token);
            }
        }
    }

    /// Takes the dispatching call's progress-consumer ownership of one
    /// request.
    ///
    /// The lease owns the caller dimension above, and it is
    /// deliberately taken *before* the call can park, fail or be cancelled:
    /// dropping it is the single relinquish point every exit path shares.
    pub(super) fn lease(
        self: &Arc<Self>,
        id: rmcp::model::RequestId,
        token: rmcp::model::ProgressToken,
    ) -> McpProgressLease {
        McpProgressLease {
            router: Arc::clone(self),
            id,
            token,
        }
    }

    /// Claims delivery of one leased request's progress, taking anything
    /// that arrived before this call with it.
    ///
    /// A request the peer has already answered may still be subscribed to:
    /// the response ends remote execution, and this call is still alive
    /// until it has processed that response and settled.
    fn subscribe(
        &self,
        id: &rmcp::model::RequestId,
        token: &rmcp::model::ProgressToken,
    ) -> McpProgressSubscription {
        let (sender, receiver) = tokio::sync::mpsc::channel(PROGRESS_SUBSCRIPTION_CAPACITY);
        let claim = sender.clone();
        {
            let mut state = self
                .state
                .lock()
                .expect("MCP progress router lock poisoned");
            let claimed = match state.requests.get_mut(id) {
                // Unreachable while a lease exists, and fail-closed if it
                // ever becomes reachable: a relinquished request is not
                // resurrected by a subscription.
                Some(entry) if matches!(entry.caller, CallerOwnership::Relinquished) => None,
                Some(entry) => {
                    match std::mem::replace(&mut entry.caller, CallerOwnership::Subscribed(sender))
                    {
                        // Whatever the outbound seam admitted, or an earlier
                        // delivery coalesced, belongs to this call —
                        // including across a correlated response.
                        CallerOwnership::AwaitingSubscription(pending) => pending.latest,
                        // A token is minted once per request, so this is
                        // unreachable in practice; taking the newest
                        // subscriber is the fail-safe direction because the
                        // dispatching call is the only consumer that can
                        // report the evidence.
                        CallerOwnership::Subscribed(_) | CallerOwnership::Relinquished => None,
                    }
                }
                // The subscription beat the outbound seam's admission. Both
                // orders create the same entry.
                None => {
                    state.requests.insert(
                        id.clone(),
                        RequestProgress {
                            token: token.clone(),
                            caller: CallerOwnership::Subscribed(sender),
                            remote: RemoteOwnership::Open,
                            admission_consumed: false,
                        },
                    );
                    state.owners.insert(token.clone(), id.clone());
                    None
                }
            };
            if let Some(latest) = claimed {
                // The coalesced payload carries the liveness occurrence the
                // pre-subscription state preserved.
                let _ = claim.try_send(latest);
                #[cfg(test)]
                test_sync::note_progress_claimed(self.scope, id);
            }
        }
        McpProgressSubscription { receiver }
    }

    /// The dispatching call relinquishes its progress-consumer ownership of
    /// one request. Called exactly once, by [`McpProgressLease`]'s `Drop`.
    ///
    /// Nothing is deliverable to a caller that no longer exists, so the
    /// payload and the delivery index go immediately. Only a pending
    /// admission with an open remote side still needs a tombstone.
    fn relinquish(&self, id: &rmcp::model::RequestId, token: &rmcp::model::ProgressToken) {
        let mut state = self
            .state
            .lock()
            .expect("MCP progress router lock poisoned");
        state.owners.remove(token);
        let forget = if let Some(entry) = state.requests.get_mut(id) {
            entry.caller = CallerOwnership::Relinquished;
            entry.admission_consumed || entry.remote == RemoteOwnership::Terminal
        } else {
            // The relinquish beat the outbound seam's admission, which can
            // still run: the record is what stops that admission from
            // creating an owned entry nothing would forget.
            state.requests.insert(
                id.clone(),
                RequestProgress {
                    token: token.clone(),
                    caller: CallerOwnership::Relinquished,
                    remote: RemoteOwnership::Open,
                    admission_consumed: false,
                },
            );
            false
        };
        if forget {
            state.requests.remove(id);
            #[cfg(test)]
            test_sync::note_progress_forgotten(self.scope, id);
        }
    }

    /// Records that no correlated response for this request can arrive any
    /// more: the peer answered it, or rustX refused to dispatch it.
    ///
    /// This is **not** evidence that the dispatching call is gone, and it
    /// therefore never removes evidence a live call has not claimed yet. It
    /// completes the remote dimension, and forgets the request only when the
    /// caller dimension is already terminal.
    pub(super) fn mark_remote_terminal(&self, id: &rmcp::model::RequestId) {
        let mut state = self
            .state
            .lock()
            .expect("MCP progress router lock poisoned");
        let Some(entry) = state.requests.get_mut(id) else {
            return;
        };
        entry.remote = RemoteOwnership::Terminal;
        let token = entry.token.clone();
        let forget = matches!(entry.caller, CallerOwnership::Relinquished);
        if forget {
            state.requests.remove(id);
            state.owners.remove(&token);
            #[cfg(test)]
            test_sync::note_progress_forgotten(self.scope, id);
        }
        #[cfg(test)]
        test_sync::note_remote_terminal(self.scope, id);
    }

    /// This router's probe identity.
    #[cfg(test)]
    pub(super) const fn scope(&self) -> u64 {
        self.scope
    }

    /// How many live requests and pre-admission tombstones this router tracks.
    #[cfg(test)]
    pub(super) fn tracked_requests(&self) -> usize {
        self.state
            .lock()
            .expect("MCP progress router lock poisoned")
            .requests
            .len()
    }

    /// How many tokens can still be delivered to a caller.
    #[cfg(test)]
    pub(super) fn deliverable_tokens(&self) -> usize {
        self.state
            .lock()
            .expect("MCP progress router lock poisoned")
            .owners
            .len()
    }

    /// The coalesced pre-subscription occurrence count of one request, when
    /// it is still waiting for its call to claim delivery.
    #[cfg(test)]
    pub(super) fn pending_occurrences(&self, id: &rmcp::model::RequestId) -> Option<u32> {
        match &self
            .state
            .lock()
            .expect("MCP progress router lock poisoned")
            .requests
            .get(id)?
            .caller
        {
            CallerOwnership::AwaitingSubscription(pending) => Some(pending.occurrences),
            CallerOwnership::Subscribed(_) | CallerOwnership::Relinquished => None,
        }
    }

    /// Whether one request's call currently owns delivery.
    #[cfg(test)]
    fn is_subscribed(&self, id: &rmcp::model::RequestId) -> bool {
        self.state
            .lock()
            .expect("MCP progress router lock poisoned")
            .requests
            .get(id)
            .is_some_and(|entry| matches!(entry.caller, CallerOwnership::Subscribed(_)))
    }

    /// How many unsolicited progress notifications were counted and dropped.
    #[cfg(test)]
    fn unsolicited_dropped(&self) -> u64 {
        self.state
            .lock()
            .expect("MCP progress router lock poisoned")
            .unsolicited
            .dropped
    }
}

/// One dispatching call's progress-consumer ownership of its own request.
///
/// It is taken as soon as the request id exists and dropped when the call's
/// frame ends, whichever way it ends — a correlated response, cancellation,
/// a deadline, transport loss, protocol corruption, a refused dispatch, or a
/// panic. That is why no exit branch carries its own progress cleanup: the
/// lease *is* the caller dimension of the ownership model, and dropping it
/// is the only way to relinquish.
pub(super) struct McpProgressLease {
    router: Arc<McpProgressRouter>,
    id: rmcp::model::RequestId,
    token: rmcp::model::ProgressToken,
}

impl McpProgressLease {
    /// Claims delivery of this request's progress, taking every occurrence
    /// observed before this moment with it.
    pub(super) fn subscribe(&self) -> McpProgressSubscription {
        self.router.subscribe(&self.id, &self.token)
    }
}

impl Drop for McpProgressLease {
    fn drop(&mut self) {
        self.router.relinquish(&self.id, &self.token);
    }
}

/// One in-flight call's claimed progress delivery.
///
/// Dropping it stops delivery; it is **not** the request's forget point,
/// which belongs to [`McpProgressLease`] alone.
pub(super) struct McpProgressSubscription {
    receiver: tokio::sync::mpsc::Receiver<ProgressNotificationParam>,
}

impl McpProgressSubscription {
    pub(super) async fn recv(&mut self) -> Option<ProgressNotificationParam> {
        self.receiver.recv().await
    }

    /// Takes every notification already delivered to this subscription
    /// without waiting for another.
    pub(super) fn drain(&mut self) -> Vec<ProgressNotificationParam> {
        let mut drained = Vec::new();
        while let Ok(notification) = self.receiver.try_recv() {
            drained.push(notification);
        }
        drained
    }
}

/// Forwards one remote progress notification through the generic progress
/// seam.
///
/// The shared canonical normalization drops non-finite `completed`/`total`
/// values, so the remote value needs no adapter-local filter.
pub(super) fn report_remote_progress(
    context: &ToolExecutionContext<'_>,
    notification: ProgressNotificationParam,
) {
    context
        .progress
        .report(bound_tool_progress(crate::tools::types::ToolProgress {
            message: notification.message,
            completed: Some(notification.progress),
            total: notification.total,
        }));
}

/// Reports every occurrence this call already owns, in arrival order.
///
/// Liveness evidence the peer delivered before a terminal outcome genuinely
/// happened first, so it is reported before that outcome is committed —
/// including evidence the call claimed from the router only after its
/// response had already resolved.
pub(super) fn drain_remote_progress(
    context: &ToolExecutionContext<'_>,
    progress: &mut McpProgressSubscription,
) {
    for notification in progress.drain() {
        report_remote_progress(context, notification);
    }
}

/// Deterministic regressions for the progress ownership contract
/// (Issue #205).
///
/// MCP progress is idle-liveness evidence, so state that can silently
/// discard the only progress report of an admitted in-flight call turns
/// genuine remote progress into a false idle timeout. These exercise the
/// ownership boundary directly: no clock, no transport, no sleep — the
/// router's own admission, delivery and ownership decisions are the whole
/// contract.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::{NumberOrString, ProgressNotificationParam, ProgressToken, RequestId};

    use super::{McpProgressRouter, PROGRESS_SUBSCRIPTION_CAPACITY};

    fn token(value: u32) -> ProgressToken {
        ProgressToken(NumberOrString::Number(value.into()))
    }

    fn id(value: i64) -> RequestId {
        RequestId::Number(value)
    }

    fn notification(value: u32, progress: f64) -> ProgressNotificationParam {
        ProgressNotificationParam::new(token(value), progress)
    }

    /// A known live request keeps its liveness occurrence under any amount
    /// of payload pressure, and the payload it keeps is the most recent one.
    ///
    /// This is the coalescing contract stated exactly: detail may be
    /// collapsed, the occurrence may not be lost.
    #[test]
    fn a_known_live_request_keeps_its_occurrence_under_payload_pressure() {
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(1), &token(7));
        let lease = router.lease(id(1), token(7));
        for pulse in 0..5_000 {
            router.deliver(notification(7, f64::from(pulse)));
        }
        let delivered = lease.subscribe().drain();
        assert_eq!(
            delivered.len(),
            1,
            "one known live request holds one O(1) coalesced entry"
        );
        assert!(
            (delivered[0].progress - 4_999.0).abs() < f64::EPSILON,
            "coalescing keeps the most recent payload"
        );
    }

    /// Peer-controlled unknown tokens can never evict a known `ToolCall`'s
    /// liveness evidence, because they occupy no storage to evict it from.
    ///
    /// This is the finding the old fixed-size FIFO window failed: an unknown
    /// token displaced an admitted request's only progress occurrence once
    /// the window was full.
    #[test]
    fn unknown_peer_token_pressure_never_evicts_a_known_live_request() {
        let router = Arc::new(McpProgressRouter::default());
        // One admitted request, with its only progress notification already
        // delivered before it could subscribe.
        router.admit(&id(1), &token(1));
        let lease = router.lease(id(1), token(1));
        router.deliver(notification(1, 0.5));
        // A peer flood, orders of magnitude above any cache a bounded
        // pre-subscription window could have had.
        for unknown in 1_000..101_000_u32 {
            router.deliver(notification(unknown, 1.0));
        }
        assert_eq!(
            router.tracked_requests(),
            1,
            "unsolicited progress is counted, never stored, so the live set is \
             bounded by rustX's own admitted requests"
        );
        assert_eq!(router.unsolicited_dropped(), 100_000);
        let delivered = lease.subscribe().drain();
        assert_eq!(
            delivered.len(),
            1,
            "the admitted request still owns its liveness evidence"
        );
        assert!((delivered[0].progress - 0.5).abs() < f64::EPSILON);
    }

    /// Far more concurrent admitted requests than the removed bound, every
    /// one of them racing its own subscription, and every one still
    /// observing its own remote liveness evidence.
    #[test]
    fn every_admitted_request_keeps_its_own_evidence_far_above_the_removed_bound() {
        let router = Arc::new(McpProgressRouter::default());
        let concurrent = 1_024_u32;
        // The whole batch crosses its dispatch frontier first, and every
        // server answers with progress, before any of them subscribes: the
        // pre-subscription window, held open for all of them at once.
        let leases: Vec<_> = (0..concurrent)
            .map(|index| {
                router.admit(&id(i64::from(index)), &token(index));
                router.lease(id(i64::from(index)), token(index))
            })
            .collect();
        for index in 0..concurrent {
            router.deliver(notification(index, f64::from(index)));
        }
        for (index, lease) in leases.iter().enumerate() {
            let delivered = lease.subscribe().drain();
            assert_eq!(
                delivered.len(),
                1,
                "request {index} must observe its own remote liveness evidence"
            );
            #[allow(clippy::cast_precision_loss)]
            let expected = index as f64;
            assert!((delivered[0].progress - expected).abs() < f64::EPSILON);
        }
    }

    /// A subscriber's queue overflowing never erases the liveness fact: the
    /// queue is full precisely because that many undelivered proofs are
    /// already waiting for it.
    #[test]
    fn a_full_subscription_queue_still_holds_undelivered_liveness_evidence() {
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(3), &token(3));
        let lease = router.lease(id(3), token(3));
        let mut subscription = lease.subscribe();
        for pulse in 0..(PROGRESS_SUBSCRIPTION_CAPACITY * 4) {
            #[allow(clippy::cast_precision_loss)]
            router.deliver(notification(3, pulse as f64));
        }
        let delivered = subscription.drain();
        assert_eq!(
            delivered.len(),
            PROGRESS_SUBSCRIPTION_CAPACITY,
            "drop-on-full bounds the queue"
        );
        assert!(
            !delivered.is_empty(),
            "the idle watchdog still observes that progress occurred"
        );
    }

    /// **The Issue #205 review finding, stated directly.**
    ///
    /// A correlated response ends remote execution; it says nothing about
    /// whether the dispatching call still exists. Here the peer answers
    /// while the call is provably still between its effect frontier and its
    /// subscription — the reachable ordering the executor cannot avoid,
    /// because rmcp mints the progress token inside the request — and the
    /// genuine progress it already observed must still be there to claim.
    ///
    /// Under the reviewed implementation `settle` removed the pending
    /// entry here and this test fails at the `pending_occurrences` assertion.
    #[test]
    fn a_correlated_response_never_deletes_pre_subscription_progress() {
        let router = Arc::new(McpProgressRouter::default());
        // 1. the outbound seam admits the request inside `Transport::send`.
        router.admit(&id(5), &token(5));
        // 2. the dispatching call owns progress from the instant its request
        //    id exists, and has not claimed delivery yet.
        let lease = router.lease(id(5), token(5));
        // 3. genuine remote progress arrives first.
        router.deliver(notification(5, 0.25));
        assert_eq!(
            router.pending_occurrences(&id(5)),
            Some(1),
            "the observed occurrence is owned by the live call"
        );
        // 4. the peer's correlated response arrives while the call is still
        //    parked before `subscribe`.
        router.mark_remote_terminal(&id(5));
        // 5. the evidence is still the call's.
        assert_eq!(
            router.pending_occurrences(&id(5)),
            Some(1),
            "a correlated response is not proof that the dispatching caller \
             relinquished its progress ownership"
        );
        assert_eq!(router.tracked_requests(), 1);
        // 6. the call finally claims delivery, and gets it.
        let delivered = lease.subscribe().drain();
        assert_eq!(
            delivered.len(),
            1,
            "the buffered occurrence is claimed by the subscription that \
             followed the response"
        );
        assert!((delivered[0].progress - 0.25).abs() < f64::EPSILON);
        // 7. only the caller's own relinquish is terminal.
        drop(lease);
        assert_eq!(
            router.tracked_requests(),
            0,
            "the caller relinquished after admission, so the request is forgotten"
        );
        assert_eq!(router.deliverable_tokens(), 0);
    }

    /// Admission is consumed even if the remote server never answers.
    #[test]
    fn an_admitted_relinquished_request_is_forgotten_while_remote_is_open() {
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(10), &token(10));
        let lease = router.lease(id(10), token(10));
        router.deliver(notification(10, 1.0));
        drop(lease);
        assert_eq!(router.tracked_requests(), 0);
        assert_eq!(router.deliverable_tokens(), 0);
        assert_eq!(router.pending_occurrences(&id(10)), None);
        router.deliver(notification(10, 2.0));
        assert_eq!(router.unsolicited_dropped(), 1);
        assert_eq!(router.tracked_requests(), 0);
    }

    /// A call that never subscribes still relinquishes: the lease is the
    /// caller dimension, so cancellation, a deadline or any other exit
    /// before the subscription leaves no retained token.
    #[test]
    fn a_call_that_never_subscribes_relinquishes_its_progress_state() {
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(7), &token(7));
        let lease = router.lease(id(7), token(7));
        router.deliver(notification(7, 1.0));
        drop(lease);
        assert_eq!(
            router.deliverable_tokens(),
            0,
            "nothing is deliverable to a caller that no longer exists"
        );
        router.deliver(notification(7, 2.0));
        assert_eq!(
            router.unsolicited_dropped(),
            1,
            "progress for a relinquished token is counted, never stored"
        );
        router.mark_remote_terminal(&id(7));
        assert_eq!(
            router.tracked_requests(),
            0,
            "caller relinquishment after admission is the forget point"
        );
    }

    /// A request rustX refused to dispatch can never be answered, so the
    /// refusal is its remote terminality and it reaches the same forget
    /// point as an answered one.
    #[test]
    fn a_refused_dispatch_reaches_the_same_forget_point() {
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(8), &token(8));
        let lease = router.lease(id(8), token(8));
        drop(lease);
        assert_eq!(
            router.tracked_requests(),
            0,
            "admission was consumed, so remote uncertainty retains nothing"
        );
        router.mark_remote_terminal(&id(8));
        assert_eq!(router.tracked_requests(), 0);
    }

    /// `Transport::send` runs in rmcp's service loop, so an admission can
    /// arrive after the dispatching call has already returned. It must not
    /// re-create an owned entry: nothing would be left to forget it.
    #[test]
    fn a_late_admission_never_resurrects_a_relinquished_request() {
        let router = Arc::new(McpProgressRouter::default());
        let lease = router.lease(id(9), token(9));
        drop(lease);
        assert_eq!(
            router.tracked_requests(),
            1,
            "pending admission needs a tombstone"
        );
        assert_eq!(router.deliverable_tokens(), 0);
        router.admit(&id(9), &token(9));
        assert_eq!(
            router.tracked_requests(),
            0,
            "late admission consumes the tombstone"
        );
        assert_eq!(router.deliverable_tokens(), 0);
        router.deliver(notification(9, 1.0));
        assert_eq!(
            router.pending_occurrences(&id(9)),
            None,
            "a relinquished request owns no evidence"
        );
        assert_eq!(router.unsolicited_dropped(), 1);
        router.mark_remote_terminal(&id(9));
        assert_eq!(router.tracked_requests(), 0);
    }

    /// Every ordering of the four events, and none of them leaks or loses.
    ///
    /// The ownership transitions are ordered by nothing, so the contract is stated
    /// as a table rather than as one happy path: what each ordering must
    /// preserve is the occurrence, and what each must reach is zero.
    #[test]
    fn every_ownership_ordering_preserves_evidence_and_returns_to_baseline() {
        // admit -> subscribe -> progress -> response
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(1), &token(1));
        let lease = router.lease(id(1), token(1));
        let mut subscription = lease.subscribe();
        router.deliver(notification(1, 1.0));
        router.mark_remote_terminal(&id(1));
        assert!(
            router.is_subscribed(&id(1)),
            "a response never disarms a live subscription"
        );
        assert_eq!(subscription.drain().len(), 1);
        drop(lease);
        assert_eq!(router.tracked_requests(), 0);

        // admit -> progress -> subscribe -> response
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(2), &token(2));
        let lease = router.lease(id(2), token(2));
        router.deliver(notification(2, 1.0));
        let mut subscription = lease.subscribe();
        router.mark_remote_terminal(&id(2));
        assert_eq!(subscription.drain().len(), 1);
        drop(lease);
        assert_eq!(router.tracked_requests(), 0);

        // admit -> progress -> response -> subscribe (the review finding)
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(3), &token(3));
        let lease = router.lease(id(3), token(3));
        router.deliver(notification(3, 1.0));
        router.mark_remote_terminal(&id(3));
        assert_eq!(lease.subscribe().drain().len(), 1);
        drop(lease);
        assert_eq!(router.tracked_requests(), 0);

        // admit -> response -> subscribe, with no progress at all
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(4), &token(4));
        let lease = router.lease(id(4), token(4));
        router.mark_remote_terminal(&id(4));
        let mut subscription = lease.subscribe();
        assert!(subscription.drain().is_empty());
        drop(lease);
        assert_eq!(router.tracked_requests(), 0);

        // admit -> relinquish -> response
        let router = Arc::new(McpProgressRouter::default());
        router.admit(&id(5), &token(5));
        drop(router.lease(id(5), token(5)));
        router.mark_remote_terminal(&id(5));
        assert_eq!(router.tracked_requests(), 0);

        // subscribe -> admit: the subscription beat the outbound seam
        let router = Arc::new(McpProgressRouter::default());
        let lease = router.lease(id(6), token(6));
        let mut subscription = lease.subscribe();
        router.admit(&id(6), &token(6));
        router.deliver(notification(6, 1.0));
        assert_eq!(subscription.drain().len(), 1);
        drop(lease);
        assert_eq!(router.tracked_requests(), 0);
        assert_eq!(router.deliverable_tokens(), 0);
        router.mark_remote_terminal(&id(6));
        assert_eq!(router.tracked_requests(), 0);
    }

    /// Pending admission is the only reason a relinquished record survives;
    /// terminal remote ownership consumes that reason too, idempotently.
    #[test]
    fn pending_admission_tombstones_end_at_admission_or_remote_terminality() {
        for subscribe in [false, true] {
            for terminal_first in [false, true] {
                let router = Arc::new(McpProgressRouter::default());
                let lease = router.lease(id(11), token(11));
                let _subscription = subscribe.then(|| lease.subscribe());
                drop(lease);
                assert_eq!(router.tracked_requests(), 1);
                assert_eq!(router.deliverable_tokens(), 0);
                assert_eq!(router.pending_occurrences(&id(11)), None);
                if terminal_first {
                    router.mark_remote_terminal(&id(11));
                } else {
                    router.admit(&id(11), &token(11));
                }
                assert_eq!(router.tracked_requests(), 0);
                router.mark_remote_terminal(&id(11));
                router.mark_remote_terminal(&id(11));
                assert_eq!(router.tracked_requests(), 0);
                assert_eq!(router.deliverable_tokens(), 0);
            }
        }

        // A live subscriber retains evidence even if remote terminality
        // precedes admission; only its lease may relinquish that evidence.
        let router = Arc::new(McpProgressRouter::default());
        let lease = router.lease(id(12), token(12));
        let mut subscription = lease.subscribe();
        router.mark_remote_terminal(&id(12));
        router.deliver(notification(12, 1.0));
        assert_eq!(router.tracked_requests(), 1);
        assert_eq!(subscription.drain().len(), 1);
        drop(lease);
        assert_eq!(router.tracked_requests(), 0);
        assert_eq!(router.deliverable_tokens(), 0);
    }

    /// A long run of complete request lifecycles leaves the router at its
    /// baseline cardinality, so nothing accumulates per historical request.
    #[test]
    fn a_completed_request_lifecycle_returns_the_router_to_baseline() {
        let router = Arc::new(McpProgressRouter::default());
        for index in 0..256_u32 {
            router.admit(&id(i64::from(index)), &token(index));
            let lease = router.lease(id(i64::from(index)), token(index));
            router.deliver(notification(index, 1.0));
            router.mark_remote_terminal(&id(i64::from(index)));
            assert_eq!(lease.subscribe().drain().len(), 1);
            drop(lease);
        }
        assert_eq!(
            router.tracked_requests(),
            0,
            "state is bounded by live request lifecycles, never by history"
        );
        assert_eq!(router.deliverable_tokens(), 0);
    }
}
