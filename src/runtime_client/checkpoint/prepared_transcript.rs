//! Immutable bodies retain normalization while native response facts stay current.
//! Complete row ownership additionally requires identical proven decoration.
use super::{CHECKPOINT_PAGE_ITEMS, CHECKPOINT_TARGET_BYTES, Capture, PreparedCapture};
use crate::runtime::identity::ConversationId;
use crate::runtime_client::snapshot::RuntimeClientTranscriptEntry;
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

mod body;

type CapturedRow = Arc<body::Captured>;
type CapturedRows = Vec<CapturedRow>;

struct Row {
    body: Arc<body::Owner>,
    decoration: body::Decoration,
    prepared: OnceLock<PreparedCapture<Option<CapturedRow>>>,
}

/// Internal evidence, never part of the public page or its wire schema.
#[derive(Clone)]
pub(crate) struct PreparedTranscriptRow(Arc<Row>);
impl std::fmt::Debug for PreparedTranscriptRow {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_tuple("PreparedTranscriptRow")
            .field(&self.0.body)
            .field(&self.0.decoration)
            .finish()
    }
}
impl PartialEq for PreparedTranscriptRow {
    fn eq(&self, other: &Self) -> bool {
        self.same_owner(other)
            || (self.0.body.equal(&other.0.body) && self.0.decoration == other.0.decoration)
    }
}
impl PreparedTranscriptRow {
    pub(crate) fn new(conversation: ConversationId, value: RuntimeClientTranscriptEntry) -> Self {
        let (body, decoration) = body::Owner::new(conversation, value);
        Self(Arc::new(Row {
            body,
            decoration,
            prepared: OnceLock::new(),
        }))
    }
    #[cfg(test)]
    pub(crate) fn value(&self) -> RuntimeClientTranscriptEntry {
        self.0.body.entry(&self.0.decoration)
    }
    /// Materialize the descriptor body after its exact native bytes are read
    /// again. Tool and response decoration must still come from the current read.
    pub(crate) fn undecorated_value(&self) -> RuntimeClientTranscriptEntry {
        self.0.body.undecorated_entry()
    }
    /// Called only after a native reader proved the current body bytes again.
    /// Current response decoration never inherits the older owner's authority.
    pub(crate) fn with_decoration(&self, value: &RuntimeClientTranscriptEntry) -> Self {
        Self(Arc::new(Row {
            body: self.0.body.clone(),
            decoration: body::Decoration::from_entry(value),
            prepared: OnceLock::new(),
        }))
    }
    #[cfg(test)]
    pub(crate) fn same_body(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0.body, &other.0.body)
    }
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    fn capture(&self, capture: &mut Capture, index: usize) -> (Option<CapturedRow>, Option<usize>) {
        let content = capture.content.len();
        let collections = capture.collections.len();
        let build = |capture: &mut Capture| {
            #[cfg(test)]
            WORK.with(|work| work.set(work.get() + 1));
            body::capture(&self.0.body, &self.0.decoration, capture)
        };
        let prepared = self.0.prepared.get_or_init(|| {
            let mut capture = Capture::new(0);
            let (value, fit) = build(&mut capture);
            PreparedCapture {
                value,
                capture,
                fit,
            }
        });
        let (value, fit) = if capture.try_replay(&prepared.capture) {
            (prepared.value.clone(), prepared.fit)
        } else {
            build(capture)
        };
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
        (value, fit.and_then(|fit| fit.checked_sub(1)))
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
            Cow::Borrowed(rows) => rows.iter().map(|row| row.entry()).collect(),
            Cow::Owned(rows) => rows
                .into_iter()
                .map(|row| Arc::unwrap_or_clone(row).into_entry())
                .collect(),
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
