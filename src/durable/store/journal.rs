//! Store contract for the Event Journal.

use crate::events::types::RuntimeEventEnvelope;

use crate::durable::{ConversationStoreError, TranscriptCursor};

/// A page of durable Event Journal envelopes.
#[derive(Debug, Clone, PartialEq)]
pub struct EventPage {
    /// Events in stable conversation sequence order.
    pub events: Vec<RuntimeEventEnvelope>,
    /// The last event sequence in this page, when non-empty.
    pub next_sequence: Option<u64>,
}

/// The Event Journal domain of the conversation Store: validated execution
/// facts, private Agent authority captured with its admission, and bounded
/// Journal reads for presentation consumers.
#[allow(clippy::missing_errors_doc)]
pub trait JournalStore: Send + Sync + 'static {
    /// Private executable Agent authority, never copied into public history or events.
    fn load_agent_authority(
        &self,
        agent_id: &crate::runtime::identity::AgentId,
    ) -> Result<crate::runtime::subagent::DurableAgentAuthority, ConversationStoreError>;
    /// Atomically admit one private authority and its public ownership reference.
    fn append_agent_admission(
        &self,
        event: RuntimeEventEnvelope,
        authority: &crate::runtime::subagent::DurableAgentAuthority,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;
    /// Appends one standalone execution fact after validating every durable
    /// reference and lifecycle terminal rule. Canonical-message,
    /// compaction-completion, request-start, and background-publication facts
    /// must use their specialized combined transition; this method rejects
    /// them so a reference event cannot be split from its durable authority.
    fn append_event(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;
    /// Commits a non-success Workflow-owned child terminal fact without a
    /// parent inbound notification. Successful Workflow terminals must use
    /// [`JournalStore::commit_workflow_agent_terminal`] so their value
    /// and lifecycle fact cannot split across durable transitions.
    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError>;
    /// Commits a successful Workflow Agent's structured value and its native
    /// `SubagentTerminalSettled` lifecycle fact in one durable transaction.
    /// This specialized transition is the Workflow terminal boundary: it
    /// records execution evidence without creating a parent message or a
    /// durable `settled -> delivered` state.
    fn commit_workflow_agent_terminal(
        &self,
        terminal: RuntimeEventEnvelope,
        output: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, RuntimeEventEnvelope), ConversationStoreError>;
    /// Commits one interaction audit event and returns the transcript
    /// position allocated in the same durable transaction.
    fn append_interaction_audit(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError>;
    /// Atomically install the leaf Journal observer and capture its initial prefix.
    fn observe_journal(
        &self,
        observer: std::sync::Arc<dyn crate::durable::presentation::JournalObserver>,
    ) -> Result<u64, ConversationStoreError>;
    /// Latest committed Journal position, read without enumerating history.
    fn presentation_frontier(&self) -> Result<u64, ConversationStoreError>;
    /// Indexed, bounded execution-fact reads for presentation consumers.
    /// `before` is exclusive; `through` fixes a durable read cut. This is an
    /// internal Journal query, never a public cursor or a semantic authority.
    fn read_presentation_events(
        &self,
        query: &crate::durable::presentation::FactQuery,
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError>;
    /// Reads a bounded Event Journal page in stable sequence order.
    fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<EventPage, ConversationStoreError>;
}
