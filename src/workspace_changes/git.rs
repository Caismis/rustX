//! Git snapshots use a private index and object directory. The repository is read-only.
use super::capture::MAX_FILE_BYTES;
use crate::runtime::{cancellation::CancellationSignal, process_runner::SupervisedCommandSpec};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) struct Repository {
    pub root: PathBuf,
    scratch: PathBuf,
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
) -> Result<(i32, Vec<u8>), String> {
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
    // printf is a shell builtin; both it and Git remain inside the existing
    // supervised group. Its format is constant and every path is quoted data.
    let command = match input {
        Some(input) => {
            let paths = std::str::from_utf8(input)
                .map_err(|error| error.to_string())?
                .split_terminator('\0')
                .map(quote)
                .collect::<Vec<_>>()
                .join(" ");
            format!("printf '%s\\0' {paths} | exec {invocation}")
        }
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
                repo.scratch.join("index").to_string_lossy().into_owned(),
            ),
            (
                "GIT_OBJECT_DIRECTORY".into(),
                repo.scratch.join("objects").to_string_lossy().into_owned(),
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
            cancellation: CancellationSignal::new(),
        },
        limit,
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
    pub async fn locate(cwd: &Path, scratch: &Path) -> Result<Option<Self>, String> {
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
        std::fs::create_dir_all(scratch.join("objects")).map_err(|error| error.to_string())?;
        Ok(Some(Self {
            root,
            scratch: scratch.to_owned(),
            objects,
            index,
        }))
    }

    pub async fn snapshot(&self) -> Result<String, String> {
        // Each endpoint follows the current tracking state. In particular, an
        // ignored file force-added during this turn must enter the end tree.
        match std::fs::copy(&self.index, self.scratch.join("index")) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::remove_file(self.scratch.join("index")) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        let mut args = vec!["add", "--all", "--ignore-errors", "--", "."];
        let exclusion;
        if let Ok(relative) = self.scratch.strip_prefix(&self.root) {
            exclusion = format!(":(exclude,literal){}", relative.to_string_lossy());
            args.push(&exclusion);
        }
        let (status, _) = run(&self.root, &args, Some(self), 8 * 1024 * 1024, None).await?;
        if status != 0 && status != 1 {
            return Err("Git snapshot add failed".into());
        }
        let (status, tree) = run(&self.root, &["write-tree"], Some(self), 1024, None).await?;
        if status != 0 {
            return Err("Git snapshot write failed".into());
        }
        Ok(String::from_utf8(tree)
            .map_err(|error| error.to_string())?
            .trim()
            .to_owned())
    }

    pub async fn changes(
        &self,
        before: &str,
        after: &str,
    ) -> Result<BTreeMap<String, Change>, String> {
        let (status, output) = run(
            &self.root,
            &[
                "diff-tree",
                "--no-commit-id",
                "-r",
                "-M",
                "--numstat",
                "-z",
                before,
                after,
            ],
            Some(self),
            8 * 1024 * 1024,
            None,
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
        )
        .await?;
        if status != 0 {
            return Err("Git file comparison unavailable".into());
        }
        Ok(Some(bytes))
    }

    /// Git already owns all ordinary work-tree paths, including paths with no
    /// net diff. Only ignored files and paths outside this repository need
    /// first-mutation captures. Nested repository contents stay with their owner.
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
        let (status, entries) = run(
            &self.root,
            &["ls-files", "-z", "--stage"],
            None,
            8 * 1024 * 1024,
            None,
        )
        .await?;
        if status != 0 {
            return Err("Git nested repository discovery failed".into());
        }
        let entries = String::from_utf8(entries).map_err(|error| error.to_string())?;
        let links = entries
            .split_terminator('\0')
            .filter(|entry| entry.starts_with("160000 "))
            .filter_map(|entry| entry.split_once('\t').map(|(_, path)| self.root.join(path)))
            .collect::<Vec<_>>();
        let mut paths = Vec::new();
        for path in inside {
            if links.iter().any(|link| path.starts_with(link)) {
                continue;
            }
            let relative = path
                .strip_prefix(&self.root)
                .map_err(|error| error.to_string())?;
            paths.push(relative.to_string_lossy().into_owned());
        }
        uncovered.extend(self.ignored(&paths).await?);
        Ok(uncovered)
    }

    async fn ignored(&self, mut paths: &[String]) -> Result<BTreeSet<PathBuf>, String> {
        let mut ignored_paths = BTreeSet::new();
        while !paths.is_empty() {
            // Bound the quoted shell argv even for unusually long paths.
            let mut bytes = 0;
            let end = paths
                .iter()
                .take_while(|path| {
                    bytes += path.len() + 1;
                    bytes <= 16 * 1024
                })
                .count()
                .max(1);
            let input = paths[..end].join("\0") + "\0";
            let (status, ignored) = run(
                &self.root,
                &["check-ignore", "-z", "--stdin"],
                None,
                8 * 1024 * 1024,
                Some(input.as_bytes()),
            )
            .await?;
            if status != 0 && status != 1 {
                return Err("Git ignored file discovery failed".into());
            }
            let ignored = String::from_utf8(ignored).map_err(|error| error.to_string())?;
            ignored_paths.extend(
                ignored
                    .split_terminator('\0')
                    .map(|path| self.root.join(path)),
            );
            paths = &paths[end..];
        }
        Ok(ignored_paths)
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
