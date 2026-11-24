//! Deterministic races against the Client Store's production owner and filesystem.

use super::*;
use rustx_process_supervisor::process_runner::{CapturedProcessResult, RunnerTestControl};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, oneshot};

struct BuildGate {
    staging: PathBuf,
    resume: oneshot::Sender<()>,
}

struct GatedRunner {
    count: AtomicUsize,
    staged: mpsc::UnboundedSender<BuildGate>,
}

impl SupervisedProcessRunner for GatedRunner {
    fn run(
        &self,
        spec: SupervisedCommandSpec,
        _control: Option<RunnerTestControl>,
    ) -> futures_util::future::BoxFuture<'_, Result<CapturedProcessResult, String>> {
        Box::pin(async move {
            let mut stdout = Vec::new();
            let mut intent = ProcessOutcomeIntent::Completed;
            if spec.command.ends_with(" --version") {
                stdout = if spec.command.contains("fixture-uv") {
                    b"uv 1.0.0\n".to_vec()
                } else {
                    b"Python 3.12.9\n".to_vec()
                };
            } else if spec.command.contains(" sync ") {
                self.count.fetch_add(1, Ordering::SeqCst);
                let (resume, resumed) = oneshot::channel();
                self.staged
                    .send(BuildGate {
                        staging: spec.cwd.clone(),
                        resume,
                    })
                    .expect("build gate observer");
                tokio::select! {
                    biased;
                    () = spec.cancellation.cancelled() => {
                        intent = ProcessOutcomeIntent::Cancelled;
                    }
                    release = resumed => {
                        release.expect("staged build owner");
                        let python = venv_python(&spec.cwd);
                        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
                        std::os::unix::fs::symlink("/bin/sh", python).unwrap();
                    }
                }
            } else {
                assert!(spec.command.contains(" lock "));
            }
            Ok(CapturedProcessResult {
                exit_code: (intent == ProcessOutcomeIntent::Completed).then_some(0),
                intent,
                physical_settlement_proven: true,
                stdout,
                stderr: Vec::new(),
            })
        })
    }
}

struct StoreFixture {
    root: tempfile::TempDir,
    package: PythonToolPackage,
    runner: Arc<GatedRunner>,
    stages: mpsc::UnboundedReceiver<BuildGate>,
}

impl StoreFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let authored = root.path().join("source");
        std::fs::create_dir(&authored).unwrap();
        std::fs::write(authored.join(SERVER_FILE), b"mcp = None\n").unwrap();
        std::fs::write(authored.join(REQUIREMENTS_FILE), b"").unwrap();
        let package = discover_package(
            &authored,
            "fixture",
            &ProcessSignal::new(),
            Arc::default(),
            None,
        )
        .unwrap()
        .0;
        let (notifications, stages) = mpsc::unbounded_channel();
        let runner = Arc::new(GatedRunner {
            count: AtomicUsize::new(0),
            staged: notifications,
        });
        Self {
            root,
            package,
            runner,
            stages,
        }
    }

    fn store(&self) -> PythonToolStore {
        let mut store = PythonToolStore::new(self.root.path().join("store")).unwrap();
        let inner = Arc::get_mut(&mut store.inner).unwrap();
        inner.runner = self.runner.clone();
        inner.uv_binary = PathBuf::from("/fixture-uv");
        inner.python_binary = PathBuf::from("/bin/sh");
        store
    }

    fn start(
        &self,
        store: PythonToolStore,
        cancellation: ProcessSignal,
    ) -> tokio::task::JoinHandle<Result<PreparedPythonPackage, PythonToolError>> {
        let package = self.package.clone();
        tokio::spawn(async move { store.ensure_prepared(&package, &cancellation).await })
    }

    async fn stage(&mut self) -> BuildGate {
        tokio::time::timeout(std::time::Duration::from_secs(5), self.stages.recv())
            .await
            .expect("build liveness guard")
            .expect("physical stage")
    }
}

#[tokio::test]
async fn same_store_joins_one_owner_and_waiter_drop_cannot_release_its_stage() {
    let mut fixture = StoreFixture::new();
    let store = fixture.store();
    let owner_signal = ProcessSignal::new();
    let owner = fixture.start(store.clone(), owner_signal.clone());
    let stage = fixture.stage().await;
    let attached_to = store
        .inner
        .in_flight
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    let unshared = Arc::strong_count(&attached_to);
    let waiter = fixture.start(store.clone(), ProcessSignal::new());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while Arc::strong_count(&attached_to) <= unshared {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("waiter joined the registered build");
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert_eq!(fixture.runner.count.load(Ordering::SeqCst), 1);
    assert!(stage.staging.is_dir());

    owner_signal.cancel();
    assert!(matches!(
        owner.await.unwrap(),
        Err(PythonToolError::Environment(_))
    ));
    assert!(!stage.staging.exists());
    assert!(store.inner.in_flight.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(store.inner.root.join(PACKAGES_DIRECTORY))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn independent_stores_reuse_the_exclusive_winner_and_refuse_corruption() {
    let mut fixture = StoreFixture::new();
    let first = fixture.store();
    let second = fixture.store();
    let primary = fixture.start(first.clone(), ProcessSignal::new());
    let winning = fixture.stage().await;
    let other = fixture.start(second.clone(), ProcessSignal::new());
    let losing = fixture.stage().await;
    assert_ne!(winning.staging, losing.staging);
    winning.resume.send(()).unwrap();
    let prepared = primary.await.unwrap().unwrap();
    let original = std::fs::metadata(&prepared.state_dir).unwrap();
    assert!(losing.staging.is_dir());
    losing.resume.send(()).unwrap();
    assert_eq!(other.await.unwrap().unwrap().state_dir, prepared.state_dir);
    assert!(!winning.staging.exists());
    assert!(!losing.staging.exists());
    assert_eq!(
        std::fs::metadata(&prepared.state_dir).unwrap().ino(),
        original.ino()
    );

    let manifest = prepared.state_dir.join(MANIFEST_FILE);
    let source = prepared.state_dir.join(SOURCE_DIRECTORY).join(SERVER_FILE);
    for path in [manifest, source] {
        let valid = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"invalid content").unwrap();
        assert!(matches!(
            second
                .ensure_prepared(&fixture.package, &ProcessSignal::new())
                .await,
            Err(PythonToolError::Storage(_)),
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid content");
        std::fs::write(path, valid).unwrap();
    }
    let python = venv_python(&prepared.state_dir);
    std::fs::remove_file(&python).unwrap();
    std::os::unix::fs::symlink("/bin/false", &python).unwrap();
    assert!(matches!(
        first
            .ensure_prepared(&fixture.package, &ProcessSignal::new())
            .await,
        Err(PythonToolError::Storage(_)),
    ));
    assert_eq!(python.read_link().unwrap(), Path::new("/bin/false"));
    assert_eq!(fixture.runner.count.load(Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read_dir(first.inner.root.join(PACKAGES_DIRECTORY))
            .unwrap()
            .count(),
        1
    );
}
