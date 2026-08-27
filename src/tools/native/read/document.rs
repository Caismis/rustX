//! The rustX-owned structured-document source classification and decode of
//! native Read (Issue #48).
//!
//! Read supports exactly one rustX-owned whitelist of structured document
//! formats — `.pdf`, `.docx`, `.xlsx`, `.pptx` — decoded through the
//! parser-only `xberg` dependency into a deterministic Markdown projection.
//! Everything else stays an ordinary text source.
//!
//! The ownership boundary is fixed: xberg only decodes. rustX owns the
//! supported-format policy (this closed enum, never derived from xberg's
//! capability surface), the path contract (classification happens on the
//! already-authorized, already-resolved Read target), the resource bounds
//! (an explicit source-size cap plus explicit xberg `SecurityLimits`), and
//! the textual projection policy. xberg never sees a URI, so URL fetching,
//! crawling, and archive-recursive ingestion are unreachable by
//! construction.
//!
//! Inference is excluded twice: the `ocr`/`ocr-pipeline`/`layout`/ML
//! features are not compiled in at all, and `disable_ocr: true` is set at
//! runtime, so a scanned or text-less PDF deterministically fails instead
//! of triggering OCR.

use std::path::Path;

use xberg::{ExtractInput, ExtractionConfig, OutputFormat, extractors::security::SecurityLimits};

use crate::tools::limits::MAX_DOCUMENT_SOURCE_BYTES;

/// The closed rustX-owned whitelist of structured document formats Read
/// decodes through xberg.
///
/// Adding a variant here is the only way a new model-facing format can
/// appear; xberg gaining support for another format never changes the Read
/// contract on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DocumentFormat {
    Pdf,
    Docx,
    Xlsx,
    Pptx,
}

impl DocumentFormat {
    /// The human-readable format label used in deterministic failure
    /// diagnostics.
    pub(super) fn label(self) -> &'static str {
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

/// The rustX-owned source classification of one resolved Read target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceKind {
    /// An ordinary byte source decoded as UTF-8 text by the faithful text
    /// path.
    Text,
    /// A whitelisted structured document decoded through xberg.
    Document(DocumentFormat),
}

/// Classifies one already-interpreted Read target by its extension.
///
/// The comparison is deterministic: the extension is matched
/// case-insensitively against the closed whitelist, and only the final
/// extension component participates. Content is never sniffed to expand or
/// redirect the whitelist; a mislabeled file is decoded by exactly the
/// decoder its extension selects.
pub(super) fn classify_source(target: &Path) -> SourceKind {
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

/// The deterministic parser-only extraction configuration of rustX Read.
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
/// - `security_limits` carries the explicit resource bounds.
/// - No wall-clock timeout: extraction is bounded by the source-size cap
///   and the deterministic expansion/element limits, so a correctness
///   failure never depends on timing.
fn extraction_config() -> ExtractionConfig {
    ExtractionConfig {
        use_cache: false,
        enable_quality_processing: false,
        ocr: None,
        force_ocr: false,
        disable_ocr: true,
        output_format: OutputFormat::Markdown,
        security_limits: Some(SecurityLimits {
            // The total uncompressed size one archive (every OOXML
            // document is a zip) may expand to.
            max_archive_size: 64 * 1024 * 1024,
            // The maximum compressed/uncompressed ratio before the input
            // is rejected as a decompression bomb.
            max_compression_ratio: 100,
            max_files_in_archive: 4_096,
            max_nesting_depth: 64,
            max_entity_length: 1024 * 1024,
            // The maximum growth of any accumulated string during
            // extraction.
            max_content_size: 64 * 1024 * 1024,
            max_iterations: 10_000_000,
            max_xml_depth: 256,
            max_table_cells: 200_000,
        }),
        max_embedded_file_bytes: None,
        extraction_timeout_secs: None,
        ..Default::default()
    }
}

/// Decodes one whitelisted document into its deterministic Markdown text.
///
/// The caller has already resolved and authorized `target` through the
/// ordinary Read path contract; only those bytes are ever handed to xberg,
/// as in-memory bytes with an explicit MIME type. The function is
/// synchronous and intended to run on a blocking thread.
pub(super) fn decode_document(target: &Path, format: DocumentFormat) -> Result<String, String> {
    let size = std::fs::metadata(target)
        .map_err(|error| format!("cannot read {}: {error}", target.display()))?
        .len();
    if size > MAX_DOCUMENT_SOURCE_BYTES as u64 {
        return Err(format!(
            "{} is {size} bytes; document reads are bounded to {MAX_DOCUMENT_SOURCE_BYTES} bytes",
            target.display()
        ));
    }
    let bytes = std::fs::read(target)
        .map_err(|error| format!("cannot read {}: {error}", target.display()))?;

    let input = ExtractInput::from_bytes(bytes, format.mime_type(), None);
    let config = extraction_config();
    // The xberg extraction API is async-only. Decode runs on a blocking
    // thread, so it drives the extraction on its own current-thread
    // runtime instead of occupying an async reactor.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start document decode runtime: {error}"))?;
    let output = runtime
        .block_on(xberg::extract(input, &config))
        .map_err(|error| {
            format!(
                "cannot decode {} as {}: {error}",
                target.display(),
                format.label()
            )
        })?;
    if let Some(error) = output.errors.first() {
        return Err(format!(
            "cannot decode {} as {}: {}",
            target.display(),
            format.label(),
            error.message
        ));
    }
    let Some(document) = output.results.first() else {
        return Err(format!(
            "cannot decode {} as {}: no document was produced",
            target.display(),
            format.label()
        ));
    };

    // The trailing-newline form of the projection is a rustX choice: the
    // renderer's trailing blank newlines are dropped so the projected line
    // sequence ends at the last content line.
    let content = document.content.trim_end_matches('\n');
    if content.trim().is_empty() {
        return Err(format!(
            "{} has no extractable text layer; rustX never performs OCR or image inference",
            target.display()
        ));
    }
    Ok(content.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{DocumentFormat, SourceKind, classify_source};

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

    /// Regenerates the committed fixture corpus after changing the
    /// generator. Run explicitly:
    /// `cargo test regenerate_committed_fixture_corpus -- --ignored`
    #[test]
    #[ignore = "writes the committed tests/fixtures/read/documents corpus"]
    fn regenerate_committed_fixture_corpus() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/read/documents");
        for (name, bytes) in super::super::testdata::corpus() {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("fixture directory");
            std::fs::write(&path, bytes).expect("fixture bytes");
        }
    }
}
