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
use super::bounds::{
    TRACE_SUMMARY_CONTEXT, TRACE_SUMMARY_CONTEXT_ARTIFACTS, TRACE_SUMMARY_CONTEXT_BYTES,
    TracePreview, encoded_len, identity_fits,
};
use super::content::{message_preview, user_artifacts};
use super::types::{
    TraceContextKind, TraceContextPresentation, TraceContextSource, TraceSystemPromptPresentation,
    TraceSystemPromptState,
};
use crate::durable::ConversationStoreError;
use crate::durable::presentation::{FactQuery, FactScope};
use crate::events::types::RuntimeEvent as E;
use crate::message::types::{ContextKind, InboundKind, MessageBlock, UserSource};
use crate::model::snapshot::RequestSnapshot;

impl TraceProjection<'_> {
    /// One indexed predecessor seek shared by BOTH immutable comparisons.
    /// `RequestNotFound` is absence; every other durable error propagates.
    pub(super) fn previous_request(
        &self,
        anchor_sequence: u64,
    ) -> Result<PreviousRequest, ConversationStoreError> {
        #[cfg(test)]
        probe::record(&probe::SYSTEM_PREDECESSOR);
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
            return Ok(PreviousRequest::None);
        };
        let E::ModelRequestStarted { request_id, .. } = previous.event else {
            return Err(ConversationStoreError::InvalidReference(
                "non-request predecessor".into(),
            ));
        };
        match self.store.load_request_snapshot(&request_id) {
            Ok(snapshot) => Ok(PreviousRequest::Frozen(Box::new(snapshot))),
            Err(ConversationStoreError::RequestNotFound(_)) => {
                Ok(PreviousRequest::Unavailable(request_id))
            }
            Err(error) => Err(error),
        }
    }

    /// The canonical Context this exact request introduced, in frozen order.
    ///
    /// `RequestSnapshot.request_context_ids` is the identity and ordering
    /// authority: it records the exact request-scoped canonical context
    /// committed atomically with this request's start. A later retry or
    /// recovery request reuses admitted context rather than admitting
    /// duplicates, so its own snapshot lists none and it introduces none —
    /// no Trace-local "already displayed" state is needed or kept.
    ///
    /// # Errors
    ///
    /// Propagates keyed Ledger read failures, and rejects a conversation
    /// whose snapshot claims an identity that the Ledger does not hold as an
    /// admitted Context fact. Such a message is never silently dropped or
    /// downgraded into an ordinary User message.
    pub(super) fn context_presentation(
        &self,
        frozen: &RequestSnapshot,
    ) -> Result<(Vec<TraceContextPresentation>, bool), ConversationStoreError> {
        let ids = &frozen.request_context_ids;
        if ids.is_empty() {
            return Ok((Vec::new(), false));
        }
        let mut truncated = ids.len() > TRACE_SUMMARY_CONTEXT;
        let kept = &ids[..ids.len().min(TRACE_SUMMARY_CONTEXT)];
        let mut additions: Vec<TraceContextPresentation> = Vec::new();
        #[cfg(test)]
        probe::record(&probe::CONTEXT_LEDGER_JOIN);
        for (id, message) in kept.iter().zip(self.store.load_messages(kept)?) {
            let MessageBlock::User(user) = &message else {
                return Err(context_invariant(frozen, id));
            };
            let InboundKind::Context(kind) = &user.kind else {
                return Err(context_invariant(frozen, id));
            };
            let Some((source, context_kind)) = context_semantics(&user.source, kind) else {
                return Err(context_invariant(frozen, id));
            };
            if !identity_fits(id.as_str()) {
                // An identity is omitted whole rather than shortened into
                // one that refers to nothing.
                truncated = true;
                continue;
            }
            let (attachments, attachments_truncated) =
                user_artifacts(&user.content, TRACE_SUMMARY_CONTEXT_ARTIFACTS);
            let contribution = frozen
                .contributions
                .iter()
                .find(|record| record.message_id == *id)
                .ok_or_else(|| context_invariant(frozen, id))?;
            contribution.validate_message(&message)?;
            additions.push(TraceContextPresentation {
                message_id: id.clone(),
                producer: contribution.producer.clone(),
                context_kind,
                source,
                preview: message_preview(&message),
                attachments,
                truncated: attachments_truncated,
            });
        }
        // Encoded bytes, not counts: JSON escaping can multiply a bounded
        // preview several times over. The retained prefix stays in frozen
        // order, so the same conversation always yields the same prefix.
        while additions.len() > 1 && encoded_len(&additions) > TRACE_SUMMARY_CONTEXT_BYTES {
            additions.pop();
            truncated = true;
        }
        if encoded_len(&additions) > TRACE_SUMMARY_CONTEXT_BYTES
            && let Some(first) = additions.first_mut()
        {
            // One adversarial fact cannot erase its own identity: the entry
            // releases its content and keeps the identity it names.
            first.preview = None;
            first.attachments.clear();
            first.truncated = true;
            truncated = true;
        }
        Ok((additions, truncated))
    }
}

/// Canonical provenance and family form one native Context Assembly relationship.
/// Keep exact extension identity; an impossible pair is a durable invariant
/// violation, never a new presentation source or a coerced family.
fn context_semantics(
    source: &UserSource,
    kind: &ContextKind,
) -> Option<(TraceContextSource, TraceContextKind)> {
    match (source, kind) {
        (UserSource::Runtime, ContextKind::NativeEnvironment) => Some((
            TraceContextSource::Runtime,
            TraceContextKind::NativeEnvironment,
        )),
        (UserSource::Runtime, ContextKind::GoalStatus(_)) => {
            Some((TraceContextSource::Runtime, TraceContextKind::GoalStatus))
        }
        (UserSource::Runtime, ContextKind::RuntimeToolObservation) => Some((
            TraceContextSource::Runtime,
            TraceContextKind::RuntimeToolObservation,
        )),
        (UserSource::Runtime, ContextKind::AgentStatus(_)) => {
            Some((TraceContextSource::Runtime, TraceContextKind::AgentStatus))
        }
        (UserSource::Extension { contributor }, ContextKind::ExtensionEnvironment) => Some((
            TraceContextSource::CertifiedExtension {
                contributor: contributor.clone(),
            },
            TraceContextKind::ExtensionEnvironment,
        )),
        _ => None,
    }
}

/// The invariant violation of a snapshot identity the Ledger does not hold
/// as an admitted Context fact.
fn context_invariant(
    frozen: &RequestSnapshot,
    id: &crate::runtime::identity::MessageId,
) -> ConversationStoreError {
    ConversationStoreError::InvalidReference(format!(
        "request {} froze {id} as request-scoped context, but the canonical Ledger message is not an admitted Context fact",
        frozen.request_id
    ))
}

/// The full frozen snapshot is loaded once, never separately per relationship.
pub(super) enum PreviousRequest {
    None,
    Unavailable(crate::runtime::identity::RequestId),
    Frozen(Box<RequestSnapshot>),
}

impl PreviousRequest {
    pub(super) fn identity(&self) -> super::types::TraceRequestPredecessor {
        use super::types::TraceRequestPredecessor as P;
        match self {
            Self::None => P::NotApplicable,
            Self::Unavailable(id) => P::Unavailable {
                request_id: identity_fits(id.as_str()).then(|| id.clone()),
            },
            Self::Frozen(snapshot) if identity_fits(snapshot.request_id.as_str()) => P::Available {
                request_id: snapshot.request_id.clone(),
            },
            Self::Frozen(_) => P::Unavailable { request_id: None },
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
        probe::record(&probe::TOOL_RELATIONSHIP);
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

    /// Zeroes both counters so the next call is measured on its own.
    pub(in crate::runtime_client::trace) fn reset() {
        SYSTEM_PREDECESSOR.with(|count| count.set(0));
        TOOL_RELATIONSHIP.with(|count| count.set(0));
        CONTEXT_LEDGER_JOIN.with(|count| count.set(0));
    }
}
