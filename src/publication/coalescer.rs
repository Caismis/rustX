//! The bounded deterministic publication coalescer.
//!
//! Provider chunk size is not the publication unit. A provider that emits one
//! delta per token would otherwise force one durable write per token; the
//! coalescer buffers those deltas in memory and turns them into a bounded
//! number of typed publication frames under one explicit policy:
//!
//! - **maximum bytes** — buffered committed-for-release bytes reached the
//!   threshold;
//! - **maximum latency** — the oldest buffered payload owns one absolute
//!   deadline, measured through an injected [`PublicationClock`]; later
//!   payloads never reset or extend that deadline;
//! - **structural boundary** — a tool-call proposal start or completion is
//!   released as its own observable transition;
//! - **stream terminal** — the publication terminal transaction always flushes
//!   whatever remains.
//!
//! Nothing here is durable and nothing here is released: the coalescer only
//! decides *when* a frame exists. The caller commits the produced frames and
//! only then releases them.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::sync::watch;

use crate::runtime::identity::{MessageId, PublicationStreamId};

use super::frame::{PublicationFrame, PublicationPayload};

/// The default publication byte threshold.
///
/// The value is a publication-plane policy, not a provider property: it is
/// large enough that ordinary token-sized deltas coalesce into few durable
/// writes and small enough that a long block still streams visibly.
pub const DEFAULT_PUBLICATION_MAX_BYTES: usize = 256;

/// The default publication latency threshold in milliseconds.
pub const DEFAULT_PUBLICATION_MAX_LATENCY_MILLIS: u64 = 50;

/// The bounded deterministic flush policy of one publication stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoalescePolicy {
    /// Flush once this many committed-for-release bytes are buffered.
    pub max_bytes: usize,
    /// Flush once the oldest buffered payload has waited this long.
    pub max_latency_millis: u64,
}

impl Default for CoalescePolicy {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_PUBLICATION_MAX_BYTES,
            max_latency_millis: DEFAULT_PUBLICATION_MAX_LATENCY_MILLIS,
        }
    }
}

/// The monotonic clock the latency policy reads.
///
/// Latency flushing is the only time-dependent part of publication. Injecting
/// the clock keeps it deterministic: tests advance a
/// [`ManualPublicationClock`] explicitly and never sleep to make a flush
/// happen.
pub trait PublicationClock: Send + Sync + fmt::Debug {
    /// Monotonic milliseconds since an arbitrary fixed origin.
    fn now_millis(&self) -> u64;

    /// Wakes at an absolute deadline in this clock's same monotonic domain.
    ///
    /// The coalescer owns the deadline; the clock owns the wake-up mechanism.
    /// This keeps production timers and deterministic test clocks from
    /// disagreeing about how much latency remains.
    fn wait_until_millis(&self, deadline_millis: u64) -> BoxFuture<'static, ()>;
}

/// The production monotonic clock.
#[derive(Debug)]
pub struct SystemPublicationClock {
    origin: tokio::time::Instant,
}

impl SystemPublicationClock {
    /// Creates a clock whose origin is now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl Default for SystemPublicationClock {
    fn default() -> Self {
        Self::new()
    }
}

impl PublicationClock for SystemPublicationClock {
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

/// A manually advanced clock for deterministic latency regressions.
///
/// This is a test seam in the same sense as
/// [`RecordingEventSink`](crate::events::RecordingEventSink): it is an
/// explicit deterministic control point, never a second production clock.
#[derive(Debug)]
pub struct ManualPublicationClock {
    millis: Arc<AtomicU64>,
    wake: watch::Sender<u64>,
}

impl Default for ManualPublicationClock {
    fn default() -> Self {
        let (wake, _receiver) = watch::channel(0);
        Self {
            millis: Arc::new(AtomicU64::new(0)),
            wake,
        }
    }
}

impl ManualPublicationClock {
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

impl PublicationClock for ManualPublicationClock {
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

/// The in-memory coalescer of one publication stream.
pub struct PublicationCoalescer {
    stream_id: PublicationStreamId,
    message_id: MessageId,
    policy: CoalescePolicy,
    clock: Arc<dyn PublicationClock>,
    next_sequence: u64,
    pending: Vec<PublicationPayload>,
    pending_bytes: usize,
    oldest_pending_deadline_millis: Option<u64>,
}

impl fmt::Debug for PublicationCoalescer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicationCoalescer")
            .field("stream_id", &self.stream_id)
            .field("next_sequence", &self.next_sequence)
            .field("pending", &self.pending.len())
            .field("pending_bytes", &self.pending_bytes)
            .finish_non_exhaustive()
    }
}

impl PublicationCoalescer {
    /// Creates the coalescer of one publication stream.
    #[must_use]
    pub fn new(
        stream_id: PublicationStreamId,
        message_id: MessageId,
        policy: CoalescePolicy,
        clock: Arc<dyn PublicationClock>,
    ) -> Self {
        Self {
            stream_id,
            message_id,
            policy,
            clock,
            next_sequence: 0,
            pending: Vec::new(),
            pending_bytes: 0,
            oldest_pending_deadline_millis: None,
        }
    }

    /// The publication stream this coalescer belongs to.
    #[must_use]
    pub const fn stream_id(&self) -> &PublicationStreamId {
        &self.stream_id
    }

    /// Buffers one committed-for-release payload, returning whether the
    /// bounded policy requires a flush now.
    ///
    /// A structural boundary always requires a flush, so a tool-call proposal
    /// start or completion is never withheld behind an unrelated byte budget.
    pub fn push(&mut self, payload: PublicationPayload) -> bool {
        let structural = payload.is_structural_boundary();
        if self.oldest_pending_deadline_millis.is_none() {
            self.oldest_pending_deadline_millis = Some(
                self.clock
                    .now_millis()
                    .saturating_add(self.policy.max_latency_millis),
            );
        }
        self.pending_bytes = self.pending_bytes.saturating_add(payload.byte_weight());
        let unmerged = if structural {
            Some(payload)
        } else {
            match self.pending.last_mut() {
                Some(last) if !last.is_structural_boundary() => last.merge(payload),
                _ => Some(payload),
            }
        };
        if let Some(payload) = unmerged {
            self.pending.push(payload);
        }
        structural || self.pending_bytes >= self.policy.max_bytes
    }

    /// Whether buffered payload has waited at least the latency threshold.
    #[must_use]
    pub fn latency_elapsed(&self) -> bool {
        let Some(deadline) = self.oldest_pending_deadline_millis else {
            return false;
        };
        self.clock.now_millis() >= deadline
    }

    /// The absolute deadline owned by the oldest buffered payload.
    #[must_use]
    pub const fn latency_deadline_millis(&self) -> Option<u64> {
        self.oldest_pending_deadline_millis
    }

    /// The unspent latency budget of the oldest buffered payload.
    #[must_use]
    pub fn latency_remaining_millis(&self) -> Option<u64> {
        self.oldest_pending_deadline_millis
            .map(|deadline| deadline.saturating_sub(self.clock.now_millis()))
    }

    /// Creates the wake-up future for the oldest payload's absolute deadline.
    ///
    /// The returned future is owned by the clock, so callers can await it in a
    /// `select!` and then mutably flush the coalescer without holding a borrow
    /// into the coalescer across the await.
    #[must_use]
    pub fn latency_wait(&self) -> Option<BoxFuture<'static, ()>> {
        self.oldest_pending_deadline_millis
            .map(|deadline| self.clock.wait_until_millis(deadline))
    }

    /// Whether any payload is buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// The number of frames this coalescer has produced so far.
    ///
    /// This is the write-amplification measure of the stream: it counts
    /// durable publication frames, never provider deltas.
    #[must_use]
    pub const fn produced_frames(&self) -> u64 {
        self.next_sequence
    }

    /// Drains the buffered payload into sequenced frames.
    ///
    /// Returns an empty vector when nothing is buffered; the caller must not
    /// open a durable transaction for an empty flush.
    pub fn take_frames(&mut self) -> Vec<PublicationFrame> {
        let payloads = std::mem::take(&mut self.pending);
        self.pending_bytes = 0;
        self.oldest_pending_deadline_millis = None;
        self.seal(payloads)
    }

    /// Drains the buffered payload into the frames of the publication
    /// terminal transaction.
    ///
    /// When no payload remains, a single
    /// [`PublicationPayload::TerminalOnly`] frame carries the terminal
    /// transition, so U always commits a final frame together with its marker
    /// and no visible text is delayed that does not exist.
    pub fn take_terminal_frames(&mut self) -> Vec<PublicationFrame> {
        let mut frames = self.take_frames();
        if frames.is_empty() {
            frames = self.seal(vec![PublicationPayload::TerminalOnly]);
        }
        frames
    }

    fn seal(&mut self, payloads: Vec<PublicationPayload>) -> Vec<PublicationFrame> {
        payloads
            .into_iter()
            .map(|payload| {
                let sequence = self.next_sequence;
                self.next_sequence += 1;
                PublicationFrame {
                    stream_id: self.stream_id.clone(),
                    message_id: self.message_id.clone(),
                    sequence,
                    payload,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{CoalescePolicy, ManualPublicationClock, PublicationClock, PublicationCoalescer};
    use crate::message::types::ContentBlockIndex;
    use crate::publication::frame::PublicationPayload;
    use crate::runtime::identity::{MessageId, PublicationStreamId, ToolCallId, ToolId};
    use crate::tools::types::ToolCallStart;

    fn coalescer(
        policy: CoalescePolicy,
        clock: &Arc<ManualPublicationClock>,
    ) -> PublicationCoalescer {
        PublicationCoalescer::new(
            PublicationStreamId::new("stream-1"),
            MessageId::new("message-1"),
            policy,
            Arc::clone(clock) as Arc<dyn super::PublicationClock>,
        )
    }

    fn text(suffix: &str) -> PublicationPayload {
        PublicationPayload::TextSuffix {
            block_index: ContentBlockIndex::new(0),
            suffix: suffix.to_owned(),
        }
    }

    /// Many small provider deltas coalesce into far fewer durable frames
    /// under the byte threshold.
    #[test]
    fn byte_threshold_coalesces_many_deltas_into_few_frames() {
        let clock = Arc::new(ManualPublicationClock::new());
        let mut coalescer = coalescer(
            CoalescePolicy {
                max_bytes: 16,
                max_latency_millis: 1_000,
            },
            &clock,
        );
        let mut flushes = 0;
        // 64 single-byte deltas at a 16-byte threshold.
        for _ in 0..64 {
            if coalescer.push(text("x")) {
                let frames = coalescer.take_frames();
                assert_eq!(frames.len(), 1, "merged suffixes seal as one frame");
                flushes += 1;
            }
        }
        assert!(coalescer.is_empty());
        assert_eq!(flushes, 4);
        assert_eq!(coalescer.produced_frames(), 4);
    }

    /// Latency flushing is driven by the injected clock alone: no elapsed
    /// wall-clock time is required and none is consulted.
    #[test]
    fn latency_flush_is_deterministic_under_a_manual_clock() {
        let clock = Arc::new(ManualPublicationClock::new());
        let mut coalescer = coalescer(
            CoalescePolicy {
                max_bytes: 4_096,
                max_latency_millis: 50,
            },
            &clock,
        );
        assert!(!coalescer.latency_elapsed(), "nothing buffered yet");
        assert!(!coalescer.push(text("hello")), "below the byte threshold");
        assert!(!coalescer.latency_elapsed());
        clock.advance(49);
        assert!(!coalescer.latency_elapsed());
        clock.advance(1);
        assert!(coalescer.latency_elapsed());
        let frames = coalescer.take_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].sequence, 0);
        assert!(
            !coalescer.latency_elapsed(),
            "the latency window restarts empty"
        );
    }

    /// The oldest buffered payload owns one absolute deadline. A later
    /// payload joins that buffer without extending the original budget.
    #[test]
    fn oldest_payload_deadline_is_not_reset_by_later_payloads() {
        let clock = Arc::new(ManualPublicationClock::new());
        let mut coalescer = coalescer(
            CoalescePolicy {
                max_bytes: usize::MAX,
                max_latency_millis: 50,
            },
            &clock,
        );
        coalescer.push(text("first"));
        assert_eq!(coalescer.latency_deadline_millis(), Some(50));
        clock.advance(49);
        coalescer.push(text("second"));
        assert_eq!(coalescer.latency_deadline_millis(), Some(50));
        assert_eq!(coalescer.latency_remaining_millis(), Some(1));
        assert!(!coalescer.latency_elapsed());
        clock.advance(1);
        assert!(coalescer.latency_elapsed());
        let frames = coalescer.take_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, text("firstsecond"));
    }

    /// Draining the buffer is the only operation that permits a new full
    /// latency window. The next payload gets a deadline relative to its own
    /// entry into the now-empty buffer.
    #[test]
    fn latency_deadline_restarts_only_after_a_successful_drain() {
        let clock = Arc::new(ManualPublicationClock::new());
        let mut coalescer = coalescer(
            CoalescePolicy {
                max_bytes: usize::MAX,
                max_latency_millis: 50,
            },
            &clock,
        );
        coalescer.push(text("first"));
        clock.advance(50);
        let _ = coalescer.take_frames();
        clock.advance(7);
        coalescer.push(text("second"));
        assert_eq!(coalescer.latency_deadline_millis(), Some(107));
        assert_eq!(coalescer.latency_remaining_millis(), Some(50));
    }

    /// Zero latency is immediately elapsed, and deadline arithmetic saturates
    /// rather than wrapping when the monotonic millisecond counter is near its
    /// maximum.
    #[test]
    fn latency_deadline_handles_zero_and_saturating_arithmetic() {
        let clock = Arc::new(ManualPublicationClock::new());
        let mut immediate = coalescer(
            CoalescePolicy {
                max_bytes: usize::MAX,
                max_latency_millis: 0,
            },
            &clock,
        );
        immediate.push(text("now"));
        assert_eq!(immediate.latency_deadline_millis(), Some(0));
        assert!(immediate.latency_elapsed());

        clock.advance(u64::MAX - 1);
        let mut saturated = coalescer(
            CoalescePolicy {
                max_bytes: usize::MAX,
                max_latency_millis: 10,
            },
            &clock,
        );
        saturated.push(text("near max"));
        assert_eq!(saturated.latency_deadline_millis(), Some(u64::MAX));
        assert_eq!(saturated.latency_remaining_millis(), Some(1));
        clock.advance(1);
        assert!(saturated.latency_elapsed());
    }

    /// The clock and wake-up seam remains deterministic even when the
    /// provider is otherwise quiet: advancing the manual clock releases the
    /// absolute-deadline future without a wall-clock sleep.
    #[tokio::test]
    async fn manual_clock_wakes_an_absolute_deadline_future() {
        let clock = Arc::new(ManualPublicationClock::new());
        let wait = clock.wait_until_millis(50);
        let clock_for_task = Arc::clone(&clock);
        let task = tokio::spawn(async move {
            wait.await;
            clock_for_task.now_millis()
        });
        tokio::task::yield_now().await;
        clock.advance(49);
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        clock.advance(1);
        assert_eq!(task.await.expect("deadline waiter"), 50);
    }

    /// A tool-call proposal start is a structural boundary: it forces a flush
    /// and is never merged into the surrounding text.
    #[test]
    fn structural_boundary_forces_its_own_frame() {
        let clock = Arc::new(ManualPublicationClock::new());
        let mut coalescer = coalescer(
            CoalescePolicy {
                max_bytes: 4_096,
                max_latency_millis: 1_000,
            },
            &clock,
        );
        assert!(!coalescer.push(text("before")));
        assert!(coalescer.push(PublicationPayload::ProposedToolCallStarted {
            block_index: ContentBlockIndex::new(1),
            call: ToolCallStart {
                id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-a"),
                name: "alpha".to_owned(),
            },
        }));
        let frames = coalescer.take_frames();
        assert_eq!(frames.len(), 2);
        assert!(matches!(
            frames[0].payload,
            PublicationPayload::TextSuffix { .. }
        ));
        assert!(matches!(
            frames[1].payload,
            PublicationPayload::ProposedToolCallStarted { .. }
        ));
    }

    /// The terminal transaction always has a frame: buffered payload when
    /// there is some, a terminal-only frame when there is not.
    #[test]
    fn terminal_flush_always_produces_a_frame() {
        let clock = Arc::new(ManualPublicationClock::new());
        let mut empty = coalescer(CoalescePolicy::default(), &clock);
        let frames = empty.take_terminal_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, PublicationPayload::TerminalOnly);

        let mut buffered = coalescer(CoalescePolicy::default(), &clock);
        buffered.push(text("tail"));
        let frames = buffered.take_terminal_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, text("tail"));
    }
}
