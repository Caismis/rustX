//! Actual per-attempt workspace changes. Private snapshots and file captures live
//! with the runtime, never enter model input, and never mutate the user's index.
mod capture;
mod command;
mod comparison;
use comparison::FileContents;
use rustx_execution_protocol::changes::FileComparison;
mod fingerprint;
pub(super) use fingerprint::WorkspaceObservation;
mod git;
mod storage;
use storage::SnapshotDirectory;

use capture::Capture;
use git::Repository;
use rustx_execution_protocol::changes::{ChangedFile, ChangesSummary, MAX_CHANGE_FILES};
use rustx_process_supervisor::signal::ProcessSignal;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

enum FileSource {
    Git { before: String, after: String },
    Captured(Capture, Capture),
}
struct Recorded {
    summary: ChangesSummary,
    sources: Vec<FileSource>,
    repository: Option<Repository>,
    trees: Option<(String, String)>,
    _directory: Arc<SnapshotDirectory>,
    _capture_directory: Option<Arc<tempfile::TempDir>>,
}

/// One runtime's immutable completed records. Dropping it releases all snapshots.
#[derive(Default)]
pub(crate) struct ChangesStore {
    records: Mutex<BTreeMap<String, Arc<Recorded>>>,
    repository: tokio::sync::OnceCell<Option<Repository>>,
    directory: Arc<OnceLock<Arc<SnapshotDirectory>>>,
}
impl ChangesStore {
    pub(crate) fn summary(&self, attempt: &str) -> Option<ChangesSummary> {
        self.records
            .lock()
            .expect("workspace changes")
            .get(attempt)
            .map(|record| record.summary.clone())
    }

    fn release(&self, attempt: &str) {
        self.records
            .lock()
            .expect("workspace changes")
            .remove(attempt);
    }

    #[cfg(test)]
    async fn comparison(
        &self,
        attempt: &str,
        index: usize,
    ) -> Result<Option<FileComparison>, String> {
        self.compare(attempt, index, &ProcessSignal::new()).await
    }

    async fn compare(
        &self,
        attempt: &str,
        index: usize,
        cancellation: &ProcessSignal,
    ) -> Result<Option<FileComparison>, String> {
        let record = self
            .records
            .lock()
            .expect("workspace changes")
            .get(attempt)
            .cloned();
        let Some(record) = record else {
            return Ok(None);
        };
        let source = record
            .sources
            .get(index)
            .ok_or("invalid changed file index")?;
        if record.summary.files[index].binary {
            return Ok(Some(FileComparison {
                hunks: Vec::new(),
                binary: true,
                oversized: false,
                truncated: false,
            }));
        }
        let (before, after) = match source {
            FileSource::Captured(before, after) => (before.bytes()?, after.bytes()?),
            FileSource::Git { before, after } => {
                let repo = record.repository.as_ref().ok_or("snapshot unavailable")?;
                let (old, new) = record.trees.as_ref().ok_or("snapshot unavailable")?;
                (
                    repo.bytes(old, before, cancellation).await?,
                    repo.bytes(new, after, cancellation).await?,
                )
            }
        };
        tokio::task::spawn_blocking(move || {
            Some(FileContents::from_bytes(before, after).comparison())
        })
        .await
        .map_err(|error| error.to_string())
    }
}

struct Baseline {
    repository: Repository,
    tree: String,
}
type BaselineTask = tokio_util::task::AbortOnDropHandle<Result<Option<Baseline>, String>>;

/// Attempt-bound file capture authority, shared only with foreground file tools.
pub(crate) struct ChangeRecording {
    attempt: String,
    cancellation: ProcessSignal,
    cwd: PathBuf,
    root: Arc<rustx_confined_fs::PackageDirectory>,
    directory: Arc<OnceLock<Arc<SnapshotDirectory>>>,
    task: tokio::sync::Mutex<Option<BaselineTask>>,
    baseline: Mutex<Option<Baseline>>,
    captures: Mutex<BTreeMap<PathBuf, Capture>>,
    capture_directory: OnceLock<Arc<tempfile::TempDir>>,
    #[cfg(test)]
    used: AtomicBool,
    failed: AtomicBool,
    store: Arc<ChangesStore>,
}

impl ChangeRecording {
    /// Start the baseline concurrently with model inference. Tools await it
    /// before dispatch, so no operation can race ahead of its baseline.
    pub(crate) fn start(
        attempt: String,
        cwd: PathBuf,
        root: Arc<rustx_confined_fs::PackageDirectory>,
        store: Arc<ChangesStore>,
        cancellation: ProcessSignal,
    ) -> Arc<Self> {
        let directory = Arc::clone(&store.directory);
        let session = Arc::clone(&store);
        let scratch = Arc::clone(&directory);
        let directory_path = cwd.clone();
        let observation_cancellation = cancellation.clone();
        let task = Some(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
            async move {
                let repository = session
                    .repository
                    .get_or_try_init(|| {
                        Repository::locate(
                            &directory_path,
                            scratch,
                            observation_cancellation.clone(),
                        )
                    })
                    .await?
                    .clone();
                let Some(mut repository) = repository else {
                    return Ok(None);
                };
                repository.cancellation = observation_cancellation;
                let tree = repository.snapshot().await?;
                Ok(Some(Baseline { repository, tree }))
            },
        )));
        Arc::new(Self {
            attempt,
            cancellation,
            cwd,
            root,
            directory,
            task: tokio::sync::Mutex::new(task),
            baseline: Mutex::new(None),
            captures: Mutex::new(BTreeMap::new()),
            capture_directory: OnceLock::new(),
            #[cfg(test)]
            used: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            store,
        })
    }

    #[cfg(test)]
    pub(crate) async fn before_tools(&self) {
        self.ready().await;
        self.used.store(true, Ordering::Relaxed);
    }

    async fn ready(&self) {
        let mut task = self.task.lock().await;
        if let Some(task) = task.take() {
            match task.await {
                Ok(Ok(baseline)) => *self.baseline.lock().expect("workspace baseline") = baseline,
                Ok(Err(error)) => {
                    self.failed.store(true, Ordering::Relaxed);
                    tracing::warn!(%error, "workspace snapshot unavailable");
                }
                Err(error) => {
                    self.failed.store(true, Ordering::Relaxed);
                    tracing::warn!(%error, "workspace snapshot task failed");
                }
            }
        }
    }

    /// Resolved, guarded mutation target, captured once before its first edit.
    pub(crate) fn capture(&self, path: &Path) {
        let Ok(relative) = path.strip_prefix(&self.cwd) else {
            return;
        };
        // Serialize initialization with captures so concurrent tools allocate at most
        // one private directory, and only when a file actually needs capture.
        let mut captures = self.captures.lock().expect("workspace captures");
        if self.directory.get().is_none() {
            match create_snapshot_directory(&std::env::temp_dir()) {
                Ok(directory) => {
                    let _ = self.directory.set(Arc::new(directory));
                }
                Err(error) => {
                    self.failed.store(true, Ordering::Relaxed);
                    tracing::warn!(%error, "workspace change recording unavailable");
                    return;
                }
            }
        }
        let directory = self
            .directory
            .get()
            .expect("snapshot directory initialized");
        if directory.contains(path) {
            return;
        }
        if captures.contains_key(path) {
            return;
        }
        if captures.len() == MAX_CHANGE_FILES {
            self.failed.store(true, Ordering::Relaxed);
            return;
        }
        if self.capture_directory.get().is_none() {
            match tempfile::Builder::new()
                .prefix("attempt-capture-")
                .tempdir_in(directory.path())
            {
                Ok(directory) => {
                    let _ = self.capture_directory.set(Arc::new(directory));
                }
                Err(error) => {
                    self.failed.store(true, Ordering::Relaxed);
                    tracing::warn!(%error, "workspace capture storage unavailable");
                    return;
                }
            }
        }
        let directory = self
            .capture_directory
            .get()
            .expect("capture directory initialized");
        match Capture::read(&self.root, relative, directory.path()) {
            Ok(Some(capture)) => {
                captures.insert(path.to_owned(), capture);
            }
            Ok(None) => {}
            Err(error) => {
                self.failed.store(true, Ordering::Relaxed);
                tracing::warn!(%error, path=%path.display(), "workspace file capture unavailable");
            }
        }
    }

    /// The conversation runtime owns this optional observation before terminal
    /// publication. Cancellation abandons the card immediately; command drivers
    /// keep ownership of their children and scratch files until physical reaping.
    #[cfg(test)]
    pub(crate) async fn finish(&self) {
        if self.cancellation.is_cancelled() {
            self.task.lock().await.take();
            return;
        }
        if !self.used.load(Ordering::Relaxed) {
            // DSH samples at turn start, including while the model is waiting.
            // Settle that work before a later turn reuses the session index.
            self.ready().await;
            return;
        }
        tokio::select! {
            biased;
            () = self.cancellation.cancelled() => {}
            () = self.finish_active() => {}
        }
    }

    #[cfg(test)]
    async fn finish_active(&self) {
        self.ready().await;
        if self.failed.load(Ordering::Relaxed) {
            return;
        }
        if let Err(error) = self.record().await {
            tracing::warn!(%error, "workspace change summary unavailable");
        }
    }

    #[allow(clippy::too_many_lines)] // Assemble one internally consistent summary and its comparison sources.
    async fn record(&self) -> Result<(), String> {
        if self.failed.load(Ordering::Relaxed) {
            return Err("workspace capture unavailable".into());
        }
        self.root.verify().map_err(|error| error.to_string())?;
        let baseline = self.baseline.lock().expect("workspace baseline").take();
        let mut files = BTreeMap::<PathBuf, (ChangedFile, FileSource)>::new();
        let mut captures = self.captures.lock().expect("workspace captures").clone();
        let (mut repository, mut repository_root, mut trees) = (None, None, None);
        if let Some(Baseline {
            repository: repo,
            tree: before,
        }) = baseline
        {
            repository_root = Some(repo.root.clone());
            let after = repo.snapshot().await?;
            let changes = resolve_git_changes(&repo, &before, &after, &mut captures).await?;
            for (_, change) in changes {
                let path = repo.root.join(&change.path);
                // Snapshot rows win over file-tool copies, as in DSH.
                captures.remove(&path);
                if !path.starts_with(&self.cwd) {
                    continue;
                }
                let display = display_path(&self.cwd, &path, Some(&repo.root));
                files.insert(
                    path.clone(),
                    (
                        ChangedFile {
                            path: path.to_string_lossy().into_owned(),
                            display,
                            added: change.added,
                            deleted: change.deleted,
                            binary: change.binary,
                            oversized: false,
                        },
                        FileSource::Git {
                            before: change.old_path,
                            after: change.path,
                        },
                    ),
                );
            }
            if !files.is_empty() {
                trees = Some((before, after));
                repository = Some(repo);
            }
        }
        let Some(directory) = self.directory.get() else {
            return Ok(());
        };
        if let Some(capture_directory) = self.capture_directory.get().cloned() {
            let cwd = self.cwd.clone();
            let root = Arc::clone(&self.root);
            files = tokio::task::spawn_blocking(move || {
                capture_endpoints(
                    &mut files,
                    &captures,
                    &cwd,
                    &root,
                    capture_directory.path(),
                    repository_root.as_deref(),
                )?;
                Ok::<_, String>(files)
            })
            .await
            .map_err(|error| error.to_string())??;
        }
        if files.is_empty() {
            return Ok(());
        }
        let total = files.len();
        let added = files.values().map(|(file, _)| file.added).sum();
        let deleted = files.values().map(|(file, _)| file.deleted).sum();
        // Presentation may be lossy or shared by different paths; only the
        // resolved path owns identity. Stable sorting keeps equal labels in
        // deterministic path order without merging their comparison sources.
        let mut files = files.into_values().collect::<Vec<_>>();
        files.sort_by(|(left, _), (right, _)| left.display.cmp(&right.display));
        let mut summary_bytes = 0;
        let (files, sources) = files
            .into_iter()
            .take(MAX_CHANGE_FILES)
            .take_while(|(file, _)| {
                summary_bytes += file.path.len() + file.display.len() + 128;
                summary_bytes <= 96 * 1024
            })
            .unzip();
        let record = Recorded {
            summary: ChangesSummary {
                files,
                total,
                added,
                deleted,
            },
            sources,
            repository,
            trees,
            _directory: Arc::clone(directory),
            _capture_directory: self.capture_directory.get().cloned(),
        };
        self.root.verify().map_err(|error| error.to_string())?;
        // Cancellation and publication share a grant boundary. A detached
        // blocking file comparison can never publish after cancellation wins.
        let _ = self.cancellation.with_active(|| {
            self.store
                .records
                .lock()
                .expect("workspace changes")
                .insert(self.attempt.clone(), Arc::new(record));
        });
        Ok(())
    }
}

impl Drop for ChangeRecording {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

async fn resolve_git_changes(
    repo: &Repository,
    before: &str,
    after: &str,
    captures: &mut BTreeMap<PathBuf, Capture>,
) -> Result<BTreeMap<String, git::Change>, String> {
    let changes = repo.changes(before, after).await?;
    captures.retain(|path, _| {
        !changes.contains_key(
            path.strip_prefix(&repo.root)
                .unwrap_or(path)
                .to_string_lossy()
                .as_ref(),
        )
    });
    if !captures.is_empty() {
        let paths = captures.keys().cloned().collect::<Vec<_>>();
        let uncovered = repo.uncovered(&paths).await?;
        captures.retain(|path, _| uncovered.contains(path));
    }
    Ok(changes)
}

fn capture_endpoints(
    files: &mut BTreeMap<PathBuf, (ChangedFile, FileSource)>,
    captures: &BTreeMap<PathBuf, Capture>,
    cwd: &Path,
    root: &rustx_confined_fs::PackageDirectory,
    directory: &Path,
    repository: Option<&Path>,
) -> Result<(), String> {
    for (path, before) in captures {
        if files.contains_key(path) {
            continue;
        }
        let Ok(relative) = path.strip_prefix(cwd) else {
            continue;
        };
        let Some(after) = Capture::read(root, relative, directory)? else {
            continue;
        };
        if before == &after && *before != Capture::Oversized {
            continue;
        }
        let comparison = FileContents::from_bytes(before.bytes()?, after.bytes()?);
        let (added, deleted) = comparison.counts();
        files.insert(
            path.clone(),
            (
                ChangedFile {
                    path: path.to_string_lossy().into_owned(),
                    display: display_path(cwd, path, repository),
                    added,
                    deleted,
                    binary: comparison.binary,
                    oversized: comparison.oversized,
                },
                FileSource::Captured(before.clone(), after),
            ),
        );
    }
    Ok(())
}

fn create_snapshot_directory(temporary_root: &Path) -> std::io::Result<SnapshotDirectory> {
    SnapshotDirectory::create(temporary_root)
}

/// Native mutation paths are already resolved. Compare them with the physical
/// temporary root too (for example macOS /var aliases /private/var).
#[cfg(test)]
fn is_temporary_path(path: &Path, temporary_root: &Path) -> bool {
    temporary_root
        .canonicalize()
        .is_ok_and(|root| path.starts_with(root))
}

fn display_path(cwd: &Path, path: &Path, repository: Option<&Path>) -> String {
    if let Ok(relative) = path.strip_prefix(cwd) {
        return relative
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
    }
    if repository.is_some_and(|root| path.starts_with(root)) {
        let mut parent = cwd.parent();
        let mut prefix = "../".to_owned();
        while let Some(directory) = parent {
            if let Ok(relative) = path.strip_prefix(directory) {
                return format!(
                    "{prefix}{}",
                    relative
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/")
                );
            }
            parent = directory.parent();
            prefix.push_str("../");
        }
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        && let Ok(relative) = path.strip_prefix(home)
    {
        return format!(
            "~/{}",
            relative
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        );
    }
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

mod registry;
pub(super) use registry::Changes;

#[cfg(test)]
mod tests;
