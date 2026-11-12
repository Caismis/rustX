//! Read worktree membership in process; only changed endpoints need supervised Git diff.
mod sampling;
mod snapshot;
use super::capture::MAX_FILE_BYTES;
use super::storage::SnapshotDirectory;
use crate::runtime::{cancellation::CancellationSignal, process_runner::SupervisedCommandSpec};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

pub(super) struct Repository {
    pub root: PathBuf,
    scratch: Arc<OnceLock<Arc<SnapshotDirectory>>>,
    exclusion: PathBuf,
    state: Arc<Mutex<snapshot::State>>,
    cancellation: CancellationSignal,
    #[cfg(test)]
    pub commands: Arc<std::sync::atomic::AtomicUsize>,
}

pub(super) struct Change {
    pub path: String,
    pub old_path: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn run(
    args: &[&str],
    scratch: &Arc<SnapshotDirectory>,
    limit: usize,
    cancellation: &CancellationSignal,
) -> Result<(i32, Vec<u8>), String> {
    if cancellation.is_cancelled() {
        return Err("Git snapshot was cancelled".into());
    }
    let invocation = [
        "git",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.attributesFile=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.splitIndex=false",
        "-c",
        "core.untrackedCache=false",
    ]
    .into_iter()
    .chain(args.iter().copied())
    .map(quote)
    .collect::<Vec<_>>()
    .join(" ");
    let command = format!("exec {invocation}");
    let mut environment = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(key, _)| {
            !key.starts_with("GIT_")
                && !key.starts_with("BASH_FUNC_")
                && !["BASH_ENV", "ENV", "BASHOPTS", "SHELLOPTS"].contains(&key.as_str())
        })
        .collect::<Vec<_>>();
    environment.extend([
        ("GIT_CONFIG_COUNT".into(), "0".into()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ("GIT_OPTIONAL_LOCKS".into(), "0".into()),
        ("LC_ALL".into(), "C".into()),
        ("GIT_NO_LAZY_FETCH".into(), "1".into()),
    ]);
    environment.extend([
        (
            "GIT_DIR".into(),
            scratch.path().join("git").to_string_lossy().into_owned(),
        ),
        (
            "GIT_INDEX_FILE".into(),
            scratch
                .path()
                .join("git/index")
                .to_string_lossy()
                .into_owned(),
        ),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_ATTR_NOSYSTEM".into(), "1".into()),
    ]);
    let (code, stdout, stderr) = super::command::run(
        SupervisedCommandSpec {
            command,
            cwd: scratch.path().to_owned(),
            command_environment: environment,
            timeout: Some(Duration::from_secs(30)),
            cancellation: cancellation.child(),
        },
        limit,
        Arc::clone(scratch),
    )
    .await?;
    if ![0, 1, 128].contains(&code) {
        return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
    }
    Ok((code, stdout))
}

/// A missing marker on every ancestor proves Git cannot discover a repository.
/// Ambiguous filesystem results and possible bare repositories use discovery.
pub(super) fn may_have_repository(cwd: &Path) -> bool {
    let Ok(cwd) = cwd.canonicalize() else {
        return true;
    };
    for directory in cwd.ancestors() {
        for marker in [".git", "HEAD"] {
            match std::fs::symlink_metadata(directory.join(marker)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) | Err(_) => return true,
            }
        }
    }
    false
}

impl Repository {
    pub async fn locate(
        cwd: &Path,
        scratch: Arc<OnceLock<Arc<SnapshotDirectory>>>,
        cancellation: CancellationSignal,
    ) -> Result<Option<Self>, String> {
        let cwd = cwd.to_owned();
        tokio::task::spawn_blocking(move || {
            let Some(repo) = snapshot::open(&cwd)? else {
                return Ok(None);
            };
            let Some(root) = repo.workdir() else {
                return Ok(None);
            };
            let root = root.canonicalize().map_err(|error| error.to_string())?;
            let exclusion = match scratch.get() {
                Some(directory) => directory.root().to_owned(),
                None => SnapshotDirectory::namespace(&std::env::temp_dir())
                    .map_err(|error| error.to_string())?,
            };
            Ok(Some(Self {
                root,
                scratch,
                exclusion,
                state: Arc::new(Mutex::new(snapshot::State::new(repo))),
                cancellation,
                #[cfg(test)]
                commands: Arc::default(),
            }))
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// Materialize only private raw objects. Repository configuration, filters,
    /// hooks and the user's object database never enter the diff subprocess.
    pub(super) async fn materialize(&self) -> Result<Arc<SnapshotDirectory>, String> {
        let state = Arc::clone(&self.state);
        let scratch = Arc::clone(&self.scratch);
        let cancellation = self.cancellation.clone();
        tokio::task::spawn_blocking(move || {
            state
                .lock()
                .expect("workspace Git state")
                .materialize(&scratch, &cancellation)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    pub async fn changes(
        &self,
        before: &str,
        after: &str,
        detect_renames: bool,
    ) -> Result<BTreeMap<String, Change>, String> {
        if before == after {
            return Ok(BTreeMap::new());
        }
        let scratch = self.materialize().await?;
        #[cfg(test)]
        self.commands
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (status, output) = run(
            &[
                "diff-tree",
                "--no-commit-id",
                "--no-ext-diff",
                "--no-textconv",
                "-r",
                if detect_renames { "-M" } else { "--no-renames" },
                "--numstat",
                "-z",
                before,
                after,
            ],
            &scratch,
            8 * 1024 * 1024,
            &self.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Git change summary failed".into());
        }
        parse_numstat(&output)
    }

    pub fn finish_sampling(&self) {
        self.state
            .lock()
            .expect("workspace Git state")
            .finish_sampling();
    }

    pub async fn bytes(&self, tree: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        let state = Arc::clone(&self.state);
        let tree = tree.to_owned();
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            state
                .lock()
                .expect("workspace Git state")
                .bytes(&tree, &path, MAX_FILE_BYTES)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// Only the initial tree owns before-images; gitlinks in either endpoint
    /// exclude nested owners regardless of later index or ignore changes.
    pub fn uncovered(
        &self,
        before: &str,
        after: &str,
        paths: &[PathBuf],
    ) -> Result<BTreeSet<PathBuf>, String> {
        let state = self.state.lock().expect("workspace Git state");
        let before = state.entries(before)?;
        let after = state.entries(after)?;
        Ok(paths
            .iter()
            .filter(|path| {
                let Ok(relative) = path.strip_prefix(&self.root) else {
                    return true;
                };
                let relative = relative.to_string_lossy();
                !before.contains_key(relative.as_ref())
                    && !before.iter().chain(after).any(|(link, entry)| {
                        entry.mode == "160000" && path.starts_with(self.root.join(link))
                    })
            })
            .cloned()
            .collect())
    }
}

fn parse_numstat(output: &[u8]) -> Result<BTreeMap<String, Change>, String> {
    let output = std::str::from_utf8(output).map_err(|error| error.to_string())?;
    let mut records = output.split_terminator('\0');
    let mut changes = BTreeMap::new();
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, '\t');
        let added = fields.next().ok_or("missing addition count")?;
        let deleted = fields.next().ok_or("missing deletion count")?;
        let initial = fields.next().ok_or("missing file path")?;
        let (old_path, path) = if initial.is_empty() {
            (
                records.next().ok_or("missing rename source")?,
                records.next().ok_or("missing rename target")?,
            )
        } else {
            (initial, initial)
        };
        let binary = added == "-" || deleted == "-";
        changes.insert(
            path.to_owned(),
            Change {
                path: path.into(),
                old_path: old_path.into(),
                binary,
                added: if binary {
                    0
                } else {
                    added.parse().map_err(|_| "invalid addition count")?
                },
                deleted: if binary {
                    0
                } else {
                    deleted.parse().map_err(|_| "invalid deletion count")?
                },
            },
        );
    }
    Ok(changes)
}
