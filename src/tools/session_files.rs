//! Explicit references to mutable files in a native Conversation filesystem.
//! No artifact storage, path guessing, or browser-supplied filesystem authority.
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use nix::fcntl::{AtFlags, OFlag, open, openat};
use nix::sys::stat::{Mode, fstatat};
use serde::{Deserialize, Serialize};

/// One JSON frame is 1 MiB. 512 KiB becomes at most 699,052 base64 bytes.
pub const SESSION_FILE_MAX_BYTES: usize = 512 * 1024;
/// File bodies are never retained in canonical history.
pub const SESSION_FILE_MAX_READS: usize = 2;
/// A declaration is small enough for ordinary Tool-result history.
pub const PRESENT_MAX_FILES: usize = 8;

/// The original native filesystem root, independent of the viewing lineage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionFileScope {
    /// Original Conversation. Copies retain this identity verbatim.
    pub conversation_id: crate::runtime::identity::ConversationId,
    /// Device/inode identity of the authorized root, encoded losslessly.
    pub device: String,
    /// Root inode; never silently rebound after root replacement.
    pub inode: String,
}

/// An explicit delivery fact. Bytes and the leaf inode remain mutable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionFileReference {
    /// Immutable historical resolution scope.
    pub scope: SessionFileScope,
    /// Normalized relative path under that root, never an `ArtifactId`.
    pub path: String,
    /// Original filename, including spaces and Unicode.
    pub name: String,
    /// Bounded optional user-facing description.
    pub description: Option<String>,
    /// Inert viewer classification. It confers no filesystem authority.
    pub mime_type: String,
}

/// Closed file-access failures; diagnostics never become path authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionFileReadFailure {
    Missing,
    Unauthorized,
    Unavailable,
    NotRegular,
    Replaced,
    TooLarge,
    Capacity,
    ReadFailed,
}
#[derive(Debug)]
struct AccessFailure {
    reason: SessionFileReadFailure,
    message: &'static str,
}
impl std::fmt::Display for AccessFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for AccessFailure {}
fn invalid(reason: SessionFileReadFailure, message: &'static str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        AccessFailure { reason, message },
    )
}
pub(crate) fn unavailable() -> io::Error {
    invalid(
        SessionFileReadFailure::Unavailable,
        "original Session filesystem unavailable",
    )
}
pub(crate) fn read_failure(error: &io::Error) -> SessionFileReadFailure {
    if let Some(failure) = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<AccessFailure>())
    {
        return failure.reason;
    }
    match error.kind() {
        io::ErrorKind::NotFound => SessionFileReadFailure::Missing,
        io::ErrorKind::PermissionDenied => SessionFileReadFailure::Unauthorized,
        _ if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) => {
            SessionFileReadFailure::Unauthorized
        }
        _ => SessionFileReadFailure::ReadFailed,
    }
}

/// Agent paths may be relative or absolute under the exact native cwd.
/// Parent traversal is rejected even when it would normalize back inside.
pub(crate) fn relative_path(root: &Path, requested: &str) -> io::Result<String> {
    if requested.is_empty() || requested.len() > 4096 || requested.contains('\0') {
        return Err(invalid(
            SessionFileReadFailure::Unauthorized,
            "invalid delivery path",
        ));
    }
    let path = Path::new(requested);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid(
            SessionFileReadFailure::Unauthorized,
            "delivery path traversal rejected",
        ));
    }
    let path = if path.is_absolute() {
        path.strip_prefix(root).map_err(|_| {
            invalid(
                SessionFileReadFailure::Unauthorized,
                "delivery outside Session cwd",
            )
        })?
    } else {
        path
    };
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => relative.push(name),
            Component::CurDir => {}
            _ => {
                return Err(invalid(
                    SessionFileReadFailure::Unauthorized,
                    "invalid delivery component",
                ));
            }
        }
    }
    let path = relative.to_str().ok_or_else(|| {
        invalid(
            SessionFileReadFailure::Unauthorized,
            "delivery path is not UTF-8",
        )
    })?;
    if path.is_empty() {
        return Err(invalid(
            SessionFileReadFailure::Unauthorized,
            "delivery requires a file",
        ));
    }
    Ok(path.to_owned())
}

fn directory(parent: &File, name: &std::ffi::OsStr) -> io::Result<File> {
    Ok(File::from(openat(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?))
}

/// Keep every ancestor descriptor until read completes. Validate edges against
/// those descriptors before any bytes are read; replacement cannot redirect an
/// already-open fd. No realpath -> ordinary open security boundary exists.
struct OpenFile {
    file: File,
    chain: Vec<(File, std::ffi::OsString)>,
}
impl OpenFile {
    fn verify(&self) -> io::Result<()> {
        for (index, (parent, name)) in self.chain.iter().enumerate() {
            let child = self.chain.get(index + 1).map_or(&self.file, |(fd, _)| fd);
            let stat = fstatat(parent, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)?;
            let metadata = child.metadata()?;
            if stat.st_dev as u64 != metadata.dev() || stat.st_ino as u64 != metadata.ino() {
                return Err(invalid(
                    SessionFileReadFailure::Replaced,
                    "Session file replaced during open",
                ));
            }
        }
        Ok(())
    }
}

fn open_file(
    root: &Path,
    path: &str,
    scope: Option<&SessionFileScope>,
    before_leaf: impl FnOnce(),
) -> io::Result<(OpenFile, (u64, u64))> {
    if !root.is_absolute() {
        return Err(invalid(
            SessionFileReadFailure::Unavailable,
            "Session filesystem unavailable",
        ));
    }
    let mut current = File::from(open(
        Path::new("/"),
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    let mut chain = Vec::new();
    for component in root.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let child = directory(&current, name)?;
                chain.push((current, name.to_owned()));
                current = child;
            }
            _ => {
                return Err(invalid(
                    SessionFileReadFailure::Unauthorized,
                    "unsafe Session root",
                ));
            }
        }
    }
    let metadata = current.metadata()?;
    let identity = (metadata.dev(), metadata.ino());
    if scope.is_some_and(|scope| {
        scope.device != identity.0.to_string() || scope.inode != identity.1.to_string()
    }) {
        return Err(invalid(
            SessionFileReadFailure::Unavailable,
            "original Session filesystem unavailable",
        ));
    }
    let relative = relative_path(root, path)?;
    if relative != path {
        return Err(invalid(
            SessionFileReadFailure::Unauthorized,
            "delivery reference must be normalized",
        ));
    }
    let components: Vec<_> = Path::new(path).components().collect();
    for component in &components[..components.len() - 1] {
        let name = component.as_os_str();
        let child = directory(&current, name)?;
        chain.push((current, name.to_owned()));
        current = child;
    }
    let name = components
        .last()
        .expect("nonempty relative path")
        .as_os_str();
    let observed = fstatat(&current, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    if observed.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(invalid(
            SessionFileReadFailure::NotRegular,
            "delivery target is not a regular file",
        ));
    }
    before_leaf();
    let file = File::from(openat(
        &current,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.dev() != observed.st_dev as u64
        || metadata.ino() != observed.st_ino as u64
    {
        return Err(invalid(
            SessionFileReadFailure::Replaced,
            "Session file replaced during open",
        ));
    }
    chain.push((current, name.to_owned()));
    let opened = OpenFile { file, chain };
    opened.verify()?;
    Ok((opened, identity))
}

pub(crate) fn declare(
    root: &Path,
    conversation_id: &crate::runtime::identity::ConversationId,
    requested: &str,
    description: Option<String>,
) -> io::Result<SessionFileReference> {
    let path = relative_path(root, requested)?;
    let (_, (device, inode)) = open_file(root, &path, None, || {})?;
    let name = Path::new(&path)
        .file_name()
        .and_then(|v| v.to_str())
        .expect("UTF-8 path")
        .to_owned();
    Ok(SessionFileReference {
        scope: SessionFileScope {
            conversation_id: conversation_id.clone(),
            device: device.to_string(),
            inode: inode.to_string(),
        },
        mime_type: mime_type(&name).into(),
        path,
        name,
        description,
    })
}

pub(crate) fn read_authorized(
    root: &Path,
    reference: &SessionFileReference,
    authorized: impl Fn() -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    authorized()?;
    read_with_hook(root, reference, || {}, authorized)
}
fn read_with_hook(
    root: &Path,
    reference: &SessionFileReference,
    before_leaf: impl FnOnce(),
    authorized: impl Fn() -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    let (mut opened, _) = open_file(root, &reference.path, Some(&reference.scope), before_leaf)?;
    if opened.file.metadata()?.len() > SESSION_FILE_MAX_BYTES as u64 {
        return Err(invalid(
            SessionFileReadFailure::TooLarge,
            "Session file exceeds 512 KiB",
        ));
    }
    let mut bytes = Vec::new();
    authorized()?;
    Read::by_ref(&mut opened.file)
        .take((SESSION_FILE_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > SESSION_FILE_MAX_BYTES {
        return Err(invalid(
            SessionFileReadFailure::TooLarge,
            "Session file exceeds 512 KiB",
        ));
    }
    opened.verify()?;
    authorized()?;
    Ok(bytes)
}

fn mime_type(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "md" | "markdown" => "text/markdown",
        "txt" | "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "json" | "toml" | "yaml" | "yml"
        | "css" | "sh" | "c" | "h" | "cpp" | "go" | "java" | "sql" | "csv" | "log" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    fn fixture() -> (tempfile::TempDir, PathBuf, SessionFileReference) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/报告 file.md"), b"# Original\r\n").unwrap();
        let file = declare(
            &root,
            &crate::runtime::identity::ConversationId::generate(),
            "sub/报告 file.md",
            Some("Report".into()),
        )
        .unwrap();
        (dir, root, file)
    }
    #[test]
    fn mutable_original_bytes_unicode_names_and_explicit_bounds() {
        let (_dir, root, file) = fixture();
        assert_eq!(file.name, "报告 file.md");
        assert_eq!(
            read_authorized(&root, &file, || Ok(())).unwrap(),
            b"# Original\r\n"
        );
        std::fs::write(root.join(&file.path), vec![b'x'; 300 * 1024]).unwrap();
        assert_eq!(
            read_authorized(&root, &file, || Ok(())).unwrap().len(),
            300 * 1024
        );
        std::fs::write(root.join(&file.path), vec![b'x'; SESSION_FILE_MAX_BYTES]).unwrap();
        assert_eq!(
            read_authorized(&root, &file, || Ok(())).unwrap().len(),
            SESSION_FILE_MAX_BYTES
        );
        std::fs::write(
            root.join(&file.path),
            vec![b'x'; SESSION_FILE_MAX_BYTES + 1],
        )
        .unwrap();
        assert!(
            read_authorized(&root, &file, || Ok(()))
                .unwrap_err()
                .to_string()
                .contains("512 KiB")
        );
        std::fs::remove_file(root.join(&file.path)).unwrap();
        assert_eq!(
            read_authorized(&root, &file, || Ok(())).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
    #[test]
    fn paths_are_exact_and_never_artifact_ids_or_guessed_host_paths() {
        let (_dir, root, file) = fixture();
        for path in [
            "../sub/报告 file.md",
            "sub/../sub/报告 file.md",
            "/etc/passwd",
            "",
            "artifact_1",
        ] {
            assert!(declare(&root, &file.scope.conversation_id, path, None).is_err());
        }
        assert_eq!(
            relative_path(&root, "./sub/报告 file.md").unwrap(),
            file.path
        );
        assert_eq!(
            relative_path(&root, root.join(&file.path).to_str().unwrap()).unwrap(),
            file.path
        );
    }
    #[test]
    fn directories_fifos_devices_and_symlinks_are_rejected_without_blocking() {
        let (_dir, root, file) = fixture();
        nix::unistd::mkfifo(&root.join("fifo"), Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
        symlink(root.join(&file.path), root.join("link")).unwrap();
        symlink("/etc", root.join("escape")).unwrap();
        for path in ["sub", "fifo", "link", "escape/passwd"] {
            assert!(declare(&root, &file.scope.conversation_id, path, None).is_err());
        }
        // Portable device probe against the actual native /dev root.
        assert!(declare(Path::new("/dev"), &file.scope.conversation_id, "null", None).is_err());
    }
    #[test]
    fn deterministic_leaf_and_ancestor_swaps_never_read_replacement_bytes() {
        for leaf in [false, true] {
            for symbolic in [false, true] {
                let (_dir, root, file) = fixture();
                let outside = tempfile::tempdir().unwrap();
                std::fs::write(outside.path().join("报告 file.md"), b"UNAUTHORIZED").unwrap();
                let result = read_with_hook(
                    &root,
                    &file,
                    || {
                        if leaf {
                            std::fs::rename(root.join(&file.path), root.join("old-file")).unwrap();
                            if symbolic {
                                symlink(outside.path().join("报告 file.md"), root.join(&file.path))
                                    .unwrap();
                            } else {
                                std::fs::write(root.join(&file.path), b"UNAUTHORIZED").unwrap();
                            }
                        } else {
                            std::fs::rename(root.join("sub"), root.join("old-dir")).unwrap();
                            if symbolic {
                                symlink(outside.path(), root.join("sub")).unwrap();
                            } else {
                                std::fs::create_dir(root.join("sub")).unwrap();
                                std::fs::write(root.join(&file.path), b"UNAUTHORIZED").unwrap();
                            }
                        }
                    },
                    || Ok(()),
                );
                assert!(result.is_err(), "leaf={leaf} symbolic={symbolic}");
            }
        }
    }
    #[test]
    fn historical_root_replacement_and_authority_revocation_fail_closed() {
        let (_dir, root, file) = fixture();
        let unrelated = tempfile::tempdir().unwrap();
        std::fs::create_dir(unrelated.path().join("sub")).unwrap();
        std::fs::write(unrelated.path().join(&file.path), b"UNRELATED").unwrap();
        assert!(
            read_authorized(&unrelated.path().canonicalize().unwrap(), &file, || Ok(())).is_err()
        );
        let authorized = std::cell::Cell::new(true);
        let result = read_with_hook(
            &root,
            &file,
            || authorized.set(false),
            || {
                if authorized.get() {
                    Ok(())
                } else {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "revoked"))
                }
            },
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }
}
