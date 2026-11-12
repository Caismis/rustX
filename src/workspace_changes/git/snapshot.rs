//! In-process index enumeration and bounded raw sampling at attempt endpoints.
use super::{Repository, sampling};
use crate::{
    runtime::cancellation::CancellationSignal, workspace_changes::storage::SnapshotDirectory,
};
use gix::{
    bstr::{BStr, ByteSlice},
    objs::Write as _,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, Read},
    path::Path,
    sync::{Arc, OnceLock, atomic::AtomicBool},
};

const PATH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Entry {
    pub mode: String,
    object: gix::ObjectId,
    sparse: bool,
}

pub(super) struct State {
    repo: gix::Repository,
    previous: BTreeMap<String, Entry>,
    snapshots: BTreeMap<String, BTreeMap<String, Entry>>,
}

/// No Git environment overrides or executable discovery. These operations only
/// parse configuration, refs, the index and ignore files; never use a filter,
/// fsmonitor, hook, credential helper, external diff or lazy object fetch.
pub(super) fn open(cwd: &Path) -> Result<Option<gix::Repository>, String> {
    let Ok((path, _)) = gix::discover::upwards(cwd) else {
        return Ok(None);
    };
    let mut permissions = gix::open::Permissions::all();
    permissions.config.git_binary = false;
    permissions.config.env = false;
    permissions.env.git_prefix = gix::sec::Permission::Deny;
    permissions.env.objects = gix::sec::Permission::Deny;
    permissions.attributes = gix::open::permissions::Attributes::isolated();
    let options = gix::open::Options::default()
        .permissions(permissions)
        .bail_if_untrusted(true)
        .strict_config(true)
        .config_overrides([
            "core.fsmonitor=false",
            "core.hooksPath=/dev/null",
            "core.attributesFile=/dev/null",
            "index.threads=1",
        ]);
    let (git_dir, _) = path.into_repository_and_work_tree_directories();
    if std::fs::metadata(git_dir.join("config")).is_ok_and(|metadata| !metadata.is_file()) {
        return Err("Git configuration is not a regular file".into());
    }
    gix::discover_opts(cwd, gix::discover::upwards::Options::default(), options)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Poll cancellation between bounded compression reads, keeping the memory and
/// private directory owned until the blocking writer has physically returned.
struct ObjectInput<'a> {
    bytes: &'a [u8],
    cancellation: &'a CancellationSignal,
}
impl Read for ObjectInput<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancellation.is_cancelled() {
            // Interrupted is retryable to io::copy; cancellation is permanent.
            return Err(io::Error::other("Git snapshot was cancelled"));
        }
        let limit = buffer.len().min(64 * 1024);
        self.bytes.read(&mut buffer[..limit])
    }
}

impl State {
    pub fn new(repo: gix::Repository) -> Self {
        Self {
            repo: repo.with_object_memory(),
            previous: BTreeMap::new(),
            snapshots: BTreeMap::new(),
        }
    }

    pub fn entries(&self, tree: &str) -> Result<&BTreeMap<String, Entry>, String> {
        self.snapshots
            .get(tree)
            .ok_or_else(|| "Git snapshot unavailable".into())
    }

    pub fn bytes(&self, tree: &str, path: &str, limit: usize) -> Result<Option<Vec<u8>>, String> {
        let id = gix::ObjectId::from_hex(tree.as_bytes())
            .map_err(|_| "Git file metadata failed".to_owned())?;
        let tree = self
            .repo
            .find_tree(id)
            .map_err(|_| "Git file metadata failed".to_owned())?;
        let Some(entry) = tree
            .lookup_entry(path.split('/').map(str::as_bytes))
            .map_err(|_| "Git file metadata failed".to_owned())?
        else {
            return Ok(Some(Vec::new()));
        };
        if entry.mode().is_commit() || entry.mode().is_tree() {
            return Ok(None);
        }
        let id = entry.object_id();
        // ls-tree -l reports BAD rather than a size for an unavailable blob.
        // Preserve that unknown endpoint without allocating an oversized body.
        let Ok(header) = self.repo.find_header(id) else {
            return Ok(None);
        };
        if header.size() > limit as u64 {
            return Ok(None);
        }
        let mut object = self
            .repo
            .find_blob(id)
            .map_err(|_| "Git file comparison unavailable".to_owned())?;
        Ok(Some(object.take_data()))
    }

    pub fn finish_sampling(&mut self) {
        self.previous.clear();
        self.snapshots.clear();
        drop(self.repo.set_freelist(None));
    }

    pub fn materialize(
        &mut self,
        scratch: &OnceLock<Arc<SnapshotDirectory>>,
        cancellation: &CancellationSignal,
    ) -> Result<Arc<SnapshotDirectory>, String> {
        if cancellation.is_cancelled() {
            return Err("Git snapshot was cancelled".into());
        }
        let directory = if let Some(directory) = scratch.get() {
            Arc::clone(directory)
        } else {
            let directory = Arc::new(
                SnapshotDirectory::create(&std::env::temp_dir())
                    .map_err(|error| error.to_string())?,
            );
            let _ = scratch.set(Arc::clone(&directory));
            directory
        };
        let git_dir = directory.path().join("git");
        if !git_dir.join("objects").exists() {
            gix::ThreadSafeRepository::init_opts(
                &git_dir,
                gix::create::Kind::Bare,
                gix::create::Options {
                    object_hash: Some(self.repo.object_hash()),
                    ..Default::default()
                },
                gix::open::Options::isolated(),
            )
            .map_err(|error| error.to_string())?;
        }
        let objects = gix::odb::loose::Store::at(git_dir.join("objects"), self.repo.object_hash());
        // reset_object_memory transfers the buffers, avoiding a second workspace
        // copy. Restore them even if an object write fails, for comparisons.
        let memory = self
            .repo
            .objects
            .reset_object_memory()
            .ok_or("Git snapshot memory unavailable")?;
        let result = memory.iter().try_for_each(|(id, (kind, bytes))| {
            objects
                .write_stream_with_known_id(
                    *kind,
                    bytes.len() as u64,
                    &mut ObjectInput {
                        bytes,
                        cancellation,
                    },
                    *id,
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        if let Err(error) = result {
            self.repo.objects.set_object_memory(memory);
            return Err(error);
        }
        // Completed records retain the private object store, not a workspace-sized
        // allocation or handles to the user's repository. New samples still write
        // into memory if this recorder takes another endpoint.
        self.repo = gix::open_opts(&git_dir, gix::open::Options::isolated().strict_config(true))
            .map_err(|error| error.to_string())?
            .with_object_memory();
        Ok(directory)
    }

    fn snapshot(
        &mut self,
        root: &Path,
        exclusion: &Path,
        cancellation: &CancellationSignal,
    ) -> Result<String, String> {
        let worktree = open(root)?.ok_or("Git worktree disappeared")?;
        let index = worktree
            .index_or_empty()
            .map_err(|error| error.to_string())?;
        let worktree_root = worktree
            .workdir()
            .ok_or("Git worktree unavailable")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let relative = root
            .strip_prefix(&worktree_root)
            .map_err(|_| "Snapshot root left the Git worktree")?;
        let scope = relative.to_str().ok_or("Git worktree path is not UTF-8")?;
        let scope = if scope.is_empty() {
            String::new()
        } else {
            format!("{scope}/")
        };
        let tracked = tracked_entries(&worktree, &index, cancellation, &scope)?;
        let mut candidates = self.previous.clone();
        candidates.extend(tracked.clone());
        for path in untracked(
            &worktree,
            &index,
            &worktree_root,
            exclusion,
            cancellation,
            &scope,
        )? {
            candidates.entry(path).or_insert(Entry {
                mode: "100644".into(),
                object: gix::ObjectId::null(self.repo.object_hash()),
                sparse: false,
            });
        }
        candidates.retain(|path, _| !root.join(path).starts_with(exclusion));
        let endpoints = self.raw_endpoints(root, candidates, &tracked, cancellation)?;
        if endpoints
            .values()
            .any(|entry| entry.mode == "160000" && entry.object.kind() != self.repo.object_hash())
        {
            return Err("Nested Git object format does not match the snapshot".into());
        }
        let mut tree = gix::objs::tree::Editor::new(
            gix::objs::Tree::default(),
            &self.repo.objects,
            self.repo.object_hash(),
        );
        for (path, entry) in &endpoints {
            validate_path(path, entry)?;
            let mode = gix::objs::tree::EntryMode::from_bytes(entry.mode.as_bytes())
                .ok_or("invalid Git snapshot mode")?;
            tree.upsert(path.split('/').map(BStr::new), mode.kind(), entry.object)
                .map_err(|error| error.to_string())?;
        }
        let id = tree
            .write(|tree| self.repo.objects.write(tree))
            .map_err(|error| error.to_string())?
            .to_string();
        self.previous.clone_from(&endpoints);
        self.snapshots.insert(id.clone(), endpoints);
        Ok(id)
    }

    fn raw_endpoints(
        &self,
        root: &Path,
        mut candidates: BTreeMap<String, Entry>,
        tracked: &BTreeMap<String, Entry>,
        cancellation: &CancellationSignal,
    ) -> Result<BTreeMap<String, Entry>, String> {
        let root_file = File::open(root).map_err(|error| error.to_string())?;
        let links = discover_links(&root_file, &candidates, tracked, cancellation)?;
        candidates.retain(|path, _| !links.keys().any(|link| Path::new(path).starts_with(link)));
        let mut endpoints = BTreeMap::new();
        for (path, retained) in links {
            let entry = if let Some(entry) = retained {
                entry
            } else {
                let nested = open(&root.join(&path))?.ok_or("Nested Git endpoint unavailable")?;
                let head = nested
                    .head()
                    .map_err(|error| error.to_string())?
                    .id()
                    .ok_or("Nested Git HEAD is unborn")?
                    .detach();
                Entry {
                    mode: "160000".into(),
                    object: head,
                    sparse: false,
                }
            };
            endpoints.insert(path, entry);
        }
        let mut budget = sampling::Budget::new(cancellation);
        for path in candidates.into_keys() {
            let mut bytes = Vec::new();
            match sampling::sample(&root_file, Path::new(&path), &mut bytes, &mut budget)? {
                sampling::Sample::File(mode) => {
                    // Bypass Repository::write_blob's disk existence check: every
                    // sampled byte must remain owned even if the real repo is deleted.
                    let object = self
                        .repo
                        .objects
                        .write_buf(gix::objs::Kind::Blob, &bytes)
                        .map_err(|error| error.to_string())?;
                    endpoints.insert(
                        path,
                        Entry {
                            mode,
                            object,
                            sparse: false,
                        },
                    );
                }
                sampling::Sample::Missing => {}
                sampling::Sample::Unsupported => {
                    if let Some(previous) = self.previous.get(&path) {
                        endpoints.insert(path, previous.clone());
                    }
                }
            }
        }
        Ok(endpoints)
    }
}

impl Repository {
    pub async fn snapshot(&self) -> Result<String, String> {
        let root = self.root.clone();
        let exclusion = self.exclusion.clone();
        let cancellation = self.cancellation.clone();
        let state = Arc::clone(&self.state);
        tokio::task::spawn_blocking(move || {
            state
                .lock()
                .expect("workspace Git state")
                .snapshot(&root, &exclusion, &cancellation)
        })
        .await
        .map_err(|error| error.to_string())?
    }
}

/// Match the private Git index's path checks before publishing a raw tree.
fn validate_path(path: &str, entry: &Entry) -> Result<(), String> {
    let mut components = path.split('/').peekable();
    while let Some(component) = components.next() {
        let mode = (components.peek().is_none() && entry.mode == "120000")
            .then_some(gix::validate::path::component::Mode::Symlink);
        gix::validate::path::component(
            BStr::new(component),
            mode,
            gix::validate::path::component::Options {
                protect_windows: cfg!(windows),
                protect_hfs: cfg!(target_os = "macos"),
                protect_ntfs: cfg!(windows),
            },
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn tracked_entries(
    repo: &gix::Repository,
    index: &gix::index::State,
    cancellation: &CancellationSignal,
    scope: &str,
) -> Result<BTreeMap<String, Entry>, String> {
    let mut tracked = BTreeMap::new();
    let mut bytes = 0;
    let budget = sampling::Budget::new(cancellation);
    for entry in index.entries() {
        budget.check(0)?;
        let path = entry
            .path(index)
            .to_str()
            .map_err(|error| error.to_string())?;
        if entry.mode.is_sparse() {
            expand_sparse(
                repo,
                entry.id,
                path.trim_end_matches('/'),
                &mut tracked,
                &mut bytes,
                &budget,
                scope,
            )?;
        } else if let Some(path) = path.strip_prefix(scope) {
            add_tracked(
                &mut tracked,
                &mut bytes,
                path.to_owned(),
                Entry {
                    mode: format!("{:o}", entry.mode.bits()),
                    object: entry.id,
                    sparse: entry
                        .flags
                        .contains(gix::index::entry::Flags::SKIP_WORKTREE),
                },
            )?;
        }
    }
    Ok(tracked)
}

fn add_tracked(
    tracked: &mut BTreeMap<String, Entry>,
    bytes: &mut usize,
    path: String,
    entry: Entry,
) -> Result<(), String> {
    // Preserve the former ls-files -v --stage -z output bound, including
    // the hash width and the flag/mode/stage separators.
    *bytes += path.len() + entry.object.kind().len_in_hex() + 13;
    if *bytes > PATH_BYTES {
        return Err("Git index paths exceeded their limit".into());
    }
    tracked.insert(path, entry);
    Ok(())
}

/// A sparse index still owns materialized excluded files. Expand its trees in
/// memory, sampling physical files exactly as with an ordinary skip-worktree index.
fn expand_sparse(
    repo: &gix::Repository,
    id: gix::ObjectId,
    prefix: &str,
    tracked: &mut BTreeMap<String, Entry>,
    bytes: &mut usize,
    budget: &sampling::Budget<'_>,
    scope: &str,
) -> Result<(), String> {
    let mut pending = vec![(id, prefix.to_owned())];
    while let Some((id, prefix)) = pending.pop() {
        budget.check(0)?;
        if !prefix.starts_with(scope) && !scope.starts_with(&format!("{prefix}/")) {
            continue;
        }
        // Directory names count too, bounding malformed/cyclic trees without
        // recursive calls or an unbounded queue.
        *bytes += prefix.len() + 1;
        if *bytes > PATH_BYTES {
            return Err("Git sparse paths exceeded their limit".into());
        }
        let tree = repo.find_tree(id).map_err(|error| error.to_string())?;
        for entry in tree.iter() {
            budget.check(0)?;
            let entry = entry.map_err(|error| error.to_string())?;
            let path = format!(
                "{prefix}/{}",
                entry
                    .filename()
                    .to_str()
                    .map_err(|error| error.to_string())?
            );
            if entry.mode().is_tree() {
                *bytes += path.len() + 1;
                if *bytes > PATH_BYTES {
                    return Err("Git sparse paths exceeded their limit".into());
                }
                pending.push((entry.object_id(), path));
            } else if let Some(path) = path.strip_prefix(scope) {
                add_tracked(
                    tracked,
                    bytes,
                    path.to_owned(),
                    Entry {
                        mode: entry.mode().kind().as_octal_str().to_string(),
                        object: entry.object_id(),
                        sparse: true,
                    },
                )?;
            }
        }
    }
    Ok(())
}

fn untracked(
    worktree: &gix::Repository,
    index: &gix::index::State,
    root: &Path,
    exclusion: &Path,
    cancellation: &CancellationSignal,
    scope: &str,
) -> Result<BTreeSet<String>, String> {
    let mut collect = Collect {
        entries: BTreeSet::new(),
        budget: sampling::Budget::new(cancellation),
        failed: None,
        bytes: 0,
        root,
        exclusion,
        scope,
    };
    // An enclosing repository may become visible if .git is removed during
    // inference. Keep ls-files' original cwd-relative membership and namespace.
    let patterns: Vec<gix::bstr::BString> = if scope.is_empty() {
        Vec::new()
    } else {
        vec![format!(":(top,literal){}", scope.trim_end_matches('/')).into()]
    };
    worktree
        .dirwalk(
            index,
            patterns,
            &AtomicBool::new(false),
            worktree
                .dirwalk_options()
                .map_err(|error| error.to_string())?,
            &mut collect,
        )
        .map_err(|error| error.to_string())?;
    if let Some(error) = collect.failed {
        return Err(error);
    }
    Ok(collect.entries)
}

struct Collect<'a> {
    entries: BTreeSet<String>,
    budget: sampling::Budget<'a>,
    failed: Option<String>,
    bytes: usize,
    root: &'a Path,
    exclusion: &'a Path,
    scope: &'a str,
}
impl gix::dir::walk::Delegate for Collect<'_> {
    fn emit(
        &mut self,
        entry: gix::dir::EntryRef<'_>,
        _: Option<gix::dir::entry::Status>,
    ) -> gix::dir::walk::Action {
        let result = (|| {
            self.budget.check(0)?;
            let path = entry
                .rela_path
                .to_str()
                .map_err(|error| error.to_string())?;
            if entry.status == gix::dir::entry::Status::Untracked
                && !self.root.join(path).starts_with(self.exclusion)
            {
                let Some(path) = path.strip_prefix(self.scope) else {
                    return Ok(());
                };
                self.bytes += path.len() + 1;
                if self.bytes > PATH_BYTES {
                    return Err("Git untracked paths exceeded their limit".into());
                }
                self.entries.insert(path.to_owned());
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.failed = Some(error);
            return std::ops::ControlFlow::Break(());
        }
        std::ops::ControlFlow::Continue(())
    }
    fn can_recurse(
        &mut self,
        entry: gix::dir::EntryRef<'_>,
        for_deletion: Option<gix::dir::walk::ForDeletionMode>,
        root_is_repository: bool,
    ) -> bool {
        if let Err(error) = self.budget.check(0) {
            self.failed = Some(error);
            return false;
        }
        let Ok(path) = entry.rela_path.to_str() else {
            self.failed = Some("Git path is not UTF-8".into());
            return false;
        };
        !self.root.join(path).starts_with(self.exclusion)
            && entry.status.can_recurse(
                entry.disk_kind,
                entry.pathspec_match,
                for_deletion,
                root_is_repository,
            )
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
