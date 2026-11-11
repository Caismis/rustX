//! Indexed read seams over existing Journal rows. No stored projection.
use crate::runtime::identity::{AttemptId, TurnId};

/// Closed indexed correlation vocabulary. Values never enter SQL source text.
#[derive(Debug, Clone)]
pub enum FactScope {
    All,
    /// Physical request facts with explicit primary Agent ownership.
    AgentRequest,
    /// Terminal request usage or a completed compaction, including an absent estimate.
    /// An expression index skips usage-free requests without scanning their history.
    ContextReading,
    Step(AttemptId, TurnId),
    Attempt(AttemptId),
    Request(String),
    Compaction(crate::runtime::identity::CompactionOperationId),
    ToolCall {
        call_id: String,
        attempt: Option<AttemptId>,
        turn: Option<TurnId>,
    },
    /// Exact canonical message identity, event kind and sequence are indexed together.
    Message(String),
    Execution(String),
    Subagent(String),
    Workflow(String),
    Interaction(String),
}

/// Every Journal fact of the given kinds, in Journal order, read through the
/// kind index at one captured frontier.
///
/// # Errors
///
/// Returns the store failure of any page.
pub fn all_facts(
    store: &dyn crate::durable::JournalStore,
    kinds: &[&'static str],
) -> Result<Vec<crate::events::types::RuntimeEventEnvelope>, crate::durable::ConversationStoreError>
{
    const PAGE: usize = 128;
    let through = store.presentation_frontier()?;
    let mut facts = Vec::new();
    let mut after = 0;
    loop {
        let page = store.read_presentation_events(&FactQuery {
            scope: FactScope::All,
            kinds: kinds.to_vec(),
            before: None,
            after,
            ascending: true,
            through,
            limit: PAGE,
        })?;
        let full = page.len() == PAGE;
        let Some(last) = page.last() else {
            break;
        };
        after = last.sequence;
        facts.extend(page);
        if !full {
            break;
        }
    }
    Ok(facts)
}

/// A finite ordered query over authoritative facts at one captured frontier.
#[derive(Debug, Clone)]
pub struct FactQuery {
    pub scope: FactScope,
    pub kinds: Vec<&'static str>,
    pub before: Option<u64>,
    pub after: u64,
    pub ascending: bool,
    pub through: u64,
    pub limit: usize,
}

/// Leaf staging receipt for committed Journal rows, ordered under the store
/// serialization lock. This is not semantic publication: a consumer must wait
/// for each corresponding owner's native installation/publication receipt.
/// Implementations may only enqueue; they never control execution or recovery.
pub trait JournalObserver: Send + Sync {
    /// None fences the read model if a committed prefix cannot be established.
    fn committed(&self, through: Option<u64>);
    /// Native source publication became readable after its own storage commit.
    fn ready(&self, _through: u64) {}
}
