//! Explicitly activated, scalar-only full-Agent Source attribution.
use super::{WORK, Work, manifest};
use crate::agent::execution::attribution::{self, Phase, SourceScope, SourceWork, Timer};

pub(super) struct Measurement {
    scope: SourceScope,
    before: Option<(Work, (usize, usize))>,
    _timer: Timer,
}
impl Measurement {
    pub(super) fn new(scope: SourceScope) -> Self {
        let before = attribution::active()
            .then(|| (WORK.with(std::cell::Cell::get), manifest::node_read_work()));
        let phase = match scope {
            SourceScope::Request => Phase::Source,
            SourceScope::Authenticate => Phase::SourceAuthenticate,
            SourceScope::Leaf => Phase::SourceLeaf,
        };
        Self {
            scope,
            before,
            _timer: Timer::new(phase),
        }
    }
}
fn difference(current: usize, before: usize) -> u64 {
    u64::try_from(current.saturating_sub(before)).unwrap_or(u64::MAX)
}
impl Drop for Measurement {
    fn drop(&mut self) {
        let Some((before, queries)) = self.before else {
            return;
        };
        let current = WORK.with(std::cell::Cell::get);
        let native = manifest::node_read_work();
        attribution::record_source(
            self.scope,
            SourceWork {
                samples: 1,
                native_header_queries: difference(native.0, queries.0),
                native_node_queries: difference(native.1, queries.1),
                index_pages: difference(current.index_pages, before.index_pages),
                index_bytes: difference(current.index_bytes, before.index_bytes),
                metadata_blobs: difference(current.source_nodes, before.source_nodes),
                metadata_bytes: difference(
                    current.source_metadata_bytes,
                    before.source_metadata_bytes,
                ),
                new_leaves: difference(current.new_leaves, before.new_leaves),
                new_nodes: difference(current.new_nodes, before.new_nodes),
                new_fragment_bytes: difference(
                    current.new_fragment_bytes,
                    before.new_fragment_bytes,
                ),
                header_decodes: difference(current.header_decodes, before.header_decodes),
            },
        );
    }
}
