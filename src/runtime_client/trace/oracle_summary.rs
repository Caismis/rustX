//! Request-relative presentation facts resolved by native authority.
//!
//! Two relationships a Trajectory reader needs are not facts about one
//! record in isolation: whether a request's System Prompt changed, and which
//! canonical Context a request introduced. Both are resolved here, from
//! durable authority, before anything reaches a client.
//!
//! The rule this module exists to enforce: **a presentation relationship is
//! resolved natively, never discovered downstream**. Nothing below reads the
//! current Session configuration, the current System Prompt assembly, the
//! current Surface, message text, message position, or the set of records a
//! client happens to have loaded.
//!
//! ```text
//! System Prompt   nearest preceding actual request, by durable start order
//! Context         RequestSnapshot.request_context_ids + keyed Ledger reads
//! ```
//!
//! Both are *immutable* historical facts, so both belong to the summary
//! projection alone. A lifecycle refresh carries neither and never calls
//! into this module; the counters under `#[cfg(test)]` below exist so a
//! regression can prove that as an implementation property rather than
//! infer it from timing.

use super::TraceProjection;
use super::bounds::{TracePreview, identity_fits};
use super::types::{TraceSystemPromptPresentation, TraceSystemPromptState};
use crate::durable::ConversationStoreError;
use crate::durable::presentation::{FactQuery, FactScope};
use crate::events::types::RuntimeEvent as E;
use crate::model::snapshot::RequestSnapshot;

impl TraceProjection<'_> {
    /// One indexed predecessor seek shared by BOTH immutable comparisons.
    /// `RequestNotFound` is absence; every other durable error propagates.
    pub(super) fn original_previous_request(
        &self,
        anchor_sequence: u64,
    ) -> Result<OriginalPreviousRequest, ConversationStoreError> {
        #[cfg(test)]
        super::summary::probe::record(&super::summary::probe::SYSTEM_PREDECESSOR);
        let Some(previous) = self
            .store
            .read_presentation_events(&FactQuery {
                scope: FactScope::All,
                kinds: vec!["model_request_started"],
                before: Some(anchor_sequence),
                after: 0,
                ascending: false,
                through: self.through,
                limit: 1,
            })?
            .pop()
        else {
            return Ok(OriginalPreviousRequest::None);
        };
        let E::ModelRequestStarted { request_id, .. } = previous.event else {
            return Err(ConversationStoreError::InvalidReference(
                "non-request predecessor".into(),
            ));
        };
        match self.store.load_request_snapshot(&request_id) {
            Ok(snapshot) => Ok(OriginalPreviousRequest::Frozen(Box::new(snapshot))),
            Err(ConversationStoreError::RequestNotFound(_)) => {
                Ok(OriginalPreviousRequest::Unavailable(request_id))
            }
            Err(error) => Err(error),
        }
    }
}

/// The full frozen snapshot is loaded once, never separately per relationship.
pub(super) enum OriginalPreviousRequest {
    None,
    Unavailable(crate::runtime::identity::RequestId),
    Frozen(Box<RequestSnapshot>),
}

impl OriginalPreviousRequest {
    pub(super) fn identity(&self) -> super::types::TraceRequestPredecessor {
        use super::types::TraceRequestPredecessor as P;
        match self {
            Self::None => P::NotApplicable,
            Self::Unavailable(id) => P::Unavailable {
                request_id: identity_fits(id.as_str()).then(|| id.clone()),
                request_id_source: None,
            },
            Self::Frozen(snapshot) if identity_fits(snapshot.request_id.as_str()) => P::Available {
                request_id: Some(snapshot.request_id.clone()),
                request_id_source: None,
            },
            Self::Frozen(_) => P::Unavailable {
                request_id: None,
                request_id_source: None,
            },
        }
    }

    pub(super) fn prompt(&self) -> Option<super::bounds::TraceText> {
        match self {
            Self::Frozen(snapshot) => Some(super::bounds::TraceText::detail(
                &snapshot.effective_system_prompt,
            )),
            _ => None,
        }
    }

    pub(super) fn presentation(
        &self,
        current: &RequestSnapshot,
    ) -> (
        TraceSystemPromptPresentation,
        super::types::TraceToolCatalogState,
    ) {
        use super::types::TraceToolCatalogState as T;
        use TraceSystemPromptState as S;
        #[cfg(test)]
        super::summary::probe::record(&super::summary::probe::TOOL_RELATIONSHIP);
        let (state, tools) = match self {
            Self::None => (S::Initial, T::Initial),
            Self::Unavailable(_) => (S::PreviousUnavailable, T::PreviousUnavailable),
            Self::Frozen(previous) => (
                if previous.effective_system_prompt == current.effective_system_prompt {
                    S::Unchanged
                } else {
                    S::Changed
                },
                if previous.tool_definitions == current.tool_definitions {
                    T::Unchanged
                } else {
                    T::Changed
                },
            ),
        };
        (
            TraceSystemPromptPresentation {
                state,
                preview: (state != S::Unchanged)
                    .then(|| TracePreview::of(&current.effective_system_prompt)),
            },
            tools,
        )
    }
}
