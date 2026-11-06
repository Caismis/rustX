//! Enumerate with the real index; sample bytes without Git attributes or filters.
use super::sampling;
use super::{GitContext, Repository, run};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs::{self, File},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::Arc,
};

const PATH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
struct Entry {
    mode: String,
    object: String,
    sparse: bool,
}

impl Repository {
    pub async fn snapshot(&self) -> Result<String, String> {
        let previous = self.entries(false).await?;
        let tracked = self.entries(true).await?;
        let candidates = self.candidates(&previous, &tracked).await?;
        let (mut endpoints, candidates) = self.gitlinks(candidates, tracked).await?;
        let samples = self.samples(candidates, previous).await?;
        endpoints.extend(samples.retained);
        endpoints.extend(self.hash_samples(samples.files).await?);
        let scratch = Arc::clone(&self.scratch);
        tokio::task::spawn_blocking(move || fs::remove_dir_all(scratch.path().join("raw")))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        self.write_tree(endpoints).await
    }

    async fn candidates(
        &self,
        previous: &BTreeMap<String, Entry>,
        tracked: &BTreeMap<String, Entry>,
    ) -> Result<BTreeMap<String, Entry>, String> {
        let mut candidates = previous.clone();
        candidates.extend(tracked.clone());
        let others = self
            .worktree_files(&["ls-files", "--others", "--exclude-standard", "-z"])
            .await?;
        let others = String::from_utf8(others).map_err(|error| error.to_string())?;
        for path in others.split_terminator('\0') {
            candidates
                .entry(path.trim_end_matches('/').to_owned())
                .or_insert_with(|| Entry {
                    mode: "100644".into(),
                    object: String::new(),
                    sparse: false,
                });
        }
        candidates.retain(|path, _| !self.scratch.contains(&self.root.join(path)));
        Ok(candidates)
    }

    async fn worktree_files(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let exclusion = self
            .scratch
            .root()
            .strip_prefix(&self.root)
            .ok()
            .map(|path| format!(":(exclude,literal){}", path.to_string_lossy()));
        let mut args = args.to_vec();
        if let Some(exclusion) = &exclusion {
            args.extend(["--", ".", exclusion]);
        }
        let (status, output) = run(
            &self.root,
            &args,
            GitContext::Worktree(&self.scratch),
            PATH_BYTES,
            None,
            &self.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Git worktree path discovery failed".into());
        }
        Ok(output)
    }

    async fn gitlinks(
        &self,
        mut candidates: BTreeMap<String, Entry>,
        tracked: BTreeMap<String, Entry>,
    ) -> Result<(BTreeMap<String, Entry>, BTreeMap<String, Entry>), String> {
        let root = self.root.clone();
        let cancellation = self.cancellation.clone();
        let (links, candidates) = tokio::task::spawn_blocking(move || {
            let root = File::open(root).map_err(|error| error.to_string())?;
            let links = discover_links(&root, &candidates, &tracked, &cancellation)?;
            candidates
                .retain(|path, _| !links.keys().any(|link| Path::new(path).starts_with(link)));
            Ok::<_, String>((links, candidates))
        })
        .await
        .map_err(|error| error.to_string())??;
        let mut endpoints = BTreeMap::new();
        for (path, retained) in links {
            let directory = self.root.join(&path);
            if let Some(entry) = retained {
                endpoints.insert(path, entry);
            } else {
                let (status, head) = run(
                    &directory,
                    &["rev-parse", "--verify", "HEAD"],
                    GitContext::Worktree(&self.scratch),
                    1024,
                    None,
                    &self.cancellation,
                )
                .await?;
                if status != 0 {
                    return Err("Nested Git endpoint unavailable".into());
                }
                endpoints.insert(
                    path.clone(),
                    Entry {
                        mode: "160000".into(),
                        object: String::from_utf8(head)
                            .map_err(|error| error.to_string())?
                            .trim()
                            .into(),
                        sparse: false,
                    },
                );
            }
        }
        Ok((endpoints, candidates))
    }

    async fn samples(
        &self,
        candidates: BTreeMap<String, Entry>,
        previous: BTreeMap<String, Entry>,
    ) -> Result<Samples, String> {
        let root = self.root.clone();
        let scratch = Arc::clone(&self.scratch);
        let cancellation = self.cancellation.clone();
        tokio::task::spawn_blocking(move || {
            let directory = scratch.path().join("raw");
            fs::create_dir(&directory).map_err(|error| error.to_string())?;
            let root = File::open(root).map_err(|error| error.to_string())?;
            let mut samples = Samples::default();
            let mut budget = sampling::Budget::new(&cancellation);
            for path in candidates.into_keys() {
                let output = directory.join(samples.files.len().to_string());
                match sampling::sample(&root, Path::new(&path), &output, &mut budget)? {
                    sampling::Sample::File(mode) => samples.files.push((path, mode, output)),
                    // The private baseline contains only physically sampled
                    // files. Missing at both endpoints is already unchanged;
                    // a sampled file that disappears is a deletion, regardless
                    // of skip-worktree flags left in the real index.
                    sampling::Sample::Missing => {}
                    sampling::Sample::Unsupported => {
                        if let Some(before) = previous.get(&path) {
                            samples.retained.insert(path, before.clone());
                        }
                    }
                }
            }
            Ok(samples)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    async fn hash_samples(
        &self,
        samples: Vec<(String, String, PathBuf)>,
    ) -> Result<BTreeMap<String, Entry>, String> {
        let mut endpoints = BTreeMap::new();
        if samples.is_empty() {
            return Ok(endpoints);
        }
        let mut input = Vec::new();
        for (_, _, path) in &samples {
            quoted_path(&mut input, path);
        }
        let (status, hashes) = run(
            &self.root,
            &["hash-object", "-w", "--no-filters", "--stdin-paths"],
            GitContext::Snapshot(&self.scratch),
            PATH_BYTES,
            Some(&input),
            &self.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Raw Git snapshot hashing failed".into());
        }
        let hashes = String::from_utf8(hashes).map_err(|error| error.to_string())?;
        let hashes = hashes.lines().collect::<Vec<_>>();
        if hashes.len() != samples.len() {
            return Err("Raw Git snapshot hash count mismatch".into());
        }
        for ((path, mode, _), object) in samples.into_iter().zip(hashes) {
            endpoints.insert(
                path,
                Entry {
                    mode,
                    object: object.into(),
                    sparse: false,
                },
            );
        }
        Ok(endpoints)
    }

    async fn write_tree(&self, endpoints: BTreeMap<String, Entry>) -> Result<String, String> {
        let (status, _) = run(
            &self.root,
            &["read-tree", "--empty"],
            GitContext::Snapshot(&self.scratch),
            1024,
            None,
            &self.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Private Git index reset failed".into());
        }
        let mut input = String::new();
        for (path, entry) in endpoints {
            write!(input, "{} {}\t{path}\0", entry.mode, entry.object)
                .map_err(|error| error.to_string())?;
        }
        if !input.is_empty() {
            let (status, _) = run(
                &self.root,
                &["update-index", "-z", "--index-info"],
                GitContext::Snapshot(&self.scratch),
                1024,
                Some(input.as_bytes()),
                &self.cancellation,
            )
            .await?;
            if status != 0 {
                return Err("Raw Git snapshot index failed".into());
            }
        }
        let (status, tree) = run(
            &self.root,
            &["write-tree"],
            GitContext::Snapshot(&self.scratch),
            1024,
            None,
            &self.cancellation,
        )
        .await?;
        if status != 0 {
            return Err("Raw Git snapshot tree failed".into());
        }
        Ok(String::from_utf8(tree)
            .map_err(|error| error.to_string())?
            .trim()
            .into())
    }

    async fn entries(&self, worktree: bool) -> Result<BTreeMap<String, Entry>, String> {
        let args = ["ls-files", "--stage", "-v", "-z"];
        let output = if worktree {
            self.worktree_files(&args).await?
        } else {
            let (status, output) = run(
                &self.root,
                &args,
                GitContext::Snapshot(&self.scratch),
                PATH_BYTES,
                None,
                &self.cancellation,
            )
            .await?;
            if status != 0 {
                return Err("Git index membership discovery failed".into());
            }
            output
        };
        let output = String::from_utf8(output).map_err(|error| error.to_string())?;
        let mut entries = BTreeMap::new();
        for entry in output.split_terminator('\0') {
            let (header, path) = entry.split_once('\t').ok_or("invalid Git index entry")?;
            let mut fields = header.split_whitespace();
            let flag = fields.next().ok_or("missing Git index flag")?;
            let mode = fields.next().ok_or("missing Git index mode")?;
            let object = fields.next().ok_or("missing Git index object")?;
            entries.insert(
                path.into(),
                Entry {
                    mode: mode.into(),
                    object: object.into(),
                    sparse: flag.eq_ignore_ascii_case("S"),
                },
            );
        }
        Ok(entries)
    }
}

/// Directory discovery has the same blocking owner and deadline as byte
/// sampling, so a large path set cannot occupy an async executor through cancel.
fn discover_links(
    root: &File,
    candidates: &BTreeMap<String, Entry>,
    tracked: &BTreeMap<String, Entry>,
    cancellation: &crate::runtime::cancellation::CancellationSignal,
) -> Result<BTreeMap<String, Option<Entry>>, String> {
    let mut links = BTreeMap::new();
    let mut inspected = BTreeSet::new();
    let budget = sampling::Budget::new(cancellation);
    for (path, entry) in candidates {
        budget.check(0)?;
        for ancestor in Path::new(path)
            .ancestors()
            .filter(|p| !p.as_os_str().is_empty())
        {
            budget.check(0)?;
            if !inspected.insert(ancestor.to_owned()) {
                continue;
            }
            if sampling::directory(root, ancestor)?.is_some_and(|directory| {
                nix::sys::stat::fstatat(
                    &directory,
                    ".git",
                    nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
                )
                .is_ok()
            }) {
                links.insert(ancestor.to_string_lossy().into_owned(), None);
            }
        }
        // Only the current index owns deinitialized submodules. A removed
        // old gitlink must not hide a deletion or a regular-file replacement.
        if entry.mode == "160000"
            && !links.contains_key(path)
            && tracked
                .get(path)
                .is_some_and(|entry| entry.mode == "160000")
            && (entry.sparse || sampling::directory(root, Path::new(path))?.is_some())
        {
            links.insert(path.clone(), Some(entry.clone()));
        }
    }
    let mut outermost = BTreeMap::new();
    for (path, entry) in links {
        budget.check(0)?;
        if !Path::new(&path)
            .ancestors()
            .skip(1)
            .any(|parent| outermost.contains_key(&parent.to_string_lossy().into_owned()))
        {
            outermost.insert(path, entry);
        }
    }
    Ok(outermost)
}

#[derive(Default)]
struct Samples {
    files: Vec<(String, String, PathBuf)>,
    retained: BTreeMap<String, Entry>,
}

/// Git's --stdin-paths accepts C-quoted byte paths, not JSON unicode escapes.
fn quoted_path(output: &mut Vec<u8>, path: &Path) {
    output.push(b'"');
    for byte in path.as_os_str().as_bytes() {
        match byte {
            b'"' | b'\\' => {
                output.push(b'\\');
                output.push(*byte);
            }
            32..=126 => output.push(*byte),
            _ => output.extend_from_slice(format!("\\{byte:03o}").as_bytes()),
        }
    }
    output.extend_from_slice(b"\"\n");
}
