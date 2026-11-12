//! Session-private Git index and objects; worktree bytes stay in supervised Git.
use super::{capture::MAX_FILE_BYTES, storage::SnapshotDirectory};
use crate::runtime::{cancellation::CancellationSignal, process_runner::SupervisedCommandSpec};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

#[derive(Clone)]
pub(super) struct Repository {
    pub root: PathBuf,
    scratch: Arc<SnapshotDirectory>,
    objects: PathBuf,
    exclusion: PathBuf,
    gate: Arc<tokio::sync::Mutex<()>>,
    pub cancellation: CancellationSignal,
    #[cfg(test)]
    pub commands: Arc<std::sync::atomic::AtomicUsize>,
}

// A killed add can leave its private index lock. Cleanup belongs to the physical
// driver, after reaping and before the next snapshot acquires the same gate.
struct SnapshotLease {
    _guard: tokio::sync::OwnedMutexGuard<()>,
    scratch: Arc<SnapshotDirectory>,
}
impl Drop for SnapshotLease {
    fn drop(&mut self) {
        let lock = self.scratch.path().join("git/index.lock");
        if lock.exists() {
            let _ = std::fs::remove_file(lock);
        }
    }
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

fn invocation(args: &[&str]) -> String {
    [
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
    .join(" ")
}

fn environment() -> Vec<(String, String)> {
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
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_ATTR_NOSYSTEM".into(), "1".into()),
    ]);
    environment
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
        if !may_have_repository(cwd) {
            return Ok(None);
        }
        // Read-only discovery/config parsing never executes repository programs.
        let command = format!(
            "{} && printf '\\0' && exec {}",
            invocation(&[
                "rev-parse",
                "--show-toplevel",
                "--absolute-git-dir",
                "--git-path",
                "objects",
                "--show-object-format",
                "--git-path",
                "info/exclude",
                "--git-path",
                "info/attributes",
                "--git-path",
                "info/sparse-checkout"
            ]),
            invocation(&["config", "--null", "--list"])
        );
        let (code, output, stderr) = super::command::run(
            SupervisedCommandSpec {
                command,
                cwd: cwd.to_owned(),
                command_environment: environment(),
                timeout: Some(Duration::from_secs(30)),
                cancellation: cancellation.child(),
            },
            8 * 1024 * 1024,
            Arc::clone(&scratch),
        )
        .await?;
        if (code == 127 && String::from_utf8_lossy(&stderr).contains("git: command not found"))
            || (code == 128
                && (String::from_utf8_lossy(&stderr).contains("not a git repository")
                    || String::from_utf8_lossy(&stderr).contains("must be run in a work tree")))
        {
            return Ok(None);
        }
        if code != 0 {
            return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
        }
        let (location, config) = split_once(&output, 0).ok_or("invalid Git discovery output")?;
        let location = std::str::from_utf8(location).map_err(|error| error.to_string())?;
        let mut lines = location.lines();
        let root = PathBuf::from(lines.next().ok_or("missing Git root")?);
        let git_dir = PathBuf::from(lines.next().ok_or("missing Git directory")?);
        let objects = cwd
            .join(lines.next().ok_or("missing Git objects")?)
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let format = lines.next().ok_or("missing Git object format")?;
        if !["sha1", "sha256"].contains(&format) {
            return Err("unsupported Git object format".into());
        }
        if scratch.get().is_none() {
            let directory = super::create_snapshot_directory(&std::env::temp_dir())
                .map_err(|error| error.to_string())?;
            let _ = scratch.set(Arc::new(directory));
        }
        let scratch = Arc::clone(scratch.get().ok_or("missing Git scratch directory")?);
        let info = ["info/exclude", "info/attributes", "info/sparse-checkout"]
            .map(|_| {
                lines
                    .next()
                    .map(|path| cwd.join(path))
                    .ok_or("missing Git info path")
            })
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        seed(&git_dir, &scratch.path().join("git"), config, format, &info)?;
        Ok(Some(Self {
            root,
            objects,
            exclusion: scratch.root().to_owned(),
            scratch,
            gate: Arc::default(),
            cancellation,
            #[cfg(test)]
            commands: Arc::default(),
        }))
    }

    async fn run(
        &self,
        command: String,
        limit: usize,
        cancellation: &CancellationSignal,
        keepalive: impl Send + 'static,
    ) -> Result<Vec<u8>, String> {
        if cancellation.is_cancelled() {
            return Err("Git snapshot was cancelled".into());
        }
        let mut environment = environment();
        environment.extend([
            (
                "GIT_DIR".into(),
                self.scratch
                    .path()
                    .join("git")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "GIT_WORK_TREE".into(),
                self.root.to_string_lossy().into_owned(),
            ),
            (
                "GIT_INDEX_FILE".into(),
                self.scratch
                    .path()
                    .join("git/index")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "GIT_OBJECT_DIRECTORY".into(),
                self.scratch
                    .path()
                    .join("git/objects")
                    .to_string_lossy()
                    .into_owned(),
            ),
            // Git accepts a C-quoted alternate, including colons/newlines in its path.
            (
                "GIT_ALTERNATE_OBJECT_DIRECTORIES".into(),
                serde_json::to_string(&self.objects).map_err(|error| error.to_string())?,
            ),
        ]);
        #[cfg(test)]
        self.commands
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (code, output, stderr) = super::command::run(
            SupervisedCommandSpec {
                command,
                cwd: self.root.clone(),
                command_environment: environment,
                timeout: Some(Duration::from_secs(30)),
                cancellation: cancellation.child(),
            },
            limit,
            (Arc::clone(&self.scratch), keepalive),
        )
        .await?;
        if code != 0 {
            return Err(format!(
                "Git observation failed ({code}): {}",
                String::from_utf8_lossy(&stderr).trim()
            ));
        }
        Ok(output)
    }

    pub async fn snapshot(&self) -> Result<String, String> {
        // The physical command driver owns the lock until it has reaped Git,
        // even when an attempt abandons its waiter during cancellation.
        let guard = SnapshotLease {
            _guard: Arc::clone(&self.gate).lock_owned().await,
            scratch: Arc::clone(&self.scratch),
        };
        let mut add = invocation(&["add", "--all", "--ignore-errors"]);
        if let Ok(relative) = self.exclusion.strip_prefix(&self.root) {
            // This one command needs exclude pathspec magic, all others are literal.
            add = format!(
                "GIT_LITERAL_PATHSPECS=0 {add} -- . {}",
                quote(&format!(":(exclude){}", relative.to_string_lossy()))
            );
        }
        let command = format!(
            "{add}; result=$?; if [ \"$result\" -gt 1 ]; then exit \"$result\"; fi; exec {}",
            invocation(&["write-tree"])
        );
        let output = self.run(command, 1024, &self.cancellation, guard).await?;
        let tree = std::str::from_utf8(&output)
            .map_err(|error| error.to_string())?
            .trim();
        if ![40, 64].contains(&tree.len()) || !tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("invalid Git snapshot tree".into());
        }
        Ok(tree.to_owned())
    }

    pub async fn changes(
        &self,
        before: &str,
        after: &str,
    ) -> Result<BTreeMap<String, Change>, String> {
        if before == after {
            return Ok(BTreeMap::new());
        }
        let output = self
            .run(
                format!(
                    "exec {}",
                    invocation(&[
                        "diff-tree",
                        "--no-ext-diff",
                        "--no-textconv",
                        "-r",
                        "-M",
                        "--numstat",
                        "-z",
                        before,
                        after
                    ])
                ),
                8 * 1024 * 1024,
                &self.cancellation,
                (),
            )
            .await?;
        parse_numstat(&output)
    }

    pub async fn bytes(&self, tree: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        let cancellation = CancellationSignal::new();
        let output = self
            .run(
                format!(
                    "GIT_LITERAL_PATHSPECS=1 exec {}",
                    invocation(&["ls-tree", "-z", "-l", tree, "--", path])
                ),
                8 * 1024 * 1024,
                &cancellation,
                (),
            )
            .await?;
        let entry = output
            .split(|byte| *byte == b'\t')
            .next()
            .unwrap_or_default();
        let entry = std::str::from_utf8(entry).map_err(|error| error.to_string())?;
        let fields = entry.split_whitespace().collect::<Vec<_>>();
        if fields.get(1) != Some(&"blob") {
            return Ok(Some(Vec::new()));
        }
        let size: usize = fields
            .get(3)
            .ok_or("missing Git blob size")?
            .parse()
            .map_err(|_| "invalid Git blob size")?;
        if size > MAX_FILE_BYTES {
            return Ok(None);
        }
        self.run(
            format!("exec {}", invocation(&["cat-file", "blob", fields[2]])),
            MAX_FILE_BYTES,
            &cancellation,
            (),
        )
        .await
        .map(Some)
    }

    /// DSH coverage: final private-index gitlinks are opaque; only ignored
    /// paths and paths outside the repository use file-tool copies.
    pub async fn uncovered(&self, paths: &[PathBuf]) -> Result<BTreeSet<PathBuf>, String> {
        let mut uncovered = paths
            .iter()
            .filter(|path| !path.starts_with(&self.root))
            .cloned()
            .collect::<BTreeSet<_>>();
        let inside = paths
            .iter()
            .filter(|path| path.starts_with(&self.root))
            .collect::<Vec<_>>();
        if inside.is_empty() {
            return Ok(uncovered);
        }
        let output = self
            .run(
                format!("exec {}", invocation(&["ls-files", "-z", "--stage"])),
                8 * 1024 * 1024,
                &self.cancellation,
                (),
            )
            .await?;
        let links = output
            .split(|byte| *byte == 0)
            .filter(|entry| entry.starts_with(b"160000 "))
            .filter_map(|entry| {
                split_once(entry, b'\t')
                    .map(|(_, path)| self.root.join(String::from_utf8_lossy(path).as_ref()))
            })
            .collect::<Vec<_>>();
        let candidates = inside
            .into_iter()
            .filter(|path| !links.iter().any(|link| path.starts_with(link)))
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Ok(uncovered);
        }
        // Feed arbitrary filenames without shell interpretation or argv limits.
        let input = tempfile::NamedTempFile::new_in(self.scratch.path())
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        for path in candidates {
            bytes.extend_from_slice(
                path.strip_prefix(&self.root)
                    .map_err(|error| error.to_string())?
                    .as_os_str()
                    .as_encoded_bytes(),
            );
            bytes.push(0);
        }
        std::fs::write(input.path(), bytes).map_err(|error| error.to_string())?;
        let command = format!(
            "{} < {}; result=$?; if [ \"$result\" -gt 1 ]; then exit \"$result\"; fi",
            invocation(&["check-ignore", "-z", "--stdin"]),
            quote(&input.path().to_string_lossy())
        );
        let output = self
            .run(command, 8 * 1024 * 1024, &self.cancellation, input)
            .await?;
        for path in output
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            uncovered.insert(self.root.join(String::from_utf8_lossy(path).as_ref()));
        }
        Ok(uncovered)
    }
}

/// Copy only data and allowlisted configuration, never filter/diff commands or
/// includes. This keeps observation from executing user programs (including LFS).
fn seed(
    source: &Path,
    destination: &Path,
    config: &[u8],
    format: &str,
    info: &[PathBuf],
) -> Result<(), String> {
    let setup = || -> std::io::Result<()> {
        std::fs::create_dir(destination)?;
        for directory in ["objects", "refs", "info"] {
            std::fs::create_dir(destination.join(directory))?;
        }
        std::fs::write(destination.join("HEAD"), "ref: refs/heads/snapshot\n")?;
        let mut private = if format == "sha256" {
            "[core]\nrepositoryformatversion=1\n[extensions]\nobjectFormat=sha256\n".to_owned()
        } else {
            "[core]\nrepositoryformatversion=0\n".to_owned()
        };
        // Snapshot objects are temporary: skip compression, retaining Git's
        // object IDs and content while trading temporary space for CPU.
        private.push_str("[core]\nbare=false\ncompression=0\n[index]\nthreads=1\n");
        for entry in config.split(|byte| *byte == 0) {
            let Some((key, value)) = split_once(entry, b'\n') else {
                continue;
            };
            let key = std::str::from_utf8(key).map_err(std::io::Error::other)?;
            if let Some(name) = key.strip_prefix("core.")
                && [
                    "filemode",
                    "symlinks",
                    "ignorecase",
                    "excludesfile",
                    "autocrlf",
                    "eol",
                    "sparsecheckout",
                    "sparsecheckoutcone",
                ]
                .contains(&name)
            {
                let value = std::str::from_utf8(value).map_err(std::io::Error::other)?;
                writeln!(private, "[core]\n{name}={}", serde_json::to_string(value)?)
                    .map_err(std::io::Error::other)?;
            }
        }
        std::fs::write(destination.join("config"), private)?;
        copy_if_present(&source.join("index"), &destination.join("index"))?;
        for (name, path) in ["info/exclude", "info/attributes", "info/sparse-checkout"]
            .iter()
            .zip(info)
        {
            copy_if_present(path, &destination.join(name))?;
        }
        // Split indices refer to sharedindex files beside the index. Copying
        // them first lets core.splitIndex=false expand only the private copy.
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("sharedindex.")
            {
                std::fs::copy(entry.path(), destination.join(entry.file_name()))?;
            }
        }
        Ok(())
    };
    setup().map_err(|error| error.to_string())
}

fn copy_if_present(source: &Path, destination: &Path) -> std::io::Result<()> {
    match std::fs::copy(source, destination) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn split_once(bytes: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let index = bytes.iter().position(|byte| *byte == separator)?;
    Some((&bytes[..index], &bytes[index + 1..]))
}

fn parse_numstat(output: &[u8]) -> Result<BTreeMap<String, Change>, String> {
    let output = std::str::from_utf8(output).map_err(|error| error.to_string())?;
    if !output.is_empty() && !output.ends_with('\0') {
        return Err("numstat output is not NUL-terminated".into());
    }
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
