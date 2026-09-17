//! Revision and persistent writer-lock primitives shared by CFG3 source authoring.
use crate::runtime_client::types::RuntimeClientError;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

fn failure(message: &str) -> RuntimeClientError {
    RuntimeClientError::InvalidRequest {
        message: message.into(),
    }
}
fn io_failure(_: impl std::fmt::Debug) -> RuntimeClientError {
    failure(
        "authored source I/O failed before publication; inspect document permissions and retry after reading its revision",
    )
}
/// Shared with create-only initialization. Never unlink the lock inode.
pub(super) fn lock_document(target: &Path) -> std::io::Result<File> {
    let name = target
        .file_name()
        .ok_or_else(|| std::io::Error::other("invalid document"))?;
    let lock = target.with_file_name(format!(".{}.lock", name.to_string_lossy()));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(lock)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("lock is not a regular file"));
    }
    file.lock()?;
    Ok(file)
}

pub(super) fn read_document(path: &Path) -> Result<Option<Vec<u8>>, RuntimeClientError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => Err(failure(
            "authored source must be a regular file, not a symlink",
        )),
        Ok(_) => crate::bounded_file::read_bounded(path)
            .map(Some)
            .map_err(io_failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_failure(error)),
    }
}
pub(super) fn revision(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(
        || "missing".into(),
        |bytes| format!("sha256:{:x}", Sha256::digest(bytes)),
    )
}
