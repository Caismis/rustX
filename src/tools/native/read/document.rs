//! The native Read adapter over the shared structured-document decoder
//! (Issue #48).
//!
//! Read supports exactly one rustX-owned whitelist of structured document
//! formats — `.pdf`, `.docx`, `.xlsx`, `.pptx` — decoded through the
//! parser-only `xberg` backend inside `rustx-document-reader` into a
//! deterministic Markdown projection. Everything else stays an ordinary
//! text source. The format whitelist, classification, bounded acquisition,
//! and parser policy live in that shared crate — the same physical decoder
//! the Execution Client serves — so there is exactly one parser policy;
//! this adapter only maps its results onto the historical Read
//! diagnostics. The ownership boundary is fixed: xberg only decodes.
//! rustX owns the supported-format policy (the closed enum, never derived
//! from xberg's capability surface), the path contract (classification
//! happens on the already-authorized, already-resolved Read target), the
//! resource bounds, and the textual projection policy. xberg never sees a
//! URI, so URL fetching and crawling are unreachable by construction;
//! recursive embedded object extraction is impossible because rustX pins
//! xberg's `max_archive_depth` to 0.
//!
//! Inference is excluded twice: the `ocr`/`ocr-pipeline`/`layout`/ML
//! features are not compiled in at all, and `disable_ocr: true` is set at
//! runtime, so a scanned or text-less PDF deterministically fails instead
//! of triggering OCR.

use std::path::Path;

use rustx_document_reader::{DecodeError, SourceError};
pub(super) use rustx_document_reader::{DocumentFormat, SourceKind, classify_source};

use crate::tools::limits::MAX_DOCUMENT_SOURCE_BYTES;

/// The oversized-source diagnostic shared by the bounded-acquisition and
/// direct-decode refusals.
fn bounded_message(target: &Path) -> String {
    format!(
        "{} is at least {} bytes; document reads are bounded to {MAX_DOCUMENT_SOURCE_BYTES} bytes",
        target.display(),
        MAX_DOCUMENT_SOURCE_BYTES + 1
    )
}

/// Acquires and decodes one whitelisted document into its deterministic
/// Markdown text, through the shared `rustx-document-reader` decoder.
///
/// The caller has already resolved and authorized `target` through the
/// ordinary Read path contract; the source is acquired through the
/// shared bounded read — at most `MAX_DOCUMENT_SOURCE_BYTES` is ever
/// buffered or handed to xberg — and decoded by the shared parser policy.
/// The function is synchronous and intended to run on a blocking thread.
pub(super) fn decode_document(target: &Path, format: DocumentFormat) -> Result<String, String> {
    // Acquisition: no more than `MAX_DOCUMENT_SOURCE_BYTES` of source data
    // is ever buffered. The bound is enforced by the read itself — the
    // bytes that enter decoding are exactly the bytes read — so no
    // metadata race can admit an oversized source.
    let file = std::fs::File::open(target)
        .map_err(|error| format!("cannot read {}: {error}", target.display()))?;
    let bytes = match rustx_document_reader::read_source(file) {
        Ok(bytes) => bytes,
        Err(SourceError::Io(error)) => {
            return Err(format!("cannot read {}: {error}", target.display()));
        }
        Err(SourceError::OverLimit) => return Err(bounded_message(target)),
    };
    // The xberg decode pipeline starts here: everything before this line is
    // rustX-owned acquisition and bounding.
    #[cfg(test)]
    decode_hooks::record(target);
    match rustx_document_reader::decode(bytes, format) {
        Ok(text) => {
            // The decode-completion fact: the decode body reached its
            // successful completion point and is returning its result.
            // (This hook observes decode-body completion only; the
            // executor's `decode.await` is what establishes blocking-task
            // physical settlement.)
            #[cfg(test)]
            decode_hooks::record_decode_completed(target);
            Ok(text)
        }
        Err(DecodeError::SourceTooLarge) => Err(bounded_message(target)),
        Err(DecodeError::Runtime(error)) => {
            Err(format!("cannot start document decode runtime: {error}"))
        }
        Err(DecodeError::Failed(message)) => Err(format!(
            "cannot decode {} as {}: {message}",
            target.display(),
            format.label()
        )),
        Err(DecodeError::NoTextLayer) => Err(format!(
            "{} has no extractable text layer; rustX never performs OCR or image inference",
            target.display()
        )),
    }
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
        completions: Arc<AtomicUsize>,
        gate: Mutex<Option<mpsc::Receiver<()>>>,
        started: Mutex<Option<oneshot::Sender<()>>>,
        completed: Mutex<Option<oneshot::Sender<()>>>,
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

    /// The installed hook: start/completion counters, start/completion
    /// rendezvous receivers, and — for gated installs — the release handle.
    /// Uninstalling on drop keeps other tests unaffected.
    pub struct Installed {
        starts: Arc<AtomicUsize>,
        completions: Arc<AtomicUsize>,
        started: Option<oneshot::Receiver<()>>,
        completed: Option<oneshot::Receiver<()>>,
        release: Option<mpsc::Sender<()>>,
    }

    impl Installed {
        /// How many times the watched document entered the decoder.
        pub fn starts(&self) -> usize {
            self.starts.load(Ordering::SeqCst)
        }

        /// How many times the watched decode body reached its successful
        /// completion point.
        pub fn completed_count(&self) -> usize {
            self.completions.load(Ordering::SeqCst)
        }

        /// Resolves when the watched decoder has recorded its start.
        pub async fn wait_started(&mut self) {
            if let Some(receiver) = self.started.take() {
                let _ = receiver.await;
            }
        }

        /// Whether the watched decode body has already completed.
        pub fn decode_completed(&mut self) -> bool {
            self.completed
                .as_mut()
                .is_some_and(|receiver| receiver.try_recv().is_ok())
        }

        /// Resolves when the watched decode body has completed.
        pub async fn wait_decode_completed(&mut self) {
            if let Some(receiver) = self.completed.take() {
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

    /// Installs start/completion counters for the watched document.
    pub fn install(watch: &Path) -> Installed {
        install_inner(watch, false)
    }

    /// Installs start/completion counters for the watched document plus a
    /// gate: the decoder blocks right after recording its start until
    /// [`Installed::release`] is called. The start and completion edges
    /// are deterministic rendezvous points ([`Installed::wait_started`],
    /// [`Installed::wait_decode_completed`]).
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
        let (completed_sender, completed_receiver) = oneshot::channel();
        let starts = Arc::new(AtomicUsize::new(0));
        let completions = Arc::new(AtomicUsize::new(0));
        let installed = Installed {
            starts: Arc::clone(&starts),
            completions: Arc::clone(&completions),
            started: Some(started_receiver),
            completed: Some(completed_receiver),
            release,
        };
        *HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Hook {
            watch: watch.to_path_buf(),
            starts,
            completions,
            gate: Mutex::new(gate),
            started: Mutex::new(Some(started_sender)),
            completed: Mutex::new(Some(completed_sender)),
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

    /// Called by the decoder right before it returns its result: counts
    /// and signals decode completion for the installed hook watching this
    /// exact path. This records that the decode body reached its
    /// successful completion point; it says nothing about the Tokio
    /// blocking task's own settlement, which the production executor
    /// establishes with its `decode.await`.
    pub fn record_decode_completed(target: &Path) {
        let mut hook_guard = HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(hook) = hook_guard.as_mut() else {
            return;
        };
        if hook.watch != target {
            return;
        }
        hook.completions.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = hook
            .completed
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

    use super::DocumentFormat;

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
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/read/documents");
        for (name, bytes) in super::super::testdata::corpus() {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("fixture directory");
            std::fs::write(&path, bytes).expect("fixture bytes");
        }
    }
}
