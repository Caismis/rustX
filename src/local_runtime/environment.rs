//! Instance-owned native environment authority, shared by every trusted client.
//! Grants come only from User/Host process policy, never a request cwd or Thread.
pub mod files;

use crate::package_io::PackageDirectory;
use crate::runtime::CancellationSignal;
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Every configured grant, under one lock: the logical membership that
/// authorizes a path, and the physical identity pins that legacy Server-side
/// file APIs resolve through. A grant without a pin is denied — it is never
/// answered by an enclosing grant's pin.
#[derive(Clone, Default)]
struct Grants {
    /// The live logical grants. Removal cancels the signal; an unchanged grant
    /// keeps it; a regrant gets a fresh one.
    policies: BTreeMap<PathBuf, CancellationSignal>,
    physical: BTreeMap<PathBuf, Arc<Root>>,
}

#[cfg_attr(not(target_os = "macos"), derive(Default))]
pub(crate) struct Environment {
    io_gate: std::sync::Mutex<()>,
    roots: RwLock<Grants>,
    #[cfg(target_os = "macos")]
    pub(crate) watch_files: Arc<tokio::sync::Semaphore>,
}
#[cfg(target_os = "macos")]
impl Default for Environment {
    fn default() -> Self {
        Self {
            io_gate: std::sync::Mutex::default(),
            roots: RwLock::default(),
            #[cfg(target_os = "macos")]
            watch_files: Arc::new(tokio::sync::Semaphore::new(1024)),
        }
    }
}

pub(crate) struct Root {
    directory: PackageDirectory,
    identity: (u64, u64),
    pub(crate) cancelled: CancellationSignal,
}

impl Environment {
    /// Publishes the configured grants: removed roots are cancelled and retired
    /// before replacements are accepted. A grant is logical and independent of
    /// the physical pin — a root that cannot be opened stays granted and simply
    /// has no pin, so it resolves to nothing rather than to an enclosing grant.
    pub(crate) fn configure(&self, paths: &[PathBuf]) {
        let mut grants = self.roots.write().expect("environment roots");
        let Grants { policies, physical } = &mut *grants;
        policies.retain(|path, signal| {
            let keep = paths.contains(path);
            if !keep {
                signal.cancel();
            }
            keep
        });
        physical.retain(|path, _| paths.contains(path));
        for path in paths {
            let signal = policies
                .entry(path.clone())
                .or_insert_with(CancellationSignal::new)
                .clone();
            if physical.contains_key(path) {
                continue;
            }
            if let Ok(directory) = PackageDirectory::open(path)
                && let Ok(file) = directory.descriptor().try_clone()
                && let Ok(metadata) = std::fs::File::from(file).metadata()
            {
                physical.insert(
                    path.clone(),
                    Arc::new(Root {
                        directory,
                        identity: (metadata.dev(), metadata.ino()),
                        cancelled: signal,
                    }),
                );
            }
        }
    }

    pub(crate) fn with_paths<T>(
        &self,
        paths: &[&str],
        operation: impl FnOnce(&[(PathBuf, Arc<Root>, PathBuf)]) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let _io = self.io_gate.lock().expect("environment I/O");
        let grants = self.roots.read().expect("environment roots").clone();
        Self::resolve(paths, &grants, operation)
    }

    pub(crate) fn try_metadata(&self, path: &str) -> std::io::Result<files::FileMetadata> {
        let busy = || std::io::Error::from(std::io::ErrorKind::WouldBlock);
        let _io = self.io_gate.try_lock().map_err(|_| busy())?;
        let grants = self.roots.try_read().map_err(|_| busy())?.clone();
        Self::resolve(&[path], &grants, |paths| {
            files::metadata(&paths[0].1, &paths[0].2)
        })
    }

    fn resolve<T>(
        paths: &[&str],
        grants: &Grants,
        operation: impl FnOnce(&[(PathBuf, Arc<Root>, PathBuf)]) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let mut resolved = Vec::with_capacity(paths.len());
        for requested in paths {
            if requested.is_empty() || requested.len() > 4096 || requested.contains('\0') {
                return Err(files::invalid("invalid environment path"));
            }
            let requested = Path::new(requested);
            if requested
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            {
                return Err(files::denied());
            }
            if !requested.is_absolute() {
                return Err(files::invalid("environment paths must be absolute"));
            }
            let absolute = requested.to_path_buf();
            let (path, _) = grants
                .policies
                .iter()
                .filter(|(path, _)| absolute.starts_with(path))
                .max_by_key(|(path, _)| path.components().count())
                .ok_or_else(files::denied)?;
            let root = grants.physical.get(path).ok_or_else(files::denied)?;
            let current = PackageDirectory::open(path)?;
            let metadata = std::fs::File::from(current.descriptor().try_clone()?).metadata()?;
            if (metadata.dev(), metadata.ino()) != root.identity || root.cancelled.is_cancelled() {
                return Err(files::denied());
            }
            let relative = absolute
                .strip_prefix(path)
                .map_err(|_| files::denied())?
                .to_path_buf();
            resolved.push((absolute, root.clone(), relative));
        }
        // Physical operations retain roots; each mutation checks its short native commit grant.
        operation(&resolved)
    }

    /// The retirement signal of the longest logical grant containing `path`,
    /// `None` when no grant covers it. Purely logical: it never opens or
    /// probes the physical root, so a grant whose directory is absent still
    /// has one. Removal cancels the signal, so a consumer races it instead of
    /// probing again.
    pub(crate) fn retirement(&self, path: &Path) -> Option<CancellationSignal> {
        let grants = self.roots.read().expect("environment roots");
        grants
            .policies
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, signal)| signal.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admits(environment: &Environment, path: &Path) -> bool {
        environment
            .with_paths(&[path.to_str().unwrap()], |_| Ok(()))
            .is_ok()
    }

    #[test]
    fn a_nonexistent_grant_is_logical_only_and_never_resolves_physically() {
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("missing");
        let environment = Environment::default();
        environment.configure(std::slice::from_ref(&missing));

        let signal = environment
            .retirement(&missing.join("file"))
            .expect("the logical grant exists without a directory");
        assert!(!signal.is_cancelled());
        assert!(!admits(&environment, &missing.join("file")));
    }

    #[test]
    fn nested_grants_resolve_longest_and_never_fall_back_across_a_missing_child() {
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("child");
        let environment = Environment::default();
        environment.configure(&[parent.path().to_path_buf(), child.clone()]);

        let inner = environment.retirement(&child.join("file")).unwrap();
        let outer = environment.retirement(&parent.path().join("file")).unwrap();
        assert!(!inner.is_cancelled() && !outer.is_cancelled());
        // The parent's pin must not answer for the unpinned child grant.
        assert!(!admits(&environment, &child.join("file")));
        assert!(admits(&environment, &parent.path().join("file")));

        std::fs::create_dir(&child).unwrap();
        environment.configure(&[parent.path().to_path_buf(), child.clone()]);
        assert!(admits(&environment, &child.join("file")));
        // Unchanged grants keep their family across publications.
        inner.cancel();
        assert!(
            environment
                .retirement(&child.join("file"))
                .unwrap()
                .is_cancelled()
        );
        environment.configure(&[parent.path().to_path_buf(), child.clone()]);
        assert!(
            environment
                .retirement(&child.join("file"))
                .unwrap()
                .is_cancelled()
        );
    }

    #[test]
    fn removing_a_nested_grant_cancels_only_it_and_falls_back_to_the_parent_grant() {
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let environment = Environment::default();
        environment.configure(&[parent.path().to_path_buf(), child.clone()]);
        let inner = environment.retirement(&child.join("file")).unwrap();
        let outer = environment.retirement(&parent.path().join("file")).unwrap();

        environment.configure(std::slice::from_ref(&parent.path().to_path_buf()));
        assert!(inner.is_cancelled());
        assert!(!outer.is_cancelled());
        let fallback = environment.retirement(&child.join("file")).unwrap();
        assert!(!fallback.is_cancelled());
    }

    #[test]
    fn revocation_cancels_the_grant_and_regrant_issues_a_fresh_signal() {
        let root = tempfile::tempdir().unwrap();
        let environment = Environment::default();
        environment.configure(&[root.path().to_path_buf()]);
        let first = environment.retirement(&root.path().join("file")).unwrap();

        environment.configure(&[root.path().to_path_buf()]);
        let unchanged = environment.retirement(&root.path().join("file")).unwrap();
        first.cancel();
        assert!(
            unchanged.is_cancelled(),
            "an unchanged grant keeps its family"
        );

        environment.configure(&[]);
        assert!(environment.retirement(&root.path().join("file")).is_none());
        assert!(!admits(&environment, &root.path().join("file")));

        environment.configure(&[root.path().to_path_buf()]);
        let regranted = environment.retirement(&root.path().join("file")).unwrap();
        assert!(!regranted.is_cancelled(), "a regrant is a fresh grant");
        assert!(admits(&environment, &root.path().join("file")));
    }
}
