//! Descriptor-relative, bounded filesystem operations under native root grants.
use super::Root;
use crate::package_io::PackageDirectory;
use rustix::fs::{Mode, OFlags};
use std::fs::File;
use std::io;
use std::path::Path;

// The physical metadata/chunk contract and its stamping live in
// `rustx_confined_fs`, shared with the Execution Client; they are re-exported
// so there is exactly one business type on each side of the workspace.
pub use rustx_confined_fs::{DirectoryEntry, DirectoryPage, FileChunk, FileMetadata};

pub const CHUNK_BYTES: usize = 512 * 1024;
pub(crate) fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "environment root unavailable or unauthorized",
    )
}
pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
pub(super) fn directory(root: &PackageDirectory, relative: &Path) -> io::Result<PackageDirectory> {
    root.descend(relative)
}
fn parent(root: &Root, relative: &Path) -> io::Result<(PackageDirectory, std::ffi::OsString)> {
    let leaf = relative
        .file_name()
        .ok_or_else(|| invalid("operation requires a leaf path"))?;
    let directory = directory(&root.directory, relative.parent().unwrap_or(Path::new("")))?;
    Ok((directory, leaf.to_os_string()))
}
fn open(directory: &PackageDirectory, leaf: &std::ffi::OsStr) -> io::Result<File> {
    directory.verify()?;
    Ok(File::from(rustix::fs::openat(
        directory.descriptor(),
        leaf,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
    )?))
}
/// The physical stat lives in `rustx_confined_fs` — no-follow descent,
/// leaf `fstatat(AT_SYMLINK_NOFOLLOW)`, chain verify before and after.
/// The root grant's cancellation is the only product-level check here.
pub(crate) fn metadata(root: &Root, relative: &Path) -> io::Result<FileMetadata> {
    if root.cancelled.is_cancelled() {
        return Err(denied());
    }
    root.directory.entry_metadata(relative)
}

/// Retain the exact native watch source instead of resolving its pathname again
/// inside an OS watcher. Both files and directories use no-follow traversal.
pub(crate) fn watch_source(root: &Root, relative: &Path) -> io::Result<(File, FileMetadata)> {
    let before = metadata(root, relative)?;
    if !before.is_file && !before.is_directory {
        return Err(denied());
    }
    let file = if relative.as_os_str().is_empty() {
        File::from(root.directory.descriptor().try_clone()?)
    } else {
        let (directory, leaf) = parent(root, relative)?;
        let file = open(&directory, &leaf)?;
        directory.verify()?;
        file
    };
    let retained = FileMetadata::of(&file)?;
    if (!retained.is_file && !retained.is_directory)
        || before.identity != retained.identity
        || metadata(root, relative)?.identity != retained.identity
    {
        return Err(denied());
    }
    Ok((file, retained))
}

/// Shared descriptor-relative range reader for native file and Skill sources.
/// It retains only the requested bytes and verifies descriptor/path identity.
/// The physical algorithm lives in `rustx_confined_fs`; the opener is the
/// root's `PackageDirectory::file` so both underlying opens stay instrumented.
pub(crate) fn read_range(
    directory: &PackageDirectory,
    relative: &Path,
    offset: u64,
    max_bytes: usize,
    expected: Option<&str>,
) -> io::Result<FileChunk> {
    rustx_confined_fs::read_range(
        relative,
        offset,
        max_bytes,
        CHUNK_BYTES,
        expected,
        |relative| directory.file(relative),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_runtime::environment::Environment;

    fn fixture() -> (tempfile::TempDir, Environment, String) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let environment = Environment::default();
        environment.configure(std::slice::from_ref(&root));
        (temporary, environment, root.display().to_string())
    }
    #[test]
    fn root_replacement_requires_a_new_native_grant() {
        let (temporary, environment, root) = fixture();
        std::fs::write(format!("{root}/file"), b"original").unwrap();
        let retired = temporary.path().with_extension("retired");
        std::fs::rename(temporary.path(), &retired).unwrap();
        std::fs::create_dir(temporary.path()).unwrap();
        std::fs::write(temporary.path().join("file"), b"replacement").unwrap();
        assert!(
            environment
                .with_paths(&[format!("{root}/file").as_str()], |paths| metadata(
                    &paths[0].1,
                    &paths[0].2
                ))
                .is_err()
        );
        environment.configure(&[]);
        environment.configure(&[Path::new(&root).to_path_buf()]);
        assert!(
            environment
                .with_paths(&[format!("{root}/file").as_str()], |paths| metadata(
                    &paths[0].1,
                    &paths[0].2
                ))
                .is_ok()
        );
        std::fs::remove_dir_all(retired).unwrap();
    }
    #[test]
    fn metadata_and_listing_never_follow_symbolic_links() {
        let (temporary, environment, root) = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("private"), b"private").unwrap();
        std::os::unix::fs::symlink(outside.path(), temporary.path().join("link")).unwrap();
        assert!(
            environment
                .with_paths(&[format!("{root}/link").as_str()], |paths| metadata(
                    &paths[0].1,
                    &paths[0].2
                ))
                .unwrap()
                .is_symlink
        );
        assert!(
            environment
                .with_paths(&[root.as_str()], |paths| paths[0]
                    .1
                    .directory
                    .directory_page(&paths[0].2, 0, 16, None, 4096, 128))
                .unwrap()
                .entries[0]
                .metadata
                .is_symlink
        );
        assert!(
            environment
                .with_paths(
                    &[format!("{root}/link/private").as_str()],
                    |paths| metadata(&paths[0].1, &paths[0].2)
                )
                .is_err()
        );
    }
}
