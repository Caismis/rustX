//! Development-layout compact pages. Limits are checked before allocation.
use super::{ConversationStoreError, FANOUT, Hash, PAGE_BYTES, Page, Reference, storage};
const MAGIC: &[u8; 5] = b"RXSP\x01";
const KEY_BYTES: usize = 4160;
const VALUE_BYTES: usize = 40_960;
pub(super) const HEADER: usize = 8;
pub(super) fn leaf_size(key: &str, value: &[u8]) -> usize {
    6 + key.len() + value.len()
}
pub(super) fn branch_size(reference: &Reference) -> usize {
    35 + reference.last.len()
}

pub(super) fn encode(page: &Page) -> Result<Vec<u8>, ConversationStoreError> {
    let (tag, count, size) = match page {
        Page::Leaf { entries } => (
            0,
            entries.len(),
            HEADER + entries.iter().map(|(k, v)| leaf_size(k, v)).sum::<usize>(),
        ),
        Page::Branch { children } => (
            1,
            children.len(),
            HEADER + children.iter().map(branch_size).sum::<usize>(),
        ),
    };
    if count == 0 || count > FANOUT || size > PAGE_BYTES {
        return Err(storage("fragment page geometry exceeds bound"));
    }
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(MAGIC);
    bytes.push(tag);
    bytes.extend_from_slice(
        &u16::try_from(count)
            .map_err(|_| storage("fragment page count"))?
            .to_le_bytes(),
    );
    let put_key = |bytes: &mut Vec<u8>, key: &str| -> Result<(), ConversationStoreError> {
        if key.len() > KEY_BYTES {
            return Err(storage("fragment key exceeds bound"));
        }
        bytes.extend_from_slice(
            &u16::try_from(key.len())
                .map_err(|_| storage("fragment key size"))?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(key.as_bytes());
        Ok(())
    };
    match page {
        Page::Leaf { entries } => {
            for (key, value) in entries {
                put_key(&mut bytes, key)?;
                if value.len() > VALUE_BYTES {
                    return Err(storage("fragment fact exceeds bound"));
                }
                bytes.extend_from_slice(
                    &u32::try_from(value.len())
                        .map_err(|_| storage("fragment value size"))?
                        .to_le_bytes(),
                );
                bytes.extend_from_slice(value);
            }
        }
        Page::Branch { children } => {
            for child in children {
                put_key(&mut bytes, &child.last)?;
                bytes.push(child.depth);
                bytes.extend_from_slice(&child.hash);
            }
        }
    }
    Ok(bytes)
}
struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], ConversationStoreError> {
        let end = self
            .offset
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| storage("truncated fragment page"))?;
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }
    fn short(&mut self) -> Result<usize, ConversationStoreError> {
        let bytes = self.take(2)?;
        Ok(usize::from(u16::from_le_bytes([bytes[0], bytes[1]])))
    }
    fn key(&mut self) -> Result<String, ConversationStoreError> {
        let length = self.short()?;
        if length > KEY_BYTES {
            return Err(storage("fragment key exceeds bound"));
        }
        std::str::from_utf8(self.take(length)?)
            .map(str::to_owned)
            .map_err(|_| storage("fragment key UTF8 unavailable"))
    }
}
pub(super) fn decode(bytes: &[u8]) -> Result<Page, ConversationStoreError> {
    if bytes.len() > PAGE_BYTES {
        return Err(storage("fragment page exceeds bound"));
    }
    let mut cursor = Cursor { bytes, offset: 0 };
    if cursor.take(MAGIC.len())? != MAGIC {
        return Err(storage("unsupported fragment page encoding"));
    }
    let tag = cursor.take(1)?[0];
    let count = cursor.short()?;
    if count == 0 || count > FANOUT {
        return Err(storage("fragment page count exceeds bound"));
    }
    let page = match tag {
        0 => {
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                let key = cursor.key()?;
                let n = cursor.take(4)?;
                let length = usize::try_from(u32::from_le_bytes([n[0], n[1], n[2], n[3]]))
                    .map_err(|_| storage("fragment value size"))?;
                if length > VALUE_BYTES {
                    return Err(storage("fragment fact exceeds bound"));
                }
                entries.push((key, cursor.take(length)?.to_vec()));
            }
            Page::Leaf { entries }
        }
        1 => {
            let mut children = Vec::with_capacity(count);
            for _ in 0..count {
                let last = cursor.key()?;
                let depth = cursor.take(1)?[0];
                let hash: Hash = cursor
                    .take(32)?
                    .try_into()
                    .map_err(|_| storage("fragment hash size"))?;
                children.push(Reference { hash, last, depth });
            }
            Page::Branch { children }
        }
        _ => return Err(storage("unsupported fragment page type")),
    };
    if cursor.offset != bytes.len() {
        return Err(storage("fragment page trailing bytes"));
    }
    Ok(page)
}
