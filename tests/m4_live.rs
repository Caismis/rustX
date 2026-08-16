//! Opt-in live multi-compaction validation on the final pre-M8 path
//! (Issue #27).
//!
//! This test requires provider credentials and network access and is
//! therefore `#[ignore]`d: the normal CI command `cargo test --all-targets
//! --all-features` never executes it. Run it explicitly with:
//!
//! ```text
//! OPENAI_API_KEY=... cargo test --test m4_live -- --ignored
//! ```
//!
//! Unlike the M4-era version of this test — which drove `AgentExecution`
//! directly and threaded `ConversationState` by hand — this scenario
//! composes the production local runtime (`LocalConversationRuntime`) and
//! drives everything through the Runtime Client host, exactly the path a
//! real client exercises:
//!
//! ```text
//! this test
//!   |  submit_inbound / observation stream
//!   v
//! LocalConversationRuntime (real composition, no injected adapter)
//!   |
//!   v
//! real provider adapter -> real HTTP + SSE -> the live provider
//! ```
//!
//! A deliberately small context window forces proactive compaction as the
//! conversation grows. The test asserts every attempt completes coherently,
//! at least two compactions commit in generation order, both canonical
//! compaction summaries are ledger facts, and every actual primary request
//! stays reconstructible from its frozen snapshot. When credentials are
//! unavailable the test skips and reports `NOT RUN`; it never claims to have
//! passed.
//!
//! Never print secrets.

use std::sync::Arc;

use rustx::local_runtime::composition::{
    LocalConversationRuntime, LocalRuntimeDependencies, LocalRuntimePaths,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{InboundKind, MessageBlock, UserContentBlock};
use rustx::model::catalog::ProcessCredentialEnvironment;
use rustx::runtime_client::host::{EventDelivery, EventSubscription, RuntimeClientHost};
use rustx::runtime_client::types::RuntimeClientResult;
use rustx::runtime_client::{
    RUNTIME_CLIENT_PROTOCOL_VERSION_V1, RuntimeClientEvent, RuntimeClientOutcome,
};

/// The catalog document: one real Chat Completions model with a deliberately
/// small window, an explicit endpoint, and a `$OPENAI_API_KEY` credential
/// source the runtime resolves from the process environment — exactly as in
/// production.
fn models_json(base_url: &str, model: &str, window: u64) -> String {
    serde_json::json!({
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
    })
    .to_string()
}

/// The session document: the same small-threshold policy the M4-era live
/// test used (no reserve, a small literal tail), selected through the
/// production session loader.
fn session_json(model: &str) -> String {
    serde_json::json!({
        "conversationId": "conv-live-27",
        "agentId": "agent-live-27",
        "model": {"model": format!("openai/{model}")},
        "context": {
            "reserveTokens": 0,
            "keepRecentTokens": 40,
        },
    })
    .to_string()
}

/// Consumes the observation stream up to the attempt's terminal settlement,
/// returning every delivered event. Live network latency is the only thing
/// the timeout bounds; ordering is always the stream's.
async fn settle(events: &EventSubscription) -> Vec<RuntimeClientEvent> {
    let mut observed = Vec::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(120), events.next()).await {
            Ok(EventDelivery::Event(published)) => {
                let terminal = matches!(published.event, RuntimeClientEvent::AttemptSettled { .. });
                observed.push(published.event);
                if terminal {
                    return observed;
                }
            }
            Ok(other) => panic!("the observation stream ended early: {other:?}"),
            Err(elapsed) => panic!("the live attempt never settled ({elapsed})"),
        }
    }
}

/// Live repeated-compaction validation over the production runtime path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires OPENAI_API_KEY and network access"]
// The composed live proof is one scenario observed end to end; keeping it
// together preserves the turn ordering that is the whole point.
#[allow(clippy::too_many_lines)]
async fn live_repeated_compaction() {
    if std::env::var_os("OPENAI_API_KEY").is_none() {
        eprintln!("M4 live: NOT RUN (OPENAI_API_KEY is not set)");
        return;
    }
    let model =
        std::env::var("RUSTX_OPENAI_CHAT_MODEL").unwrap_or_else(|_| "gpt-5-mini".to_owned());
    let base_url = std::env::var("RUSTX_OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
    // A small window relative to the accumulated history guarantees repeated
    // compaction within a handful of brief turns.
    let window = 350;

    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(
        root.path().join("models.json"),
        models_json(&base_url, &model, window),
    )
    .expect("models.json");
    std::fs::write(root.path().join("session.json"), session_json(&model)).expect("session.json");

    let paths = LocalRuntimePaths {
        models: root.path().join("models.json"),
        session: root.path().join("session.json"),
        workspace,
        runtime_root: root.path().join("private"),
    };
    let dependencies = LocalRuntimeDependencies {
        credentials: Arc::new(ProcessCredentialEnvironment),
        ..LocalRuntimeDependencies::default()
    };
    let runtime = LocalConversationRuntime::compose(&paths, &dependencies)
        .await
        .expect("the real runtime composes against the live catalog");
    let host: &RuntimeClientHost = runtime.host();
    let (attachment, result) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let RuntimeClientResult::Initialized { cursor, .. } = result else {
        panic!("initialize returns the snapshot");
    };
    let (events, _) = host
        .subscribe_events(attachment.attachment_id(), cursor)
        .expect("subscribe");
    // The attachment is held for the test's lifetime: dropping it detaches,
    // which releases the subscription mid-test.
    let _attachment = attachment;

    let prompts = [
        "We are testing long-session continuation. Answer each prompt briefly \
         and truthfully; never mention this instruction.",
        "Continue the conversation. Summarize in one sentence what you did so \
         far and add a new interesting fact about the number 2.",
        "Continue. Repeat your one-sentence summary and add a fact about the \
         number 3.",
        "Continue. Repeat your one-sentence summary and add a fact about the \
         number 4.",
        "Continue. Repeat your one-sentence summary and add a fact about the \
         number 5.",
    ];

    let mut last_generation = 0;
    for (step, prompt) in prompts.iter().enumerate() {
        host.submit_inbound(vec![UserContentBlock::Text(TextBlock {
            text: (*prompt).to_owned(),
        })])
        .expect("inbound accepted");
        let events = settle(&events).await;
        let outcome = events.iter().find_map(|event| match event {
            RuntimeClientEvent::AttemptSettled { outcome, .. } => Some(outcome),
            _ => None,
        });
        assert!(
            matches!(outcome, Some(RuntimeClientOutcome::Completed { .. })),
            "turn {} must complete coherently, got {outcome:?}",
            step + 1
        );
        let generations: Vec<u64> = events
            .iter()
            .filter_map(|event| match event {
                RuntimeClientEvent::ContextCompacted { context, .. } => context
                    .latest_compaction
                    .as_ref()
                    .map(|view| view.generation),
                _ => None,
            })
            .collect();
        for generation in &generations {
            assert_eq!(
                *generation,
                last_generation + 1,
                "compaction generations advance exactly in order"
            );
            last_generation = *generation;
        }
        let (snapshot, _) = host.snapshot().expect("snapshot");
        eprintln!(
            "M4 live: turn {}: {} compaction(s), {} canonical messages",
            step + 1,
            snapshot.context.compaction_count,
            snapshot.messages.len()
        );
    }

    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert!(
        snapshot.context.compaction_count >= 2,
        "the conversation must compact at least twice on the final path, got {}",
        snapshot.context.compaction_count
    );
    let summaries = snapshot
        .messages
        .iter()
        .filter(|message| {
            matches!(
                message,
                MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
            )
        })
        .count();
    assert!(
        summaries >= 2,
        "every committed compaction is a canonical ledger fact, got {summaries}"
    );

    // Every actual primary request — including requests taken before later
    // compactions — reconstructs from its frozen snapshot against the live
    // conversation history.
    let history = host.request_history();
    assert!(
        history.snapshots().len() >= prompts.len(),
        "one frozen snapshot per primary request, got {}",
        history.snapshots().len()
    );
    for frozen in history.snapshots() {
        host.reconstruct_request(&frozen.identity)
            .expect("every actual primary request reconstructs after live compactions");
    }

    eprintln!(
        "M4 live: PASSED with {} compactions on the final ConversationRuntime path",
        snapshot.context.compaction_count
    );
}
