//! The shared streaming logical-text projection.
//!
//! One scanner owns the complete-line 2000-line/50KB page contract for the
//! native Read tool and the Execution Client's `ReadLines` exchange: it
//! reads the source in fixed blocks, validates UTF-8 across the *entire*
//! source — a rejected page never hides an invalid tail — and counts every
//! byte and newline so `total_lines`/`original_bytes` describe the whole
//! file, never the shown window. Only the retained output prefix and the
//! current bounded line are staged; the file is never loaded into memory.

use std::io::{self, Read};

/// The fixed read block the streaming scan consumes: bounded independent
/// of source size.
const READ_BLOCK_BYTES: usize = 32 * 1024;

/// Why one page ended.
///
/// The variants encode the established Read precedence exactly: a first
/// selected line over the byte budget outranks every other stop; a page
/// that hit its line or byte budget reports that bound; a page that ended
/// only because the caller's `limit` selected fewer lines than remain
/// reports the requested limit; everything else completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageEnd {
    /// The selected window ran to the end of the source.
    Complete,
    /// The caller's `limit` ended the selection while lines remained.
    RequestedLimit,
    /// The complete-line count reached the line budget.
    LineLimit,
    /// The joined bytes reached the byte budget — including the boundary
    /// case where the only excess byte is the terminal newline separator.
    ByteLimit,
    /// The first selected line alone exceeds the byte budget; `bytes`
    /// counts its full length even when it was never staged.
    FirstLineTooLarge { bytes: u64 },
    /// The requested offset addresses a line beyond the source's
    /// addressable count.
    OffsetPastEnd,
}

/// One logical-text page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPage {
    /// The shown text: the shown elements joined by `\n` — including the
    /// trailing newline a complete page keeps when the source ends with
    /// one. Empty for `FirstLineTooLarge` and `OffsetPastEnd`.
    pub text: String,
    /// Addressable line count of the whole source: `split('\n')` elements,
    /// so an empty source counts one line and a trailing newline adds one.
    pub total_lines: u64,
    /// Byte length of the whole source — validation always ran to EOF.
    pub original_bytes: u64,
    /// The number of selected elements shown: at most `max_lines` counted
    /// elements, plus one when a complete page keeps the trailing empty.
    pub shown_lines: u32,
    /// Why the page ended.
    pub end: PageEnd,
}

/// The two budgets one page is projected under, passed explicitly so the
/// wire protocol and the in-process tool can each supply their single
/// policy source — the scanner owns no constants of its own.
#[derive(Debug, Clone, Copy)]
pub struct PageLimits {
    /// The joined-byte budget of one page.
    pub max_bytes: usize,
    /// The complete-line budget of one page.
    pub max_lines: usize,
}

impl PageLimits {
    /// Scans `reader` into one bounded logical-text page under these
    /// bounds.
    ///
    /// `offset` is the 1-based first line and must be nonzero — callers
    /// normalize a zero offset before this boundary. `limit` bounds the
    /// selected lines; `Some(0)` is invalid. The scan reads the whole
    /// source: UTF-8 validation, byte count, and line count never stop
    /// at the page boundary.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for a zero offset or zero
    /// limit, [`io::ErrorKind::InvalidData`] when the source is not UTF-8 —
    /// including invalid bytes after the shown window — and the reader's
    /// own error otherwise.
    pub fn scan_text(
        &self,
        reader: impl Read,
        offset: u64,
        limit: Option<u64>,
    ) -> io::Result<TextPage> {
        scan(reader, offset, limit, self)
    }
}

/// The accumulating state of one scan. `output` is bounded by
/// `max_bytes`; `line_buf` is bounded by `max_bytes` and stops growing the
/// moment a line can no longer be returned, while `line_len` still counts
/// it to the full `u64` — a giant line never forces a giant buffer.
struct Scan<'a> {
    bounds: &'a PageLimits,
    offset: u64,
    limit: u64,
    original_bytes: u64,
    total_newlines: u64,
    line_len: u64,
    line_buf: Vec<u8>,
    selected: u64,
    shown: u32,
    output: Vec<u8>,
    stop: Option<PageEnd>,
    first_too_large: Option<u64>,
    trailing_empty: bool,
}

impl Scan<'_> {
    /// The 1-based index of the element currently being accumulated.
    fn index(&self) -> u64 {
        self.total_newlines + 1
    }

    /// Whether the element at `index` falls inside the requested window.
    fn in_selection(&self, index: u64) -> bool {
        index >= self.offset && self.selected < self.limit
    }

    /// Whether bytes of the current line still need staging: a line is
    /// staged only while it can still be returned — inside the selection,
    /// before any stop, and while it still fits the byte budget.
    fn buffering(&self) -> bool {
        self.stop.is_none()
            && self.in_selection(self.index())
            && self.line_buf.len() < self.bounds.max_bytes
    }

    /// Feeds validated bytes of the source into the line accounting.
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                self.finish_element(false);
                self.total_newlines += 1;
                self.line_len = 0;
                self.line_buf.clear();
            } else {
                self.line_len += 1;
                if self.buffering() {
                    self.line_buf.push(byte);
                }
            }
        }
    }

    /// Closes the current element: `final_element` marks the implicit
    /// last `split('\n')` element. An empty *last selected* element —
    /// the source's own trailing empty, or the element a `limit` ends
    /// the selection on — is exempt from the truncation budgets exactly
    /// alike: it is retained only when the whole selection, its
    /// separator included, fits.
    fn finish_element(&mut self, final_element: bool) {
        let index = self.index();
        if !self.in_selection(index) {
            return;
        }
        self.selected += 1;
        if self.line_len == 0 && (final_element || self.selected == self.limit) {
            self.trailing_empty = true;
            return;
        }
        if self.stop.is_some() {
            return;
        }
        if self.selected == 1 && self.line_len > self.bounds.max_bytes as u64 {
            self.first_too_large = Some(self.line_len);
            self.stop = Some(PageEnd::ByteLimit);
            return;
        }
        if self.shown as usize >= self.bounds.max_lines {
            self.stop = Some(PageEnd::LineLimit);
            return;
        }
        let separator = u64::from(self.shown > 0);
        if u64::try_from(self.output.len()).unwrap_or(u64::MAX) + separator + self.line_len
            > u64::try_from(self.bounds.max_bytes).unwrap_or(u64::MAX)
        {
            self.stop = Some(PageEnd::ByteLimit);
            return;
        }
        if separator == 1 {
            self.output.push(b'\n');
        }
        self.output.extend_from_slice(&self.line_buf);
        self.shown += 1;
    }

    /// Resolves the retained state into the page once the source is
    /// exhausted and fully validated.
    fn finish(mut self) -> TextPage {
        let total_lines = self.total_newlines + 1;
        if self.offset > total_lines {
            self.output.clear();
            self.shown = 0;
            self.stop = None;
            self.first_too_large = None;
        } else if self.stop.is_none() && self.trailing_empty {
            // The selected trailing empty stays whenever the whole
            // selection — its separator included — fits both budgets;
            // otherwise it is dropped and the byte budget reports the
            // stop, matching the complete-page rule exactly.
            let separator = usize::from(self.shown > 0);
            if self.output.len() + separator > self.bounds.max_bytes {
                self.stop = Some(PageEnd::ByteLimit);
            } else {
                if separator == 1 {
                    self.output.push(b'\n');
                }
                self.shown += 1;
            }
        }
        let end = if self.offset > total_lines {
            PageEnd::OffsetPastEnd
        } else if let Some(bytes) = self.first_too_large {
            PageEnd::FirstLineTooLarge { bytes }
        } else if let Some(stop) = self.stop {
            stop
        } else if self.limit < total_lines - (self.offset - 1) {
            // The requested window ended on the caller's `limit`, not on
            // the source: lines remain unshown by request.
            PageEnd::RequestedLimit
        } else {
            PageEnd::Complete
        };
        TextPage {
            text: String::from_utf8(self.output).expect("the scanner only retains validated bytes"),
            total_lines,
            original_bytes: self.original_bytes,
            shown_lines: self.shown,
            end,
        }
    }
}

fn scan(
    mut reader: impl Read,
    offset: u64,
    limit: Option<u64>,
    bounds: &PageLimits,
) -> io::Result<TextPage> {
    if offset == 0 || limit == Some(0) {
        return Err(invalid());
    }
    let mut scan = Scan {
        bounds,
        offset,
        limit: limit.unwrap_or(u64::MAX),
        original_bytes: 0,
        total_newlines: 0,
        line_len: 0,
        line_buf: Vec::new(),
        selected: 0,
        shown: 0,
        output: Vec::new(),
        stop: None,
        first_too_large: None,
        trailing_empty: false,
    };
    scan.original_bytes = scan_utf8_blocks(&mut reader, |valid| scan.feed(valid))?;
    scan.finish_element(true);
    Ok(scan.finish())
}

/// Streams `reader` to EOF through the whole-source UTF-8 validation the
/// line scanner performs, feeding every validated run to `feed` and
/// returning the exact byte count observed. At most the three tail bytes
/// of an incomplete sequence carry between blocks; an invalid sequence
/// fails at the first invalid byte and a source ending mid-sequence is
/// invalid too.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidData`] for non-UTF-8 content and the
/// reader's own errors otherwise.
fn scan_utf8_blocks(mut reader: impl Read, mut feed: impl FnMut(&[u8])) -> io::Result<u64> {
    let mut original_bytes = 0_u64;
    let mut carry = Vec::new();
    let mut block = vec![0u8; READ_BLOCK_BYTES];
    loop {
        // A reader may report an interrupted call: nothing was consumed,
        // so the scan simply retries the same block.
        let read = match reader.read(&mut block) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            break;
        }
        original_bytes += read as u64;
        carry.extend_from_slice(&block[..read]);
        let valid = match std::str::from_utf8(&carry) {
            Ok(_) => carry.len(),
            Err(error) => {
                if error.error_len().is_some() {
                    return Err(invalid_utf8());
                }
                error.valid_up_to()
            }
        };
        feed(&carry[..valid]);
        carry.drain(..valid);
    }
    if !carry.is_empty() {
        return Err(invalid_utf8());
    }
    Ok(original_bytes)
}

/// Whole-source UTF-8 validation without any line accounting: streams
/// `reader` to EOF and returns the exact observed byte count. The grep
/// engine validates a file this way before matching — a file invalid
/// anywhere, even after its last match, is skipped entirely.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidData`] for non-UTF-8 content and the
/// reader's own errors otherwise.
pub fn validate_utf8(reader: impl Read) -> io::Result<u64> {
    scan_utf8_blocks(reader, |_| ())
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid text page request")
}

fn invalid_utf8() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "source is not UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustx_execution_protocol::{MAX_TEXT_BYTES, MAX_TEXT_LINES};

    /// The test policy is the wire policy — the same constants the Client
    /// hands the scanner.
    fn limits() -> PageLimits {
        PageLimits {
            max_bytes: MAX_TEXT_BYTES,
            max_lines: MAX_TEXT_LINES,
        }
    }

    fn scan_bytes(source: &[u8], offset: u64, limit: Option<u64>) -> io::Result<TextPage> {
        limits().scan_text(source, offset, limit)
    }

    /// Feeds the source one byte at a time, forcing every multi-byte
    /// boundary through the carry path.
    fn scan_byte_at_a_time(source: &[u8], offset: u64, limit: Option<u64>) -> io::Result<TextPage> {
        struct OneByte<'a>(&'a [u8]);
        impl Read for OneByte<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                buffer[0] = self.0[0];
                self.0 = &self.0[1..];
                Ok(1)
            }
        }
        limits().scan_text(OneByte(source), offset, limit)
    }

    #[test]
    fn an_empty_source_is_one_addressable_line() {
        let page = scan_bytes(b"", 1, None).unwrap();
        assert_eq!(
            page,
            TextPage {
                text: String::new(),
                total_lines: 1,
                original_bytes: 0,
                shown_lines: 1,
                end: PageEnd::Complete,
            }
        );
        assert!(scan_bytes(b"", 2, None).is_ok_and(|page| page.end == PageEnd::OffsetPastEnd));
    }

    #[test]
    fn a_trailing_newline_keeps_one_extra_addressable_line() {
        let page = scan_bytes(b"a\n", 1, None).unwrap();
        assert_eq!(page.text, "a\n");
        assert_eq!(page.total_lines, 2);
        assert_eq!(page.shown_lines, 2);
        assert_eq!(page.end, PageEnd::Complete);
        // The trailing empty line is addressable too.
        let tail = scan_bytes(b"a\n", 2, None).unwrap();
        assert_eq!(tail.text, "");
        assert_eq!(tail.shown_lines, 1);
        assert_eq!(tail.end, PageEnd::Complete);
    }

    #[test]
    fn carriage_returns_and_bom_are_preserved_verbatim() {
        let source = b"\xef\xbb\xbfone\r\ntwo\r\n".as_slice();
        let page = scan_bytes(source, 1, None).unwrap();
        assert_eq!(page.text, "\u{feff}one\r\ntwo\r\n");
        assert_eq!(page.total_lines, 3);
        assert_eq!(page.original_bytes, source.len() as u64);
    }

    #[test]
    fn the_requested_limit_ends_the_selection_not_the_validation() {
        let source = b"one\ntwo\nthree".as_slice();
        let page = scan_bytes(source, 1, Some(2)).unwrap();
        assert_eq!(page.text, "one\ntwo");
        assert_eq!(page.shown_lines, 2);
        assert_eq!(page.total_lines, 3);
        assert_eq!(page.end, PageEnd::RequestedLimit);
        // A limit covering the remainder is a complete page, not a
        // requested-limit one.
        let full = scan_bytes(source, 1, Some(5)).unwrap();
        assert_eq!(full.end, PageEnd::Complete);
        assert_eq!(full.shown_lines, 3);
        assert!(scan_bytes(source, 1, Some(0)).is_err());
    }

    #[test]
    fn an_offset_past_the_end_reports_the_total() {
        let page = scan_bytes(b"one\ntwo", 5, None).unwrap();
        assert_eq!(page.end, PageEnd::OffsetPastEnd);
        assert_eq!(page.total_lines, 2);
        assert_eq!(page.shown_lines, 0);
        assert_eq!(page.text, "");
    }

    #[test]
    fn invalid_utf8_anywhere_is_rejected_even_after_the_window() {
        let mut source = b"one\n".to_vec();
        source.extend_from_slice(&[0xff, 0xfe]);
        assert_eq!(
            scan_bytes(&source, 1, Some(1)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        // A multi-byte character split across every boundary is still
        // text — byte-at-a-time feeding cannot break the carry.
        let mut unicode = "héllo\n".as_bytes().to_vec();
        unicode.extend_from_slice("wörld".as_bytes());
        let page = scan_byte_at_a_time(&unicode, 1, None).unwrap();
        assert_eq!(page.text, "héllo\nwörld");
        assert_eq!(page.total_lines, 2);
        let mut tail_bad = "héllo\n".as_bytes().to_vec();
        tail_bad.extend_from_slice(&[0xc3]);
        assert_eq!(
            scan_byte_at_a_time(&tail_bad, 1, None).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn a_first_line_beyond_the_budget_is_counted_not_staged() {
        let mut source = vec![b'x'; MAX_TEXT_BYTES + 1];
        source.extend_from_slice(b"\nrest");
        let page = scan_bytes(&source, 1, None).unwrap();
        assert_eq!(
            page.end,
            PageEnd::FirstLineTooLarge {
                bytes: (MAX_TEXT_BYTES + 1) as u64
            }
        );
        assert_eq!(page.shown_lines, 0);
        assert_eq!(page.text, "");
        assert_eq!(page.total_lines, 2);
        // A later offset skips it: the same file pages normally then.
        let rest = scan_bytes(&source, 2, None).unwrap();
        assert_eq!(rest.text, "rest");
        assert_eq!(rest.end, PageEnd::Complete);
    }

    #[test]
    fn the_byte_budget_includes_separators_and_the_terminal_newline() {
        // Exactly the budget of content plus a final newline still fits.
        let mut exact = vec![b'y'; MAX_TEXT_BYTES - 1];
        exact.push(b'\n');
        let page = scan_bytes(&exact, 1, None).unwrap();
        assert_eq!(page.end, PageEnd::Complete);
        assert_eq!(page.shown_lines, 2);
        assert_eq!(page.text.len(), MAX_TEXT_BYTES);
        // One more content byte pushes the terminal separator over: the
        // bounded page drops it and reports the byte budget.
        let mut over = vec![b'y'; MAX_TEXT_BYTES];
        over.push(b'\n');
        let page = scan_bytes(&over, 1, None).unwrap();
        assert_eq!(page.end, PageEnd::ByteLimit);
        assert_eq!(page.shown_lines, 1);
        assert_eq!(page.text.len(), MAX_TEXT_BYTES);
        assert!(!page.text.ends_with('\n'));
    }

    #[test]
    fn the_line_budget_stops_at_the_counted_limit() {
        let mut source = Vec::new();
        for _ in 0..MAX_TEXT_LINES + 5 {
            source.extend_from_slice(b"l\n");
        }
        let page = scan_bytes(&source, 1, None).unwrap();
        assert_eq!(page.end, PageEnd::LineLimit);
        assert_eq!(page.shown_lines, u32::try_from(MAX_TEXT_LINES).unwrap());
        assert_eq!(page.total_lines, (MAX_TEXT_LINES + 6) as u64);
        // 2000 counted lines plus the trailing empty fit exactly: the
        // complete page may show 2001 elements.
        let mut fitting = Vec::new();
        for _ in 0..MAX_TEXT_LINES {
            fitting.extend_from_slice(b"l\n");
        }
        let page = scan_bytes(&fitting, 1, None).unwrap();
        assert_eq!(page.end, PageEnd::Complete);
        assert_eq!(page.shown_lines, u32::try_from(MAX_TEXT_LINES + 1).unwrap());
        assert!(page.text.ends_with('\n'));
    }

    #[test]
    fn a_limit_ending_on_an_empty_element_keeps_it_like_the_final_one() {
        // 2000 counted lines, then the empty element the caller's limit
        // selects, then more source: the last selected empty is exempt
        // from the budgets exactly like the source's own trailing empty —
        // counting it would falsely stop at the line budget.
        let mut source = Vec::new();
        for _ in 0..MAX_TEXT_LINES {
            source.extend_from_slice(b"x\n");
        }
        source.extend_from_slice(b"\nrest");
        let limit = u64::try_from(MAX_TEXT_LINES + 1).unwrap();
        let page = scan_bytes(&source, 1, Some(limit)).unwrap();
        assert_eq!(page.end, PageEnd::RequestedLimit);
        assert_eq!(page.shown_lines, u32::try_from(MAX_TEXT_LINES + 1).unwrap());
        assert!(page.text.ends_with('\n'));
        assert_eq!(page.total_lines, u64::try_from(MAX_TEXT_LINES + 2).unwrap());
    }

    #[test]
    fn a_last_selected_empty_that_cannot_fit_is_dropped() {
        // The exemption keeps the empty element only when the whole
        // selection — its separator included — fits the byte budget.
        let mut source = vec![b'y'; MAX_TEXT_BYTES];
        source.extend_from_slice(b"\n\nrest");
        let page = scan_bytes(&source, 1, Some(2)).unwrap();
        assert_eq!(page.end, PageEnd::ByteLimit);
        assert_eq!(page.shown_lines, 1);
        assert_eq!(page.text.len(), MAX_TEXT_BYTES);
    }

    #[test]
    fn an_interior_empty_element_is_an_ordinary_counted_line() {
        // Only the last selected empty is exempt: a mid-selection empty
        // element counts against the budgets like any other line.
        let page = scan_bytes(b"a\n\nb", 1, Some(2)).unwrap();
        assert_eq!(page.text, "a\n");
        assert_eq!(page.shown_lines, 2);
        assert_eq!(page.end, PageEnd::RequestedLimit);
        let full = scan_bytes(b"a\n\nb", 1, None).unwrap();
        assert_eq!(full.text, "a\n\nb");
        assert_eq!(full.shown_lines, 3);
        assert_eq!(full.end, PageEnd::Complete);
    }

    #[test]
    fn a_large_source_pages_late_ranges_with_a_fixed_result() {
        // ~1 MiB of short lines: a page near the end stays bounded and
        // the whole source is still validated and counted.
        let mut source = Vec::new();
        for index in 0..60_000u64 {
            source.extend_from_slice(format!("line{index}\n").as_bytes());
        }
        let page = scan_bytes(&source, 59_990, Some(5)).unwrap();
        assert_eq!(
            page.text,
            "line59989\nline59990\nline59991\nline59992\nline59993"
        );
        assert_eq!(page.shown_lines, 5);
        assert_eq!(page.total_lines, 60_001);
        assert_eq!(page.original_bytes, source.len() as u64);
        assert_eq!(page.end, PageEnd::RequestedLimit);
        // A mid-file byte-budget page keeps only its bounded prefix.
        let head = scan_bytes(&source, 1, None).unwrap();
        assert!(head.text.len() <= MAX_TEXT_BYTES);
        assert!(matches!(head.end, PageEnd::ByteLimit | PageEnd::LineLimit));
        assert!(head.shown_lines <= u32::try_from(MAX_TEXT_LINES + 1).unwrap());
    }

    #[test]
    fn a_zero_offset_is_invalid() {
        assert_eq!(
            scan_bytes(b"a", 0, None).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
