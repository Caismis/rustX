//! Bounded native observation of the supported configuration sources.
//!
//! A filesystem notification is invalidation, never trusted content. It only
//! names the source target that owns a changed path; the existing capture
//! path then rereads, fences and validates that target's files exactly as it
//! does after a native source write, and an unchanged healthy source is a
//! no-op. Notifications for one target coalesce into one capture.
//!
//! Coverage, and nothing else:
//!
//! | Source | Watched |
//! | --- | --- |
//! | User document | its directory, non-recursively, for that one file |
//! | User resources and guidance | `~/rustx/.agents` for `mcp.toml` and its guidance files, and its `agents`, `skills`, `tools` and `workflows`, recursively (the parent until it exists) |
//! | Workspace document and root guidance | each known Workspace root, non-recursively, for `rustx.toml`, the guidance files and `*.md` |
//! | Workspace resources | each known Workspace's `.agents`, as for the User |
//!
//! The guidance files are exactly the loader's (`AGENTS.override.md`,
//! `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD`). A notification for a
//! watched directory (or an ancestor) may mean it was removed or replaced at
//! the same path, so its watches are registered again; only a watch that
//! actually registered counts as active.
//!
//! When the platform reports that notifications may have been lost (an
//! overflowed queue, for one), no path says what changed: every known source
//! target is captured again and every watch registered again. Unchanged
//! sources stay no-ops, so a repeated rescan records nothing.
//!
//! A notification names a path as the volume reports it. A resource root,
//! source entry, guidance file or document reported in another case of the
//! same entry (a volume that folds case) is respelled with the fixed name the
//! loaders open before it is routed or invalidates watches, so the watcher
//! sees exactly the sources they read. The entry decides by identity, never
//! by probing the volume through an unrelated name.
//!
//! Anything else under `.agents` — Session uploads in particular — is not a
//! configuration source and is neither watched nor routed.
//!
//! Known Workspaces are the directories that already hold source authority or
//! are a Session application's configuration directory; an unrelated
//! directory is never scanned.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use notify::{RecursiveMode, Watcher};

use super::SessionRuntimeManager;
use crate::local_runtime::configuration::settings::SourceTarget;

use crate::local_runtime::resource_directory::SOURCE_ENTRIES;

/// Whether `path` is `root` itself, one of its guidance files, or lies under
/// one of its source entries.
fn is_resource(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        let mut components = relative.components();
        components.next().is_none_or(|entry| {
            SOURCE_ENTRIES.iter().any(|name| entry.as_os_str() == *name)
                || (components.next().is_none() && is_guidance(entry.as_os_str()))
        })
    })
}

/// Whether `name` is one of the guidance files the loader reads.
fn is_guidance(name: &std::ffi::OsStr) -> bool {
    crate::runtime::resources::PROJECT_CONTEXT_FILENAMES
        .iter()
        .any(|guidance| name == *guidance)
}

/// Whether `actual` and `fixed`, two spellings of one name in one directory,
/// are the entry a loader opening `fixed` reaches: the same file (a volume
/// that folds case), or both gone (a removed source, which a case-folding
/// volume no longer tells apart; capturing it again is a no-op elsewhere).
/// One present without the other, or two distinct files, are two entries.
fn same_entry(actual: &Path, fixed: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (
        std::fs::symlink_metadata(actual),
        std::fs::symlink_metadata(fixed),
    ) {
        (Ok(actual), Ok(fixed)) => (actual.dev(), actual.ino()) == (fixed.dev(), fixed.ino()),
        (Err(actual), Err(fixed)) => {
            actual.kind() == std::io::ErrorKind::NotFound
                && fixed.kind() == std::io::ErrorKind::NotFound
        }
        _ => false,
    }
}

/// The fixed name, among `names`, that a loader opening it in `directory`
/// reaches at `actual`: the same spelling, or another case of it that is
/// [`same`](same_entry) entry.
fn loader_name<'a>(
    directory: &Path,
    actual: &std::ffi::OsStr,
    mut names: impl Iterator<Item = &'a str> + Clone,
    same: &impl Fn(&Path, &Path) -> bool,
) -> Option<&'a str> {
    if let Some(name) = names.clone().find(|name| actual == *name) {
        return Some(name);
    }
    let actual = actual.to_str()?;
    names.find(|name| {
        name.eq_ignore_ascii_case(actual) && same(&directory.join(actual), &directory.join(name))
    })
}

/// `path` as the loaders spell it: each fixed component (a resource root, a
/// source entry, a guidance file, a document) that the volume reports in
/// another case of the same entry is respelled with the name the loaders
/// open, so routing and watch invalidation see exactly the sources they read.
/// Anything else is returned unchanged.
fn loader_path(
    path: &Path,
    user: &UserSourceRoots,
    workspaces: &[PathBuf],
    same: &impl Fn(&Path, &Path) -> bool,
) -> PathBuf {
    let guidance = || crate::runtime::resources::PROJECT_CONTEXT_FILENAMES.into_iter();
    let mut path = path.to_path_buf();
    let resource_roots = std::iter::once((
        user.resources.ancestors().nth(USER_RESOURCES.len()),
        &USER_RESOURCES[..],
    ))
    .chain(
        workspaces
            .iter()
            .map(|workspace| (Some(workspace.as_path()), &[".agents"][..])),
    );
    for (base, fixed) in resource_roots {
        let Some((root, entry, rest)) = base.and_then(|base| loader_root(&path, base, fixed, same))
        else {
            continue;
        };
        let Some(entry) = entry else {
            return root;
        };
        let names = SOURCE_ENTRIES.into_iter().chain(
            rest.as_os_str()
                .is_empty()
                .then(guidance)
                .into_iter()
                .flatten(),
        );
        if let Some(name) = loader_name(&root, &entry, names, same) {
            return root.join(name).join(rest);
        }
        // Not a source of this root: a more specific Workspace inside it may
        // still own the path, under the root's loader spelling.
        path = root.join(entry).join(rest);
    }
    let (Some(parent), Some(actual)) = (path.parent(), path.file_name()) else {
        return path;
    };
    if Some(parent) == user.document.parent()
        && let Some(document) = user.document.file_name().and_then(|name| name.to_str())
        && let Some(name) = loader_name(parent, actual, std::iter::once(document), same)
    {
        return parent.join(name);
    }
    if workspaces.iter().any(|workspace| workspace == parent)
        && let Some(name) = loader_name(
            parent,
            actual,
            std::iter::once("rustx.toml").chain(guidance()),
            same,
        )
    {
        return parent.join(name);
    }
    path
}

/// The resource root `base/fixed…` that `path` lies in, spelled as the
/// loaders open it, with the entry directly beneath it and the rest of the
/// path; `None` when any fixed component is another entry.
fn loader_root(
    path: &Path,
    base: &Path,
    fixed: &[&str],
    same: &impl Fn(&Path, &Path) -> bool,
) -> Option<(PathBuf, Option<std::ffi::OsString>, PathBuf)> {
    let mut components = path.strip_prefix(base).ok()?.components();
    let mut root = base.to_path_buf();
    for name in fixed {
        let actual = components.next()?;
        root.push(loader_name(
            &root,
            actual.as_os_str(),
            std::iter::once(*name),
            same,
        )?);
    }
    let entry = components.next().map(|entry| entry.as_os_str().to_owned());
    Some((root, entry, components.as_path().to_path_buf()))
}

/// The watches of one existing resource root: the root itself, for its
/// direct entries, and each source directory recursively.
fn resource_watches(root: &Path, watches: &mut BTreeMap<PathBuf, RecursiveMode>) {
    watches.insert(root.to_path_buf(), RecursiveMode::NonRecursive);
    for name in SOURCE_ENTRIES {
        let entry = root.join(name);
        if entry.is_dir() {
            watches.insert(entry, RecursiveMode::Recursive);
        }
    }
}

/// The fixed components of the User resource root beneath the home
/// directory.
const USER_RESOURCES: [&str; 2] = ["rustx", ".agents"];

/// The fixed User source locations of one process.
#[derive(Debug, Clone)]
pub(super) struct UserSourceRoots {
    pub(super) document: PathBuf,
    pub(super) resources: PathBuf,
}

/// The source target that owns a changed `path`, or `None` for a path no
/// supported source reads.
pub(super) fn route(
    path: &Path,
    user: &UserSourceRoots,
    workspaces: &[PathBuf],
) -> Option<SourceTarget> {
    if path == user.document || is_resource(path, &user.resources) {
        return Some(SourceTarget::User);
    }
    workspaces
        .iter()
        .filter(|workspace| {
            is_resource(path, &workspace.join(".agents"))
                || (path.parent() == Some(workspace.as_path())
                    && (path
                        .file_name()
                        .is_some_and(|name| name == "rustx.toml" || is_guidance(name))
                        || path.extension().is_some_and(|extension| extension == "md")))
        })
        .max_by_key(|workspace| workspace.as_os_str().len())
        .map(|workspace| SourceTarget::Workspace {
            directory: workspace.clone(),
        })
}

/// What one platform notification invalidates.
#[derive(Debug)]
pub(super) enum Signal {
    /// A path that may have changed.
    Path(PathBuf),
    /// Notifications may have been lost: every source may have changed.
    Rescan,
}

/// Forward one platform notification as the signals it carries. A rescan
/// carries no path, so it is kept apart from the paths it may also name.
pub(super) fn forward(
    result: notify::Result<notify::Event>,
    sender: &tokio::sync::mpsc::UnboundedSender<Signal>,
) {
    let Ok(event) = result else {
        return;
    };
    if event.need_rescan() {
        let _ = sender.send(Signal::Rescan);
    }
    if !event.kind.is_access() {
        for path in event.paths {
            let _ = sender.send(Signal::Path(path));
        }
    }
}

/// A notification at `path` may mean that directory was removed or replaced
/// in place, which silently ends the platform watches at or below it: those
/// are registered again on the next pass.
fn invalidate(active: &mut BTreeMap<PathBuf, RecursiveMode>, path: &Path) {
    active.retain(|watched, _| !watched.starts_with(path));
}

/// The exact watch set for these roots: existing directories only.
pub(super) fn desired(
    user: &UserSourceRoots,
    workspaces: &[PathBuf],
) -> BTreeMap<PathBuf, RecursiveMode> {
    let mut watches = BTreeMap::new();
    if let Some(parent) = user.document.parent() {
        watches.insert(parent.to_path_buf(), RecursiveMode::NonRecursive);
    }
    if user.resources.is_dir() {
        resource_watches(&user.resources, &mut watches);
    } else if let Some(parent) = user.resources.parent().filter(|parent| parent.is_dir()) {
        watches
            .entry(parent.to_path_buf())
            .or_insert(RecursiveMode::NonRecursive);
    }
    for workspace in workspaces {
        watches
            .entry(workspace.clone())
            .or_insert(RecursiveMode::NonRecursive);
        let resources = workspace.join(".agents");
        if resources.is_dir() {
            resource_watches(&resources, &mut watches);
        }
    }
    watches.retain(|path, _| path.is_dir());
    watches
}

/// Converge the registered watches on `desired`; only a watch that
/// registered is active.
fn rearm(
    watcher: &mut impl Watcher,
    active: &mut BTreeMap<PathBuf, RecursiveMode>,
    desired: &BTreeMap<PathBuf, RecursiveMode>,
) {
    active.retain(|path, _| {
        let retained = desired.contains_key(path);
        if !retained {
            let _ = watcher.unwatch(path);
        }
        retained
    });
    for (path, mode) in desired {
        if active.get(path) != Some(mode) {
            let _ = watcher.unwatch(path);
            active.remove(path);
            if watcher.watch(path, *mode).is_ok() {
                active.insert(path.clone(), *mode);
            }
        }
    }
}

impl SessionRuntimeManager {
    fn user_source_roots(&self) -> UserSourceRoots {
        UserSourceRoots {
            document: self.configuration.sources.config_path.clone(),
            resources: USER_RESOURCES.iter().fold(
                self.configuration.sources.home_directory.clone(),
                |path, name| path.join(name),
            ),
        }
    }

    fn known_workspaces(&self) -> Vec<PathBuf> {
        self.applications.lock().watched_source_directories()
    }

    /// Deliver `event` as the platform watcher of the running source watch
    /// would. Returns `false` while no source watch has started.
    #[cfg(test)]
    pub(crate) fn notify_source_watch(&self, event: notify::Event) -> bool {
        let registry = self.registry.0.lock().expect("registry mutex");
        let Some(sender) = &registry.source_watch_signals else {
            return false;
        };
        forward(Ok(event), sender);
        true
    }

    /// Observe the supported sources until `stop`. Each batch of notifications
    /// is routed to its owning targets and captured once per target through
    /// the same path as a native source write; nothing here reads content.
    ///
    /// # Errors
    ///
    /// Returns the platform watcher's construction failure.
    ///
    /// # Panics
    ///
    /// Panics only if the test-only registry mutex is poisoned.
    pub async fn run_source_watch(
        &self,
        stop: tokio_util::sync::CancellationToken,
    ) -> Result<(), String> {
        let (sender, mut signals) = tokio::sync::mpsc::unbounded_channel::<Signal>();
        #[cfg(test)]
        {
            self.registry
                .0
                .lock()
                .expect("registry mutex")
                .source_watch_signals = Some(sender.clone());
        }
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                forward(result, &sender);
            })
            .map_err(|error| error.to_string())?;
        let mut changes = self.configuration_changes();
        let mut active = BTreeMap::<PathBuf, RecursiveMode>::new();
        loop {
            let user = self.user_source_roots();
            let workspaces = self.known_workspaces();
            rearm(&mut watcher, &mut active, &desired(&user, &workspaces));
            tokio::select! {
                () = stop.cancelled() => return Ok(()),
                result = changes.changed() => if result.is_err() { return Ok(()) },
                Some(signal) = signals.recv() => {
                    // Coalesce everything already queued into one capture per
                    // owning target, in a stable order: User composes first.
                    #[cfg(test)]
                    let aliased = self
                        .registry
                        .0
                        .lock()
                        .expect("registry mutex")
                        .source_watch_case_aliases;
                    #[cfg(not(test))]
                    let aliased = false;
                    let same = |actual: &Path, fixed: &Path| aliased || same_entry(actual, fixed);
                    let mut targets = Vec::new();
                    let mut next = Some(signal);
                    while let Some(signal) = next {
                        let owners = match signal {
                            Signal::Path(path) => {
                                let path = loader_path(&path, &user, &workspaces, &same);
                                invalidate(&mut active, &path);
                                route(&path, &user, &workspaces).into_iter().collect()
                            }
                            Signal::Rescan => {
                                active.clear();
                                std::iter::once(SourceTarget::User)
                                    .chain(workspaces.iter().map(|directory| {
                                        SourceTarget::Workspace {
                                            directory: directory.clone(),
                                        }
                                    }))
                                    .collect::<Vec<_>>()
                            }
                        };
                        for target in owners {
                            if !targets.contains(&target) {
                                targets.push(target);
                            }
                        }
                        next = signals.try_recv().ok();
                    }
                    // Watch the replacements before capturing them, so an
                    // edit after the capture reads is always notified.
                    rearm(&mut watcher, &mut active, &desired(&user, &workspaces));
                    targets.sort_by_key(|target| target.workspace().is_some());
                    for target in targets {
                        self.observe_source_change(&target).await;
                    }
                    #[cfg(test)]
                    {
                        let gate = self
                            .registry
                            .0
                            .lock()
                            .expect("registry mutex")
                            .source_watch_captured
                            .clone();
                        gate.park().await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(home: &Path) -> UserSourceRoots {
        UserSourceRoots {
            document: home.join("config/rustx.toml"),
            resources: home.join("rustx/.agents"),
        }
    }

    #[test]
    fn notifications_route_only_supported_sources_to_their_owner() {
        let home = Path::new("/home/u");
        let user = roots(home);
        let workspaces = vec![PathBuf::from("/work/a"), PathBuf::from("/work/a/nested")];
        let workspace = |directory: &str| {
            Some(SourceTarget::Workspace {
                directory: directory.into(),
            })
        };
        assert_eq!(
            route(&home.join("config/rustx.toml"), &user, &workspaces),
            Some(SourceTarget::User)
        );
        assert_eq!(
            route(&home.join("config/other.toml"), &user, &workspaces),
            None
        );
        assert_eq!(
            route(
                &home.join("rustx/.agents/skills/x/SKILL.md"),
                &user,
                &workspaces
            ),
            Some(SourceTarget::User)
        );
        assert_eq!(
            route(Path::new("/work/a/rustx.toml"), &user, &workspaces),
            workspace("/work/a")
        );
        assert_eq!(
            route(Path::new("/work/a/AGENTS.md"), &user, &workspaces),
            workspace("/work/a")
        );
        assert_eq!(
            route(Path::new("/work/a/.agents/mcp.toml"), &user, &workspaces),
            workspace("/work/a")
        );
        // The most specific known Workspace owns its own sources.
        assert_eq!(
            route(Path::new("/work/a/nested/rustx.toml"), &user, &workspaces),
            workspace("/work/a/nested")
        );
        // Session uploads and other `.agents` entries are not sources.
        assert_eq!(
            route(
                Path::new("/work/a/.agents/uploads/s/b/note.md"),
                &user,
                &workspaces
            ),
            None
        );
        assert_eq!(
            route(&home.join("rustx/.agents/uploads/x"), &user, &workspaces),
            None
        );
        assert_eq!(
            route(&home.join("rustx/.agents"), &user, &workspaces),
            Some(SourceTarget::User)
        );
        // Every guidance file the loader reads, at either root.
        for name in ["AGENTS.override.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"] {
            assert_eq!(
                route(&Path::new("/work/a").join(name), &user, &workspaces),
                workspace("/work/a")
            );
            assert_eq!(
                route(&home.join("rustx/.agents").join(name), &user, &workspaces),
                Some(SourceTarget::User)
            );
        }
        assert_eq!(
            route(&home.join("rustx/.agents/notes.md"), &user, &workspaces),
            None
        );
        assert_eq!(
            route(
                &home.join("rustx/.agents/uploads/AGENTS.md"),
                &user,
                &workspaces
            ),
            None
        );
        // Ordinary project files never reach configuration.
        assert_eq!(
            route(Path::new("/work/a/src/main.rs"), &user, &workspaces),
            None
        );
        assert_eq!(
            route(Path::new("/work/a/docs/notes.md"), &user, &workspaces),
            None
        );
        assert_eq!(
            route(Path::new("/elsewhere/rustx.toml"), &user, &workspaces),
            None
        );
    }

    /// A case-folding volume reports a source in the case it was created
    /// in; the loaders open the fixed names and read it, so the watcher
    /// routes and invalidates it under those names — the resource root
    /// included, in a Workspace of any name, for removals too. Where the
    /// spellings are distinct entries, the variant is not a source.
    #[test]
    fn a_case_variant_routes_only_as_the_entry_the_loaders_open() {
        let home = Path::new("/home/u");
        let user = roots(home);
        let workspaces = vec![
            PathBuf::from("/w"),
            PathBuf::from("/2026"),
            PathBuf::from("/w/.agents/project"),
        ];
        let aliases = |_: &Path, _: &Path| true;
        let distinct = |_: &Path, _: &Path| false;
        let spelled = |path: &str, same: &dyn Fn(&Path, &Path) -> bool| {
            loader_path(Path::new(path), &user, &workspaces, &same)
        };
        for (actual, loader, owner) in [
            (
                "/w/.agents/Skills/guide/SKILL.md",
                "/w/.agents/skills/guide/SKILL.md",
                "/w",
            ),
            (
                "/w/.Agents/skills/guide/SKILL.md",
                "/w/.agents/skills/guide/SKILL.md",
                "/w",
            ),
            ("/w/.AGENTS/Skills", "/w/.agents/skills", "/w"),
            ("/w/.Agents", "/w/.agents", "/w"),
            ("/w/.agents/MCP.toml", "/w/.agents/mcp.toml", "/w"),
            ("/w/.agents/claude.md", "/w/.agents/CLAUDE.md", "/w"),
            ("/w/RustX.toml", "/w/rustx.toml", "/w"),
            ("/2026/RustX.toml", "/2026/rustx.toml", "/2026"),
            (
                "/2026/.Agents/tools/t.toml",
                "/2026/.agents/tools/t.toml",
                "/2026",
            ),
            // A Workspace inside another's `.agents` owns its own document:
            // the outer root, which has no `project` source, never ends the
            // search.
            (
                "/w/.agents/project/RustX.toml",
                "/w/.agents/project/rustx.toml",
                "/w/.agents/project",
            ),
            (
                "/w/.Agents/project/RustX.toml",
                "/w/.agents/project/rustx.toml",
                "/w/.agents/project",
            ),
            (
                "/w/.agents/project/.Agents/skills/x/SKILL.md",
                "/w/.agents/project/.agents/skills/x/SKILL.md",
                "/w/.agents/project",
            ),
        ] {
            let path = spelled(actual, &aliases);
            assert_eq!(path, Path::new(loader));
            assert_eq!(
                route(&path, &user, &workspaces),
                Some(SourceTarget::Workspace {
                    directory: owner.into()
                }),
                "{actual}"
            );
            let path = spelled(actual, &distinct);
            assert_eq!(path, Path::new(actual));
            assert_eq!(route(&path, &user, &workspaces), None, "{actual}");
        }
        for (actual, loader) in [
            (
                "/home/u/rustx/.Agents/Workflows/a.yaml",
                "/home/u/rustx/.agents/workflows/a.yaml",
            ),
            (
                "/home/u/RustX/.agents/skills/guide/SKILL.md",
                "/home/u/rustx/.agents/skills/guide/SKILL.md",
            ),
            ("/home/u/RUSTX/.AGENTS", "/home/u/rustx/.agents"),
            ("/home/u/config/RUSTX.toml", "/home/u/config/rustx.toml"),
        ] {
            let path = spelled(actual, &aliases);
            assert_eq!(path, Path::new(loader));
            assert_eq!(route(&path, &user, &workspaces), Some(SourceTarget::User));
            assert_eq!(route(&spelled(actual, &distinct), &user, &workspaces), None);
        }
        // Uploads stay unrouted in any case.
        assert_eq!(
            route(
                &spelled("/w/.Agents/Uploads/s/AGENTS.md", &aliases),
                &user,
                &workspaces
            ),
            None
        );
        // The exact spelling wins where both exist.
        assert_eq!(spelled("/w/AGENTS.MD", &aliases), Path::new("/w/AGENTS.MD"));
        // A replaced case-variant directory re-registers the watches beneath
        // the loaders' spelling.
        let mut active = BTreeMap::from([
            (PathBuf::from("/w"), RecursiveMode::NonRecursive),
            (PathBuf::from("/w/.agents"), RecursiveMode::NonRecursive),
            (PathBuf::from("/w/.agents/skills"), RecursiveMode::Recursive),
        ]);
        invalidate(&mut active, &spelled("/w/.agents/Skills", &aliases));
        assert_eq!(active.len(), 2);
        invalidate(&mut active, &spelled("/w/.Agents", &aliases));
        assert_eq!(active.keys().collect::<Vec<_>>(), [Path::new("/w")]);
    }

    /// On a verified case-sensitive volume, a case variant that exists apart
    /// from the loaders' spelling, or without it, is another entry and stays
    /// unrouted; a variant gone together with the loaders' spelling is the
    /// removed source.
    #[cfg(target_os = "linux")]
    #[test]
    fn distinct_case_variants_are_not_the_entry_the_loaders_open() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("w");
        std::fs::create_dir_all(workspace.join(".agents/skills/x")).unwrap();
        assert_eq!(
            std::fs::symlink_metadata(workspace.join(".AGENTS"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound,
            "this test needs a case-sensitive volume"
        );
        std::fs::create_dir_all(workspace.join(".Agents/skills/x")).unwrap();
        std::fs::create_dir_all(workspace.join(".agents/Skills/x")).unwrap();
        std::fs::write(workspace.join("RustX.toml"), b"").unwrap();
        let user = roots(directory.path());
        let workspaces = vec![workspace.clone()];
        for actual in [
            ".Agents/skills/x/SKILL.md",
            ".agents/Skills/x/SKILL.md",
            "RustX.toml",
        ] {
            let actual = workspace.join(actual);
            let path = loader_path(&actual, &user, &workspaces, &same_entry);
            assert_eq!(path, actual);
            assert_eq!(route(&path, &user, &workspaces), None, "{actual:?}");
        }
        assert_eq!(
            loader_path(
                &workspace.join(".agents/Tools"),
                &user,
                &workspaces,
                &same_entry
            ),
            workspace.join(".agents/tools")
        );
    }

    #[test]
    fn a_notification_reregisters_the_watches_at_or_below_it() {
        let mut active = BTreeMap::from([
            (PathBuf::from("/w"), RecursiveMode::NonRecursive),
            (PathBuf::from("/w/.agents"), RecursiveMode::NonRecursive),
            (PathBuf::from("/w/.agents/skills"), RecursiveMode::Recursive),
            (PathBuf::from("/w/.agents/tools"), RecursiveMode::Recursive),
        ]);
        invalidate(&mut active, Path::new("/w/.agents/skills/a/SKILL.md"));
        assert_eq!(active.len(), 4, "a file edit replaces no watch");
        invalidate(&mut active, Path::new("/w/.agents/skills"));
        assert!(!active.contains_key(Path::new("/w/.agents/skills")));
        assert!(active.contains_key(Path::new("/w/.agents/tools")));
        invalidate(&mut active, Path::new("/w/.agents"));
        assert_eq!(active.keys().collect::<Vec<_>>(), [Path::new("/w")]);
    }

    #[test]
    fn the_watch_set_is_bounded_to_existing_source_directories() {
        let home = tempfile::tempdir().unwrap();
        let user = roots(home.path());
        std::fs::create_dir_all(home.path().join("config")).unwrap();
        std::fs::create_dir_all(home.path().join("rustx")).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspaces = vec![workspace.path().to_path_buf()];
        let watches = desired(&user, &workspaces);
        assert_eq!(
            watches,
            BTreeMap::from([
                (home.path().join("config"), RecursiveMode::NonRecursive),
                (home.path().join("rustx"), RecursiveMode::NonRecursive),
                (workspace.path().to_path_buf(), RecursiveMode::NonRecursive),
            ])
        );
        std::fs::create_dir_all(home.path().join("rustx/.agents/skills")).unwrap();
        std::fs::create_dir_all(workspace.path().join(".agents/uploads")).unwrap();
        std::fs::create_dir_all(workspace.path().join(".agents/workflows")).unwrap();
        let watches = desired(&user, &workspaces);
        assert_eq!(
            watches.get(&home.path().join("rustx/.agents")),
            Some(&RecursiveMode::NonRecursive)
        );
        assert_eq!(
            watches.get(&home.path().join("rustx/.agents/skills")),
            Some(&RecursiveMode::Recursive)
        );
        assert_eq!(
            watches.get(&workspace.path().join(".agents/workflows")),
            Some(&RecursiveMode::Recursive)
        );
        // Uploads are never watched, however many batches they hold.
        assert!(!watches.contains_key(&workspace.path().join(".agents/uploads")));
        assert!(
            watches
                .keys()
                .all(|path| !path.starts_with(workspace.path().join(".agents/uploads")))
        );
        assert!(!watches.contains_key(&home.path().join("rustx")));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod case_insensitive_tests {
    use super::*;

    /// On a case-insensitive volume each source the loaders read through
    /// their fixed spelling — under `.agents/Skills`, under a `.Agents` root,
    /// a `RustX.toml` in a Workspace whose name has no letters or that lies
    /// inside another Workspace's `.agents`, and a User Skill under `RustX` —
    /// routes to its owner when notified under its own spelling; a
    /// renamed-away root invalidates the watches beneath the loaders'
    /// spelling.
    #[test]
    fn a_case_variant_the_loaders_read_routes_to_its_workspace() {
        let volume = crate::local_runtime::case_insensitive_volume::CaseInsensitiveVolume::new();
        let entry = volume.path().join("entry");
        let root = volume.path().join("root");
        let digits = volume.path().join("2026");
        let nested = entry.join(".agents/project");
        let home = volume.path().join("home");
        let files = [
            (
                entry.join(".agents/Skills/guide/SKILL.md"),
                entry.join(".agents/skills/guide/SKILL.md"),
                &entry,
            ),
            (
                root.join(".Agents/skills/guide/SKILL.md"),
                root.join(".agents/skills/guide/SKILL.md"),
                &root,
            ),
            (
                digits.join("RustX.toml"),
                digits.join("rustx.toml"),
                &digits,
            ),
            (
                nested.join("RustX.toml"),
                nested.join("rustx.toml"),
                &nested,
            ),
        ];
        let user = UserSourceRoots {
            document: volume.path().join("config/rustx.toml"),
            resources: USER_RESOURCES
                .iter()
                .fold(home.clone(), |path, name| path.join(name)),
        };
        let workspaces = vec![entry.clone(), root.clone(), digits.clone(), nested.clone()];
        for (actual, loader, workspace) in &files {
            std::fs::create_dir_all(actual.parent().unwrap()).unwrap();
            std::fs::write(actual, b"source").unwrap();
            assert_eq!(
                std::fs::read(loader).unwrap(),
                b"source",
                "the loaders read {actual:?}"
            );
            let path = loader_path(actual, &user, &workspaces, &same_entry);
            assert_eq!(&path, loader);
            assert_eq!(
                route(&path, &user, &workspaces),
                Some(SourceTarget::Workspace {
                    directory: (*workspace).clone()
                })
            );
        }
        // A User Skill under `RustX`, which the loaders read through
        // `rustx/.agents/skills`.
        let skill = home.join("RustX/.agents/skills/guide/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(&skill, b"user").unwrap();
        let loader = user.resources.join("skills/guide/SKILL.md");
        assert_eq!(std::fs::read(&loader).unwrap(), b"user");
        let path = loader_path(&skill, &user, &workspaces, &same_entry);
        assert_eq!(path, loader);
        assert_eq!(route(&path, &user, &workspaces), Some(SourceTarget::User));
        std::fs::rename(root.join(".Agents"), root.join("old")).unwrap();
        let mut active = BTreeMap::from([
            (root.join(".agents"), RecursiveMode::NonRecursive),
            (root.join(".agents/skills"), RecursiveMode::Recursive),
        ]);
        invalidate(
            &mut active,
            &loader_path(&root.join(".Agents"), &user, &workspaces, &same_entry),
        );
        assert!(active.is_empty());
    }
}
