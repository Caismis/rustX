//! A smaller primary can require reducing an existing Summary on its own.
use super::super::support;
use std::sync::Arc;
use std::time::Duration;

use rustx::durable::ProjectionStore;
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::catalog::ModelRef;
use rustx::model::session::{SessionModelConfig, SessionModelState, SummaryModelPolicy};
use rustx::model::{ModelAdapter, ModelEvent, ModelFinishReason, ModelProtocol, RequestParams};
use rustx::runtime::identity::{ConversationId, MessageId};
use rustx::runtime_client::types::{RequestId, RuntimeClientRequest};
use rustx::runtime_client::{EventDelivery, RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientEvent};
use support::fake::{FakeModel, FakeStep};
use support::model::{FixtureModel, ScriptedAdapterFactory, fixture_registry};
use support::runtime_client_fixture::RuntimeClientFixture;

fn completion(text: String) -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text,
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

fn summary(goal: &str, critical_context: &[String]) -> Vec<FakeStep> {
    completion(
        serde_json::json!({
            "goal": goal,
            "constraints_and_preferences": [],
            "progress": {"done": [], "in_progress": [], "blocked": []},
            "key_decisions": [], "next_steps": [], "critical_context": critical_context
        })
        .to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One runtime proves model switching, reduction and fresh-input retention.
async fn smaller_primary_reduces_summary_only_history_and_preserves_fresh_input() {
    let adapter = Arc::new(FakeModel::new(vec![
        summary(
            &format!("OLD-SUMMARY {}", "s".repeat(3_800)),
            &["c".repeat(3_800)],
        ),
        summary("REDUCED-SUMMARY", &[]),
        completion("finished".into()),
    ]));
    let models = [
        FixtureModel::text("fixture/large", ModelProtocol::OpenAiChatCompletions)
            .with_context_window(100_000)
            .with_max_output_tokens(128),
        FixtureModel::text("fixture/small", ModelProtocol::OpenAiChatCompletions)
            .with_context_window(2_048)
            .with_max_output_tokens(128),
        FixtureModel::text("fixture/summary", ModelProtocol::OpenAiChatCompletions)
            .with_context_window(100_000)
            .with_max_output_tokens(4_096),
    ];
    let registry = fixture_registry(
        &models,
        &ScriptedAdapterFactory::new(adapter.clone() as Arc<dyn ModelAdapter>),
    );
    let config = |model: &str| SessionModelConfig {
        summary_model: SummaryModelPolicy::Explicit {
            model: ModelRef::parse("fixture/summary").unwrap(),
            profile: None,
            request_params: RequestParams::new(),
        },
        ..SessionModelConfig::of(ModelRef::parse(model).unwrap())
    };
    let session = SessionModelState::new(registry, config("fixture/large")).unwrap();
    let history = vec![MessageBlock::User(UserMessageBlock {
        id: MessageId::new("old-history"),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!("OLD-HISTORY {}", "h".repeat(20_000)),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })];
    let conversation = "conv_01900000-0000-7000-8000-000000000481";
    let fixture = RuntimeClientFixture::builder(conversation)
        .session_model(session)
        .durable_history(history)
        .build()
        .await;
    let host = &fixture.host;
    let (attachment, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    fixture
        .runtime
        .compact_context(None, None)
        .await
        .expect("first checkpoint");
    host.projection_settled().await;
    let store = rustx::durable::SqliteConversationStore::open(
        ConversationId::new(conversation),
        &fixture
            .workspace_root()
            .parent()
            .unwrap()
            .join("artifacts/conversation.sqlite"),
    )
    .unwrap();
    let before = store.projected_history().unwrap();
    assert_eq!(
        before.messages.len(),
        1,
        "checkpoint contains only the Summary"
    );
    assert!(matches!(
        &before.messages[0],
        rustx::model::ModelInputMessage::Derived(_)
    ));
    let start = rustx::context::checkpoint::ProjectionActivationId(0);
    assert_eq!(store.projection_timeline(start, 64).unwrap().len(), 1);
    host.model_set(config("fixture/small")).unwrap();
    let subscription = attachment
        .subscribe_events(host.checkpoint().unwrap().cursor)
        .unwrap();
    let response = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: RequestId::new(1),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "FRESH-INPUT".into(),
        })],
    });
    assert!(response.error.is_none(), "{response:?}");
    let outcome = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let EventDelivery::Event(event) = subscription.next().await else {
                panic!("subscription closed")
            };
            if let Some(outcome) = event.event.facts().iter().find_map(|fact| match fact {
                RuntimeClientEvent::AttemptSettled { outcome, .. } => Some(outcome.clone()),
                _ => None,
            }) {
                break outcome;
            }
        }
    })
    .await
    .expect("attempt settles");
    assert!(
        matches!(
            outcome,
            rustx::runtime_client::event::RuntimeClientOutcome::Completed { .. }
        ),
        "{outcome:?}"
    );
    let requests = adapter.requests();
    assert_eq!(
        requests.len(),
        3,
        "two bounded Summary calls and one primary"
    );
    assert_eq!(requests[2].model(), "small");
    let wire = serde_json::to_string(&requests[2].messages).unwrap();
    assert!(wire.contains("REDUCED-SUMMARY"));
    assert!(wire.contains("FRESH-INPUT"));
    assert!(!wire.contains("OLD-SUMMARY"));
    assert_eq!(store.projection_timeline(start, 64).unwrap().len(), 2);
    let after = store.projected_history().unwrap();
    assert!(
        after.messages.iter().any(|message| matches!(
            message.as_canonical(),
            Some(MessageBlock::User(user)) if user.content.iter().any(|block| matches!(
                block, UserContentBlock::Text(text) if text.text == "FRESH-INPUT"
            ))
        )),
        "fresh input remains an active canonical message"
    );
}
