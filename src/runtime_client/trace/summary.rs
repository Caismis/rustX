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
    /// The System Prompt presentation of one actual request.
    ///
    /// # Errors
    ///
    /// Propagates the durable read failure of the predecessor lookup or of
    /// its immutable snapshot. A failed read is never reported as "no
    /// predecessor": that would turn an unreadable store into a claim that
    /// this request is the first one.
    pub(super) fn system_prompt_presentation(
        &self,
        anchor_sequence: u64,
        frozen: &RequestSnapshot,
    ) -> Result<TraceSystemPromptPresentation, ConversationStoreError> {
        let previous = self.previous_request_prompt(anchor_sequence)?;
        let state = system_prompt_state(&previous, &frozen.effective_system_prompt);
        Ok(TraceSystemPromptPresentation {
            // `Unchanged` says the preceding request's row already carries
            // this preview, so repeating it on every row of a long run of
            // identical requests would be pure duplication.
            preview: match state {
                TraceSystemPromptState::Unchanged => None,
                _ => Some(TracePreview::of(&frozen.effective_system_prompt)),
            },
            state,
        })
    }

    /// The frozen prompt of the nearest preceding actual request.
    ///
    /// The predecessor is whichever `ModelRequestStarted` immediately
    /// precedes this one in durable start order: it may belong to an earlier
    /// retry, a recovery request, an earlier logical Step or an earlier
    /// Attempt. The lookup is a single indexed seek bounded by this
    /// projection's own read cut, so it never scans the Journal and never
    /// depends on which records a client loaded.
    ///
    fn previous_request_prompt(
        &self,
        anchor_sequence: u64,
    ) -> Result<PreviousPrompt, ConversationStoreError> {
        #[cfg(test)]
        probe::record(&probe::SYSTEM_PREDECESSOR);
        let Some(previous) = self
            .store
            .read_presentation_events(&FactQuery {
                scope: FactScope::All,
                kinds: vec!["model_request_started"],
                // Exclusive, so this request can never be its own predecessor.
                before: Some(anchor_sequence),
                after: 0,
                ascending: false,
                through: self.through,
                limit: 1,
            })?
            .pop()
        else {
            return Ok(PreviousPrompt::None);
        };
        let E::ModelRequestStarted { request_id, .. } = &previous.event else {
            return Ok(PreviousPrompt::Unavailable);
        };
        Ok(PreviousPrompt::Frozen(
            self.store
                .load_request_snapshot(request_id)?
                .effective_system_prompt,
        ))
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

/// What native authority established about one request's predecessor.
///
/// The three cases stay apart deliberately: "there is no earlier actual
/// request" and "there is one whose frozen prompt could not be read" are
/// different answers, and collapsing them would let an unreadable
/// predecessor be presented as the start of the conversation.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PreviousPrompt {
    /// Native authority proved no earlier actual request exists.
    None,
    /// A predecessor exists, but its frozen prompt could not be established.
    Unavailable,
    /// The predecessor's exact frozen prompt.
    Frozen(String),
}

/// Classifies one request's prompt against its resolved predecessor.
///
/// Comparison is exact historical value equality between two immutable
/// snapshots — never against current configuration, current assembly, or
/// System section names.
pub(super) fn system_prompt_state(
    previous: &PreviousPrompt,
    current: &str,
) -> TraceSystemPromptState {
    match previous {
        PreviousPrompt::None => TraceSystemPromptState::Initial,
        PreviousPrompt::Unavailable => TraceSystemPromptState::PreviousUnavailable,
        PreviousPrompt::Frozen(previous) if previous == current => {
            TraceSystemPromptState::Unchanged
        }
        PreviousPrompt::Frozen(_) => TraceSystemPromptState::Changed,
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

    /// Zeroes both counters so the next call is measured on its own.
    pub(in crate::runtime_client::trace) fn reset() {
        SYSTEM_PREDECESSOR.with(|count| count.set(0));
        CONTEXT_LEDGER_JOIN.with(|count| count.set(0));
    }
}

#[cfg(test)]
mod tests {
    use super::{PreviousPrompt as P, TraceSystemPromptState as S, system_prompt_state};

    fn frozen(prompt: &str) -> P {
        P::Frozen(prompt.to_owned())
    }

    /// Each closed answer comes from exactly one resolved predecessor shape.
    #[test]
    fn system_prompt_classification_is_total_and_exact() {
        assert_eq!(system_prompt_state(&P::None, "prompt"), S::Initial);
        assert_eq!(
            system_prompt_state(&P::Unavailable, "prompt"),
            S::PreviousUnavailable
        );
        assert_eq!(
            system_prompt_state(&frozen("prompt"), "prompt"),
            S::Unchanged
        );
        assert_eq!(system_prompt_state(&frozen("older"), "prompt"), S::Changed);
        // Equality is exact: whitespace is content, not formatting.
        assert_eq!(
            system_prompt_state(&frozen("prompt "), "prompt"),
            S::Changed
        );
        // An empty prompt is a real historical value, not an absent one.
        assert_eq!(system_prompt_state(&frozen(""), ""), S::Unchanged);
        assert_eq!(system_prompt_state(&P::None, ""), S::Initial);
    }
}
