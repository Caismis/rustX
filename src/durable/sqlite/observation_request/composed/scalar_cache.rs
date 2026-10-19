//! Derived public Tools facts; no native identity, root, lease or authorization.
//! Every fact is freshly produced and compared before its spool write is reused.
use super::{ConversationStoreError, storage};
use crate::runtime::identity::ConversationId;
use std::sync::{Arc, Mutex};

const MAX_BYTES: usize = 1_048_576;
const MAX_ROWS: usize = 2048;
const MAX_SCOPE: usize = 256;
type Row = (String, Vec<u8>);

#[derive(Default)]
pub(in crate::durable::sqlite) struct Owner {
    catalog: Mutex<Option<Arc<Catalog>>>,
}
struct Catalog {
    scope: ConversationId,
    start: u64,
    end: u64,
    bytes: Vec<u8>,
    rows: Vec<Row>,
    owned: usize,
}
pub(in crate::durable::sqlite) struct Session {
    previous: Option<Arc<Catalog>>,
    pending: Option<Catalog>,
    #[cfg(test)]
    reused: usize,
}
impl Owner {
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn retained_work(&self) -> Option<(usize, usize)> {
        self.catalog
            .lock()
            .ok()
            .and_then(|entry| entry.as_ref().map(|entry| (entry.rows.len(), entry.owned)))
    }
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn retained_bytes(&self) -> Option<Vec<u8>> {
        self.catalog
            .lock()
            .ok()
            .and_then(|entry| entry.as_ref().map(|entry| entry.bytes.clone()))
    }
    pub(in crate::durable::sqlite) fn session(&self, scope: &ConversationId) -> Session {
        let previous = self.catalog.lock().ok().and_then(|entry| {
            entry
                .as_ref()
                .filter(|entry| &entry.scope == scope)
                .cloned()
        });
        Session {
            previous,
            pending: None,
            #[cfg(test)]
            reused: 0,
        }
    }
}
impl Session {
    pub(in crate::durable::sqlite) fn discard(&mut self) {
        self.pending = None;
    }
    // Call only after the complete optional layout and native transaction commit.
    pub(in crate::durable::sqlite) fn commit(self, owner: &Owner) {
        if let Some(catalog) = self.pending
            && let Ok(mut entry) = owner.catalog.lock()
        {
            *entry = Some(Arc::new(catalog));
        }
    }
    #[cfg(test)]
    pub(super) fn work(&self) -> (usize, usize, usize) {
        (
            self.reused,
            self.pending.as_ref().map_or(0, |entry| entry.rows.len()),
            self.pending.as_ref().map_or(0, |entry| entry.owned),
        )
    }
    pub(super) fn capture<'a>(
        &'a mut self,
        scope: &ConversationId,
        start: u64,
        end: u64,
        bytes: Vec<u8>,
    ) -> Capture<'a> {
        self.pending = None;
        #[cfg(test)]
        {
            self.reused = 0;
        }
        let hit = self.previous.as_deref().filter(|entry| {
            &entry.scope == scope
                && entry.start == start
                && entry.end == end
                && entry.bytes == bytes
        });
        let pending = if hit.is_none() && scope.as_str().len() <= MAX_SCOPE {
            let rows = Vec::with_capacity(MAX_ROWS);
            let owned = std::mem::size_of::<Catalog>()
                + 2 * std::mem::size_of::<usize>()
                + scope.as_str().len()
                + bytes.capacity()
                + rows.capacity() * std::mem::size_of::<Row>();
            (owned <= MAX_BYTES).then(|| Catalog {
                scope: scope.clone(),
                start,
                end,
                bytes,
                rows,
                owned,
            })
        } else {
            None
        };
        Capture {
            hit,
            pending,
            seen: 0,
            visited: [0; MAX_ROWS / 64],
            #[cfg(test)]
            reused: &mut self.reused,
            result: &mut self.pending,
        }
    }
}
pub(super) struct Capture<'a> {
    hit: Option<&'a Catalog>,
    pending: Option<Catalog>,
    seen: usize,
    visited: [u64; MAX_ROWS / 64],
    #[cfg(test)]
    reused: &'a mut usize,
    result: &'a mut Option<Catalog>,
}
fn tools_row(key: &str, value: &[u8]) -> bool {
    key.starts_with("F[\"tools\"")
        || key.starts_with("C[\"tools\"")
        || key.starts_with("C[]:") && value == b"[\"tools\"]"
}
impl Capture<'_> {
    // Fresh metadata/preview/bounds checks and encoding precede this call.
    pub(super) fn reuse(
        &mut self,
        key: &str,
        value: &[u8],
    ) -> Result<bool, ConversationStoreError> {
        if !tools_row(key, value) {
            return Ok(false);
        }
        if let Some(entry) = self.hit {
            let index = entry
                .rows
                .binary_search_by(|row| row.0.as_str().cmp(key))
                .map_err(|_| storage("fresh public catalog fact is not retained"))?;
            let bit = 1_u64 << (index % 64);
            if entry.rows[index].1 != value || self.visited[index / 64] & bit != 0 {
                return Err(storage("fresh public catalog fact differs or repeats"));
            }
            self.visited[index / 64] |= bit;
            self.seen += 1;
            #[cfg(test)]
            {
                *self.reused += 1;
            }
            return Ok(true);
        }
        let mut overflow = false;
        if let Some(entry) = self.pending.as_mut() {
            if entry.rows.len() == MAX_ROWS {
                overflow = true;
            } else {
                let row = (key.to_owned(), value.to_vec());
                let size = row.0.capacity().saturating_add(row.1.capacity());
                if let Some(owned) = entry.owned.checked_add(size).filter(|n| *n <= MAX_BYTES) {
                    entry.owned = owned;
                    entry.rows.push(row);
                } else {
                    overflow = true;
                }
            }
        }
        if overflow {
            self.pending = None;
        }
        Ok(false)
    }
    pub(super) fn rows(&self) -> Result<&[Row], ConversationStoreError> {
        if let Some(entry) = self.hit {
            if self.seen != entry.rows.len() {
                return Err(storage("fresh public catalog fact count differs"));
            }
            Ok(&entry.rows)
        } else {
            Ok(&[])
        }
    }
    // Only complete successful scalar capture can become a prospective entry.
    pub(super) fn finish(mut self) -> Result<(), ConversationStoreError> {
        self.rows()?;
        if let Some(mut entry) = self.pending.take() {
            entry.rows.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            if entry.rows.windows(2).any(|rows| rows[0].0 == rows[1].0) {
                return Err(storage("public catalog fact repeats"));
            }
            *self.result = Some(entry);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> ConversationId {
        ConversationId::generate()
    }
    fn learn(session: &mut Session, scope: &ConversationId) {
        let mut capture = session.capture(scope, 10, 12, b"[]".to_vec());
        assert!(
            !capture
                .reuse("F[\"tools\"]", b"fresh encoded field")
                .unwrap()
        );
        assert!(
            !capture
                .reuse("C[]:00000000000000000001", b"[\"tools\"]")
                .unwrap()
        );
        capture.finish().unwrap();
    }
    #[test]
    fn retained_rows_require_committed_owner_scope_exact_bytes_and_geometry() {
        let owner = Owner::default();
        let scope = scope();
        let mut initial = owner.session(&scope);
        learn(&mut initial, &scope);
        assert!(owner.catalog.lock().unwrap().is_none());
        initial.commit(&owner);
        for (start, end, bytes, same_scope) in [
            (10, 12, b"[]".as_slice(), true),
            (11, 13, b"[]".as_slice(), true),
            (10, 12, b"{}".as_slice(), true),
            (10, 12, b"[]".as_slice(), false),
        ] {
            let current_scope = if same_scope {
                scope.clone()
            } else {
                self::scope()
            };
            let mut session = owner.session(&current_scope);
            let mut capture = session.capture(&current_scope, start, end, bytes.to_vec());
            let hit = start == 10 && bytes == b"[]" && same_scope;
            assert_eq!(
                capture
                    .reuse("F[\"tools\"]", b"fresh encoded field")
                    .unwrap(),
                hit
            );
            assert_eq!(
                capture
                    .reuse("C[]:00000000000000000001", b"[\"tools\"]")
                    .unwrap(),
                hit
            );
            capture.finish().unwrap();
            assert_eq!(session.reused, usize::from(hit) * 2);
        }
        let mut different_owner = Owner::default().session(&scope);
        learn(&mut different_owner, &scope);
        assert_eq!(different_owner.reused, 0);
    }
    #[test]
    fn discarded_or_failed_capture_never_replaces_the_committed_entry() {
        let owner = Owner::default();
        let scope = scope();
        let mut discarded = owner.session(&scope);
        learn(&mut discarded, &scope);
        discarded.discard();
        discarded.commit(&owner);
        assert!(owner.catalog.lock().unwrap().is_none());
        let mut aborted = owner.session(&scope);
        let mut capture = aborted.capture(&scope, 10, 12, b"[]".to_vec());
        capture.reuse("F[\"tools\"]", b"encoded").unwrap();
        drop(capture); // Failure before complete scalar capture.
        aborted.commit(&owner);
        assert!(owner.catalog.lock().unwrap().is_none());
    }
    #[test]
    fn hit_rejects_changed_repeated_missing_and_extra_fresh_facts() {
        let owner = Owner::default();
        let scope = scope();
        let mut initial = owner.session(&scope);
        learn(&mut initial, &scope);
        initial.commit(&owner);
        for mode in 0..4 {
            let mut session = owner.session(&scope);
            let mut capture = session.capture(&scope, 10, 12, b"[]".to_vec());
            match mode {
                0 => assert!(capture.reuse("F[\"tools\"]", b"changed").is_err()),
                1 => {
                    assert!(
                        capture
                            .reuse("F[\"tools\"]", b"fresh encoded field")
                            .unwrap()
                    );
                    assert!(
                        capture
                            .reuse("F[\"tools\"]", b"fresh encoded field")
                            .is_err()
                    );
                }
                2 => assert!(capture.rows().is_err()),
                _ => assert!(capture.reuse("F[\"tools\",\"extra\"]", b"extra").is_err()),
            }
        }
    }
    #[test]
    fn capacity_count_and_owned_buffers_bound_catalog_retention() {
        let owner = Owner::default();
        let scope = scope();
        for count_bound in [false, true] {
            let mut session = owner.session(&scope);
            let mut capture = session.capture(&scope, 10, 12, b"[]".to_vec());
            for ordinal in 0..=MAX_ROWS {
                let bytes = vec![b'x'; if count_bound { 1 } else { 40_960 }];
                capture
                    .reuse(&format!("F[\"tools\",\"{ordinal}\"]"), &bytes)
                    .unwrap();
                if let Some(entry) = &capture.pending {
                    assert!(entry.owned <= MAX_BYTES);
                    assert_eq!(entry.rows.capacity(), MAX_ROWS);
                } else {
                    break;
                }
            }
            assert!(capture.pending.is_none());
            capture.finish().unwrap();
            session.commit(&owner);
            assert!(owner.catalog.lock().unwrap().is_none());
        }
    }
}
