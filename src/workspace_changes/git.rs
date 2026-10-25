//! Git only reads worktree membership. Raw snapshots live in a separate bare repository.
mod sampling;
mod snapshot;
use super::capture::MAX_FILE_BYTES;
use super::storage::SnapshotDirectory;
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
    scratch: Arc<SnapshotDirectory>,
    cancellation: CancellationSignal,
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
    context: GitContext<'_>,
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
    // Keep NUL-separated paths out of the shell's argv size limit. The private
    // input file lives until the supervised command has physically settled.
    let scratch = match context {
        GitContext::Worktree(scratch) | GitContext::Snapshot(scratch) => scratch,
    };
    let input_file = input
        .map(|bytes| {
            let mut file = tempfile::NamedTempFile::new_in(scratch.path())
                .map_err(|error| error.to_string())?;
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
        ("GIT_NO_LAZY_FETCH".into(), "1".into()),
    ]);
    let private = matches!(context, GitContext::Snapshot(_));
    if private {
        environment.extend([
            (
                "GIT_DIR".into(),
                scratch.path().to_string_lossy().into_owned(),
            ),
            (
                "GIT_INDEX_FILE".into(),
                scratch.path().join("index").to_string_lossy().into_owned(),
            ),
            ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_ATTR_NOSYSTEM".into(), "1".into()),
        ]);
    }
    let (code, stdout, stderr) = super::command::run(
        SupervisedCommandSpec {
            command,
            cwd: if private { scratch.path() } else { cwd }.to_owned(),
            command_environment: environment,
            timeout: Some(Duration::from_secs(30)),
            cancellation: cancellation.child(),
        },
        limit,
        (input_file, Arc::clone(scratch)),
    )
    .await?;
    if code == 127 && !private {
        return Ok((128, Vec::new()));
    }
    if ![0, 1, 128].contains(&code) {
        return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
    }
    Ok((code, stdout))
}

#[derive(Clone, Copy)]
enum GitContext<'a> {
    Worktree(&'a Arc<SnapshotDirectory>),
    Snapshot(&'a Arc<SnapshotDirectory>),
}

impl Repository {
    pub async fn locate(
        cwd: &Path,
        scratch: Arc<SnapshotDirectory>,
        cancellation: CancellationSignal,
    ) -> Result<Option<Self>, String> {
        let (status, output) = run(
            cwd,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--show-object-format",
            ],
            GitContext::Worktree(&scratch),
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
        let format = lines.next().ok_or("missing Git object format")?;
        if !matches!(format, "sha1" | "sha256") {
            return Err("unsupported Git object format".into());
        }
        let repo = Self {
            root,
            scratch,
            cancellation,
        };
        let (status, _) = run(
            &repo.root,
            &[
                "init",
                "--quiet",
                "--bare",
                "--template=",
                &format!("--object-format={format}"),
            ],
            GitContext::Snapshot(&repo.scratch),
            64 * 1024,
            None,
            &repo.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Private Git initialization failed".into());
        }
        Ok(Some(repo))
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
                "--no-ext-diff",
                "--no-textconv",
                "-r",
                if detect_renames { "-M" } else { "--no-renames" },
                "--numstat",
                "-z",
                before,
                after,
            ],
            GitContext::Snapshot(&self.scratch),
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
            GitContext::Snapshot(&self.scratch),
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
            GitContext::Snapshot(&self.scratch),
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
                GitContext::Snapshot(&self.scratch),
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
