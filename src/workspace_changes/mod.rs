//! Actual per-attempt workspace changes. Private snapshots and file captures live
//! with the runtime, never enter model input, and never mutate the user's index.
mod capture;
mod command;
mod comparison;
use comparison::FileContents;
pub use comparison::{ComparisonHunk, FileComparison};
mod git;

use crate::runtime::identity::AttemptId;
use capture::Capture;
use git::Repository;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_FILES: usize = 500;

/// One file's actual net change during an attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangedFile {
    /// Absolute resolved Host path; never accepted from a comparison request.
    pub path: String,
    pub display: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
    pub oversized: bool,
}

/// Bounded paths and counts; whole file content is read only on demand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangesSummary {
    pub files: Vec<ChangedFile>,
    pub total: usize,
    pub added: usize,
    pub deleted: usize,
}

enum FileSource {
    Git { before: String, after: String },
    Captured(Capture, Capture),
}
struct Recorded {
    summary: ChangesSummary,
    sources: Vec<FileSource>,
    repository: Option<Repository>,
    trees: Option<(String, String)>,
    _directory: Arc<tempfile::TempDir>,
}

/// One runtime's immutable completed records. Dropping it releases all snapshots.
#[derive(Default)]
pub(crate) struct ChangesStore(Mutex<BTreeMap<String, Arc<Recorded>>>);
impl ChangesStore {
    pub(crate) fn summary(&self, attempt: &AttemptId) -> Option<ChangesSummary> {
        self.0
            .lock()
            .expect("workspace changes")
            .get(attempt.as_str())
            .map(|record| record.summary.clone())
    }

    pub(crate) async fn comparison(
        &self,
        attempt: &AttemptId,
        index: usize,
    ) -> Result<Option<FileComparison>, String> {
        let record = self
            .0
            .lock()
            .expect("workspace changes")
            .get(attempt.as_str())
            .cloned();
        let Some(record) = record else {
            return Ok(None);
        };
        let source = record
            .sources
            .get(index)
            .ok_or("invalid changed file index")?;
        let (before, after) = match source {
            FileSource::Captured(before, after) => (before.bytes()?, after.bytes()?),
            FileSource::Git { before, after } => {
                let repo = record.repository.as_ref().ok_or("snapshot unavailable")?;
                let (old, new) = record.trees.as_ref().ok_or("snapshot unavailable")?;
                (
                    repo.bytes(old, before).await?,
                    repo.bytes(new, after).await?,
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
    attempt: AttemptId,
    cwd: PathBuf,
    directory: Arc<tempfile::TempDir>,
    task: tokio::sync::Mutex<Option<BaselineTask>>,
    baseline: Mutex<Option<Baseline>>,
    captures: Mutex<BTreeMap<PathBuf, Capture>>,
    used: AtomicBool,
    failed: AtomicBool,
    store: Arc<ChangesStore>,
}

impl ChangeRecording {
    /// Start the baseline concurrently with model inference. Tools await it
    /// before dispatch, so no operation can race ahead of its baseline.
    pub(crate) fn start(
        attempt: AttemptId,
        cwd: PathBuf,
        store: Arc<ChangesStore>,
    ) -> Option<Arc<Self>> {
        let directory = match tempfile::Builder::new().prefix("rustx-changes-").tempdir() {
            Ok(directory) => Arc::new(directory),
            Err(error) => {
                tracing::warn!(%error, "workspace change recording unavailable");
                return None;
            }
        };
        let root = cwd.clone();
        let scratch = Arc::clone(&directory);
        let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            let Some(repository) = Repository::locate(&root, scratch.path()).await? else {
                return Ok(None);
            };
            let tree = repository.snapshot().await?;
            Ok(Some(Baseline { repository, tree }))
        }));
        Some(Arc::new(Self {
            attempt,
            cwd,
            directory,
            task: tokio::sync::Mutex::new(Some(task)),
            baseline: Mutex::new(None),
            captures: Mutex::new(BTreeMap::new()),
            used: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            store,
        }))
    }

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
        // Temporary output outside the workspace is not a user deliverable.
        if path.starts_with(std::env::temp_dir())
            && !path.starts_with(&self.cwd)
            && !self
                .baseline
                .lock()
                .expect("workspace baseline")
                .as_ref()
                .is_some_and(|base| path.starts_with(&base.repository.root))
        {
            return;
        }
        let mut captures = self.captures.lock().expect("workspace captures");
        if captures.contains_key(path) {
            return;
        }
        match Capture::read(path, self.directory.path()) {
            Ok(Some(capture)) => {
                captures.insert(path.to_owned(), capture);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, path=%path.display(), "workspace file capture unavailable");
            }
        }
    }

    /// Record before publishing the Attempt terminal, so its first settled
    /// client observation can already read a complete card.
    pub(crate) async fn finish(&self) {
        if !self.used.load(Ordering::Relaxed) {
            // A plain reply or cancellation before tools needs no endpoint.
            // Dropping the owned task aborts its bounded Git child as well.
            self.task.lock().await.take();
            return;
        }
        self.ready().await;
        if self.failed.load(Ordering::Relaxed) {
            return;
        }
        if let Err(error) = self.record().await {
            tracing::warn!(%error, "workspace change summary unavailable");
        }
    }

    async fn record(&self) -> Result<(), String> {
        let baseline = self.baseline.lock().expect("workspace baseline").take();
        let mut files = BTreeMap::<String, (ChangedFile, FileSource)>::new();
        let mut captures = self.captures.lock().expect("workspace captures").clone();
        let mut repository = None;
        let mut trees = None;
        if let Some(Baseline {
            repository: repo,
            tree: before,
        }) = baseline
        {
            let after = repo.snapshot().await?;
            for (_, change) in repo.changes(&before, &after).await? {
                let path = repo.root.join(&change.path);
                // Renamed source paths have left the final index, but their
                // Attempt endpoints are still fully owned by this Git diff.
                captures.remove(&path);
                captures.remove(&repo.root.join(&change.old_path));
                let display = display_path(&self.cwd, &path, Some(&repo.root));
                files.insert(
                    display.clone(),
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
            trees = Some((before, after));
            repository = Some(repo);
        }
        if let Some(repo) = &repository {
            let uncovered = repo
                .uncovered(&captures.keys().cloned().collect::<Vec<_>>())
                .await?;
            captures.retain(|path, _| uncovered.contains(path));
        }
        let cwd = self.cwd.clone();
        let directory = Arc::clone(&self.directory);
        let repository_root = repository.as_ref().map(|repo| repo.root.clone());
        let files = tokio::task::spawn_blocking(move || {
            capture_endpoints(
                &mut files,
                &captures,
                &cwd,
                directory.path(),
                repository_root.as_deref(),
            )?;
            Ok::<_, String>(files)
        })
        .await
        .map_err(|error| error.to_string())??;
        if files.is_empty() {
            return Ok(());
        }
        let total = files.len();
        let added = files.values().map(|(file, _)| file.added).sum();
        let deleted = files.values().map(|(file, _)| file.deleted).sum();
        let mut summary_bytes = 0;
        let (files, sources) = files
            .into_values()
            .take(MAX_FILES)
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
            _directory: Arc::clone(&self.directory),
        };
        self.store
            .0
            .lock()
            .expect("workspace changes")
            .insert(self.attempt.as_str().to_owned(), Arc::new(record));
        Ok(())
    }
}

fn capture_endpoints(
    files: &mut BTreeMap<String, (ChangedFile, FileSource)>,
    captures: &BTreeMap<PathBuf, Capture>,
    cwd: &Path,
    directory: &Path,
    repository: Option<&Path>,
) -> Result<(), String> {
    for (path, before) in captures {
        let display = display_path(cwd, path, repository);
        if files.contains_key(&display) {
            continue;
        }
        let Some(after) = Capture::read(path, directory)? else {
            continue;
        };
        if before == &after && *before != Capture::Oversized {
            continue;
        }
        let comparison = FileContents::from_bytes(before.bytes()?, after.bytes()?);
        let (added, deleted) = comparison.counts();
        files.insert(
            display.clone(),
            (
                ChangedFile {
                    path: path.to_string_lossy().into_owned(),
                    display,
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

fn display_path(cwd: &Path, path: &Path, repository: Option<&Path>) -> String {
    if let Ok(relative) = path.strip_prefix(cwd) {
        return relative.to_string_lossy().replace('\\', "/");
    }
    if repository.is_some_and(|root| path.starts_with(root)) {
        let mut parent = cwd.parent();
        let mut prefix = "../".to_owned();
        while let Some(directory) = parent {
            if let Ok(relative) = path.strip_prefix(directory) {
                return format!("{prefix}{}", relative.to_string_lossy().replace('\\', "/"));
            }
            parent = directory.parent();
            prefix.push_str("../");
        }
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        && let Ok(relative) = path.strip_prefix(home)
    {
        return format!("~/{}", relative.to_string_lossy().replace('\\', "/"));
    }
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests;
