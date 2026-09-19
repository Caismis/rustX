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

use super::TraceProjection;
use super::bounds::{
    TRACE_SUMMARY_CONTEXT, TRACE_SUMMARY_CONTEXT_ARTIFACTS, TRACE_SUMMARY_CONTEXT_BYTES,
    TracePreview, encoded_len, identity_fits,
};
use super::content::{message_preview, user_artifacts, user_source_label};
use super::types::{
    TraceContextKind, TraceContextPresentation, TraceSystemPromptPresentation,
    TraceSystemPromptState,
};
use crate::durable::ConversationStoreError;
use crate::durable::presentation::{FactQuery, FactScope};
use crate::events::types::RuntimeEvent as E;
use crate::message::types::{ContextKind, InboundKind, MessageBlock};
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
        for (id, message) in kept.iter().zip(self.store.load_messages(kept)?) {
            let MessageBlock::User(user) = &message else {
                return Err(context_invariant(frozen, id));
            };
            let InboundKind::Context(kind) = &user.kind else {
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
            additions.push(TraceContextPresentation {
                message_id: id.clone(),
                context_kind: context_family(kind),
                source: user_source_label(&user.source).to_owned(),
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

/// The bounded presentation family of one admitted context fact.
///
/// Only the family crosses. The frozen `GoalSnapshot` and the Agent Status
/// generation metadata stay inside the runtime: neither is needed to present
/// that a Context fact of that family was introduced.
const fn context_family(kind: &ContextKind) -> TraceContextKind {
    match kind {
        ContextKind::GoalStatus(_) => TraceContextKind::GoalStatus,
        ContextKind::RuntimeToolObservation => TraceContextKind::RuntimeToolObservation,
        ContextKind::ExtensionEnvironment => TraceContextKind::ExtensionEnvironment,
        ContextKind::AgentStatus(_) => TraceContextKind::AgentStatus,
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
