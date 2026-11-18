//! Length-prefixed record framing shared by Server and Client.
//!
//! One record is a 4-byte big-endian length prefix followed by exactly one
//! JSON-encoded DTO. The [`MAX_ENCODED_FRAME_BYTES`] bound covers the whole
//! frame — prefix included — so a payload never exceeds
//! [`MAX_RECORD_BYTES`]. Lengths are checked before any allocation and
//! parse errors never expose payload bytes.

use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::MAX_ENCODED_FRAME_BYTES;

const PREFIX_BYTES: usize = 4;

/// Maximum JSON payload bytes in one record. The frame bound includes the
/// 4-byte length prefix.
pub const MAX_RECORD_BYTES: usize = MAX_ENCODED_FRAME_BYTES - PREFIX_BYTES;

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

/// Serializes `record` and writes it as one framed record.
///
/// Serialization streams into a bounded buffer that fails at
/// [`MAX_RECORD_BYTES`], so no oversized payload `Vec` is ever allocated
/// and nothing is written until the whole record fits.
///
/// # Errors
///
/// Returns an [`io::ErrorKind::InvalidData`] error when the DTO does not
/// serialize or exceeds the bound, or the writer's own error.
///
/// # Panics
///
/// Never: the bounded buffer guarantees the length fits `u32`.
pub fn write_record<T: Serialize>(writer: &mut impl Write, record: &T) -> io::Result<()> {
    let mut body = BoundedBuf::new(MAX_RECORD_BYTES);
    serde_json::to_writer(&mut body, record).map_err(|error| {
        if error.is_io() {
            invalid("record exceeds the encoded frame bound")
        } else {
            invalid("record is not serializable")
        }
    })?;
    let length = u32::try_from(body.buf.len()).expect("record within bound");
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&body.buf)
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
}
