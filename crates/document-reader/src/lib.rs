//! The physical structured-document decoder shared by the rustX Agent
//! Server's native Read tool and the physical Execution Client.
//!
//! One rustX-owned whitelist of structured document formats — `.pdf`,
//! `.docx`, `.xlsx`, `.pptx` — decodes through the parser-only `xberg`
//! dependency into a deterministic Markdown projection. Everything else
//! stays an ordinary text source ([`SourceKind::Text`]).
//!
//! The ownership boundary is fixed: xberg only decodes. rustX owns the
//! supported-format policy (the closed [`DocumentFormat`] enum, never
//! derived from xberg's capability surface), the path contract
//! (classification happens on an already-authorized target), the resource
//! bounds (the [`MAX_DOCUMENT_SOURCE_BYTES`] pre-decode cap plus explicit
//! xberg limits), and the textual projection policy. xberg never sees a
//! URI, so URL fetching and crawling are unreachable by construction;
//! recursive embedded object extraction is impossible because rustX pins
//! xberg's `max_archive_depth` to 0, so a whitelisted document can never
//! pull embedded content back through the extraction pipeline.
//!
//! Inference is excluded twice: the `ocr`/`ocr-pipeline`/`layout`/ML
//! features are not compiled in at all, and `disable_ocr: true` is set at
//! runtime, so a scanned or text-less PDF deterministically fails instead
//! of triggering OCR.
//!
//! Buffering and bounds are honest: [`read_source`] buffers the complete
//! source in memory — bounded to [`MAX_DOCUMENT_SOURCE_BYTES`], never
//! streamed — because xberg's `from_bytes` API requires the whole document
//! up front, and [`decode`] similarly materializes the complete Markdown
//! projection before returning it (capped by the parser's own
//! `max_content_size` and re-verified against
//! [`MAX_PROJECTED_DOCUMENT_BYTES`]). The configured
//! [`DOCUMENT_DECODE_TIMEOUT_SECS`] is xberg's *cooperative* parser
//! timeout: the parser observes it between its own checkpoint polls, so it
//! bounds well-behaved parse work but is not a physical kill — a parser
//! stuck inside one unchecked operation still runs to its own completion.

use std::io::Read;
use std::path::Path;

use xberg::{ExtractInput, ExtractionConfig, OutputFormat, extractors::security::SecurityLimits};

/// The maximum bytes of one structured-document source handed to the
/// decoder. This is an extraction-resource bound, not a model-facing
/// output bound: decoding happens before the complete-line projection,
/// so a huge archive-backed document must be rejected before unbounded
/// parse work starts, not after.
pub const MAX_DOCUMENT_SOURCE_BYTES: usize = 32 * 1024 * 1024;

/// The maximum bytes of projected Markdown text one decoded document may
/// produce. The parser's `max_content_size` is set to the same value;
/// [`decode`] re-verifies the returned projection so the bound holds even
/// if the parser's accounting changes.
pub const MAX_PROJECTED_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;

/// The cooperative extraction timeout handed to xberg, in seconds. This
/// is *not* a physical kill guarantee: it is the deadline the parser
/// checks cooperatively between its own internal steps, so a decoder that
/// never reaches a checkpoint is not interrupted. The hard resource bound
/// remains [`MAX_DOCUMENT_SOURCE_BYTES`] plus the deterministic
/// `SecurityLimits` below.
pub const DOCUMENT_DECODE_TIMEOUT_SECS: u64 = 30;

/// The closed rustX-owned whitelist of structured document formats
/// decoded through xberg.
///
/// Adding a variant here is the only way a new model-facing format can
/// appear; xberg gaining support for another format never changes the
/// contract on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFormat {
    /// Portable Document Format.
    Pdf,
    /// Office Open XML word processing document.
    Docx,
    /// Office Open XML spreadsheet.
    Xlsx,
    /// Office Open XML presentation.
    Pptx,
}

impl DocumentFormat {
    /// The human-readable format label used in deterministic failure
    /// diagnostics.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Docx => "DOCX",
            Self::Xlsx => "XLSX",
            Self::Pptx => "PPTX",
        }
    }

    /// The exact MIME type handed to xberg. rustX chooses the decoder by
    /// extension and passes the MIME type explicitly, so xberg never
    /// content-sniffs and a mislabeled file fails in exactly one decoder.
    fn mime_type(self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Pptx => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            }
        }
    }
}

/// The rustX-owned source classification of one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// An ordinary byte source decoded as UTF-8 text by the faithful text
    /// path.
    Text,
    /// A whitelisted structured document decoded through xberg.
    Document(DocumentFormat),
}

/// Classifies one already-interpreted target by its extension.
///
/// The comparison is deterministic: the extension is matched
/// case-insensitively against the closed whitelist, and only the final
/// extension component participates. Content is never sniffed to expand or
/// redirect the whitelist; a mislabeled file is decoded by exactly the
/// decoder its extension selects.
#[must_use]
pub fn classify_source(target: &Path) -> SourceKind {
    let Some(extension) = target.extension().and_then(|e| e.to_str()) else {
        return SourceKind::Text;
    };
    let normalized = extension.to_ascii_lowercase();
    let format = match normalized.as_str() {
        "pdf" => DocumentFormat::Pdf,
        "docx" => DocumentFormat::Docx,
        "xlsx" => DocumentFormat::Xlsx,
        "pptx" => DocumentFormat::Pptx,
        _ => return SourceKind::Text,
    };
    SourceKind::Document(format)
}

/// The deterministic parser-only extraction configuration.
///
/// Every non-default choice is rustX policy, not xberg defaults:
///
/// - `disable_ocr: true` hard-disables OCR (including the automatic
///   scanned-PDF fallback xberg would otherwise apply under
///   `OcrStrategy::Auto`); a text-less PDF fails honestly instead.
/// - `use_cache: false` keeps reads side-effect free and repeatable.
/// - `enable_quality_processing: false` keeps the projection free of
///   heuristic content rewriting.
/// - `output_format: Markdown` is the deterministic textual projection.
/// - `max_archive_depth: 0` disables xberg's recursive embedded-object
///   extraction: its DOCX (`word/embeddings/`), PPTX (`ppt/embeddings/`),
///   and PDF embedded-file paths all short-circuit at depth 0, so a
///   whitelisted top-level document can never recursively feed embedded
///   content back into the extraction pipeline. This — not `from_bytes`
///   alone — is what keeps the decoded format surface equal to the rustX
///   whitelist.
/// - `security_limits` carries the explicit resource bounds.
/// - `extraction_timeout_secs` is xberg's cooperative timeout: the parser
///   checks it between its own internal steps, so it bounds well-behaved
///   parse work but cannot interrupt a parser stuck inside one unchecked
///   operation. The hard bound remains the source-size cap and the
///   deterministic expansion limits.
fn extraction_config() -> ExtractionConfig {
    ExtractionConfig {
        use_cache: false,
        enable_quality_processing: false,
        ocr: None,
        force_ocr: false,
        disable_ocr: true,
        output_format: OutputFormat::Markdown,
        // Structural recursion ban: with depth 0, embedded objects can
        // never expand the decoded content or the model-facing format
        // surface. (With recursion disabled, xberg's
        // `max_embedded_file_bytes` has no consumer on any whitelisted
        // path, so it is left at its default.)
        max_archive_depth: 0,
        security_limits: Some(SecurityLimits {
            // Active on the whitelisted decode paths (xberg SecurityBudget
            // accounting):
            // The maximum growth of any accumulated string during
            // extraction.
            max_content_size: MAX_PROJECTED_DOCUMENT_BYTES,
            // The parser-loop iteration budget.
            max_iterations: 10_000_000,
            // The XML element/entity nesting budget for OOXML part
            // parsing (xberg enforces the larger of the two depth
            // fields).
            max_xml_depth: 256,
            max_nesting_depth: 64,
            // The maximum length of one XML entity/attribute token.
            max_entity_length: 1024 * 1024,
            // The XLSX cell budget.
            max_table_cells: 200_000,
            // No consumer on the whitelisted decode paths (they guard
            // xberg's standalone archive extractor, which rustX never
            // routes to; DOCX enforces its own internal 10_000-entry
            // package bound instead). Kept only to fill the struct; they
            // are not rustX's active policy.
            max_archive_size: 64 * 1024 * 1024,
            max_compression_ratio: 100,
            max_files_in_archive: 4_096,
        }),
        extraction_timeout_secs: Some(DOCUMENT_DECODE_TIMEOUT_SECS),
        ..Default::default()
    }
}

/// A failure of one bounded source acquisition.
#[derive(Debug)]
pub enum SourceError {
    /// The source could not be read.
    Io(std::io::Error),
    /// The source offered more than [`MAX_DOCUMENT_SOURCE_BYTES`]: at
    /// least that many bytes plus one were available.
    OverLimit,
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "source read failed: {error}"),
            Self::OverLimit => write!(
                f,
                "source exceeds the {MAX_DOCUMENT_SOURCE_BYTES}-byte document bound"
            ),
        }
    }
}

impl std::error::Error for SourceError {}

/// Acquires one document source through a bounded read: no more than
/// [`MAX_DOCUMENT_SOURCE_BYTES`] of source data is handed to the decoder.
/// The reader consumes at most one extra byte to detect overflow; this is
/// a bound on input length, not on the allocator's capacity for the vector.
///
/// # Errors
///
/// Returns [`SourceError::Io`] when the reader fails and
/// [`SourceError::OverLimit`] when the source exceeds the bound.
pub fn read_source(reader: impl Read) -> Result<Vec<u8>, SourceError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_DOCUMENT_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(SourceError::Io)?;
    if bytes.len() > MAX_DOCUMENT_SOURCE_BYTES {
        return Err(SourceError::OverLimit);
    }
    Ok(bytes)
}

/// A failure of one structured-document decode.
#[derive(Debug)]
pub enum DecodeError {
    /// The supplied bytes exceed [`MAX_DOCUMENT_SOURCE_BYTES`]. The bound
    /// is enforced at this direct API too, not only inside
    /// [`read_source`]: no path into the parser may carry an oversized
    /// source.
    SourceTooLarge,
    /// The dedicated decode runtime could not start.
    Runtime(std::io::Error),
    /// The parser rejected the document: an extraction error, an exceeded
    /// parser budget, or no produced document. This parser diagnostic may
    /// contain input-derived information; it is not a wire error code.
    Failed(String),
    /// The document decoded but has no extractable text layer: rustX
    /// never performs OCR or image inference, so a scanned or text-less
    /// document fails honestly instead.
    NoTextLayer,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceTooLarge => write!(
                f,
                "source exceeds the {MAX_DOCUMENT_SOURCE_BYTES}-byte document bound"
            ),
            Self::Runtime(error) => write!(f, "document decode runtime failed: {error}"),
            Self::Failed(message) => write!(f, "document decode failed: {message}"),
            Self::NoTextLayer => write!(f, "document has no extractable text layer"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decodes one whitelisted document into its deterministic Markdown text.
///
/// The caller has already resolved and authorized the source; only the
/// bytes supplied here are ever handed to xberg, as in-memory bytes with
/// an explicit MIME type. The source bound is enforced even at this
/// direct API — a caller that acquired its bytes outside [`read_source`]
/// cannot push an oversized source into the parser. The returned
/// projection is verified against [`MAX_PROJECTED_DOCUMENT_BYTES`]
/// before it leaves: the owned parser string is truncated in place to
/// its projection form (trailing blank newlines dropped), never cloned.
///
/// The function is synchronous and intended to run on a blocking thread:
/// it drives xberg's async-only extraction API on its own current-thread
/// runtime instead of occupying a caller's async reactor.
///
/// # Errors
///
/// Returns [`DecodeError::SourceTooLarge`] when `bytes` exceeds
/// [`MAX_DOCUMENT_SOURCE_BYTES`], [`DecodeError::Runtime`] when the
/// decode runtime cannot start, [`DecodeError::Failed`] when the parser
/// rejects the document, and [`DecodeError::NoTextLayer`] when the
/// document has no extractable text layer.
pub fn decode(bytes: Vec<u8>, format: DocumentFormat) -> Result<String, DecodeError> {
    if bytes.len() > MAX_DOCUMENT_SOURCE_BYTES {
        return Err(DecodeError::SourceTooLarge);
    }
    let input = ExtractInput::from_bytes(bytes, format.mime_type(), None);
    let config = extraction_config();
    // The xberg extraction API is async-only. Decode is synchronous and
    // runs on a blocking thread, so it drives the extraction on its own
    // current-thread runtime instead of occupying an async reactor.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(DecodeError::Runtime)?;
    let output = runtime
        .block_on(xberg::extract(input, &config))
        .map_err(|error| DecodeError::Failed(error.to_string()))?;
    if let Some(error) = output.errors.first() {
        return Err(DecodeError::Failed(error.message.clone()));
    }
    let Some(document) = output.results.into_iter().next() else {
        return Err(DecodeError::Failed(String::from(
            "no document was produced",
        )));
    };

    // The projection cap is re-verified on the owned parser string before
    // it can leave: `max_content_size` is the parser's own accounting and
    // this check is the rustX-owned bound behind it.
    let mut content = document.content;
    if content.len() > MAX_PROJECTED_DOCUMENT_BYTES {
        return Err(DecodeError::Failed(String::from(
            "projected document exceeds the output bound",
        )));
    }
    if content.trim().is_empty() {
        return Err(DecodeError::NoTextLayer);
    }
    // The trailing-newline form of the projection is a rustX choice: the
    // renderer's trailing blank newlines are dropped so the projected line
    // sequence ends at the last content line. The owned string is
    // truncated in place — never cloned.
    let keep = content.trim_end_matches('\n').len();
    content.truncate(keep);
    Ok(content)
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::path::Path;

    use super::{
        DecodeError, DocumentFormat, MAX_DOCUMENT_SOURCE_BYTES, SourceError, SourceKind,
        classify_source, decode, read_source,
    };

    #[test]
    fn classification_matches_the_closed_whitelist_case_insensitively() {
        assert_eq!(
            classify_source(Path::new("a/report.PDF")),
            SourceKind::Document(DocumentFormat::Pdf)
        );
        assert_eq!(
            classify_source(Path::new("a/b.DocX")),
            SourceKind::Document(DocumentFormat::Docx)
        );
        assert_eq!(
            classify_source(Path::new("sheet.XLSX")),
            SourceKind::Document(DocumentFormat::Xlsx)
        );
        assert_eq!(
            classify_source(Path::new("/host/deck.Pptx")),
            SourceKind::Document(DocumentFormat::Pptx)
        );
    }

    #[test]
    fn classification_leaves_every_other_extension_on_the_text_path() {
        // Ordinary sources, including formats xberg itself could decode.
        for name in [
            "main.rs",
            "notes.md",
            "legacy.doc",
            "book.epub",
            "data.xls",
            "deck.ppt",
        ] {
            assert_eq!(
                classify_source(Path::new(name)),
                SourceKind::Text,
                "{name} must stay on the text path"
            );
        }
        // No extension and dotfiles stay on the text path.
        assert_eq!(classify_source(Path::new("Makefile")), SourceKind::Text);
        assert_eq!(classify_source(Path::new(".docx")), SourceKind::Text);
        // Only the final extension participates.
        assert_eq!(
            classify_source(Path::new("report.pdf.txt")),
            SourceKind::Text
        );
    }

    /// The bounded acquisition accepts a source exactly at the limit and
    /// never buffers more than limit+1 bytes.
    #[test]
    fn bounded_reader_accepts_exactly_the_limit() {
        let bytes: Vec<u8> = (0..=255u8).cycle().take(64).collect();
        let read = read_source_limited(std::io::Cursor::new(bytes.clone()), 64);
        assert_eq!(read.expect("at the limit is accepted"), bytes);
        // An empty source is trivially within the bound.
        let read = read_source_limited(std::io::Cursor::new(Vec::new()), 8);
        assert_eq!(read.expect("empty is accepted"), [] as [u8; 0]);
    }

    /// The production [`read_source`] is bound to the document cap; the
    /// generic bounded read behind it is exercised here at a small fixture
    /// limit so the limit+1 boundary is testable without a 32 MiB buffer.
    fn read_source_limited<R: std::io::Read>(
        reader: R,
        limit: usize,
    ) -> Result<Vec<u8>, SourceError> {
        let mut bytes = Vec::new();
        reader
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(SourceError::Io)?;
        if bytes.len() > limit {
            return Err(SourceError::OverLimit);
        }
        Ok(bytes)
    }

    /// The bounded acquisition rejects a source one byte over the limit;
    /// the buffered result never exceeds the bound.
    #[test]
    fn bounded_reader_rejects_one_byte_over_the_limit() {
        let bytes = vec![7u8; 9];
        let read = read_source_limited(std::io::Cursor::new(bytes), 8);
        assert!(
            matches!(read, Err(SourceError::OverLimit)),
            "limit+1 must be rejected"
        );
    }

    /// `read_source` enforces the production bound: a source larger than
    /// [`MAX_DOCUMENT_SOURCE_BYTES`] is rejected by the read itself. The
    /// oversized tail is produced lazily — no 32 MiB buffer is allocated.
    #[test]
    fn bounded_reader_rejects_past_the_document_bound() {
        struct Endless;
        impl std::io::Read for Endless {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                buffer.fill(7);
                Ok(buffer.len())
            }
        }
        let read = read_source(Endless);
        assert!(matches!(read, Err(SourceError::OverLimit)));
    }

    /// The source bound is enforced at the direct decode API too: bytes
    /// acquired outside [`read_source`] cannot reach the parser.
    #[test]
    fn decode_rejects_an_oversized_source_before_parsing() {
        let bytes = vec![0u8; MAX_DOCUMENT_SOURCE_BYTES + 1];
        let error = decode(bytes, DocumentFormat::Docx).expect_err("over the bound");
        assert!(matches!(error, DecodeError::SourceTooLarge));
    }

    /// A committed fixture decodes to deterministic nonempty Markdown
    /// through the real parser — the same decode path the product runs.
    #[test]
    fn decode_projects_a_whitelisted_document() {
        let bytes = std::fs::read(fixture("docx/small.docx")).expect("committed fixture");
        let text = decode(bytes, DocumentFormat::Docx).expect("fixture decodes");
        assert!(text.contains("## Quarterly Report"), "unexpected: {text}");
        assert!(!text.ends_with('\n'));
    }

    /// A text-less document reports the honest no-text-layer failure, never
    /// an OCR fallback — no OCR pipeline is compiled in.
    #[test]
    fn decode_refuses_a_text_less_document() {
        let bytes = std::fs::read(fixture("pdf/scanned-no-text.pdf")).expect("committed fixture");
        let error = decode(bytes, DocumentFormat::Pdf).expect_err("no text layer");
        assert!(matches!(error, DecodeError::NoTextLayer));
    }

    /// A corrupt whitelisted source is a parser failure, not a panic.
    #[test]
    fn decode_reports_a_corrupt_document() {
        let error = decode(vec![b'%'; 256], DocumentFormat::Pdf).expect_err("corrupt source");
        assert!(matches!(error, DecodeError::Failed(_)));
    }

    /// One committed corpus fixture inside the repository.
    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/read/documents")
            .join(name)
    }
}
