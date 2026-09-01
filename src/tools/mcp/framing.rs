//! The generic stdio protocol-corruption observation seam (Issue #174).
//!
//! rmcp's [`AsyncRwTransport`] is the one MCP framing/protocol authority: it
//! decodes newline-delimited JSON-RPC/MCP messages with its
//! [`JsonRpcMessageCodec`]. On a decode failure it deliberately keeps the
//! failure to itself (rmcp 3.1.3, `transport/async_rw.rs`):
//!
//! - serde `Syntax`/`Eof` input (plain non-JSON noise) is ignored, matching
//!   the other official MCP SDKs;
//! - serde `Data`/`Io` input — well-formed JSON that is not a valid
//!   MCP/JSON-RPC message — is answered with a bounded `Invalid Request`
//!   reply *to the peer*, and the exchange then continues as though
//!   healthy.
//!
//! The peer observing that reply is not a rustX diagnostic: without this
//! module the protocol violation disappears from rustX's point of view.
//! rmcp exposes no error/event/callback seam for these failures (its
//! `Transport::receive` returns `Option`, so a violation can only ever
//! surface as stream termination), so the narrowest possible observation
//! seam is installed *around* the unmodified rmcp transport: an `AsyncRead`
//! tee between the child stdout pipe and rmcp's reader.
//!
//! The tee classifies each completed line with rmcp's own codec — the same
//! decoder type, the same compatibility accept set — so there is still
//! exactly one framing authority and no second MCP implementation: the tee
//! never delivers, correlates, or handles a message. On a
//! confirmed structurally invalid message it records a bounded
//! [`ProtocolViolationRecorder`] fact and then ends the byte stream (EOF)
//! immediately after the offending line — rmcp still sends its own
//! peer-facing `Invalid Request` reply for that line, and its service loop
//! then terminates, so every pending and future operation on the
//! connection resolves through the recorder as an explicit rustX protocol
//! failure instead of a silently healthy exchange.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use rmcp::service::{RoleClient, RxJsonRpcMessage};
use rmcp::transport::async_rw::{JsonRpcMessageCodec, JsonRpcMessageCodecError};
use serde_json::error::Category;
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::bytes::BytesMut;
use tokio_util::codec::Decoder as _;

use crate::tools::limits::bounded_text_preview;

/// The bounded serde diagnostic retained for a protocol violation.
const VIOLATION_REASON_BYTES: usize = 256;
/// The bounded preview of the offending wire line retained for a protocol
/// violation. Peer output is evidence, not a log stream: diagnostics never
/// echo unlimited stdout.
const VIOLATION_LINE_PREVIEW_BYTES: usize = 160;

/// The recorded fact that one connection observed a structurally invalid
/// MCP peer message. The first violation wins: it is the one the protocol
/// machine acted on, and later lines were never delivered.
pub(crate) struct ProtocolViolationRecorder {
    first: Mutex<Option<String>>,
}

impl ProtocolViolationRecorder {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            first: Mutex::new(None),
        })
    }

    /// Records the first confirmed violation. Later violations are
    /// irrelevant: the stream ended at the first one.
    fn record(&self, reason: &str, line: &[u8]) {
        let mut first = self
            .first
            .lock()
            .expect("MCP protocol violation recorder lock poisoned");
        if first.is_some() {
            return;
        }
        let (reason, _) = bounded_text_preview(reason.as_bytes(), VIOLATION_REASON_BYTES);
        let (preview, _) = bounded_text_preview(line, VIOLATION_LINE_PREVIEW_BYTES);
        *first = Some(format!(
            "well-formed JSON could not be decoded as an MCP message ({reason}; offending line: '{preview}')"
        ));
    }

    /// The bounded first-violation diagnostic, when one was recorded.
    pub(crate) fn violation(&self) -> Option<String> {
        self.first
            .lock()
            .expect("MCP protocol violation recorder lock poisoned")
            .clone()
    }
}

/// How one completed wire line classifies under the framing authority's
/// own accept set.
enum LineVerdict {
    /// A valid MCP message, or a notification rmcp deliberately ignores for
    /// compatibility — the codec's accept set is the authority.
    Healthy,
    /// Not JSON at all (serde `Syntax`/`Eof`): plain noise the generic
    /// framing deliberately ignores. An implementation characteristic, not
    /// a supported logging contract.
    Noise,
    /// Well-formed JSON that is not a valid MCP/JSON-RPC message (serde
    /// `Data`/`Io`): a confirmed protocol violation.
    Violation(String),
}

/// Classifies one completed line with rmcp's own codec, so the accept set —
/// including rmcp's compatibility-ignored notifications — is exactly the
/// transport's own.
fn classify_line(line: &[u8]) -> LineVerdict {
    let mut codec = JsonRpcMessageCodec::<RxJsonRpcMessage<RoleClient>>::new();
    let mut frame = BytesMut::from(line);
    frame.extend_from_slice(b"\n");
    match codec.decode(&mut frame) {
        Ok(_) => LineVerdict::Healthy,
        Err(JsonRpcMessageCodecError::Serde(error)) => match error.classify() {
            Category::Syntax | Category::Eof => LineVerdict::Noise,
            Category::Data | Category::Io => LineVerdict::Violation(error.to_string()),
        },
        // `MaxLineLengthExceeded` needs an explicit bound the default codec
        // never installs, and an `Io` error cannot come from a slice decode.
        Err(_) => LineVerdict::Noise,
    }
}

/// An `AsyncRead` tee between the child stdout pipe and rmcp's stdio
/// transport reader. Bytes pass through unchanged and in order; completed
/// lines are classified with rmcp's own codec as they flow. After a
/// confirmed protocol violation the reader delivers exactly the bytes up to
/// and including the offending line and then reports EOF, terminating the
/// connection's protocol machine.
pub(crate) struct ViolationObservingReader<R> {
    inner: R,
    /// Bytes read from `inner` but not yet delivered downstream.
    pending: Vec<u8>,
    /// Offset into `pending` already delivered downstream.
    delivered: usize,
    /// The current unterminated line. Classification happens at line
    /// granularity, so partial-line bytes survive across reads and
    /// deliveries until their newline arrives — exactly like the framing
    /// buffer of the transport this wraps.
    line: Vec<u8>,
    /// Set once a violation was recorded: deliver the remainder of the
    /// (truncated) `pending`, then EOF.
    violated: bool,
    recorder: Arc<ProtocolViolationRecorder>,
}

impl<R> ViolationObservingReader<R> {
    pub(crate) fn new(inner: R, recorder: Arc<ProtocolViolationRecorder>) -> Self {
        Self {
            inner,
            pending: Vec::new(),
            delivered: 0,
            line: Vec::new(),
            violated: false,
            recorder,
        }
    }

    /// Observes the bytes `pending[start..]` (just read from `inner`),
    /// classifying each completed line. On the first violation, records it
    /// and truncates `pending` at the offending line's end: bytes after a
    /// confirmed violation never reach the protocol machine.
    fn observe(&mut self, start: usize) {
        let mut index = start;
        while index < self.pending.len() {
            let byte = self.pending[index];
            index += 1;
            if byte != b'\n' {
                self.line.push(byte);
                continue;
            }
            let verdict = classify_line(&self.line);
            if let LineVerdict::Violation(reason) = verdict {
                self.recorder.record(&reason, &self.line);
                self.line.clear();
                self.pending.truncate(index);
                self.violated = true;
                return;
            }
            self.line.clear();
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ViolationObservingReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.delivered < this.pending.len() {
                let count = buf.remaining().min(this.pending.len() - this.delivered);
                buf.put_slice(&this.pending[this.delivered..this.delivered + count]);
                this.delivered += count;
                if this.delivered == this.pending.len() {
                    this.pending.clear();
                    this.delivered = 0;
                }
                return Poll::Ready(Ok(()));
            }
            if this.violated {
                // The confirmed violation terminated the protocol
                // authority: the byte stream ends right after the offending
                // line. EOF is sticky.
                return Poll::Ready(Ok(()));
            }
            let mut chunk = [0_u8; 8192];
            let mut chunk_buf = ReadBuf::new(&mut chunk);
            match Pin::new(&mut this.inner).poll_read(cx, &mut chunk_buf) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => {
                    let filled = chunk_buf.filled();
                    if filled.is_empty() {
                        // Genuine peer EOF.
                        return Poll::Ready(Ok(()));
                    }
                    let start = this.pending.len();
                    this.pending.extend_from_slice(filled);
                    this.observe(start);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    fn verdict(line: &str) -> LineVerdict {
        classify_line(line.as_bytes())
    }

    #[test]
    fn a_valid_mcp_message_is_healthy() {
        assert!(matches!(
            verdict(
                r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"s","version":"0"}}}"#
            ),
            LineVerdict::Healthy
        ));
        assert!(matches!(
            verdict(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#),
            LineVerdict::Healthy
        ));
    }

    #[test]
    fn a_compatibility_ignored_notification_is_healthy() {
        // rmcp deliberately ignores non-standard notifications (e.g. LSP
        // interop); the observation seam inherits that exact accept set.
        assert!(matches!(
            verdict(r#"{"jsonrpc":"2.0","method":"notifications/custom","params":{}}"#),
            LineVerdict::Healthy
        ));
        assert!(matches!(
            verdict(r#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":1}}"#),
            LineVerdict::Healthy
        ));
    }

    #[test]
    fn plain_non_json_noise_is_noise() {
        assert!(matches!(
            verdict("plain non-protocol noise on the wire"),
            LineVerdict::Noise
        ));
    }

    #[test]
    fn well_formed_json_with_an_invalid_message_shape_is_a_violation() {
        let LineVerdict::Violation(reason) = verdict(r#"{"jsonrpc":"2.0","id":1,"method":123}"#)
        else {
            panic!("a numeric `method` is a confirmed protocol violation");
        };
        assert!(!reason.is_empty());
    }

    #[tokio::test]
    async fn a_healthy_stream_passes_bytes_through_unchanged() {
        let wire = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\nplain noise\n";
        let recorder = ProtocolViolationRecorder::new();
        let mut reader = ViolationObservingReader::new(&wire[..], recorder.clone());
        let mut observed = Vec::new();
        reader.read_to_end(&mut observed).await.expect("read");
        assert_eq!(observed, wire);
        assert!(recorder.violation().is_none());
    }

    #[tokio::test]
    async fn a_violation_is_recorded_and_the_stream_ends_after_the_offending_line() {
        let wire = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":123}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n";
        let recorder = ProtocolViolationRecorder::new();
        let mut reader = ViolationObservingReader::new(&wire[..], recorder.clone());
        let mut observed = Vec::new();
        reader.read_to_end(&mut observed).await.expect("read");
        // The offending line itself is delivered (rmcp still answers it
        // with its own bounded Invalid Request reply); the following valid
        // response never reaches the protocol machine.
        assert_eq!(observed, b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":123}\n");
        let violation = recorder.violation().expect("violation recorded");
        assert!(
            violation.contains("could not be decoded as an MCP message"),
            "bounded actionable diagnostic: {violation}"
        );
        assert!(
            violation.contains("method"),
            "the diagnostic carries a bounded preview of the offending line: {violation}"
        );
    }

    #[tokio::test]
    async fn the_first_violation_wins() {
        let wire =
            b"{\"jsonrpc\":\"2.0\",\"method\":123}\n{\"jsonrpc\":\"2.0\",\"method\":false}\n";
        let recorder = ProtocolViolationRecorder::new();
        let mut reader = ViolationObservingReader::new(&wire[..], recorder.clone());
        let mut observed = Vec::new();
        reader.read_to_end(&mut observed).await.expect("read");
        let violation = recorder.violation().expect("violation recorded");
        assert!(
            violation.contains("123"),
            "the first violation is the recorded fact: {violation}"
        );
    }

    #[tokio::test]
    async fn a_line_split_across_reads_is_classified_once_complete() {
        // A partial line must never be classified: the seam observes at
        // line granularity, exactly like the framing it wraps.
        let (mut writer, reader) = tokio::io::duplex(64);
        let recorder = ProtocolViolationRecorder::new();
        let mut reader = ViolationObservingReader::new(reader, recorder.clone());
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"meth")
            .await
            .expect("first half");
        let mut buf = vec![0_u8; 16];
        let first = reader.read(&mut buf).await.expect("partial read");
        assert_eq!(first, 16, "bytes flow before the line completes");
        assert!(recorder.violation().is_none());
        writer.write_all(b"od\":123}\n").await.expect("second half");
        let mut observed = Vec::new();
        reader.read_to_end(&mut observed).await.expect("rest");
        assert!(recorder.violation().is_some());
        drop(writer);
    }
}
