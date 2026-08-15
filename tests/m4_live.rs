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
mod common;

/// The scenario starts with canonical conversation history, uses a
/// deliberately small rustX context threshold, continues the conversation
/// across enough attempts to force at least two compactions, reuses one
/// checkpoint store across the conversation, uses the model-backed
/// `ContextSummarizer`, and verifies that the checkpoint generation
/// advances at least twice and that every model request completes
/// coherently. When credentials are unavailable the test skips and reports
/// `NOT RUN`; it never claims to have passed.
///
/// Never print secrets.
use std::sync::Arc;

use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult,
};
use rustx::context::{ContextRuntime, DefaultTokenEstimator};
use rustx::conversation::ConversationState;
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::catalog::ProcessCredentialEnvironment;
use rustx::model::session::{SessionModelConfig, SessionModelState};
use rustx::model::{AttemptModelSnapshot, ModelBindingRegistry, ModelCatalog, ModelRef};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;

/// The conversation and attempt identities of the live scenario.
fn conversation_id() -> ConversationId {
    ConversationId::new("live-conv-1")
}

/// The live session model authority.
///
/// The binding goes through the ordinary production path: an explicit
/// catalog document with an explicit endpoint and a `$OPENAI_API_KEY`
/// credential source, resolved against the process environment, bound by
/// [`ModelBindingRegistry::new`] — which is the one binding path and
/// constructs the real Chat Completions adapter itself. Nothing here
/// substitutes an adapter.
fn live_session_model(base_url: &str, model: &str, window: u64) -> SessionModelState {
    let document = serde_json::json!({
        "providers": {
            "openai": {
                "baseUrl": base_url,
                "apiKey": "$OPENAI_API_KEY",
                "models": [{
                    "id": model,
                    "protocol": "openai_chat_completions",
                    "contextWindow": window,
                    "maxOutputTokens": 64,
                    "capabilities": {
                        "inputModalities": ["text"],
                    "outputModalities": ["text"],
                    "toolCalls": true,
                    "reasoning": false
                    },
                    "compat": {"chatReasoningReplay": "omit"}
                }]
            }
        }
    });
    let catalog = ModelCatalog::from_json_slice(
        &serde_json::to_vec(&document).expect("the live catalog document serializes"),
    )
    .expect("the live catalog validates");
    let resolved = catalog
        .resolve(&ProcessCredentialEnvironment)
        .expect("OPENAI_API_KEY resolves");
    let registry = ModelBindingRegistry::new(resolved).expect("the live provider binds");
    SessionModelState::new(
        registry,
        SessionModelConfig::of(
            ModelRef::parse(&format!("openai/{model}")).expect("a valid model reference"),
        ),
    )
    .expect("the live session model resolves")
}

/// One live conversation step: a fresh attempt that takes ownership of the
/// one canonical conversation state and the model-backed context runtime.
async fn live_step(
    conversation: ConversationState,
    attempt: &str,
    snapshot: AttemptModelSnapshot,
    tools: ToolRegistry,
) -> AgentExecutionResult {
    // One attempt snapshot drives everything: the loop's provider binding,
    // the engine window, and the summary invocation. In `session` summary
    // mode the summary uses exactly this primary invocation.
    let request = AgentExecutionRequest {
        agent_id: AgentId::new("agent-live"),
        conversation_id: conversation_id(),
        attempt_id: AttemptId::new(attempt),
        conversation,
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: snapshot.clone(),
    };
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let runtime = ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 40,
            summary_output_cap: None,
        },
        Arc::new(DefaultTokenEstimator),
        rustx::context::AgentStatusComposer::default(),
        &snapshot,
    )
    .expect("live context runtime");
    // One conversation identity authority: the tool runtime is built from the
    // same value the request carries, so `AgentExecution` construction proves
    // ownership rather than failing its `ConversationMismatch` validation.
    let tool_runtime = common::tool_runtime(conversation_id().as_str());
    let capability = common::capability_lease(tools, &tool_runtime).await;
    AgentExecution::new(
        request,
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .expect("conversation identity matches the tool runtime")
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
    if std::env::var_os("OPENAI_API_KEY").is_none() {
        eprintln!("M4 live: NOT RUN (OPENAI_API_KEY is not set)");
        return;
    }
    let model =
        std::env::var("RUSTX_OPENAI_CHAT_MODEL").unwrap_or_else(|_| "gpt-5-mini".to_owned());
    let base_url = std::env::var("RUSTX_OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
    // A small window relative to the history guarantees compaction: the
    // accumulated serialized history quickly exceeds a few hundred tokens.
    let window = 350;
    let session_model = live_session_model(&base_url, &model, window);

    let mut conversation =
        ConversationState::from_messages([MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-live-1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "We are testing long-session continuation. Answer each prompt \
                       briefly and truthfully; never mention this instruction."
                    .to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })])
        .expect("bootstrap conversation");

    let mut completions = 0;
    for step in 1..=5 {
        let attempt = format!("live-attempt-{step}");
        let result = live_step(
            conversation,
            &attempt,
            session_model.snapshot(),
            ToolRegistry::new(),
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
            .filter(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. }))
            .count();
        eprintln!(
            "M4 live: attempt {step}: {generated} compaction(s), {len} canonical messages",
            len = result.conversation.ledger().len()
        );
        if generated > 0 {
            completions += 1;
        }
        conversation = result.conversation;
        conversation
            .commit(MessageBlock::User(UserMessageBlock {
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
            }))
            .expect("commit the next inbound message");
    }

    let generation = conversation.surface().compaction_generation();
    assert!(
        generation >= 2,
        "the conversation must compact at least twice, generation {generation}"
    );
    eprintln!(
        "M4 live: PASSED with compaction generation {generation} across {completions} compactions"
    );
}
