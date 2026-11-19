//! Test-only support for the native Read document path.
//!
//! Production document handling no longer lives here: the extension
//! whitelist and classification are the shared policy in
//! `rustx_execution_protocol`, physical acquisition and decode belong to
//! the Execution Client through `rustx-document-reader`, and the tool
//! dispatches the page exchange through the Session's execution scope.
//! What remains in this module is test support: deterministic
//! exchange-boundary hooks the cancellation proofs synchronize on, the
//! committed-corpus regeneration entry point, and parser-boundary tests
//! that exercise `rustx-document-reader` (a dev-dependency) directly —
//! parser detail belongs at the parser boundary, never at the Agent one.

/// Deterministic exchange-synchronization hooks for tests. A test
/// installs a hook watching one exact path at one of two deterministic
/// cuts inside the admitted exchange: `Admit` — the acceptance gate,
/// after the request is encoded and the binding check passes but before
/// the first request byte — or `Sent` — after the full request frame is
/// written, before the reply is read. The worker records its entry at
/// the installed cut and, when the hook is gated, blocks there until
/// the test releases it. A successful reply is recorded separately, so
/// tests can prove an accepted exchange's reply was awaited. No
/// production behavior reads any of this.
pub(crate) mod read_hooks {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    /// The deterministic cut a hook observes inside one exchange.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Cut {
        /// The acceptance gate: after encode + binding check, before the
        /// first request byte — a cancellation observed here still
        /// yields zero request frames.
        Admit,
        /// The post-send cut: the request frame is fully written and the
        /// reply has not been read — the accepted exchange is on the
        /// wire and must be awaited to settlement.
        Sent,
    }

    struct Hook {
        watch: PathBuf,
        cut: Cut,
        starts: Arc<AtomicUsize>,
        replies: Arc<AtomicUsize>,
        gate: Mutex<Option<mpsc::Receiver<()>>>,
        started: Mutex<Option<oneshot::Sender<()>>>,
        replied: Mutex<Option<oneshot::Sender<()>>>,
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

    /// The installed hook: dispatch/reply counters, dispatch/reply
    /// rendezvous receivers, and — for gated installs — the release
    /// handle. Uninstalling on drop keeps other tests unaffected.
    pub struct Installed {
        starts: Arc<AtomicUsize>,
        replies: Arc<AtomicUsize>,
        started: Option<oneshot::Receiver<()>>,
        replied: Option<oneshot::Receiver<()>>,
        release: Option<mpsc::Sender<()>>,
    }

    impl Installed {
        /// How many times the watched target entered the dispatch worker.
        pub fn starts(&self) -> usize {
            self.starts.load(Ordering::SeqCst)
        }

        /// How many times the watched exchange produced a successful
        /// reply.
        pub fn replies(&self) -> usize {
            self.replies.load(Ordering::SeqCst)
        }

        /// Resolves when the watched worker has recorded its dispatch
        /// entry.
        pub async fn wait_started(&mut self) {
            if let Some(receiver) = self.started.take() {
                let _ = receiver.await;
            }
        }

        /// Whether the watched exchange has already replied.
        pub fn replied(&mut self) -> bool {
            self.replied
                .as_mut()
                .is_some_and(|receiver| receiver.try_recv().is_ok())
        }

        /// Resolves when the watched exchange has produced a successful
        /// reply.
        pub async fn wait_replied(&mut self) {
            if let Some(receiver) = self.replied.take() {
                let _ = receiver.await;
            }
        }

        /// Releases a gated worker.
        pub fn release(&mut self) {
            if let Some(sender) = self.release.take() {
                sender.send(()).expect("dispatch gate receiver is alive");
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

    /// Installs admission/reply counters for the watched target at the
    /// acceptance-gate cut plus a gate: the worker blocks inside the
    /// acceptance gate — before the linearized `with_active` grant —
    /// until [`Installed::release`] is called. The admission and reply
    /// edges are deterministic rendezvous points
    /// ([`Installed::wait_started`], [`Installed::wait_replied`]).
    pub fn install_gated(watch: &Path) -> Installed {
        install_inner(watch, Cut::Admit, true)
    }

    /// Installs send/reply counters for the watched target at the
    /// post-send cut: `starts` counts request frames actually written.
    pub fn install_sent(watch: &Path) -> Installed {
        install_inner(watch, Cut::Sent, false)
    }

    /// Installs send/reply counters for the watched target at the
    /// post-send cut plus a gate: the worker blocks with the request
    /// frame already on the wire and the reply unread until
    /// [`Installed::release`] is called.
    pub fn install_sent_gated(watch: &Path) -> Installed {
        install_inner(watch, Cut::Sent, true)
    }

    fn install_inner(watch: &Path, cut: Cut, gated: bool) -> Installed {
        let (release, gate) = if gated {
            let (sender, receiver) = mpsc::channel();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let (started_sender, started_receiver) = oneshot::channel();
        let (replied_sender, replied_receiver) = oneshot::channel();
        let starts = Arc::new(AtomicUsize::new(0));
        let replies = Arc::new(AtomicUsize::new(0));
        let installed = Installed {
            starts: Arc::clone(&starts),
            replies: Arc::clone(&replies),
            started: Some(started_receiver),
            replied: Some(replied_receiver),
            release,
        };
        *HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Hook {
            watch: watch.to_path_buf(),
            cut,
            starts,
            replies,
            gate: Mutex::new(gate),
            started: Mutex::new(Some(started_sender)),
            replied: Mutex::new(Some(replied_sender)),
        });
        installed
    }

    /// Called inside the exchange's acceptance gate — after the request
    /// is encoded and the binding check passes, before the first request
    /// byte and before the linearized `with_active` grant. Counts the
    /// entry for an installed admit-cut hook watching this exact path,
    /// signals the start rendezvous, and — when gated — blocks the
    /// dispatching thread until the test releases it.
    pub fn record_admit(target: &Path) {
        record_at(target, Cut::Admit);
    }

    /// Called after the whole request frame is on the wire, before the
    /// reply is read. Same contract as [`record_admit`] at the post-send
    /// cut.
    pub fn record_sent(target: &Path) {
        record_at(target, Cut::Sent);
    }

    fn record_at(target: &Path, cut: Cut) {
        let mut hook_guard = HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(hook) = hook_guard.as_mut() else {
            return;
        };
        if hook.watch != target || hook.cut != cut {
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
            // Blocking the dedicated dispatch thread is exactly the
            // point: the test controls when the exchange may proceed.
            let _ = receiver.recv();
        }
    }

    /// Called by the executor when the exchange produced a successful
    /// reply: counts and signals it for the installed hook watching this
    /// exact path. This records that the accepted operation's reply was
    /// received; it says nothing about the blocking task's own
    /// settlement, which the production executor establishes with its
    /// exchange await.
    pub fn record_reply(target: &Path) {
        let mut hook_guard = HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(hook) = hook_guard.as_mut() else {
            return;
        };
        if hook.watch != target {
            return;
        }
        hook.replies.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = hook
            .replied
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

    /// The rustX-owned source-size bound rejects an oversized document at
    /// the parser boundary itself: both the bounded acquisition and the
    /// direct decode API refuse before a parser step runs. The file is
    /// sparse: only its length matters.
    #[test]
    fn oversized_sources_never_reach_the_parser() {
        let directory = tempfile::tempdir().expect("temporary root");
        let path = directory.path().join("huge.docx");
        let file = std::fs::File::create(&path).expect("sparse fixture");
        file.set_len((rustx_execution_protocol::MAX_DOCUMENT_SOURCE_BYTES as u64) + 1)
            .expect("sparse length");
        drop(file);

        assert!(matches!(
            rustx_document_reader::read_source(std::fs::File::open(&path).expect("fixture open")),
            Err(rustx_document_reader::SourceError::OverLimit)
        ));
        assert!(matches!(
            rustx_document_reader::decode(
                vec![0u8; rustx_execution_protocol::MAX_DOCUMENT_SOURCE_BYTES + 1],
                rustx_document_reader::DocumentFormat::Docx,
            ),
            Err(rustx_document_reader::DecodeError::SourceTooLarge)
        ));
    }

    /// The decoder's OOXML member bound is a parser-boundary contract: two
    /// otherwise equivalent packages from the same generator differ only
    /// in member count, and only the one above the bound fails — with the
    /// parser's own classification, never relayed across the Agent
    /// boundary.
    #[test]
    fn archive_member_accounting_is_a_parser_boundary_contract() {
        use super::super::testdata;
        let under = rustx_document_reader::decode(
            testdata::docx_with_member_count(testdata::DOCX_BASE_MEMBERS),
            rustx_document_reader::DocumentFormat::Docx,
        )
        .expect("package under the member bound decodes");
        assert!(under.contains("member accounting control"));

        let error = rustx_document_reader::decode(
            testdata::docx_with_member_count(10_001),
            rustx_document_reader::DocumentFormat::Docx,
        )
        .expect_err("package over the member bound");
        let rustx_document_reader::DecodeError::Failed(message) = error else {
            panic!("the member bound is a parse refusal: {error:?}");
        };
        assert!(
            message.contains("exceeds limit of 10000"),
            "the member bound, not package malformation, must reject the package: {message}"
        );
    }
}
