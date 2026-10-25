//! Git snapshots use a private index and object directory. The repository is read-only.
use super::capture::MAX_FILE_BYTES;
use crate::runtime::{cancellation::CancellationSignal, process_runner::SupervisedCommandSpec};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub(super) struct Repository {
    pub root: PathBuf,
    scratch: Arc<tempfile::TempDir>,
    cancellation: CancellationSignal,
    objects: PathBuf,
    index: PathBuf,
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
    cwd: &Path,
    args: &[&str],
    repository: Option<&Repository>,
    limit: usize,
    input: Option<&[u8]>,
    cancellation: &CancellationSignal,
) -> Result<(i32, Vec<u8>), String> {
    if cancellation.is_cancelled() {
        return Err("Git snapshot was cancelled".into());
    }
    let invocation = [
        "git",
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
    // Keep NUL-separated paths out of the shell's argv size limit. The private
    // input file lives until the supervised command has physically settled.
    let input_file = input
        .map(|bytes| {
            let mut file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
            file.write_all(bytes).map_err(|error| error.to_string())?;
            Ok::<_, String>(file)
        })
        .transpose()?;
    let command = match &input_file {
        Some(file) => format!(
            "exec {invocation} < {}",
            quote(&file.path().to_string_lossy())
        ),
        None => format!("exec {invocation}"),
    };
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
    ]);
    if let Some(repo) = repository {
        environment.extend([
            (
                "GIT_INDEX_FILE".into(),
                repo.scratch
                    .path()
                    .join("index")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "GIT_OBJECT_DIRECTORY".into(),
                repo.scratch
                    .path()
                    .join("objects")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "GIT_ALTERNATE_OBJECT_DIRECTORIES".into(),
                repo.objects.to_string_lossy().into_owned(),
            ),
        ]);
    }
    let (code, stdout, stderr) = super::command::run(
        SupervisedCommandSpec {
            command,
            cwd: cwd.to_owned(),
            command_environment: environment,
            timeout: Some(Duration::from_secs(30)),
            cancellation: cancellation.child(),
        },
        limit,
        (input_file, repository.map(|repo| Arc::clone(&repo.scratch))),
    )
    .await?;
    if code == 127 && repository.is_none() {
        return Ok((128, Vec::new()));
    }
    if ![0, 1, 128].contains(&code) {
        return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
    }
    Ok((code, stdout))
}

impl Repository {
    pub async fn locate(
        cwd: &Path,
        scratch: Arc<tempfile::TempDir>,
        cancellation: CancellationSignal,
    ) -> Result<Option<Self>, String> {
        let (status, output) = run(
            cwd,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-path",
                "index",
                "--git-path",
                "objects",
            ],
            None,
            64 * 1024,
            None,
            &cancellation,
        )
        .await?;
        if status == 128 {
            return Ok(None);
        }
        if status != 0 {
            return Err("Git repository discovery failed".into());
        }
        let output = String::from_utf8(output).map_err(|error| error.to_string())?;
        let mut lines = output.lines();
        let root = PathBuf::from(lines.next().ok_or("missing Git root")?);
        let index = PathBuf::from(lines.next().ok_or("missing Git index")?);
        let objects = PathBuf::from(lines.next().ok_or("missing Git objects")?);
        std::fs::create_dir_all(scratch.path().join("objects"))
            .map_err(|error| error.to_string())?;
        Ok(Some(Self {
            root,
            scratch,
            cancellation,
            objects,
            index,
        }))
    }

    pub async fn snapshot(&self) -> Result<String, String> {
        // Retain baseline membership: ignoring or untracking an existing file
        // during the Attempt is not a physical deletion of its bytes.
        if !self.scratch.path().join("index").exists() {
            match std::fs::copy(&self.index, self.scratch.path().join("index")) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        self.refresh_index_flags().await?;
        let mut args = vec!["add", "--sparse", "--all", "--ignore-errors", "--", "."];
        let exclusion;
        if let Ok(relative) = self.scratch.path().strip_prefix(&self.root) {
            exclusion = format!(":(exclude,literal){}", relative.to_string_lossy());
            args.push(&exclusion);
        }
        let (status, _) = run(
            &self.root,
            &args,
            Some(self),
            8 * 1024 * 1024,
            None,
            &self.cancellation,
        )
        .await?;
        if status != 0 && status != 1 {
            return Err("Git snapshot add failed".into());
        }
        // A newly force-tracked ignored file has no entry in the retained
        // private index. Include its actual current bytes without replacing
        // earlier entries or touching the user's index.
        let (status, paths) = run(
            &self.root,
            &[
                "ls-files",
                "--cached",
                "--ignored",
                "--exclude-standard",
                "-z",
            ],
            None,
            8 * 1024 * 1024,
            None,
            &self.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Git tracked ignored file discovery failed".into());
        }
        let paths = String::from_utf8(paths).map_err(|error| error.to_string())?;
        let paths = paths
            .split_terminator('\0')
            .filter(|path| {
                let path = self.root.join(path);
                !path.starts_with(self.scratch.path()) && std::fs::symlink_metadata(path).is_ok()
            })
            .fold(String::new(), |mut paths, path| {
                paths.push_str(path);
                paths.push('\0');
                paths
            });
        if !paths.is_empty() {
            let (status, _) = run(
                &self.root,
                &[
                    "--literal-pathspecs",
                    "add",
                    "--sparse",
                    "--force",
                    "--all",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
                Some(self),
                8 * 1024 * 1024,
                Some(paths.as_bytes()),
                &self.cancellation,
            )
            .await?;
            if status != 0 {
                return Err("Git tracked ignored file snapshot failed".into());
            }
        }
        let (status, tree) = run(
            &self.root,
            &["write-tree"],
            Some(self),
            1024,
            None,
            &self.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Git snapshot write failed".into());
        }
        Ok(String::from_utf8(tree)
            .map_err(|error| error.to_string())?
            .trim()
            .to_owned())
    }

    async fn refresh_index_flags(&self) -> Result<(), String> {
        let (status, entries) = run(
            &self.root,
            &["ls-files", "-v", "-z"],
            Some(self),
            8 * 1024 * 1024,
            None,
            &self.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Git snapshot index inspection failed".into());
        }
        let entries = String::from_utf8(entries).map_err(|error| error.to_string())?;
        let paths = entries
            .split_terminator('\0')
            .filter_map(|entry| {
                let (flag, path) = entry.split_once(' ')?;
                let skipped = flag.eq_ignore_ascii_case("S");
                // Absent sparse-checkout entries remain outside the physical
                // workspace. Existing skipped/assumed entries must be sampled.
                if skipped && std::fs::symlink_metadata(self.root.join(path)).is_err() {
                    return None;
                }
                (skipped || flag.bytes().any(|byte| byte.is_ascii_lowercase()))
                    .then(|| format!("{path}\0"))
            })
            .collect::<String>();
        if !paths.is_empty() {
            // Git applies only one of these bit updates per invocation.
            for flag in ["--no-assume-unchanged", "--no-skip-worktree"] {
                let (status, _) = run(
                    &self.root,
                    &["update-index", flag, "-z", "--stdin"],
                    Some(self),
                    8 * 1024 * 1024,
                    Some(paths.as_bytes()),
                    &self.cancellation,
                )
                .await?;
                if status != 0 {
                    return Err("Git snapshot index refresh failed".into());
                }
            }
        }
        Ok(())
    }

    pub async fn changes(
        &self,
        before: &str,
        after: &str,
        detect_renames: bool,
    ) -> Result<BTreeMap<String, Change>, String> {
        let (status, output) = run(
            &self.root,
            &[
                "diff-tree",
                "--no-commit-id",
                "-r",
                if detect_renames { "-M" } else { "--no-renames" },
                "--numstat",
                "-z",
                before,
                after,
            ],
            Some(self),
            8 * 1024 * 1024,
            None,
            &self.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Git change summary failed".into());
        }
        parse_numstat(&output)
    }

    pub async fn bytes(&self, tree: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        let (status, metadata) = run(
            &self.root,
            &[
                "--literal-pathspecs",
                "ls-tree",
                "-z",
                "-l",
                tree,
                "--",
                path,
            ],
            Some(self),
            64 * 1024,
            None,
            &CancellationSignal::new(),
        )
        .await?;
        if status != 0 {
            return Err("Git file metadata failed".into());
        }
        if metadata.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let header = metadata
            .split(|byte| *byte == b'\t')
            .next()
            .ok_or("invalid Git tree entry")?;
        let size = std::str::from_utf8(header)
            .map_err(|error| error.to_string())?
            .split_whitespace()
            .nth(3)
            .and_then(|size| size.parse::<usize>().ok());
        let Some(size) = size else {
            return Ok(None);
        };
        if size > MAX_FILE_BYTES {
            return Ok(None);
        }
        let object = format!("{tree}:{path}");
        let (status, bytes) = run(
            &self.root,
            &["cat-file", "blob", &object],
            Some(self),
            MAX_FILE_BYTES,
            None,
            &CancellationSignal::new(),
        )
        .await?;
        if status != 0 {
            return Err("Git file comparison unavailable".into());
        }
        Ok(Some(bytes))
    }

    /// Only the initial tree decides whether Git owns a captured before-image.
    /// Final ignore/index membership cannot turn an existing file into a creation.
    /// Nested repositories in either endpoint remain outside this recorder's authority.
    pub async fn uncovered(
        &self,
        before: &str,
        after: &str,
        paths: &[PathBuf],
    ) -> Result<BTreeSet<PathBuf>, String> {
        if paths.is_empty() {
            return Ok(BTreeSet::new());
        }
        let mut covered = BTreeSet::new();
        let mut links = Vec::new();
        for tree in [before, after] {
            let (status, entries) = run(
                &self.root,
                &["ls-tree", "-r", "-z", tree],
                Some(self),
                8 * 1024 * 1024,
                None,
                &self.cancellation,
            )
            .await?;
            if status != 0 {
                return Err("Git snapshot coverage discovery failed".into());
            }
            let entries = String::from_utf8(entries).map_err(|error| error.to_string())?;
            for entry in entries.split_terminator('\0') {
                let (header, path) = entry.split_once('\t').ok_or("invalid Git snapshot entry")?;
                let path = self.root.join(path);
                if header.starts_with("160000 ") {
                    links.push(path.clone());
                }
                if tree == before {
                    covered.insert(path);
                }
            }
        }
        Ok(paths
            .iter()
            .filter(|path| {
                !covered.contains(*path) && !links.iter().any(|link| path.starts_with(link))
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
