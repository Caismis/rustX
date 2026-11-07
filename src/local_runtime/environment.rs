//! Instance-owned native environment authority, shared by every trusted client.
//! Grants come only from User/Host process policy, never a request cwd or Thread.
pub mod files;

use crate::package_io::PackageDirectory;
use crate::runtime::CancellationSignal;
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

#[derive(Default)]
pub(crate) struct Environment {
    io_gate: std::sync::Mutex<()>,
    roots: RwLock<BTreeMap<PathBuf, Arc<Root>>>,
}

pub(crate) struct Root {
    directory: PackageDirectory,
    identity: (u64, u64),
    pub(crate) cancelled: CancellationSignal,
}

impl Environment {
    /// Retire removed roots before accepting their replacements. A failed open
    /// leaves a configured root unavailable rather than following a replacement.
    pub(crate) fn configure(&self, paths: &[PathBuf]) {
        let mut roots = self.roots.write().expect("environment roots");
        roots.retain(|path, root| {
            let keep = paths.contains(path);
            if !keep {
                root.cancelled.cancel();
            }
            keep
        });
        for path in paths {
            if roots.contains_key(path) {
                continue;
            }
            if let Ok(directory) = PackageDirectory::open(path)
                && let Ok(file) = directory.descriptor().try_clone()
                && let Ok(metadata) = std::fs::File::from(file).metadata()
            {
                roots.insert(
                    path.clone(),
                    Arc::new(Root {
                        directory,
                        identity: (metadata.dev(), metadata.ino()),
                        cancelled: CancellationSignal::new(),
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
        let roots = self.roots.read().expect("environment roots").clone();
        Self::resolve(paths, &roots, operation)
    }

    pub(crate) fn try_metadata(&self, path: &str) -> std::io::Result<files::FileMetadata> {
        let busy = || std::io::Error::from(std::io::ErrorKind::WouldBlock);
        let _io = self.io_gate.try_lock().map_err(|_| busy())?;
        let roots = self.roots.try_read().map_err(|_| busy())?.clone();
        Self::resolve(&[path], &roots, |paths| {
            files::metadata(&paths[0].1, &paths[0].2)
        })
    }

    fn resolve<T>(
        paths: &[&str],
        roots: &BTreeMap<PathBuf, Arc<Root>>,
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
            let (path, root) = roots
                .iter()
                .filter(|(path, _)| absolute.starts_with(path))
                .max_by_key(|(path, _)| path.components().count())
                .ok_or_else(files::denied)?;
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

    pub(crate) fn directory(
        &self,
        requested: &str,
    ) -> std::io::Result<(PathBuf, (u64, u64), CancellationSignal)> {
        self.with_paths(&[requested], |paths| {
            let (absolute, root, relative) = &paths[0];
            let directory = files::directory(&root.directory, relative)?;
            let metadata = std::fs::File::from(directory.descriptor().try_clone()?).metadata()?;
            Ok((
                absolute.clone(),
                (metadata.dev(), metadata.ino()),
                root.cancelled.clone(),
            ))
        })
    }
}
