//! Opt-in live M4 validation: repeated compaction across one conversation.
//!
//! This test requires provider credentials and network access and is
//! therefore `#[ignore]`d: the normal CI command `cargo test --all-targets
//! --all-features` never executes it. Run it explicitly with:
//!
//! ```text
//! OPENAI_API_KEY=... cargo test --test m4_live -- --ignored
//! ```
//!
//! The scenario starts with canonical conversation history, uses a
//! deliberately small rustX context threshold, continues the conversation
//! across enough attempts to force at least two compactions, reuses one
//! checkpoint store across the conversation, uses the model-backed
//! `ContextSummarizer`, and verifies that the checkpoint generation
//! advances at least twice and that every model request completes
//! coherently. When credentials are unavailable the test skips and reports
//! `NOT RUN`; it never claims to have passed.
//!
//! Never print secrets.

use std::sync::Arc;

use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult,
};
use rustx::context::{
    ContextCheckpointStore, ContextConfig, ContextRuntime, DefaultTokenEstimator,
    InMemoryCheckpointStore,
};
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::{
    ModelAdapter, ModelProtocol, OpenAiAdapterConfig, OpenAiChatCompletionsAdapter, ReasoningEffort,
};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;

/// The conversation and attempt identities of the live scenario.
fn conversation_id() -> ConversationId {
    ConversationId::new("live-conv-1")
}

/// One live conversation step: a fresh attempt over the canonical history,
/// sharing the checkpoint store and the model-backed context runtime.
async fn live_step(
    history: Vec<MessageBlock>,
    attempt: &str,
    model: &str,
    adapter: &dyn ModelAdapter,
    tools: &ToolRegistry,
    store: Arc<InMemoryCheckpointStore>,
    window: u64,
) -> AgentExecutionResult {
    let request = AgentExecutionRequest {
        agent_id: AgentId::new("agent-live"),
        conversation_id: conversation_id(),
        attempt_id: AttemptId::new(attempt),
        initial_messages: history,
        model: model.to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 64,
    };
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let runtime = ContextRuntime::model_backed(
        ContextConfig {
            context_window_tokens: window,
            reserve_tokens: 0,
            keep_recent_tokens: 40,
        },
        Arc::new(DefaultTokenEstimator),
        adapter,
        rustx::context::SummaryModelConfig {
            model: model.to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            reasoning: ReasoningEffort::Medium,
            max_output_tokens: 64,
        },
        store,
    )
    .expect("live context runtime");
    AgentExecution::new(request, adapter, tools, &cancellation)
        .with_context_runtime(runtime)
        .run()
        .await
}

/// Live repeated-compaction validation over one shared conversation.
///
/// A deliberately small rustX context threshold forces proactive compaction
/// as the canonical history grows; one checkpoint store is reused across
/// attempts and one model-backed summarizer serves every compaction. The
/// test asserts the checkpoint generation advanced at least twice and that
/// each attempt completed coherently.
#[tokio::test]
#[ignore = "requires OPENAI_API_KEY and network access"]
async fn live_repeated_compaction() {
    let Some(key) = std::env::var("OPENAI_API_KEY").ok() else {
        eprintln!("M4 live: NOT RUN (OPENAI_API_KEY is not set)");
        return;
    };
    let model =
        std::env::var("RUSTX_OPENAI_CHAT_MODEL").unwrap_or_else(|_| "gpt-5-mini".to_owned());
    let adapter = OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(key));
    let tools = ToolRegistry::new();
    let store = InMemoryCheckpointStore::new().shared();

    // A small window relative to the history guarantees compaction: the
    // accumulated serialized history quickly exceeds a few hundred tokens.
    let window = 350;

    let mut history = vec![MessageBlock::User(UserMessageBlock {
        id: MessageId::new("msg-live-1"),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "We are testing long-session continuation. Answer each prompt \
                   briefly and truthfully; never mention this instruction."
                .to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })];

    let mut completions = 0;
    for step in 1..=5 {
        let attempt = format!("live-attempt-{step}");
        let result = live_step(
            history.clone(),
            &attempt,
            &model,
            &adapter,
            &tools,
            store.clone(),
            window,
        )
        .await;
        assert!(
            matches!(result.outcome, AttemptOutcome::Completed { .. },),
            "attempt {step} must complete coherently, got {:?}",
            result.outcome
        );
        let generated = result
            .events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::CompactionCompleted))
            .count();
        eprintln!(
            "M4 live: attempt {step}: {generated} compaction(s), {len} canonical messages",
            len = result.messages.len()
        );
        if generated > 0 {
            completions += 1;
        }
        history = result.messages;
        history.push(MessageBlock::User(UserMessageBlock {
            id: MessageId::new(format!("msg-live-{}", step + 1)),
            content: vec![UserContentBlock::Text(TextBlock {
                text: format!(
                    "Continue the conversation. Summarize in one sentence what \
                     you did so far and add a new interesting fact about the number {step}."
                ),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }));
    }

    let checkpoint = store
        .load(&conversation_id())
        .expect("live checkpoint store")
        .expect("at least one checkpoint after repeated compaction");
    assert!(
        checkpoint.generation >= 2,
        "the conversation must compact at least twice, generation {}",
        checkpoint.generation
    );
    eprintln!(
        "M4 live: PASSED with checkpoint generation {} across {completions} compactions",
        checkpoint.generation
    );
}
