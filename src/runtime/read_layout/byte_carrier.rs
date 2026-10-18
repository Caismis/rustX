//! Bounded resident Source bytes with one-way spill to the Source-owned directory.
use std::io::{Read, Seek, Write};
use std::sync::Arc;

pub(super) const RESIDENT_BYTES: usize = 4096;

#[derive(Debug)]
pub(super) enum Bytes {
    Writing(Vec<u8>),
    Resident(Arc<[u8]>),
    Disk(Option<std::fs::File>),
}
impl Default for Bytes {
    fn default() -> Self {
        Self::Writing(Vec::new())
    }
}

pub(super) fn directory(
    directory: &mut Option<tempfile::TempDir>,
) -> std::io::Result<&tempfile::TempDir> {
    if directory.is_none() {
        *directory = Some(tempfile::tempdir()?);
    }
    Ok(directory.as_ref().expect("Source directory initialized"))
}

impl Bytes {
    pub(super) fn write(
        &mut self,
        input: &[u8],
        directory: &mut Option<tempfile::TempDir>,
    ) -> std::io::Result<usize> {
        let input = &input[..input.len().min(65_536)];
        match self {
            Self::Writing(bytes) if input.len() <= RESIDENT_BYTES - bytes.len() => {
                // Reserve exactly the fixed cap on the first nonempty write;
                // neither Vec growth nor serializer chunk size can exceed it.
                if !input.is_empty() && bytes.capacity() == 0 {
                    bytes.reserve_exact(RESIDENT_BYTES);
                }
                bytes.extend_from_slice(input);
                Ok(input.len())
            }
            Self::Writing(bytes) => {
                let directory = self::directory(directory)?;
                let mut file = std::fs::File::options()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(directory.path().join("bytes"))?;
                file.write_all(bytes)?;
                #[cfg(test)]
                if !bytes.is_empty() {
                    record_write();
                }
                let count = file.write(input)?;
                #[cfg(test)]
                record_write();
                // A write failure aborts Source construction. Its directory
                // owns any partial file, and no sealed Source is published.
                *self = Self::Disk(Some(file));
                Ok(count)
            }
            Self::Disk(Some(file)) => {
                let count = file.write(input)?;
                #[cfg(test)]
                record_write();
                Ok(count)
            }
            Self::Resident(_) | Self::Disk(None) => Err(std::io::Error::other("source sealed")),
        }
    }

    pub(super) fn seal(&mut self) {
        match self {
            Self::Writing(bytes) => *self = Self::Resident(std::mem::take(bytes).into()),
            Self::Disk(file) => *file = None,
            Self::Resident(_) => {}
        }
    }

    pub(super) fn reader(&self, directory: Option<&tempfile::TempDir>) -> std::io::Result<Reader> {
        match self {
            Self::Writing(bytes) => Ok(Reader::Memory(std::io::Cursor::new(Arc::from(
                bytes.as_slice(),
            )))),
            Self::Resident(bytes) => Ok(Reader::Memory(std::io::Cursor::new(bytes.clone()))),
            Self::Disk(handle) => {
                let file = match handle {
                    Some(file) => file.try_clone()?,
                    None => std::fs::File::open(
                        directory
                            .ok_or_else(|| std::io::Error::other("Source directory unavailable"))?
                            .path()
                            .join("bytes"),
                    )?,
                };
                #[cfg(test)]
                super::FILE_WORK.with(|work| {
                    let (writes, opens) = work.get();
                    work.set((writes, opens + 1));
                });
                Ok(Reader::File(file))
            }
        }
    }
}

#[cfg(test)]
fn record_write() {
    super::FILE_WORK.with(|work| {
        let (writes, opens) = work.get();
        work.set((writes + 1, opens));
    });
}

pub(super) enum Reader {
    Memory(std::io::Cursor<Arc<[u8]>>),
    File(std::fs::File),
}
impl Reader {
    pub(super) fn read_exact_at(&self, output: &mut [u8], start: u64) -> std::io::Result<()> {
        match self {
            Self::Memory(cursor) => {
                let start = usize::try_from(start).map_err(|_| {
                    std::io::Error::other("Source offset exceeds platform capacity")
                })?;
                let end = start
                    .checked_add(output.len())
                    .ok_or_else(|| std::io::Error::other("Source range overflow"))?;
                let bytes = cursor
                    .get_ref()
                    .get(start..end)
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))?;
                output.copy_from_slice(bytes);
                Ok(())
            }
            Self::File(file) => {
                use std::os::unix::fs::FileExt as _;
                file.read_exact_at(output, start)
            }
        }
    }
}
impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Memory(cursor) => cursor.read(output),
            Self::File(file) => file.read(output),
        }
    }
}
impl Seek for Reader {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Memory(cursor) => cursor.seek(position),
            Self::File(file) => file.seek(position),
        }
    }
}
