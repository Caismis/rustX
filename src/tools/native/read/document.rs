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
//! (a hard pre-decode source-size cap plus explicit xberg limits), and
//! the textual projection policy. xberg never sees a URI, so URL fetching
//! and crawling are unreachable by construction; recursive embedded
//! object extraction is impossible because rustX pins xberg's
//! `max_archive_depth` to 0, so a whitelisted document can never pull
//! embedded content back through the extraction pipeline.
//!
//! Inference is excluded twice: the `ocr`/`ocr-pipeline`/`layout`/ML
//! features are not compiled in at all, and `disable_ocr: true` is set at
//! runtime, so a scanned or text-less PDF deterministically fails instead
//! of triggering OCR.

use std::io::Read as _;
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
/// - `max_archive_depth: 0` disables xberg's recursive embedded-object
///   extraction: its DOCX (`word/embeddings/`), PPTX (`ppt/embeddings/`),
///   and PDF embedded-file paths all short-circuit at depth 0, so a
///   whitelisted top-level document can never recursively feed embedded
///   content back into the extraction pipeline. This — not `from_bytes`
///   alone — is what keeps the decoded format surface equal to the rustX
///   whitelist.
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
            max_content_size: 64 * 1024 * 1024,
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
        extraction_timeout_secs: None,
        ..Default::default()
    }
}

/// A failure of one bounded source acquisition.
#[derive(Debug)]
enum BoundedReadError {
    /// The source could not be read.
    Io(std::io::Error),
    /// The source offered more than the bound: at least `limit + 1` bytes
    /// were available.
    OverLimit,
}

/// Reads at most `limit` bytes from `reader`, deterministically rejecting a
/// larger source. At most `limit + 1` bytes are ever buffered, so no
/// allocation grows with an oversized input before the bound is enforced.
fn read_bounded<R: std::io::Read>(reader: R, limit: usize) -> Result<Vec<u8>, BoundedReadError> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(BoundedReadError::Io)?;
    if bytes.len() > limit {
        return Err(BoundedReadError::OverLimit);
    }
    Ok(bytes)
}

/// Acquires the document source through one bounded read of the opened
/// file: no more than `MAX_DOCUMENT_SOURCE_BYTES` of source data is ever
/// buffered or handed to xberg. The bound is enforced by the read itself —
/// the bytes that enter decoding are exactly the bytes this function
/// returned — so no metadata race can admit an oversized source.
fn read_document_source_bounded(target: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(target)
        .map_err(|error| format!("cannot read {}: {error}", target.display()))?;
    match read_bounded(file, MAX_DOCUMENT_SOURCE_BYTES) {
        Ok(bytes) => Ok(bytes),
        Err(BoundedReadError::Io(error)) => {
            Err(format!("cannot read {}: {error}", target.display()))
        }
        Err(BoundedReadError::OverLimit) => Err(format!(
            "{} is at least {} bytes; document reads are bounded to {MAX_DOCUMENT_SOURCE_BYTES} bytes",
            target.display(),
            MAX_DOCUMENT_SOURCE_BYTES + 1
        )),
    }
}

/// Decodes one whitelisted document into its deterministic Markdown text.
///
/// The caller has already resolved and authorized `target` through the
/// ordinary Read path contract; only those bytes are ever handed to xberg,
/// as in-memory bytes with an explicit MIME type. The function is
/// synchronous and intended to run on a blocking thread.
pub(super) fn decode_document(target: &Path, format: DocumentFormat) -> Result<String, String> {
    let bytes = read_document_source_bounded(target)?;
    // The xberg decode pipeline starts here: everything before this line is
    // rustX-owned acquisition and bounding.
    #[cfg(test)]
    decode_hooks::record(target);

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
    // The physical-finish fact: the blocking decode is done with xberg and
    // is returning its result.
    #[cfg(test)]
    decode_hooks::record_finish(target);
    Ok(content.to_owned())
}

/// Deterministic decode-synchronization hooks for tests. A test installs a
/// hook watching one exact document path; the decoder records its start
/// there and, when the hook is gated, blocks until the test releases it.
/// No production behavior reads any of this.
#[cfg(test)]
pub(super) mod decode_hooks {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    struct Hook {
        watch: PathBuf,
        starts: Arc<AtomicUsize>,
        finishes: Arc<AtomicUsize>,
        gate: Mutex<Option<mpsc::Receiver<()>>>,
        started: Mutex<Option<oneshot::Sender<()>>>,
        finished: Mutex<Option<oneshot::Sender<()>>>,
    }

    static HOOK: Mutex<Option<Hook>> = Mutex::new(None);

    /// Serializes the tests that install hooks, so one test's install can
    /// never overwrite another's while it is still observing.
    pub fn lock_session() -> std::sync::MutexGuard<'static, ()> {
        SESSION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    static SESSION: Mutex<()> = Mutex::new(());

    /// The installed hook: start/finish counters, start/finish rendezvous
    /// receivers, and — for gated installs — the release handle.
    /// Uninstalling on drop keeps other tests unaffected.
    pub struct Installed {
        starts: Arc<AtomicUsize>,
        finishes: Arc<AtomicUsize>,
        started: Option<oneshot::Receiver<()>>,
        finished: Option<oneshot::Receiver<()>>,
        release: Option<mpsc::Sender<()>>,
    }

    impl Installed {
        /// How many times the watched document entered the decoder.
        pub fn starts(&self) -> usize {
            self.starts.load(Ordering::SeqCst)
        }

        /// How many times the watched decoder physically finished.
        pub fn finished_count(&self) -> usize {
            self.finishes.load(Ordering::SeqCst)
        }

        /// Resolves when the watched decoder has recorded its start.
        pub async fn wait_started(&mut self) {
            if let Some(receiver) = self.started.take() {
                let _ = receiver.await;
            }
        }

        /// Whether the watched decoder has already physically finished.
        pub fn physically_finished(&mut self) -> bool {
            self.finished
                .as_mut()
                .is_some_and(|receiver| receiver.try_recv().is_ok())
        }

        /// Resolves when the watched decoder has physically finished.
        pub async fn wait_physically_finished(&mut self) {
            if let Some(receiver) = self.finished.take() {
                let _ = receiver.await;
            }
        }

        /// Releases a gated decoder.
        pub fn release(&mut self) {
            if let Some(sender) = self.release.take() {
                sender.send(()).expect("decoder gate receiver is alive");
            }
        }
    }

    impl Drop for Installed {
        fn drop(&mut self) {
            *HOOK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }

    /// Installs start/finish counters for the watched document.
    pub fn install(watch: &Path) -> Installed {
        install_inner(watch, false)
    }

    /// Installs start/finish counters for the watched document plus a gate:
    /// the decoder blocks right after recording its start until
    /// [`Installed::release`] is called. The start and finish edges are
    /// deterministic rendezvous points ([`Installed::wait_started`],
    /// [`Installed::wait_physically_finished`]).
    pub fn install_gated(watch: &Path) -> Installed {
        install_inner(watch, true)
    }

    fn install_inner(watch: &Path, gated: bool) -> Installed {
        let (release, gate) = if gated {
            let (sender, receiver) = mpsc::channel();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let (started_sender, started_receiver) = oneshot::channel();
        let (finished_sender, finished_receiver) = oneshot::channel();
        let starts = Arc::new(AtomicUsize::new(0));
        let finishes = Arc::new(AtomicUsize::new(0));
        let installed = Installed {
            starts: Arc::clone(&starts),
            finishes: Arc::clone(&finishes),
            started: Some(started_receiver),
            finished: Some(finished_receiver),
            release,
        };
        *HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Hook {
            watch: watch.to_path_buf(),
            starts,
            finishes,
            gate: Mutex::new(gate),
            started: Mutex::new(Some(started_sender)),
            finished: Mutex::new(Some(finished_sender)),
        });
        installed
    }

    /// Called by the decoder when the xberg decode pipeline is entered.
    /// Counts the start for the installed hook watching this exact path,
    /// signals the start rendezvous, and — if that hook is gated — blocks
    /// the decoding thread until the test releases it.
    pub fn record(target: &Path) {
        let mut hook_guard = HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(hook) = hook_guard.as_mut() else {
            return;
        };
        if hook.watch != target {
            return;
        }
        hook.starts.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = hook
            .started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(());
        }
        let receiver = hook
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(hook_guard);
        if let Some(receiver) = receiver {
            // Blocking the dedicated decode thread is exactly the point:
            // the test controls when the decode may proceed.
            let _ = receiver.recv();
        }
    }

    /// Called by the decoder right before it returns its result: counts and
    /// signals the physical-finish fact for the installed hook watching
    /// this exact path.
    pub fn record_finish(target: &Path) {
        let mut hook_guard = HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(hook) = hook_guard.as_mut() else {
            return;
        };
        if hook.watch != target {
            return;
        }
        hook.finishes.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = hook
            .finished
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(());
        }
    }
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

    /// The bounded acquisition primitive accepts a source exactly at the
    /// limit and never buffers more than limit+1 bytes.
    #[test]
    fn bounded_reader_accepts_exactly_the_limit() {
        let bytes: Vec<u8> = (0..=255u8).cycle().take(64).collect();
        let read = super::read_bounded(std::io::Cursor::new(bytes.clone()), 64);
        assert_eq!(read.expect("at the limit is accepted"), bytes);
        // An empty source is trivially within the bound.
        let read = super::read_bounded(std::io::Cursor::new(Vec::new()), 8);
        assert!(read.expect("empty is accepted").is_empty());
    }

    /// The bounded acquisition primitive rejects a source one byte over the
    /// limit; the buffered result never exceeds the bound.
    #[test]
    fn bounded_reader_rejects_one_byte_over_the_limit() {
        let bytes = vec![7u8; 9];
        let read = super::read_bounded(std::io::Cursor::new(bytes), 8);
        assert!(
            matches!(read, Err(super::BoundedReadError::OverLimit)),
            "limit+1 must be rejected"
        );
    }

    /// An oversized document source is rejected by the bounded acquisition
    /// itself, so the decoder hook proves the xberg decode is never even
    /// entered. The file is sparse: only its length matters.
    #[test]
    fn oversized_sources_never_reach_the_decoder() {
        let directory = tempfile::tempdir().expect("temporary root");
        let path = directory.path().join("huge.docx");
        let file = std::fs::File::create(&path).expect("sparse fixture");
        file.set_len((crate::tools::limits::MAX_DOCUMENT_SOURCE_BYTES as u64) + 1)
            .expect("sparse length");
        drop(file);

        let _session = super::decode_hooks::lock_session();
        let hook = super::decode_hooks::install(&path);
        let error =
            super::decode_document(&path, DocumentFormat::Docx).expect_err("over the bound");
        assert!(
            error.contains(&format!(
                "document reads are bounded to {} bytes",
                crate::tools::limits::MAX_DOCUMENT_SOURCE_BYTES
            )),
            "unexpected failure: {error}"
        );
        assert_eq!(hook.starts(), 0, "an oversized source must never decode");
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
