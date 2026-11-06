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
    ContextError, ContextErrorKind, ContextSummarizer, SummaryRequest, TokenEstimator,
};
use rustx::message::types::MessageBlock;
use rustx::model::ModelInputMessage;
use rustx::tools::types::ModelToolDefinition;
use tokio::sync::watch;

/// One scripted step of a fake summarizer invocation.
#[derive(Debug, Clone)]
pub enum FakeSummaryStep {
    /// Return this summary text.
    Return(String),
    /// Fail with this context error.
    Fail(ContextError),
    /// Park until the owning operation is cancelled or reaches its deadline.
    ParkUntilCancelled,
}

/// A scripted deterministic summary service.
///
/// The script is a queue of invocation scripts: `summarize` pops the next
/// step per invocation and records every request, so tests can prove which
/// exact Surface span was selected, and cancellation, without network
/// access.
#[derive(Clone)]
pub struct FakeContextSummarizer {
    scripts: Arc<Mutex<VecDeque<FakeSummaryStep>>>,
    requests: Arc<Mutex<Vec<SummaryRequest>>>,
    parked: watch::Sender<bool>,
}

impl FakeContextSummarizer {
    /// Creates a fake summarizer from one scripted step per invocation.
    #[must_use]
    pub fn new(scripts: Vec<FakeSummaryStep>) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(scripts.into())),
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
    fn bind_backend(
        &self,
        _backend: Arc<dyn rustx::context::summarizer::SummaryBackend>,
        _resource_revision: u64,
    ) {
    }
    fn has_backend(&self) -> bool {
        true
    }
    fn operation(
        &self,
        budget: rustx::context::compaction_budget::CompactionBudget,
        cancellation: rustx::runtime::CancellationSignal,
    ) -> Result<Box<dyn rustx::context::summarizer::SummaryOperation>, ContextError> {
        Ok(Box::new(FakeSummaryOperation {
            summarizer: self.clone(),
            budget,
            cancellation: cancellation.child(),
        }))
    }
}

struct FakeSummaryOperation {
    summarizer: FakeContextSummarizer,
    budget: rustx::context::compaction_budget::CompactionBudget,
    cancellation: rustx::runtime::CancellationSignal,
}

impl rustx::context::summarizer::SummaryOperation for FakeSummaryOperation {
    fn budget(&self) -> rustx::context::compaction_budget::CompactionBudget {
        self.budget.clone()
    }
    fn prepare(
        &self,
    ) -> Result<Box<dyn rustx::context::summarizer::PreparedSummary>, ContextError> {
        self.budget.check(&self.cancellation)?;
        Ok(Box::new(FakePreparedSummary {
            summarizer: self.summarizer.clone(),
            cancellation: self.cancellation.child(),
            budget: self.budget.clone(),
        }))
    }
    fn authorize_commit(
        &self,
    ) -> Result<rustx::runtime::cancellation::CompactionCommitGrant, ContextError> {
        self.cancellation
            .authorize_compaction_commit_with(self.budget.operation_id().clone(), || {
                self.budget.check(&self.cancellation).is_ok()
            })
            .ok_or_else(|| {
                self.budget
                    .check(&self.cancellation)
                    .err()
                    .unwrap_or_else(|| {
                        ContextError::new(
                            ContextErrorKind::Cancelled,
                            "fake Summary operation closed",
                        )
                    })
            })
    }
    fn close(&self) -> BoxFuture<'_, ()> {
        self.cancellation.cancel();
        Box::pin(async {})
    }
}

struct FakePreparedSummary {
    summarizer: FakeContextSummarizer,
    cancellation: rustx::runtime::CancellationSignal,
    budget: rustx::context::compaction_budget::CompactionBudget,
}

impl rustx::context::summarizer::PreparedSummary for FakePreparedSummary {
    fn summarize(
        self: Box<Self>,
        request: SummaryRequest,
        _cut: Option<rustx::context::resources::ResourceReadCut>,
    ) -> BoxFuture<'static, Result<rustx::context::summarizer::SummaryGeneration, ContextError>>
    {
        self.summarizer
            .requests
            .lock()
            .expect("fake summarizer request lock")
            .push(request);
        let step = self.summarizer.pop_step();
        let parked = self.summarizer.parked.clone();
        let cancellation = self.cancellation;
        let budget = self.budget;
        Box::pin(async move {
            match step {
                FakeSummaryStep::Return(text) => {
                    Ok(rustx::context::summarizer::SummaryGeneration {
                        text,
                        generation: None,
                    })
                }
                FakeSummaryStep::Fail(error) => Err(error),
                FakeSummaryStep::ParkUntilCancelled => {
                    parked.send_replace(true);
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => Err(ContextError::new(ContextErrorKind::Cancelled, "parked summarizer cancelled")),
                        () = budget.wait_until_deadline() => Err(ContextError::new(ContextErrorKind::AuxiliaryDeadlineExceeded, "parked Summary exceeded its operation deadline")),
                    }
                }
            }
        })
    }
}

/// A deterministic scripted token estimator.
///
/// Weights:
///
/// - `per_message`: one whole canonical User or Tool message.
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
        messages: &[ModelInputMessage],
        effective_system_prompt: &str,
        tool_definitions: &[ModelToolDefinition],
    ) -> u64 {
        let mut total = tool_definitions.len() as u64 * self.per_tool;
        total += self.items_estimate(messages);
        total += (effective_system_prompt.len() as u64).div_ceil(4);
        total
    }

    fn estimate_conversation_input(&self, messages: &[MessageBlock]) -> u64 {
        let messages = rustx::model::input::canonical_input(messages);
        self.items_estimate(&messages)
    }
}

impl ScriptedEstimator {
    /// The conversation-content estimate: message/block/summary weights only,
    /// never the fixed per-tool overhead.
    fn items_estimate(&self, messages: &[ModelInputMessage]) -> u64 {
        messages
            .iter()
            .map(|message| match message {
                ModelInputMessage::Canonical(message) => self.message_estimate(message),
                ModelInputMessage::Derived(derived) => match &derived.message {
                    rustx::model::projected::ProjectedMessage::User { content, .. }
                        if derived.transformations.contains(
                            &rustx::model::projected::SemanticTransformation::Summary,
                        ) =>
                    {
                        content
                            .iter()
                            .filter_map(|b| match b {
                                rustx::message::types::UserContentBlock::Text(t) => {
                                    Some(t.text.len() as u64)
                                }
                                _ => None,
                            })
                            .sum::<u64>()
                            .div_ceil(4)
                    }
                    rustx::model::projected::ProjectedMessage::User { .. }
                    | rustx::model::projected::ProjectedMessage::Tool { .. } => self.per_message,
                    rustx::model::projected::ProjectedMessage::Assistant { content } => {
                        content.len() as u64 * self.per_block
                    }
                },
                ModelInputMessage::RequestOnly(context) => {
                    (context.render().len() as u64).div_ceil(4)
                }
            })
            .sum()
    }

    /// The weight of one complete canonical message.
    #[must_use]
    pub fn message_estimate(&self, message: &MessageBlock) -> u64 {
        if let Some(weight) = self.overrides.get(message_id(message).as_str()) {
            *weight
        } else if let MessageBlock::Assistant(assistant) = message {
            assistant.content.len() as u64 * self.per_block
        } else {
            self.per_message
        }
    }
}

fn message_id(message: &MessageBlock) -> rustx::runtime::identity::MessageId {
    match message {
        MessageBlock::User(user) => user.id.clone(),
        MessageBlock::Assistant(assistant) => assistant.id.clone(),
        MessageBlock::Tool(tool) => tool.id.clone(),
    }
}

/// A shared fake summarizer handle for tests that need to observe and
/// release it across an execution.
#[must_use]
pub fn shared_summarizer(summarizer: FakeContextSummarizer) -> Arc<FakeContextSummarizer> {
    Arc::new(summarizer)
}

/// Native durable checkpoint fixture. Selection is explicit in model-input indices;
/// the fixture never inserts a summary into the canonical Ledger.
pub(crate) fn checkpoint_fixture(
    store: &dyn rustx::durable::ConversationStore,
    id: &str,
    retired: usize,
    text: &str,
) -> rustx::context::checkpoint::CheckpointCandidate {
    use rustx::context::checkpoint::{CheckpointCandidate, CheckpointItem, CheckpointMessage};
    use rustx::model::projected::{
        CanonicalMessageRef, CheckpointSourceRef, DerivedModelMessage, ProjectedMessage,
        SemanticTransformation,
    };
    let history = store.projected_history().expect("fixture historical input");
    assert!(retired > 0 && retired <= history.messages.len());
    let parent = store.projection_head().unwrap();
    let originals: Vec<_> = history
        .messages
        .iter()
        .take(retired)
        .filter_map(|m| m.as_canonical().cloned())
        .collect();
    let physical = crate::context::compaction_metadata::retired_span_metadata(&originals);
    let uses_parent=history.messages.iter().take(retired).any(|m|matches!(m,ModelInputMessage::Derived(d) if d.transformations.contains(&SemanticTransformation::Summary)));
    let metadata = rustx::message::types::CompactionSummaryMetadata::accumulate(
        parent
            .as_ref()
            .filter(|_| uses_parent)
            .and_then(|p| p.candidate.summary_metadata.as_ref()),
        physical.read_files().to_vec(),
        physical.modified_files().to_vec(),
    );
    let checkpoint_id = rustx::runtime::identity::CheckpointId::new(id);
    let mut items = vec![CheckpointItem {
        message: CheckpointMessage::Derived(Box::new(DerivedModelMessage {
            message: ProjectedMessage::User {
                content: vec![rustx::message::types::UserContentBlock::Text(
                    rustx::message::TextBlock {
                        text: crate::context::compaction_metadata::render_summary_text(
                            text, &metadata,
                        ),
                    },
                )],
                source: rustx::message::types::UserSource::Runtime,
                kind: rustx::model::projected::ProjectedUserKind::Message,
                timestamp: None,
            },
            sources: originals
                .iter()
                .map(|m| CanonicalMessageRef::of(store.conversation_id(), m))
                .collect(),
            checkpoint_sources: parent
                .as_ref()
                .filter(|_| uses_parent)
                .map(|p| CheckpointSourceRef {
                    conversation_id: store.conversation_id().clone(),
                    checkpoint_id: p.candidate.checkpoint_id.clone(),
                    content_digest: p.fingerprint.clone(),
                })
                .into_iter()
                .collect(),
            transformations: vec![SemanticTransformation::Summary],
        })),
        call_sources: vec![],
        result_call: None,
    }];
    for mut item in history.items.into_iter().skip(retired) {
        if let Some(call) = &mut item.result_call {
            call.message.checkpoint_id = checkpoint_id.clone();
            call.message.item_index = call
                .message
                .item_index
                .checked_sub(u32::try_from(retired).expect("bounded fixture prefix"))
                .unwrap()
                + 1;
        }
        items.push(item);
    }
    CheckpointCandidate {
        checkpoint_id,
        expected_head: history.cut.checkpoint_id,
        read_cut: store.compaction_resource_cut().unwrap(),
        base_frontier: history.cut.ledger,
        items,
        summary_metadata: Some(metadata),
        summary_generation: None,
    }
}

#[allow(clippy::needless_pass_by_value)] // Fixture constructs an owned commit receipt input.
pub(crate) fn checkpoint_commit_fixture(
    store: &dyn rustx::durable::ConversationStore,
    candidate: rustx::context::checkpoint::CheckpointCandidate,
    attempt: Option<rustx::runtime::identity::AttemptId>,
) -> rustx::durable::projection::ProjectionCommitInput {
    let operation_id = rustx::runtime::identity::CompactionOperationId::new(format!(
        "operation:{}",
        candidate.checkpoint_id
    ));
    let signal = rustx::runtime::CancellationSignal::new();
    rustx::durable::projection::ProjectionCommitInput {
        grant: signal
            .authorize_compaction_commit(operation_id.clone())
            .unwrap(),
        operation_id,
        candidate,
        owner: store.projection_owner(attempt.as_ref()).unwrap(),
        tokens_before: rustx::runtime::types::TokenMeasurement {
            input_tokens: 100,
            source: rustx::runtime::types::TokenMeasurementSource::Estimated,
        },
        estimated_tokens_after: 10,
        occupancy: None,
        turn_id: None,
        timestamp: chrono::Utc::now(),
    }
}
