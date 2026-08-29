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
//!            reader task                       writer task
//!   (sole reader of the read half)   (sole writer of the write half)
//!            |                                     ^
//!   Delegate -> delegate channel                   |  Ready
//!   Cancel   -> cancel channel        outbound ----+  Result
//!   AnchorAccepted / AnchorRefused                    StartupError
//!            -> the exact pending unit's waiter       AnchorOffered
//!   EOF      -> parent-lost watch, every waiter       AnchorReleased
//!               fails ParentLost                      Diagnostic
//! ```
//!
//! Everything else in the child talks to the dispatcher through narrow
//! bounded in-process channels and never learns that a socket exists.
//! There is no second socket, no listener, and no network service.
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
//! The read half is the parent-liveness authority. When it reaches EOF the
//! dispatcher publishes parent loss once, fails every outstanding anchor
//! offer with [`AnchorError::ParentLost`], and every consumer — including a
//! long-running external capability preparation — observes it immediately.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;

use crate::runtime::identity::ProcessUnitId;
use crate::runtime::nested_containment::{AnchorError, NestedAnchorAuthority};
use crate::runtime::subagent::ipc::{
    ChildFrame, DelegationFrame, ParentFrame, ProcessUnitAnchorFrame, read_parent_frame,
    write_child_frame,
};

/// The bound of the dispatcher's outbound queue.
///
/// Outbound frames are small typed envelopes emitted by a bounded number of
/// concurrent owners (the driver plus one per live supervised unit). The
/// bound exists so a stalled parent applies backpressure instead of letting
/// the child accumulate unbounded pending frames.
const OUTBOUND_CAPACITY: usize = 64;

/// One control event the child's semantic driver must act on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ChildControlEvent {
    /// The delegated task arrived (exactly once, after `Ready`).
    Delegate(DelegationFrame),
    /// The parent requested cancellation.
    Cancel,
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
    outbound: tokio::sync::mpsc::Sender<ChildFrame>,
    pending: Mutex<HashMap<ProcessUnitId, tokio::sync::oneshot::Sender<Result<(), AnchorError>>>>,
    parent_lost: tokio::sync::watch::Sender<bool>,
}

impl ChildControlHandle {
    /// Sends one bounded frame to the parent.
    pub(crate) async fn send(&self, frame: ChildFrame) -> Result<(), AnchorError> {
        self.inner
            .outbound
            .send(frame)
            .await
            .map_err(|_| AnchorError::ParentLost)
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
        let frame = ChildFrame::AnchorOffered(ProcessUnitAnchorFrame {
            unit_id: unit,
            pgid,
        });
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

    fn release(&self, unit: ProcessUnitId, pgid: i32) {
        // Release is best-effort by construction: a parent that is already
        // gone has no retained anchor left to drop, and the send is
        // non-blocking so a proven-terminal settlement is never delayed by
        // a stalled control channel.
        let _ = self
            .outbound
            .try_send(ChildFrame::AnchorReleased(ProcessUnitAnchorFrame {
                unit_id: unit,
                pgid,
            }));
    }
}

/// The started dispatcher: the handle every owner shares, the semantic
/// control-event stream the driver consumes, and the two owned tasks.
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
}

impl ChildControlDispatcher {
    /// Takes sole ownership of the raw control transport and starts the one
    /// reader task and the one writer task.
    pub(crate) fn start(control: tokio::net::UnixStream) -> Self {
        let (read_half, write_half) = tokio::io::split(control);
        let (outbound_tx, mut outbound_rx) =
            tokio::sync::mpsc::channel::<ChildFrame>(OUTBOUND_CAPACITY);
        let (events_tx, events_rx) = tokio::sync::mpsc::channel::<ChildControlEvent>(4);
        let (lost_tx, _lost_rx) = tokio::sync::watch::channel(false);
        let inner = Arc::new(DispatcherInner {
            outbound: outbound_tx,
            pending: Mutex::new(HashMap::new()),
            parent_lost: lost_tx,
        });

        let close = Arc::new(tokio::sync::Notify::new());
        let writer_close = close.clone();
        let writer = tokio::spawn(async move {
            let mut write_half = write_half;
            let mut closing = false;
            loop {
                tokio::select! {
                    biased;
                    frame = outbound_rx.recv() => match frame {
                        Some(frame) => {
                            if write_child_frame(&mut write_half, &frame).await.is_err() {
                                // The parent is gone; the reader publishes
                                // parent loss.
                                break;
                            }
                        }
                        None => break,
                    },
                    () = writer_close.notified(), if !closing => {
                        // Refuse new frames and drain what is already
                        // queued; `recv()` then reports the closed queue.
                        closing = true;
                        outbound_rx.close();
                    }
                }
            }
        });

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
                    Ok(Some(ParentFrame::Cancel)) => {
                        if events_tx.send(ChildControlEvent::Cancel).await.is_err() {
                            break;
                        }
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

    /// Drains the transport: closes the outbound queue, joins the writer
    /// (which flushes what is already queued and then drops the write half,
    /// so the parent observes the child's drain), and stops the reader.
    pub(crate) async fn shutdown(self) {
        let Self {
            handle,
            events,
            close,
            reader,
            writer,
        } = self;
        drop(events);
        drop(handle);
        close.notify_one();
        let _ = writer.await;
        reader.abort();
        let _ = reader.await;
    }
}

impl DispatcherInner {
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
    }
}

#[cfg(test)]
mod tests {
    use super::{ChildControlDispatcher, ChildControlEvent};
    use crate::runtime::identity::ProcessUnitId;
    use crate::runtime::nested_containment::AnchorError;
    use crate::runtime::subagent::ipc::{
        ChildFrame, DelegationFrame, DiagnosticFrame, ParentFrame, ProcessUnitAckFrame,
        ProcessUnitRefusalFrame, read_child_frame, write_parent_frame,
    };

    fn pair() -> (tokio::net::UnixStream, tokio::net::UnixStream) {
        tokio::net::UnixStream::pair().expect("control socket pair")
    }

    /// Two units may have offers outstanding at once, and each ACK opens
    /// exactly its own gate: routing is by typed identity, never by order.
    #[tokio::test]
    async fn acknowledgements_route_by_exact_unit_identity() {
        let (mut parent, child) = pair();
        let dispatcher = ChildControlDispatcher::start(child);
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
        let dispatcher = ChildControlDispatcher::start(child);
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
    #[tokio::test]
    async fn parent_eof_fails_every_outstanding_offer() {
        let (parent, child) = pair();
        let dispatcher = ChildControlDispatcher::start(child);
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
        let dispatcher = ChildControlDispatcher::start(child);
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
        let dispatcher = ChildControlDispatcher::start(child);
        // Two independent owners keep senders alive across the drain,
        // exactly as the installed authority and a live unit lease would.
        let authority = dispatcher.anchor_authority();
        let handle = dispatcher.handle();
        handle
            .send(ChildFrame::Result(
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
        authority.release(ProcessUnitId::new("unit-a"), 11);
        assert_eq!(
            handle
                .send(ChildFrame::Diagnostic(DiagnosticFrame {
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
        let mut dispatcher = ChildControlDispatcher::start(child);
        write_parent_frame(
            &mut parent,
            &ParentFrame::Delegate(DelegationFrame {
                task: "inspect".to_owned(),
                context: None,
            }),
        )
        .await
        .expect("delegate");
        write_parent_frame(&mut parent, &ParentFrame::Cancel)
            .await
            .expect("cancel");
        assert!(matches!(
            dispatcher.next_event().await,
            Some(ChildControlEvent::Delegate(_))
        ));
        assert_eq!(
            dispatcher.next_event().await,
            Some(ChildControlEvent::Cancel)
        );
    }
}
