//! Length-prefixed record framing shared by Server and Client.
//!
//! One record is a 4-byte big-endian length prefix followed by exactly one
//! JSON-encoded DTO. The [`MAX_ENCODED_FRAME_BYTES`] bound covers the whole
//! frame — prefix included — so a payload never exceeds
//! [`MAX_RECORD_BYTES`]. Lengths are checked before any allocation and
//! parse errors never expose payload bytes.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::MAX_ENCODED_FRAME_BYTES;

const PREFIX_BYTES: usize = 4;

/// Maximum JSON payload bytes in one record. The frame bound includes the
/// 4-byte length prefix.
pub const MAX_RECORD_BYTES: usize = MAX_ENCODED_FRAME_BYTES - PREFIX_BYTES;

/// The absolute deadline for one whole local handshake: Hello through
/// admission reply, armed once when the socket is established and never
/// re-armed. Server and Client share exactly this budget.
pub const LOCAL_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Reads exactly one framed record.
///
/// The declared length is validated before any allocation: zero and
/// oversized records are rejected from the prefix alone, and at most
/// [`MAX_RECORD_BYTES`] are ever buffered.
///
/// # Errors
///
/// Returns an [`io::ErrorKind::InvalidData`] error on an invalid length or
/// an unparseable record, or the reader's own error on EOF. Parse errors
/// are replaced with a static message; payload bytes are never exposed.
pub fn read_record<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<T> {
    let mut prefix = [0u8; PREFIX_BYTES];
    reader.read_exact(&mut prefix)?;
    read_record_body(reader, prefix)
}

/// Reads one framed record, or reports a clean end of stream.
///
/// `Ok(None)` means EOF arrived before the first prefix byte — the peer
/// closed an idle connection. Any EOF after the first byte is a truncated
/// record and an error, exactly like [`read_record`].
///
/// # Errors
///
/// Same as [`read_record`]; a partial record is always an error.
pub fn read_optional_record<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<Option<T>> {
    let mut first = [0u8; 1];
    if reader.read(&mut first)? == 0 {
        return Ok(None);
    }
    let mut prefix = [0u8; PREFIX_BYTES];
    prefix[0] = first[0];
    reader.read_exact(&mut prefix[1..])?;
    read_record_body(reader, prefix).map(Some)
}

fn read_record_body<T: DeserializeOwned>(
    reader: &mut impl Read,
    prefix: [u8; PREFIX_BYTES],
) -> io::Result<T> {
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_RECORD_BYTES {
        return Err(invalid("invalid record length"));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|_| invalid("invalid record payload"))
}

/// One serialized record whose payload fits [`MAX_RECORD_BYTES`].
///
/// Encoding is a pure step, separate from emission: a caller can prove a
/// request is encodable before committing to write anything, and the same
/// bytes can be offered to a writer exactly once.
pub struct EncodedRecord(Vec<u8>);

impl EncodedRecord {
    /// Emits the record as one length-prefixed frame.
    ///
    /// # Errors
    ///
    /// Returns the writer's own error.
    ///
    /// # Panics
    ///
    /// Never: the bounded buffer guarantees the length fits `u32`.
    pub fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        let length = u32::try_from(self.0.len()).expect("record within bound");
        writer.write_all(&length.to_be_bytes())?;
        writer.write_all(&self.0)
    }
}

/// Serializes `record` into a bounded buffer that fails at
/// [`MAX_RECORD_BYTES`], so no oversized payload `Vec` is ever allocated.
/// Nothing is emitted — bytes reach the wire only through
/// [`EncodedRecord::write_to`].
///
/// # Errors
///
/// Returns an [`io::ErrorKind::InvalidData`] error when the DTO does not
/// serialize or exceeds the bound.
pub fn encode_record<T: Serialize>(record: &T) -> io::Result<EncodedRecord> {
    let mut body = BoundedBuf::new(MAX_RECORD_BYTES);
    serde_json::to_writer(&mut body, record).map_err(|error| {
        if error.is_io() {
            invalid("record exceeds the encoded frame bound")
        } else {
            invalid("record is not serializable")
        }
    })?;
    Ok(EncodedRecord(body.buf))
}

/// Serializes `record` and writes it as one framed record.
///
/// Equivalent to `encode_record(record)?.write_to(writer)`: nothing is
/// written until the whole record fits the bound.
///
/// # Errors
///
/// Returns an [`io::ErrorKind::InvalidData`] error when the DTO does not
/// serialize or exceeds the bound, or the writer's own error.
pub fn write_record<T: Serialize>(writer: &mut impl Write, record: &T) -> io::Result<()> {
    encode_record(record)?.write_to(writer)
}

/// A `Vec<u8>` sink that refuses growth past `limit`.
struct BoundedBuf {
    buf: Vec<u8>,
    limit: usize,
}

impl BoundedBuf {
    fn new(limit: usize) -> Self {
        Self {
            buf: Vec::new(),
            limit,
        }
    }
}

impl Write for BoundedBuf {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len() + data.len() > self.limit {
            return Err(invalid("record exceeds the encoded frame bound"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// One absolute-deadline I/O budget over a local handshake socket.
///
/// Every read and write arms the socket timeout with the *remaining*
/// budget — the deadline is armed once when the socket is established and
/// never re-arms, so a record delivered byte by byte consumes the same
/// budget as one sent at once. A syscall that returns `WouldBlock` or
/// `TimedOut` with the deadline exhausted reports `TimedOut`; the
/// deadline is also rechecked after a successful syscall so a peer cannot
/// slide just under the wire with a stalled prefix.
///
/// [`check`](Self::check) tests the deadline without I/O;
/// [`finish`](Self::finish) is the success path: it rechecks the deadline
/// and clears both socket timeouts so the stream returns to ordinary
/// blocking I/O. It is only ever called on a completed handshake — a
/// failed handshake stream is shut down, never returned to service.
#[cfg(unix)]
pub struct Handshake<'a> {
    stream: &'a mut std::os::unix::net::UnixStream,
    deadline: Instant,
    now: fn() -> Instant,
}

#[cfg(unix)]
impl<'a> Handshake<'a> {
    /// Wraps `stream` with the absolute `deadline` every handshake I/O
    /// must finish inside.
    pub fn new(stream: &'a mut std::os::unix::net::UnixStream, deadline: Instant) -> Self {
        Self {
            stream,
            deadline,
            now: Instant::now,
        }
    }

    /// Access to the wrapped stream, for shutdown on a failed handshake.
    pub fn get_mut(&mut self) -> &mut std::os::unix::net::UnixStream {
        self.stream
    }

    /// The budget left before `deadline`; an expired or zero budget is a
    /// `TimedOut` error, never a zero or negative socket timeout.
    fn remaining(&self) -> io::Result<Duration> {
        deadline_remaining(self.deadline, self.now)
    }

    /// Fails `TimedOut` if the deadline has passed; does no I/O.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::TimedOut`] when the budget is exhausted.
    pub fn check(&self) -> io::Result<()> {
        self.remaining().map(|_| ())
    }

    /// Ends a completed handshake: clears both socket timeouts so the
    /// stream returns to blocking I/O for the session that follows. The
    /// deadline covers the cleanup too: it is checked before and after
    /// the clears.
    ///
    /// # Errors
    ///
    /// Returns `TimedOut` when the budget is exhausted, or the socket
    /// error if a timeout could not be cleared.
    pub fn finish(&mut self) -> io::Result<()> {
        self.check()?;
        self.stream.set_read_timeout(None)?;
        self.stream.set_write_timeout(None)?;
        self.check()
    }
}

#[cfg(unix)]
impl Read for Handshake<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        let outcome = self.stream.read(buffer);
        // A result may still have arrived after the deadline — including a
        // `WouldBlock`/`TimedOut` socket timeout: the absolute check
        // decides, not the syscall's clock.
        self.check()?;
        outcome
    }
}

#[cfg(unix)]
impl Write for Handshake<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        let outcome = self.stream.write(buffer);
        self.check()?;
        outcome
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

#[cfg(unix)]
fn deadline_remaining(deadline: Instant, now: fn() -> Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(deadline_expired)
}

#[cfg(unix)]
fn deadline_expired() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "handshake deadline expired")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientRequest;

    #[test]
    fn oversize_prefix_rejects_before_allocating_or_reading() {
        let over = u32::try_from(MAX_RECORD_BYTES).unwrap() + 1;
        for declared in [0u32, 1, over, u32::MAX] {
            let mut reader: &[u8] = &declared.to_be_bytes();
            let result = read_record::<ClientRequest>(&mut reader);
            if declared == 0 || declared as usize > MAX_RECORD_BYTES {
                // Zero and oversize are rejected from the prefix alone: the
                // reader held only the 4 bytes and no body read was attempted.
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{declared}");
            } else {
                // A valid length proceeds to read the body and hits EOF.
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
            }
        }
    }

    #[test]
    fn roundtrip_writes_prefix_then_exactly_one_dto() {
        let mut wire = Vec::new();
        write_record(&mut wire, &ClientRequest::Ping).unwrap();
        assert_eq!(
            u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize,
            wire.len() - 4
        );
        let mut slice: &[u8] = &wire;
        assert_eq!(
            read_record::<ClientRequest>(&mut slice).unwrap(),
            ClientRequest::Ping
        );
        assert_eq!(slice.len(), 0);
    }

    #[test]
    fn optional_record_distinguishes_clean_eof_from_truncation() {
        let mut empty: &[u8] = &[];
        assert_eq!(
            read_optional_record::<ClientRequest>(&mut empty).unwrap(),
            None
        );

        let mut partial_prefix: &[u8] = &[0, 0];
        assert_eq!(
            read_optional_record::<ClientRequest>(&mut partial_prefix)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );

        let mut truncated_body: &[u8] = &[0, 0, 0, 8, b'{'];
        assert_eq!(
            read_optional_record::<ClientRequest>(&mut truncated_body)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );

        let mut malformed: &[u8] = &[0, 0, 0, 1, b'!'];
        assert_eq!(
            read_optional_record::<ClientRequest>(&mut malformed)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let mut wire = Vec::new();
        write_record(&mut wire, &ClientRequest::Ping).unwrap();
        let mut slice: &[u8] = &wire;
        assert_eq!(
            read_optional_record::<ClientRequest>(&mut slice).unwrap(),
            Some(ClientRequest::Ping)
        );
    }

    #[test]
    fn writer_never_allocates_past_the_bound() {
        #[derive(Serialize)]
        struct Big {
            data: Vec<u8>,
        }
        let big = Big {
            data: vec![b'x'; MAX_RECORD_BYTES],
        };
        let mut wire = Vec::new();
        assert!(write_record(&mut wire, &big).is_err());
        // Nothing was written: the frame is all-or-nothing.
        assert_eq!(wire.len(), 0);
    }

    #[test]
    fn parse_errors_do_not_expose_payload() {
        let payload = b"not json secret bytes".to_vec();
        let mut wire = u32::try_from(payload.len()).unwrap().to_be_bytes().to_vec();
        wire.extend_from_slice(&payload);
        let mut slice: &[u8] = &wire;
        let error = read_record::<ClientRequest>(&mut slice).unwrap_err();
        let text = error.to_string();
        assert!(!text.contains("secret"));
        assert!(!text.contains("not json"));
    }

    #[cfg(unix)]
    mod handshake {
        use super::*;
        use std::cell::Cell;
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        thread_local! {
            static STUB_NOW: Cell<Instant> = Cell::new(Instant::now());
        }

        fn stub_now() -> Instant {
            STUB_NOW.with(Cell::get)
        }

        fn stub_advance(by: Duration) {
            STUB_NOW.with(|now| now.set(now.get() + by));
        }

        fn stub_handshake(stream: &mut UnixStream, budget: Duration) -> Handshake<'_> {
            Handshake {
                stream,
                deadline: stub_now() + budget,
                now: stub_now,
            }
        }

        #[test]
        fn one_absolute_budget_covers_partial_reads_and_writes() {
            let (mut peer, mut stream) = UnixStream::pair().unwrap();
            let mut handshake = stub_handshake(&mut stream, Duration::from_secs(5));

            // Partial progress inside the budget succeeds...
            peer.write_all(b"he").unwrap();
            let mut buf = [0u8; 4];
            handshake.read_exact(&mut buf[..2]).unwrap();
            assert_eq!(&buf[..2], b"he");
            handshake.write_all(b"ok").unwrap();

            // ...but the budget is never re-armed: once the absolute
            // deadline passes, every operation fails before any syscall.
            stub_advance(Duration::from_secs(6));
            assert_eq!(
                handshake.read(&mut buf).unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
            assert_eq!(
                handshake.write(b"x").unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
            assert_eq!(
                handshake.check().unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
            assert_eq!(
                handshake.finish().unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
        }

        #[test]
        fn a_socket_timeout_maps_to_the_deadline_error() {
            let (_peer, mut stream) = UnixStream::pair().unwrap();
            // A real (non-stub) tiny budget: the socket timeout fires and
            // the exhausted deadline reports it as TimedOut.
            let mut handshake =
                Handshake::new(&mut stream, Instant::now() + Duration::from_millis(30));
            let mut buf = [0u8; 1];
            assert_eq!(
                handshake.read(&mut buf).unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
        }

        #[test]
        fn finish_clears_both_socket_timeouts() {
            let (mut peer, mut stream) = UnixStream::pair().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut handshake = Handshake::new(&mut stream, deadline);
            peer.write_all(b"x").unwrap();
            let mut buf = [0u8; 1];
            handshake.read_exact(&mut buf).unwrap();
            handshake.finish().unwrap();
            assert_eq!(stream.read_timeout().unwrap(), None);
            assert_eq!(stream.write_timeout().unwrap(), None);
        }
    }
}
