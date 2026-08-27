//! The strict stdio / JSONL Runtime Client transport (Issue #38).
//!
//! This module is a bounded byte-stream adapter around
//! [`RuntimeClientEndpoint`]. It is not a runtime coordinator, not an
//! attachment authority, not a cancellation authority, not a replay
//! authority, not a second protocol, and not a runtime bootstrap layer.
//!
//! # Framing contract
//!
//! ```text
//! stdin  : one RuntimeClientRequest JSON object per LF-delimited record
//! stdout : one RuntimeClientResponse OR RuntimeClientProtocolEvent
//!          per LF-delimited record
//! ```
//!
//! - `\n` (LF) is the sole record delimiter. One physical LF terminates one
//!   record, so a JSON string containing an escaped `\\n` stays inside one
//!   record and multiline pretty-printed JSON is not supported.
//! - CRLF input is accepted by removing exactly one `\r` immediately before
//!   the terminating LF. No other whitespace is touched: the transport
//!   defines no second whitespace grammar beyond `serde_json`'s.
//! - [`STDIO_JSONL_MAX_RECORD_BYTES`] bounds one record's JSON payload in
//!   both directions, excluding the terminating LF and including a trailing
//!   CR when CRLF was used on input.
//! - The output sink carries protocol records only. This module never
//!   writes human or operator logging anywhere (it uses no `println!` /
//!   `eprintln!`): failures are returned to the caller, and a future
//!   process-composition layer decides whether to log them to stderr.
//!
//! # Bounded dispatch
//!
//! ```text
//! bytes -> physical LF found? -> record <= limit? -> strip one terminal CR
//!       -> exact RuntimeClientRequest -> RuntimeClientEndpoint::handle_request
//! ```
//!
//! No semantic operation runs before every preceding framing and
//! deserialization check succeeded. Any complete in-bound-size record that
//! does not deserialize to the exact v4 request type — malformed JSON,
//! unknown method, unknown field, wrong parameter type, empty record,
//! whitespace-only record — is transport-fatal: the session returns a
//! framing error, the endpoint is dropped (RAII detach), no fabricated
//! protocol record is written, and no semantic request is applied. Protocol
//! v4 has no uncorrelated error envelope, and this transport does not
//! invent one.
//!
//! # Session shape
//!
//! ```text
//! RuntimeClientEndpoint
//!         |
//!         v
//! one stdio JSONL session
//!         |
//!         +-- bounded frame reader
//!         +-- active EventSubscription view
//!         +-- request/event mux (biased: input first)
//!         +-- one JSON serializer / write path
//!         |
//!         v
//!     AsyncWrite
//! ```
//!
//! One async session loop owns the endpoint, the reader, the writer, and
//! the framing state. There are no transport tasks, no channels, and no
//! outbound queue: exactly one outbound record is serialized and written at
//! a time, and the next input record or event is selected only after that
//! write completed. A slow consumer therefore stalls the transport, never
//! rustX Runtime — attempt execution, event publication, mailbox activity,
//! background execution, and capability state keep progressing under their
//! own owners, and no host lock is held across any await here.
//!
//! # Transport loss is never cancellation
//!
//! EOF, a truncated record, a malformed record, an oversized record, a
//! broken output pipe, subscription lag, and dropping the session all end
//! the local transport and detach the attachment by dropping the endpoint.
//! None of them cancels the current attempt, settles anything, drains the
//! mailbox, mutates canonical history, fabricates a terminal event, or
//! shuts the runtime down. Conversely a successful semantic `shutdown`
//! request does not close the transport: its correlated response is written
//! and the session keeps serving.

use serde::Serialize;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};

use super::super::endpoint::RuntimeClientEndpoint;
use super::super::host::EventDelivery;
use super::super::types::{RuntimeClientCursor, RuntimeClientRequest};

/// The maximum size in bytes of one JSONL protocol record.
///
/// The same v4 limit applies in both directions. It bounds the JSON payload
/// of one record: the terminating LF is not counted, and a trailing CR is
/// counted when CRLF was used on input.
///
/// An inbound record that reaches the limit before its LF is session-fatal
/// immediately — the transport does not keep buffering, does not discard
/// through a later LF, and never partially applies a Runtime Client
/// request. An outbound record that would exceed the limit is never
/// truncated and never split across records; the session terminates with
/// [`StdioTransportError::OutboundRecordTooLarge`].
pub const STDIO_JSONL_MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;

/// The fixed size of the transport's input read chunk.
///
/// The reader's internal chunk never grows: records are accumulated out of
/// this fixed chunk into a record buffer that is itself bounded by
/// [`STDIO_JSONL_MAX_RECORD_BYTES`].
pub const STDIO_JSONL_READ_CHUNK_BYTES: usize = 8 * 1024;

/// Why one stdio JSONL session ended normally.
///
/// A normal end is peer transport closure. It detaches the attachment and
/// nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioSessionEnd {
    /// The input stream reached EOF at a record boundary: clean transport
    /// input closure.
    InputEof,
    /// The output stream reported [`std::io::ErrorKind::BrokenPipe`]: clean
    /// peer transport loss. The session stops without retrying and without
    /// spinning.
    OutputBrokenPipe,
}

impl core::fmt::Display for StdioSessionEnd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InputEof => f.write_str("the client closed the transport input stream"),
            Self::OutputBrokenPipe => f.write_str("the client closed the transport output stream"),
        }
    }
}

/// A framing failure of one inbound JSONL record.
///
/// Every variant is transport-fatal and semantically inert: the offending
/// record never reaches the semantic endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdioFramingError {
    /// The record reached the record limit before its terminating LF.
    RecordTooLarge {
        /// The transport record limit in bytes.
        limit: usize,
    },
    /// The input stream ended with a partial record buffered (no
    /// terminating LF).
    TruncatedRecord {
        /// The number of buffered bytes of the truncated record.
        bytes: usize,
    },
    /// A complete in-bound-size record did not deserialize to the exact
    /// Runtime Client Protocol v4 request type.
    MalformedRecord {
        /// The decoder's human-readable detail.
        message: String,
    },
}

impl core::fmt::Display for StdioFramingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RecordTooLarge { limit } => {
                write!(
                    f,
                    "an inbound record exceeded the {limit}-byte record limit"
                )
            }
            Self::TruncatedRecord { bytes } => write!(
                f,
                "the input stream ended with a {bytes}-byte record that has no terminating newline"
            ),
            Self::MalformedRecord { message } => {
                write!(f, "an inbound record is not a valid v4 request: {message}")
            }
        }
    }
}

impl std::error::Error for StdioFramingError {}

/// A transport-local failure of one stdio JSONL session.
///
/// These are deliberately distinct from
/// [`RuntimeClientError`](super::super::types::RuntimeClientError): a
/// transport failure is never a semantic protocol error, is never written
/// to the wire, and never appears in a Runtime Client response.
#[derive(Debug)]
pub enum StdioTransportError {
    /// The inbound byte stream violated the framing contract.
    Framing(StdioFramingError),
    /// Reading the input stream failed.
    InputIo(std::io::Error),
    /// Writing the output stream failed for a reason other than a broken
    /// pipe. The session never retries a write that could have partially
    /// reached the peer.
    OutputIo(std::io::Error),
    /// One outbound protocol record would exceed the transport record
    /// limit. Serialization is refused mid-way, so the oversized record is
    /// never fully built, never truncated, and never split.
    ///
    /// Semantic work already committed is never rolled back: the client may
    /// have an unknown-outcome request and repairs it from an authoritative
    /// snapshot after reconnecting.
    OutboundRecordTooLarge {
        /// The transport record limit in bytes.
        limit: usize,
    },
    /// The active subscription fell behind the bounded replay ring while
    /// the transport was stalled. Protocol v4 has no uncorrelated
    /// stream-error record, so the session terminates and the client
    /// repairs from an authoritative snapshot after reconnecting.
    SubscriptionLagged {
        /// The cursor the subscription consumed through.
        after_cursor: RuntimeClientCursor,
        /// The oldest cursor the runtime can still serve.
        earliest_serviceable: RuntimeClientCursor,
    },
    /// The Runtime Client observation stream is exhausted; nothing further
    /// will ever be published.
    SubscriptionExhausted,
    /// The observation stream stopped delivering while the endpoint still
    /// reports an active subscription. The session terminates rather than
    /// spinning on a stream it cannot pump.
    SubscriptionClosed,
}

impl core::fmt::Display for StdioTransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Framing(error) => write!(f, "stdio framing error: {error}"),
            Self::InputIo(error) => write!(f, "stdio input failed: {error}"),
            Self::OutputIo(error) => write!(f, "stdio output failed: {error}"),
            Self::OutboundRecordTooLarge { limit } => write!(
                f,
                "an outbound record exceeded the {limit}-byte record limit"
            ),
            Self::SubscriptionLagged {
                after_cursor,
                earliest_serviceable,
            } => write!(
                f,
                "the transport subscription fell behind retention after cursor {after_cursor} \
                 (earliest serviceable {earliest_serviceable})"
            ),
            Self::SubscriptionExhausted => {
                f.write_str("the Runtime Client observation stream is exhausted")
            }
            Self::SubscriptionClosed => {
                f.write_str("the active Runtime Client subscription stopped delivering")
            }
        }
    }
}

impl std::error::Error for StdioTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Framing(error) => Some(error),
            Self::InputIo(error) | Self::OutputIo(error) => Some(error),
            _ => None,
        }
    }
}

/// Serves one Runtime Client endpoint as a strict stdio JSONL session over
/// the process's standard input and output.
///
/// This is the concrete process composition of
/// [`serve_stdio_jsonl_with_io`]; it adds no behavior. The endpoint is
/// consumed, so returning from this function drops it and detaches the
/// attachment by RAII.
///
/// # Errors
///
/// Returns [`StdioTransportError`] for every abnormal local transport
/// termination: framing violations, input/output I/O failures, an oversized
/// outbound record, and subscription lag/exhaustion. None of them mutates
/// semantic runtime state.
pub async fn serve_stdio_jsonl(
    endpoint: RuntimeClientEndpoint,
) -> Result<StdioSessionEnd, StdioTransportError> {
    serve_stdio_jsonl_with_io(endpoint, tokio::io::stdin(), tokio::io::stdout()).await
}

/// Serves one Runtime Client endpoint as a strict stdio JSONL session over
/// arbitrary byte streams.
///
/// This is the transport core: the process-stdio adapter, integration
/// tests, and any future in-process composition all run exactly this loop.
/// The endpoint is consumed, so returning drops it and detaches the
/// attachment by RAII.
///
/// The session is one task. It selects between the next complete input
/// record and the next delivery of the currently active subscription — with
/// client input biased first — handles it, writes exactly one outbound
/// record, and only then selects again. There is no outbound queue and no
/// second writer.
///
/// # Errors
///
/// Returns [`StdioTransportError`] for every abnormal local transport
/// termination; see [`serve_stdio_jsonl`].
pub async fn serve_stdio_jsonl_with_io<R, W>(
    endpoint: RuntimeClientEndpoint,
    reader: R,
    writer: W,
) -> Result<StdioSessionEnd, StdioTransportError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    serve(endpoint, reader, writer, STDIO_JSONL_MAX_RECORD_BYTES).await
}

/// One step the session loop selected.
// The delivery variant carries the projection's own event enum, which is
// deliberately unboxed there (one allocation per delivered event would be
// paid on the hot path to shrink a short-lived stack value). This wrapper
// exists for exactly one loop iteration and inherits that trade-off.
#[allow(clippy::large_enum_variant)]
enum SessionStep {
    /// A complete input record, or `None` at clean EOF.
    Input(Option<Vec<u8>>),
    /// One delivery of the active subscription.
    Delivery(EventDelivery),
}

/// The one stdio JSONL session loop.
///
/// The record limit is a parameter so the in-crate framing tests can drive
/// the exact boundary behavior without allocating multi-megabyte fixtures;
/// the public API exposes only the frozen v4 limit.
async fn serve<R, W>(
    endpoint: RuntimeClientEndpoint,
    reader: R,
    writer: W,
    limit: usize,
) -> Result<StdioSessionEnd, StdioTransportError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut input = RecordReader::new(reader, limit);
    let mut output = RecordWriter::new(writer, limit);
    loop {
        // A clone of the *currently* active subscription, re-read every
        // iteration: `subscribe_events` installs it, a later re-subscription
        // replaces it, and detach removes it. The clone is a registration
        // handle plus a wakeup — it owns no event buffer — and no lock is
        // held while it is polled.
        let subscription = endpoint.subscription();
        let step = match subscription.as_ref() {
            // Without a subscription there is nothing to forward, so the
            // session waits only for the next input record.
            None => SessionStep::Input(input.next_record().await?),
            Some(subscription) => {
                tokio::select! {
                    // Deterministic mux rule, not scheduler timing: when a
                    // request and an event are ready at the same poll,
                    // client input is handled first. This is a local
                    // transport scheduling rule; it defines no Runtime
                    // Client ordering contract between a response and an
                    // unrelated notification.
                    biased;

                    record = input.next_record() => SessionStep::Input(record?),
                    delivery = subscription.next() => SessionStep::Delivery(delivery),
                }
            }
        };

        match step {
            SessionStep::Input(None) => return Ok(StdioSessionEnd::InputEof),
            SessionStep::Input(Some(record)) => {
                // Framing and exact deserialization both succeeded before
                // any semantic dispatch happens.
                let request = decode_request(&record)?;
                let response = endpoint.handle_request_async(request).await;
                // A successful `shutdown` means semantic quiescence. The
                // session still keeps serving read/control requests because
                // transport attachment lifetime is a separate concern.
                if let Some(end) = output.write_record(&response).await? {
                    return Ok(end);
                }
            }
            SessionStep::Delivery(EventDelivery::Event(event)) => {
                if let Some(end) = output.write_record(&event).await? {
                    return Ok(end);
                }
            }
            SessionStep::Delivery(EventDelivery::ResyncRequired {
                after_cursor,
                earliest_serviceable,
            }) => {
                return Err(StdioTransportError::SubscriptionLagged {
                    after_cursor,
                    earliest_serviceable,
                });
            }
            SessionStep::Delivery(EventDelivery::Exhausted) => {
                return Err(StdioTransportError::SubscriptionExhausted);
            }
            // `EventSubscription::next` never yields `Pending`, and a
            // registration cannot be removed while this session holds a
            // handle, so neither shape is reachable in practice. Both are
            // handled by re-evaluating once: if the endpoint no longer has
            // a subscription the session continues in input-only mode,
            // otherwise it terminates. Neither path can spin.
            SessionStep::Delivery(EventDelivery::Closed | EventDelivery::Pending) => {
                drop(subscription);
                if endpoint.subscription().is_some() {
                    return Err(StdioTransportError::SubscriptionClosed);
                }
            }
        }
    }
}

/// Decodes one complete in-bound-size record to the exact v4 request type.
fn decode_request(record: &[u8]) -> Result<RuntimeClientRequest, StdioTransportError> {
    serde_json::from_slice(record).map_err(|error| {
        StdioTransportError::Framing(StdioFramingError::MalformedRecord {
            message: error.to_string(),
        })
    })
}

/// The bounded LF-delimited record reader.
///
/// The reader owns two buffers and nothing else: a fixed
/// [`STDIO_JSONL_READ_CHUNK_BYTES`] chunk inside the [`BufReader`], and a
/// record accumulator whose length is checked against the limit *before*
/// every append, so it can never exceed the limit. There is no
/// `read_line`/`read_until`/`read_to_end` anywhere in the transport.
///
/// [`RecordReader::next_record`] is cancel-safe: all accumulation state
/// lives in `self`, and the only await is [`BufReader::fill_buf`], which
/// reads nothing when it is dropped while pending. Losing the select race
/// against an event delivery therefore never loses input bytes.
struct RecordReader<R> {
    /// The fixed-chunk buffered input.
    reader: BufReader<R>,
    /// The partially accumulated record, bounded by `limit`.
    pending: Vec<u8>,
    /// The record limit in bytes.
    limit: usize,
}

impl<R> RecordReader<R>
where
    R: AsyncRead + Unpin,
{
    /// Creates the reader over one input stream.
    fn new(reader: R, limit: usize) -> Self {
        Self {
            reader: BufReader::with_capacity(STDIO_JSONL_READ_CHUNK_BYTES, reader),
            pending: Vec::new(),
            limit,
        }
    }

    /// Reads the next complete record, with exactly one terminal CR
    /// removed when CRLF was used.
    ///
    /// Returns `Ok(None)` at clean EOF (no partial record buffered), and a
    /// [`StdioFramingError::TruncatedRecord`] when EOF interrupts a record.
    async fn next_record(&mut self) -> Result<Option<Vec<u8>>, StdioTransportError> {
        loop {
            let chunk = self
                .reader
                .fill_buf()
                .await
                .map_err(StdioTransportError::InputIo)?;
            if chunk.is_empty() {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                return Err(StdioTransportError::Framing(
                    StdioFramingError::TruncatedRecord {
                        bytes: self.pending.len(),
                    },
                ));
            }
            // One physical LF terminates one record. An escaped `\\n` inside
            // a JSON string is two source bytes and never matches here.
            let (take, consume, complete) = match chunk.iter().position(|byte| *byte == b'\n') {
                Some(index) => (index, index + 1, true),
                None => (chunk.len(), chunk.len(), false),
            };
            // The bound is enforced before the append, so an oversized
            // record is rejected without ever being buffered — and long
            // before any semantic dispatch could observe it.
            if self.pending.len() + take > self.limit {
                return Err(StdioTransportError::Framing(
                    StdioFramingError::RecordTooLarge { limit: self.limit },
                ));
            }
            reserve_bounded(&mut self.pending, take, self.limit);
            self.pending.extend_from_slice(&chunk[..take]);
            self.reader.consume(consume);
            if complete {
                let mut record = std::mem::take(&mut self.pending);
                // Accept CRLF by removing exactly one CR. No `trim`: other
                // whitespace is left to `serde_json`.
                if record.last() == Some(&b'\r') {
                    record.pop();
                }
                return Ok(Some(record));
            }
        }
    }
}

/// The one outbound record path: serialize, append LF, write, flush.
///
/// Exactly one code path owns JSON serialization, the LF, the write, and
/// the flush, so protocol records can never interleave and the completion
/// order of this path *is* the serialization point of the output stream.
/// The writer retains one serialization buffer whose length is bounded by
/// the record limit; it holds no queue of records.
struct RecordWriter<W> {
    /// The output stream.
    writer: W,
    /// The reused serialization buffer, bounded by `limit`.
    buffer: Vec<u8>,
    /// The record limit in bytes.
    limit: usize,
}

impl<W> RecordWriter<W>
where
    W: AsyncWrite + Unpin,
{
    /// Creates the writer over one output stream.
    fn new(writer: W, limit: usize) -> Self {
        Self {
            writer,
            buffer: Vec::new(),
            limit,
        }
    }

    /// Serializes and writes exactly one protocol record.
    ///
    /// Returns `Ok(None)` when the record was written and flushed, and
    /// `Ok(Some(StdioSessionEnd::OutputBrokenPipe))` when the peer closed
    /// the output stream.
    async fn write_record<T>(
        &mut self,
        message: &T,
    ) -> Result<Option<StdioSessionEnd>, StdioTransportError>
    where
        T: Serialize,
    {
        self.buffer.clear();
        // Serialization is bounded as it happens rather than measured
        // afterwards: an oversized record is refused mid-way and is never
        // fully built, truncated, or split across records.
        let mut sink = LimitedBuffer {
            buffer: &mut self.buffer,
            limit: self.limit,
            overflowed: false,
        };
        if let Err(error) = serde_json::to_writer(&mut sink, message) {
            if sink.overflowed {
                self.buffer.clear();
                return Err(StdioTransportError::OutboundRecordTooLarge { limit: self.limit });
            }
            // `serde_json` only fails here for a value that cannot be
            // represented; the Runtime Client protocol types always can.
            return Err(StdioTransportError::OutputIo(std::io::Error::other(error)));
        }
        // The LF is the delimiter, not payload: it is not counted against
        // the record limit.
        match self.writer.write_all(&self.buffer).await {
            Ok(()) => {}
            Err(error) => return Self::classify_output(error),
        }
        match self.writer.write_all(b"\n").await {
            Ok(()) => {}
            Err(error) => return Self::classify_output(error),
        }
        match self.writer.flush().await {
            Ok(()) => Ok(None),
            Err(error) => Self::classify_output(error),
        }
    }

    /// Classifies an output failure: a broken pipe is clean peer transport
    /// loss, everything else is a typed transport error. Neither is ever
    /// retried, because a failed write may have partially reached the peer.
    fn classify_output(
        error: std::io::Error,
    ) -> Result<Option<StdioSessionEnd>, StdioTransportError> {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            return Ok(Some(StdioSessionEnd::OutputBrokenPipe));
        }
        Err(StdioTransportError::OutputIo(error))
    }
}

/// A size-limited [`std::io::Write`] sink over a reused buffer.
///
/// A write that would push the buffer past the limit is refused instead of
/// performed, so the serializer stops at the bound rather than allocating
/// past it and being measured afterwards. Growth is doubling clamped to the
/// limit, so the buffer never retains more than one record's bytes and no
/// reservation larger than one record is ever requested.
struct LimitedBuffer<'a> {
    /// The buffer being filled.
    buffer: &'a mut Vec<u8>,
    /// The maximum buffer length in bytes.
    limit: usize,
    /// Whether a write was refused for exceeding the limit.
    overflowed: bool,
}

impl std::io::Write for LimitedBuffer<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let required = self.buffer.len() + buf.len();
        if required > self.limit {
            self.overflowed = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "the outbound record exceeds the transport record limit",
            ));
        }
        reserve_bounded(self.buffer, buf.len(), self.limit);
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Reserves room for `additional` bytes without ever requesting capacity
/// beyond `limit`.
///
/// Growth stays amortized (doubling) but is clamped to the record limit, so
/// the transport never intentionally asks for more room than one record —
/// the usual `Vec` growth strategy cannot double the transport's documented
/// memory bound on its own.
///
/// The bound this establishes is on *logical record retention*: the record
/// accumulator and the serialization buffer each hold at most one record's
/// bytes, and rustX requests no capacity above the limit. What an allocator
/// then does with such a request — rounding a reservation up to a size
/// class or a page — is outside this contract, so `Vec::capacity()` itself
/// is not claimed to be bounded by the limit.
///
/// The caller has already established `buffer.len() + additional <= limit`.
fn reserve_bounded(buffer: &mut Vec<u8>, additional: usize, limit: usize) {
    let required = buffer.len() + additional;
    if required > buffer.capacity() {
        let target = buffer.capacity().saturating_mul(2).max(required).min(limit);
        buffer.reserve_exact(target - buffer.len());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LimitedBuffer, RecordReader, RecordWriter, STDIO_JSONL_READ_CHUNK_BYTES, StdioFramingError,
        StdioSessionEnd, StdioTransportError, decode_request,
    };
    use crate::runtime_client::types::{RequestId, RuntimeClientRequest};

    /// Reads every record the input yields, stopping at clean EOF.
    async fn records(input: &[u8], limit: usize) -> Result<Vec<Vec<u8>>, StdioTransportError> {
        let mut reader = RecordReader::new(input, limit);
        let mut out = Vec::new();
        while let Some(record) = reader.next_record().await? {
            out.push(record);
        }
        Ok(out)
    }

    /// LF is the sole delimiter, and several records inside one underlying
    /// read are separated exactly.
    #[tokio::test]
    async fn lf_delimits_records() {
        let records = records(b"{\"a\":1}\n{\"b\":2}\n", 1024)
            .await
            .expect("records");
        assert_eq!(records, vec![b"{\"a\":1}".to_vec(), b"{\"b\":2}".to_vec()]);
    }

    /// CRLF is accepted by removing exactly one terminal CR; no other
    /// whitespace is touched.
    #[tokio::test]
    async fn crlf_strips_exactly_one_carriage_return() {
        let records = records(b" {\"a\":1} \r\n\r\r\n", 1024)
            .await
            .expect("records");
        assert_eq!(records, vec![b" {\"a\":1} ".to_vec(), b"\r".to_vec()]);
    }

    /// A record split across many underlying reads is reassembled, and the
    /// reader's own chunk never grows while doing it.
    #[tokio::test]
    async fn a_split_record_is_reassembled_within_the_fixed_chunk() {
        let payload = format!("{{\"text\":\"{}\"}}", "x".repeat(64 * 1024));
        let input = format!("{payload}\n");
        let mut reader = RecordReader::new(input.as_bytes(), 1024 * 1024);
        let record = reader
            .next_record()
            .await
            .expect("record")
            .expect("one record");
        assert_eq!(record, payload.as_bytes());
        assert!(
            reader.reader.buffer().len() <= STDIO_JSONL_READ_CHUNK_BYTES,
            "the reader's internal chunk is fixed regardless of record size"
        );
        assert!(
            record.len() <= 1024 * 1024 && reader.pending.is_empty(),
            "the record accumulator retains at most one record's bytes"
        );
    }

    /// Clean EOF at a record boundary is normal closure; EOF with a partial
    /// record is an explicit framing error.
    #[tokio::test]
    async fn eof_distinguishes_boundary_from_truncation() {
        assert!(records(b"", 1024).await.expect("clean eof").is_empty());
        let error = records(b"{\"a\":1}", 1024)
            .await
            .expect_err("a partial record is truncated");
        assert!(matches!(
            error,
            StdioTransportError::Framing(StdioFramingError::TruncatedRecord { bytes: 7 })
        ));
    }

    /// A record exactly at the limit is accepted; one byte more is rejected
    /// before the record is buffered, and the LF is not counted.
    #[tokio::test]
    async fn the_record_limit_is_exact() {
        let at_limit = vec![b'x'; 16];
        let mut input = at_limit.clone();
        input.push(b'\n');
        assert_eq!(
            records(&input, 16).await.expect("at the limit"),
            vec![at_limit]
        );

        let mut over = vec![b'x'; 17];
        over.push(b'\n');
        let error = records(&over, 16).await.expect_err("over the limit");
        assert!(matches!(
            error,
            StdioTransportError::Framing(StdioFramingError::RecordTooLarge { limit: 16 })
        ));
    }

    /// An oversized record fails on the byte that crosses the bound, so the
    /// reader never accumulates past the limit even when the LF never
    /// arrives.
    #[tokio::test]
    async fn an_oversized_record_never_buffers_past_the_limit() {
        let input = vec![b'x'; 4096];
        let mut reader = RecordReader::new(input.as_slice(), 64);
        let error = reader.next_record().await.expect_err("over the limit");
        assert!(matches!(
            error,
            StdioTransportError::Framing(StdioFramingError::RecordTooLarge { limit: 64 })
        ));
        assert!(reader.pending.len() <= 64);
    }

    /// A physical LF inside otherwise-valid JSON terminates the record, so
    /// pretty-printed multiline JSON fails as the first malformed record —
    /// while an escaped newline stays inside one record.
    #[test]
    fn physical_newlines_terminate_records_and_escaped_ones_do_not() {
        decode_request(
            br#"{"method":"submit_inbound","id":1,"content":[{"type":"text","text":"a\nb"}]}"#,
        )
        .expect("an escaped newline is one record");
        let error = decode_request(b"{").expect_err("a multiline fragment is not a request");
        assert!(matches!(
            error,
            StdioTransportError::Framing(StdioFramingError::MalformedRecord { .. })
        ));
    }

    /// Empty, whitespace-only, malformed, unknown-method, and structurally
    /// invalid records are all framing failures, never semantic requests.
    #[test]
    fn invalid_records_never_decode() {
        for record in [
            &b""[..],
            &b"   "[..],
            &b"not json"[..],
            &br#"{"method":"future_method","id":1}"#[..],
            &br#"{"method":"snapshot_get","id":1,"extra":true}"#[..],
            &br#"{"method":"initialize","id":"one","protocol_version":4}"#[..],
            &br"[]"[..],
        ] {
            assert!(
                matches!(
                    decode_request(record),
                    Err(StdioTransportError::Framing(
                        StdioFramingError::MalformedRecord { .. }
                    ))
                ),
                "record {:?} must not decode",
                String::from_utf8_lossy(record)
            );
        }
        assert_eq!(
            decode_request(br#"{"method":"snapshot_get","id":3}"#).expect("valid"),
            RuntimeClientRequest::SnapshotGet {
                id: RequestId::new(3)
            }
        );
    }

    /// Every written record is one JSON payload followed by exactly one LF,
    /// with no blank lines between records.
    #[tokio::test]
    async fn records_are_written_with_exactly_one_newline() {
        let mut sink = Vec::new();
        let mut writer = RecordWriter::new(&mut sink, 1024);
        for id in 1..=3_u64 {
            assert_eq!(
                writer
                    .write_record(&serde_json::json!({ "id": id }))
                    .await
                    .expect("write"),
                None
            );
        }
        assert_eq!(
            sink,
            br#"{"id":1}
{"id":2}
{"id":3}
"#
        );
    }

    /// An outbound record over the limit terminates the session without
    /// writing a truncated or split record.
    #[tokio::test]
    async fn an_oversized_outbound_record_is_refused_whole() {
        let mut sink = Vec::new();
        let mut writer = RecordWriter::new(&mut sink, 32);
        let error = writer
            .write_record(&serde_json::json!({ "text": "y".repeat(64) }))
            .await
            .expect_err("over the limit");
        assert!(matches!(
            error,
            StdioTransportError::OutboundRecordTooLarge { limit: 32 }
        ));
        assert!(sink.is_empty(), "nothing partial reached the output");
    }

    /// The serialization buffer retains at most one record's bytes: writes
    /// are refused at the bound rather than measured afterwards.
    ///
    /// The assertion is on retained bytes, not on `Vec::capacity()`: rustX
    /// never requests capacity above the limit, but how an allocator rounds
    /// such a request is outside this contract.
    #[test]
    fn the_serialization_buffer_retention_is_bounded() {
        let mut buffer = Vec::new();
        let mut sink = LimitedBuffer {
            buffer: &mut buffer,
            limit: 100,
            overflowed: false,
        };
        for _ in 0..10 {
            std::io::Write::write_all(&mut sink, &[b'z'; 10]).expect("within the limit");
        }
        assert!(std::io::Write::write_all(&mut sink, b"z").is_err());
        assert!(sink.overflowed);
        assert_eq!(
            buffer.len(),
            100,
            "the refused write left the buffer at exactly one record's bytes"
        );
    }

    /// A broken output pipe is a normal session end, not an error.
    #[tokio::test]
    async fn a_broken_pipe_ends_the_session_normally() {
        struct BrokenPipe;

        impl tokio::io::AsyncWrite for BrokenPipe {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "closed",
                )))
            }

            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }

            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }

        let mut writer = RecordWriter::new(BrokenPipe, 1024);
        assert_eq!(
            writer
                .write_record(&serde_json::json!({"id": 1}))
                .await
                .expect("a broken pipe is not a transport error"),
            Some(StdioSessionEnd::OutputBrokenPipe)
        );
    }
}
