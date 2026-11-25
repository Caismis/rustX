//! Shared protocol projections and bounded Skill reads.
use crate::package_io::PackageDirectory;
pub use rustx_confined_fs::{DirectoryEntry, DirectoryPage, FileChunk, FileMetadata};
use std::io;
use std::path::Path;

pub const CHUNK_BYTES: usize = 512 * 1024;

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
