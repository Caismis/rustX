//! Issue #38: stdio/JSONL framing, bounds, ordering, and lifecycle.
//!
//! These are the transport-specific regressions. Everything semantic lives
//! in `issue38_conformance.rs`, which runs the same scenarios through this
//! transport and through the direct endpoint; this file asserts the things
//! only a byte stream can be wrong about:
//!
//! - framing (LF, CRLF, split/coalesced reads, physical newlines);
//! - the record limit in both directions;
//! - that a malformed or oversized record never reaches the semantic
//!   endpoint and never produces a fabricated protocol record;
//! - output purity, one LF per record, and record ordering;
//! - EOF, truncation, broken pipe, detach, and shutdown lifecycle;
//! - backpressure: a stalled consumer stalls the transport, never the
//!   runtime.
//!
//! # Determinism
//!
//! No test proves anything with a sleep. The model parks on explicit
//! release channels, the gated writer publishes its blocked state on a
//! `watch`, and every wait is a barrier. The one time bound is an outer
//! liveness guard that no assertion depends on.

use super::support;

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use rustx::message::content::TextBlock;
use rustx::message::types::{ContentBlockIndex, MessageBlock, UserContentBlock, UserSource};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime_client::transport::stdio::{
    STDIO_JSONL_MAX_RECORD_BYTES, StdioFramingError, StdioSessionEnd, StdioTransportError,
    serve_stdio_jsonl_with_io,
};
use rustx::runtime_client::{
    RuntimeClientAttemptPhase, RuntimeClientCursor, RuntimeClientEndpoint, RuntimeClientEvent,
    RuntimeClientHost, RuntimeClientOutcome, RuntimeClientProtocolEvent, RuntimeClientResponse,
    RuntimeClientResult,
};
use support::fake::{FakeStep, model_release};
use support::runtime_client_fixture::RuntimeClientFixture;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, ReadBuf};
use tokio::sync::watch;

/// The outer liveness guard of one whole wait. Every wait below is exact;
/// no assertion depends on this value.
const LIVENESS_GUARD: Duration = Duration::from_secs(120);

/// The client pipe capacity for ordinary records.
const PIPE_BYTES: usize = 256 * 1024;

/// The client pipe capacity for record-limit tests, large enough to hold a
/// whole over-limit record without the session having to consume it.
const LARGE_PIPE_BYTES: usize = 2 * STDIO_JSONL_MAX_RECORD_BYTES;

// ---------------------------------------------------------------------------
// Test I/O
// ---------------------------------------------------------------------------

/// The number of complete records in a captured output buffer.
///
/// Records are LF-delimited, so complete records are exactly the delimiters
/// written so far. Test bookkeeping over kilobyte buffers; a vectorized
/// byte counter would be noise here.
#[allow(clippy::naive_bytecount)]
fn count_records(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
}

/// An output sink that records every byte the transport writes.
///
/// `complete` publishes the number of whole records that reached the sink,
/// so a test can wait on wire progress with a `watch` barrier instead of a
/// sleep.
#[derive(Clone)]
struct CapturingSink {
    bytes: Arc<Mutex<Vec<u8>>>,
    complete: watch::Sender<usize>,
}

impl Default for CapturingSink {
    fn default() -> Self {
        Self {
            bytes: Arc::new(Mutex::new(Vec::new())),
            complete: watch::Sender::new(0),
        }
    }
}

impl CapturingSink {
    /// The whole records written so far, ignoring a record still being
    /// written. Safe to call while the session runs.
    fn complete_records(&self) -> Vec<String> {
        let bytes = self.bytes.lock().expect("sink lock").clone();
        let text = String::from_utf8(bytes).expect("the output stream is UTF-8");
        text.split_inclusive('\n')
            .filter(|record| record.ends_with('\n'))
            .map(|record| record[..record.len() - 1].to_owned())
            .collect()
    }

    /// Waits until a whole record satisfying the predicate reached the
    /// sink. Watch-driven: every wakeup is an actual write.
    async fn await_record(&self, mut predicate: impl FnMut(&str) -> bool) {
        let mut updates = self.complete.subscribe();
        tokio::time::timeout(LIVENESS_GUARD, async {
            loop {
                if self
                    .complete_records()
                    .iter()
                    .any(|record| predicate(record))
                {
                    return;
                }
                updates
                    .changed()
                    .await
                    .expect("the sink channel stays open");
            }
        })
        .await
        .expect("the expected record must reach the output stream");
    }
    /// The captured records, split on the record delimiter.
    ///
    /// A trailing empty element would mean a record without its LF, and an
    /// interior empty element a blank line; both are asserted against.
    fn records(&self) -> Vec<String> {
        let bytes = self.bytes.lock().expect("sink lock").clone();
        let text = String::from_utf8(bytes).expect("the output stream is UTF-8");
        if text.is_empty() {
            return Vec::new();
        }
        assert!(
            text.ends_with('\n'),
            "every written record ends with its LF: {text:?}"
        );
        let records: Vec<String> = text[..text.len() - 1]
            .split('\n')
            .map(ToOwned::to_owned)
            .collect();
        for record in &records {
            assert!(!record.is_empty(), "the transport writes no blank records");
        }
        records
    }

    /// The raw captured bytes.
    fn bytes(&self) -> Vec<u8> {
        self.bytes.lock().expect("sink lock").clone()
    }
}

impl AsyncWrite for CapturingSink {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let complete = {
            let mut bytes = self.bytes.lock().expect("sink lock");
            bytes.extend_from_slice(buf);
            count_records(&bytes)
        };
        self.complete.send_replace(complete);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// An output stream that always reports a broken pipe, counting attempts so
/// a retry or a spin would be visible.
#[derive(Clone, Default)]
struct BrokenPipeSink {
    attempts: Arc<Mutex<usize>>,
}

impl AsyncWrite for BrokenPipeSink {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        *self.attempts.lock().expect("attempt lock") += 1;
        Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "the peer closed the output stream",
        )))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// The shared state of a gated output stream.
struct GateState {
    /// Whether writes are currently allowed through.
    open: bool,
    /// Everything written so far.
    bytes: Vec<u8>,
    /// The writer parked on a closed gate.
    waker: Option<Waker>,
}

/// An output stream that can be blocked deterministically.
///
/// Closing the gate parks the transport's current write; the `blocked` and
/// `records` watches let a test wait for the exact moment the transport is
/// stuck, and for exactly how many records reached the peer — with no
/// sleeps anywhere.
#[derive(Clone)]
struct GatedSink {
    state: Arc<Mutex<GateState>>,
    blocked: watch::Sender<bool>,
    records: watch::Sender<usize>,
}

impl GatedSink {
    /// Creates an open gate.
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(GateState {
                open: true,
                bytes: Vec::new(),
                waker: None,
            })),
            blocked: watch::Sender::new(false),
            records: watch::Sender::new(0),
        }
    }

    /// Blocks every subsequent write until [`GatedSink::open`].
    fn close(&self) {
        self.state.lock().expect("gate lock").open = false;
    }

    /// Releases the gate and wakes a parked writer.
    fn open(&self) {
        let waker = {
            let mut state = self.state.lock().expect("gate lock");
            state.open = true;
            state.waker.take()
        };
        self.blocked.send_replace(false);
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Waits until the transport is parked on a blocked write.
    async fn await_blocked(&self) {
        let mut blocked = self.blocked.subscribe();
        tokio::time::timeout(LIVENESS_GUARD, blocked.wait_for(|blocked| *blocked))
            .await
            .expect("the transport must block on the closed gate")
            .expect("the gate channel stays open");
    }

    /// Waits until at least `count` complete records reached the peer.
    async fn await_records(&self, count: usize) {
        let mut records = self.records.subscribe();
        tokio::time::timeout(LIVENESS_GUARD, records.wait_for(|seen| *seen >= count))
            .await
            .expect("the transport must write the expected records")
            .expect("the record channel stays open");
    }

    /// The number of complete records written so far.
    fn record_count(&self) -> usize {
        *self.records.borrow()
    }
}

impl AsyncWrite for GatedSink {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let written = {
            let mut state = self.state.lock().expect("gate lock");
            if !state.open {
                state.waker = Some(cx.waker().clone());
                drop(state);
                self.blocked.send_replace(true);
                return Poll::Pending;
            }
            state.bytes.extend_from_slice(buf);
            count_records(&state.bytes)
        };
        self.records.send_replace(written);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// The shared state of a test-driven input stream.
struct ScriptedInputState {
    /// Bytes made readable but not yet taken by the transport's reader.
    readable: VecDeque<u8>,
    /// How many bytes the transport's reader has taken so far.
    taken: usize,
    /// Whether the stream has been closed (EOF).
    closed: bool,
    /// The reader parked on an exhausted stream.
    waker: Option<Waker>,
}

/// An input stream a test feeds byte-exactly.
///
/// `progress` publishes `(bytes taken by the reader, reader parked waiting
/// for more)` as one value, so a test can establish the exact state "every
/// pushed byte has been consumed and the reader is now waiting for the rest
/// of a record" with a `watch` barrier rather than a sleep. `parked` is only
/// ever published from a poll that found the stream exhausted, so the pair
/// cannot report a park that happened before the last bytes were taken.
#[derive(Clone)]
struct ScriptedInput {
    state: Arc<Mutex<ScriptedInputState>>,
    progress: watch::Sender<(usize, bool)>,
}

impl ScriptedInput {
    /// Creates an open, empty stream.
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ScriptedInputState {
                readable: VecDeque::new(),
                taken: 0,
                closed: false,
                waker: None,
            })),
            progress: watch::Sender::new((0, false)),
        }
    }

    /// Makes `bytes` readable, wakes a parked reader, and reports how many
    /// bytes were pushed.
    fn push(&self, bytes: &[u8]) -> usize {
        let waker = {
            let mut state = self.state.lock().expect("input lock");
            state.readable.extend(bytes.iter().copied());
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        bytes.len()
    }

    /// Closes the stream at EOF and wakes a parked reader.
    fn close(&self) {
        let waker = {
            let mut state = self.state.lock().expect("input lock");
            state.closed = true;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Waits until the reader has taken exactly `taken` bytes and is parked
    /// waiting for more.
    async fn await_parked_after(&self, taken: usize) {
        let mut progress = self.progress.subscribe();
        tokio::time::timeout(
            LIVENESS_GUARD,
            progress.wait_for(|(seen, parked)| *seen == taken && *parked),
        )
        .await
        .expect("the reader must consume the pushed bytes and wait for more")
        .expect("the progress channel stays open");
    }
}

impl AsyncRead for ScriptedInput {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut state = self.state.lock().expect("input lock");
        if state.readable.is_empty() {
            if state.closed {
                return Poll::Ready(Ok(()));
            }
            state.waker = Some(cx.waker().clone());
            let taken = state.taken;
            drop(state);
            self.progress.send_replace((taken, true));
            return Poll::Pending;
        }
        let count = state.readable.len().min(buf.remaining());
        let chunk: Vec<u8> = state.readable.drain(..count).collect();
        buf.put_slice(&chunk);
        state.taken += count;
        let taken = state.taken;
        drop(state);
        self.progress.send_replace((taken, false));
        Poll::Ready(Ok(()))
    }
}

// ---------------------------------------------------------------------------
// Session harness
// ---------------------------------------------------------------------------

/// The outcome of one captured stdio session.
struct SessionOutcome {
    /// The records the transport wrote.
    records: Vec<String>,
    /// The raw bytes the transport wrote.
    bytes: Vec<u8>,
    /// The session result.
    result: Result<StdioSessionEnd, StdioTransportError>,
}

/// Runs one stdio session over the given input byte chunks and returns
/// everything it wrote plus its outcome.
///
/// Each chunk is one underlying write, so a caller controls exactly how
/// records are split across or coalesced into reads. The input stream is
/// closed after the last chunk.
async fn run_session(
    endpoint: RuntimeClientEndpoint,
    chunks: &[&[u8]],
    pipe_bytes: usize,
) -> SessionOutcome {
    let sink = CapturingSink::default();
    let (mut client, session_input) = tokio::io::duplex(pipe_bytes);
    let session = tokio::spawn({
        let sink = sink.clone();
        async move { serve_stdio_jsonl_with_io(endpoint, session_input, sink).await }
    });
    for chunk in chunks {
        // A session that already failed has dropped its reader; the write
        // failing is exactly that, and the assertion is on the outcome.
        if client.write_all(chunk).await.is_err() {
            break;
        }
        let _ = client.flush().await;
    }
    drop(client);
    let result = tokio::time::timeout(LIVENESS_GUARD, session)
        .await
        .expect("the session must terminate")
        .expect("the session task must not panic");
    SessionOutcome {
        records: sink.records(),
        bytes: sink.bytes(),
        result,
    }
}

/// One `initialize` record.
fn initialize_record(id: u64) -> Vec<u8> {
    format!("{{\"method\":\"initialize\",\"id\":{id},\"protocol_version\":1}}\n").into_bytes()
}

/// Parses one captured record as a response.
fn as_response(record: &str) -> RuntimeClientResponse {
    serde_json::from_str(record).expect("the record is a Runtime Client response")
}

/// Parses one captured record as a notification.
fn as_event(record: &str) -> RuntimeClientProtocolEvent {
    serde_json::from_str(record).expect("the record is a Runtime Client notification")
}

/// Whether a captured record is a notification.
fn is_event(record: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(record)
        .expect("the record is JSON")
        .get("cursor")
        .is_some()
}

/// A host with no scripted turns.
async fn idle_host(conversation: &str) -> RuntimeClientHost {
    RuntimeClientFixture::builder(conversation)
        .build()
        .await
        .into_parts()
        .1
}

// ---------------------------------------------------------------------------
// Input framing
// ---------------------------------------------------------------------------

/// LF delimits records: several records coalesced into one underlying write
/// are separated exactly, and each produces exactly one response.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coalesced_records_in_one_read_are_separated() {
    let host = idle_host("conv-38-coalesced").await;
    let mut input = initialize_record(1);
    input.extend_from_slice(b"{\"method\":\"snapshot_get\",\"id\":2}\n");
    input.extend_from_slice(b"{\"method\":\"capability_get\",\"id\":3}\n");
    let outcome = run_session(host.endpoint(), &[&input], PIPE_BYTES).await;

    assert!(matches!(outcome.result, Ok(StdioSessionEnd::InputEof)));
    assert_eq!(outcome.records.len(), 3);
    for (index, record) in outcome.records.iter().enumerate() {
        let response = as_response(record);
        assert_eq!(response.id.get(), index as u64 + 1);
        assert!(response.error.is_none());
    }
}

/// One record split across many underlying reads is reassembled exactly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_split_across_reads_is_reassembled() {
    let host = idle_host("conv-38-split").await;
    let record = initialize_record(1);
    let chunks: Vec<&[u8]> = record.chunks(3).collect();
    let outcome = run_session(host.endpoint(), &chunks, PIPE_BYTES).await;

    assert!(matches!(outcome.result, Ok(StdioSessionEnd::InputEof)));
    assert_eq!(outcome.records.len(), 1);
    assert!(matches!(
        as_response(&outcome.records[0]).result,
        Some(RuntimeClientResult::Initialized { .. })
    ));
}

/// CRLF input is accepted by removing exactly one terminal CR.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crlf_records_are_accepted() {
    let host = idle_host("conv-38-crlf").await;
    let outcome = run_session(
        host.endpoint(),
        &[
            b"{\"method\":\"initialize\",\"id\":1,\"protocol_version\":1}\r\n",
            b"{\"method\":\"snapshot_get\",\"id\":2}\r\n",
        ],
        PIPE_BYTES,
    )
    .await;

    assert!(matches!(outcome.result, Ok(StdioSessionEnd::InputEof)));
    assert_eq!(outcome.records.len(), 2);
    assert!(as_response(&outcome.records[1]).error.is_none());
    // The transport does not answer in CRLF: LF is the sole delimiter.
    assert!(!outcome.bytes.contains(&b'\r'));
}

/// An escaped newline inside a JSON string stays inside one record and
/// reaches canonical history unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_escaped_newline_stays_inside_one_record() {
    let fixture = RuntimeClientFixture::builder("conv-38-escaped")
        .build()
        .await;
    let outcome = run_session(
        fixture.host.endpoint(),
        &[
            &initialize_record(1),
            br#"{"method":"submit_inbound","id":2,"content":[{"type":"text","text":"a\nb"}]}"#,
            b"\n",
        ],
        PIPE_BYTES,
    )
    .await;

    assert!(matches!(outcome.result, Ok(StdioSessionEnd::InputEof)));
    assert_eq!(outcome.records.len(), 2);
    assert!(matches!(
        as_response(&outcome.records[1]).result,
        Some(RuntimeClientResult::InboundAccepted { .. })
    ));
    let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
    let MessageBlock::User(message) = &snapshot.messages[0] else {
        panic!("the inbound message committed");
    };
    assert_eq!(
        message.content,
        vec![UserContentBlock::Text(TextBlock {
            text: "a\nb".to_owned()
        })]
    );
}

/// A physical newline terminates the record, so pretty-printed multiline
/// JSON fails as the first malformed record and applies nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn physical_multiline_json_is_a_framing_failure() {
    let fixture = RuntimeClientFixture::builder("conv-38-multiline")
        .build()
        .await;
    let outcome = run_session(
        fixture.host.endpoint(),
        &[
            &initialize_record(1),
            b"{\n  \"method\": \"snapshot_get\",\n  \"id\": 2\n}\n",
        ],
        PIPE_BYTES,
    )
    .await;

    assert!(matches!(
        outcome.result,
        Err(StdioTransportError::Framing(
            StdioFramingError::MalformedRecord { .. }
        ))
    ));
    assert_eq!(
        outcome.records.len(),
        1,
        "only the initialize response was written"
    );
}

/// Every structurally invalid record is transport-fatal, applies nothing,
/// and produces no fabricated protocol record.
///
/// Protocol v1 has no uncorrelated error envelope; a malformed frame may
/// not even carry a request id, so inventing one would be a second
/// protocol.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_records_are_fatal_and_write_nothing() {
    let cases: [(&str, &[u8]); 7] = [
        ("empty", b"\n"),
        ("whitespace", b"   \n"),
        ("not-json", b"not json\n"),
        ("array", b"[]\n"),
        ("unknown-method", br#"{"method":"future_method","id":2}"#),
        (
            "unknown-field",
            br#"{"method":"snapshot_get","id":2,"extra":true}"#,
        ),
        (
            "wrong-type",
            br#"{"method":"initialize","id":"two","protocol_version":1}"#,
        ),
    ];
    for (name, record) in cases {
        let fixture = RuntimeClientFixture::builder(&format!("conv-38-invalid-{name}"))
            .build()
            .await;
        let mut chunk = record.to_vec();
        if !chunk.ends_with(b"\n") {
            chunk.push(b'\n');
        }
        let outcome = run_session(
            fixture.host.endpoint(),
            &[&initialize_record(1), &chunk],
            PIPE_BYTES,
        )
        .await;

        assert!(
            matches!(
                outcome.result,
                Err(StdioTransportError::Framing(
                    StdioFramingError::MalformedRecord { .. }
                ))
            ),
            "case {name}: expected a framing failure, got {:?}",
            outcome.result
        );
        assert_eq!(
            outcome.records.len(),
            1,
            "case {name}: no protocol record is fabricated for a malformed frame"
        );
        assert!(
            fixture
                .host
                .snapshot()
                .expect("snapshot")
                .0
                .messages
                .is_empty(),
            "case {name}: a malformed record applies nothing"
        );
    }
}

/// EOF at a record boundary is normal closure; EOF with a partial record is
/// an explicit truncation error. Neither is cancellation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eof_distinguishes_a_boundary_from_a_truncated_record() {
    let host = idle_host("conv-38-eof-clean").await;
    let outcome = run_session(host.endpoint(), &[&initialize_record(1)], PIPE_BYTES).await;
    assert!(matches!(outcome.result, Ok(StdioSessionEnd::InputEof)));
    assert_eq!(outcome.records.len(), 1);

    let host = idle_host("conv-38-eof-truncated").await;
    let mut truncated = initialize_record(1);
    truncated.extend_from_slice(br#"{"method":"snapshot_get","id":2}"#);
    let outcome = run_session(host.endpoint(), &[&truncated], PIPE_BYTES).await;
    assert!(matches!(
        outcome.result,
        Err(StdioTransportError::Framing(
            StdioFramingError::TruncatedRecord { .. }
        ))
    ));
    assert_eq!(
        outcome.records.len(),
        1,
        "the truncated record was never applied"
    );
}

// ---------------------------------------------------------------------------
// Reader cancellation safety
// ---------------------------------------------------------------------------

/// A half-read record survives losing the session loop's `tokio::select!`
/// race to an event delivery.
///
/// The session drops the in-flight `next_record` future every time the
/// subscription branch wins. The reader's accumulation state therefore lives
/// in the reader, not in that future, and its only await reads nothing when
/// it is dropped while pending — so a partially consumed record is neither
/// lost, duplicated, nor corrupted by an interleaved event.
///
/// The interleaving is established, never timed:
///
/// 1. `initialize` + `subscribe_events` make the subscription branch live;
/// 2. only a prefix of the next request is fed, with no terminating LF;
/// 3. the input stream's `(bytes taken, reader parked)` watch proves the
///    prefix was consumed and the reader is waiting for more bytes, which
///    is exactly the state in which the input future is pending;
/// 4. a Runtime Client event is published out of band, so the subscription
///    branch is the only ready one and wins the select, dropping the input
///    future mid-record;
/// 5. the event reaching the output stream proves that happened;
/// 6. the suffix and its LF are released;
/// 7. the original request decodes whole — its id came from the prefix —
///    and receives its correlated response.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_partly_read_record_survives_an_event_winning_the_select() {
    /// The prefix of the pending request, carrying its correlation id: a
    /// response for it can only exist if these exact bytes survived.
    const REQUEST_PREFIX: &[u8] = br#"{"method":"snapshot_get","id":7"#;
    /// The rest of that request, released only after the event was written.
    const REQUEST_SUFFIX: &[u8] = b"}\n";

    let fixture = RuntimeClientFixture::builder("conv-38-partial-record")
        .build()
        .await;
    let sink = CapturingSink::default();
    let input = ScriptedInput::new();
    let endpoint = fixture.host.endpoint();
    let session = tokio::spawn({
        let sink = sink.clone();
        let input = input.clone();
        async move { serve_stdio_jsonl_with_io(endpoint, input, sink).await }
    });

    // 1. Attach and subscribe, so the session selects over both arms.
    let mut fed = input.push(&initialize_record(1));
    fed += input.push(b"{\"method\":\"subscribe_events\",\"id\":2,\"after_cursor\":0}\n");
    sink.await_record(|record| !is_event(record) && as_response(record).id.get() == 2)
        .await;

    // 2/3. Feed only a prefix of the next request, then establish that the
    //      reader consumed it and is waiting for the rest: the input future
    //      is pending with a partial record accumulated.
    fed += input.push(REQUEST_PREFIX);
    input.await_parked_after(fed).await;

    // 4/5. Publish one event out of band. The input arm is pending, so the
    //      subscription arm wins the select and the input future is dropped
    //      mid-record; the event reaching the wire is the proof.
    assert_eq!(
        fixture.host.shutdown(),
        RuntimeClientResult::ShutdownAccepted
    );
    sink.await_record(|record| {
        is_event(record) && matches!(as_event(record).event, RuntimeClientEvent::RuntimeShutdown)
    })
    .await;
    let written = sink.complete_records();
    assert_eq!(
        written.len(),
        3,
        "exactly the two responses and the event: the partial record was \
         not answered, fabricated, or split, got {written:?}"
    );
    assert!(
        is_event(&written[2]),
        "the event overtook a record the transport had already partly read"
    );

    // 6/7. Release the suffix. The request the reader was midway through
    //      decodes whole and gets its correlated response.
    input.push(REQUEST_SUFFIX);
    sink.await_record(|record| !is_event(record) && as_response(record).id.get() == 7)
        .await;
    input.close();
    let result = tokio::time::timeout(LIVENESS_GUARD, session)
        .await
        .expect("the session must terminate")
        .expect("the session task must not panic");
    assert!(matches!(result, Ok(StdioSessionEnd::InputEof)));

    let records = sink.records();
    assert_eq!(
        records.len(),
        4,
        "the interrupted record produced exactly one response: {records:?}"
    );
    let response = as_response(&records[3]);
    assert_eq!(response.id.get(), 7, "the prefix's id survived the drop");
    assert!(
        matches!(response.result, Some(RuntimeClientResult::Snapshot { .. })),
        "the reassembled record decoded to the original request, got {:?}",
        response.result
    );
}

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// Builds a `submit_inbound` record whose JSON payload is exactly `bytes`
/// long, padding the message text.
fn sized_submit_record(id: u64, bytes: usize) -> Vec<u8> {
    let empty = format!(
        r#"{{"method":"submit_inbound","id":{id},"content":[{{"type":"text","text":""}}]}}"#
    );
    let padding = bytes
        .checked_sub(empty.len())
        .expect("the requested size holds the envelope");
    let mut record = empty.replace(
        r#""text":"""#,
        &format!(r#""text":"{}""#, "x".repeat(padding)),
    );
    assert_eq!(record.len(), bytes);
    record.push('\n');
    record.into_bytes()
}

/// A record of exactly the limit is accepted; the LF is not counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_record_at_the_limit_is_accepted() {
    let fixture = RuntimeClientFixture::builder("conv-38-at-limit")
        .build()
        .await;
    let record = sized_submit_record(2, STDIO_JSONL_MAX_RECORD_BYTES);
    assert_eq!(record.len(), STDIO_JSONL_MAX_RECORD_BYTES + 1);
    let outcome = run_session(
        fixture.host.endpoint(),
        &[&initialize_record(1), &record],
        LARGE_PIPE_BYTES,
    )
    .await;

    assert!(matches!(outcome.result, Ok(StdioSessionEnd::InputEof)));
    assert_eq!(outcome.records.len(), 2);
    assert!(matches!(
        as_response(&outcome.records[1]).result,
        Some(RuntimeClientResult::InboundAccepted { .. })
    ));
}

/// One byte over the limit is rejected before any semantic dispatch, and no
/// part of the oversized request is applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_record_over_the_limit_is_rejected_without_dispatch() {
    let fixture = RuntimeClientFixture::builder("conv-38-over-limit")
        .build()
        .await;
    let record = sized_submit_record(2, STDIO_JSONL_MAX_RECORD_BYTES + 1);
    let outcome = run_session(
        fixture.host.endpoint(),
        &[&initialize_record(1), &record],
        LARGE_PIPE_BYTES,
    )
    .await;

    assert!(
        matches!(
            outcome.result,
            Err(StdioTransportError::Framing(
                StdioFramingError::RecordTooLarge {
                    limit: STDIO_JSONL_MAX_RECORD_BYTES
                }
            ))
        ),
        "got {:?}",
        outcome.result
    );
    assert_eq!(
        outcome.records.len(),
        1,
        "no response was fabricated for the oversized record"
    );
    assert!(
        fixture
            .host
            .snapshot()
            .expect("snapshot")
            .0
            .messages
            .is_empty(),
        "an oversized record never partially applies a request"
    );
}

/// An outbound record over the limit terminates the session without
/// truncating it and without splitting it across records.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_oversized_outbound_record_terminates_without_truncating() {
    // The seeded canonical history alone exceeds the record limit, so the
    // `initialize` response (which carries the linearized snapshot) cannot
    // be framed.
    let huge = "y".repeat(STDIO_JSONL_MAX_RECORD_BYTES + 1024);
    let fixture = RuntimeClientFixture::builder("conv-38-huge-output")
        .initial_messages(vec![MessageBlock::User(support::fake::inbound_message(
            "seed",
            &huge,
            UserSource::Human,
        ))])
        .build()
        .await;
    let outcome = run_session(
        fixture.host.endpoint(),
        &[&initialize_record(1)],
        LARGE_PIPE_BYTES,
    )
    .await;

    assert!(
        matches!(
            outcome.result,
            Err(StdioTransportError::OutboundRecordTooLarge {
                limit: STDIO_JSONL_MAX_RECORD_BYTES
            })
        ),
        "got {:?}",
        outcome.result
    );
    assert!(
        outcome.bytes.is_empty(),
        "nothing partial or split reached the output stream"
    );
}

// ---------------------------------------------------------------------------
// Output purity and ordering
// ---------------------------------------------------------------------------

/// The output stream carries protocol only: every record is exactly one
/// Runtime Client protocol object followed by one LF, no record interleaves
/// with another, and no human text is ever written.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_output_stream_is_protocol_only_and_ordered() {
    let fixture = RuntimeClientFixture::builder("conv-38-purity")
        .script(vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ])
        .build()
        .await;

    // Drive a whole turn, then close input once the attempt settled so the
    // captured stream is complete and deterministic.
    let sink = CapturingSink::default();
    let (mut client, session_input) = tokio::io::duplex(PIPE_BYTES);
    let endpoint = fixture.host.endpoint();
    let session = tokio::spawn({
        let sink = sink.clone();
        async move { serve_stdio_jsonl_with_io(endpoint, session_input, sink).await }
    });
    client
        .write_all(&initialize_record(1))
        .await
        .expect("write initialize");
    client
        .write_all(b"{\"method\":\"subscribe_events\",\"id\":2,\"after_cursor\":0}\n")
        .await
        .expect("write subscribe");
    client
        .write_all(br#"{"method":"submit_inbound","id":3,"content":[{"type":"text","text":"hi"}]}"#)
        .await
        .expect("write submit");
    client.write_all(b"\n").await.expect("write delimiter");

    // Barrier: wait until the settlement record reached the wire, so the
    // captured stream is complete before EOF stops servicing.
    sink.await_record(|record| {
        is_event(record)
            && matches!(
                as_event(record).event,
                RuntimeClientEvent::AttemptSettled { .. }
            )
    })
    .await;
    drop(client);
    let result = tokio::time::timeout(LIVENESS_GUARD, session)
        .await
        .expect("the session must terminate")
        .expect("the session task must not panic");
    assert!(matches!(result, Ok(StdioSessionEnd::InputEof)));

    // `CapturingSink::records` already asserted exactly one trailing LF per
    // record and no blank records. Every record parsing exactly is also the
    // proof that two records' bytes never interleaved.
    let records = sink.records();
    let mut responses = Vec::new();
    let mut cursors = Vec::new();
    let mut subscribe_index = None;
    let mut first_event_index = None;
    for (index, record) in records.iter().enumerate() {
        if is_event(record) {
            let event = as_event(record);
            cursors.push(event.cursor);
            first_event_index.get_or_insert(index);
        } else {
            let response = as_response(record);
            if matches!(
                response.result,
                Some(RuntimeClientResult::Subscribed { .. })
            ) {
                subscribe_index = Some(index);
            }
            responses.push(response.id.get());
        }
    }
    assert_eq!(responses, vec![1, 2, 3], "response ids correlate exactly");
    assert!(
        cursors.windows(2).all(|pair| pair[1] > pair[0]),
        "notification cursors preserve RuntimeClientCursor order: {cursors:?}"
    );
    assert!(
        cursors
            .windows(2)
            .all(|pair| pair[1].get() == pair[0].get() + 1),
        "one subscription observes strictly contiguous cursors: {cursors:?}"
    );
    assert_eq!(
        cursors.first().copied(),
        Some(RuntimeClientCursor::new(1)),
        "the subscription resumed after cursor 0"
    );
    assert!(
        subscribe_index < first_event_index,
        "the subscribe response precedes the first event of that subscription"
    );
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Input EOF detaches the attachment and nothing else: a running attempt is
/// neither cancelled nor settled, and a fresh connection can attach and
/// watch that same attempt finish normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eof_detaches_without_cancelling_the_running_attempt() {
    let (release, release_rx) = model_release();
    let fixture = RuntimeClientFixture::builder("conv-38-eof-detach")
        .script(vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ])
        .build()
        .await;

    let outcome = run_session(
        fixture.host.endpoint(),
        &[
            &initialize_record(1),
            br#"{"method":"submit_inbound","id":2,"content":[{"type":"text","text":"go"}]}"#,
            b"\n",
        ],
        PIPE_BYTES,
    )
    .await;
    assert!(matches!(outcome.result, Ok(StdioSessionEnd::InputEof)));
    assert_eq!(outcome.records.len(), 2);

    // Barrier: the model parked, so the attempt is provably mid-flight.
    let mut parked = fixture.model.parked();
    tokio::time::timeout(LIVENESS_GUARD, parked.wait_for(|parked| *parked))
        .await
        .expect("the model must park")
        .expect("the park channel stays open");
    let (snapshot, cursor) = fixture.host.snapshot().expect("snapshot");
    assert!(
        !matches!(
            snapshot.attempt.as_ref().expect("an attempt exists").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ),
        "EOF neither cancelled nor settled the attempt"
    );

    // The attachment was released, so a fresh connection attaches — and it
    // observes the same attempt reaching its own natural settlement.
    let endpoint = fixture.host.endpoint();
    let response =
        endpoint.handle_request(rustx::runtime_client::RuntimeClientRequest::Initialize {
            id: rustx::runtime_client::RequestId::new(1),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        });
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::Initialized { .. })
        ),
        "the dropped session released the attachment"
    );
    let response = endpoint.handle_request(
        rustx::runtime_client::RuntimeClientRequest::SubscribeEvents {
            id: rustx::runtime_client::RequestId::new(2),
            after_cursor: cursor,
        },
    );
    assert!(response.error.is_none());

    // Release the parked attempt so it terminates on its own terms.
    release.send_replace(true);
    let settled = tokio::time::timeout(LIVENESS_GUARD, async {
        loop {
            let rustx::runtime_client::EventDelivery::Event(event) = endpoint.next_event().await
            else {
                panic!("the subscription stays open");
            };
            if let RuntimeClientEvent::AttemptSettled { outcome, .. } = event.event {
                return outcome;
            }
        }
    })
    .await
    .expect("the attempt must settle");
    assert!(
        matches!(settled, RuntimeClientOutcome::Completed { .. }),
        "the attempt completed normally; transport loss never cancelled it"
    );
}

/// A broken output pipe is clean peer transport loss: the session ends
/// normally, detaches, retries nothing, and cancels nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broken_output_pipe_detaches_without_retrying() {
    let (release, release_rx) = model_release();
    let fixture = RuntimeClientFixture::builder("conv-38-broken-pipe")
        .script(vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ])
        .build()
        .await;
    // A running attempt the transport must not disturb.
    fixture
        .host
        .submit_inbound(vec![UserContentBlock::Text(TextBlock {
            text: "go".to_owned(),
        })])
        .expect("submit");
    let mut parked = fixture.model.parked();
    tokio::time::timeout(LIVENESS_GUARD, parked.wait_for(|parked| *parked))
        .await
        .expect("the model must park")
        .expect("the park channel stays open");

    let sink = BrokenPipeSink::default();
    let (mut client, session_input) = tokio::io::duplex(PIPE_BYTES);
    let endpoint = fixture.host.endpoint();
    let session = tokio::spawn({
        let sink = sink.clone();
        async move { serve_stdio_jsonl_with_io(endpoint, session_input, sink).await }
    });
    client
        .write_all(&initialize_record(1))
        .await
        .expect("write initialize");
    let result = tokio::time::timeout(LIVENESS_GUARD, session)
        .await
        .expect("the session must terminate")
        .expect("the session task must not panic");
    drop(client);

    assert!(matches!(result, Ok(StdioSessionEnd::OutputBrokenPipe)));
    assert_eq!(
        *sink.attempts.lock().expect("attempt lock"),
        1,
        "a failed write is never retried and never spins"
    );

    // Detached, and the attempt is untouched.
    let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
    assert!(!matches!(
        snapshot.attempt.expect("an attempt exists").phase,
        RuntimeClientAttemptPhase::Settled { .. }
    ));
    let endpoint = fixture.host.endpoint();
    let response =
        endpoint.handle_request(rustx::runtime_client::RuntimeClientRequest::Initialize {
            id: rustx::runtime_client::RequestId::new(1),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        });
    assert!(matches!(
        response.result,
        Some(RuntimeClientResult::Initialized { .. })
    ));
    release.send_replace(true);
}

/// An explicit `detach` does not close the transport: the connection stays
/// open, stops pumping notifications, and may re-initialize into a fresh
/// attachment identity on the same byte stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_keeps_the_byte_stream_open() {
    let host = idle_host("conv-38-detach").await;
    let outcome = run_session(
        host.endpoint(),
        &[
            &initialize_record(1),
            b"{\"method\":\"subscribe_events\",\"id\":2,\"after_cursor\":0}\n",
            b"{\"method\":\"detach\",\"id\":3}\n",
            b"{\"method\":\"snapshot_get\",\"id\":4}\n",
            &initialize_record(5),
        ],
        PIPE_BYTES,
    )
    .await;

    assert!(matches!(outcome.result, Ok(StdioSessionEnd::InputEof)));
    assert_eq!(
        outcome.records.len(),
        5,
        "the session kept serving after detach"
    );
    let Some(RuntimeClientResult::Initialized {
        attachment_id: first,
        ..
    }) = as_response(&outcome.records[0]).result
    else {
        panic!("initialized");
    };
    assert_eq!(
        as_response(&outcome.records[2]).result,
        Some(RuntimeClientResult::Detached)
    );
    assert_eq!(
        as_response(&outcome.records[3]).error,
        Some(rustx::runtime_client::RuntimeClientError::NotAttached)
    );
    let Some(RuntimeClientResult::Initialized {
        attachment_id: second,
        ..
    }) = as_response(&outcome.records[4]).result
    else {
        panic!("re-initialize on the same byte stream succeeds");
    };
    assert_ne!(first, second, "reconnecting receives a fresh identity");
    assert!(
        !outcome.records.iter().any(|record| is_event(record)),
        "no notification is pumped while unattached"
    );
}

/// A successful `shutdown` is answered and the session keeps serving: the
/// runtime shutdown observation arrives on the stream, reads still work,
/// new inbound is the typed semantic error, and only a later EOF ends the
/// transport.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_does_not_close_the_transport() {
    let fixture = RuntimeClientFixture::builder("conv-38-shutdown")
        .build()
        .await;
    let sink = CapturingSink::default();
    let (mut client, session_input) = tokio::io::duplex(PIPE_BYTES);
    let endpoint = fixture.host.endpoint();
    let session = tokio::spawn({
        let sink = sink.clone();
        async move { serve_stdio_jsonl_with_io(endpoint, session_input, sink).await }
    });

    for record in [
        initialize_record(1),
        b"{\"method\":\"subscribe_events\",\"id\":2,\"after_cursor\":0}\n".to_vec(),
        b"{\"method\":\"shutdown\",\"id\":3}\n".to_vec(),
    ] {
        client.write_all(&record).await.expect("write record");
    }

    // Barrier: the runtime shutdown observation reached the wire. Input is
    // biased over notifications, so the remaining requests are written only
    // after this, keeping the captured stream deterministic.
    sink.await_record(|record| {
        is_event(record) && matches!(as_event(record).event, RuntimeClientEvent::RuntimeShutdown)
    })
    .await;

    for record in [
        b"{\"method\":\"snapshot_get\",\"id\":4}\n".to_vec(),
        br#"{"method":"submit_inbound","id":5,"content":[{"type":"text","text":"late"}]}"#.to_vec(),
        b"\n".to_vec(),
        b"{\"method\":\"capability_get\",\"id\":6}\n".to_vec(),
    ] {
        client.write_all(&record).await.expect("write record");
    }
    sink.await_record(|record| !is_event(record) && as_response(record).id.get() == 6)
        .await;

    // Only EOF ends the transport — not the accepted shutdown.
    drop(client);
    let result = tokio::time::timeout(LIVENESS_GUARD, session)
        .await
        .expect("the session must terminate")
        .expect("the session task must not panic");
    assert!(matches!(result, Ok(StdioSessionEnd::InputEof)));

    let records = sink.records();
    let responses: Vec<RuntimeClientResponse> = records
        .iter()
        .filter(|record| !is_event(record))
        .map(|record| as_response(record))
        .collect();
    assert_eq!(
        responses.iter().map(|r| r.id.get()).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6]
    );
    assert_eq!(
        responses[2].result,
        Some(RuntimeClientResult::ShutdownAccepted)
    );
    assert!(
        matches!(
            responses[3].result,
            Some(RuntimeClientResult::Snapshot { .. })
        ),
        "reads still work after shutdown"
    );
    assert_eq!(
        responses[4].error,
        Some(rustx::runtime_client::RuntimeClientError::RuntimeShutdown),
        "new inbound is the typed semantic error, not a transport decision"
    );
    assert!(matches!(
        responses[5].result,
        Some(RuntimeClientResult::Capability { .. })
    ));

    // Shutdown is not cancellation: canonical history is untouched.
    assert!(
        fixture
            .host
            .snapshot()
            .expect("snapshot")
            .0
            .messages
            .is_empty()
    );
}

/// Backpressure: a blocked output consumer stalls the transport, never the
/// runtime.
///
/// The transport has no outbound queue, so a stalled consumer cannot grow
/// transport memory — it costs exactly the one record in flight. The
/// runtime meanwhile keeps admitting inbound, running attempts, and
/// publishing into its own bounded replay ring. When the consumer finally
/// resumes, the transport discovers its subscription fell behind that ring;
/// Protocol v1 has no uncorrelated stream-error record, so the session
/// terminates with the typed local lag error and the client repairs from an
/// authoritative snapshot after reconnecting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete backpressure lifecycle
async fn a_blocked_consumer_stalls_the_transport_not_the_runtime() {
    let turn = |text: &str| {
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: text.to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]
    };
    let fixture = RuntimeClientFixture::builder("conv-38-backpressure")
        // A deliberately tiny replay ring, so a stalled consumer provably
        // falls behind retention rather than merely lagging.
        .replay_limit(Some(2))
        .script(turn("first"))
        .script(turn("second"))
        .build()
        .await;
    let scripted_events = 6_u64;

    let sink = GatedSink::new();
    let (mut client, session_input) = tokio::io::duplex(PIPE_BYTES);
    let endpoint = fixture.host.endpoint();
    let session = tokio::spawn({
        let sink = sink.clone();
        async move { serve_stdio_jsonl_with_io(endpoint, session_input, sink).await }
    });

    // 1. The gate is open: initialize and subscribe get their responses.
    client
        .write_all(&initialize_record(1))
        .await
        .expect("write initialize");
    client
        .write_all(b"{\"method\":\"subscribe_events\",\"id\":2,\"after_cursor\":0}\n")
        .await
        .expect("write subscribe");
    sink.await_records(2).await;

    // 2. Block the consumer, then send one more request. Nothing has been
    //    published yet, so the input arm is the only ready one: the
    //    transport handles the request semantically and then parks on the
    //    blocked write of its response.
    sink.close();
    client
        .write_all(b"{\"method\":\"snapshot_get\",\"id\":3}\n")
        .await
        .expect("write snapshot_get");
    sink.await_blocked().await;

    // 3. Submit work through the runtime harness while the transport is
    //    provably stuck: the runtime must not depend on its consumer. The
    //    second submit lands only after the first attempt settled, so the
    //    second message provably enters the next attempt rather than the
    //    first attempt's batch.
    let mut emitted = fixture.model.emitted();
    fixture
        .host
        .submit_inbound(vec![UserContentBlock::Text(TextBlock {
            text: "first".to_owned(),
        })])
        .expect("the runtime keeps admitting inbound while the transport stalls");
    tokio::time::timeout(LIVENESS_GUARD, emitted.wait_for(|count| *count >= 3))
        .await
        .expect("the first attempt must run to settlement under the stall")
        .expect("the emitted channel stays open");
    fixture
        .host
        .submit_inbound(vec![UserContentBlock::Text(TextBlock {
            text: "second".to_owned(),
        })])
        .expect("the runtime keeps admitting inbound while the transport stalls");

    // 4. The runtime ran to terminal settlement and beyond while stdout was
    //    blocked: the second model invocation only happens after the first
    //    attempt settled and the queued message drained, so reaching every
    //    scripted event proves settlement occurred under the stall.
    tokio::time::timeout(
        LIVENESS_GUARD,
        emitted.wait_for(|count| *count >= scripted_events),
    )
    .await
    .expect("the runtime must keep executing while the transport is stalled")
    .expect("the emitted channel stays open");
    let (progressed, _) = fixture.host.snapshot().expect("snapshot");
    assert!(
        progressed.messages.len() >= 3,
        "the first attempt settled and the queued message drained while the \
         consumer was blocked, got {} messages",
        progressed.messages.len()
    );

    // 5. The transport grew nothing: still exactly the two records it had
    //    written before the block, plus the one record in flight. There is
    //    no outbound queue for a backlog to accumulate in, so a stalled
    //    consumer cannot grow transport memory.
    assert_eq!(
        sink.record_count(),
        2,
        "a stalled transport queues no protocol records"
    );

    // 6. Release the consumer. The blocked response completes, and the
    //    transport then discovers its subscription fell behind the bounded
    //    replay ring. Protocol v1 has no uncorrelated stream-error record,
    //    so the session ends with the typed local lag error.
    sink.open();
    let result = tokio::time::timeout(LIVENESS_GUARD, session)
        .await
        .expect("the session must terminate")
        .expect("the session task must not panic");
    let Err(StdioTransportError::SubscriptionLagged {
        after_cursor,
        earliest_serviceable,
    }) = result
    else {
        panic!("expected the typed transport lag error, got {result:?}");
    };
    assert!(earliest_serviceable > after_cursor);
    assert_eq!(
        sink.record_count(),
        3,
        "the blocked record completed once, and nothing was queued behind it"
    );
    drop(client);

    // 7. Transport loss is not cancellation: the committed conversation
    //    stands and no attempt was cancelled.
    let (final_snapshot, _) = fixture.host.snapshot().expect("snapshot");
    assert!(final_snapshot.messages.len() >= 3);
    if let RuntimeClientAttemptPhase::Settled { outcome } =
        final_snapshot.attempt.expect("an attempt exists").phase
    {
        assert!(
            !matches!(outcome, RuntimeClientOutcome::Cancelled { .. }),
            "transport lag never cancels runtime work"
        );
    }

    // 8. The client repairs by reconnecting and taking a fresh snapshot at
    //    a serviceable cursor.
    let endpoint = fixture.host.endpoint();
    let response =
        endpoint.handle_request(rustx::runtime_client::RuntimeClientRequest::Initialize {
            id: rustx::runtime_client::RequestId::new(1),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        });
    let Some(RuntimeClientResult::Initialized { cursor, .. }) = response.result else {
        panic!("the lagged transport released its attachment");
    };
    let response = endpoint.handle_request(
        rustx::runtime_client::RuntimeClientRequest::SubscribeEvents {
            id: rustx::runtime_client::RequestId::new(2),
            after_cursor: cursor,
        },
    );
    assert!(
        response.error.is_none(),
        "a fresh snapshot cursor is always serviceable"
    );
}
