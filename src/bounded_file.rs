//! Shared bounded reads of authored files, independent of their format.

use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The byte bound of every authored file: its readers refuse anything
/// larger, and its writers never publish anything larger.
pub(crate) const MAX_RESOURCE_BYTES: u64 = 1024 * 1024;

/// Whether `bytes` fit the authored file bound.
pub(crate) fn within_bound(bytes: &[u8]) -> bool {
    u64::try_from(bytes.len()).is_ok_and(|length| length <= MAX_RESOURCE_BYTES)
}

pub(crate) fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    if !std::fs::metadata(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .is_file()
    {
        return Err(format!("{} must be a regular file", path.display()));
    }
    // A regular file replaced by a FIFO between metadata and open must not block.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    read_bounded_file(file).map_err(|reason| format!("{} {reason}", path.display()))
}

/// The bounded contents of an already opened file, which must be regular.
pub(crate) fn read_bounded_file(file: std::fs::File) -> Result<Vec<u8>, String> {
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_RESOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if !within_bound(&bytes) {
        return Err("exceeds 1 MiB".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_files_are_bounded_in_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("resource");
        let bytes = vec![b'x'; usize::try_from(MAX_RESOURCE_BYTES).unwrap()];
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(read_bounded(&path).unwrap(), bytes);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_RESOURCE_BYTES + 1)
            .unwrap();
        assert!(read_bounded(&path).unwrap_err().contains("exceeds 1 MiB"));
        assert!(
            read_bounded(directory.path())
                .unwrap_err()
                .contains("regular file")
        );
        assert!(
            read_bounded(&directory.path().join("missing"))
                .unwrap_err()
                .contains("cannot read")
        );
    }

    #[test]
    fn fifo_is_rejected_without_opening_a_blocking_reader() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fifo");
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRUSR).unwrap();
        assert!(read_bounded(&path).unwrap_err().contains("regular file"));
    }
}
