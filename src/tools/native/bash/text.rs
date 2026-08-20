//! The deterministic text-decoding contract of Bash output.
//!
//! Every path rustX explicitly tells the model to inspect with Read or
//! Grep — the foreground result spill and the background live-output file —
//! must contain valid UTF-8 text, because Read parses UTF-8 and Grep
//! searches UTF-8. Raw Bash output is bytes, so each output stream is
//! decoded with its own incremental decoder **before** the stdout/stderr
//! textual fragments are multiplexed:
//!
//! ```text
//! stdout byte stream -> its own IncrementalUtf8Decoder --\
//!                                                          > combined text
//! stderr byte stream -> its own IncrementalUtf8Decoder --/
//! ```
//!
//! Decoding per source stream matters: a multi-byte UTF-8 sequence may be
//! split across two reads of stdout while stderr output is interleaved
//! between those reads. Decoding after multiplexing could fabricate
//! invalid UTF-8 from two independently valid streams; decoding each
//! stream incrementally cannot.
//!
//! The invalid-byte policy is explicit and deterministic: every invalid
//! byte sequence decodes to U+FFFD (the Unicode replacement character),
//! exactly like [`String::from_utf8_lossy`], and an incomplete sequence
//! held at end-of-stream flushes to U+FFFD at [`IncrementalUtf8Decoder::finish`].

/// One incremental UTF-8 decoder of one output stream.
///
/// The decoder preserves decoder state across read chunks: an incomplete
/// trailing sequence (at most three bytes) is held in `pending` and
/// completed by the next [`IncrementalUtf8Decoder::push`], so a code point
/// split across arbitrary read boundaries decodes exactly once, at the
/// chunk boundary where its final byte arrives.
#[derive(Debug, Default)]
pub(super) struct IncrementalUtf8Decoder {
    /// The held incomplete trailing sequence; at most three bytes.
    pending: Vec<u8>,
}

impl IncrementalUtf8Decoder {
    /// Decodes one read chunk, returning the decoded text. An incomplete
    /// trailing sequence is held for the next chunk; invalid sequences
    /// decode to U+FFFD.
    pub(super) fn push(&mut self, bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return String::new();
        }
        let mut buffer = std::mem::take(&mut self.pending);
        buffer.extend_from_slice(bytes);
        self.decode(buffer, false)
    }

    /// Flushes the decoder at end-of-stream: an incomplete held sequence
    /// decodes deterministically to U+FFFD.
    pub(super) fn finish(&mut self) -> String {
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return String::new();
        }
        self.decode(pending, true)
    }

    /// The lossy decode loop: emits every valid prefix verbatim, replaces
    /// every invalid sequence with U+FFFD, and either holds (`eof ==
    /// false`) or replaces (`eof == true`) an incomplete trailing sequence.
    fn decode(&mut self, mut buffer: Vec<u8>, eof: bool) -> String {
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&buffer) {
                Ok(valid) => {
                    out.push_str(valid);
                    return out;
                }
                Err(error) => {
                    out.push_str(
                        std::str::from_utf8(&buffer[..error.valid_up_to()])
                            .expect("the valid prefix is valid"),
                    );
                    if let Some(len) = error.error_len() {
                        // An invalid sequence: one U+FFFD, then continue
                        // after it.
                        out.push('\u{FFFD}');
                        buffer.drain(..error.valid_up_to() + len);
                    } else {
                        // An incomplete trailing sequence: hold it for the
                        // next chunk, or flush it as U+FFFD at EOF.
                        if eof {
                            out.push('\u{FFFD}');
                        } else {
                            self.pending = buffer[error.valid_up_to()..].to_vec();
                        }
                        return out;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::IncrementalUtf8Decoder;

    /// Feeds `bytes` through one decoder in the given chunk sizes and
    /// collects the decoded text.
    fn decode_in_chunks(bytes: &[u8], chunks: &[usize]) -> String {
        let mut decoder = IncrementalUtf8Decoder::default();
        let mut out = String::new();
        let mut offset = 0;
        for size in chunks {
            let end = (offset + size).min(bytes.len());
            out.push_str(&decoder.push(&bytes[offset..end]));
            offset = end;
        }
        if offset < bytes.len() {
            out.push_str(&decoder.push(&bytes[offset..]));
        }
        out.push_str(&decoder.finish());
        out
    }

    /// A multi-byte sequence split across read boundaries decodes exactly
    /// once, at the boundary where its final byte arrives, for every
    /// possible split point.
    #[test]
    fn split_multibyte_sequences_decode_once_at_every_split_point() {
        // "aé😀z": 1 + 2 + 4 + 1 bytes of valid UTF-8.
        let bytes = "aé😀z".as_bytes();
        for split in 1..bytes.len() {
            assert_eq!(
                decode_in_chunks(bytes, &[split]),
                "aé😀z",
                "split at byte {split}"
            );
        }
        // One byte per read: the most fragmented delivery possible.
        assert_eq!(decode_in_chunks(bytes, &[1]), "aé😀z");
    }

    /// Invalid byte sequences decode deterministically to U+FFFD, exactly
    /// like `String::from_utf8_lossy`, in one pass and across boundaries.
    #[test]
    fn invalid_sequences_become_replacement_characters() {
        assert_eq!(
            decode_in_chunks(b"ok\xff\xffdone", &[1024]),
            "ok\u{FFFD}\u{FFFD}done"
        );
        // A stray continuation byte is invalid on its own.
        assert_eq!(decode_in_chunks(b"a\x80b", &[1024]), "a\u{FFFD}b");
        // An invalid sequence split across chunks stays invalid.
        assert_eq!(decode_in_chunks(b"\xff\xfe", &[1]), "\u{FFFD}\u{FFFD}");
        // Agreement with the one-shot lossy contract for a mixed payload.
        let mixed = b"text\xC3\x28more";
        assert_eq!(
            decode_in_chunks(mixed, &[3]),
            String::from_utf8_lossy(mixed).into_owned()
        );
    }

    /// An incomplete sequence at end-of-stream flushes deterministically
    /// to U+FFFD.
    #[test]
    fn an_incomplete_sequence_at_eof_flushes_to_replacement() {
        // The first two bytes of the 4-byte 😀 (F0 9F 98 80), then EOF.
        assert_eq!(decode_in_chunks(b"x\xF0\x9F", &[1024]), "x\u{FFFD}");
        // The same bytes held across a chunk boundary flush identically.
        assert_eq!(decode_in_chunks(b"x\xF0\x9F", &[2]), "x\u{FFFD}");
        // A lone leading byte at EOF.
        assert_eq!(decode_in_chunks(b"\xC3", &[1]), "\u{FFFD}");
    }

    /// Two streams decoded independently and multiplexed as text can never
    /// fabricate invalid UTF-8, even when each stream splits a multi-byte
    /// sequence around the other stream's interleaved chunk. Decoding the
    /// raw multiplex instead would corrupt both sequences.
    #[test]
    fn interleaved_streams_never_fabricate_invalid_utf8() {
        // stdout emits "é" (C3 A9) split into two reads; stderr emits "😀"
        // (F0 9F 98 80) split into two reads; the runtime observes the
        // interleaved order: stdout[0], stderr[0], stdout[1], stderr[1].
        let stdout: [&[u8]; 2] = [b"\xC3", b"\xA9"];
        let stderr: [&[u8]; 2] = [b"\xF0\x9F", b"\x98\x80"];
        let mut stdout_decoder = IncrementalUtf8Decoder::default();
        let mut stderr_decoder = IncrementalUtf8Decoder::default();
        let mut combined = String::new();
        combined.push_str(&stdout_decoder.push(stdout[0]));
        combined.push_str(&stderr_decoder.push(stderr[0]));
        combined.push_str(&stdout_decoder.push(stdout[1]));
        combined.push_str(&stderr_decoder.push(stderr[1]));
        combined.push_str(&stdout_decoder.finish());
        combined.push_str(&stderr_decoder.finish());
        assert_eq!(combined, "é😀");

        // The negative reference: decoding the raw multiplex in that
        // observation order would be invalid UTF-8.
        let mut raw = Vec::new();
        raw.extend_from_slice(stdout[0]);
        raw.extend_from_slice(stderr[0]);
        raw.extend_from_slice(stdout[1]);
        raw.extend_from_slice(stderr[1]);
        assert!(std::str::from_utf8(&raw).is_err());
    }
}
