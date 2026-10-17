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

use super::super::support;

use rustx::message::types::MessageBlock;
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime_client::{
    EventDelivery, RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientEndpoint, RuntimeClientHost,
    RuntimeClientRequest, RuntimeClientResponse,
};

use support::fake::FakeStep;

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

    fn initialize(&self, id: u64) -> serde_json::Value {
        self.exchange(
            &serde_json::json!({
                "method": "initialize",
                "id": id,
                "protocol_version": RUNTIME_CLIENT_PROTOCOL_VERSION,
            })
            .to_string(),
        )
    }

    /// One request frame in, one response frame out.
    fn exchange(&self, line: &str) -> serde_json::Value {
        let request: RuntimeClientRequest =
            serde_json::from_str(line).expect("the frame decodes to a Runtime Client request");
        let response = self.endpoint.handle_request(request);
        let encoded = serde_json::to_string(&response).expect("the response encodes");
        // Round-trip through the wire shape so the test only ever asserts
        // on what a transport can actually observe.
        let decoded: RuntimeClientResponse =
            serde_json::from_str(&encoded).expect("the response decodes");
        assert_eq!(decoded, response, "the response frame round-trips exactly");
        serde_json::from_str(&encoded).expect("the response frame is JSON")
    }

    async fn exchange_async(&self, line: &str) -> serde_json::Value {
        let request: RuntimeClientRequest =
            serde_json::from_str(line).expect("the frame decodes to a Runtime Client request");
        let response = self.endpoint.handle_request_async(request).await;
        let encoded = serde_json::to_string(&response).expect("the response encodes");
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

fn framed_checkpoint(
    value: serde_json::Value,
) -> Box<rustx::runtime_client::checkpoint::RuntimeClientCheckpoint> {
    let response: RuntimeClientResponse =
        serde_json::from_value(value).expect("typed framed response");
    let Some(rustx::runtime_client::RuntimeClientResult::Checkpoint { checkpoint }) =
        response.result
    else {
        panic!("independent bounded checkpoint");
    };
    assert_eq!(checkpoint.read_cut.revision, checkpoint.cursor.get());
    assert_eq!(
        &checkpoint.read_cut.conversation_id,
        checkpoint.snapshot.conversation_id()
    );
    checkpoint
}

fn assert_event_cut(event: &rustx::runtime_client::RuntimeClientProtocolEvent) {
    match &event.event {
        rustx::runtime_client::RuntimeClientEvent::ProjectionTransition { metadata, .. } => {
            assert_eq!(metadata.read_cut.revision, event.cursor.get());
        }
        rustx::runtime_client::RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
            assert_eq!(checkpoint.cursor, event.cursor);
            assert_eq!(checkpoint.read_cut.revision, event.cursor.get());
        }
        _ => panic!("whole native publication required"),
    }
}

fn assert_read_suffix(
    event: &rustx::runtime_client::RuntimeClientProtocolEvent,
    terminal: &rustx::runtime::identity::AttemptId,
) {
    match &event.event {
        rustx::runtime_client::RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
            let attempt = checkpoint.snapshot.attempt.as_ref().expect("retained native Attempt");
            assert_eq!(attempt.attempt_id.as_ref(), Some(terminal));
            assert!(matches!(attempt.phase, rustx::runtime_client::RuntimeClientAttemptPhase::Settled { .. }));
        }
        _ => assert!(event.event.facts().iter().all(|fact| matches!(fact,
            rustx::runtime_client::RuntimeClientEvent::ReadDomainsUpdated { .. } |
            rustx::runtime_client::RuntimeClientEvent::TraceChanged |
            rustx::runtime_client::RuntimeClientEvent::ConfigurationAdoptionEligibilityChanged { .. }
        )), "only read-domain/adoption suffix follows the native terminal"),
    }
}

async fn receive_terminal(
    adapter: &FramingAdapter,
    mut expected: u64,
) -> (u64, rustx::runtime::identity::AttemptId) {
    // Whole native records retain their original cursor and publication cut.
    let mut terminal = None;
    while terminal.is_none() {
        let frame = tokio::time::timeout(std::time::Duration::from_mins(2), adapter.notification())
            .await
            .expect("the notification stream must not stall")
            .expect("subscription stays open");
        assert!(
            frame.get("id").is_none(),
            "notifications never fabricate request ids"
        );
        let event: rustx::runtime_client::RuntimeClientProtocolEvent =
            serde_json::from_value(frame).expect("complete native event frame");
        expected += 1;
        assert_eq!(event.cursor.get(), expected);
        assert_event_cut(&event);
        for fact in event.event.facts() {
            if let rustx::runtime_client::RuntimeClientEvent::AttemptSettled {
                attempt_id, ..
            } = fact
            {
                assert!(
                    terminal.replace(attempt_id.clone()).is_none(),
                    "exactly one terminal settlement"
                );
            }
        }
        if let rustx::runtime_client::RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } =
            &event.event
            && let Some(attempt) = &checkpoint.snapshot.attempt
            && matches!(
                attempt.phase,
                rustx::runtime_client::RuntimeClientAttemptPhase::Settled { .. }
            )
        {
            terminal = Some(
                attempt
                    .attempt_id
                    .clone()
                    .expect("normal native execution identity"),
            );
        }
    }
    (expected, terminal.expect("actual recorded terminal"))
}

async fn drain_read_suffix(
    adapter: &FramingAdapter,
    mut expected: u64,
    through: u64,
    terminal: &rustx::runtime::identity::AttemptId,
) -> u64 {
    while expected < through {
        let event: rustx::runtime_client::RuntimeClientProtocolEvent = serde_json::from_value(
            adapter
                .notification()
                .await
                .expect("derived native publication suffix"),
        )
        .expect("whole native frame");
        expected += 1;
        assert_eq!(event.cursor.get(), expected);
        assert_event_cut(&event);
        assert_read_suffix(&event, terminal);
    }
    assert_eq!(expected, through);
    expected
}

/// A host over one conversation with the given model script.
///
/// Construction is the shared Runtime Client fixture, so this file and the
/// Issue #38 conformance scenarios exercise identically built runtimes.
///
/// The host outlives the fixture handle here, so it is taken through the
/// fixture's own `into_parts` ownership path, which keeps the temporary
/// workspace alive for the rest of the process. Moving the host field out
/// instead would drop the workspace directory under a live runtime.
async fn host(conversation: &str, scripts: Vec<Vec<FakeStep>>) -> RuntimeClientHost {
    support::runtime_client_fixture::RuntimeClientFixture::builder(conversation)
        .scripts(scripts)
        .build()
        .await
        .into_parts()
        .1
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
/// identity allocation, and returns the bounded read-only identity receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn initialize_alone_establishes_the_attachment() {
    let host = host("conv_bec42b36-775e-75ba-95e2-701564095b4a", Vec::new()).await;
    let adapter = FramingAdapter::new(&host);

    // Before initialize the endpoint is unattached, and it says so with the
    // correlated typed error rather than by any transport-side check.
    let response = adapter.exchange(r#"{"method":"checkpoint_get","id":1}"#);
    assert_eq!(response["id"], 1);
    assert_eq!(response["error"]["type"], "not_attached");
    assert!(response.get("result").is_none());

    let response = adapter.initialize(2);
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
        "conv_bec42b36-775e-75ba-95e2-701564095b4a"
    );
    assert_eq!(response["result"]["agent_id"], "agent-a");
    assert!(response["result"].get("snapshot").is_none());
    assert!(response["result"].get("cursor").is_none());
    assert_eq!(response["result"]["read_only"], false);
    assert_eq!(
        response["result"]["protocol_version"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert!(response["result"]["capabilities"].is_array());

    // The attachment is live: an ordinary request now succeeds, and no
    // out-of-band attach ever happened.
    let response = adapter.exchange(r#"{"method":"checkpoint_get","id":3}"#);
    assert!(response.get("error").is_none());
    assert_eq!(response["result"]["type"], "checkpoint");
    let independent = framed_checkpoint(response);
    assert_eq!(
        independent.snapshot.conversation_id().as_str(),
        "conv_bec42b36-775e-75ba-95e2-701564095b4a"
    );
    assert_eq!(independent.read_cut.revision, independent.cursor.get());
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
    let host = host("conv_20c5c7c8-943c-72b3-9455-17a812aef199", Vec::new()).await;
    let adapter = FramingAdapter::new(&host);

    // v33 requires the removed global SessionSummary.active field. Reject it
    // before attachment or any Session list decoding can occur.
    let response = adapter.exchange(r#"{"method":"initialize","id":33,"protocol_version":33}"#);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 33);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // v40 is an earlier contract. Its `GoalView` carries the
    // obsolete `armed` member, so it can spell `Active + disarmed` — a state
    // that no longer exists (Issue #351). It is refused by strict negotiation
    // rather than served a view it would misread.
    let response = adapter.exchange(r#"{"method":"initialize","id":38,"protocol_version":38}"#);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 38);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // Both the original review base and current main lack inherited_through.
    // Reject either before attachment can deliver an incompatible snapshot.
    for version in [61, 66, 67] {
        let response = adapter.exchange(
            &serde_json::json!({
                "method": "initialize", "id": version, "protocol_version": version,
            })
            .to_string(),
        );
        assert!(response.get("result").is_none());
        assert_eq!(response["error"]["type"], "unsupported_protocol_version");
        assert_eq!(
            response["error"]["supported"],
            RUNTIME_CLIENT_PROTOCOL_VERSION
        );
        assert_eq!(response["error"]["requested"], version);
        assert_eq!(adapter.endpoint.attachment_id(), None);
        let application = adapter.exchange(r#"{"method":"checkpoint_get","id":99}"#);
        assert_eq!(application["error"]["type"], "not_attached");
        assert_eq!(adapter.endpoint.attachment_id(), None);
    }

    // A future version the runtime does not speak is rejected explicitly.
    let future = RUNTIME_CLIENT_PROTOCOL_VERSION + 1;
    let response = adapter.exchange(
        &serde_json::json!({
            "method": "initialize", "id": 7, "protocol_version": future,
        })
        .to_string(),
    );
    assert_eq!(response["id"], 7);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], future);
    assert_eq!(
        adapter.endpoint.attachment_id(),
        None,
        "a rejected negotiation admits nothing"
    );

    // v28 is an earlier contract: its `effective_extensions`
    // record has no `todo` member, and its `todos` is a bare snapshot that
    // cannot distinguish "no Todo extension composed" from "composed over an
    // empty list" (Issue #259). rustX is pre-1.0, so it is refused outright
    // rather than served a projection it would misread — there is no v28
    // decoder and no compatibility shim.
    let response = adapter.exchange(r#"{"method":"initialize","id":28,"protocol_version":28}"#);
    assert_eq!(response["id"], 28);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 28);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // v27 additionally has no Session deletion control contract (Issue #255).
    let response = adapter.exchange(r#"{"method":"initialize","id":27,"protocol_version":27}"#);
    assert_eq!(response["id"], 27);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 27);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // v26 additionally predates the effective execution-profile identity on
    // projected subagents (Issue #258), and is refused for the same reason.
    let response = adapter.exchange(r#"{"method":"initialize","id":26,"protocol_version":26}"#);
    assert_eq!(response["id"], 26);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 26);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // v26 predates the `effective_extensions` snapshot section and the
    // `settings_lifetimes.extensions` boundary (Issue #256), and is refused
    // for the same reason.
    let response = adapter.exchange(r#"{"method":"initialize","id":25,"protocol_version":25}"#);
    assert_eq!(response["id"], 25);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 25);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // v15 is the pre-#202 contract: its tool status vocabulary spelled the
    // unsettled case `interrupted` with no bounded `detail`, and its
    // `timed_out` covered deadline expiry whether or not terminal settlement
    // was proven. It is refused outright rather than converted, because
    // there is no v15 -> v16 decoding path.
    let response = adapter.exchange(r#"{"method":"initialize","id":15,"protocol_version":15}"#);
    assert_eq!(response["id"], 15);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 15);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // v14 is the pre-#190 contract: it already carries the Issue #187
    // logical/physical workspace projection and Issue #194 Agent Status
    // window. It is refused outright rather than converted because the v15
    // disposal/resource shape has no v14 decoding path.
    let response = adapter.exchange(r#"{"method":"initialize","id":13,"protocol_version":14}"#);
    assert_eq!(response["id"], 13);
    assert!(response.get("result").is_none());
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 14);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // v13 carries the Issue #187 workspace projection but predates the v14
    // Agent Status window. It is refused rather than served a snapshot whose
    // status shape a v13 client would silently misread.
    let response = adapter.exchange(r#"{"method":"initialize","id":14,"protocol_version":13}"#);
    assert_eq!(response["id"], 14);
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 13);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    let response = adapter.exchange(r#"{"method":"initialize","id":8,"protocol_version":1}"#);
    assert_eq!(response["id"], 8);
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 1);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // The previous wire contracts are also rejected explicitly rather than
    // being treated as an additive-compatible version.
    let response = adapter.exchange(r#"{"method":"initialize","id":10,"protocol_version":7}"#);
    assert_eq!(response["id"], 10);
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 7);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // v10 is the pre-#178 contract whose subagent `detail` carried the
    // successful answer content; it is refused outright rather than served
    // a payload a v10 client would silently misread.
    let response = adapter.exchange(r#"{"method":"initialize","id":12,"protocol_version":10}"#);
    assert_eq!(response["id"], 12);
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 10);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // v6 is the obsolete profile-shaped subagent projection (Issue #144).
    // It is refused outright rather than served a renamed payload a v6
    // client would silently misread.
    let response = adapter.exchange(r#"{"method":"initialize","id":11,"protocol_version":6}"#);
    assert_eq!(response["id"], 11);
    assert_eq!(response["error"]["type"], "unsupported_protocol_version");
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 6);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // Trace changes the mandatory snapshot/event vocabulary; v34 is obsolete.
    let response = adapter.exchange(r#"{"method":"initialize","id":34,"protocol_version":34}"#);
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 34);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // Native deletion mutations were removed in v41, not kept as aliases.
    let response = adapter.exchange(r#"{"method":"initialize","id":39,"protocol_version":39}"#);
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 39);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // Trace replaced its single-level entry with the summary/detail split in
    // v41, so a v41 client would decode a vocabulary that no longer exists.
    let response = adapter.exchange(r#"{"method":"initialize","id":40,"protocol_version":40}"#);
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 40);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // Completed-response projections are mandatory in v45.
    let response = adapter.exchange(r#"{"method":"initialize","id":41,"protocol_version":41}"#);
    assert_eq!(
        response["error"]["supported"],
        RUNTIME_CLIENT_PROTOCOL_VERSION
    );
    assert_eq!(response["error"]["requested"], 41);
    assert_eq!(adapter.endpoint.attachment_id(), None);

    // The runtime is still attachable at the supported version.
    let response = adapter.initialize(9);
    assert_eq!(response["result"]["type"], "initialized");
}

/// A second attachment is rejected deterministically and never evicts the
/// first — whether it arrives on the same connection or a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_initialize_is_rejected_without_eviction() {
    let host = host("conv_ffcc0cfe-1c74-7297-8bc1-77dcbb60c993", Vec::new()).await;
    let first = FramingAdapter::new(&host);
    let second = FramingAdapter::new(&host);

    let response = first.initialize(1);
    let first_id = response["result"]["attachment_id"]
        .as_str()
        .expect("attachment identity")
        .to_owned();

    // A second connection: rejected with the active identity, not admitted.
    let response = second.initialize(1);
    assert_eq!(response["error"]["type"], "attachment_in_use");
    assert_eq!(response["error"]["existing_attachment_id"], first_id);
    assert_eq!(second.endpoint.attachment_id(), None);

    // Re-initializing the same connection is invalid, and equally
    // non-destructive.
    let response = first.initialize(2);
    assert_eq!(response["error"]["type"], "invalid_request");

    // The first attachment was never evicted: it still serves requests
    // under its original identity.
    let response = first.exchange(r#"{"method":"checkpoint_get","id":3}"#);
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
    let response = second.initialize(2);
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
    let host = host(
        "conv_06eda99c-75ce-7f8a-b46c-ceffdefa93bb",
        vec![one_turn_stop()],
    )
    .await;
    let adapter = FramingAdapter::new(&host);

    let initialized = adapter.initialize(1);
    assert_eq!(initialized["result"]["type"], "initialized");
    assert!(initialized["result"].get("snapshot").is_none());
    let initial = framed_checkpoint(adapter.exchange(r#"{"method":"checkpoint_get","id":10}"#));
    let cursor = initial.cursor;
    let response = adapter.exchange(
        &serde_json::json!({"method":"subscribe_events","id":2,"after_cursor":cursor}).to_string(),
    );
    assert_eq!(response["result"]["type"], "subscribed");
    assert_eq!(
        response["result"]["after_cursor"],
        serde_json::to_value(cursor).unwrap()
    );

    let response = adapter.exchange(
        r#"{"method":"submit_inbound","id":3,"content":[{"type":"text","text":"hello"}]}"#,
    );
    assert_eq!(response["result"]["type"], "inbound_accepted");
    assert!(response["result"]["message_id"].is_string());
    assert_eq!(
        response["result"]["inbound_sequence"],
        serde_json::to_value(rustx::runtime::inbound::InboundSequence::new(1)).unwrap()
    );
    assert!(
        response["result"]["inbound_sequence"].is_u64(),
        "the internal Runtime Client sequence preserves its native numeric wire domain"
    );

    let (mut expected, terminal) = receive_terminal(&adapter, cursor.get()).await;
    let response = adapter.exchange(r#"{"method":"capability_get","id":4}"#);
    assert_eq!(response["result"]["type"], "domain_page");
    assert_eq!(response["result"]["page"]["domain"], "capabilities");
    let history = adapter.exchange(r#"{"method":"transcript_page_get","id":11,"limit":32}"#);
    let history: RuntimeClientResponse =
        serde_json::from_value(history).expect("bounded transcript response");
    let Some(rustx::runtime_client::RuntimeClientResult::TranscriptPage { page }) = history.result
    else {
        panic!("explicit bounded canonical transcript");
    };
    assert!(page.source_cut.is_some());
    assert!(
        page.entries
            .iter()
            .any(|entry| entry.completed_response.is_some())
            || page.deferred_entries.iter().any(|item| item
                .value
                .as_ref()
                .and_then(|value| value.get("completed_response"))
                .is_some_and(|value| !value.is_null())),
        "the canonical reader carries positive native completed response evidence"
    );

    let checkpoint = framed_checkpoint(adapter.exchange(r#"{"method":"checkpoint_get","id":5}"#));
    expected = drain_read_suffix(&adapter, expected, checkpoint.cursor.get(), &terminal).await;
    assert_eq!(
        adapter.exchange(r#"{"method":"detach","id":6}"#)["result"]["type"],
        "detached"
    );
    assert_eq!(
        adapter.exchange(r#"{"method":"checkpoint_get","id":7}"#)["error"]["type"],
        "not_attached"
    );
    assert_eq!(adapter.initialize(8)["result"]["type"], "initialized");
    let reattached = framed_checkpoint(adapter.exchange(r#"{"method":"checkpoint_get","id":12}"#));
    let receipt = adapter.exchange(&serde_json::json!({"method":"subscribe_events","id":9,"after_cursor":rustx::runtime_client::RuntimeClientCursor::new(expected)}).to_string());
    assert_eq!(receipt["result"]["type"], "subscribed");
    expected = drain_read_suffix(&adapter, expected, reattached.cursor.get(), &terminal).await;
    assert_eq!(
        expected,
        reattached.cursor.get(),
        "complete drain reaches the actual reattached frontier"
    );
    assert_eq!(expected, reattached.read_cut.revision);
    assert_eq!(
        reattached.snapshot.messages.len(),
        3,
        "one inbound message, one admitted Agent Status fact, and one agent reply"
    );
    assert_eq!(
        host.checkpoint()
            .expect("native checkpoint")
            .snapshot
            .messages
            .len(),
        3,
        "the framed view matches the independent native projection"
    );
}

/// Dropping the endpoint releases the attachment (RAII), so a transport
/// that loses its connection needs no explicit teardown semantics either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_the_endpoint_releases_the_attachment() {
    let host = host("conv_1c8d2cd1-ed58-70cb-a610-ed7d8d4744b8", Vec::new()).await;
    let adapter = FramingAdapter::new(&host);
    adapter.initialize(1);
    drop(adapter);

    let reconnected = FramingAdapter::new(&host);
    let response = reconnected.initialize(1);
    assert_eq!(
        response["result"]["type"], "initialized",
        "the dropped connection released the attachment"
    );
}

/// Shutdown remains distinct from detach across the framing boundary: it
/// completes only after runtime quiescence, stops further inbound admission,
/// and never mutates canonical history.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_is_not_detach_and_reaches_quiescence() {
    let host = host("conv_8347b205-5919-7d09-84a7-c99c982d1162", Vec::new()).await;
    let adapter = FramingAdapter::new(&host);
    adapter.initialize(1);

    let before: Vec<MessageBlock> = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot")
        .0
        .messages;
    let response = adapter
        .exchange_async(r#"{"method":"shutdown","id":2}"#)
        .await;
    assert_eq!(response["result"]["type"], "shutdown_completed");

    let response = adapter.exchange(
        r#"{"method":"submit_inbound","id":3,"content":[{"type":"text","text":"late"}]}"#,
    );
    assert_eq!(response["error"]["type"], "runtime_shutdown");

    // Still attached (shutdown is not detach), and canonical history is
    // untouched by this idle shutdown.
    let response = adapter.exchange(r#"{"method":"checkpoint_get","id":4}"#);
    assert!(response.get("error").is_none());
    assert_eq!(
        host.checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot")
            .0
            .messages,
        before
    );
}
