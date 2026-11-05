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
//! | Captured inputs | each other file a known Workspace's retained capture read or tried to read (a selected instruction file, for one, valid or not), as every directory entry opening it traverses — each component, each link and what it resolves to, up to the first missing entry — through each entry's canonical directory, non-recursively |
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
//! A captured input, or a directory holding it that was replaced, removed or
//! created, routes to every Workspace whose capture read it; the set is
//! exactly the captures' own source manifests, so nothing else under a
//! Workspace is watched. One physical directory takes one watch, since the
//! platform shares a registration between its spellings. Watches are
//! registered until the set settles before anything is captured, and an
//! input the watch has not yet seen — every input on its first pass — is
//! captured again once it is watched, so no edit falls between the two.
//!
//! A path belongs to every source target whose loaders read it, and only to
//! those: a known Workspace inside the User root, or inside another
//! Workspace's `.agents`, shares a source the other reads instead of taking
//! it, so its User consumers in every cwd are still captured.
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

/// The components of `path` beneath `base`, matching each component of
/// `base` with the path's own by [`same`](same_entry) entry: a base the volume
/// reports in another case still contains the path, while `base` keeps its
/// own canonical spelling.
fn beneath<'p>(
    path: &'p Path,
    base: &Path,
    same: &impl Fn(&Path, &Path) -> bool,
) -> Option<std::path::Components<'p>> {
    let mut components = path.components();
    let mut bound = PathBuf::new();
    for expected in base.components() {
        let actual = components.next()?;
        if actual != expected {
            let (Some(actual), Some(name)) =
                (actual.as_os_str().to_str(), expected.as_os_str().to_str())
            else {
                return None;
            };
            if !name.eq_ignore_ascii_case(actual) || !same(&bound.join(actual), &bound.join(name)) {
                return None;
            }
        }
        bound.push(expected);
    }
    Some(components)
}

/// Every source target whose loaders read `path`, each with `path` as that
/// owner opens it. Each owner — the User and each known Workspace, through a
/// resource root or a document — is matched [`beneath`] its own canonical
/// base and through the fixed names its loaders open against the reported
/// path itself, then respelled under that base, so one owner's spelling
/// never changes what another sees. A path several owners read, such as a
/// User Skill inside a known Workspace, belongs to each of them: registering a
/// directory as a Workspace never ends another source's consumers.
fn owners(
    path: &Path,
    user: &UserSourceRoots,
    workspaces: &[PathBuf],
    same: &impl Fn(&Path, &Path) -> bool,
) -> Vec<(SourceTarget, PathBuf)> {
    let guidance = || crate::runtime::resources::PROJECT_CONTEXT_FILENAMES.into_iter();
    let workspace = |directory: &Path| SourceTarget::Workspace {
        directory: directory.to_path_buf(),
    };
    let mut owners = Vec::new();
    let resource_roots = std::iter::once((
        SourceTarget::User,
        user.resources.ancestors().nth(USER_RESOURCES.len()),
        &USER_RESOURCES[..],
    ))
    .chain(workspaces.iter().map(|directory| {
        (
            workspace(directory),
            Some(directory.as_path()),
            &[".agents"][..],
        )
    }));
    for (target, base, fixed) in resource_roots {
        let Some(base) = base else {
            continue;
        };
        let Some(mut components) = beneath(path, base, same) else {
            continue;
        };
        let mut root = base.to_path_buf();
        let mut fixed = fixed.iter();
        let within = fixed.all(|name| {
            components
                .next()
                .and_then(|actual| {
                    loader_name(&root, actual.as_os_str(), std::iter::once(*name), same)
                })
                .map(|name| root.push(name))
                .is_some()
        });
        if !within {
            continue;
        }
        let Some(entry) = components.next() else {
            owners.push((target, root));
            continue;
        };
        let rest = components.as_path();
        let names = SOURCE_ENTRIES.into_iter().chain(
            rest.as_os_str()
                .is_empty()
                .then(guidance)
                .into_iter()
                .flatten(),
        );
        if let Some(name) = loader_name(&root, entry.as_os_str(), names, same) {
            owners.push((target, root.join(name).join(rest)));
        }
    }
    let document = |directory: &Path| {
        let mut components = beneath(path, directory, same)?;
        let file = components.next()?;
        components.next().is_none().then_some(file.as_os_str())
    };
    if let Some(directory) = user.document.parent()
        && let Some(file) = document(directory)
        && let Some(fixed) = user.document.file_name().and_then(|name| name.to_str())
        && let Some(name) = loader_name(directory, file, std::iter::once(fixed), same)
    {
        owners.push((SourceTarget::User, directory.join(name)));
    }
    for directory in workspaces {
        let Some(file) = document(directory) else {
            continue;
        };
        // The document, the guidance files, and any root `*.md` as listed.
        let name = loader_name(
            directory,
            file,
            std::iter::once("rustx.toml").chain(guidance()),
            same,
        )
        .map(std::ffi::OsStr::new)
        .or_else(|| {
            Path::new(file)
                .extension()
                .is_some_and(|extension| extension == "md")
                .then_some(file)
        });
        if let Some(name) = name {
            owners.push((workspace(directory), directory.join(name)));
        }
    }
    owners
}

/// The Workspaces whose retained capture read an input file at or beneath
/// `path`, matched by the same identity rule, each with the file as it reads
/// it: the file itself, or a directory holding it that was replaced, removed
/// or created.
fn input_owners<'a>(
    path: &'a Path,
    inputs: &'a [(PathBuf, PathBuf)],
    same: &'a impl Fn(&Path, &Path) -> bool,
) -> impl Iterator<Item = (SourceTarget, PathBuf)> + 'a {
    inputs
        .iter()
        .filter(move |(_, file)| beneath(file, path, same).is_some())
        .map(|(directory, file)| {
            (
                SourceTarget::Workspace {
                    directory: directory.clone(),
                },
                file.clone(),
            )
        })
}

/// The source targets a notification at `path` invalidates: its owners, and
/// the Workspaces whose capture read it as an input. The watches at or below
/// it, as reported and as each owner opens it, are registered again.
fn notified(
    path: &Path,
    sources: &KnownSources,
    same: &impl Fn(&Path, &Path) -> bool,
    active: &mut BTreeMap<PathBuf, RecursiveMode>,
) -> Vec<SourceTarget> {
    invalidate(active, path);
    // Inputs are spelled under canonical directories; a watch registered
    // under another spelling of the same directory reports that spelling.
    let canonical = path
        .parent()
        .zip(path.file_name())
        .and_then(|(parent, name)| Some(std::fs::canonicalize(parent).ok()?.join(name)))
        .filter(|canonical| canonical != path);
    let mut targets = Vec::new();
    for (target, path) in owners(path, &sources.user, &sources.workspaces, same)
        .into_iter()
        .chain(input_owners(path, &sources.inputs, same))
        .chain(
            canonical
                .iter()
                .flat_map(|canonical| input_owners(canonical, &sources.inputs, same)),
        )
    {
        invalidate(active, &path);
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

/// Every directory entry a loader opening `file` traverses, each spelled
/// under its canonical directory: each component beneath `base` (the
/// Workspace that reads it, or the file's own directory), each link on the
/// way and what it resolves to, up to and including the first entry that
/// does not exist. Watching each entry's directory reports any of them
/// replaced, re-pointed, removed or created; a link and its target are
/// separate entries, so neither hides the other.
fn resolution(base: &Path, file: &Path) -> Vec<PathBuf> {
    use std::collections::VecDeque;
    use std::path::Component;
    let (base, rest) = match file.strip_prefix(base) {
        Ok(rest) => (base, rest),
        Err(_) => match file.parent().zip(file.file_name()) {
            Some((parent, name)) => (parent, Path::new(name)),
            None => return Vec::new(),
        },
    };
    let Ok(mut directory) = std::fs::canonicalize(base) else {
        return vec![file.to_path_buf()];
    };
    let mut pending: VecDeque<_> = rest
        .components()
        .map(Component::as_os_str)
        .map(ToOwned::to_owned)
        .collect();
    let mut entries = Vec::new();
    let mut links = 0;
    while let Some(name) = pending.pop_front() {
        if name == ".." {
            directory.pop();
            continue;
        }
        if name == "." {
            continue;
        }
        let entry = directory.join(&name);
        entries.push(entry.clone());
        let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
            break;
        };
        if metadata.file_type().is_symlink() {
            links += 1;
            let Some(target) = std::fs::read_link(&entry).ok().filter(|_| links <= 40) else {
                break;
            };
            let mut spliced: VecDeque<_> = if target.is_absolute() {
                // Resolved from its nearest existing ancestor, which is
                // canonical once it exists.
                let Some((existing, canonical)) = target
                    .ancestors()
                    .find_map(|ancestor| Some((ancestor, std::fs::canonicalize(ancestor).ok()?)))
                else {
                    break;
                };
                directory = canonical;
                target
                    .strip_prefix(existing)
                    .unwrap_or(Path::new(""))
                    .components()
                    .map(|component| component.as_os_str().to_owned())
                    .collect()
            } else {
                target
                    .components()
                    .map(|component| component.as_os_str().to_owned())
                    .collect()
            };
            spliced.extend(pending);
            pending = spliced;
        } else if metadata.is_dir() {
            directory = entry;
        }
    }
    entries
}

/// Each reader's input files as the [`resolution`] entries a loader opening
/// them traverses.
fn resolved(inputs: Vec<(PathBuf, PathBuf)>) -> Vec<(PathBuf, PathBuf)> {
    let mut entries: Vec<_> = inputs
        .into_iter()
        .flat_map(|(directory, file)| {
            resolution(&directory, &file)
                .into_iter()
                .map(move |entry| (directory.clone(), entry))
        })
        .collect();
    entries.sort();
    entries.dedup();
    entries
}

/// The sources one pass of the watch observes.
pub(super) struct KnownSources {
    pub(super) user: UserSourceRoots,
    pub(super) workspaces: Vec<PathBuf>,
    /// Each known Workspace with a file its retained capture read.
    pub(super) inputs: Vec<(PathBuf, PathBuf)>,
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

/// The exact watch set for these sources: existing directories only. An
/// input file outside them is watched through its directory, which also
/// reports the file replaced in place.
pub(super) fn desired(sources: &KnownSources) -> BTreeMap<PathBuf, RecursiveMode> {
    let KnownSources {
        user,
        workspaces,
        inputs,
    } = sources;
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
    // One physical directory takes one watch: the platform shares a single
    // registration between its spellings, so removing one would end the
    // others. A fixed watch keeps its own spelling.
    let identity = |path: &Path| {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path)
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()))
    };
    let mut identities: std::collections::BTreeSet<_> =
        watches.keys().filter_map(|path| identity(path)).collect();
    // An input whose directory is gone is watched through its nearest
    // existing ancestor, which reports the directory coming back.
    for directory in inputs
        .iter()
        .filter_map(|(_, file)| file.ancestors().skip(1).find(|path| path.is_dir()))
    {
        let covered = watches.iter().any(|(watched, mode)| {
            watched == directory
                || (*mode == RecursiveMode::Recursive && directory.starts_with(watched))
        });
        if !covered && identity(directory).is_none_or(|identity| identities.insert(identity)) {
            watches.insert(directory.to_path_buf(), RecursiveMode::NonRecursive);
        }
    }
    watches.retain(|path, _| path.is_dir());
    watches
}

/// Register the watches for `sources` until the set no longer changes, so a
/// directory created while its parent's watch was being registered is
/// watched too before anything beneath it is read.
fn settle(
    watcher: &mut dyn Watcher,
    active: &mut BTreeMap<PathBuf, RecursiveMode>,
    sources: &KnownSources,
) {
    let mut registered = desired(sources);
    loop {
        rearm(watcher, active, &registered);
        let now = desired(sources);
        if now == registered {
            return;
        }
        registered = now;
    }
}

/// Converge the registered watches on `desired`; only a watch that
/// registered is active.
fn rearm(
    watcher: &mut dyn Watcher,
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

    fn known_sources(&self) -> KnownSources {
        let (workspaces, inputs) = {
            let application = self.applications.lock();
            (
                application.watched_source_directories(),
                application.source_inputs(),
            )
        };
        KnownSources {
            user: self.user_source_roots(),
            workspaces,
            inputs: resolved(inputs),
        }
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

    /// The platform watcher that forwards notifications to `sender`, or,
    /// where a test turned platform watches off, one that registers nothing.
    #[cfg_attr(not(test), allow(clippy::unused_self))] // the switch is a test hook
    fn platform_watcher(
        &self,
        sender: tokio::sync::mpsc::UnboundedSender<Signal>,
    ) -> Result<Box<dyn Watcher + Send>, String> {
        #[cfg(test)]
        let platform = self
            .registry
            .0
            .lock()
            .expect("registry mutex")
            .source_watch_platform;
        #[cfg(not(test))]
        let platform = true;
        Ok(if platform {
            Box::new(
                notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                    forward(result, &sender);
                })
                .map_err(|error| error.to_string())?,
            )
        } else {
            Box::new(notify::NullWatcher)
        })
    }

    /// Capture again each Workspace with an input `known` did not hold, now
    /// that every input is watched, and remember the inputs. Returns whether
    /// anything was captured.
    async fn capture_new_inputs(
        &self,
        known: &mut Vec<(PathBuf, PathBuf)>,
        sources: &KnownSources,
    ) -> bool {
        let mut appeared = Vec::new();
        for (directory, _) in sources.inputs.iter().filter(|input| !known.contains(input)) {
            let target = SourceTarget::Workspace {
                directory: directory.clone(),
            };
            if !appeared.contains(&target) {
                appeared.push(target);
            }
        }
        known.clone_from(&sources.inputs);
        for target in &appeared {
            self.observe_source_change(target).await;
        }
        !appeared.is_empty()
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
        let mut watcher = self.platform_watcher(sender)?;
        let mut changes = self.configuration_changes();
        let mut active = BTreeMap::<PathBuf, RecursiveMode>::new();
        // Nothing is watched before the first pass, so every input the first
        // pass finds is new: an edit after the capture that read it and
        // before its watch is captured again like any other.
        let mut known: Vec<(PathBuf, PathBuf)> = Vec::new();
        loop {
            let sources = self.known_sources();
            settle(watcher.as_mut(), &mut active, &sources);
            // An input that just appeared — a newly selected instruction
            // file, a link's new target — may have changed between the
            // capture that found it and its watch: capture its readers again
            // now that it is watched. Unchanged sources stay no-ops.
            if self.capture_new_inputs(&mut known, &sources).await {
                continue;
            }
            #[cfg(test)]
            self.registry
                .0
                .lock()
                .expect("registry mutex")
                .source_watch_settled
                .send_modify(|passes| *passes += 1);
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
                                notified(&path, &sources, &same, &mut active)
                            }
                            Signal::Rescan => {
                                active.clear();
                                std::iter::once(SourceTarget::User)
                                    .chain(sources.workspaces.iter().map(|directory| {
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
                    settle(watcher.as_mut(), &mut active, &sources);
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

    fn known(user: &UserSourceRoots, workspaces: &[PathBuf]) -> KnownSources {
        KnownSources {
            user: user.clone(),
            workspaces: workspaces.to_vec(),
            inputs: Vec::new(),
        }
    }

    /// The targets that read `path` as reported, where no other spelling is
    /// the same entry.
    fn route(path: &Path, user: &UserSourceRoots, workspaces: &[PathBuf]) -> Vec<SourceTarget> {
        owners(path, user, workspaces, &|_: &Path, _: &Path| false)
            .into_iter()
            .map(|(target, _)| target)
            .collect()
    }

    #[test]
    fn notifications_route_only_supported_sources_to_their_owner() {
        let home = Path::new("/home/u");
        let user = roots(home);
        let workspaces = vec![PathBuf::from("/work/a"), PathBuf::from("/work/a/nested")];
        let workspace = |directory: &str| {
            vec![SourceTarget::Workspace {
                directory: directory.into(),
            }]
        };
        assert_eq!(
            route(&home.join("config/rustx.toml"), &user, &workspaces),
            vec![SourceTarget::User]
        );
        assert_eq!(
            route(&home.join("config/other.toml"), &user, &workspaces),
            Vec::<SourceTarget>::new()
        );
        assert_eq!(
            route(
                &home.join("rustx/.agents/skills/x/SKILL.md"),
                &user,
                &workspaces
            ),
            vec![SourceTarget::User]
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
        // A nested Workspace's document is not its parent's source.
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
            Vec::<SourceTarget>::new()
        );
        assert_eq!(
            route(&home.join("rustx/.agents/uploads/x"), &user, &workspaces),
            Vec::<SourceTarget>::new()
        );
        assert_eq!(
            route(&home.join("rustx/.agents"), &user, &workspaces),
            vec![SourceTarget::User]
        );
        // Every guidance file the loader reads, at either root.
        for name in ["AGENTS.override.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"] {
            assert_eq!(
                route(&Path::new("/work/a").join(name), &user, &workspaces),
                workspace("/work/a")
            );
            assert_eq!(
                route(&home.join("rustx/.agents").join(name), &user, &workspaces),
                vec![SourceTarget::User]
            );
        }
        assert_eq!(
            route(&home.join("rustx/.agents/notes.md"), &user, &workspaces),
            Vec::<SourceTarget>::new()
        );
        assert_eq!(
            route(
                &home.join("rustx/.agents/uploads/AGENTS.md"),
                &user,
                &workspaces
            ),
            Vec::<SourceTarget>::new()
        );
        // Ordinary project files never reach configuration.
        assert_eq!(
            route(Path::new("/work/a/src/main.rs"), &user, &workspaces),
            Vec::<SourceTarget>::new()
        );
        assert_eq!(
            route(Path::new("/work/a/docs/notes.md"), &user, &workspaces),
            Vec::<SourceTarget>::new()
        );
        assert_eq!(
            route(Path::new("/elsewhere/rustx.toml"), &user, &workspaces),
            Vec::<SourceTarget>::new()
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
            owners(Path::new(path), &user, &workspaces, &same)
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
            assert_eq!(
                spelled(actual, &aliases),
                [(
                    SourceTarget::Workspace {
                        directory: owner.into()
                    },
                    PathBuf::from(loader)
                )],
                "{actual}"
            );
            assert!(spelled(actual, &distinct).is_empty(), "{actual}");
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
            assert_eq!(
                spelled(actual, &aliases),
                [(SourceTarget::User, PathBuf::from(loader))]
            );
            assert!(spelled(actual, &distinct).is_empty(), "{actual}");
        }
        // Uploads stay unrouted in any case.
        assert!(spelled("/w/.Agents/Uploads/s/AGENTS.md", &aliases).is_empty());
        // The exact spelling wins where both exist.
        assert_eq!(
            spelled("/w/AGENTS.MD", &aliases),
            [(
                SourceTarget::Workspace {
                    directory: "/w".into()
                },
                PathBuf::from("/w/AGENTS.MD")
            )]
        );
    }

    /// A replaced case-variant directory re-registers the watches beneath the
    /// loaders' spelling.
    #[test]
    fn a_case_variant_notification_reregisters_the_loaders_watches() {
        let user = roots(Path::new("/home/u"));
        let workspaces = vec![PathBuf::from("/w")];
        let mut active = BTreeMap::from([
            (PathBuf::from("/w"), RecursiveMode::NonRecursive),
            (PathBuf::from("/w/.agents"), RecursiveMode::NonRecursive),
            (PathBuf::from("/w/.agents/skills"), RecursiveMode::Recursive),
        ]);
        let notify = |path: &str, active: &mut BTreeMap<PathBuf, RecursiveMode>| {
            notified(
                Path::new(path),
                &known(&user, &workspaces),
                &|_, _| true,
                active,
            )
        };
        notify("/w/.agents/Skills", &mut active);
        assert_eq!(active.len(), 2);
        notify("/w/.Agents", &mut active);
        assert_eq!(active.keys().collect::<Vec<_>>(), [Path::new("/w")]);
    }

    /// An owner keeps its own canonical spelling: matching another source's
    /// fixed names — the User root, an outer Workspace's `.agents` — never
    /// rewrites the path a Workspace whose canonical path keeps `RustX` or
    /// `.Agents` is matched with.
    #[test]
    fn an_owner_keeps_its_canonical_spelling() {
        let home = Path::new("/home/u");
        let user = roots(home);
        let workspaces = vec![
            PathBuf::from("/w"),
            PathBuf::from("/home/u/RustX/.agents/project"),
            PathBuf::from("/w/.Agents/inner"),
        ];
        let aliases = |_: &Path, _: &Path| true;
        let distinct = |_: &Path, _: &Path| false;
        let spelled = |path: &str, same: &dyn Fn(&Path, &Path) -> bool| {
            owners(Path::new(path), &user, &workspaces, &same)
        };
        for (actual, loader, owner, exact) in [
            (
                "/home/u/RustX/.agents/project/rustx.toml",
                "/home/u/RustX/.agents/project/rustx.toml",
                "/home/u/RustX/.agents/project",
                true,
            ),
            (
                "/w/.Agents/inner/rustx.toml",
                "/w/.Agents/inner/rustx.toml",
                "/w/.Agents/inner",
                true,
            ),
            (
                "/w/.agents/inner/RustX.toml",
                "/w/.Agents/inner/rustx.toml",
                "/w/.Agents/inner",
                false,
            ),
        ] {
            let owner = [(
                SourceTarget::Workspace {
                    directory: owner.into(),
                },
                PathBuf::from(loader),
            )];
            assert_eq!(spelled(actual, &aliases), owner, "{actual}");
            let distinct = spelled(actual, &distinct);
            if exact {
                assert_eq!(distinct, owner, "{actual}");
            } else {
                assert!(distinct.is_empty(), "{actual}");
            }
        }
    }

    /// A source belongs to every owner whose loaders read it: a known
    /// Workspace inside the User root, or inside another Workspace's
    /// `.agents`, never takes a User Agent or Skill, or that Workspace's
    /// guidance, from the owner that reads it, whatever the spelling.
    #[test]
    fn a_source_belongs_to_every_owner_that_reads_it() {
        let user = roots(Path::new("/home/u"));
        let workspaces = vec![
            PathBuf::from("/home/u/RustX/.agents/agents"),
            PathBuf::from("/home/u/RustX/.agents/skills/guide"),
            PathBuf::from("/w"),
            PathBuf::from("/w/.agents"),
        ];
        let workspace = |directory: &str, path: &str| {
            (
                SourceTarget::Workspace {
                    directory: directory.into(),
                },
                PathBuf::from(path),
            )
        };
        let aliases = |_: &Path, _: &Path| true;
        assert_eq!(
            owners(
                Path::new("/home/u/RustX/.agents/agents/reviewer.toml"),
                &user,
                &workspaces,
                &aliases
            ),
            [(
                SourceTarget::User,
                PathBuf::from("/home/u/rustx/.agents/agents/reviewer.toml")
            )]
        );
        assert_eq!(
            owners(
                Path::new("/home/u/RustX/.agents/skills/guide/SKILL.md"),
                &user,
                &workspaces,
                &aliases
            ),
            [
                (
                    SourceTarget::User,
                    PathBuf::from("/home/u/rustx/.agents/skills/guide/SKILL.md")
                ),
                workspace(
                    "/home/u/RustX/.agents/skills/guide",
                    "/home/u/RustX/.agents/skills/guide/SKILL.md"
                ),
            ]
        );
        assert_eq!(
            route(Path::new("/w/.agents/AGENTS.md"), &user, &workspaces),
            [
                workspace("/w", "/w/.agents/AGENTS.md").0,
                workspace("/w/.agents", "/w/.agents/AGENTS.md").0,
            ]
        );
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
            assert!(
                owners(&actual, &user, &workspaces, &same_entry).is_empty(),
                "{actual:?}"
            );
        }
        assert_eq!(
            owners(
                &workspace.join(".agents/Tools"),
                &user,
                &workspaces,
                &same_entry
            ),
            [(
                SourceTarget::Workspace {
                    directory: workspace.clone()
                },
                workspace.join(".agents/tools")
            )]
        );
    }

    /// A file a Workspace's capture read outside its fixed sources — a
    /// selected instruction file — is watched through its directory, unless a
    /// fixed watch already covers it, and routes to every Workspace that read
    /// it; a sibling no capture read routes nowhere.
    #[test]
    fn a_captured_input_file_is_watched_and_routed_to_its_readers() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("w");
        let other = root.path().join("v");
        for directory in ["w/docs", "w/.agents/skills/x", "v"] {
            std::fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        let policy = workspace.join("docs/policy.md");
        let sources = KnownSources {
            user: roots(&root.path().join("home")),
            workspaces: vec![workspace.clone(), other.clone()],
            inputs: vec![
                (other.clone(), policy.clone()),
                (workspace.clone(), policy.clone()),
                (
                    workspace.clone(),
                    workspace.join(".agents/skills/x/SKILL.md"),
                ),
            ],
        };
        let watches = desired(&sources);
        assert_eq!(
            watches.get(&workspace.join("docs")),
            Some(&RecursiveMode::NonRecursive)
        );
        assert!(!watches.contains_key(&workspace.join(".agents/skills/x")));
        let mut active = BTreeMap::new();
        let workspace_target = |directory: &Path| SourceTarget::Workspace {
            directory: directory.to_path_buf(),
        };
        assert_eq!(
            notified(&policy, &sources, &|_, _| false, &mut active),
            [workspace_target(&other), workspace_target(&workspace)]
        );
        assert!(
            notified(
                &workspace.join("docs/other.md"),
                &sources,
                &|_, _| false,
                &mut active
            )
            .is_empty()
        );
    }

    /// A directory holding a captured input that is replaced, removed or
    /// created routes to the input's readers; an input whose directory is
    /// gone is watched through its nearest existing ancestor.
    #[test]
    fn a_captured_input_directory_lifecycle_stays_observed() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("w");
        std::fs::create_dir_all(workspace.join("docs")).unwrap();
        let sources = KnownSources {
            user: roots(&root.path().join("home")),
            workspaces: vec![workspace.clone()],
            inputs: vec![
                (workspace.clone(), workspace.join("docs/policy.md")),
                (
                    workspace.clone(),
                    workspace.join("docs/policies/deep/policy.md"),
                ),
            ],
        };
        let target = [SourceTarget::Workspace {
            directory: workspace.clone(),
        }];
        let mut active = BTreeMap::new();
        for directory in ["docs", "docs/policies", "docs/policies/deep"] {
            assert_eq!(
                notified(
                    &workspace.join(directory),
                    &sources,
                    &|_, _| false,
                    &mut active
                ),
                target,
                "{directory}"
            );
        }
        let watches = desired(&sources);
        assert_eq!(
            watches.get(&workspace.join("docs")),
            Some(&RecursiveMode::NonRecursive)
        );
        assert!(!watches.contains_key(&workspace.join("docs/policies")));
        std::fs::create_dir_all(workspace.join("docs/policies/deep")).unwrap();
        assert_eq!(
            desired(&sources).get(&workspace.join("docs/policies/deep")),
            Some(&RecursiveMode::NonRecursive)
        );
    }

    /// A watcher that creates a directory beneath the first one it watches,
    /// as a recreation racing the registration would.
    struct Racing {
        create: Option<PathBuf>,
        watched: Vec<PathBuf>,
    }

    impl Watcher for Racing {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            unreachable!()
        }
        fn watch(&mut self, path: &Path, _: RecursiveMode) -> notify::Result<()> {
            if let Some(directory) = self.create.take() {
                std::fs::create_dir_all(directory).unwrap();
            }
            self.watched.push(path.to_path_buf());
            Ok(())
        }
        fn unwatch(&mut self, _: &Path) -> notify::Result<()> {
            Ok(())
        }
        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    /// A directory created while its parent's watch is registered is watched
    /// before anything beneath it is read.
    #[test]
    fn the_watch_set_settles_on_directories_created_while_registering() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("w");
        std::fs::create_dir_all(workspace.join("docs")).unwrap();
        let sources = KnownSources {
            user: roots(&root.path().join("home")),
            workspaces: vec![workspace.clone()],
            inputs: vec![(workspace.clone(), workspace.join("docs/policies/policy.md"))],
        };
        let mut watcher = Racing {
            create: Some(workspace.join("docs/policies")),
            watched: Vec::new(),
        };
        let mut active = BTreeMap::new();
        settle(&mut watcher, &mut active, &sources);
        assert!(active.contains_key(&workspace.join("docs/policies")));
        assert!(watcher.watched.contains(&workspace.join("docs/policies")));
    }

    /// A linked input's target is an input too: its directory is watched and
    /// an edit of the target routes to the link's readers.
    #[test]
    fn a_linked_input_is_observed_through_its_target() {
        let root = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(root.path()).unwrap().join("w");
        std::fs::create_dir_all(workspace.join("docs")).unwrap();
        std::fs::create_dir_all(workspace.join("policies")).unwrap();
        std::fs::write(workspace.join("policies/main.md"), "policy").unwrap();
        std::os::unix::fs::symlink("../policies/main.md", workspace.join("docs/policy.md"))
            .unwrap();
        let sources = KnownSources {
            user: roots(&root.path().join("home")),
            workspaces: vec![workspace.clone()],
            inputs: resolved(vec![(workspace.clone(), workspace.join("docs/policy.md"))]),
        };
        assert_eq!(
            desired(&sources).get(&workspace.join("policies")),
            Some(&RecursiveMode::NonRecursive)
        );
        assert_eq!(
            notified(
                &workspace.join("policies/main.md"),
                &sources,
                &|_, _| false,
                &mut BTreeMap::new()
            ),
            [SourceTarget::Workspace {
                directory: workspace.clone()
            }]
        );
    }

    /// Every link on the way to an input is an entry of its own, and a
    /// target that is gone stays one: re-pointing an intermediate link, or
    /// restoring the target, is reported by a watched directory and routes to
    /// the reader.
    #[test]
    fn a_link_chain_is_observed_link_by_link() {
        let root = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(root.path()).unwrap().join("w");
        for directory in ["docs", "links", "policies"] {
            std::fs::create_dir_all(workspace.join(directory)).unwrap();
        }
        std::fs::write(workspace.join("policies/a.md"), "a").unwrap();
        std::os::unix::fs::symlink("../links/current", workspace.join("docs/policy.md")).unwrap();
        std::os::unix::fs::symlink("../policies/a.md", workspace.join("links/current")).unwrap();
        let sources = || KnownSources {
            user: roots(&root.path().join("home")),
            workspaces: vec![workspace.clone()],
            inputs: resolved(vec![(workspace.clone(), workspace.join("docs/policy.md"))]),
        };
        let reader = [SourceTarget::Workspace {
            directory: workspace.clone(),
        }];
        let watches = desired(&sources());
        for directory in ["docs", "links", "policies"] {
            assert!(
                watches.contains_key(&workspace.join(directory)),
                "{directory}"
            );
        }
        let routed = |path: &str| {
            notified(
                &workspace.join(path),
                &sources(),
                &|_, _| false,
                &mut BTreeMap::new(),
            )
        };
        assert_eq!(routed("links/current"), reader);
        assert_eq!(routed("policies/a.md"), reader);
        std::fs::remove_file(workspace.join("policies/a.md")).unwrap();
        assert!(desired(&sources()).contains_key(&workspace.join("policies")));
        assert_eq!(routed("policies/a.md"), reader);
        assert!(routed("policies/b.md").is_empty());
    }

    /// A directory reached through a link takes one watch, under its
    /// canonical path: when the link stops being an input, the platform
    /// watch the remaining input needs still reports its edits.
    #[test]
    fn a_directory_reached_through_a_link_keeps_its_one_watch() {
        let root = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(root.path()).unwrap().join("w");
        std::fs::create_dir_all(workspace.join("policies")).unwrap();
        std::fs::write(workspace.join("policies/a.md"), "a").unwrap();
        std::fs::write(workspace.join("policies/b.md"), "b").unwrap();
        std::os::unix::fs::symlink("policies", workspace.join("docs")).unwrap();
        let sources = |file: &str| KnownSources {
            user: roots(&root.path().join("home")),
            workspaces: vec![workspace.clone()],
            inputs: resolved(vec![(workspace.clone(), workspace.join(file))]),
        };
        assert!(!desired(&sources("docs/a.md")).contains_key(&workspace.join("docs")));
        let (sender, events) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(sender).unwrap();
        let mut active = BTreeMap::new();
        settle(&mut watcher, &mut active, &sources("docs/a.md"));
        settle(&mut watcher, &mut active, &sources("policies/b.md"));
        std::fs::write(workspace.join("policies/b.md"), "edited").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let event: notify::Event = events
                .recv_timeout(remaining)
                .expect("the remaining watch reports the edit")
                .unwrap();
            if event.paths.iter().any(|path| path.ends_with("b.md")) {
                break;
            }
        }
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
        let watches = desired(&known(&user, &workspaces));
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
        let watches = desired(&known(&user, &workspaces));
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
    /// routes to its owner when notified under its own spelling; Workspaces
    /// whose canonical paths keep `RustX` or `.Agents` keep owning their
    /// documents; a renamed-away root invalidates the watches beneath the
    /// loaders' spelling.
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
            assert_eq!(
                owners(actual, &user, &workspaces, &same_entry),
                [(
                    SourceTarget::Workspace {
                        directory: (*workspace).clone()
                    },
                    loader.clone()
                )]
            );
        }
        // A User Skill under `RustX`, which the loaders read through
        // `rustx/.agents/skills`.
        let skill = home.join("RustX/.agents/skills/guide/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(&skill, b"user").unwrap();
        let loader = user.resources.join("skills/guide/SKILL.md");
        assert_eq!(std::fs::read(&loader).unwrap(), b"user");
        assert_eq!(
            owners(&skill, &user, &workspaces, &same_entry),
            [(SourceTarget::User, loader)]
        );
        // Workspaces whose canonical paths keep `RustX` and `.Agents` stay
        // the owners of their own documents, notified under that spelling or
        // the loaders'.
        let canonical = |path: &Path| {
            std::fs::create_dir_all(path).unwrap();
            crate::local_runtime::configuration::capture::canonical_directory(path).unwrap()
        };
        let canonical_owners = [
            canonical(&home.join("RustX/.agents/project")),
            canonical(&digits.join(".Agents/inner")),
        ];
        let workspaces: Vec<_> = workspaces
            .iter()
            .chain(&canonical_owners)
            .cloned()
            .collect();
        for owner in &canonical_owners {
            std::fs::write(owner.join("rustx.toml"), b"workspace").unwrap();
            for actual in [owner.join("rustx.toml"), owner.join("RustX.toml")] {
                assert_eq!(
                    owners(&actual, &user, &workspaces, &same_entry),
                    [(
                        SourceTarget::Workspace {
                            directory: owner.clone()
                        },
                        owner.join("rustx.toml")
                    )],
                    "{actual:?}"
                );
            }
        }
        std::fs::rename(root.join(".Agents"), root.join("old")).unwrap();
        let mut active = BTreeMap::from([
            (root.join(".agents"), RecursiveMode::NonRecursive),
            (root.join(".agents/skills"), RecursiveMode::Recursive),
        ]);
        let sources = KnownSources {
            user,
            workspaces,
            inputs: Vec::new(),
        };
        notified(&root.join(".Agents"), &sources, &same_entry, &mut active);
        assert!(active.is_empty());
    }
}
