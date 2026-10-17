//! Admission belongs to one installed finite Trace record head, not its cut.
use super::{CHECKPOINT_PAGE_ITEMS, Capture, prepare_capture_item};
use crate::runtime_client::trace::TraceRecord;
use std::sync::{Arc, OnceLock};

#[derive(Debug)]
struct PreparedTraceRecords {
    records: Vec<(TraceRecord, usize, usize)>,
    total: u64,
    invalid_head: bool,
}
impl PreparedTraceRecords {
    fn new(records: &[TraceRecord]) -> Self {
        let mut prepared = Self {
            records: Vec::new(),
            total: records.len() as u64,
            invalid_head: false,
        };
        for record in records.iter().take(CHECKPOINT_PAGE_ITEMS) {
            let Some(row) = prepare_capture_item(record) else {
                prepared.invalid_head = true;
                break;
            };
            prepared.records.push(row);
        }
        prepared
    }
}

/// The Projection replaces this owner only when installed, live-repaired rows
/// change. First capture prepares off-lock from the frozen seed; stamping the
/// page's read cut cannot alter these record admission certificates.
#[derive(Debug, Clone, Default)]
pub(crate) struct PreparedTraceHead(Arc<OnceLock<PreparedTraceRecords>>);
impl PreparedTraceHead {
    pub(crate) fn capture(
        &self,
        records: &[TraceRecord],
        capture: &mut Capture,
    ) -> Vec<TraceRecord> {
        let prepared = self.0.get_or_init(|| PreparedTraceRecords::new(records));
        let mut result = Vec::new();
        for (record, bytes, required) in &prepared.records {
            if !capture.admit_prepared(*bytes, *required) {
                break;
            }
            result.push(record.clone());
        }
        if result.len() == prepared.records.len() && prepared.invalid_head {
            capture.evicted = true;
        }
        let loaded = result.len() as u64;
        capture.frontier(
            "trace.records",
            prepared.total,
            loaded,
            (loaded < prepared.total).then_some(loaded),
        );
        result
    }
    #[cfg(test)]
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    #[cfg(test)]
    pub(crate) fn prepared(&self) -> bool {
        self.0.get().is_some()
    }
}
