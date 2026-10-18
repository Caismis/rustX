//! Native settled-artifact evidence, retained by the original reservation owner.
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read as _, Seek as _};
use std::os::unix::fs::FileExt as _;

pub(super) const CHUNK: usize = 65_536;
const CHUNK_U64: u64 = 65_536;
const HEADER_BYTES: usize = 64;
const HEADER: u64 = 64;
const MAGIC: &[u8; 8] = b"RTXART01";
const DOMAIN: &[u8] = b"rustx-artifact-seal-v1\0";

pub(super) struct Seal {
    pub(super) length: u64,
    count: u64,
    digest: [u8; 32],
}

fn invalid() -> io::Error {
    io::Error::other("artifact settlement evidence unavailable")
}
fn count(length: u64) -> u64 {
    length.div_ceil(CHUNK_U64)
}
fn manifest_hash(length: u64, count: u64) -> Sha256 {
    let mut digest = Sha256::new();
    digest.update(DOMAIN);
    digest.update(length.to_le_bytes());
    digest.update(count.to_le_bytes());
    digest
}

pub(super) fn settle(
    file: &mut File,
    reservation: &File,
    root: &std::path::Path,
    original: Option<&Seal>,
) -> io::Result<()> {
    let length = file.metadata()?.len();
    let count = count(length);
    let manifest_length = count
        .checked_mul(32)
        .and_then(|n| n.checked_add(HEADER))
        .ok_or_else(invalid)?;
    file.sync_all()?;
    // No readable header exists while partial evidence is being constructed.
    reservation.set_len(HEADER)?;
    reservation.write_all_at(&[0; HEADER_BYTES], 0)?;
    file.seek(io::SeekFrom::Start(0))?;
    let mut buffer = vec![0; CHUNK];
    let mut offset = 0;
    let mut aggregate = manifest_hash(length, count);
    while offset < length {
        let size = usize::try_from((length - offset).min(CHUNK_U64)).map_err(|_| invalid())?;
        file.read_exact(&mut buffer[..size])?;
        let digest = Sha256::digest(&buffer[..size]);
        reservation.write_all_at(&digest, HEADER + offset / CHUNK_U64 * 32)?;
        aggregate.update(digest);
        offset += u64::try_from(size).map_err(|_| invalid())?;
    }
    let mut extra = [0];
    if file.read(&mut extra)? != 0
        || file.metadata()?.len() != length
        || reservation.metadata()?.len() != manifest_length
    {
        return Err(invalid());
    }
    let digest: [u8; 32] = aggregate.finalize().into();
    if original.is_some_and(|original| {
        original.length != length || original.count != count || original.digest != digest
    }) {
        return Err(invalid());
    }
    reservation.sync_all()?;
    File::open(root)?.sync_all()?;
    #[cfg(test)]
    super::integrity_tests::before_seal_header(root)?;
    let mut header = [0; HEADER_BYTES];
    header[..8].copy_from_slice(MAGIC);
    header[8..16].copy_from_slice(&length.to_le_bytes());
    header[16..24].copy_from_slice(&count.to_le_bytes());
    header[24..56].copy_from_slice(&digest);
    // Publish last while the original exclusive reservation is still held.
    reservation.write_all_at(&header, 0)?;
    reservation.sync_all()
}

impl Seal {
    pub(super) fn read(reservation: &File, length: u64) -> io::Result<Self> {
        if !reservation.metadata()?.is_file() {
            return Err(invalid());
        }
        let mut header = [0; HEADER_BYTES];
        reservation.read_exact_at(&mut header, 0)?;
        if &header[..8] != MAGIC || header[56..].iter().any(|b| *b != 0) {
            return Err(invalid());
        }
        let sealed_length = u64::from_le_bytes(header[8..16].try_into().map_err(|_| invalid())?);
        let chunks = u64::from_le_bytes(header[16..24].try_into().map_err(|_| invalid())?);
        let expected = chunks
            .checked_mul(32)
            .and_then(|n| n.checked_add(HEADER))
            .ok_or_else(invalid)?;
        if sealed_length != length
            || chunks != count(length)
            || expected != reservation.metadata()?.len()
        {
            return Err(invalid());
        }
        Ok(Self {
            length,
            count: chunks,
            digest: header[24..56].try_into().map_err(|_| invalid())?,
        })
    }
    pub(super) fn verify(
        &self,
        reservation: &File,
        verification: &mut Verification,
        offset: u64,
        bytes: &[u8],
    ) -> io::Result<[u8; 32]> {
        if offset != verification.next {
            return Err(invalid());
        }
        if offset >= self.length
            || !offset.is_multiple_of(CHUNK_U64)
            || u64::try_from(bytes.len()).map_err(|_| invalid())?
                != (self.length - offset).min(CHUNK_U64)
        {
            return Err(invalid());
        }
        let mut expected = [0; 32];
        let position = (offset / CHUNK_U64)
            .checked_mul(32)
            .and_then(|n| n.checked_add(HEADER))
            .ok_or_else(invalid)?;
        reservation.read_exact_at(&mut expected, position)?;
        let actual: [u8; 32] = Sha256::digest(bytes).into();
        if actual != expected {
            return Err(invalid());
        }
        verification.aggregate.update(expected);
        verification.next = offset
            .checked_add(u64::try_from(bytes.len()).map_err(|_| invalid())?)
            .ok_or_else(invalid)?;
        Ok(expected)
    }
    pub(super) fn verification(&self) -> Verification {
        Verification {
            aggregate: manifest_hash(self.length, self.count),
            next: 0,
        }
    }
    pub(super) fn check_complete(&self, verification: Verification) -> io::Result<()> {
        if verification.next != self.length {
            return Err(invalid());
        }
        let actual: [u8; 32] = verification.aggregate.finalize().into();
        if actual != self.digest {
            return Err(invalid());
        }
        Ok(())
    }
}

/// One sequential native-artifact verification; callers cannot skip/reorder chunks.
pub(crate) struct Verification {
    aggregate: Sha256,
    next: u64,
}
