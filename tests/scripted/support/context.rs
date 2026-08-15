//! M4 deterministic context fixtures: a scripted summarizer and a
//! deterministic scripted token estimator.
//!
//! All M4 semantics are deterministic and network-free: the fake summarizer
//! records every `SummaryRequest`, returns scripted summaries, can fail, and
//! can park until cancellation, and the scripted estimator derives every
//! decision from exact token weights.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use rustx::context::{
    ContextError, ContextErrorKind, ContextProjection, ContextSummarizer, SummaryRequest,
    TokenEstimator,
};
use rustx::message::types::{InboundKind, MessageBlock};
use rustx::tools::types::ModelToolDefinition;
use tokio::sync::watch;

/// One scripted step of a fake summarizer invocation.
#[derive(Debug, Clone)]
pub enum FakeSummaryStep {
    /// Return this summary text.
    Return(String),
    /// Fail with this context error.
    Fail(ContextError),
    /// Park until the invocation's cancellation signal fires. The future is
    /// expected to be dropped when cancellation wins the biased race.
    ParkUntilCancelled,
}

/// A scripted deterministic summary service.
///
/// The script is a queue of invocation scripts: `summarize` pops the next
/// step per invocation and records every request, so tests can prove which
/// exact Surface span was selected, and cancellation, without network
/// access.
pub struct FakeContextSummarizer {
    scripts: Mutex<VecDeque<FakeSummaryStep>>,
    requests: Arc<Mutex<Vec<SummaryRequest>>>,
    parked: watch::Sender<bool>,
}

impl FakeContextSummarizer {
    /// Creates a fake summarizer from one scripted step per invocation.
    #[must_use]
    pub fn new(scripts: Vec<FakeSummaryStep>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Arc::new(Mutex::new(Vec::new())),
            parked: watch::Sender::new(false),
        }
    }

    /// The summary requests received so far, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<SummaryRequest> {
        self.requests.lock().expect("fake summarizer lock").clone()
    }

    /// A receiver observing whether the current invocation is parked
    /// awaiting cancellation.
    #[must_use]
    pub fn parked(&self) -> watch::Receiver<bool> {
        self.parked.subscribe()
    }

    fn pop_step(&self) -> FakeSummaryStep {
        self.scripts
            .lock()
            .expect("fake summarizer script lock")
            .pop_front()
            .unwrap_or_else(|| {
                FakeSummaryStep::Fail(ContextError::new(
                    ContextErrorKind::SummaryFailed,
                    "fake summarizer script exhausted",
                ))
            })
    }
}

impl ContextSummarizer for FakeContextSummarizer {
    fn summarize(
        &self,
        request: SummaryRequest,
        cancellation: rustx::runtime::CancellationSignal,
    ) -> BoxFuture<'_, Result<String, ContextError>> {
        self.requests
            .lock()
            .expect("fake summarizer request lock")
            .push(request);
        let step = self.pop_step();
        let parked = self.parked.clone();
        Box::pin(async move {
            match step {
                FakeSummaryStep::Return(text) => Ok(text),
                FakeSummaryStep::Fail(error) => Err(error),
                FakeSummaryStep::ParkUntilCancelled => {
                    parked.send_replace(true);
                    cancellation.cancelled().await;
                    Err(ContextError::new(
                        ContextErrorKind::Cancelled,
                        "parked summarizer cancelled",
                    ))
                }
            }
        })
    }
}

/// A deterministic scripted token estimator.
///
/// Weights:
///
/// - `per_message`: one whole user/tool/system message.
/// - `per_block`: one content block of an Assistant message.
/// - `per_tool`: one tool definition.
/// - `per_summary_byte`: one summary text byte; a compaction summary message
///   weighs `ceil(bytes / 4)`, mirroring the default estimator formula, so
///   scripted summaries can deterministically produce no-progress
///   compactions.
/// - the Effective System Prompt contributes to the full request estimate but
///   never to the recent-conversation retention estimate.
/// - per-message overrides by `MessageId`, applied before role defaults.
#[derive(Debug, Clone)]
pub struct ScriptedEstimator {
    per_message: u64,
    per_block: u64,
    per_tool: u64,
    overrides: HashMap<String, u64>,
}

impl ScriptedEstimator {
    /// Creates a scripted estimator with the given base weights.
    #[must_use]
    pub fn new(per_message: u64, per_block: u64, per_tool: u64) -> Self {
        Self {
            per_message,
            per_block,
            per_tool,
            overrides: HashMap::new(),
        }
    }

    /// Overrides the weight of one message by its id.
    #[must_use]
    pub fn with_override(mut self, message_id: &str, tokens: u64) -> Self {
        self.overrides.insert(message_id.to_owned(), tokens);
        self
    }
}

impl TokenEstimator for ScriptedEstimator {
    fn estimate_input(
        &self,
        projection: &ContextProjection,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        let mut total = tool_definitions.len() as u64 * self.per_tool;
        total += self.items_estimate(projection);
        total += (projection.effective_system_prompt.len() as u64).div_ceil(4);
        total
    }

    fn estimate_conversation_input(&self, projection: &ContextProjection) -> u64 {
        self.items_estimate(projection)
    }
}

impl ScriptedEstimator {
    /// The conversation-content estimate: message/block/summary weights only,
    /// never the fixed per-tool overhead.
    fn items_estimate(&self, projection: &ContextProjection) -> u64 {
        projection
            .messages
            .iter()
            .map(|message| self.message_estimate(message))
            .sum()
    }

    /// The weight of one complete canonical message.
    #[must_use]
    pub fn message_estimate(&self, message: &MessageBlock) -> u64 {
        if let Some(weight) = self.overrides.get(message_id(message).as_str()) {
            *weight
        } else if is_summary(message) {
            (summary_text(message).len() as u64).div_ceil(4)
        } else if let MessageBlock::Assistant(assistant) = message {
            assistant.content.len() as u64 * self.per_block
        } else {
            self.per_message
        }
    }
}

fn message_id(message: &MessageBlock) -> rustx::runtime::identity::MessageId {
    match message {
        MessageBlock::System(system) => system.id.clone(),
        MessageBlock::User(user) => user.id.clone(),
        MessageBlock::Assistant(assistant) => assistant.id.clone(),
        MessageBlock::Tool(tool) => tool.id.clone(),
    }
}

fn is_summary(message: &MessageBlock) -> bool {
    matches!(
        message,
        MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
    )
}

fn summary_text(message: &MessageBlock) -> String {
    let MessageBlock::User(user) = message else {
        return String::new();
    };
    user.content
        .iter()
        .filter_map(|block| match block {
            rustx::message::types::UserContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A shared fake summarizer handle for tests that need to observe and
/// release it across an execution.
#[must_use]
pub fn shared_summarizer(summarizer: FakeContextSummarizer) -> Arc<FakeContextSummarizer> {
    Arc::new(summarizer)
}
