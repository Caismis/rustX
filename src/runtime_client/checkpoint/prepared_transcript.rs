//! A complete, immutable transcript row owns its normalization and admission.
//! Native readers may share an owner only after proving both body and decoration.
use super::{CHECKPOINT_PAGE_ITEMS, CHECKPOINT_TARGET_BYTES, Capture, PreparedCapture};
use crate::runtime::identity::ConversationId;
use crate::runtime_client::snapshot::RuntimeClientTranscriptEntry;
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

struct Row {
    conversation: ConversationId,
    value: RuntimeClientTranscriptEntry,
    prepared: OnceLock<PreparedCapture<Option<RuntimeClientTranscriptEntry>>>,
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
    fn capture(
        &self,
        capture: &mut Capture,
        index: usize,
    ) -> (Option<Cow<'_, RuntimeClientTranscriptEntry>>, Option<usize>) {
        let content = capture.content.len();
        let collections = capture.collections.len();
        let (value, fit) = capture.prepared(&self.0.prepared, |capture| {
            #[cfg(test)]
            WORK.with(|work| work.set(work.get() + 1));
            capture.transcript_row(&self.0.value, &self.0.conversation, 0)
        });
        // Position belongs to the page, not the immutable row. Generated row
        // metadata never participates in the row-body admission charge.
        if index != 0 {
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
        let value = match value {
            Cow::Borrowed(value) => value.as_ref().map(Cow::Borrowed),
            Cow::Owned(value) => value.map(Cow::Owned),
        };
        // `Some(row)` adds one conservative delimiter to the row's own charge.
        (value, fit.and_then(|fit| fit.checked_sub(1)))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PreparedTranscriptHead {
    rows: Vec<PreparedTranscriptRow>,
    total: usize,
}
impl PreparedTranscriptHead {
    pub(crate) fn new(rows: Vec<PreparedTranscriptRow>) -> Self {
        let total = rows.len();
        Self {
            rows: rows.into_iter().take(CHECKPOINT_PAGE_ITEMS).collect(),
            total,
        }
    }
    pub(crate) fn from_entries(
        conversation: &ConversationId,
        entries: &[RuntimeClientTranscriptEntry],
    ) -> Self {
        Self {
            rows: entries
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .cloned()
                .map(|entry| PreparedTranscriptRow::new(conversation.clone(), entry))
                .collect(),
            total: entries.len(),
        }
    }
    pub(crate) fn capture<'a>(
        &'a self,
        capture: &mut Capture,
    ) -> (CapturedTranscript<'a>, Option<usize>) {
        let mut entries = Vec::new();
        let mut budget = Some(2usize);
        for (index, row) in self.rows.iter().enumerate() {
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
            self.total as u64,
            loaded,
            (entries.len() < self.total).then_some(loaded),
        );
        (
            CapturedTranscript(entries),
            budget.filter(|budget| *budget <= CHECKPOINT_TARGET_BYTES),
        )
    }
    #[cfg(test)]
    pub(crate) fn same_owners(&self, other: &Self) -> bool {
        self.total == other.total
            && self.rows.len() == other.rows.len()
            && self
                .rows
                .iter()
                .zip(&other.rows)
                .all(|(left, right)| left.same_owner(right))
    }
}

pub(crate) struct CapturedTranscript<'a>(Vec<Cow<'a, RuntimeClientTranscriptEntry>>);
impl CapturedTranscript<'_> {
    pub(crate) fn into_owned(self) -> Vec<RuntimeClientTranscriptEntry> {
        self.0.into_iter().map(Cow::into_owned).collect()
    }
}

#[cfg(test)]
thread_local! { static WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_work() -> usize {
    WORK.with(|work| work.replace(0))
}

#[cfg(test)]
#[path = "prepared_transcript_tests.rs"]
mod tests;
