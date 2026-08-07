//! Adapter-private canonical content block index allocation.
//!
//! Provider streams identify output blocks with provider-owned indexes that
//! may include provider-only blocks, fallback markers, different content-part
//! layers, tool indexes, and reasoning indexes. Those indexes are never
//! reused as canonical [`ContentBlockIndex`] values. This allocator maps a
//! provider block identity to a canonical index, allocating indexes
//! sequentially in the order canonical output blocks first appear.

use std::collections::HashMap;
use std::hash::Hash;

use crate::message::types::ContentBlockIndex;

/// Allocates canonical [`ContentBlockIndex`] values for provider block keys.
///
/// The first appearance of a key allocates the next sequential canonical
/// index; later appearances of the same key return the same index. Keys are
/// adapter-local (for example provider block indexes or output-item
/// coordinates) and never leave the adapter boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockAllocator<K>
where
    K: Eq + Hash,
{
    map: HashMap<K, ContentBlockIndex>,
    next: u32,
}

impl<K> Default for BlockAllocator<K>
where
    K: Eq + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K> BlockAllocator<K>
where
    K: Eq + Hash,
{
    /// Creates an empty allocator whose first allocation is index 0.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            next: 0,
        }
    }

    /// Returns the canonical index for the provider key, allocating the next
    /// sequential index on first appearance.
    #[must_use]
    pub fn allocate(&mut self, key: K) -> ContentBlockIndex {
        *self.map.entry(key).or_insert_with(|| {
            let index = ContentBlockIndex::new(self.next);
            self.next += 1;
            index
        })
    }

    /// The canonical index the next allocation would produce.
    #[must_use]
    pub fn peek_next(&self) -> ContentBlockIndex {
        ContentBlockIndex::new(self.next)
    }

    /// Number of canonical indexes allocated so far.
    #[must_use]
    pub fn allocated_count(&self) -> u32 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::BlockAllocator;
    use crate::message::types::ContentBlockIndex;

    /// Allocation is sequential in first-appearance order.
    #[test]
    fn allocates_sequentially_on_first_appearance() {
        let mut allocator = BlockAllocator::new();
        assert_eq!(allocator.allocate(7u32), ContentBlockIndex::new(0));
        assert_eq!(allocator.allocate(3u32), ContentBlockIndex::new(1));
        assert_eq!(allocator.allocate(9u32), ContentBlockIndex::new(2));
    }

    /// The same provider key always maps to the same canonical index.
    #[test]
    fn provider_keys_stay_stable() {
        let mut allocator = BlockAllocator::new();
        let first = allocator.allocate(4u32);
        assert_eq!(allocator.allocate(5u32), ContentBlockIndex::new(1));
        assert_eq!(allocator.allocate(4u32), first);
        assert_eq!(allocator.allocate(5u32), ContentBlockIndex::new(1));
    }

    /// Sparse or skipped provider keys (for example Anthropic fallback
    /// blocks) never consume canonical indexes: only allocated keys do.
    #[test]
    fn skipped_provider_indexes_do_not_shift_canonical_indexes() {
        let mut allocator = BlockAllocator::new();
        assert_eq!(allocator.allocate(0u32), ContentBlockIndex::new(0));
        // Provider index 1 (a fallback block) is never allocated.
        assert_eq!(allocator.allocate(2u32), ContentBlockIndex::new(1));
        assert_eq!(allocator.allocate(0u32), ContentBlockIndex::new(0));
    }

    /// Composite provider keys (output item coordinates) stay distinct per
    /// coordinate while a single-item key collapses its parts.
    #[test]
    fn composite_keys_are_distinct_per_coordinate() {
        let mut allocator = BlockAllocator::new();
        let message_text = (1u32, 0u32);
        let message_refusal = (1u32, 1u32);
        assert_eq!(allocator.allocate(message_text), ContentBlockIndex::new(0));
        assert_eq!(
            allocator.allocate(message_refusal),
            ContentBlockIndex::new(1)
        );
        assert_eq!(allocator.allocate(message_text), ContentBlockIndex::new(0));
    }

    /// `peek_next` reports the next unallocated index without allocating.
    #[test]
    fn peek_next_does_not_allocate() {
        let mut allocator = BlockAllocator::new();
        assert_eq!(allocator.peek_next(), ContentBlockIndex::new(0));
        let _ = allocator.allocate(1u32);
        assert_eq!(allocator.peek_next(), ContentBlockIndex::new(1));
        assert_eq!(allocator.allocated_count(), 1);
    }
}
