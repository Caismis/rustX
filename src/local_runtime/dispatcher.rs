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

use crate::runtime::interaction::{
    InteractionAdmissionError, InteractionPublicationPermit, InteractionRef,
};
use crate::runtime::subagent::activity::SubagentObservation;
use crate::runtime::subagent::ipc::{
    ActivityFrame, ChildFrame, DelegationFrame, ParentFrame, ProcessUnitAnchorFrame,
    read_parent_frame, write_activity_frame, write_child_frame,
};
use crate::runtime::types::CancellationReason;
use rustx_process_supervisor::identity::ProcessUnitId;
use rustx_process_supervisor::nested_containment::{AnchorError, NestedAnchorAuthority};

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
    AdmissionReopened,
    /// The registry closed message admission for this activation.
    SealGranted,
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
        response: crate::runtime::interaction::InteractionControl,
    },
    /// An early root Runtime Client human-provider availability hint.
    /// Publication admission uses a separate authoritative handshake.
    InteractionProviderAvailable {
        /// Whether a root control attachment was present when this hint was
        /// sent.
        available: bool,
    },
    /// One parent-authored in-flight guidance envelope arrived (Issue
    /// #193). The semantic driver answers it through the child
    /// conversation's own durable acceptance authority; the dispatcher only
    /// carries it, in the reliable channel's arrival order.
    Guidance {
        /// Transport-only acceptance correlation identity.
        guidance_id: u64,
        /// The bounded parent-authored guidance text.
        message: String,
        attachments: Vec<crate::message::content::UploadedFileRef>,
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
            Self::start_impl(control, observation, None, None)
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
        Self::start_impl(control, observation, writer_gate, observation_gate)
    }

    #[allow(clippy::too_many_lines)] // one owner initializes both reliable/disposable lanes
    fn start_impl(
        control: tokio::net::UnixStream,
        observation: tokio::net::UnixStream,
        #[cfg(test)] writer_gate: Option<WriterGate>,
        #[cfg(test)] observation_gate: Option<WriterGate>,
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
                        if events_tx
                            .send(ChildControlEvent::InteractionProviderAvailable { available })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Some(ParentFrame::AdmissionReopened)) => {
                        if events_tx
                            .send(ChildControlEvent::AdmissionReopened)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Some(ParentFrame::SealGranted)) => {
                        if events_tx
                            .send(ChildControlEvent::SealGranted)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Some(ParentFrame::Guidance(guidance))) => {
                        if events_tx
                            .send(ChildControlEvent::Guidance {
                                guidance_id: guidance.guidance_id,
                                message: guidance.message,
                                attachments: guidance.attachments,
                            })
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
mod tests;
