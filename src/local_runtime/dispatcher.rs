//! The one owner of the subagent child's raw control transport (Issue #145).
//!
//! Before #145 the child driver was the only thing that ever touched the
//! inherited `UnixStream`, so a single `&mut` was enough. #145 makes the
//! child create real supervised process units, and each of those must
//! offer its containment anchor to the parent and wait for the
//! acknowledgement — concurrently with the driver reading `Delegate`,
//! serving `Cancel`, and writing the terminal `Result`.
//!
//! Letting each unit owner lock the stream would create several readers and
//! several writers of one frame protocol: interleaved partial frames, lost
//! EOF observation, and acknowledgements delivered to whichever task
//! happened to be reading. Instead there is exactly **one** dispatcher:
//!
//! ```text
//!                        ChildControlDispatcher
//!   reliable control channel (fd 0)     observation channel (fd 1)
//!   reader task        writer task      observation writer task
//!   (sole reader of    (sole writer of  (sole writer of the
//!    the read half)     the write half)  disposable transport)
//!        |                   ^                 ^
//!   Delegate -> delegate     |  Ready          |  Activity
//!   Cancel   -> cancel       |  Result         (disposable:
//!   AnchorAccepted/Refused   |  StartupError    latest-value,
//!        -> pending waiter   |  AnchorOffered   intermediates may
//!   EOF    -> parent-lost    |  AnchorReleased  be skipped; its
//!            watch           |  Diagnostic)     stall or loss is
//!                                              diagnostics-only)
//! ```
//!
//! Everything else in the child talks to the dispatcher through narrow
//! bounded in-process channels and never learns that a socket exists.
//! There is no listener and no network service.
//!
//! # Two delivery classes, two transports
//!
//! The two delivery classes (Issue #178) travel on **separate inherited
//! channels** so they share no transport backpressure dependency:
//!
//! - **Reliable** frames (everything except `Activity`): one bounded,
//!   ordered mpsc queue drained in order by the control-channel writer. A
//!   send awaits capacity and is never silently dropped while the parent
//!   control plane is alive.
//! - **Disposable** `Activity` frames: one latest-value `watch` slot
//!   drained by the observation-channel writer. A publication overwrites
//!   the previous unpublished value in place, so activity traffic consumes
//!   **no** reliable-queue capacity, and because the observation writer
//!   owns a different stream, a stalled observation transport can never
//!   delay a terminal `Result`, a containment ownership frame, or a
//!   startup failure — even mid-write.
//!
//! # Acknowledgement routing
//!
//! Anchor acknowledgements are routed by **exact typed identity**
//! ([`ProcessUnitId`]), never by arrival order: two units may have offers
//! outstanding at the same time, and a late acknowledgement must not be
//! able to open a different unit's start gate.
//!
//! # Parent liveness
//!
//! The control channel's read half is the parent-liveness authority. When
//! it reaches EOF the dispatcher publishes parent loss once, fails every
//! outstanding anchor offer with [`AnchorError::ParentLost`], and every
//! consumer — including a long-running external capability preparation —
//! observes it immediately. The observation channel is never liveness
//! evidence: its EOF ends only the observation writer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;

use crate::runtime::identity::ProcessUnitId;
use crate::runtime::interaction::{
    InteractionAdmissionError, InteractionPublicationPermit, InteractionRef,
};
use crate::runtime::nested_containment::{AnchorError, NestedAnchorAuthority};
use crate::runtime::subagent::activity::SubagentObservation;
use crate::runtime::subagent::ipc::{
    ActivityFrame, ChildFrame, DelegationFrame, ParentFrame, ProcessUnitAnchorFrame,
    read_parent_frame, write_activity_frame, write_child_frame,
};
use crate::runtime::types::CancellationReason;

/// The bound of the dispatcher's outbound queue.
///
/// Outbound frames are small typed envelopes emitted by a bounded number of
/// concurrent owners (the driver plus one per live supervised unit). The
/// bound exists so a stalled parent applies backpressure instead of letting
/// the child accumulate unbounded pending frames.
const OUTBOUND_CAPACITY: usize = 64;

type InteractionAdmissionWaiter = (
    InteractionRef,
    tokio::sync::oneshot::Sender<Result<InteractionPublicationPermit, InteractionAdmissionError>>,
);

/// One control event the child's semantic driver must act on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ChildControlEvent {
    /// The delegated task arrived (exactly once, after `Ready`).
    Delegate(DelegationFrame),
    /// The parent requested cancellation. A semantic reason is present for a
    /// committed child cancellation; `None` is reserved for pre-Ready
    /// preparation cancellation, which has no child attempt to settle.
    Cancel {
        /// The first-winner semantic cancellation cause, when available.
        reason: Option<CancellationReason>,
    },
    /// A root Runtime Client response addressed to this child's coordinator.
    InteractionRespond {
        /// Transport-only response correlation identity.
        response_id: u64,
        /// The full routed semantic address.
        interaction: crate::runtime::interaction::InteractionRef,
        /// The typed response to validate at the child coordinator.
        response: crate::runtime::interaction::InteractionResponse,
    },
    /// An early root Runtime Client human-provider availability hint.
    /// Publication admission uses a separate authoritative handshake.
    InteractionProviderAvailable {
        /// Whether a root control attachment was present when this hint was
        /// sent.
        available: bool,
    },
    /// The parent violated the bounded control protocol.
    ProtocolViolation(String),
}

/// The clonable handle every child-side owner uses to reach the parent.
#[derive(Clone)]
pub(crate) struct ChildControlHandle {
    inner: Arc<DispatcherInner>,
}

impl std::fmt::Debug for ChildControlHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChildControlHandle")
            .field("parent_lost", &self.parent_lost())
            .finish_non_exhaustive()
    }
}

struct DispatcherInner {
    /// The reliable lane: bounded, ordered, never silently dropped.
    outbound: tokio::sync::mpsc::Sender<ReliableFrame>,
    /// The disposable lane (Issue #178): the latest unpublished activity
    /// projection. A publication overwrites in place; the writer may skip
    /// intermediate values.
    activity: tokio::sync::watch::Sender<ActivityFrame>,
    pending: Mutex<HashMap<ProcessUnitId, tokio::sync::oneshot::Sender<Result<(), AnchorError>>>>,
    /// Publication-admission replies awaiting the exact child-side route
    /// call. This is transport correlation only; the originating coordinator
    /// still owns the interaction and consumes the resulting permit.
    interaction_admissions: Mutex<HashMap<u64, InteractionAdmissionWaiter>>,
    next_interaction_admission_id: std::sync::atomic::AtomicU64,
    parent_lost: tokio::sync::watch::Sender<bool>,
}

/// One frame in the reliable outbound queue. Most control traffic only needs
/// the established bounded-queue guarantee; routed interaction events also
/// request a completion acknowledgement from the sole writer so their owner
/// cannot wake before the control write either succeeds or fails.
struct ReliableFrame {
    frame: ChildFrame,
    completion: Option<tokio::sync::oneshot::Sender<Result<(), AnchorError>>>,
}

impl ReliableFrame {
    fn queued(frame: ChildFrame) -> Self {
        Self {
            frame,
            completion: None,
        }
    }
}

impl ChildControlHandle {
    /// Sends one **reliable** frame to the parent.
    ///
    /// The reliable lane is ordered and non-lossy: the send awaits bounded
    /// queue capacity and the frame is never silently dropped while the
    /// parent control plane is alive. This is the lane of every lifecycle,
    /// settlement, and containment ownership frame.
    pub(crate) async fn send_reliable(&self, frame: ChildFrame) -> Result<(), AnchorError> {
        self.inner
            .outbound
            .send(ReliableFrame::queued(frame))
            .await
            .map_err(|_| {
                self.inner.publish_parent_loss();
                AnchorError::ParentLost
            })
    }

    /// Sends one routed interaction event and waits for the reliable writer
    /// to complete the control write. Queue acceptance alone is insufficient
    /// for interaction settlement: the originating waiter must not wake as a
    /// healthy execution if the peer has already rejected the frame.
    pub(crate) async fn send_reliable_confirmed(
        &self,
        frame: ChildFrame,
    ) -> Result<(), AnchorError> {
        if self.parent_lost() {
            return Err(AnchorError::ParentLost);
        }
        let (completion, receiver) = tokio::sync::oneshot::channel();
        self.inner
            .outbound
            .send(ReliableFrame {
                frame,
                completion: Some(completion),
            })
            .await
            .map_err(|_| {
                self.inner.publish_parent_loss();
                AnchorError::ParentLost
            })?;
        receiver.await.unwrap_or_else(|_| {
            self.inner.publish_parent_loss();
            Err(AnchorError::ParentLost)
        })
    }

    /// Requests root publication admission for one exact routed interaction.
    /// The response is correlated by both a transport request id and the
    /// echoed `InteractionRef`; a stale or mismatched response is rejected.
    pub(crate) fn admit_interaction_publication(
        &self,
        interaction: InteractionRef,
    ) -> futures_util::future::BoxFuture<
        'static,
        Result<InteractionPublicationPermit, InteractionAdmissionError>,
    > {
        let request_id = self
            .inner
            .next_interaction_admission_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if request_id == 0 {
            return Box::pin(std::future::ready(Err(
                InteractionAdmissionError::ControlLost,
            )));
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.inner
            .interaction_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(request_id, (interaction.clone(), sender));
        let handle = self.clone();
        Box::pin(async move {
            if handle.parent_lost() {
                handle
                    .inner
                    .interaction_admissions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&request_id);
                return Err(InteractionAdmissionError::ControlLost);
            }
            if handle
                .send_reliable(ChildFrame::InteractionPublicationAdmissionRequested(
                    crate::runtime::subagent::ipc::InteractionPublicationAdmissionFrame {
                        request_id,
                        interaction,
                        admitted: false,
                    },
                ))
                .await
                .is_err()
            {
                handle
                    .inner
                    .interaction_admissions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&request_id);
                return Err(InteractionAdmissionError::ControlLost);
            }
            receiver
                .await
                .unwrap_or(Err(InteractionAdmissionError::ControlLost))
        })
    }

    /// Attempts to enqueue one reliable frame without waiting. `Full` is
    /// reported to the caller so a publication can fail closed; no semantic
    /// frame is ever coalesced or silently discarded.
    #[cfg(test)]
    pub(crate) fn try_send_reliable(&self, frame: ChildFrame) -> Result<(), AnchorError> {
        self.inner
            .outbound
            .try_send(ReliableFrame::queued(frame))
            .map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => AnchorError::Refused(
                    "the reliable child control lane is at capacity".to_owned(),
                ),
                tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                    self.inner.publish_parent_loss();
                    AnchorError::ParentLost
                }
            })
    }

    /// Publishes one **disposable** live-activity projection (Issue #178).
    ///
    /// Synchronous, infallible, and non-blocking: the publication
    /// overwrites the previous unpublished value in place, so intermediate
    /// values may never reach the wire, and a stalled parent can never
    /// apply backpressure to the publisher. This lane carries observation
    /// traffic only — never anything a consumer must observe.
    pub(crate) fn publish_activity(&self, frame: ActivityFrame) {
        self.inner.activity.send_replace(frame);
    }

    /// Whether the parent control channel has already reached EOF.
    pub(crate) fn parent_lost(&self) -> bool {
        *self.inner.parent_lost.borrow()
    }

    /// Resolves when the parent control channel reaches EOF.
    ///
    /// This is the child's parent-liveness authority during **preparation**:
    /// a long MCP connect or uv build races this future and settles instead
    /// of finishing work for a parent that no longer exists.
    pub(crate) async fn parent_lost_signal(&self) {
        let mut receiver = self.inner.parent_lost.subscribe();
        if *receiver.borrow() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow() {
                return;
            }
        }
        // The sender is gone, which can only happen once the dispatcher is
        // finished; treat that as parent loss too.
    }
}

impl std::fmt::Debug for DispatcherInner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ChildControlDispatcher")
    }
}

impl NestedAnchorAuthority for DispatcherInner {
    fn offer(&self, unit: ProcessUnitId, pgid: i32) -> BoxFuture<'static, Result<(), AnchorError>> {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending.contains_key(&unit) {
                return Box::pin(std::future::ready(Err(AnchorError::Refused(
                    "a nested process unit of that identity already has an offer outstanding"
                        .to_owned(),
                ))));
            }
            pending.insert(unit.clone(), ack_tx);
        }
        if *self.parent_lost.borrow() {
            self.pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&unit);
            return Box::pin(std::future::ready(Err(AnchorError::ParentLost)));
        }
        let outbound = self.outbound.clone();
        let frame = ReliableFrame::queued(ChildFrame::AnchorOffered(ProcessUnitAnchorFrame {
            unit_id: unit,
            pgid,
        }));
        Box::pin(async move {
            outbound
                .send(frame)
                .await
                .map_err(|_| AnchorError::ParentLost)?;
            // The offer is outstanding until the parent answers or the
            // parent-liveness authority fails it. There is deliberately no
            // local timeout: a nested unit that cannot be anchored must
            // never start, and a live parent that is slow is not a reason
            // to start one.
            ack_rx.await.unwrap_or(Err(AnchorError::ParentLost))
        })
    }

    fn release(&self, unit: ProcessUnitId, pgid: i32) -> BoxFuture<'static, ()> {
        let outbound = self.outbound.clone();
        let frame = ReliableFrame::queued(ChildFrame::AnchorReleased(ProcessUnitAnchorFrame {
            unit_id: unit,
            pgid,
        }));
        Box::pin(async move {
            // A release is an ownership transition, not telemetry: while
            // the parent control plane is alive it is never silently
            // dropped. The bounded queue applies backpressure — this send
            // awaits capacity and the one writer delivers the frame — so a
            // proven-terminal release cannot be lost to a full queue. A
            // closed queue means the dispatcher has drained or the parent
            // is definitively gone; that is the separate state in which
            // there is nothing left to release.
            let _ = outbound.send(frame).await;
        })
    }
}

/// Test-only gate in front of a writer's next transport write (Issue #145).
///
/// While the gate is closed the gated writer waits instead of receiving and
/// writing, so a test can fill a bounded queue or stall one transport
/// *deterministically* — for the reliable writer, `OUTBOUND_CAPACITY`
/// completed sends prove the queue full; for the observation writer, a
/// closed gate is the exact model of a stalled observation transport. The
/// gate is level-triggered: once opened it stays open, and opening it lets
/// the writer drain normally from there.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct WriterGate {
    open: Arc<tokio::sync::watch::Sender<bool>>,
}

#[cfg(test)]
impl WriterGate {
    /// Opens the gate permanently: the writer drains normally from here.
    ///
    /// `send_replace`, not `send`: a `watch` send drops the value when no
    /// receiver is currently subscribed, and the writer subscribes only
    /// when its task first runs — losing the open would park the writer
    /// forever.
    pub(crate) fn open(&self) {
        self.open.send_replace(true);
    }

    /// Parks while the gate is closed; resolves immediately once open (or
    /// when the gate handle is gone, which only happens at test teardown).
    async fn wait_open(&self) {
        let mut receiver = self.open.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Test-only gate that delays delivery of one provider-detach notification
/// after it has reached the child dispatcher. The root host's admission
/// authority remains ungated; this exists solely to make stale child-cache
/// interleavings explicit without scheduler timing.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct ProviderAvailabilityGate {
    armed: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<tokio::sync::watch::Sender<bool>>,
    release: Arc<tokio::sync::watch::Sender<bool>>,
}

#[cfg(test)]
impl Default for ProviderAvailabilityGate {
    fn default() -> Self {
        let (entered, _) = tokio::sync::watch::channel(false);
        let (release, _) = tokio::sync::watch::channel(false);
        Self {
            armed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            entered: Arc::new(entered),
            release: Arc::new(release),
        }
    }
}

#[cfg(test)]
impl ProviderAvailabilityGate {
    /// Arms the next false provider update and resets the deterministic
    /// entered/released observations.
    pub(crate) fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        self.entered.send_replace(false);
        self.release.send_replace(false);
    }

    /// Waits until the child reader has received the false update and is
    /// deliberately holding it before converting it into a child event.
    pub(crate) async fn wait_entered(&self) {
        let mut entered = self.entered.subscribe();
        while !*entered.borrow_and_update() {
            if entered.changed().await.is_err() {
                return;
            }
        }
    }

    /// Releases the delayed provider update.
    pub(crate) fn release(&self) {
        self.release.send_replace(true);
    }

    fn should_delay(&self, available: bool) -> bool {
        if available || !self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        self.entered.send_replace(true);
        true
    }

    async fn wait_released(&self) {
        let mut release = self.release.subscribe();
        while !*release.borrow_and_update() {
            if release.changed().await.is_err() {
                return;
            }
        }
    }
}

/// The started dispatcher: the handle every owner shares, the semantic
/// control-event stream the driver consumes, and the three owned tasks.
pub(crate) struct ChildControlDispatcher {
    handle: ChildControlHandle,
    events: tokio::sync::mpsc::Receiver<ChildControlEvent>,
    /// Closes the outbound queue at drain.
    ///
    /// The writer cannot simply end when every sender drops: the nested
    /// anchor authority installed process-wide holds one, and so does every
    /// live supervised unit's lease. Drain therefore closes the queue
    /// explicitly, which lets the writer flush what is already queued and
    /// then finish.
    close: Arc<tokio::sync::Notify>,
    reader: tokio::task::JoinHandle<()>,
    writer: tokio::task::JoinHandle<()>,
    observation_writer: tokio::task::JoinHandle<()>,
}

impl ChildControlDispatcher {
    /// Takes sole ownership of the raw control transport and the disposable
    /// observation transport, and starts the one reader task, the one
    /// reliable writer task, and the one observation writer task.
    pub(crate) fn start(
        control: tokio::net::UnixStream,
        observation: tokio::net::UnixStream,
    ) -> Self {
        #[cfg(test)]
        {
            Self::start_impl(control, observation, None, None, None)
        }
        #[cfg(not(test))]
        {
            Self::start_impl(control, observation)
        }
    }

    /// Starts the dispatcher with writer gates installed (tests only), so
    /// bounded-queue backpressure and a stalled observation transport are
    /// exercised deterministically.
    #[cfg(test)]
    pub(crate) fn start_with_gates(
        control: tokio::net::UnixStream,
        observation: tokio::net::UnixStream,
        writer_gate: Option<WriterGate>,
        observation_gate: Option<WriterGate>,
    ) -> Self {
        Self::start_impl(control, observation, writer_gate, observation_gate, None)
    }

    /// Starts the dispatcher with a deterministic provider-detach delivery
    /// gate. This is a test seam for proving that root admission, rather than
    /// the child's cached availability bit, decides publication.
    #[cfg(test)]
    pub(crate) fn start_with_provider_gate(
        control: tokio::net::UnixStream,
        observation: tokio::net::UnixStream,
        provider_gate: ProviderAvailabilityGate,
    ) -> Self {
        Self::start_impl(control, observation, None, None, Some(provider_gate))
    }

    #[allow(clippy::too_many_lines)] // one owner initializes both reliable/disposable lanes
    fn start_impl(
        control: tokio::net::UnixStream,
        observation: tokio::net::UnixStream,
        #[cfg(test)] writer_gate: Option<WriterGate>,
        #[cfg(test)] observation_gate: Option<WriterGate>,
        #[cfg(test)] provider_gate: Option<ProviderAvailabilityGate>,
    ) -> Self {
        let (read_half, write_half) = tokio::io::split(control);
        let (outbound_tx, outbound_rx) =
            tokio::sync::mpsc::channel::<ReliableFrame>(OUTBOUND_CAPACITY);
        let (events_tx, events_rx) = tokio::sync::mpsc::channel::<ChildControlEvent>(4);
        let (lost_tx, _lost_rx) = tokio::sync::watch::channel(false);
        let (activity_tx, activity_rx) = tokio::sync::watch::channel(ActivityFrame {
            observation: SubagentObservation::default(),
        });
        let inner = Arc::new(DispatcherInner {
            outbound: outbound_tx,
            activity: activity_tx,
            pending: Mutex::new(HashMap::new()),
            interaction_admissions: Mutex::new(HashMap::new()),
            next_interaction_admission_id: std::sync::atomic::AtomicU64::new(1),
            parent_lost: lost_tx,
        });

        let close = Arc::new(tokio::sync::Notify::new());
        let writer_inner = Arc::clone(&inner);
        let writer = tokio::spawn(run_writer(
            write_half,
            outbound_rx,
            close.clone(),
            writer_inner,
            #[cfg(test)]
            writer_gate,
        ));
        let observation_writer = tokio::spawn(run_observation_writer(
            observation,
            activity_rx,
            #[cfg(test)]
            observation_gate,
        ));

        let reader_inner = Arc::clone(&inner);
        let reader = tokio::spawn(async move {
            let mut read_half = read_half;
            loop {
                match read_parent_frame(&mut read_half).await {
                    Ok(Some(ParentFrame::Delegate(delegate))) => {
                        if events_tx
                            .send(ChildControlEvent::Delegate(delegate))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Some(ParentFrame::Cancel { reason })) => {
                        if events_tx
                            .send(ChildControlEvent::Cancel { reason })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Some(ParentFrame::InteractionRespond {
                        response_id,
                        interaction,
                        response,
                    })) => {
                        if events_tx
                            .send(ChildControlEvent::InteractionRespond {
                                response_id,
                                interaction,
                                response,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Some(ParentFrame::InteractionProviderAvailable { available })) => {
                        #[cfg(test)]
                        if let Some(gate) = &provider_gate
                            && gate.should_delay(available)
                        {
                            let gate = gate.clone();
                            let events_tx = events_tx.clone();
                            tokio::spawn(async move {
                                gate.wait_released().await;
                                let _ = events_tx
                                    .send(ChildControlEvent::InteractionProviderAvailable {
                                        available,
                                    })
                                    .await;
                            });
                            continue;
                        }
                        if events_tx
                            .send(ChildControlEvent::InteractionProviderAvailable { available })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Some(ParentFrame::InteractionPublicationAdmissionResult(result))) => {
                        reader_inner.resolve_interaction_admission(&result);
                    }
                    Ok(Some(ParentFrame::AnchorAccepted(ack))) => {
                        reader_inner.resolve(&ack.unit_id, Ok(()));
                    }
                    Ok(Some(ParentFrame::AnchorRefused(refusal))) => {
                        reader_inner
                            .resolve(&refusal.unit_id, Err(AnchorError::Refused(refusal.reason)));
                    }
                    Ok(Some(ParentFrame::Hello(_))) => {
                        let _ = events_tx
                            .send(ChildControlEvent::ProtocolViolation(
                                "a second Hello frame arrived".to_owned(),
                            ))
                            .await;
                        break;
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = events_tx
                            .send(ChildControlEvent::ProtocolViolation(error.to_string()))
                            .await;
                        break;
                    }
                }
            }
            reader_inner.publish_parent_loss();
        });

        Self {
            handle: ChildControlHandle { inner },
            events: events_rx,
            close,
            reader,
            writer,
            observation_writer,
        }
    }

    /// The clonable handle shared by every child-side owner.
    pub(crate) fn handle(&self) -> ChildControlHandle {
        self.handle.clone()
    }
    /// The nested anchor authority backed by this dispatcher.
    pub(crate) fn anchor_authority(&self) -> Arc<dyn NestedAnchorAuthority> {
        Arc::clone(&self.handle.inner) as Arc<dyn NestedAnchorAuthority>
    }

    /// The next semantic control event, or `None` once the parent control
    /// channel is finished.
    pub(crate) async fn next_event(&mut self) -> Option<ChildControlEvent> {
        self.events.recv().await
    }

    /// Drains the transport: closes the outbound queue, joins the reliable
    /// writer (which flushes what is already queued and then drops the
    /// control write half, so the parent observes the child's drain), stops
    /// the observation writer (a still-pending activity is dropped, which
    /// is exactly the disposable contract), and stops the reader.
    pub(crate) async fn shutdown(self) {
        let Self {
            handle,
            events,
            close,
            reader,
            writer,
            observation_writer,
        } = self;
        drop(events);
        drop(handle);
        close.notify_one();
        let _ = writer.await;
        observation_writer.abort();
        let _ = observation_writer.await;
        reader.abort();
        let _ = reader.await;
    }
}

/// The one reliable writer task: the sole writer of the control channel's
/// write half.
///
/// Queued reliable frames drain in order, and once the reliable queue is
/// closed and drained (`recv()` reports `None`) the writer ends. This task
/// never touches the observation channel, so nothing it does can be delayed
/// by observation traffic — and vice versa.
async fn run_writer(
    mut write_half: tokio::io::WriteHalf<tokio::net::UnixStream>,
    mut outbound_rx: tokio::sync::mpsc::Receiver<ReliableFrame>,
    close: Arc<tokio::sync::Notify>,
    inner: Arc<DispatcherInner>,
    #[cfg(test)] writer_gate: Option<WriterGate>,
) {
    let mut closing = false;
    loop {
        // The test gate parks the writer BEFORE it receives, so the
        // bounded queue provably stays full while the gate is
        // closed. Level-triggered: once open this never waits.
        #[cfg(test)]
        if let Some(gate) = &writer_gate {
            gate.wait_open().await;
        }
        tokio::select! {
            biased;
            frame = outbound_rx.recv() => match frame {
                Some(ReliableFrame { frame, completion }) => {
                    if write_child_frame(&mut write_half, &frame).await.is_err() {
                        // A failed reliable write is itself a parent-control
                        // loss. Publish it immediately instead of relying on
                        // a later send or on the read half also reaching EOF;
                        // a frame accepted by the bounded queue is not
                        // considered delivered until this write succeeds.
                        if let Some(completion) = completion {
                            let _ = completion.send(Err(AnchorError::ParentLost));
                        }
                        inner.publish_parent_loss();
                        break;
                    }
                    if let Some(completion) = completion {
                        let result = if inner.is_parent_lost() {
                            Err(AnchorError::ParentLost)
                        } else {
                            Ok(())
                        };
                        let _ = completion.send(result);
                    }
                }
                None => break,
            },
            () = close.notified(), if !closing => {
                // Refuse new frames and drain what is already
                // queued; `recv()` then reports the closed queue.
                closing = true;
                outbound_rx.close();
            }
        }
    }
}

/// The one observation writer task: the sole writer of the disposable
/// observation channel (Issue #178).
///
/// The loop waits for a published activity, then writes the newest
/// projection to the observation transport. A stalled transport parks this
/// task alone — the reliable writer owns a different stream — and a failed
/// transport ends only this task: publications keep overwriting the slot,
/// and no lifecycle, settlement, or ownership path is affected.
async fn run_observation_writer(
    mut stream: tokio::net::UnixStream,
    activity_rx: tokio::sync::watch::Receiver<ActivityFrame>,
    #[cfg(test)] observation_gate: Option<WriterGate>,
) {
    // The receiver is the channel's original, so it has already seen the
    // initial placeholder value: `changed()` fires only for values
    // published after the channel was created. Deliberately do NOT
    // `borrow_and_update()` here — that would additionally swallow every
    // value published before this task is first scheduled, which is not
    // the disposable contract (the latest published value must reach the
    // wire regardless of writer scheduling).
    let mut activity_rx = activity_rx;
    loop {
        match activity_rx.changed().await {
            Ok(()) => {
                // The test gate parks the writer with a publication in
                // hand: the deterministic model of an observation
                // transport that accepted nothing more. While parked,
                // newer publications keep overwriting the slot, and the
                // write below still carries only the newest value.
                #[cfg(test)]
                if let Some(gate) = &observation_gate {
                    gate.wait_open().await;
                }
                let frame = activity_rx.borrow_and_update().clone();
                if write_activity_frame(&mut stream, &frame).await.is_err() {
                    // The observation transport is gone. That is
                    // diagnostics-only: never parent loss, never
                    // lifecycle evidence — this task simply ends.
                    return;
                }
            }
            // Every activity sender is gone (the dispatcher is being
            // torn down): the observation writer's work is over.
            Err(_) => return,
        }
    }
}

impl DispatcherInner {
    fn is_parent_lost(&self) -> bool {
        *self.parent_lost.borrow()
    }

    fn resolve(&self, unit: &ProcessUnitId, outcome: Result<(), AnchorError>) {
        let waiter = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(unit);
        if let Some(waiter) = waiter {
            let _ = waiter.send(outcome);
        }
    }

    fn resolve_interaction_admission(
        &self,
        result: &crate::runtime::subagent::ipc::InteractionPublicationAdmissionFrame,
    ) {
        let waiter = self
            .interaction_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&result.request_id);
        let Some((expected, sender)) = waiter else {
            return;
        };
        if expected != result.interaction || !result.admitted {
            let failure = if expected == result.interaction {
                InteractionAdmissionError::Unavailable
            } else {
                InteractionAdmissionError::ControlLost
            };
            let _ = sender.send(Err(failure));
            return;
        }
        let _ = sender.send(Ok(InteractionPublicationPermit::for_interaction(expected)));
    }

    fn publish_parent_loss(&self) {
        // `send_replace`, not `send`: a `watch` send fails and leaves the
        // value untouched when no receiver is currently subscribed, and both
        // `parent_lost()` and `parent_lost_signal()` read the current value.
        // Losing this publication would make a child wait forever for a
        // parent that is already gone.
        self.parent_lost.send_replace(true);
        let pending = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for (_, waiter) in pending {
            let _ = waiter.send(Err(AnchorError::ParentLost));
        }
        let admissions = std::mem::take(
            &mut *self
                .interaction_admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for (_, (_, waiter)) in admissions {
            let _ = waiter.send(Err(InteractionAdmissionError::ControlLost));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ChildControlDispatcher, ChildControlEvent};
    use crate::runtime::identity::ProcessUnitId;
    use crate::runtime::identity::{AttemptId, ConversationId, InteractionId, ToolCallId, ToolId};
    use crate::runtime::interaction::{
        ApprovalDecision, InteractionAdmissionError, InteractionKind, InteractionOutcome,
        InteractionRef, InteractionRequest, InteractionResponse,
    };
    use crate::runtime::nested_containment::AnchorError;
    use crate::runtime::subagent::activity::SubagentObservation;
    use crate::runtime::subagent::ipc::{
        ActivityFrame, ChildFrame, DelegationFrame, DiagnosticFrame,
        InteractionPublicationAdmissionFrame, ParentFrame, ProcessUnitAckFrame,
        ProcessUnitRefusalFrame, read_activity_frame, read_child_frame, write_parent_frame,
    };
    use crate::runtime::types::CancellationReason;
    use crate::tools::types::{ToolInvocationMode, ToolOrigin};
    use futures_util::FutureExt;

    fn pair() -> (tokio::net::UnixStream, tokio::net::UnixStream) {
        tokio::net::UnixStream::pair().expect("control socket pair")
    }

    fn activity(revision: u64) -> ActivityFrame {
        ActivityFrame {
            observation: SubagentObservation {
                revision,
                ..SubagentObservation::default()
            },
        }
    }

    /// Two units may have offers outstanding at once, and each ACK opens
    /// exactly its own gate: routing is by typed identity, never by order.
    #[tokio::test]
    async fn acknowledgements_route_by_exact_unit_identity() {
        let (mut parent, child) = pair();
        // The observation channel is held open but carries nothing here.
        let (_observation_parent, observation_child) = pair();
        let dispatcher = ChildControlDispatcher::start(child, observation_child);
        let authority = dispatcher.anchor_authority();

        let first = ProcessUnitId::new("unit-a");
        let second = ProcessUnitId::new("unit-b");
        let offer_first = tokio::spawn({
            let authority = authority.clone();
            let unit = first.clone();
            async move { authority.offer(unit, 11).await }
        });
        let offer_second = tokio::spawn({
            let authority = authority.clone();
            let unit = second.clone();
            async move { authority.offer(unit, 22).await }
        });

        // Collect both offers before answering, so the answers are
        // deliberately delivered in the opposite order.
        let mut offered = Vec::new();
        for _ in 0..2 {
            match read_child_frame(&mut parent).await.expect("offer") {
                Some(ChildFrame::AnchorOffered(frame)) => offered.push(frame),
                other => panic!("expected an anchor offer, got {other:?}"),
            }
        }
        assert_eq!(offered.len(), 2);

        write_parent_frame(
            &mut parent,
            &ParentFrame::AnchorAccepted(ProcessUnitAckFrame {
                unit_id: second.clone(),
            }),
        )
        .await
        .expect("ack second");
        assert_eq!(
            offer_second.await.expect("join"),
            Ok(()),
            "the second unit's gate opens on its own ACK"
        );
        assert!(
            !offer_first.is_finished(),
            "the first unit's gate stays closed until its own ACK arrives"
        );

        write_parent_frame(
            &mut parent,
            &ParentFrame::AnchorAccepted(ProcessUnitAckFrame { unit_id: first }),
        )
        .await
        .expect("ack first");
        assert_eq!(offer_first.await.expect("join"), Ok(()));
    }

    /// A refusal fails exactly the refused unit's gate with its reason.
    #[tokio::test]
    async fn a_refusal_fails_only_that_unit() {
        let (mut parent, child) = pair();
        let (_observation_parent, observation_child) = pair();
        let dispatcher = ChildControlDispatcher::start(child, observation_child);
        let authority = dispatcher.anchor_authority();
        let unit = ProcessUnitId::new("unit-a");
        let offer = tokio::spawn({
            let authority = authority.clone();
            let unit = unit.clone();
            async move { authority.offer(unit, 11).await }
        });
        assert!(matches!(
            read_child_frame(&mut parent).await.expect("offer"),
            Some(ChildFrame::AnchorOffered(_))
        ));
        write_parent_frame(
            &mut parent,
            &ParentFrame::AnchorRefused(ProcessUnitRefusalFrame {
                unit_id: unit,
                reason: "duplicate".to_owned(),
            }),
        )
        .await
        .expect("refuse");
        assert_eq!(
            offer.await.expect("join"),
            Err(AnchorError::Refused("duplicate".to_owned()))
        );
    }

    /// Parent EOF fails every outstanding offer and publishes parent loss,
    /// so a preparation racing it settles rather than continuing.
    ///
    /// The observation channel stays **open** the whole time (Issue #178):
    /// control-channel EOF alone is the parent-liveness authority, and a
    /// live observation transport can never substitute for it.
    #[tokio::test]
    async fn parent_eof_fails_every_outstanding_offer() {
        let (parent, child) = pair();
        let (_observation_parent, observation_child) = pair();
        let dispatcher = ChildControlDispatcher::start(child, observation_child);
        let authority = dispatcher.anchor_authority();
        let handle = dispatcher.handle();
        let offer = tokio::spawn({
            let authority = authority.clone();
            async move { authority.offer(ProcessUnitId::new("unit-a"), 11).await }
        });
        // Drop the parent end only after the offer is provably outstanding.
        let mut parent = parent;
        assert!(matches!(
            read_child_frame(&mut parent).await.expect("offer"),
            Some(ChildFrame::AnchorOffered(_))
        ));
        drop(parent);
        assert_eq!(offer.await.expect("join"), Err(AnchorError::ParentLost));
        handle.parent_lost_signal().await;
        assert!(handle.parent_lost());
    }

    /// An offer made after parent loss fails immediately: nothing may start.
    #[tokio::test]
    async fn an_offer_after_parent_loss_fails_immediately() {
        let (parent, child) = pair();
        let (_observation_parent, observation_child) = pair();
        let dispatcher = ChildControlDispatcher::start(child, observation_child);
        let handle = dispatcher.handle();
        let authority = dispatcher.anchor_authority();
        drop(parent);
        handle.parent_lost_signal().await;
        assert_eq!(
            authority.offer(ProcessUnitId::new("unit-a"), 11).await,
            Err(AnchorError::ParentLost)
        );
    }

    /// The child's drain must complete even while other owners still hold
    /// outbound senders.
    ///
    /// The nested anchor authority is installed **process-wide** and every
    /// live supervised unit lease holds a clone, so "the writer ends when
    /// every sender drops" would never happen: the child would hang at exit
    /// instead of letting the parent observe its drain. Drain therefore
    /// closes the queue explicitly, flushing what is already queued.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn drain_completes_while_other_owners_still_hold_senders() {
        let (mut parent, child) = pair();
        let (_observation_parent, observation_child) = pair();
        let dispatcher = ChildControlDispatcher::start(child, observation_child);
        // Two independent owners keep senders alive across the drain,
        // exactly as the installed authority and a live unit lease would.
        let authority = dispatcher.anchor_authority();
        let handle = dispatcher.handle();
        handle
            .send_reliable(ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                    content: Some("answer".to_owned()),
                    diagnostic: None,
                },
            ))
            .await
            .expect("the terminal result is queued");

        tokio::time::timeout(std::time::Duration::from_secs(10), dispatcher.shutdown())
            .await
            .expect("the drain must complete while other senders are alive");

        // The queued terminal result was flushed before the write half
        // closed, and the parent then observes a clean EOF.
        assert!(matches!(
            read_child_frame(&mut parent).await.expect("result"),
            Some(ChildFrame::Result(_))
        ));
        assert_eq!(read_child_frame(&mut parent).await.expect("eof"), None);
        // The surviving owners are inert rather than dangling: a release on
        // a drained transport is a no-op, not a panic or a hang.
        authority.release(ProcessUnitId::new("unit-a"), 11).await;
        assert_eq!(
            handle
                .send_reliable(ChildFrame::Diagnostic(DiagnosticFrame {
                    message: "after drain".to_owned(),
                }))
                .await,
            Err(AnchorError::ParentLost),
            "a send after the drain is refused rather than queued forever"
        );
    }

    /// Delegate and Cancel reach the semantic driver as events; the raw
    /// stream is never exposed.
    #[tokio::test]
    async fn semantic_frames_become_control_events() {
        let (mut parent, child) = pair();
        let (_observation_parent, observation_child) = pair();
        let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
        write_parent_frame(
            &mut parent,
            &ParentFrame::Delegate(DelegationFrame {
                task: "inspect".to_owned(),
                context: None,
                interaction_provider_available: false,
            }),
        )
        .await
        .expect("delegate");
        write_parent_frame(
            &mut parent,
            &ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested),
            },
        )
        .await
        .expect("cancel");
        assert!(matches!(
            dispatcher.next_event().await,
            Some(ChildControlEvent::Delegate(_))
        ));
        assert_eq!(
            dispatcher.next_event().await,
            Some(ChildControlEvent::Cancel {
                reason: Some(CancellationReason::UserRequested),
            })
        );
    }

    /// A mismatched admission result is rejected by the child-side transport
    /// correlation and cannot authorize a different routed interaction. A
    /// later exact response still resolves only its own request.
    #[tokio::test]
    async fn publication_admission_requires_exact_routed_identity() {
        let (mut parent, child) = pair();
        let (_observation_parent, observation_child) = pair();
        let dispatcher = ChildControlDispatcher::start(child, observation_child);
        let handle = dispatcher.handle();
        let first = InteractionRef::new(
            ConversationId::new("child-conversation"),
            InteractionId::new("interaction-1"),
        );
        let first_waiter = tokio::spawn({
            let handle = handle.clone();
            let first = first.clone();
            async move { handle.admit_interaction_publication(first).await }
        });
        let first_request = match read_child_frame(&mut parent).await.expect("read admission") {
            Some(ChildFrame::InteractionPublicationAdmissionRequested(request)) => request,
            other => panic!("unexpected admission frame: {other:?}"),
        };
        assert_eq!(first_request.interaction, first);
        let mut mismatched = first_request.clone();
        mismatched.interaction = InteractionRef::new(
            ConversationId::new("child-conversation"),
            InteractionId::new("interaction-2"),
        );
        mismatched.admitted = true;
        write_parent_frame(
            &mut parent,
            &ParentFrame::InteractionPublicationAdmissionResult(mismatched),
        )
        .await
        .expect("write mismatched admission");
        assert_eq!(
            first_waiter.await.expect("first waiter"),
            Err(InteractionAdmissionError::ControlLost)
        );

        let second = InteractionRef::new(
            ConversationId::new("child-conversation"),
            InteractionId::new("interaction-2"),
        );
        let second_waiter = tokio::spawn({
            let handle = handle.clone();
            let second = second.clone();
            async move { handle.admit_interaction_publication(second).await }
        });
        let second_request = match read_child_frame(&mut parent).await.expect("read admission") {
            Some(ChildFrame::InteractionPublicationAdmissionRequested(request)) => request,
            other => panic!("unexpected admission frame: {other:?}"),
        };
        assert_eq!(second_request.interaction, second);
        write_parent_frame(
            &mut parent,
            &ParentFrame::InteractionPublicationAdmissionResult(
                InteractionPublicationAdmissionFrame {
                    request_id: second_request.request_id,
                    interaction: second_request.interaction.clone(),
                    admitted: true,
                },
            ),
        )
        .await
        .expect("write exact admission");
        let permit = second_waiter.await.expect("second waiter").expect("permit");
        assert!(permit.matches(&second));

        dispatcher.shutdown().await;
    }

    /// A proven-terminal release under bounded outbound backpressure is
    /// **never silently dropped**: with the queue provably full (the writer
    /// is gated, so `OUTBOUND_CAPACITY` completed sends fill it exactly),
    /// the release future pends instead of resolving, and once the
    /// backpressure clears the exact `AnchorReleased` frame arrives and
    /// removes exactly the parent's retained anchor.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_proven_terminal_release_survives_bounded_backpressure() {
        use futures_util::FutureExt;

        let (mut parent, child) = pair();
        let (_observation_parent, observation_child) = pair();
        let gate = super::WriterGate::default();
        let dispatcher = ChildControlDispatcher::start_with_gates(
            child,
            observation_child,
            Some(gate.clone()),
            None,
        );
        let handle = dispatcher.handle();
        let authority = dispatcher.anchor_authority();

        // Fill the bounded outbound queue exactly. The writer is gated
        // before its first receive, so these completed sends are the proof
        // that the queue is now full — no timing is involved.
        for index in 0..super::OUTBOUND_CAPACITY {
            handle
                .send_reliable(ChildFrame::Diagnostic(DiagnosticFrame {
                    message: format!("filler {index}"),
                }))
                .await
                .expect("queue capacity remains while filling");
        }

        // The parent retains this exact anchor; the child has just proven
        // the unit physically terminal and releases it under backpressure.
        let unit = ProcessUnitId::new("unit-a");
        let mut retained = crate::runtime::subagent::anchors::RetainedProcessUnits::default();
        retained.retain(unit.clone(), 4242).expect("retained");
        let mut release = Box::pin(authority.release(unit.clone(), 4242));
        assert!(
            release.as_mut().now_or_never().is_none(),
            "a release behind a full queue must wait for capacity; it is never dropped"
        );

        // The backpressure clears: the writer drains the queue in order and
        // the release is delivered — never lost.
        gate.open();
        release.await;
        for _ in 0..super::OUTBOUND_CAPACITY {
            assert!(
                matches!(
                    read_child_frame(&mut parent).await.expect("queued frame"),
                    Some(ChildFrame::Diagnostic(_))
                ),
                "the queued fillers drain first, in order"
            );
        }
        assert_eq!(
            read_child_frame(&mut parent)
                .await
                .expect("the release frame"),
            Some(ChildFrame::AnchorReleased(
                crate::runtime::subagent::ipc::ProcessUnitAnchorFrame {
                    unit_id: unit.clone(),
                    pgid: 4242
                }
            )),
            "the exact AnchorReleased arrives once the backpressure clears"
        );
        // The parent applies exactly this release to exactly this anchor.
        assert!(retained.release(&unit, 4242));
        assert!(retained.is_empty());

        dispatcher.shutdown().await;
    }

    /// Transport isolation of the terminal result (Issue #178). The
    /// observation writer is gated — the deterministic model of a
    /// backpressured observation transport — with 100 activity revisions
    /// saturated into the latest-value slot. The terminal `Result`:
    ///
    /// 1. completes its send immediately even with the reliable writer
    ///    gated (activity consumed no reliable-queue capacity), and
    /// 2. crosses the reliable control channel the instant the reliable
    ///    writer runs, while the observation transport stays stalled.
    ///
    /// Only when the observation stall clears does exactly one activity
    /// frame arrive on the observation channel: the latest revision, never
    /// an intermediate, and never on the control channel.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stalled_observation_transport_never_delays_the_terminal_result() {
        use futures_util::FutureExt;

        let (mut parent, child) = pair();
        let (mut observation_parent, observation_child) = pair();
        let writer_gate = super::WriterGate::default();
        let observation_gate = super::WriterGate::default();
        let dispatcher = ChildControlDispatcher::start_with_gates(
            child,
            observation_child,
            Some(writer_gate.clone()),
            Some(observation_gate.clone()),
        );
        let handle = dispatcher.handle();

        // Saturate the disposable lane: every publication overwrites the
        // previous unpublished value in place, synchronously.
        for revision in 1..=100u64 {
            handle.publish_activity(activity(revision));
        }

        // The terminal result never waits on activity capacity: the
        // reliable queue is provably empty, so this send resolves
        // immediately even though both writers are parked.
        let result = crate::runtime::subagent::ipc::ResultFrame {
            status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            content: Some("the terminal answer".to_owned()),
            diagnostic: None,
        };
        handle
            .send_reliable(ChildFrame::Result(result.clone()))
            .await
            .expect("Result never waits on disposable activity");
        assert!(
            read_child_frame(&mut parent).now_or_never().is_none(),
            "the gated reliable writer has written nothing yet"
        );

        // The observation transport stays stalled: the Result still
        // crosses the reliable control channel immediately and intact.
        writer_gate.open();
        assert_eq!(
            read_child_frame(&mut parent).await.expect("result frame"),
            Some(ChildFrame::Result(result)),
            "the Result arrives while the observation transport is stalled"
        );
        assert!(
            read_activity_frame(&mut observation_parent)
                .now_or_never()
                .is_none(),
            "the stalled observation channel carries nothing"
        );

        // Once the stall clears, exactly one activity frame arrives on the
        // observation channel: the latest revision, never an intermediate.
        observation_gate.open();
        assert_eq!(
            read_activity_frame(&mut observation_parent)
                .await
                .expect("activity frame"),
            Some(activity(100)),
            "the coalesced activity carries only the latest revision"
        );
        assert!(
            read_activity_frame(&mut observation_parent)
                .now_or_never()
                .is_none(),
            "no intermediate revision ever reached the wire"
        );
        assert!(
            read_child_frame(&mut parent).now_or_never().is_none(),
            "activity never touches the reliable control channel"
        );

        dispatcher.shutdown().await;
    }

    /// Routed interaction requests and settlements use the reliable control
    /// lane, even while the disposable observation writer is stalled and its
    /// latest-value slot is being overwritten. The exact pair identity and
    /// both interaction kinds stay intact on the semantic lane.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one deterministic two-lane transport proof
    async fn routed_interactions_are_reliable_and_observation_independent() {
        let (mut parent, child) = pair();
        let (mut observation_parent, observation_child) = pair();
        let observation_gate = super::WriterGate::default();
        let dispatcher = ChildControlDispatcher::start_with_gates(
            child,
            observation_child,
            None,
            Some(observation_gate.clone()),
        );
        let handle = dispatcher.handle();

        for revision in 1..=100u64 {
            handle.publish_activity(activity(revision));
        }

        let approval = InteractionRequest {
            id: InteractionId::new("approval-1"),
            conversation_id: ConversationId::new("child-conversation"),
            attempt_id: AttemptId::new("attempt-1"),
            turn: 1,
            kind: InteractionKind::Approval {
                call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-1"),
                tool_name: "probe".to_owned(),
                origin: ToolOrigin::Builtin,
                mode: ToolInvocationMode::Foreground,
                arguments: serde_json::json!({"path": "original"}),
                reason: "approval required".to_owned(),
            },
        };
        let questionnaire = InteractionRequest {
            id: InteractionId::new("questionnaire-1"),
            conversation_id: approval.conversation_id.clone(),
            attempt_id: AttemptId::new("attempt-1"),
            turn: 2,
            kind: InteractionKind::Questionnaire {
                questionnaire: crate::runtime::interaction::QuestionnaireSpecification {
                    questions: vec![crate::runtime::interaction::QuestionSpecification {
                        question: "Which target?".to_owned(),
                        header: "Target".to_owned(),
                        options: vec![
                            crate::runtime::interaction::OptionSpecification {
                                label: "staging".to_owned(),
                                description: "safe".to_owned(),
                                preview: None,
                            },
                            crate::runtime::interaction::OptionSpecification {
                                label: "production".to_owned(),
                                description: "live".to_owned(),
                                preview: None,
                            },
                        ],
                        multi_select: false,
                    }],
                },
            },
        };
        let approval_ref =
            InteractionRef::new(approval.conversation_id.clone(), approval.id.clone());
        let questionnaire_ref = InteractionRef::new(
            questionnaire.conversation_id.clone(),
            questionnaire.id.clone(),
        );
        handle
            .send_reliable(ChildFrame::InteractionRequested(approval.clone()))
            .await
            .expect("approval request is reliable");
        handle
            .send_reliable(ChildFrame::InteractionRequested(questionnaire.clone()))
            .await
            .expect("questionnaire request is reliable");
        handle
            .send_reliable(ChildFrame::InteractionSettled {
                interaction: approval_ref.clone(),
                outcome: InteractionOutcome::Responded {
                    response: InteractionResponse::Approval {
                        decision: ApprovalDecision::Allow,
                    },
                },
            })
            .await
            .expect("approval settlement is reliable");
        handle
            .send_reliable(ChildFrame::InteractionSettled {
                interaction: questionnaire_ref.clone(),
                outcome: InteractionOutcome::Cancelled {
                    reason: CancellationReason::UserRequested,
                },
            })
            .await
            .expect("questionnaire settlement is reliable");

        assert_eq!(
            read_child_frame(&mut parent)
                .await
                .expect("approval request"),
            Some(ChildFrame::InteractionRequested(approval)),
        );
        assert_eq!(
            read_child_frame(&mut parent)
                .await
                .expect("questionnaire request"),
            Some(ChildFrame::InteractionRequested(questionnaire)),
        );
        assert_eq!(
            read_child_frame(&mut parent)
                .await
                .expect("approval settlement"),
            Some(ChildFrame::InteractionSettled {
                interaction: approval_ref,
                outcome: InteractionOutcome::Responded {
                    response: InteractionResponse::Approval {
                        decision: ApprovalDecision::Allow,
                    },
                },
            }),
        );
        assert_eq!(
            read_child_frame(&mut parent)
                .await
                .expect("questionnaire settlement"),
            Some(ChildFrame::InteractionSettled {
                interaction: questionnaire_ref,
                outcome: InteractionOutcome::Cancelled {
                    reason: CancellationReason::UserRequested,
                },
            }),
        );
        assert!(
            read_activity_frame(&mut observation_parent)
                .now_or_never()
                .is_none(),
            "semantic interaction traffic never uses the stalled observation lane"
        );

        observation_gate.open();
        assert_eq!(
            read_activity_frame(&mut observation_parent)
                .await
                .expect("coalesced activity"),
            Some(activity(100)),
        );
        dispatcher.shutdown().await;
    }

    /// Transport isolation of containment ownership (Issue #178): with the
    /// observation transport stalled and the activity lane saturated, a
    /// proven-terminal `release` resolves immediately — the reliable lane
    /// has capacity by construction — and the exact `AnchorReleased` frame
    /// crosses the control channel while the observation transport stays
    /// stalled, releasing exactly the parent's retained anchor.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stalled_observation_transport_never_delays_a_containment_release() {
        use futures_util::FutureExt;

        let (mut parent, child) = pair();
        let (mut observation_parent, observation_child) = pair();
        let observation_gate = super::WriterGate::default();
        let dispatcher = ChildControlDispatcher::start_with_gates(
            child,
            observation_child,
            None,
            Some(observation_gate.clone()),
        );
        let handle = dispatcher.handle();
        let authority = dispatcher.anchor_authority();

        // Saturate the disposable lane.
        for revision in 1..=100u64 {
            handle.publish_activity(activity(revision));
        }

        // The parent retains this exact anchor; the child has just proven
        // the unit physically terminal. The release resolves immediately
        // and crosses the control channel even though the observation
        // transport never accepts another byte.
        let unit = ProcessUnitId::new("unit-a");
        let mut retained = crate::runtime::subagent::anchors::RetainedProcessUnits::default();
        retained.retain(unit.clone(), 4242).expect("retained");
        authority.release(unit.clone(), 4242).await;
        assert_eq!(
            read_child_frame(&mut parent).await.expect("release frame"),
            Some(ChildFrame::AnchorReleased(
                crate::runtime::subagent::ipc::ProcessUnitAnchorFrame {
                    unit_id: unit.clone(),
                    pgid: 4242
                }
            )),
            "the exact AnchorReleased arrives while the observation transport is stalled"
        );
        assert!(
            read_activity_frame(&mut observation_parent)
                .now_or_never()
                .is_none(),
            "the stalled observation channel carries nothing"
        );
        // The parent applies exactly this release to exactly this anchor.
        assert!(retained.release(&unit, 4242));
        assert!(retained.is_empty());

        // Once the stall clears, the coalesced activity arrives on the
        // observation channel only.
        observation_gate.open();
        assert_eq!(
            read_activity_frame(&mut observation_parent)
                .await
                .expect("activity frame"),
            Some(activity(100))
        );
        assert!(
            read_child_frame(&mut parent).now_or_never().is_none(),
            "activity never touches the reliable control channel"
        );

        dispatcher.shutdown().await;
    }

    /// Observation transport loss is diagnostics-only (Issue #178): with
    /// the parent end of the observation channel dropped, activity
    /// publication stays synchronous and infallible and the observation
    /// writer quietly ends, while the reliable control channel keeps every
    /// guarantee — an anchor offer/ack round-trip and the terminal result
    /// both complete — and the child never mistakes observation EOF for
    /// parent loss.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn observation_transport_loss_is_diagnostics_only() {
        let (mut parent, child) = pair();
        let (observation_parent, observation_child) = pair();
        let dispatcher = ChildControlDispatcher::start(child, observation_child);
        let handle = dispatcher.handle();
        let authority = dispatcher.anchor_authority();

        // The observation transport is gone. Publication is unaffected.
        drop(observation_parent);
        for revision in 1..=10u64 {
            handle.publish_activity(activity(revision));
        }

        // The reliable channel is untouched: an anchor offer round-trips...
        let offer = tokio::spawn({
            let authority = authority.clone();
            async move { authority.offer(ProcessUnitId::new("unit-a"), 11).await }
        });
        assert!(matches!(
            read_child_frame(&mut parent).await.expect("offer"),
            Some(ChildFrame::AnchorOffered(_))
        ));
        write_parent_frame(
            &mut parent,
            &ParentFrame::AnchorAccepted(ProcessUnitAckFrame {
                unit_id: ProcessUnitId::new("unit-a"),
            }),
        )
        .await
        .expect("ack");
        assert_eq!(offer.await.expect("join"), Ok(()));

        // ...and the terminal result arrives.
        handle
            .send_reliable(ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                    content: Some("the terminal answer".to_owned()),
                    diagnostic: None,
                },
            ))
            .await
            .expect("the terminal result is queued");
        assert!(matches!(
            read_child_frame(&mut parent).await.expect("result"),
            Some(ChildFrame::Result(_))
        ));

        // Observation EOF is never parent loss.
        assert!(
            !handle.parent_lost(),
            "observation transport loss is not lifecycle evidence"
        );

        dispatcher.shutdown().await;
    }

    /// Destroying a held anchor's owner — by plain drop or by task abort —
    /// is **not** terminal proof: no `AnchorReleased` may be emitted, and
    /// the parent's retained anchor (its catastrophic containment authority
    /// for that exact unit) must survive.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_unproven_owner_loss_never_releases_the_retained_anchor() {
        let (mut parent, child) = pair();
        let (_observation_parent, observation_child) = pair();
        let dispatcher = ChildControlDispatcher::start(child, observation_child);
        let authority = dispatcher.anchor_authority();
        let handle = dispatcher.handle();

        // The parent-side view: both offered anchors are ACKed and
        // retained. Each lease comes from the real `retain_with` path a
        // supervised-unit owner uses.
        let mut retained = crate::runtime::subagent::anchors::RetainedProcessUnits::default();
        let mut acknowledge = async |unit: &ProcessUnitId, pgid: i32| {
            let pending = tokio::spawn({
                let authority = authority.clone();
                let unit = unit.clone();
                async move {
                    crate::runtime::nested_containment::retain_with(unit, pgid, Some(authority))
                        .await
                }
            });
            assert!(matches!(
                read_child_frame(&mut parent).await.expect("offer frame"),
                Some(ChildFrame::AnchorOffered(_))
            ));
            retained.retain(unit.clone(), pgid).expect("retained");
            write_parent_frame(
                &mut parent,
                &ParentFrame::AnchorAccepted(ProcessUnitAckFrame {
                    unit_id: unit.clone(),
                }),
            )
            .await
            .expect("ack");
            pending
                .await
                .expect("offer task")
                .expect("acknowledged lease")
        };

        // 1. A held lease destroyed by plain owner drop.
        let dropped = acknowledge(&ProcessUnitId::new("unit-drop"), 11).await;
        drop(dropped);

        // 2. A held lease destroyed by owner task abort mid-park.
        let aborted = acknowledge(&ProcessUnitId::new("unit-abort"), 22).await;
        let owner = tokio::spawn(async move {
            let _held = aborted;
            std::future::pending::<()>().await;
        });
        owner.abort();
        let _ = owner.await;

        // The wire proves the silence: frames are totally ordered, so any
        // manufactured release would arrive before this trailing frame.
        handle
            .send_reliable(ChildFrame::Diagnostic(DiagnosticFrame {
                message: "after both owner losses".to_owned(),
            }))
            .await
            .expect("trailing frame queued");
        assert_eq!(
            read_child_frame(&mut parent).await.expect("trailing frame"),
            Some(ChildFrame::Diagnostic(DiagnosticFrame {
                message: "after both owner losses".to_owned()
            })),
            "no AnchorReleased was manufactured by drop or abort"
        );

        // The parent still retains both exact anchors: catastrophic
        // containment authority for both units remains available.
        assert_eq!(retained.len(), 2);
        assert!(!retained.release(&ProcessUnitId::new("unit-drop"), 99));
        assert!(!retained.release(&ProcessUnitId::new("unit-abort"), 99));

        dispatcher.shutdown().await;
    }
}
