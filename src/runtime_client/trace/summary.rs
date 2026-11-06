//! Optional historical relationships over native frozen summary headers.
use super::TraceProjection;
use crate::durable::ConversationStoreError;
use crate::durable::observation::{
    RequestReadSummary, TraceRequestContextPage, TraceRequestPredecessorHeader,
};
use crate::observation::trace::types::{
    TraceRequestPredecessor, TraceSystemPromptPresentation, TraceSystemPromptState,
    TraceToolCatalogState,
};

impl TraceProjection<'_> {
    pub(super) fn previous_request(
        &self,
        start: u64,
    ) -> Result<PreviousRequest, ConversationStoreError> {
        #[cfg(test)]
        probe::record(&probe::SYSTEM_PREDECESSOR);
        self.store
            .observation_request_predecessor_at(start, self.through)
            .map(PreviousRequest)
    }
    pub(super) fn context_presentation(
        &self,
        start: u64,
    ) -> Result<TraceRequestContextPage, ConversationStoreError> {
        let mut page = self.store.observation_request_context_at(
            self.cut()?,
            start,
            0,
            crate::observation::trace::bounds::TRACE_SUMMARY_CONTEXT,
        )?;
        #[cfg(test)]
        if !page.items.is_empty() {
            probe::record(&probe::CONTEXT_LEDGER_JOIN);
        }
        while crate::observation::trace::bounds::encoded_len(&page.items)
            > crate::observation::trace::bounds::TRACE_SUMMARY_CONTEXT_BYTES
        {
            page.items.pop();
            page.next_offset = Some(page.items.len() as u64);
        }
        Ok(page)
    }
}
pub(super) fn record_tool_proposal() {
    #[cfg(test)]
    probe::record(&probe::TOOL_PROPOSAL);
}
pub(super) struct PreviousRequest(Option<TraceRequestPredecessorHeader>);
impl PreviousRequest {
    pub(super) fn identity(&self) -> TraceRequestPredecessor {
        match &self.0 {
            None => TraceRequestPredecessor::NotApplicable,
            Some(previous) if previous.summary.is_some() => TraceRequestPredecessor::Available {
                request_id: previous.request_id.clone(),
                request_id_source: previous.source.clone(),
            },
            Some(previous) => TraceRequestPredecessor::Unavailable {
                request_id: previous.request_id.clone(),
                request_id_source: previous.source.clone(),
            },
        }
    }
    pub(super) fn presentation(
        &self,
        current: &RequestReadSummary,
    ) -> (TraceSystemPromptPresentation, TraceToolCatalogState) {
        #[cfg(test)]
        probe::record(&probe::TOOL_RELATIONSHIP);
        let (state, tools) = match &self.0 {
            None => (
                TraceSystemPromptState::Initial,
                TraceToolCatalogState::Initial,
            ),
            Some(previous) => match &previous.summary {
                None => (
                    TraceSystemPromptState::PreviousUnavailable,
                    TraceToolCatalogState::PreviousUnavailable,
                ),
                Some(previous) => (
                    if previous.prompt_digest == current.prompt_digest {
                        TraceSystemPromptState::Unchanged
                    } else {
                        TraceSystemPromptState::Changed
                    },
                    if previous.tools_digest == current.tools_digest {
                        TraceToolCatalogState::Unchanged
                    } else {
                        TraceToolCatalogState::Changed
                    },
                ),
            },
        };
        (
            TraceSystemPromptPresentation {
                state,
                preview: (state != TraceSystemPromptState::Unchanged)
                    .then(|| current.prompt_preview.clone()),
            },
            tools,
        )
    }
}

/// Deterministic counters proving which read paths a projection took.
///
/// A regression that asserts a lifecycle refresh is cheap by timing it is
/// not a regression. These counters make the ownership rule an observable
/// implementation property instead: the immutable relationship paths
/// increment them, and a refresh must leave them at zero.
#[cfg(test)]
pub(super) mod probe {
    use std::cell::Cell;

    thread_local! {
        pub(in crate::runtime_client::trace) static TOOL_PROPOSAL: Cell<u32> = const { Cell::new(0) };
        /// Resolutions of the System Prompt predecessor presentation.
        pub(in crate::runtime_client::trace) static SYSTEM_PREDECESSOR: Cell<u32> =
            const { Cell::new(0) };
        pub(in crate::runtime_client::trace) static TOOL_RELATIONSHIP: Cell<u32> = const { Cell::new(0) };
        /// Canonical Context presentation joins against the Message Ledger.
        pub(in crate::runtime_client::trace) static CONTEXT_LEDGER_JOIN: Cell<u32> =
            const { Cell::new(0) };
    }

    pub(in crate::runtime_client::trace) fn record(
        counter: &'static std::thread::LocalKey<Cell<u32>>,
    ) {
        counter.with(|count| count.set(count.get().saturating_add(1)));
    }

    /// Both counters, as (System predecessor, Context Ledger join).
    #[must_use]
    pub(in crate::runtime_client::trace) fn counts() -> (u32, u32) {
        (
            SYSTEM_PREDECESSOR.with(Cell::get),
            CONTEXT_LEDGER_JOIN.with(Cell::get),
        )
    }

    pub(in crate::runtime_client::trace) fn tool_count() -> u32 {
        TOOL_RELATIONSHIP.with(Cell::get)
    }

    pub(in crate::runtime_client::trace) fn proposal_count() -> u32 {
        TOOL_PROPOSAL.with(Cell::get)
    }

    /// Zeroes both counters so the next call is measured on its own.
    pub(in crate::runtime_client::trace) fn reset() {
        TOOL_PROPOSAL.with(|count| count.set(0));
        SYSTEM_PREDECESSOR.with(|count| count.set(0));
        TOOL_RELATIONSHIP.with(|count| count.set(0));
        CONTEXT_LEDGER_JOIN.with(|count| count.set(0));
    }
}
