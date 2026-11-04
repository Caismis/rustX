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
//! A notification names a path as the volume reports it. Where the volume
//! folds case, a source entry, guidance file or document reported in another
//! case is respelled with the fixed name the loaders open before it is
//! routed, so the watcher sees exactly the sources they read.
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

/// Whether the volume holding `directory` folds case: its own name with the
/// case of its ASCII letters flipped names the same directory. A name without
/// letters cannot be probed and counts as case-sensitive.
fn folds_case(directory: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let flipped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect();
    if flipped == name {
        return false;
    }
    match (
        std::fs::symlink_metadata(directory),
        std::fs::symlink_metadata(directory.with_file_name(flipped)),
    ) {
        (Ok(actual), Ok(alias)) => (actual.dev(), actual.ino()) == (alias.dev(), alias.ino()),
        _ => false,
    }
}

/// The fixed name, among `names`, that a loader opening it in `directory`
/// would reach at `actual`: the same spelling, or — on a volume that folds
/// case — any case of it. The volume is probed through `directory`, which
/// outlives a removed or renamed `actual`.
fn loader_name<'a>(
    directory: &Path,
    actual: &std::ffi::OsStr,
    names: impl Iterator<Item = &'a str> + Clone,
    folds: &impl Fn(&Path) -> bool,
) -> Option<&'a str> {
    if let Some(name) = names.clone().find(|name| actual == *name) {
        return Some(name);
    }
    let actual = actual.to_str()?;
    let mut folded = names
        .filter(|name| name.eq_ignore_ascii_case(actual))
        .peekable();
    folded.peek()?;
    if folds(directory) {
        folded.next()
    } else {
        None
    }
}

/// `path` as the loaders spell it: a source entry, guidance file or document
/// that a case-folding volume reports in another case is respelled with the
/// fixed name the loaders open, so routing and watch invalidation see the
/// source they read. Anything else is returned unchanged.
fn loader_path(
    path: &Path,
    user: &UserSourceRoots,
    workspaces: &[PathBuf],
    folds: &impl Fn(&Path) -> bool,
) -> PathBuf {
    let guidance = || crate::runtime::resources::PROJECT_CONTEXT_FILENAMES.into_iter();
    let resource_roots = std::iter::once(user.resources.clone())
        .chain(workspaces.iter().map(|workspace| workspace.join(".agents")));
    for root in resource_roots {
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let mut components = relative.components();
        let Some(entry) = components.next() else {
            return path.to_path_buf();
        };
        let rest = components.as_path();
        let names = SOURCE_ENTRIES.into_iter().chain(
            rest.as_os_str()
                .is_empty()
                .then(guidance)
                .into_iter()
                .flatten(),
        );
        if let Some(name) = loader_name(&root, entry.as_os_str(), names, folds) {
            return root.join(name).join(rest);
        }
    }
    let (Some(parent), Some(actual)) = (path.parent(), path.file_name()) else {
        return path.to_path_buf();
    };
    if Some(parent) == user.document.parent()
        && let Some(document) = user.document.file_name().and_then(|name| name.to_str())
        && let Some(name) = loader_name(parent, actual, std::iter::once(document), folds)
    {
        return parent.join(name);
    }
    if workspaces.iter().any(|workspace| workspace == parent)
        && let Some(name) = loader_name(
            parent,
            actual,
            std::iter::once("rustx.toml").chain(guidance()),
            folds,
        )
    {
        return parent.join(name);
    }
    path.to_path_buf()
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
            resources: self
                .configuration
                .sources
                .home_directory
                .join("rustx/.agents"),
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
                    let folded = self
                        .registry
                        .0
                        .lock()
                        .expect("registry mutex")
                        .source_watch_folds_case;
                    #[cfg(not(test))]
                    let folded = false;
                    let folds = |directory: &Path| folded || folds_case(directory);
                    let mut targets = Vec::new();
                    let mut next = Some(signal);
                    while let Some(signal) = next {
                        let owners = match signal {
                            Signal::Path(path) => {
                                let path = loader_path(&path, &user, &workspaces, &folds);
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
    /// in; the loaders open the fixed name and read it, so the watcher routes
    /// and invalidates it under that name, for removals too. On a
    /// case-sensitive volume the same spelling is not a source.
    #[test]
    fn a_case_variant_routes_only_where_the_volume_folds_case() {
        let home = Path::new("/home/u");
        let user = roots(home);
        let workspaces = vec![PathBuf::from("/w")];
        let workspace = Some(SourceTarget::Workspace {
            directory: "/w".into(),
        });
        let folding = |_: &Path| true;
        let sensitive = |_: &Path| false;
        let spelled = |path: &str, folds: &dyn Fn(&Path) -> bool| {
            loader_path(Path::new(path), &user, &workspaces, &folds)
        };
        for (actual, loader) in [
            (
                "/w/.agents/Skills/guide/SKILL.md",
                "/w/.agents/skills/guide/SKILL.md",
            ),
            ("/w/.agents/Skills", "/w/.agents/skills"),
            ("/w/.agents/MCP.toml", "/w/.agents/mcp.toml"),
            ("/w/.agents/claude.md", "/w/.agents/CLAUDE.md"),
            ("/w/RustX.toml", "/w/rustx.toml"),
            (
                "/home/u/rustx/.agents/Workflows/a.yaml",
                "/home/u/rustx/.agents/workflows/a.yaml",
            ),
            ("/home/u/config/RUSTX.toml", "/home/u/config/rustx.toml"),
        ] {
            let path = spelled(actual, &folding);
            assert_eq!(path, Path::new(loader));
            assert!(route(&path, &user, &workspaces).is_some(), "{actual}");
            let path = spelled(actual, &sensitive);
            assert_eq!(path, Path::new(actual));
            assert_eq!(route(&path, &user, &workspaces), None, "{actual}");
        }
        assert_eq!(
            route(
                &spelled("/w/.agents/Skills/x/SKILL.md", &folding),
                &user,
                &workspaces
            ),
            workspace
        );
        // Uploads stay unrouted in any case.
        assert_eq!(
            route(
                &spelled("/w/.agents/Uploads/s/AGENTS.md", &folding),
                &user,
                &workspaces
            ),
            None
        );
        // The exact spelling wins where both exist.
        assert_eq!(spelled("/w/AGENTS.MD", &folding), Path::new("/w/AGENTS.MD"));
        let mut active =
            BTreeMap::from([(PathBuf::from("/w/.agents/skills"), RecursiveMode::Recursive)]);
        invalidate(&mut active, &spelled("/w/.agents/Skills", &folding));
        assert!(
            active.is_empty(),
            "a replaced case-variant directory is re-registered"
        );
    }

    #[test]
    fn a_case_sensitive_volume_does_not_fold_case() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join(".agents");
        std::fs::create_dir_all(&root).unwrap();
        assert!(!folds_case(&root));
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

    /// On a case-insensitive volume a notification for `.agents/Skills`,
    /// which the loaders read through `.agents/skills`, routes to its
    /// Workspace.
    #[test]
    fn a_case_variant_the_loaders_read_routes_to_its_workspace() {
        let volume = crate::local_runtime::case_insensitive_volume::CaseInsensitiveVolume::new();
        let workspace = volume.path().join("w");
        let skill = workspace.join(".agents/Skills/guide/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(&skill, b"guide").unwrap();
        assert!(folds_case(&workspace.join(".agents")));
        let user = UserSourceRoots {
            document: volume.path().join("config/rustx.toml"),
            resources: volume.path().join("rustx/.agents"),
        };
        let workspaces = vec![workspace.clone()];
        let path = loader_path(&skill, &user, &workspaces, &folds_case);
        assert_eq!(
            route(&path, &user, &workspaces),
            Some(SourceTarget::Workspace {
                directory: workspace
            })
        );
    }
}
