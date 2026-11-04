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
                    let mut targets = Vec::new();
                    let mut next = Some(signal);
                    while let Some(signal) = next {
                        let owners = match signal {
                            Signal::Path(path) => {
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
