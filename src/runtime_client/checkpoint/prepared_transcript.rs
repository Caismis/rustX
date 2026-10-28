//! A complete, immutable transcript row owns its normalization and admission.
//! Native readers may share an owner only after proving both body and decoration.
use super::{CHECKPOINT_PAGE_ITEMS, CHECKPOINT_TARGET_BYTES, Capture, PreparedCapture};
use crate::runtime::identity::ConversationId;
use crate::runtime_client::snapshot::RuntimeClientTranscriptEntry;
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

type CapturedRow = Arc<RuntimeClientTranscriptEntry>;
type CapturedRows = Vec<CapturedRow>;

struct Row {
    conversation: ConversationId,
    value: RuntimeClientTranscriptEntry,
    prepared: OnceLock<PreparedCapture<Option<CapturedRow>>>,
}

/// Internal evidence, never part of the public page or its wire schema.
#[derive(Clone)]
pub(crate) struct PreparedTranscriptRow(Arc<Row>);
impl std::fmt::Debug for PreparedTranscriptRow {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_tuple("PreparedTranscriptRow")
            .field(&self.0.value)
            .finish()
    }
}
impl PartialEq for PreparedTranscriptRow {
    fn eq(&self, other: &Self) -> bool {
        self.same_owner(other)
            || (self.0.conversation == other.0.conversation && self.0.value == other.0.value)
    }
}
impl PreparedTranscriptRow {
    pub(crate) fn new(conversation: ConversationId, value: RuntimeClientTranscriptEntry) -> Self {
        Self(Arc::new(Row {
            conversation,
            value,
            prepared: OnceLock::new(),
        }))
    }
    #[cfg(test)]
    pub(crate) fn value(&self) -> &RuntimeClientTranscriptEntry {
        &self.0.value
    }
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    fn capture(&self, capture: &mut Capture, index: usize) -> (Option<CapturedRow>, Option<usize>) {
        let content = capture.content.len();
        let collections = capture.collections.len();
        let (value, fit) = capture.prepared(&self.0.prepared, |capture| {
            #[cfg(test)]
            WORK.with(|work| work.set(work.get() + 1));
            capture
                .transcript_row(&self.0.value, &self.0.conversation, 0)
                .map(Arc::new)
        });
        // Position belongs to the page, not the immutable row. Generated row
        // metadata never participates in the row-body admission charge.
        if index != 0 {
            #[cfg(test)]
            record_page_work(3);
            let index = index.to_string();
            for binding in &mut capture.content[content..] {
                let mut value = (**binding).clone();
                debug_assert_eq!(&value.path[..3], &["transcript", "entries", "0"]);
                value.path[2].clone_from(&index);
                *binding = value.into();
            }
            for frontier in &mut capture.collections[collections..] {
                for separator in ["/", "."] {
                    let prefix = ["transcript", "entries", "0"].join(separator);
                    if let Some(suffix) = frontier.domain.strip_prefix(&prefix)
                        && (suffix.is_empty() || suffix.starts_with(separator))
                    {
                        frontier.domain =
                            format!("transcript{separator}entries{separator}{index}{suffix}");
                        break;
                    }
                }
            }
        }
        // `Some(row)` adds one conservative delimiter to the row's own charge.
        (value.into_owned(), fit.and_then(|fit| fit.checked_sub(1)))
    }
}

#[derive(Default)]
struct Head {
    rows: Vec<PreparedTranscriptRow>,
    total: usize,
    prepared: OnceLock<PreparedCapture<CapturedRows>>,
}

/// A fixed ordered set of immutable row owners. Only this head's admission and
/// positional metadata are retained; it never points to an older head.
#[derive(Clone, Default)]
pub(crate) struct PreparedTranscriptHead(Arc<Head>);
impl std::fmt::Debug for PreparedTranscriptHead {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("PreparedTranscriptHead")
            .field("rows", &self.0.rows)
            .field("total", &self.0.total)
            .finish()
    }
}
impl PartialEq for PreparedTranscriptHead {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || (self.0.total == other.0.total && self.0.rows == other.0.rows)
    }
}
impl PreparedTranscriptHead {
    pub(crate) fn new(rows: Vec<PreparedTranscriptRow>) -> Self {
        let total = rows.len();
        Self(Arc::new(Head {
            rows: rows.into_iter().take(CHECKPOINT_PAGE_ITEMS).collect(),
            total,
            prepared: OnceLock::new(),
        }))
    }
    pub(crate) fn from_entries(
        conversation: &ConversationId,
        entries: &[RuntimeClientTranscriptEntry],
    ) -> Self {
        Self(Arc::new(Head {
            rows: entries
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .cloned()
                .map(|entry| PreparedTranscriptRow::new(conversation.clone(), entry))
                .collect(),
            total: entries.len(),
            prepared: OnceLock::new(),
        }))
    }
    pub(crate) fn capture<'a>(
        &'a self,
        capture: &mut Capture,
    ) -> (CapturedTranscript<'a>, Option<usize>) {
        let prepared = self.0.prepared.get_or_init(|| {
            #[cfg(test)]
            record_page_work(0);
            let mut capture = Capture::new(0);
            // Compose the array charge from the row certificates. Running the
            // generic prepared builder here would traverse every row again.
            let (value, fit) = self.capture_rows(&mut capture);
            PreparedCapture {
                value,
                capture,
                fit,
            }
        });
        if !capture.try_replay(&prepared.capture) {
            #[cfg(test)]
            record_page_work(1);
            let (value, fit) = self.capture_rows(capture);
            return (CapturedTranscript(Cow::Owned(value)), fit);
        }
        #[cfg(test)]
        record_page_work(2);
        (
            CapturedTranscript(Cow::Borrowed(&prepared.value)),
            prepared.fit,
        )
    }

    fn capture_rows(&self, capture: &mut Capture) -> (CapturedRows, Option<usize>) {
        let mut entries = Vec::new();
        let mut budget = Some(2usize);
        for (index, row) in self.0.rows.iter().enumerate() {
            let (value, fit) = row.capture(capture, index);
            let Some(value) = value else { break };
            // Same conservative sequence charge as preflight's Check: length
            // plus one delimiter per member, in addition to the member charge.
            budget = budget
                .zip(fit)
                .and_then(|(sum, item)| sum.checked_add(item)?.checked_add(2));
            entries.push(value);
        }
        let loaded = entries.len() as u64;
        capture.frontier(
            "transcript.entries",
            self.0.total as u64,
            loaded,
            (entries.len() < self.0.total).then_some(loaded),
        );
        (
            entries,
            budget.filter(|budget| *budget <= CHECKPOINT_TARGET_BYTES),
        )
    }
    /// Called only when installing a freshly proven page. Scalar cardinality
    /// and pointer identity in page order suffice; no body comparison or hash.
    pub(crate) fn replace(&mut self, next: Self) {
        if !self.same_owners(&next) {
            *self = next;
        }
    }
    pub(crate) fn same_owners(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || (self.0.total == other.0.total
                && self.0.rows.len() == other.0.rows.len()
                && self
                    .0
                    .rows
                    .iter()
                    .zip(&other.0.rows)
                    .all(|(left, right)| left.same_owner(right)))
    }
}

pub(crate) struct CapturedTranscript<'a>(Cow<'a, CapturedRows>);
impl CapturedTranscript<'_> {
    pub(crate) fn into_owned(self) -> Vec<RuntimeClientTranscriptEntry> {
        match self.0 {
            Cow::Borrowed(rows) => rows.iter().map(|row| row.as_ref().clone()).collect(),
            Cow::Owned(rows) => rows.into_iter().map(Arc::unwrap_or_clone).collect(),
        }
    }
}

#[cfg(test)]
thread_local! { static WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_work() -> usize {
    WORK.with(|work| work.replace(0))
}

#[cfg(test)]
// Page builds, budget-dependent rebuilds, page replays, row reindex visits.
thread_local! { static PAGE_WORK: std::cell::Cell<[usize; 4]> = const { std::cell::Cell::new([0; 4]) }; }
#[cfg(test)]
fn record_page_work(index: usize) {
    PAGE_WORK.with(|work| {
        let mut value = work.get();
        value[index] += 1;
        work.set(value);
    });
}
#[cfg(test)]
fn take_page_work() -> [usize; 4] {
    PAGE_WORK.with(|work| work.replace([0; 4]))
}

#[cfg(test)]
#[path = "prepared_transcript_tests.rs"]
mod tests;
