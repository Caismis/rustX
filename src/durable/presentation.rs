//! Indexed read seams over existing Journal rows. No stored projection.
use crate::runtime::identity::{AttemptId, TurnId};

/// Closed indexed correlation vocabulary. Values never enter SQL source text.
#[derive(Debug, Clone)]
pub enum FactScope {
    All,
    Step(AttemptId, TurnId),
    Attempt(AttemptId),
    Request(String),
    ToolCall {
        call_id: String,
        attempt: Option<AttemptId>,
        turn: Option<TurnId>,
    },
    Execution(String),
    Subagent(String),
    Workflow(String),
    Interaction(String),
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

/// Leaf, non-authoritative observation of a committed Journal prefix. Called
/// under the store serialization lock; implementations may only enqueue.
pub trait JournalObserver: Send + Sync {
    /// None fences the read model if a committed prefix cannot be established.
    fn committed(&self, through: Option<u64>);
}
