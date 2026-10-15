//! Bounded write-owner reuse of producer-certified immutable canonical fragments.
//! Every request still commits its own source bytes, field rows and native facts.
//! One `ConnectionOwner` owns at most8MiB retained/pending fragment storage and8192
//! entries. Request-local emission/virtual-seat metadata has its own fixed8MiB
//! bound in `read_layout`; it pins only already charged immutable fragment data.
//! No eviction happens during a request scan. After commit, unused entries are
//! dropped; a full cache declines new admission rather than evicting a prefix
//! that the same growing request will need again. No runtime/storage lease or
//! product owner is retained. Reopen, eviction and disabled reuse change no reads.
use super::{Connection, ConversationStoreError, OptionalExtension, storage};
use crate::model::projected::FrozenModelInput;
use crate::runtime::read_layout::{FragmentCapture, SeekableObservationSource, TrustedFragment};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::cell::RefCell;
use std::sync::Arc;

const CACHE_BYTES: usize = 8 * 1024 * 1024;
const CACHE_ENTRIES: usize = 8192;
const ENTRY_BYTES: usize = 64 * 1024;
const ENTRY_NODES: usize = 512;
const HEADER_BYTES: usize = 16_384;
// Retained candidate/header comparison, bounded fragment capture, root-name
// vectors and allocator capacity slack; no decoded header Value tree is built.
const PENDING_BYTES: usize = ENTRY_BYTES + 4 * HEADER_BYTES + 4096;
const POLICY: u8 = 1;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key {
    position: u64,
    scope: Box<str>,
    digest: [u8; 64],
    policy: u8,
    header_digest: [u8; 32],
}
impl Key {
    pub(super) fn original(
        connection: &Connection,
        message: &crate::model::ModelInputMessage,
        frozen: &FrozenModelInput,
    ) -> Result<Option<Self>, ConversationStoreError> {
        let (Some(id), FrozenModelInput::Original(original)) = (message.canonical_id(), frozen)
        else {
            return Ok(None);
        };
        // Long identity/provenance remains on the existing native producer path.
        if id.as_str().len() > 256 || original.conversation_id.as_str().len() > 256 {
            return Ok(None);
        }
        let Ok(digest) = <[u8; 64]>::try_from(original.content_digest.as_bytes()) else {
            return Ok(None);
        };
        let current: Option<(u64, Option<Vec<u8>>)> = connection
            .prepare_cached("SELECT m.position,CASE WHEN typeof(h.header_json)='blob' AND length(h.header_json)<=16384 THEN h.header_json END FROM message_ledger m JOIN observation_headers h ON h.reference_kind='message' AND h.reference_id=m.message_id WHERE m.message_id=?1").map_err(|error| storage(error.to_string()))?
            .query_row([id.as_str()], |row| Ok((row.get(0)?,row.get(1)?)))
            .optional().map_err(|error| storage(error.to_string()))?;
        let Some((position, Some(header))) = current else {
            return Ok(None);
        };
        Ok(Some(Self {
            position,
            scope: original.conversation_id.as_str().into(),
            digest,
            policy: POLICY,
            header_digest: Sha256::digest(&header).into(),
        }))
    }
    pub(super) fn producer_hash(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"rustx-request-shape-producer-v1");
        hash.update(self.position.to_le_bytes());
        hash.update((self.scope.len() as u64).to_le_bytes());
        hash.update(self.scope.as_bytes());
        hash.update(self.digest);
        hash.update([self.policy]);
        hash.update(self.header_digest);
        hash.finalize().into()
    }
    fn owned_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.scope.len()
    }
}
struct Slot {
    name: Box<str>,
    quoted: Box<[u8]>,
    fragment: Option<Arc<TrustedFragment>>,
}
struct Entry {
    key: Key,
    slots: Vec<Slot>,
    provenance_bytes: u64,
    bytes: usize,
    touched: u64,
}
#[derive(Default)]
pub(in crate::durable::sqlite) struct Cache {
    entries: Vec<Entry>,
    epoch: u64,
    #[cfg(test)]
    disabled: bool,
    #[cfg(test)]
    hits: usize,
    #[cfg(test)]
    captures: usize,
    #[cfg(test)]
    peak_bytes: usize,
}
struct Candidate {
    key: Key,
    item: u64,
    header: Vec<u8>,
}
pub(in crate::durable::sqlite) struct Session<'a> {
    cache: &'a mut Cache,
    pending: Vec<Entry>,
    candidates: Vec<Candidate>,
    reserved: usize,
    capture: FragmentCapture,
}
impl Cache {
    pub(in crate::durable::sqlite) fn session(&mut self) -> RefCell<Session<'_>> {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.entries.clear();
            self.epoch = 1;
        }
        RefCell::new(Session {
            cache: self,
            pending: Vec::new(),
            candidates: Vec::new(),
            reserved: 0,
            capture: FragmentCapture::new(CACHE_ENTRIES * 16),
        })
    }
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn stats(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.entries.len(),
            self.bytes(),
            self.hits,
            self.captures,
            self.peak_bytes,
        )
    }
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn entry_sizes(&self) -> Vec<(u64, usize)> {
        self.entries
            .iter()
            .map(|entry| (entry.key.position, entry.bytes))
            .collect()
    }
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn disable(&mut self) {
        self.disabled = true;
        self.entries.clear();
    }
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn evict(&mut self) {
        self.entries.clear();
    }
    fn bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<Entry>()
            + self.entries.iter().map(|entry| entry.bytes).sum::<usize>()
    }
}
impl Session<'_> {
    pub(super) fn capture(&self) -> FragmentCapture {
        self.capture.clone()
    }
    pub(super) fn item(
        &mut self,
        key: &Key,
        sequence: u64,
        item: u64,
    ) -> Result<Option<Item<'_>>, ConversationStoreError> {
        #[cfg(test)]
        if self.cache.disabled {
            return Ok(None);
        }
        let Ok(index) = self
            .cache
            .entries
            .binary_search_by(|entry| entry.key.cmp(key))
        else {
            return Ok(None);
        };
        let entry = &mut self.cache.entries[index];
        entry.touched = self.cache.epoch;
        let field = super::typed_value(&super::provenance_source(
            sequence,
            item,
            entry.provenance_bytes,
        ))?;
        let dynamic = crate::runtime_client::checkpoint::bounded_bytes(&field, 1024)
            .ok_or_else(|| storage("canonical fragment occurrence exceeds bound"))?;
        let dynamic = serde_json::value::RawValue::from_string(
            String::from_utf8(dynamic).map_err(|error| storage(error.to_string()))?,
        )
        .map_err(|error| storage(error.to_string()))?;
        let value = Item {
            entry,
            dynamic,
            capture: &self.capture,
        };
        if value.header_length().is_none() {
            return Ok(None);
        }
        #[cfg(test)]
        {
            self.cache.hits += 1;
        }
        Ok(Some(value))
    }
    pub(super) fn candidate(&mut self, key: Key, item: u64, header: &[u8]) {
        #[cfg(test)]
        if self.cache.disabled {
            return;
        }
        if header.len() > HEADER_BYTES
            || self
                .cache
                .entries
                .binary_search_by(|entry| entry.key.cmp(&key))
                .is_ok()
            || self.candidates.iter().any(|candidate| candidate.key == key)
            || self.cache.entries.len() + self.pending.len() + self.candidates.len()
                >= CACHE_ENTRIES
            || self.cache.bytes() + self.reserved + PENDING_BYTES > CACHE_BYTES
        {
            return;
        }
        self.reserved += PENDING_BYTES;
        self.candidates.push(Candidate {
            key,
            item,
            header: header.to_vec(),
        });
        #[cfg(test)]
        {
            self.cache.peak_bytes = self
                .cache
                .peak_bytes
                .max(self.cache.bytes() + self.reserved);
        }
    }
    pub(super) fn finish(&mut self, source: &SeekableObservationSource) {
        for candidate in std::mem::take(&mut self.candidates) {
            // Private reuse admission cannot invalidate an already completed
            // request layout. No candidate is advertised as source evidence.
            let entry = Self::capture_entry(source, candidate, self.cache.epoch).unwrap_or(None);
            self.reserved -= PENDING_BYTES;
            if let Some(entry) = entry {
                self.reserved += entry.bytes
                    + 2 * (std::mem::size_of::<Entry>() + std::mem::size_of::<Candidate>());
                self.pending.push(entry);
                #[cfg(test)]
                {
                    self.cache.captures += 1;
                }
            }
        }
    }
    fn capture_entry(
        source: &SeekableObservationSource,
        candidate: Candidate,
        epoch: u64,
    ) -> Result<Option<Entry>, ConversationStoreError> {
        let path = ["messages".into(), candidate.item.to_string()];
        let raw = source
            .read(&path, 0, 16_384)
            .map_err(|error| storage(error.to_string()))?;
        if raw.next_offset.is_some() || raw.bytes != candidate.header {
            return Ok(None);
        }
        let Some(fields) = property_names(&candidate.header) else {
            return Ok(None);
        };
        let mut slots = Vec::with_capacity(fields.len());
        let mut bytes = candidate.key.owned_bytes() + fields.len() * std::mem::size_of::<Slot>();
        let mut nodes = 0;
        let mut provenance_bytes = None;
        for name in &fields {
            let quoted = serde_json::to_vec(name).map_err(|error| storage(error.to_string()))?;
            bytes += name.len() + quoted.len();
            let fragment = if name == "provenance_source" {
                None
            } else {
                let path = ["messages".into(), candidate.item.to_string(), name.clone()];
                let Some(fragment) = source
                    .capture_fragment(
                        &path,
                        ENTRY_BYTES.saturating_sub(bytes),
                        ENTRY_NODES.saturating_sub(nodes),
                    )
                    .map_err(storage)?
                else {
                    return Ok(None);
                };
                bytes += fragment.owned_bytes();
                nodes += fragment.node_count();
                if name == "provenance" {
                    provenance_bytes = Some(fragment.raw_bytes().len() as u64);
                }
                Some(fragment)
            };
            if bytes > ENTRY_BYTES || nodes > ENTRY_NODES {
                return Ok(None);
            }
            slots.push(Slot {
                name: name.as_str().into(),
                quoted: quoted.into(),
                fragment,
            });
        }
        let Some(provenance_bytes) = provenance_bytes else {
            return Ok(None);
        };
        Ok(Some(Entry {
            key: candidate.key,
            slots,
            provenance_bytes,
            bytes,
            touched: epoch,
        }))
    }
    pub(in crate::durable::sqlite) fn abort_layout(&mut self) {
        self.candidates.clear();
        self.pending.clear();
        self.reserved = 0;
    }
    pub(in crate::durable::sqlite) fn commit(mut self) {
        self.cache
            .entries
            .retain(|entry| entry.touched == self.cache.epoch);
        for entry in self.pending.drain(..) {
            if self.cache.bytes() + entry.bytes + std::mem::size_of::<Entry>() > CACHE_BYTES {
                continue;
            }
            let index = self
                .cache
                .entries
                .binary_search_by(|stored| stored.key.cmp(&entry.key))
                .unwrap_or_else(|index| index);
            self.cache.entries.reserve_exact(1);
            if self.cache.bytes() + entry.bytes > CACHE_BYTES {
                continue;
            }
            self.cache.entries.insert(index, entry);
        }
    }
}
// Scan only the closed descriptor's root names. Ignoring subtree values avoids
// a second decoded Value tree; the original producer bytes remain unchanged.
fn property_names(bytes: &[u8]) -> Option<Vec<String>> {
    struct Names;
    impl<'de> serde::de::Visitor<'de> for Names {
        type Value = Vec<String>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("finite native descriptor properties")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut names = Vec::new();
            while let Some(name) = map.next_key_seed(Name)? {
                if names.len() == 32 {
                    return Err(serde::de::Error::custom("fragment property count"));
                }
                names.reserve_exact(1);
                names.push(name);
                map.next_value::<serde::de::IgnoredAny>()?;
            }
            Ok(names)
        }
    }
    struct Name;
    impl serde::de::Visitor<'_> for Name {
        type Value = String;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("bounded native property name")
        }
        fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<String, E> {
            if name.len() > 64 {
                return Err(E::custom("fragment property bytes"));
            }
            Ok(name.to_owned())
        }
    }
    impl<'de> serde::de::DeserializeSeed<'de> for Name {
        type Value = String;
        fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<String, D::Error> {
            decoder.deserialize_str(self)
        }
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let names = serde::de::Deserializer::deserialize_map(&mut decoder, Names).ok()?;
    decoder.end().ok()?;
    Some(names)
}

pub(super) struct Item<'a> {
    entry: &'a Entry,
    dynamic: Box<serde_json::value::RawValue>,
    capture: &'a FragmentCapture,
}
impl Item<'_> {
    // Preserve the original per-property fit predicate, including its first-key
    // closing-byte allowance. Both admission and construction use this check.
    fn header_length(&self) -> Option<usize> {
        let mut length = 1_usize;
        for (index, slot) in self.entry.slots.iter().enumerate() {
            let value = slot.bytes(&self.dynamic);
            length
                .checked_add(slot.quoted.len())?
                .checked_add(value.len())?
                .checked_add(3)
                .filter(|length| *length <= HEADER_BYTES)?;
            length = length
                .checked_add(slot.quoted.len())?
                .checked_add(value.len())?
                .checked_add(1 + usize::from(index != 0))?;
        }
        length.checked_add(1)
    }
    pub(super) fn header(&self) -> Option<Vec<u8>> {
        let length = self.header_length()?;
        #[cfg(test)]
        HEADER_ALLOCATIONS.with(|work| work.set(work.get() + 1));
        let mut bytes = Vec::with_capacity(length);
        bytes.push(b'{');
        for (index, slot) in self.entry.slots.iter().enumerate() {
            if index != 0 {
                bytes.push(b',');
            }
            bytes.extend_from_slice(&slot.quoted);
            bytes.push(b':');
            bytes.extend_from_slice(slot.bytes(&self.dynamic));
        }
        bytes.push(b'}');
        debug_assert_eq!(bytes.len(), length);
        Some(bytes)
    }
}
impl Slot {
    fn bytes<'a>(&'a self, dynamic: &'a serde_json::value::RawValue) -> &'a [u8] {
        self.fragment
            .as_ref()
            .map_or(dynamic.get().as_bytes(), |fragment| fragment.raw_bytes())
    }
}
#[cfg(test)]
thread_local! {static HEADER_ALLOCATIONS:std::cell::Cell<usize> = const{std::cell::Cell::new(0)};}

impl Serialize for Item<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(Some(self.entry.slots.len()))?;
        for slot in &self.entry.slots {
            if let Some(fragment) = &slot.fragment {
                map.serialize_entry(&slot.name, &fragment.emitted(self.capture))?;
            } else {
                map.serialize_entry(&slot.name, &self.dynamic)?;
            }
        }
        map.end()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn t20_canonical_fragment_property_scan_keeps_order_and_bounds_only_root_scratch() {
        let raw =
            br#"{"last":{"huge":[1,2,3,{"ignored":"original"}]},"first":[[[]]],"escaped\n":null}"#;
        assert_eq!(
            property_names(raw),
            Some(vec!["last".into(), "first".into(), "escaped\n".into()])
        );
        let deep = serde_json::json!({"only": "x".repeat(15_000)});
        assert_eq!(
            property_names(&serde_json::to_vec(&deep).unwrap()),
            Some(vec!["only".into()])
        );
        let too_many = (0..33)
            .map(|index| (format!("key-{index}"), serde_json::Value::Null))
            .collect::<serde_json::Map<_, _>>();
        assert!(property_names(&serde_json::to_vec(&too_many).unwrap()).is_none());
        let large_key = serde_json::json!({"x".repeat(65): null});
        assert!(property_names(&serde_json::to_vec(&large_key).unwrap()).is_none());
        assert!(property_names(b"{} trailing").is_none());
    }
    #[test]
    fn t20_canonical_fragment_pending_storage_is_bounded_before_allocation_and_not_published() {
        let mut cache = Cache::default();
        let session = cache.session();
        let header = vec![b'x'; HEADER_BYTES];
        for position in 0..10_000 {
            session.borrow_mut().candidate(
                Key {
                    position,
                    scope: "bounded-test-scope".into(),
                    digest: [b'0'; 64],
                    policy: POLICY,
                    header_digest: [0; 32],
                },
                0,
                &header,
            );
        }
        let mut session = session.into_inner();
        assert!(!session.candidates.is_empty());
        assert!(session.candidates.len() < CACHE_ENTRIES);
        assert!(session.cache.bytes() + session.reserved <= CACHE_BYTES);
        let actual = session.candidates.capacity() * std::mem::size_of::<Candidate>()
            + session
                .candidates
                .iter()
                .map(|candidate| candidate.header.capacity() + candidate.key.scope.len())
                .sum::<usize>();
        assert!(actual <= session.reserved);
        assert_eq!(session.cache.entries.len(), 0);
        session.abort_layout();
        assert_eq!(session.reserved, 0);
        assert!(session.candidates.is_empty());
        drop(session);
        assert_eq!(cache.stats().0, 0);
        assert!(cache.stats().4 <= CACHE_BYTES);
    }
}

#[cfg(test)]
mod header_tests;
