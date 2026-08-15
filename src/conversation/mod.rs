//! The provider-independent conversation domain (M7.5, Issue #54).
//!
//! This module owns the canonical conversation model for Issue #54:
//!
//! ```text
//! ConversationState
//! ├── MessageLedger        immutable committed conversational facts
//! └── ConversationSurface  active model-visible identity/order/visibility
//!                          at a stable SurfaceRevision
//! ```
//!
//! The ownership split is strict:
//!
//! - the Ledger is append-only and carries **no** visibility flags;
//! - the Surface is the **sole** authority for what is currently active and
//!   in what order, and it holds identities only;
//! - compaction commits exactly one canonical
//!   `User(Runtime / CompactionSummary)` message to the Ledger plus exactly
//!   one valid Surface `Replace`; it never deletes, mutates, rewrites, or
//!   overwrites an earlier Ledger record.
//!
//! Reads follow one direction, and only one:
//!
//! ```text
//! Surface @ current revision
//!   → finite active MessageIds
//!   → keyed Ledger lookups for exactly those bodies
//! ```
//!
//! Nothing in the normal projection/compaction path enumerates the Ledger.
//!
//! There is exactly **one** mutable conversation-state authority at a time.
//! Between attempts `RuntimeClientHost` owns the `ConversationState`; while
//! an attempt runs, `AgentExecution` owns it; settlement transfers it back.
//! (The full `ConversationRuntime` extraction belongs to Issue #61.)

pub mod ledger;
pub mod structure;
pub mod surface;

pub use ledger::{LedgerAccess, LedgerError, MessageLedger, message_id_of};
pub use structure::{StructuralError, StructuralIndex};
pub use surface::{
    ConversationSurface, SurfaceAccess, SurfaceError, SurfaceOp, SurfaceRevision, SurfaceSpan,
};

use std::sync::Arc;

use crate::message::types::{MessageBlock, UserMessageBlock};
use crate::runtime::identity::MessageId;

/// A conversation-state contract violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationError {
    /// The Message Ledger rejected the commit.
    Ledger(LedgerError),
    /// The Conversation Surface rejected the mutation.
    Surface(SurfaceError),
    /// The resulting active conversation would violate a structural
    /// contract.
    Structural(StructuralError),
    /// A Surface identity has no committed Ledger record. This is an
    /// impossible state by construction; it is reported rather than
    /// silently repaired.
    DanglingSurfaceIdentity(MessageId),
}

impl core::fmt::Display for ConversationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Ledger(error) => write!(f, "{error}"),
            Self::Surface(error) => write!(f, "{error}"),
            Self::Structural(error) => write!(f, "{error}"),
            Self::DanglingSurfaceIdentity(id) => {
                write!(f, "surface message {id} has no committed ledger record")
            }
        }
    }
}

impl std::error::Error for ConversationError {}

impl From<LedgerError> for ConversationError {
    fn from(error: LedgerError) -> Self {
        Self::Ledger(error)
    }
}

impl From<SurfaceError> for ConversationError {
    fn from(error: SurfaceError) -> Self {
        Self::Surface(error)
    }
}

impl From<StructuralError> for ConversationError {
    fn from(error: StructuralError) -> Self {
        Self::Structural(error)
    }
}

/// The validated, not-yet-applied semantic commit of one compaction.
///
/// The command is produced by the Context Engine after planning,
/// summarization, and the progress/fit checks, and applied by
/// [`ConversationState::commit_compaction`] — the single linearization
/// point of compaction. Constructing it mutates nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionCommit {
    /// The canonical runtime compaction summary to append to the Ledger.
    pub summary: UserMessageBlock,
    /// The inclusive active span the summary replaces.
    pub span: SurfaceSpan,
    /// The Surface revision the command was validated against. A commit
    /// against a different current revision is rejected as stale.
    pub expected_revision: SurfaceRevision,
}

/// The committed record of one applied compaction.
///
/// Every field is derived from already-committed conversation state; no
/// summary content is duplicated here, and this record is never a second
/// active-projection authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionRecord {
    /// The identity of the canonical summary message now in the Ledger.
    pub summary_message_id: MessageId,
    /// The inclusive active span the summary replaced.
    pub replaced: SurfaceSpan,
    /// The Surface revision established by the rewrite.
    pub surface_revision: SurfaceRevision,
    /// The monotonic compaction generation maintained in the Surface head.
    pub generation: u64,
}

/// The one canonical conversation state of a conversation.
///
/// This is the single mutable conversation authority. The Runtime Client is
/// a read model over it and never mutates it.
///
/// ```compile_fail
/// use rustx::conversation::ConversationState;
///
/// let state = ConversationState::new();
/// let _competing_authority = state.clone();
/// ```
#[derive(Debug, Default)]
pub struct ConversationState {
    ledger: MessageLedger,
    surface: ConversationSurface,
}

impl PartialEq for ConversationState {
    fn eq(&self, other: &Self) -> bool {
        self.ledger == other.ledger && self.surface == other.surface
    }
}

impl ConversationState {
    /// Creates an empty conversation state at [`SurfaceRevision::INITIAL`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bootstraps a conversation state from ordered canonical messages.
    ///
    /// Every message is committed through the one ordinary commit path, so
    /// the bootstrapped Surface is exactly the bootstrapped Ledger order.
    ///
    /// # Errors
    ///
    /// Returns the [`ConversationError`] of the first rejected commit (for
    /// example a duplicate `MessageId`).
    pub fn from_messages(
        messages: impl IntoIterator<Item = MessageBlock>,
    ) -> Result<Self, ConversationError> {
        let mut state = Self::new();
        for message in messages {
            state.commit(message)?;
        }
        Ok(state)
    }

    /// The immutable Message Ledger.
    #[must_use]
    pub fn ledger(&self) -> &MessageLedger {
        &self.ledger
    }

    /// The Conversation Surface.
    #[must_use]
    pub fn surface(&self) -> &ConversationSurface {
        &self.surface
    }

    /// The current Surface revision.
    #[must_use]
    pub fn revision(&self) -> SurfaceRevision {
        self.surface.revision()
    }

    /// The shared Ledger read instrumentation handle.
    #[must_use]
    pub fn ledger_access(&self) -> &Arc<LedgerAccess> {
        self.ledger.access()
    }

    /// The shared Surface read instrumentation handle.
    #[must_use]
    pub fn surface_access(&self) -> &Arc<SurfaceAccess> {
        self.surface.access()
    }

    /// The current active ordered message identities.
    #[must_use]
    pub fn active_ids(&self) -> &[MessageId] {
        self.surface.active()
    }

    /// The **one** ordinary canonical commit path: append the Ledger fact
    /// and append it to the Surface.
    ///
    /// Independent `ledger.push()` / `surface.push()` call sites do not
    /// exist anywhere else in the runtime.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::Ledger`] for a duplicate `MessageId` and
    /// [`ConversationError::Surface`] when the identity is already active.
    pub fn commit(&mut self, message: MessageBlock) -> Result<MessageId, ConversationError> {
        let id = message_id_of(&message);
        if self.surface.is_active(&id) {
            return Err(ConversationError::Surface(SurfaceError::AlreadyActive(id)));
        }
        let id = self.ledger.append(message)?;
        self.surface.append_after_validation(id.clone());
        Ok(id)
    }

    /// Hydrates the current Surface: the finite active messages in active
    /// order.
    ///
    /// This performs exactly one keyed Ledger lookup per active identity and
    /// never enumerates the Ledger, so its cost is a function of the active
    /// Surface size alone — never of retired history.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::DanglingSurfaceIdentity`] when a Surface
    /// identity has no committed record.
    pub fn active_messages(&self) -> Result<Vec<MessageBlock>, ConversationError> {
        self.hydrate(self.surface.active())
    }

    /// Hydrates an explicit ordered identity list through keyed lookups.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::DanglingSurfaceIdentity`] for an
    /// identity with no committed record.
    pub fn hydrate(&self, ids: &[MessageId]) -> Result<Vec<MessageBlock>, ConversationError> {
        ids.iter()
            .map(|id| {
                self.ledger
                    .get(id)
                    .cloned()
                    .ok_or_else(|| ConversationError::DanglingSurfaceIdentity(id.clone()))
            })
            .collect()
    }

    /// Reconstructs the exact active ordered identities of a historical
    /// Surface revision.
    ///
    /// Identity and order come from Surface history alone; only afterwards
    /// may a caller resolve bodies with [`ConversationState::hydrate`].
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::Surface`] for a revision beyond this
    /// Surface's history.
    pub fn reconstruct(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageId>, ConversationError> {
        Ok(self.surface.reconstruct(revision)?)
    }

    /// The structural index of the current active conversation.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::Structural`] for malformed active
    /// structure and [`ConversationError::DanglingSurfaceIdentity`] for an
    /// unresolvable Surface identity.
    pub fn structure(&self) -> Result<(Vec<MessageBlock>, StructuralIndex), ConversationError> {
        let active = self.active_messages()?;
        let index = StructuralIndex::build(&active)?;
        Ok((active, index))
    }

    /// Validates one compaction span against the **current** Surface without
    /// mutating anything, and produces the semantic commit command.
    ///
    /// The span is validated for Surface membership (unknown, retired,
    /// reversed endpoints are rejected) and for structural integrity
    /// (complete canonical messages only, no trusted `System` message, no
    /// split tool-call/result relationship).
    ///
    /// # Errors
    ///
    /// Returns the [`ConversationError`] of the first violation.
    pub fn prepare_compaction(
        &self,
        summary: UserMessageBlock,
        span: SurfaceSpan,
    ) -> Result<CompactionCommit, ConversationError> {
        let replacement = summary.id.clone();
        self.validate_compaction_span(&replacement, &span)?;
        Ok(CompactionCommit {
            summary,
            span,
            expected_revision: self.surface.revision(),
        })
    }

    fn validate_compaction_span(
        &self,
        replacement: &MessageId,
        span: &SurfaceSpan,
    ) -> Result<(usize, usize), ConversationError> {
        if self.ledger.contains(replacement) {
            return Err(ConversationError::Ledger(LedgerError::DuplicateMessageId(
                replacement.clone(),
            )));
        }
        let (start, end) = self.surface.validate_replace(span, replacement)?;
        let (_, index) = self.structure()?;
        index.validate_span(start, end)?;
        Ok((start, end))
    }

    /// The single semantic commit/linearization point of compaction:
    /// append the canonical summary fact, then rewrite the Surface.
    ///
    /// Before this call the old Ledger, the old Surface, and the old
    /// continuation semantics are authoritative. After it the summary exists
    /// in the Ledger, a new Surface revision exists in which the summary
    /// replaces the selected active span, every covered Ledger fact remains
    /// intact and addressable, and the provider continuation is known to be
    /// incompatible.
    ///
    /// Everything is re-validated against the current Surface first, so a
    /// rejected commit leaves neither a half-committed summary nor a
    /// half-applied Surface rewrite.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::Surface`] with
    /// [`SurfaceError::StaleRevision`] when the Surface moved since the
    /// command was prepared, and otherwise the [`ConversationError`] of the
    /// first violation.
    pub fn commit_compaction(
        &mut self,
        commit: CompactionCommit,
    ) -> Result<CompactionRecord, ConversationError> {
        let current = self.surface.revision();
        if commit.expected_revision != current {
            return Err(ConversationError::Surface(SurfaceError::StaleRevision {
                expected: commit.expected_revision,
                actual: current,
            }));
        }
        // Full re-validation before any mutation. The returned active range
        // is the proof used by the infallible Surface mutation below; no
        // ordinary recoverable Surface error remains after the append.
        let (start, end) = self.validate_compaction_span(&commit.summary.id, &commit.span)?;
        let summary_message_id = self.ledger.append(MessageBlock::User(commit.summary))?;
        let surface_revision = self.surface.replace_after_validation(
            &commit.span,
            summary_message_id.clone(),
            start,
            end,
        );
        Ok(CompactionRecord {
            summary_message_id,
            replaced: commit.span,
            surface_revision,
            generation: self.surface.compaction_generation(),
        })
    }
}

/// The deterministic message identity of a runtime compaction summary.
///
/// The identity is a namespaced function of the conversation id and the
/// compaction generation, so summaries are reproducible without random ids.
#[must_use]
pub fn summary_message_id(
    conversation_id: &crate::runtime::identity::ConversationId,
    generation: u64,
) -> MessageId {
    MessageId::new(format!("{conversation_id}-summary-{generation}"))
}

#[cfg(test)]
mod tests {
    use super::{
        CompactionCommit, ConversationError, ConversationState, LedgerError, SurfaceError,
        SurfaceRevision, SurfaceSpan, message_id_of, summary_message_id,
    };
    use crate::conversation::structure::StructuralError;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, SystemAuthority,
        SystemMessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
    use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};

    fn user(id: &str) -> MessageBlock {
        user_with_text(id, &format!("content {id}"))
    }

    fn user_with_text(id: &str, text: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    fn system(id: &str) -> MessageBlock {
        MessageBlock::System(SystemMessageBlock {
            id: MessageId::new(id),
            authority: SystemAuthority::Platform,
            content: vec![TextBlock {
                text: "be concise".to_owned(),
            }],
        })
    }

    fn assistant(id: &str, calls: &[&str]) -> MessageBlock {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content: calls
                .iter()
                .map(|call| {
                    AssistantContentBlock::ToolCall(ToolCall {
                        id: ToolCallId::new(*call),
                        tool_id: ToolId::new("tool-a"),
                        name: "alpha".to_owned(),
                        arguments: serde_json::json!({}),
                    })
                })
                .collect(),
        })
    }

    fn tool(call: &str) -> MessageBlock {
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new(format!("tool-{call}")),
            tool_call_id: ToolCallId::new(call),
            tool_id: ToolId::new("tool-a"),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 1,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
            },
        })
    }

    fn summary(id: &str, text: &str) -> UserMessageBlock {
        UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::CompactionSummary,
            timestamp: None,
        }
    }

    fn ids(state: &ConversationState) -> Vec<String> {
        state
            .active_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect()
    }

    fn ledger_ids(state: &ConversationState) -> Vec<String> {
        state
            .ledger()
            .audit_records()
            .iter()
            .map(|message| super::message_id_of(message).as_str().to_owned())
            .collect()
    }

    /// An ordinary commit performs exactly one Ledger append and one Surface
    /// append, advancing the revision by exactly one.
    #[test]
    fn ordinary_commit_appends_ledger_and_surface_once() {
        let mut state = ConversationState::new();
        assert_eq!(state.revision(), SurfaceRevision::INITIAL);
        state.commit(user("a")).expect("commit a");
        assert_eq!(state.revision(), SurfaceRevision::new(1));
        assert_eq!(state.ledger().len(), 1);
        state.commit(user("b")).expect("commit b");
        assert_eq!(state.revision(), SurfaceRevision::new(2));
        assert_eq!(state.ledger().len(), 2);
        assert_eq!(ids(&state), vec!["a", "b"]);
    }

    /// A duplicate `MessageId` commit is rejected.
    #[test]
    fn duplicate_message_id_commit_is_rejected() {
        let mut state = ConversationState::new();
        state.commit(user("a")).expect("commit");
        assert_eq!(
            state.commit(user("a")).expect_err("duplicate"),
            ConversationError::Surface(SurfaceError::AlreadyActive(MessageId::new("a")))
        );
        assert_eq!(state.ledger().len(), 1);
        assert_eq!(state.revision(), SurfaceRevision::new(1));
    }

    /// The compaction semantic commit: one canonical summary appended, one
    /// Surface replacement applied, and every covered original still in the
    /// Ledger.
    #[test]
    fn compaction_commits_one_summary_and_one_replacement() {
        let mut state =
            ConversationState::from_messages([user("a"), user("b"), user("c"), user("d")])
                .expect("bootstrap");
        let command = state
            .prepare_compaction(
                summary("s1", "earlier context"),
                SurfaceSpan::new(MessageId::new("a"), MessageId::new("c")),
            )
            .expect("prepare");
        let record = state.commit_compaction(command).expect("commit");
        assert_eq!(record.summary_message_id, MessageId::new("s1"));
        assert_eq!(record.generation, 1);
        assert_eq!(record.surface_revision, SurfaceRevision::new(5));
        assert_eq!(ids(&state), vec!["s1", "d"]);
        assert_eq!(ledger_ids(&state), vec!["a", "b", "c", "d", "s1"]);
        // The originals are still addressable and unchanged.
        assert_eq!(
            state.ledger().get(&MessageId::new("b")),
            Some(&user("b")),
            "a committed ledger fact is never edited by compaction"
        );
    }

    /// Repeated compaction operates from the current Surface and never
    /// resurrects retired Ledger history.
    #[test]
    fn repeated_compaction_never_resurrects_retired_history() {
        let mut state =
            ConversationState::from_messages([user("a"), user("b"), user("c"), user("d")])
                .expect("bootstrap");
        let first = state
            .prepare_compaction(
                summary("s1", "first"),
                SurfaceSpan::new(MessageId::new("a"), MessageId::new("c")),
            )
            .expect("prepare first");
        state.commit_compaction(first).expect("commit first");
        assert_eq!(ids(&state), vec!["s1", "d"]);

        state.commit(user("e")).expect("commit e");
        state.commit(user("f")).expect("commit f");
        assert_eq!(ids(&state), vec!["s1", "d", "e", "f"]);

        let second = state
            .prepare_compaction(
                summary("s2", "second"),
                SurfaceSpan::new(MessageId::new("s1"), MessageId::new("e")),
            )
            .expect("prepare second");
        let record = state.commit_compaction(second).expect("commit second");
        assert_eq!(record.generation, 2);
        assert_eq!(ids(&state), vec!["s2", "f"]);
        assert_eq!(
            ledger_ids(&state),
            vec!["a", "b", "c", "d", "s1", "e", "f", "s2"],
            "every committed fact survives; nothing is rewritten"
        );
        assert!(
            !state
                .active_ids()
                .iter()
                .any(|id| { matches!(id.as_str(), "a" | "b" | "c") }),
            "the second compaction must not rediscover retired ledger history"
        );
    }

    /// Historical Surface revisions reconstruct deterministically and are
    /// stable under later mutation.
    #[test]
    fn historical_revisions_reconstruct_deterministically() {
        let mut state =
            ConversationState::from_messages([user("a"), user("b"), user("c")]).expect("bootstrap");
        let historical = state.revision();
        let command = state
            .prepare_compaction(
                summary("s1", "x"),
                SurfaceSpan::new(MessageId::new("a"), MessageId::new("b")),
            )
            .expect("prepare");
        state.commit_compaction(command).expect("commit");
        state.commit(user("d")).expect("commit d");
        assert_eq!(
            state.reconstruct(historical).expect("historical"),
            vec![
                MessageId::new("a"),
                MessageId::new("b"),
                MessageId::new("c")
            ]
        );
        assert_eq!(
            state.reconstruct(state.revision()).expect("current"),
            state.active_ids().to_vec()
        );
    }

    /// Every accepted append and replacement creates a stable reconstruction
    /// boundary, including the intermediate revision between two compactions.
    #[test]
    fn intermediate_compaction_revisions_remain_exact_after_later_mutations() {
        let mut state =
            ConversationState::from_messages([user("a"), user("b"), user("c"), user("d")])
                .expect("bootstrap");
        let initial = state.revision();

        let first = state
            .prepare_compaction(
                summary("s1", "first"),
                SurfaceSpan::new(MessageId::new("a"), MessageId::new("c")),
            )
            .expect("prepare first");
        let first_record = state.commit_compaction(first).expect("commit first");
        let first_revision = first_record.surface_revision;

        state.commit(user("e")).expect("commit e");
        state.commit(user("f")).expect("commit f");
        let after_append = state.revision();

        let second = state
            .prepare_compaction(
                summary("s2", "second"),
                SurfaceSpan::new(MessageId::new("s1"), MessageId::new("e")),
            )
            .expect("prepare second");
        let second_record = state.commit_compaction(second).expect("commit second");
        let second_revision = second_record.surface_revision;
        state.commit(user("g")).expect("commit later append");

        assert_eq!(
            state.reconstruct(initial).expect("initial revision"),
            vec![
                MessageId::new("a"),
                MessageId::new("b"),
                MessageId::new("c"),
                MessageId::new("d")
            ]
        );
        assert_eq!(
            state.reconstruct(first_revision).expect("first revision"),
            vec![MessageId::new("s1"), MessageId::new("d")]
        );
        assert_eq!(
            state.reconstruct(after_append).expect("append revision"),
            vec![
                MessageId::new("s1"),
                MessageId::new("d"),
                MessageId::new("e"),
                MessageId::new("f")
            ]
        );
        assert_eq!(
            state.reconstruct(second_revision).expect("second revision"),
            vec![MessageId::new("s2"), MessageId::new("f")]
        );
    }

    /// Equal content never aliases canonical identity: Surface spans and
    /// historical reconstruction operate on `MessageId`, not message bytes.
    #[test]
    fn equal_content_messages_remain_distinct_identities() {
        let first = user_with_text("same-a", "identical");
        let second = user_with_text("same-b", "identical");
        let mut state =
            ConversationState::from_messages([first.clone(), second.clone()]).expect("bootstrap");
        let before = state.revision();

        assert_ne!(message_id_of(&first), message_id_of(&second));
        assert_eq!(
            state.active_ids(),
            &[MessageId::new("same-a"), MessageId::new("same-b")]
        );
        assert_eq!(state.ledger().get(&MessageId::new("same-a")), Some(&first));
        assert_eq!(state.ledger().get(&MessageId::new("same-b")), Some(&second));

        let commit = state
            .prepare_compaction(
                summary("same-summary", "summary"),
                SurfaceSpan::new(MessageId::new("same-b"), MessageId::new("same-b")),
            )
            .expect("select the second equal-content message by id");
        state.commit_compaction(commit).expect("commit");
        assert_eq!(
            state.active_ids(),
            &[MessageId::new("same-a"), MessageId::new("same-summary")]
        );
        assert_eq!(
            state.reconstruct(before).expect("historical identities"),
            vec![MessageId::new("same-a"), MessageId::new("same-b")]
        );
        assert_eq!(state.ledger().get(&MessageId::new("same-a")), Some(&first));
        assert_eq!(state.ledger().get(&MessageId::new("same-b")), Some(&second));
    }

    /// Invalid replacements are rejected at preparation and never mutate.
    #[test]
    fn invalid_replacements_are_rejected() {
        let mut state =
            ConversationState::from_messages([user("a"), user("b"), user("c")]).expect("bootstrap");
        assert_eq!(
            state
                .prepare_compaction(
                    summary("s", "x"),
                    SurfaceSpan::new(MessageId::new("ghost"), MessageId::new("b")),
                )
                .expect_err("unknown start"),
            ConversationError::Surface(SurfaceError::NotActive(MessageId::new("ghost")))
        );
        assert_eq!(
            state
                .prepare_compaction(
                    summary("s", "x"),
                    SurfaceSpan::new(MessageId::new("c"), MessageId::new("a")),
                )
                .expect_err("reversed"),
            ConversationError::Surface(SurfaceError::ReversedSpan {
                start: MessageId::new("c"),
                end: MessageId::new("a"),
            })
        );
        assert_eq!(
            state
                .prepare_compaction(
                    summary("a", "x"),
                    SurfaceSpan::new(MessageId::new("a"), MessageId::new("b")),
                )
                .expect_err("replacement identity already committed"),
            ConversationError::Ledger(LedgerError::DuplicateMessageId(MessageId::new("a")))
        );
        // Retired spans stay rejected after a first compaction.
        let command = state
            .prepare_compaction(
                summary("s1", "x"),
                SurfaceSpan::new(MessageId::new("a"), MessageId::new("b")),
            )
            .expect("prepare");
        state.commit_compaction(command).expect("commit");
        assert_eq!(
            state
                .prepare_compaction(
                    summary("s2", "x"),
                    SurfaceSpan::new(MessageId::new("a"), MessageId::new("b")),
                )
                .expect_err("retired span"),
            ConversationError::Surface(SurfaceError::NotActive(MessageId::new("a")))
        );
        assert_eq!(state.ledger().len(), 4);
    }

    /// A stale prepared command is rejected at the commit point.
    #[test]
    fn a_stale_command_is_rejected_at_the_commit_point() {
        let mut state =
            ConversationState::from_messages([user("a"), user("b"), user("c")]).expect("bootstrap");
        let command = state
            .prepare_compaction(
                summary("s1", "x"),
                SurfaceSpan::new(MessageId::new("a"), MessageId::new("b")),
            )
            .expect("prepare");
        state.commit(user("d")).expect("the surface moves on");
        assert_eq!(
            state.commit_compaction(command).expect_err("stale"),
            ConversationError::Surface(SurfaceError::StaleRevision {
                expected: SurfaceRevision::new(3),
                actual: SurfaceRevision::new(4),
            })
        );
        assert_eq!(state.ledger().len(), 4, "no half-committed summary");
        assert_eq!(ids(&state), vec!["a", "b", "c", "d"]);
    }

    /// A duplicate summary identity and an invalid prepared span are rejected
    /// before the Ledger append, leaving both authorities unchanged.
    #[test]
    fn invalid_prepared_compactions_are_atomic() {
        let mut state =
            ConversationState::from_messages([user("a"), user("b"), user("c")]).expect("bootstrap");
        let before_ids = state.active_ids().to_vec();
        let before_revision = state.revision();
        let before_ledger_len = state.ledger().len();

        let duplicate = CompactionCommit {
            summary: summary("a", "duplicate"),
            span: SurfaceSpan::new(MessageId::new("a"), MessageId::new("b")),
            expected_revision: before_revision,
        };
        assert!(matches!(
            state.commit_compaction(duplicate),
            Err(ConversationError::Ledger(LedgerError::DuplicateMessageId(id)))
                if id == MessageId::new("a")
        ));

        let invalid_span = CompactionCommit {
            summary: summary("s1", "invalid"),
            span: SurfaceSpan::new(MessageId::new("ghost"), MessageId::new("b")),
            expected_revision: before_revision,
        };
        assert!(matches!(
            state.commit_compaction(invalid_span),
            Err(ConversationError::Surface(SurfaceError::NotActive(id)))
                if id == MessageId::new("ghost")
        ));
        assert_eq!(state.active_ids(), before_ids.as_slice());
        assert_eq!(state.revision(), before_revision);
        assert_eq!(state.ledger().len(), before_ledger_len);
    }

    /// A replacement can never separate a tool result from its active
    /// owning tool call.
    #[test]
    fn a_replacement_never_splits_a_tool_pair() {
        let state = ConversationState::from_messages([
            user("u1"),
            assistant("a1", &["c1"]),
            tool("c1"),
            user("u2"),
        ])
        .expect("bootstrap");
        assert_eq!(
            state
                .prepare_compaction(
                    summary("s", "x"),
                    SurfaceSpan::new(MessageId::new("u1"), MessageId::new("a1")),
                )
                .expect_err("retires the call without its result"),
            ConversationError::Structural(StructuralError::SplitToolPair {
                tool_call_id: ToolCallId::new("c1"),
            })
        );
        assert!(
            state
                .prepare_compaction(
                    summary("s", "x"),
                    SurfaceSpan::new(MessageId::new("u1"), MessageId::new("tool-c1")),
                )
                .is_ok(),
            "the complete turn is replaceable"
        );
    }

    /// Trusted system content is never replaced by a runtime summary.
    #[test]
    fn a_replacement_never_covers_system_content() {
        let state = ConversationState::from_messages([system("sys"), user("a"), user("b")])
            .expect("bootstrap");
        assert_eq!(
            state
                .prepare_compaction(
                    summary("s", "x"),
                    SurfaceSpan::new(MessageId::new("sys"), MessageId::new("b")),
                )
                .expect_err("system inside span"),
            ConversationError::Structural(StructuralError::SystemMessageInSpan(MessageId::new(
                "sys"
            )))
        );
    }

    /// Hydrating the current Surface performs keyed reads only.
    #[test]
    fn hydration_never_enumerates_the_ledger() {
        let mut state = ConversationState::new();
        for index in 0..200 {
            state.commit(user(&format!("m{index}"))).expect("commit");
        }
        let command = state
            .prepare_compaction(
                summary("s1", "x"),
                SurfaceSpan::new(MessageId::new("m0"), MessageId::new("m197")),
            )
            .expect("prepare");
        state.commit_compaction(command).expect("commit");
        state.ledger_access().reset();
        let active = state.active_messages().expect("hydrate");
        assert_eq!(active.len(), 3);
        assert_eq!(state.ledger_access().enumerations(), 0);
        assert_eq!(state.ledger_access().keyed_reads(), 3);
    }

    /// Current compaction generation is maintained as head metadata and does
    /// not inspect the retained historical operation log.
    #[test]
    fn compaction_generation_is_current_head_work_only() {
        let mut state =
            ConversationState::from_messages([user("a"), user("b"), user("c")]).expect("bootstrap");
        let command = state
            .prepare_compaction(
                summary("s1", "summary"),
                SurfaceSpan::new(MessageId::new("a"), MessageId::new("b")),
            )
            .expect("prepare");
        state.commit_compaction(command).expect("commit");
        state.surface_access().reset();
        assert_eq!(state.surface().compaction_generation(), 1);
        assert_eq!(state.surface_access().history_enumerations(), 0);
        assert_eq!(state.surface_access().history_steps(), 0);
    }

    /// Summary identities are deterministic and namespaced by conversation.
    #[test]
    fn summary_ids_are_deterministic_and_namespaced() {
        let conversation = ConversationId::new("conv-1");
        assert_eq!(
            summary_message_id(&conversation, 1).as_str(),
            "conv-1-summary-1"
        );
        assert_ne!(
            summary_message_id(&conversation, 1),
            summary_message_id(&conversation, 2)
        );
        assert_ne!(
            summary_message_id(&conversation, 1),
            summary_message_id(&ConversationId::new("conv-2"), 1)
        );
    }
}
