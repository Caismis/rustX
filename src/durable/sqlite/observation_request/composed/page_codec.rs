//! Page-local key prefixes; both stored and expanded page sizes remain bounded.
use super::{ConversationStoreError, FANOUT, Hash, PAGE_BYTES, Page, Reference, storage};
const MAGIC: &[u8; 5] = b"RXSP\x02";
const KEY_BYTES: usize = 4160;
const VALUE_BYTES: usize = 40_960;
const HEADER: usize = 8;
/// `expanded` uses the previous full-key layout's accounting. Compression must
/// not increase how much decoded key/value data a single bounded read admits.
#[derive(Clone, Copy)]
pub(super) struct Size {
    stored: usize,
    expanded: usize,
}
impl Size {
    pub(super) const fn new() -> Self {
        Self {
            stored: HEADER,
            expanded: HEADER,
        }
    }
    pub(super) fn plus(self, other: Self) -> Self {
        Self {
            stored: self.stored.saturating_add(other.stored),
            expanded: self.expanded.saturating_add(other.expanded),
        }
    }
    pub(super) fn fits(self) -> bool {
        self.stored <= PAGE_BYTES && self.expanded <= PAGE_BYTES
    }
}
fn common_prefix(previous: &str, key: &str) -> usize {
    let mut length = previous
        .bytes()
        .zip(key.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    // A shared byte prefix can end inside two different multibyte characters.
    while !key.is_char_boundary(length) {
        length -= 1;
    }
    length
}
fn leaf_size_at(prefix: usize, key: &str, value: &[u8]) -> Size {
    Size {
        stored: 8_usize
            .saturating_add(key.len() - prefix)
            .saturating_add(value.len()),
        expanded: 6_usize
            .saturating_add(key.len())
            .saturating_add(value.len()),
    }
}
pub(super) fn leaf_size(previous: &str, key: &str, value: &[u8]) -> Size {
    leaf_size_at(common_prefix(previous, key), key, value)
}
fn branch_size_at(prefix: usize, reference: &Reference) -> Size {
    Size {
        stored: 37 + reference.last.len() - prefix,
        expanded: 35 + reference.last.len(),
    }
}
pub(super) fn branch_size(previous: &str, reference: &Reference) -> Size {
    branch_size_at(common_prefix(previous, &reference.last), reference)
}

pub(super) fn encode(page: &Page) -> Result<Vec<u8>, ConversationStoreError> {
    let (tag, count) = match page {
        Page::Leaf { entries } => (0, entries.len()),
        Page::Branch { children } => (1, children.len()),
    };
    if count == 0 || count > FANOUT {
        return Err(storage("fragment page geometry exceeds bound"));
    }
    let mut prefixes = [0; FANOUT];
    let mut size = Size::new();
    let mut previous = "";
    match page {
        Page::Leaf { entries } => {
            for (index, (key, value)) in entries.iter().enumerate() {
                if key.len() > KEY_BYTES || value.len() > VALUE_BYTES {
                    return Err(storage("fragment fact exceeds bound"));
                }
                let prefix = common_prefix(previous, key);
                prefixes[index] = prefix;
                size = size.plus(leaf_size_at(prefix, key, value));
                previous = key;
            }
        }
        Page::Branch { children } => {
            for (index, child) in children.iter().enumerate() {
                if child.last.len() > KEY_BYTES {
                    return Err(storage("fragment key exceeds bound"));
                }
                let prefix = common_prefix(previous, &child.last);
                prefixes[index] = prefix;
                size = size.plus(branch_size_at(prefix, child));
                previous = &child.last;
            }
        }
    }
    if !size.fits() {
        return Err(storage("fragment page geometry exceeds bound"));
    }
    let mut bytes = Vec::with_capacity(size.stored);
    bytes.extend_from_slice(MAGIC);
    bytes.push(tag);
    bytes.extend_from_slice(
        &u16::try_from(count)
            .expect("bounded page count")
            .to_le_bytes(),
    );
    let put_key = |bytes: &mut Vec<u8>, prefix: usize, key: &str| {
        bytes.extend_from_slice(
            &u16::try_from(prefix)
                .expect("bounded key prefix")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(
            &u16::try_from(key.len() - prefix)
                .expect("bounded key suffix")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&key.as_bytes()[prefix..]);
    };
    match page {
        Page::Leaf { entries } => {
            for (index, (key, value)) in entries.iter().enumerate() {
                put_key(&mut bytes, prefixes[index], key);
                bytes.extend_from_slice(
                    &u32::try_from(value.len())
                        .expect("bounded fact")
                        .to_le_bytes(),
                );
                bytes.extend_from_slice(value);
            }
        }
        Page::Branch { children } => {
            for (index, child) in children.iter().enumerate() {
                put_key(&mut bytes, prefixes[index], &child.last);
                bytes.push(child.depth);
                bytes.extend_from_slice(&child.hash);
            }
        }
    }
    debug_assert_eq!(bytes.len(), size.stored);
    Ok(bytes)
}
struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
    expanded: usize,
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
    fn admit(&mut self, length: usize) -> Result<(), ConversationStoreError> {
        self.expanded = self
            .expanded
            .checked_add(length)
            .filter(|size| *size <= PAGE_BYTES)
            .ok_or_else(|| storage("expanded fragment page exceeds bound"))?;
        Ok(())
    }
    fn key(&mut self, previous: &str, overhead: usize) -> Result<String, ConversationStoreError> {
        let prefix = self.short()?;
        let suffix = self.short()?;
        let length = prefix
            .checked_add(suffix)
            .filter(|length| *length <= KEY_BYTES)
            .ok_or_else(|| storage("fragment key exceeds bound"))?;
        let head = previous
            .get(..prefix)
            .ok_or_else(|| storage("fragment key prefix unavailable"))?;
        self.admit(overhead + length)?;
        let tail = std::str::from_utf8(self.take(suffix)?)
            .map_err(|_| storage("fragment key UTF8 unavailable"))?;
        let mut key = String::with_capacity(length);
        key.push_str(head);
        key.push_str(tail);
        Ok(key)
    }
}
pub(super) fn decode(bytes: &[u8]) -> Result<Page, ConversationStoreError> {
    if bytes.len() > PAGE_BYTES {
        return Err(storage("fragment page exceeds bound"));
    }
    let mut cursor = Cursor {
        bytes,
        offset: 0,
        expanded: HEADER,
    };
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
            let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(count);
            for _ in 0..count {
                let previous = entries.last().map_or("", |entry| entry.0.as_str());
                let key = cursor.key(previous, 6)?;
                let n = cursor.take(4)?;
                let length = usize::try_from(u32::from_le_bytes([n[0], n[1], n[2], n[3]]))
                    .map_err(|_| storage("fragment value size"))?;
                if length > VALUE_BYTES {
                    return Err(storage("fragment fact exceeds bound"));
                }
                cursor.admit(length)?;
                entries.push((key, cursor.take(length)?.to_vec()));
            }
            Page::Leaf { entries }
        }
        1 => {
            let mut children: Vec<Reference> = Vec::with_capacity(count);
            for _ in 0..count {
                let previous = children.last().map_or("", |child| child.last.as_str());
                let last = cursor.key(previous, 35)?;
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
