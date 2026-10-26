//! Bounded whole-file captures outside Git's coverage.
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
};

pub(super) const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Capture {
    Missing,
    Blob(PathBuf),
    Oversized,
}

impl Capture {
    pub(super) fn read(path: &Path, directory: &Path) -> Result<Option<Self>, String> {
        match std::fs::metadata(path) {
            Ok(metadata) if !metadata.is_file() => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Some(Self::Missing));
            }
            Err(error) => return Err(error.to_string()),
            Ok(_) => {}
        }
        let mut options = OpenOptions::new();
        options.read(true);
        // A regular file can be replaced after metadata(). A FIFO must not
        // block open() before the handle's own type check can reject it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Some(Self::Missing));
            }
            Err(error) => return Err(error.to_string()),
        };
        if !file
            .metadata()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() > MAX_FILE_BYTES {
            return Ok(Some(Self::Oversized));
        }
        let blob = directory.join(format!("{:x}", Sha256::digest(&bytes)));
        if !blob.exists() {
            std::fs::write(&blob, bytes).map_err(|error| error.to_string())?;
        }
        Ok(Some(Self::Blob(blob)))
    }

    pub(super) fn bytes(&self) -> Result<Option<Vec<u8>>, String> {
        match self {
            Self::Missing => Ok(Some(Vec::new())),
            Self::Blob(path) => std::fs::read(path)
                .map(Some)
                .map_err(|error| error.to_string()),
            Self::Oversized => Ok(None),
        }
    }
}
