//! Git snapshots use a private index and object directory. The repository is read-only.
use super::capture::MAX_FILE_BYTES;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

pub(super) struct Repository {
    pub root: PathBuf,
    scratch: PathBuf,
    objects: PathBuf,
}

pub(super) struct Change {
    pub path: String,
    pub old_path: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
}

async fn limited(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err("Git output exceeded its limit".into());
    }
    Ok(bytes)
}

async fn run(
    cwd: &Path,
    args: &[&str],
    repository: Option<&Repository>,
    limit: usize,
) -> Result<(i32, Vec<u8>), String> {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.splitIndex=false",
            "-c",
            "core.untrackedCache=false",
        ])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    if let Some(repo) = repository {
        command
            .env("GIT_INDEX_FILE", repo.scratch.join("index"))
            .env("GIT_OBJECT_DIRECTORY", repo.scratch.join("objects"))
            .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &repo.objects);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if repository.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((128, Vec::new()));
        }
        Err(error) => return Err(error.to_string()),
    };
    let stdout = child.stdout.take().ok_or("missing Git stdout")?;
    let stderr = child.stderr.take().ok_or("missing Git stderr")?;
    let future = async {
        let (stdout, stderr, status) =
            tokio::try_join!(limited(stdout, limit), limited(stderr, 16 * 1024), async {
                child.wait().await.map_err(|error| error.to_string())
            })?;
        let code = status.code().ok_or("Git was terminated")?;
        if code != 0 && code != 1 && code != 128 {
            return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
        }
        Ok((code, stdout))
    };
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .map_err(|_| "Git snapshot timed out".to_owned())?
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
        if index.exists() {
            std::fs::copy(index, scratch.join("index")).map_err(|error| error.to_string())?;
        }
        Ok(Some(Self {
            root,
            scratch: scratch.to_owned(),
            objects,
        }))
    }

    pub async fn snapshot(&self) -> Result<String, String> {
        let mut args = vec!["add", "--all", "--ignore-errors", "--", "."];
        let exclusion;
        if let Ok(relative) = self.scratch.strip_prefix(&self.root) {
            exclusion = format!(":(exclude,literal){}", relative.to_string_lossy());
            args.push(&exclusion);
        }
        let (status, _) = run(&self.root, &args, Some(self), 8 * 1024 * 1024).await?;
        if status != 0 && status != 1 {
            return Err("Git snapshot add failed".into());
        }
        let (status, tree) = run(&self.root, &["write-tree"], Some(self), 1024).await?;
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
        )
        .await?;
        if status != 0 {
            return Err("Git file comparison unavailable".into());
        }
        Ok(Some(bytes))
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
