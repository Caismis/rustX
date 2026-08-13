//! Issue #37: the Runtime Client semantic endpoint owns protocol
//! negotiation and attachment admission.
//!
//! Every test in this file drives the runtime through
//! [`FramingAdapter`] — a stand-in for the Issue #38 stdio/JSONL
//! transport that is *structurally* incapable of protocol semantics: it
//! deserializes a `RuntimeClientRequest`, hands it to the endpoint,
//! serializes the `RuntimeClientResponse`, and serializes notifications.
//! It never calls `RuntimeClientHost::attach`, never constructs an
//! `AttachmentId`, never compares protocol versions, and never inspects
//! attachment state.
//!
//! If any of those semantics leaked out of the endpoint, the adapter below
//! could not be written at all.

#[path = "common/mod.rs"]
mod common;

use std::sync::Arc;

use rustx::context::{
    AgentStatusComposer, ContextConfig, ContextEngine, DefaultTokenEstimator,
    InMemoryCheckpointStore, TokenEstimator,
};
use rustx::message::types::MessageBlock;
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::{ModelProtocol, ReasoningEffort};
use rustx::runtime::identity::AgentId;
use rustx::runtime_client::{
    EventDelivery, RuntimeClientContextConfig, RuntimeClientEndpoint, RuntimeClientHost,
    RuntimeClientHostConfig, RuntimeClientRequest, RuntimeClientResponse,
};
use rustx::tools::executor::ToolRegistry;

use common::fake::{FakeModel, FakeStep};

/// The complete set of operations a future transport performs.
///
/// This is deliberately the whole type: framing in, framing out. There is
/// no method here that negotiates, admits, allocates identity, or decides
/// replacement — those live behind [`RuntimeClientEndpoint::handle_request`].
struct FramingAdapter {
    endpoint: RuntimeClientEndpoint,
}

impl FramingAdapter {
    fn new(host: &RuntimeClientHost) -> Self {
        Self {
            endpoint: host.endpoint(),
        }
    }

    /// One request frame in, one response frame out.
    fn exchange(&self, line: &str) -> serde_json::Value {
        let request: RuntimeClientRequest =
            serde_json::from_str(line).expect("the frame decodes to a v1 request");
        let response = self.endpoint.handle_request(request);
        let encoded = serde_json::to_string(&response).expect("the response encodes");
        // Round-trip through the wire shape so the test only ever asserts
        // on what a transport can actually observe.
        let decoded: RuntimeClientResponse =
            serde_json::from_str(&encoded).expect("the response decodes");
        assert_eq!(decoded, response, "the response frame round-trips exactly");
        serde_json::from_str(&encoded).expect("the response frame is JSON")
    }

    /// One notification frame out, or `None` when the stream is not
    /// deliverable.
    async fn notification(&self) -> Option<serde_json::Value> {
        match self.endpoint.next_event().await {
            EventDelivery::Event(event) => {
                let encoded = serde_json::to_string(&event).expect("the event encodes");
                Some(serde_json::from_str(&encoded).expect("the event frame is JSON"))
            }
            _ => None,
        }
    }
}

/// A host over one conversation with the given model script.
async fn host(conversation: &str, model: FakeModel) -> (Arc<FakeModel>, RuntimeClientHost) {
    let model = Arc::new(model);
    let adapter: Arc<dyn rustx::model::ModelAdapter> = model.clone();
    let tool_runtime = common::tool_runtime(conversation);
    let coordinator = {
        let dir = tempfile::tempdir().expect("temp dir");
        let coordinator = rustx::capabilities::CapabilityCoordinator::new(
            rustx::capabilities::CapabilityCoordinatorConfig {
                conversation_id: tool_runtime.conversation_id().clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(ToolRegistry::new()),
                mcp_servers: Vec::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        std::mem::forget(dir);
        coordinator
    };
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .expect("engine");
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        agent_id: AgentId::new("agent-a"),
        model: "scripted".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 512,
        timezone: None,
        adapter,
        context: RuntimeClientContextConfig {
            engine,
            summarizer: Arc::new(common::context::FakeContextSummarizer::new(Vec::new())),
            checkpoint_store: Arc::new(InMemoryCheckpointStore::new()),
            status_composer: AgentStatusComposer::default(),
        },
        tool_runtime,
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        replay_limit: None,
    })
    .expect("host");
    (model, host)
}

fn one_turn_stop() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

/// An `initialize` frame is by itself sufficient to establish the active
/// attachment: the semantic endpoint performs negotiation, admission, and
/// identity allocation, and returns the linearized initial snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn initialize_alone_establishes_the_attachment() {
    let (_, host) = host("conv-37-endpoint-init", FakeModel::new(Vec::new())).await;
    let adapter = FramingAdapter::new(&host);

    // Before initialize the endpoint is unattached, and it says so with the
    // correlated typed error rather than by any transport-side check.
    let response = adapter.exchange(r#"{"method":"snapshot_get","id":1}"#);
    assert_eq!(response["id"], 1);
    assert_eq!(response["error"]["type"], "not_attached");
    assert!(response.get("result").is_none());

    let response = adapter.exchange(r#"{"method":"initialize","id":2,"protocol_version":1}"#);
    assert_eq!(response["id"], 2, "the response correlates the request id");
    assert!(response.get("error").is_none());
    assert_eq!(response["result"]["type"], "initialized");
    // The runtime allocated the attachment identity; the transport neither
    // supplied nor derived it.
    let attachment_id = response["result"]["attachment_id"]
        .as_str()
        .expect("the runtime returns the attachment identity")
        .to_owned();
    assert!(!attachment_id.is_empty());
    assert_eq!(
        response["result"]["conversation_id"],
        "conv-37-endpoint-init"
    );
    assert_eq!(response["result"]["agent_id"], "agent-a");
    assert!(
        response["result"]["snapshot"].is_object(),
        "initialize returns the snapshot linearized with its cursor"
    );
    assert!(response["result"]["cursor"].is_u64());

    // The attachment is live: an ordinary request now succeeds, and no
    // out-of-band attach ever happened.
    let response = adapter.exchange(r#"{"method":"snapshot_get","id":3}"#);
    assert!(response.get("error").is_none());
    assert_eq!(response["result"]["type"], "snapshot");
    assert_eq!(
        host.endpoint().attachment_id(),
        None,
        "a distinct endpoint is independently unattached"
    );
}

/// An unsupported protocol version fails with the correlated typed error
/// and admits nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_protocol_version_is_a_correlated_typed_error() {
    let (_, host) = host("conv-37-endpoint-version", FakeModel::new(Vec::new())).await;
    let adapter = FramingAdapter::new(&host);

    let response = adapter.exchange(r#"{"method":"initialize","id":7,"protocol_version":9}"#);
    assert_eq!(response["id"], 7);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(response["error"]["supported"], 1);
    assert_eq!(response["error"]["requested"], 9);
    assert_eq!(
        adapter.endpoint.attachment_id(),
        None,
        "a rejected negotiation admits nothing"
    );

    // The runtime is still attachable at the supported version.
    let response = adapter.exchange(r#"{"method":"initialize","id":8,"protocol_version":1}"#);
    assert_eq!(response["result"]["type"], "initialized");
}

/// A second attachment is rejected deterministically and never evicts the
/// first — whether it arrives on the same connection or a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_initialize_is_rejected_without_eviction() {
    let (_, host) = host("conv-37-endpoint-second", FakeModel::new(Vec::new())).await;
    let first = FramingAdapter::new(&host);
    let second = FramingAdapter::new(&host);

    let response = first.exchange(r#"{"method":"initialize","id":1,"protocol_version":1}"#);
    let first_id = response["result"]["attachment_id"]
        .as_str()
        .expect("attachment identity")
        .to_owned();

    // A second connection: rejected with the active identity, not admitted.
    let response = second.exchange(r#"{"method":"initialize","id":1,"protocol_version":1}"#);
    assert_eq!(response["error"]["type"], "attachment_in_use");
    assert_eq!(response["error"]["existing_attachment_id"], first_id);
    assert_eq!(second.endpoint.attachment_id(), None);

    // Re-initializing the same connection is invalid, and equally
    // non-destructive.
    let response = first.exchange(r#"{"method":"initialize","id":2,"protocol_version":1}"#);
    assert_eq!(response["error"]["type"], "invalid_request");

    // The first attachment was never evicted: it still serves requests
    // under its original identity.
    let response = first.exchange(r#"{"method":"snapshot_get","id":3}"#);
    assert!(response.get("error").is_none());
    assert_eq!(
        first
            .endpoint
            .attachment_id()
            .expect("still attached")
            .to_string(),
        first_id
    );

    // Only an explicit detach releases it; the second connection can then
    // initialize into a *fresh* identity.
    let response = first.exchange(r#"{"method":"detach","id":4}"#);
    assert_eq!(response["result"]["type"], "detached");
    assert_eq!(first.endpoint.attachment_id(), None);
    let response = second.exchange(r#"{"method":"initialize","id":2,"protocol_version":1}"#);
    let second_id = response["result"]["attachment_id"]
        .as_str()
        .expect("attachment identity");
    assert_ne!(second_id, first_id, "reconnecting receives a new identity");
}

/// A complete client session driven exclusively by frames: nothing in this
/// test reaches for a semantic host operation, which is the property Issue
/// #38 depends on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_session_needs_no_out_of_band_semantic_operation() {
    let (_, host) = host(
        "conv-37-endpoint-session",
        FakeModel::new(vec![one_turn_stop()]),
    )
    .await;
    let adapter = FramingAdapter::new(&host);

    let response = adapter.exchange(r#"{"method":"initialize","id":1,"protocol_version":1}"#);
    let cursor = response["result"]["cursor"]
        .as_u64()
        .expect("initialize returns the cursor to resume after");

    let response = adapter.exchange(&format!(
        r#"{{"method":"subscribe_events","id":2,"after_cursor":{cursor}}}"#
    ));
    assert_eq!(response["result"]["type"], "subscribed");

    let response = adapter.exchange(
        r#"{"method":"submit_inbound","id":3,"content":[{"type":"text","text":"hello"}]}"#,
    );
    assert_eq!(response["result"]["type"], "inbound_accepted");
    assert!(response["result"]["message_id"].is_string());
    assert!(response["result"]["inbound_sequence"].is_u64());

    // Notifications are frames too: cursor plus typed payload, no request
    // id, strictly contiguous.
    let mut expected = cursor;
    let mut settled = false;
    while !settled {
        // Liveness guard only: the notification wait itself is exact.
        let frame =
            tokio::time::timeout(std::time::Duration::from_secs(120), adapter.notification())
                .await
                .expect("the notification stream must not stall")
                .expect("the subscription stays open");
        expected += 1;
        assert_eq!(frame["cursor"].as_u64(), Some(expected));
        assert!(
            frame.get("id").is_none(),
            "notifications never fabricate request ids"
        );
        settled = frame["event"]["type"] == "attempt_settled";
    }

    let response = adapter.exchange(r#"{"method":"capability_get","id":4}"#);
    assert_eq!(response["result"]["type"], "capability");

    let response = adapter.exchange(r#"{"method":"snapshot_get","id":5}"#);
    assert_eq!(response["result"]["cursor"].as_u64(), Some(expected));

    // Detach is never cancellation: the settled attempt and the canonical
    // history survive it, and re-initializing observes exactly that state.
    let response = adapter.exchange(r#"{"method":"detach","id":6}"#);
    assert_eq!(response["result"]["type"], "detached");
    let response = adapter.exchange(r#"{"method":"snapshot_get","id":7}"#);
    assert_eq!(response["error"]["type"], "not_attached");

    let response = adapter.exchange(r#"{"method":"initialize","id":8,"protocol_version":1}"#);
    assert_eq!(response["result"]["cursor"].as_u64(), Some(expected));
    let messages = response["result"]["snapshot"]["messages"]
        .as_array()
        .expect("the snapshot carries canonical history");
    assert_eq!(messages.len(), 2, "one inbound message and one agent reply");
    assert_eq!(
        host.snapshot().expect("snapshot").0.messages.len(),
        2,
        "the framed view matches the authoritative projection"
    );
}

/// Dropping the endpoint releases the attachment (RAII), so a transport
/// that loses its connection needs no explicit teardown semantics either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_the_endpoint_releases_the_attachment() {
    let (_, host) = host("conv-37-endpoint-drop", FakeModel::new(Vec::new())).await;
    let adapter = FramingAdapter::new(&host);
    adapter.exchange(r#"{"method":"initialize","id":1,"protocol_version":1}"#);
    drop(adapter);

    let reconnected = FramingAdapter::new(&host);
    let response = reconnected.exchange(r#"{"method":"initialize","id":1,"protocol_version":1}"#);
    assert_eq!(
        response["result"]["type"], "initialized",
        "the dropped connection released the attachment"
    );
}

/// Shutdown remains distinct from detach and from cancellation across the
/// framing boundary: it is accepted, it stops further inbound admission,
/// and it never mutates canonical history.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_is_neither_detach_nor_cancellation() {
    let (_, host) = host("conv-37-endpoint-shutdown", FakeModel::new(Vec::new())).await;
    let adapter = FramingAdapter::new(&host);
    adapter.exchange(r#"{"method":"initialize","id":1,"protocol_version":1}"#);

    let before: Vec<MessageBlock> = host.snapshot().expect("snapshot").0.messages;
    let response = adapter.exchange(r#"{"method":"shutdown","id":2}"#);
    assert_eq!(response["result"]["type"], "shutdown_accepted");

    let response = adapter.exchange(
        r#"{"method":"submit_inbound","id":3,"content":[{"type":"text","text":"late"}]}"#,
    );
    assert_eq!(response["error"]["type"], "runtime_shutdown");

    // Still attached (shutdown is not detach), and canonical history is
    // untouched (shutdown is not cancellation).
    let response = adapter.exchange(r#"{"method":"snapshot_get","id":4}"#);
    assert!(response.get("error").is_none());
    assert_eq!(host.snapshot().expect("snapshot").0.messages, before);
}
