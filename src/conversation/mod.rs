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
//! Between attempts the conversation runtime coordinator owns the
//! `ConversationState`; while
//! an attempt runs, `AgentExecution` owns it; settlement transfers it back.

pub mod ledger;
pub mod structure;
pub mod surface;

pub use ledger::{LedgerAccess, LedgerError, MessageLedger, message_id_of};
pub use structure::{StructuralError, StructuralIndex};
pub use surface::{
    ConversationSurface, SurfaceAccess, SurfaceError, SurfaceOp, SurfaceRevision, SurfaceSpan,
};

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::message::types::{AssistantContentBlock, InboundKind, MessageBlock, UserMessageBlock};
use crate::runtime::identity::{MessageId, ToolCallId};

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

/// A validated, not-yet-installed canonical commit.
///
/// Produced by [`ConversationState::prepare_commit`] after every fallible
/// condition (duplicate Ledger identity, already-active Surface identity) has
/// been checked against the current state; consumed by
/// [`ConversationState::install_prepared`], which is infallible under
/// exclusive ownership. The exact message that was validated is carried
/// inside the value, so a caller can never substitute a different message at
/// install time.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedCanonicalCommit {
    /// The validated message identity.
    id: MessageId,
    /// The exact message that was validated.
    message: MessageBlock,
}

impl PreparedCanonicalCommit {
    /// The exact message this commit validated.
    #[must_use]
    pub fn message(&self) -> &MessageBlock {
        &self.message
    }

    /// The validated message identity.
    #[must_use]
    pub fn message_id(&self) -> &MessageId {
        &self.id
    }
}

/// A validated, not-yet-installed compaction commit.
///
/// Produced by [`ConversationState::prepare_compaction`] after every fallible
/// condition (stale/valid span, structural integrity, duplicate identity) has
/// been checked against the current state; consumed by
/// [`ConversationState::install_prepared_compaction`], which is infallible
/// under exclusive ownership. The exact summary, span, validated indices, and
/// validated Surface revision are carried inside the value, so no caller may
/// substitute another summary or span at install time.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedCompactionCommit {
    /// The canonical runtime compaction summary to append to the Ledger.
    summary: UserMessageBlock,
    /// The inclusive active span the summary replaces.
    span: SurfaceSpan,
    /// The Surface revision the commit was validated against.
    expected_revision: SurfaceRevision,
    /// The resolved inclusive active index range of the span.
    start: usize,
    /// The resolved inclusive active index range of the span.
    end: usize,
}

impl PreparedCompactionCommit {
    /// The exact canonical summary this commit validated.
    #[must_use]
    pub fn summary(&self) -> &UserMessageBlock {
        &self.summary
    }

    /// The canonical summary as a [`MessageBlock`], for the durable append.
    #[must_use]
    pub fn summary_block(&self) -> MessageBlock {
        MessageBlock::User(self.summary.clone())
    }

    /// The inclusive active span this commit replaces.
    #[must_use]
    pub fn span(&self) -> &SurfaceSpan {
        &self.span
    }

    /// The Surface revision this commit was validated against.
    #[must_use]
    pub fn expected_revision(&self) -> SurfaceRevision {
        self.expected_revision
    }
}

/// A durable canonical prefix that cannot be resumed automatically.
///
/// This is the smallest semantic evidence of the Issue #63 restart gate: a
/// durable Message Ledger append does **not** by itself imply that the head
/// is a resumable `ConversationRuntime` safe boundary. It fails closed on the
/// two states a Ledger-only Surface reconstruction cannot represent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverySafetyError {
    /// A compaction summary is committed but its Surface `Replace` is not
    /// durably reconstructable (full #11 surface-revision durability is a
    /// later milestone). Ordinary append would keep the replaced span active
    /// and append the summary after it.
    CompactionSurfaceNotReconstructable(MessageId),
    /// An Assistant message issued a tool call whose `ToolResult` sibling is
    /// not yet committed: resuming admission here would let asynchronous
    /// inbound cross the incomplete tool-call/result structure.
    IncompleteToolTurn {
        /// The tool call without a committed result.
        tool_call_id: ToolCallId,
    },
}

impl core::fmt::Display for RecoverySafetyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CompactionSurfaceNotReconstructable(id) => write!(
                f,
                "the durable canonical prefix contains compaction summary {id} whose Surface replacement is not durably reconstructable"
            ),
            Self::IncompleteToolTurn { tool_call_id } => write!(
                f,
                "the durable canonical prefix ends inside an incomplete tool turn: tool call {tool_call_id} has no committed ToolResult"
            ),
        }
    }
}

impl std::error::Error for RecoverySafetyError {}

/// The first committed tool call with no committed `ToolResult` sibling.
///
/// This is the live-admission half of the recovery gate: an active Surface
/// (or a reconstructed prefix) that ends inside an incomplete tool turn must
/// not admit asynchronous inbound, which would cross the tool-call/result
/// structure.
#[must_use]
pub fn pending_tool_call(messages: &[MessageBlock]) -> Option<ToolCallId> {
    let mut tool_calls: BTreeSet<ToolCallId> = BTreeSet::new();
    let mut tool_results: BTreeSet<ToolCallId> = BTreeSet::new();
    for message in messages {
        match message {
            MessageBlock::Assistant(assistant) => {
                for block in &assistant.content {
                    if let AssistantContentBlock::ToolCall(call) = block {
                        tool_calls.insert(call.id.clone());
                    }
                }
            }
            MessageBlock::Tool(tool) => {
                tool_results.insert(tool.tool_call_id.clone());
            }
            MessageBlock::User(_) | MessageBlock::System(_) => {}
        }
    }
    tool_calls
        .into_iter()
        .find(|call| !tool_results.contains(call))
}

/// Whether an ordered canonical prefix may be resumed as a live
/// [`ConversationState`] without guessing missing Surface/execution state.
///
/// The predicate answers the restart-gate question exactly: a durable Ledger
/// prefix is resumable only when ordinary-append Surface reconstruction
/// (`ConversationState::from_messages`) reproduces a structurally safe live
/// conversation. It is **not** a generic checkpoint and not a recovery log.
///
/// # Errors
///
/// Returns [`RecoverySafetyError::CompactionSurfaceNotReconstructable`] for a
/// committed compaction summary whose Surface `Replace` is not durably
/// reconstructable, and [`RecoverySafetyError::IncompleteToolTurn`] for an
/// `Assistant` tool call without its committed `ToolResult` sibling.
pub fn recovery_safety(messages: &[MessageBlock]) -> Result<(), RecoverySafetyError> {
    for message in messages {
        if let MessageBlock::User(user) = message
            && user.kind == InboundKind::CompactionSummary
        {
            return Err(RecoverySafetyError::CompactionSurfaceNotReconstructable(
                user.id.clone(),
            ));
        }
    }
    if let Some(tool_call_id) = pending_tool_call(messages) {
        return Err(RecoverySafetyError::IncompleteToolTurn { tool_call_id });
    }
    Ok(())
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
        let id = self.validate_commit(&message)?;
        Ok(self.install_prepared(PreparedCanonicalCommit { id, message }))
    }

    fn validate_commit(&self, message: &MessageBlock) -> Result<MessageId, ConversationError> {
        let id = message_id_of(message);
        if self.surface.is_active(&id) {
            return Err(ConversationError::Surface(SurfaceError::AlreadyActive(id)));
        }
        if self.ledger.contains(&id) {
            return Err(ConversationError::Ledger(LedgerError::DuplicateMessageId(
                id,
            )));
        }
        Ok(id)
    }

    /// Validates that `message` can be committed without mutating anything,
    /// producing a typed [`PreparedCanonicalCommit`] that binds the exact
    /// validated message to its install.
    ///
    /// This is the prepare half of the Issue #63 canonical-commit seam: the
    /// caller validates every fallible condition (duplicate Ledger identity,
    /// already-active Surface identity) against the current state, then
    /// commits the exact same message durably, and only then installs it with
    /// [`ConversationState::install_prepared`], which is infallible under
    /// exclusive ownership.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::Ledger`] for a duplicate `MessageId` and
    /// [`ConversationError::Surface`] when the identity is already active.
    pub fn prepare_commit(
        &self,
        message: &MessageBlock,
    ) -> Result<PreparedCanonicalCommit, ConversationError> {
        let id = self.validate_commit(message)?;
        Ok(PreparedCanonicalCommit {
            id,
            message: message.clone(),
        })
    }

    /// Installs a commit whose exact message was already validated by
    /// [`ConversationState::prepare_commit`].
    ///
    /// Infallible: the caller validated against this exact state and holds
    /// exclusive ownership, so neither the Ledger nor the Surface can reject
    /// the same message again. The installed message is the exact one carried
    /// inside the prepared value — no substitution is representable.
    pub(crate) fn install_prepared(&mut self, prepared: PreparedCanonicalCommit) -> MessageId {
        let id = prepared.id.clone();
        debug_assert!(!self.surface.is_active(&id));
        debug_assert!(!self.ledger.contains(&id));
        self.ledger.append_after_validation(prepared.message);
        self.surface.append_after_validation(id.clone());
        id
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

    /// Reconstructs the exact canonical messages of a historical Surface
    /// revision. Surface history supplies identities and order first; the
    /// Ledger is then queried only for those identities.
    ///
    /// # Errors
    ///
    /// Returns a conversation error when the revision is unavailable or a
    /// referenced message cannot be hydrated.
    pub fn reconstruct_messages(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageBlock>, ConversationError> {
        let ids = self.reconstruct(revision)?;
        self.hydrate(&ids)
    }

    /// Allocates a core-owned context `MessageId`. Transient contributors do
    /// not receive this allocator and therefore cannot choose canonical ids.
    #[must_use]
    pub fn allocate_context_message_id(&self, namespace: &str) -> MessageId {
        let mut serial = self.ledger.len();
        loop {
            let candidate = MessageId::new(format!("rustx-context-{namespace}-{serial}"));
            if !self.ledger.contains(&candidate) {
                return candidate;
            }
            serial = serial.saturating_add(1);
        }
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
    ) -> Result<PreparedCompactionCommit, ConversationError> {
        let replacement = summary.id.clone();
        let (start, end) = self.validate_compaction_span(&replacement, &span)?;
        Ok(PreparedCompactionCommit {
            summary,
            span,
            expected_revision: self.surface.revision(),
            start,
            end,
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

    /// Installs a compaction whose exact summary/span/indices were already
    /// validated by [`ConversationState::prepare_compaction`].
    ///
    /// Infallible: the caller validated against this exact state and holds
    /// exclusive ownership, so the span and identity re-validation can no
    /// longer fail. The installed summary and span are the exact values
    /// carried inside the prepared value — no substitution is representable.
    pub(crate) fn install_prepared_compaction(
        &mut self,
        prepared: PreparedCompactionCommit,
    ) -> CompactionRecord {
        debug_assert_eq!(prepared.expected_revision, self.surface.revision());
        let summary_message_id = self
            .ledger
            .append_after_validation(MessageBlock::User(prepared.summary));
        let surface_revision = self.surface.replace_after_validation(
            &prepared.span,
            summary_message_id.clone(),
            prepared.start,
            prepared.end,
        );
        CompactionRecord {
            summary_message_id,
            replaced: prepared.span,
            surface_revision,
            generation: self.surface.compaction_generation(),
        }
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
        prepared: PreparedCompactionCommit,
    ) -> Result<CompactionRecord, ConversationError> {
        let current = self.surface.revision();
        if prepared.expected_revision != current {
            return Err(ConversationError::Surface(SurfaceError::StaleRevision {
                expected: prepared.expected_revision,
                actual: current,
            }));
        }
        Ok(self.install_prepared_compaction(prepared))
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
        ConversationError, ConversationState, LedgerError, SurfaceError, SurfaceRevision,
        SurfaceSpan, message_id_of, summary_message_id,
    };
    use crate::conversation::structure::StructuralError;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContextKind, InboundKind, MessageBlock,
        SystemAuthority, SystemMessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock,
        UserSource,
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

    /// Two admitted Runtime context snapshots with identical rendered bytes
    /// remain distinct historical facts: identity is allocated at admission,
    /// never deduplicated by content.
    #[test]
    fn identical_runtime_context_bytes_are_distinct_admitted_facts() {
        let context = |id: MessageId| {
            MessageBlock::User(UserMessageBlock {
                id,
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "identical runtime snapshot".to_owned(),
                })],
                source: UserSource::Runtime,
                kind: InboundKind::Context(ContextKind::AgentStatus),
                timestamp: None,
            })
        };
        let mut state = ConversationState::new();
        let first_id = state.allocate_context_message_id("attempt-1-turn-1");
        state
            .commit(context(first_id.clone()))
            .expect("commit first");
        let second_id = state.allocate_context_message_id("attempt-1-turn-2");
        state
            .commit(context(second_id.clone()))
            .expect("commit second");

        assert_ne!(first_id, second_id);
        assert_eq!(state.ledger().len(), 2);
        assert_eq!(state.active_ids(), &[first_id, second_id]);
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
        let state =
            ConversationState::from_messages([user("a"), user("b"), user("c")]).expect("bootstrap");
        let before_ids = state.active_ids().to_vec();
        let before_revision = state.revision();
        let before_ledger_len = state.ledger().len();

        assert!(matches!(
            state.prepare_compaction(
                summary("a", "duplicate"),
                SurfaceSpan::new(MessageId::new("a"), MessageId::new("b")),
            ),
            Err(ConversationError::Ledger(LedgerError::DuplicateMessageId(id)))
                if id == MessageId::new("a")
        ));

        assert!(matches!(
            state.prepare_compaction(
                summary("s1", "invalid"),
                SurfaceSpan::new(MessageId::new("ghost"), MessageId::new("b")),
            ),
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
