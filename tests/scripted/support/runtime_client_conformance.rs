//! Transport-independent Runtime Client Protocol v5 conformance fixtures
//! (Issue #38).
//!
//! # Why this layer exists
//!
//! Issue #37 defined what the protocol *means*; Issue #38 defines how those
//! exact messages become a bounded local byte stream. A transport is
//! therefore correct exactly when it changes nothing semantic — and that is
//! only provable by running the same semantic scenarios through more than
//! one framing.
//!
//! ```text
//!            reusable semantic scenarios (this module)
//!                          |
//!            +-------------+-------------+
//!            |                           |
//!   DirectEndpointDriver          StdioJsonlDriver
//!   typed request                 typed request
//!     -> RuntimeClientEndpoint      -> JSONL bytes -> stdio session
//!     -> typed response/event       -> JSONL bytes -> typed response/event
//! ```
//!
//! A future Issue #36 WebSocket transport adds one `DriverFactory` and
//! reuses every scenario function below unchanged: no scenario names a
//! framing, a byte, an I/O type, or a transport error.
//!
//! # What belongs here and what does not
//!
//! - Scenarios assert externally visible *semantics*: correlated responses,
//!   runtime-originated events, authoritative snapshots and cursors.
//! - Byte-level framing, record limits, stdout purity, and backpressure are
//!   transport-specific and live in the stdio framing tests instead.
//! - The in-crate host tests remain the synchronization/linearization
//!   authority; nothing here re-proves internal ordering.
//!
//! # Determinism
//!
//! No scenario proves anything with a sleep. Model parking (`FakeStep`
//! release/park channels), tool release notifies, and `watch` barriers
//! establish exact interleavings; the only time bounds are outer liveness
//! guards around whole waits.

#![allow(dead_code)] // every scenario is used only by some test binaries

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::future::BoxFuture;
use rustx::message::content::TextBlock;
use rustx::message::types::{ContentBlockIndex, MessageBlock, UserContentBlock};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::identity::ToolExecutionId;
use rustx::runtime_client::transport::stdio::{
    StdioSessionEnd, StdioTransportError, serve_stdio_jsonl_with_io,
};
use rustx::runtime_client::{
    EventDelivery, RequestId, RuntimeClientCursor, RuntimeClientEndpoint, RuntimeClientError,
    RuntimeClientEvent, RuntimeClientHost, RuntimeClientOutcome, RuntimeClientProtocolEvent,
    RuntimeClientRequest, RuntimeClientResponse, RuntimeClientResult, RuntimeClientSnapshot,
};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolOrigin};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use super::fake::{FakeModel, FakeStep, FakeTool, ScriptedCall, await_started, success_result};
use crate::scripted_suites::common;

/// The outer liveness guard of one whole wait.
///
/// Every wait below is exact (a subscription wakes on publication, a parked
/// model releases on an explicit signal). This bounds only total wall time
/// so a scheduling stall on a loaded runner cannot hang the suite; no
/// assertion depends on its value.
const LIVENESS_GUARD: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// The driver abstraction
// ---------------------------------------------------------------------------

/// One transport-independent Runtime Client protocol driver.
///
/// This is the *entire* surface a scenario may use: send one typed request
/// and receive its correlated typed response, or receive the next typed
/// notification. A driver may not expose framing, transport errors, or any
/// semantic shortcut around the protocol.
pub trait RuntimeClientProtocolDriver: Send {
    /// Sends one request and returns its correlated response.
    fn request(&mut self, request: RuntimeClientRequest) -> BoxFuture<'_, RuntimeClientResponse>;

    /// Returns the next notification of the active subscription.
    fn next_event(&mut self) -> BoxFuture<'_, RuntimeClientProtocolEvent>;
}

/// Creates protocol drivers of one transport over a runtime.
///
/// Issue #36 adds a `WebSocketDriverFactory` here and inherits every
/// scenario; nothing else changes.
pub trait DriverFactory: Send + Sync {
    /// The transport name, used in assertion messages.
    fn name(&self) -> &'static str;

    /// Connects one new client session to the runtime.
    fn connect(&self, host: &RuntimeClientHost) -> Box<dyn RuntimeClientProtocolDriver>;
}

/// The in-process driver: typed request straight into the semantic
/// endpoint, typed response straight back.
///
/// This is the semantic reference. A transport driver must be
/// indistinguishable from it for every scenario in this module.
pub struct DirectEndpointDriver {
    /// The semantic endpoint of this client session.
    endpoint: RuntimeClientEndpoint,
}

impl DirectEndpointDriver {
    /// Connects one direct session to the runtime.
    #[must_use]
    pub fn new(host: &RuntimeClientHost) -> Self {
        Self {
            endpoint: host.endpoint(),
        }
    }
}

impl RuntimeClientProtocolDriver for DirectEndpointDriver {
    fn request(&mut self, request: RuntimeClientRequest) -> BoxFuture<'_, RuntimeClientResponse> {
        Box::pin(async move { self.endpoint.handle_request_async(request).await })
    }

    fn next_event(&mut self) -> BoxFuture<'_, RuntimeClientProtocolEvent> {
        Box::pin(async move {
            match self.endpoint.next_event().await {
                EventDelivery::Event(event) => event,
                other => panic!("the subscription must stay open and contiguous, got {other:?}"),
            }
        })
    }
}

/// The factory of [`DirectEndpointDriver`].
pub struct DirectDriverFactory;

impl DriverFactory for DirectDriverFactory {
    fn name(&self) -> &'static str {
        "direct"
    }

    fn connect(&self, host: &RuntimeClientHost) -> Box<dyn RuntimeClientProtocolDriver> {
        Box::new(DirectEndpointDriver::new(host))
    }
}

/// The stdio/JSONL driver: typed request -> JSONL bytes -> a real stdio
/// session -> JSONL bytes -> typed response/event.
///
/// The session under test is exactly `serve_stdio_jsonl_with_io`; only the
/// byte streams are in-memory duplex pipes instead of process stdio.
///
/// The client demultiplexes structurally — a record with a `cursor` is a
/// notification, anything else is a response — because a notification may
/// arrive at any moment, including between a request and its response.
/// Notifications observed while waiting for a response are buffered *in the
/// client* for later `next_event` calls; the transport itself queues
/// nothing.
pub struct StdioJsonlDriver {
    /// The client's request stream (closing it is EOF for the session).
    to_session: Option<tokio::io::DuplexStream>,
    /// The client's line-buffered response/notification stream.
    from_session: BufReader<tokio::io::DuplexStream>,
    /// Notifications observed while awaiting a correlated response.
    buffered_events: VecDeque<RuntimeClientProtocolEvent>,
    /// The running session task.
    session: Option<tokio::task::JoinHandle<Result<StdioSessionEnd, StdioTransportError>>>,
}

/// The duplex capacity of the test client's byte pipes.
///
/// Only large enough that a deterministic scenario never blocks the client
/// mid-request; it is test-client plumbing and bounds nothing in the
/// transport under test.
const DRIVER_PIPE_BYTES: usize = 1024 * 1024;

impl StdioJsonlDriver {
    /// Connects one stdio JSONL session to the runtime.
    #[must_use]
    pub fn new(host: &RuntimeClientHost) -> Self {
        Self::over_endpoint(host.endpoint())
    }

    /// Connects one stdio JSONL session over an already-built endpoint.
    #[must_use]
    pub fn over_endpoint(endpoint: RuntimeClientEndpoint) -> Self {
        // Two independent pipes so each direction closes independently: a
        // dropped client request stream is EOF for the session while the
        // response stream stays readable.
        let (to_session, session_input) = tokio::io::duplex(DRIVER_PIPE_BYTES);
        let (session_output, from_session) = tokio::io::duplex(DRIVER_PIPE_BYTES);
        let session = tokio::spawn(async move {
            serve_stdio_jsonl_with_io(endpoint, session_input, session_output).await
        });
        Self {
            to_session: Some(to_session),
            from_session: BufReader::new(from_session),
            buffered_events: VecDeque::new(),
            session: Some(session),
        }
    }

    /// Closes the client's request stream and returns the session outcome.
    pub async fn finish(mut self) -> Result<StdioSessionEnd, StdioTransportError> {
        drop(self.to_session.take());
        let session = self.session.take().expect("the session runs once");
        tokio::time::timeout(LIVENESS_GUARD, session)
            .await
            .expect("the session must terminate after input closure")
            .expect("the session task must not panic")
    }

    /// Writes one request record: JSON payload plus exactly one LF.
    async fn send(&mut self, request: &RuntimeClientRequest) {
        let mut record = serde_json::to_vec(request).expect("a v5 request serializes");
        record.push(b'\n');
        let stream = self.to_session.as_mut().expect("the session is open");
        stream.write_all(&record).await.expect("write the record");
        stream.flush().await.expect("flush the record");
    }

    /// Reads and classifies exactly one outbound record.
    async fn receive(&mut self) -> OutboundRecord {
        let mut line = String::new();
        let read = self
            .from_session
            .read_line(&mut line)
            .await
            .expect("read one outbound record");
        assert!(
            read > 0,
            "the session closed the output stream unexpectedly"
        );
        assert!(
            line.ends_with('\n'),
            "every outbound record ends with exactly one LF"
        );
        let payload = &line[..line.len() - 1];
        assert!(
            !payload.is_empty(),
            "the transport never writes a blank record"
        );
        let value: serde_json::Value =
            serde_json::from_str(payload).expect("every outbound record is JSON");
        // Structural classification: notifications carry a cursor and no
        // request id, responses carry a request id and no cursor.
        if value.get("cursor").is_some() {
            OutboundRecord::Event(
                serde_json::from_str(payload).expect("a notification decodes exactly"),
            )
        } else {
            OutboundRecord::Response(
                serde_json::from_str(payload).expect("a response decodes exactly"),
            )
        }
    }
}

/// One classified outbound record of the stdio driver.
// Mirrors the protocol enums' own unboxed shape; one record exists per
// classification and is consumed immediately.
#[allow(clippy::large_enum_variant)]
enum OutboundRecord {
    /// A correlated response.
    Response(RuntimeClientResponse),
    /// A notification of the active subscription.
    Event(RuntimeClientProtocolEvent),
}

impl RuntimeClientProtocolDriver for StdioJsonlDriver {
    fn request(&mut self, request: RuntimeClientRequest) -> BoxFuture<'_, RuntimeClientResponse> {
        Box::pin(async move {
            let id = request.id();
            self.send(&request).await;
            loop {
                match self.receive().await {
                    OutboundRecord::Response(response) => {
                        assert_eq!(
                            response.id, id,
                            "a response record correlates its request id exactly"
                        );
                        return response;
                    }
                    // A notification arriving before the response is normal:
                    // buffer it in the client, never drop it.
                    OutboundRecord::Event(event) => self.buffered_events.push_back(event),
                }
            }
        })
    }

    fn next_event(&mut self) -> BoxFuture<'_, RuntimeClientProtocolEvent> {
        Box::pin(async move {
            if let Some(event) = self.buffered_events.pop_front() {
                return event;
            }
            match self.receive().await {
                OutboundRecord::Event(event) => event,
                OutboundRecord::Response(response) => {
                    panic!("no request is outstanding, got response {response:?}")
                }
            }
        })
    }
}

/// The factory of [`StdioJsonlDriver`].
pub struct StdioJsonlDriverFactory;

impl DriverFactory for StdioJsonlDriverFactory {
    fn name(&self) -> &'static str {
        "stdio-jsonl"
    }

    fn connect(&self, host: &RuntimeClientHost) -> Box<dyn RuntimeClientProtocolDriver> {
        Box::new(StdioJsonlDriver::new(host))
    }
}

/// Every driver factory the conformance suite runs.
#[must_use]
pub fn all_driver_factories() -> Vec<Box<dyn DriverFactory>> {
    vec![
        Box::new(DirectDriverFactory),
        Box::new(StdioJsonlDriverFactory),
    ]
}

// ---------------------------------------------------------------------------
// The runtime fixture
// ---------------------------------------------------------------------------

/// The shared Runtime Client host fixture the scenarios build runtimes with.
///
/// Construction lives in [`crate::scripted_suites::support::runtime_client_fixture`] and is
/// shared with the Issue #37 semantic tests, so a scenario and its #37
/// counterpart always exercise an identically built runtime.
pub use crate::scripted_suites::support::runtime_client_fixture::{
    RuntimeClientFixture as ConformanceFixture, skill_location, uv_available, write_python_package,
    write_skill,
};

/// Connects one client session of the given transport to a fixture.
#[must_use]
pub fn connect(
    fixture: &ConformanceFixture,
    factory: &dyn DriverFactory,
) -> Box<dyn RuntimeClientProtocolDriver> {
    factory.connect(&fixture.host)
}

// ---------------------------------------------------------------------------
// Scenario helpers
// ---------------------------------------------------------------------------

/// One text content block.
#[must_use]
pub fn text(value: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock {
        text: value.to_owned(),
    })]
}

/// A model script that streams one text block and stops.
#[must_use]
pub fn one_turn_stop() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

/// A scripted model tool call with static identities.
#[must_use]
pub fn scripted_call(
    id: &str,
    tool_id: &str,
    name: &str,
    arguments: serde_json::Value,
) -> ScriptedCall {
    ScriptedCall {
        id: Box::leak(id.to_owned().into_boxed_str()),
        tool_id: Box::leak(tool_id.to_owned().into_boxed_str()),
        name: Box::leak(name.to_owned().into_boxed_str()),
        arguments,
    }
}

/// Unwraps the successful result of a response, or panics with the error.
#[must_use]
pub fn result(response: RuntimeClientResponse) -> RuntimeClientResult {
    match (response.result, response.error) {
        (Some(result), None) => result,
        (None, Some(error)) => panic!("expected a successful result, got error {error:?}"),
        other => panic!("a response carries exactly one of result/error, got {other:?}"),
    }
}

/// Unwraps the typed error of a response, or panics with the result.
#[must_use]
pub fn error(response: RuntimeClientResponse) -> RuntimeClientError {
    match (response.result, response.error) {
        (None, Some(error)) => error,
        (Some(result), None) => panic!("expected a typed error, got result {result:?}"),
        other => panic!("a response carries exactly one of result/error, got {other:?}"),
    }
}

/// Initializes one session and returns the linearized snapshot and cursor.
pub async fn initialize(
    driver: &mut dyn RuntimeClientProtocolDriver,
    id: u64,
) -> (RuntimeClientSnapshot, RuntimeClientCursor) {
    let response = driver
        .request(RuntimeClientRequest::Initialize {
            id: RequestId::new(id),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        })
        .await;
    assert_eq!(response.id, RequestId::new(id));
    let RuntimeClientResult::Initialized {
        snapshot, cursor, ..
    } = result(response)
    else {
        panic!("initialize returns the initialized result");
    };
    (snapshot, cursor)
}

/// Subscribes one session after a cursor.
pub async fn subscribe(
    driver: &mut dyn RuntimeClientProtocolDriver,
    id: u64,
    after_cursor: RuntimeClientCursor,
) {
    let response = driver
        .request(RuntimeClientRequest::SubscribeEvents {
            id: RequestId::new(id),
            after_cursor,
        })
        .await;
    assert_eq!(
        result(response),
        RuntimeClientResult::Subscribed { after_cursor }
    );
}

/// Submits one inbound text message and returns the accepted result.
pub async fn submit(
    driver: &mut dyn RuntimeClientProtocolDriver,
    id: u64,
    message: &str,
) -> RuntimeClientResult {
    result(
        driver
            .request(RuntimeClientRequest::SubmitInbound {
                id: RequestId::new(id),
                content: text(message),
            })
            .await,
    )
}

/// Reads the authoritative snapshot and its cursor.
pub async fn snapshot(
    driver: &mut dyn RuntimeClientProtocolDriver,
    id: u64,
) -> (RuntimeClientSnapshot, RuntimeClientCursor) {
    let RuntimeClientResult::Snapshot { snapshot, cursor } = result(
        driver
            .request(RuntimeClientRequest::SnapshotGet {
                id: RequestId::new(id),
            })
            .await,
    ) else {
        panic!("snapshot_get returns the snapshot result");
    };
    (snapshot, cursor)
}

/// Receives notifications until the predicate matches, asserting strictly
/// contiguous cursors, and returns everything observed.
pub async fn receive_until(
    driver: &mut dyn RuntimeClientProtocolDriver,
    after_cursor: RuntimeClientCursor,
    mut predicate: impl FnMut(&RuntimeClientProtocolEvent) -> bool,
) -> Vec<RuntimeClientProtocolEvent> {
    tokio::time::timeout(LIVENESS_GUARD, async {
        let mut expected = after_cursor.get();
        let mut seen = Vec::new();
        loop {
            let event = driver.next_event().await;
            expected += 1;
            assert_eq!(
                event.cursor.get(),
                expected,
                "one subscription observes strictly contiguous cursors"
            );
            let matched = predicate(&event);
            seen.push(event);
            if matched {
                return seen;
            }
        }
    })
    .await
    .expect("the observation stream must not stall")
}

/// Counts the terminal attempt settlements in an observed event sequence.
#[must_use]
pub fn settlements(events: &[RuntimeClientProtocolEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }))
        .count()
}

/// Waits until the scripted model is provably parked.
///
/// A barrier, not a sleep: once the model parks, the exact set of
/// publications that already happened is fixed and no further one can occur
/// until the test releases it.
pub async fn await_model_parked(model: &FakeModel) {
    let mut parked = model.parked();
    tokio::time::timeout(LIVENESS_GUARD, parked.wait_for(|parked| *parked))
        .await
        .expect("the model must park")
        .expect("the model park channel stays open");
}

// ---------------------------------------------------------------------------
// Protocol / session scenarios
// ---------------------------------------------------------------------------

/// `initialize` alone admits the attachment and returns the runtime-owned
/// identities plus the snapshot linearized with its cursor.
pub async fn initialize_admits_the_attachment(factory: &dyn DriverFactory) {
    let fixture = ConformanceFixture::builder(&conversation(factory, "init"))
        .build()
        .await;
    let mut driver = connect(&fixture, factory);

    // Before initialize every other method is the typed not-attached error;
    // the transport never decides this.
    let response = driver
        .request(RuntimeClientRequest::SnapshotGet {
            id: RequestId::new(1),
        })
        .await;
    assert_eq!(error(response), RuntimeClientError::NotAttached);

    let response = driver
        .request(RuntimeClientRequest::Initialize {
            id: RequestId::new(2),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        })
        .await;
    assert_eq!(response.id, RequestId::new(2));
    let RuntimeClientResult::Initialized {
        attachment_id,
        conversation_id,
        agent_id,
        snapshot,
        cursor,
    } = result(response)
    else {
        panic!("initialize returns the initialized result");
    };
    assert!(
        !attachment_id.as_str().is_empty(),
        "the runtime allocated the attachment identity"
    );
    assert_eq!(conversation_id.as_str(), conversation(factory, "init"));
    assert_eq!(agent_id.as_str(), "agent-a");
    assert_eq!(snapshot.conversation_id(), &conversation_id);

    // The attachment is live and the snapshot cursor is stable.
    let (_, again) = snapshot_of(&mut *driver, 3).await;
    assert_eq!(again, cursor);
}

/// An unsupported protocol version stays the typed semantic error and
/// admits nothing.
pub async fn unsupported_protocol_version_is_typed(factory: &dyn DriverFactory) {
    let fixture = ConformanceFixture::builder(&conversation(factory, "version"))
        .build()
        .await;
    let mut driver = connect(&fixture, factory);

    let response = driver
        .request(RuntimeClientRequest::Initialize {
            id: RequestId::new(7),
            protocol_version: 9,
        })
        .await;
    assert_eq!(response.id, RequestId::new(7));
    assert_eq!(
        error(response),
        RuntimeClientError::UnsupportedProtocolVersion {
            supported: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
            requested: 9,
        }
    );

    // A rejected negotiation admitted nothing, and the supported version
    // still attaches.
    let response = driver
        .request(RuntimeClientRequest::SnapshotGet {
            id: RequestId::new(8),
        })
        .await;
    assert_eq!(error(response), RuntimeClientError::NotAttached);
    initialize(&mut *driver, 9).await;
}

/// Responses echo request ids exactly, including out-of-order and repeated
/// client-chosen ids.
pub async fn responses_correlate_request_ids(factory: &dyn DriverFactory) {
    let fixture = ConformanceFixture::builder(&conversation(factory, "correlate"))
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    initialize(&mut *driver, 100).await;

    for id in [7_u64, 3, 4096, u64::MAX, 3] {
        let response = driver
            .request(RuntimeClientRequest::SnapshotGet {
                id: RequestId::new(id),
            })
            .await;
        assert_eq!(
            response.id,
            RequestId::new(id),
            "the response echoes the client-chosen request id"
        );
        assert!(response.error.is_none());
    }
}

/// A second attachment is rejected with the active identity and never
/// evicts the first.
pub async fn a_second_attachment_is_rejected(factory: &dyn DriverFactory) {
    let fixture = ConformanceFixture::builder(&conversation(factory, "second"))
        .build()
        .await;
    let mut first = connect(&fixture, factory);
    let mut second = connect(&fixture, factory);

    let response = first
        .request(RuntimeClientRequest::Initialize {
            id: RequestId::new(1),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        })
        .await;
    let RuntimeClientResult::Initialized {
        attachment_id: first_id,
        ..
    } = result(response)
    else {
        panic!("initialized");
    };

    let response = second
        .request(RuntimeClientRequest::Initialize {
            id: RequestId::new(1),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        })
        .await;
    assert_eq!(
        error(response),
        RuntimeClientError::AttachmentInUse {
            existing_attachment_id: first_id.clone(),
        }
    );

    // The first attachment was never evicted.
    let (_, _) = snapshot_of(&mut *first, 2).await;
}

/// Explicit detach releases the attachment without touching semantic state,
/// and the same session may re-initialize into a fresh identity.
pub async fn detach_then_reinitialize(factory: &dyn DriverFactory) {
    let fixture = ConformanceFixture::builder(&conversation(factory, "detach"))
        .script(one_turn_stop())
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (_, cursor) = initialize(&mut *driver, 1).await;
    subscribe(&mut *driver, 2, cursor).await;
    submit(&mut *driver, 3, "hello").await;
    let events = receive_until(&mut *driver, cursor, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    let settled_cursor = events.last().expect("at least one event").cursor;

    let response = driver
        .request(RuntimeClientRequest::Detach {
            id: RequestId::new(4),
        })
        .await;
    assert_eq!(result(response), RuntimeClientResult::Detached);

    let response = driver
        .request(RuntimeClientRequest::SnapshotGet {
            id: RequestId::new(5),
        })
        .await;
    assert_eq!(error(response), RuntimeClientError::NotAttached);

    // Re-initializing on the same session observes exactly the state detach
    // left behind: detach is never cancellation.
    let response = driver
        .request(RuntimeClientRequest::Initialize {
            id: RequestId::new(6),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        })
        .await;
    let RuntimeClientResult::Initialized {
        snapshot,
        cursor: reattached,
        ..
    } = result(response)
    else {
        panic!("initialized");
    };
    assert_eq!(reattached, settled_cursor);
    assert_eq!(
        snapshot.messages.len(),
        3,
        "the settled attempt and canonical history survived detach"
    );
}

/// A successful `shutdown` is semantic shutdown, not detach and not
/// cancellation: the session stays usable and reads keep working.
pub async fn shutdown_is_not_detach(factory: &dyn DriverFactory) {
    let fixture = ConformanceFixture::builder(&conversation(factory, "shutdown"))
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (before, cursor) = initialize(&mut *driver, 1).await;
    subscribe(&mut *driver, 2, cursor).await;

    let response = driver
        .request(RuntimeClientRequest::Shutdown {
            id: RequestId::new(3),
        })
        .await;
    assert_eq!(result(response), RuntimeClientResult::ShutdownCompleted);

    // The runtime shutdown observation is runtime-originated.
    let events = receive_until(&mut *driver, cursor, |event| {
        matches!(event.event, RuntimeClientEvent::RuntimeShutdown)
    })
    .await;
    assert_eq!(events.len(), 1);

    // Still attached: reads work, new inbound is refused with the typed
    // semantic error, and canonical history is untouched.
    let response = driver
        .request(RuntimeClientRequest::SubmitInbound {
            id: RequestId::new(4),
            content: text("late"),
        })
        .await;
    assert_eq!(error(response), RuntimeClientError::RuntimeShutdown);

    let (after, _) = snapshot_of(&mut *driver, 5).await;
    assert_eq!(after.messages, before.messages);
    let RuntimeClientResult::Capability { .. } = result(
        driver
            .request(RuntimeClientRequest::CapabilityGet {
                id: RequestId::new(6),
            })
            .await,
    ) else {
        panic!("reads stay available after shutdown");
    };
}

// ---------------------------------------------------------------------------
// Conversation / attempt lifecycle scenarios
// ---------------------------------------------------------------------------

/// A successful submission is acceptance, never settlement: the attempt
/// starts, streams, and settles exactly once, asynchronously.
pub async fn submission_acceptance_is_not_settlement(factory: &dyn DriverFactory) {
    let (release, release_rx) = super::fake::model_release();
    let fixture = ConformanceFixture::builder(&conversation(factory, "accept"))
        .script(vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ])
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (_, cursor) = initialize(&mut *driver, 1).await;
    subscribe(&mut *driver, 2, cursor).await;

    let accepted = submit(&mut *driver, 3, "hello").await;
    let RuntimeClientResult::InboundAccepted {
        message_id,
        inbound_sequence,
    } = accepted
    else {
        panic!("submit_inbound is accepted");
    };
    assert_eq!(
        inbound_sequence.get(),
        1,
        "the runtime assigned the sequence"
    );
    assert!(
        message_id
            .as_str()
            .starts_with(&conversation(factory, "accept")),
        "the runtime owns the canonical message identity"
    );

    // The model is parked, so the publications that happened are fixed: the
    // attempt started, and nothing settled.
    let observed = receive_until(&mut *driver, cursor, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
    })
    .await;
    await_model_parked(&fixture.model).await;
    assert_eq!(
        settlements(&observed),
        0,
        "acceptance is not settlement: nothing terminal was published yet"
    );
    let last = observed.last().expect("at least the start").cursor;

    release.send_replace(true);
    let rest = receive_until(&mut *driver, last, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    assert_eq!(
        settlements(&rest),
        1,
        "terminal settlement appears exactly once"
    );
    assert!(
        rest.iter()
            .any(|event| matches!(event.event, RuntimeClientEvent::AssistantTextDelta { .. })),
        "streaming assistant output is projected"
    );
    assert!(rest.iter().any(|event| matches!(
        event.event,
        RuntimeClientEvent::AttemptSettled {
            outcome: RuntimeClientOutcome::Completed { .. },
            ..
        }
    )));

    // The authoritative snapshot reflects committed state at its cursor.
    let (snapshot, snapshot_cursor) = snapshot_of(&mut *driver, 4).await;
    assert_eq!(snapshot_cursor, rest.last().expect("settled").cursor);
    assert_eq!(snapshot.messages.len(), 3);
    assert!(matches!(
        snapshot.attempt.expect("attempt").phase,
        rustx::runtime_client::RuntimeClientAttemptPhase::Settled { .. }
    ));
}

/// Submitting while busy queues the message; the runtime drains it into the
/// next turn and both the pending view and the drain are runtime-originated.
pub async fn inbound_batches_and_drains(factory: &dyn DriverFactory) {
    let (release, release_rx) = super::fake::model_release();
    let fixture = ConformanceFixture::builder(&conversation(factory, "batch"))
        .script(vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ])
        .script(one_turn_stop())
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (_, cursor) = initialize(&mut *driver, 1).await;
    subscribe(&mut *driver, 2, cursor).await;

    submit(&mut *driver, 3, "first").await;
    let started = receive_until(&mut *driver, cursor, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
    })
    .await;
    let observed = started.last().expect("the attempt started").cursor;
    await_model_parked(&fixture.model).await;

    // The second submission is admitted while the attempt runs.
    let RuntimeClientResult::InboundAccepted {
        message_id: queued_id,
        inbound_sequence,
    } = submit(&mut *driver, 4, "second").await
    else {
        panic!("submit_inbound is accepted while busy");
    };
    assert_eq!(inbound_sequence.get(), 2);

    // The pending projection is the runtime's mailbox state, not a client
    // or transport construction.
    let (busy, _) = snapshot_of(&mut *driver, 5).await;
    assert_eq!(busy.inbound.pending.len(), 1);
    assert_eq!(busy.inbound.pending[0].sequence, inbound_sequence);
    assert_eq!(busy.inbound.pending[0].message.id, queued_id);

    release.send_replace(true);
    let events = receive_until(&mut *driver, observed, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    // The drain is one finite, runtime-committed boundary.
    let drains: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::InboundDrained {
                watermark,
                count,
                message_ids,
            } => Some((*watermark, *count, message_ids.clone())),
            _ => None,
        })
        .collect();
    let drained = drains
        .iter()
        .find(|(watermark, _, _)| *watermark == inbound_sequence)
        .expect("the queued message drains into the next turn");
    assert_eq!(drained.1, 1);
    assert_eq!(drained.2, vec![queued_id]);

    let (settled, _) = snapshot_of(&mut *driver, 6).await;
    assert!(
        settled.inbound.pending.is_empty(),
        "the drained batch left the mailbox"
    );
    assert_eq!(
        settled
            .inbound
            .last_drain
            .expect("a drain was observed")
            .watermark,
        inbound_sequence
    );
}

/// Cancellation of the current attempt is acceptance; the terminal
/// settlement remains runtime-originated and appears exactly once.
pub async fn cancellation_is_acceptance_not_settlement(factory: &dyn DriverFactory) {
    let fixture = ConformanceFixture::builder(&conversation(factory, "cancel"))
        .script(vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilCancelled,
        ])
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (_, cursor) = initialize(&mut *driver, 1).await;
    subscribe(&mut *driver, 2, cursor).await;

    // No attempt yet: cancellation is the typed semantic error.
    let response = driver
        .request(RuntimeClientRequest::CancelCurrentAttempt {
            id: RequestId::new(3),
        })
        .await;
    assert_eq!(error(response), RuntimeClientError::NoCurrentAttempt);

    submit(&mut *driver, 4, "go").await;
    let started = receive_until(&mut *driver, cursor, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
    })
    .await;
    let last = started.last().expect("started").cursor;

    let RuntimeClientResult::AttemptCancellationAccepted { attempt_id } = result(
        driver
            .request(RuntimeClientRequest::CancelCurrentAttempt {
                id: RequestId::new(5),
            })
            .await,
    ) else {
        panic!("cancellation is accepted");
    };

    let events = receive_until(&mut *driver, last, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    assert_eq!(settlements(&events), 1);
    assert!(events.iter().any(|event| matches!(
        &event.event,
        RuntimeClientEvent::AttemptSettled {
            attempt_id: settled,
            outcome: RuntimeClientOutcome::Cancelled { .. },
        } if *settled == attempt_id
    )));
}

// ---------------------------------------------------------------------------
// Tool scenarios
// ---------------------------------------------------------------------------

/// One foreground tool call projects its whole lifecycle and continues to
/// the model with the committed result.
pub async fn foreground_tool_lifecycle(factory: &dyn DriverFactory) {
    let call = scripted_call("call-1", "tool-alpha", "alpha", serde_json::json!({"n": 1}));
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for event in super::fake::tool_call_events(0, &call) {
        first.push(FakeStep::Emit(event));
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let mut tools = ToolRegistry::new();
    FakeTool::new(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("a"),
    )
    .register(&mut tools);
    let fixture = ConformanceFixture::builder(&conversation(factory, "tool"))
        .script(first)
        .script(one_turn_stop())
        .tools(tools)
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (_, cursor) = initialize(&mut *driver, 1).await;
    subscribe(&mut *driver, 2, cursor).await;
    submit(&mut *driver, 3, "run the tool").await;

    let events = receive_until(&mut *driver, cursor, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;

    // The full generic tool lifecycle is projected, in order, with stable
    // identities throughout.
    let lifecycle: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::ToolCallStarted { .. } => Some("call_started"),
            RuntimeClientEvent::ToolCallAssembled { .. } => Some("call_assembled"),
            RuntimeClientEvent::ToolExecutionStarted { .. } => Some("execution_started"),
            RuntimeClientEvent::ToolExecutionSettled { .. } => Some("execution_settled"),
            _ => None,
        })
        .collect();
    assert_eq!(
        lifecycle,
        vec![
            "call_started",
            "call_assembled",
            "execution_started",
            "execution_settled"
        ]
    );
    for event in &events {
        match &event.event {
            RuntimeClientEvent::ToolExecutionStarted {
                tool_call_id,
                tool_id,
                ..
            }
            | RuntimeClientEvent::ToolExecutionSettled {
                tool_call_id,
                tool_id,
                ..
            } => {
                assert_eq!(tool_call_id.as_str(), "call-1");
                assert_eq!(tool_id.as_str(), "tool-alpha");
            }
            _ => {}
        }
    }

    // The result continued to the model: the second request carries it.
    let requests = fixture.model.requests();
    assert_eq!(requests.len(), 2, "the tool result continued the turn");
    assert!(
        requests[1].messages.iter().any(|message| matches!(
            message,
            MessageBlock::Tool(tool) if tool.tool_call_id.as_str() == "call-1"
        )),
        "the continuation carries the committed tool result"
    );

    let (snapshot, _) = snapshot_of(&mut *driver, 4).await;
    let foreground = &snapshot.attempt.expect("attempt").foreground;
    assert_eq!(foreground.len(), 1);
    assert_eq!(foreground[0].call_id.as_str(), "call-1");
}

/// Two parallel tool calls keep independent identities, and the
/// client-visible order is the canonical model-call order even when
/// physical completion is reversed.
pub async fn parallel_tools_keep_independent_identities(factory: &dyn DriverFactory) {
    let call_a = scripted_call("call-a", "tool-alpha", "alpha", serde_json::json!({"n": 1}));
    let call_b = scripted_call("call-b", "tool-beta", "beta", serde_json::json!({"n": 2}));
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for (block, call) in [&call_a, &call_b].into_iter().enumerate() {
        let block = u32::try_from(block).expect("fits");
        for event in super::fake::tool_call_events(block, call) {
            first.push(FakeStep::Emit(event));
        }
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));

    let (tool_a, release_a) = FakeTool::parking(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("a"),
    );
    let (tool_b, release_b) = FakeTool::parking(
        common::tool_policies(
            "beta",
            "tool-beta",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("b"),
    );
    let mut started_a = tool_a.started();
    let mut started_b = tool_b.started();
    let mut completed_b = tool_b.completed();
    let mut tools = ToolRegistry::new();
    tool_a.register(&mut tools);
    tool_b.register(&mut tools);

    let fixture = ConformanceFixture::builder(&conversation(factory, "parallel"))
        .script(first)
        .script(one_turn_stop())
        .tools(tools)
        .build()
        .await;

    // Deterministic reversal: B is released and physically completes first.
    let controller = tokio::spawn(async move {
        await_started(&mut started_a, "A ran").await;
        await_started(&mut started_b, "B ran").await;
        release_b.send_replace(true);
        completed_b
            .wait_for(|order| order.iter().any(|name| name == "beta"))
            .await
            .expect("B completed first");
        release_a.send_replace(true);
    });

    let mut driver = connect(&fixture, factory);
    let (_, cursor) = initialize(&mut *driver, 1).await;
    subscribe(&mut *driver, 2, cursor).await;
    submit(&mut *driver, 3, "run both").await;
    let events = receive_until(&mut *driver, cursor, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    controller.await.expect("controller");

    let started: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::ToolExecutionStarted { tool_call_id, .. } => {
                Some(tool_call_id.as_str())
            }
            _ => None,
        })
        .collect();
    let settled: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::ToolExecutionSettled { tool_call_id, .. } => {
                Some(tool_call_id.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(started, vec!["call-a", "call-b"]);
    assert_eq!(
        settled,
        vec!["call-a", "call-b"],
        "the client-visible order is the canonical call order, not physical completion"
    );

    let (snapshot, _) = snapshot_of(&mut *driver, 4).await;
    let foreground = snapshot.attempt.expect("attempt").foreground;
    assert_eq!(foreground.len(), 2);
    assert_eq!(foreground[0].call_id.as_str(), "call-a");
    assert_eq!(foreground[1].call_id.as_str(), "call-b");
}

/// Background execution is conversation-owned: dispatch is accepted inside
/// the attempt, lifecycle and completion are runtime-originated, and
/// `background_cancel` is acceptance rather than settlement.
#[allow(clippy::too_many_lines)] // one complete background lifecycle
pub async fn background_execution_lifecycle(factory: &dyn DriverFactory) {
    let call = scripted_call(
        "call-bg",
        "tool-bg",
        "bg",
        serde_json::json!({"execution_mode": "background"}),
    );
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for event in super::fake::tool_call_events(0, &call) {
        first.push(FakeStep::Emit(event));
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));

    let (tool, _release) = FakeTool::parking(
        common::tool_policies(
            "bg",
            "tool-bg",
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("bg"),
    );
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);

    let fixture = ConformanceFixture::builder(&conversation(factory, "background"))
        .script(first)
        .script(one_turn_stop())
        .tools(tools)
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (_, cursor) = initialize(&mut *driver, 1).await;
    subscribe(&mut *driver, 2, cursor).await;
    submit(&mut *driver, 3, "dispatch").await;

    // Dispatch is observed as a runtime registry transition.
    let events = receive_until(&mut *driver, cursor, |event| {
        matches!(
            event.event,
            RuntimeClientEvent::BackgroundExecutionUpdated { .. }
        )
    })
    .await;
    let RuntimeClientEvent::BackgroundExecutionUpdated { execution } =
        &events.last().expect("a dispatch transition").event
    else {
        panic!("background transition");
    };
    let execution_id = execution.execution_id.clone();
    assert_eq!(execution.tool_id.as_str(), "tool-bg");
    let mut last = events.last().expect("dispatch").cursor;

    // The registry is inspectable by identity, and an unknown identity is
    // the typed semantic error.
    let RuntimeClientResult::BackgroundStatus { execution } = result(
        driver
            .request(RuntimeClientRequest::BackgroundStatus {
                id: RequestId::new(4),
                execution_id: execution_id.clone(),
            })
            .await,
    ) else {
        panic!("background_status returns the registry snapshot");
    };
    assert_eq!(execution.execution_id, execution_id);
    let response = driver
        .request(RuntimeClientRequest::BackgroundStatus {
            id: RequestId::new(5),
            execution_id: ToolExecutionId::new("exec_absent"),
        })
        .await;
    assert_eq!(
        error(response),
        RuntimeClientError::UnknownBackgroundExecution {
            execution_id: ToolExecutionId::new("exec_absent"),
        }
    );

    // Cancellation is acceptance: the response carries the registry
    // snapshot after the request, never the terminal result.
    let RuntimeClientResult::BackgroundCancelAccepted { .. } = result(
        driver
            .request(RuntimeClientRequest::BackgroundCancel {
                id: RequestId::new(6),
                execution_id: execution_id.clone(),
            })
            .await,
    ) else {
        panic!("background_cancel is accepted");
    };

    // Terminal settlement stays runtime-originated and arrives on the
    // observation stream.
    loop {
        let events = receive_until(&mut *driver, last, |event| {
            matches!(
                event.event,
                RuntimeClientEvent::BackgroundExecutionUpdated { .. }
            )
        })
        .await;
        last = events.last().expect("a transition").cursor;
        let RuntimeClientEvent::BackgroundExecutionUpdated { execution } =
            &events.last().expect("a transition").event
        else {
            panic!("background transition");
        };
        if execution.state.is_terminal() {
            assert_eq!(execution.execution_id, execution_id);
            assert!(
                execution.result.is_some(),
                "a terminal registry record carries its bounded result"
            );
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Projection scenarios
// ---------------------------------------------------------------------------

/// Agent Status is a runtime-owned projection: the transport neither
/// composes nor reinterprets it, and the event and the snapshot carry the
/// one composition.
pub async fn agent_status_is_runtime_owned(factory: &dyn DriverFactory) {
    let fixture = ConformanceFixture::builder(&conversation(factory, "status"))
        .script(one_turn_stop())
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (initial, cursor) = initialize(&mut *driver, 1).await;
    assert!(
        initial.status.is_none(),
        "no status exists before the first turn"
    );
    subscribe(&mut *driver, 2, cursor).await;
    let RuntimeClientResult::InboundAccepted { message_id, .. } =
        submit(&mut *driver, 3, "hello").await
    else {
        panic!("accepted");
    };

    let events = receive_until(&mut *driver, cursor, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    let composed: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::AgentStatusComposed { turn, status, .. } => Some((
                *turn,
                status.status_message_id.clone(),
                status
                    .opportunities
                    .fresh_inbound
                    .as_ref()
                    .expect("FreshInbound is populated by the current producer")
                    .target_message_id
                    .clone(),
                status.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(composed.len(), 1, "one composition per fresh inbound turn");
    let (turn, status_message_id, target, status) = &composed[0];
    assert_eq!(*turn, 1);
    assert_eq!(target, &message_id);
    assert!(status.opportunities.fresh_inbound.is_some());
    assert!(
        !status.sections.is_empty(),
        "the structured sections are projected, not only the rendering"
    );
    assert!(!status.rendered.is_empty());

    // The snapshot carries the same composed observation.
    let (snapshot, _) = snapshot_of(&mut *driver, 4).await;
    assert_eq!(snapshot.status.as_ref(), Some(status));
    assert!(snapshot.messages.iter().any(|message| {
        matches!(
            message,
            rustx::message::types::MessageBlock::User(user)
                if user.id == *status_message_id
        )
    }));
}

/// The capability projection carries the revision, the deterministic tool
/// catalog with typed origin metadata, and the Skill-level visible catalog
/// including each exact virtual Read locator. No executor or environment
/// internals are exposed.
pub async fn capability_projection_is_deterministic(factory: &dyn DriverFactory) {
    let mut base = ToolRegistry::new();
    FakeTool::new(
        common::tool_policies(
            "ls",
            "tool-ls",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("listed"),
    )
    .register(&mut base);

    let fixture = ConformanceFixture::builder(&conversation(factory, "capability"))
        .tools(base)
        .native_tools()
        .tool_activation(rustx::capabilities::ToolActivationPolicy {
            tools: Some(vec!["ls".to_owned(), "read".to_owned()]),
            ..rustx::capabilities::ToolActivationPolicy::default()
        })
        .workspace_fixture(|workspace| {
            write_skill(workspace, "skill-readme", "Reads the README");
        })
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    initialize(&mut *driver, 1).await;

    let RuntimeClientResult::Capability { capabilities } = result(
        driver
            .request(RuntimeClientRequest::CapabilityGet {
                id: RequestId::new(2),
            })
            .await,
    ) else {
        panic!("capability_get returns the projection");
    };
    assert!(capabilities.revision.get() >= 1, "an activated revision");
    assert_eq!(
        capabilities
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ls", "read"]
    );
    assert_eq!(capabilities.tools[0].origin, ToolOrigin::Builtin);
    assert_eq!(capabilities.skills.len(), 1);
    assert_eq!(capabilities.skills[0].name, "skill-readme");
    assert_eq!(
        capabilities.skills[0].location,
        skill_location(
            fixture.runtime.tool_runtime().workspace().root(),
            "skill-readme",
        )
    );

    // Deterministic: a second read is byte-identical, and the snapshot
    // carries the same view.
    let RuntimeClientResult::Capability {
        capabilities: again,
    } = result(
        driver
            .request(RuntimeClientRequest::CapabilityGet {
                id: RequestId::new(3),
            })
            .await,
    )
    else {
        panic!("capability_get returns the projection");
    };
    assert_eq!(again, capabilities);
    let (snapshot, _) = snapshot_of(&mut *driver, 4).await;
    assert_eq!(snapshot.capabilities, capabilities);

    // The wire shape carries no executor or environment internals.
    let wire = serde_json::to_string(&capabilities).expect("the projection serializes");
    for forbidden in [
        "\"executor\":",
        "\"environment\":",
        "\"interpreter\":",
        "\"worker\":",
    ] {
        assert!(
            !wire.contains(forbidden),
            "the capability wire shape must not carry `{forbidden}`: {wire}"
        );
    }
}

/// Python tool packages are projected with typed Python origin metadata.
///
/// Opt-in by `uv` availability, mirroring the existing Issue #37 capability
/// fixture: a missing `uv` skips the environment step entirely.
pub async fn capability_projection_covers_python_origins(factory: &dyn DriverFactory) {
    if !uv_available() {
        eprintln!("uv unavailable; the Python capability origin is not exercised");
        return;
    }
    let fixture = ConformanceFixture::builder(&conversation(factory, "python"))
        .workspace_fixture(|workspace| {
            write_python_package(workspace, "py-echo", "Echoes arguments");
        })
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    initialize(&mut *driver, 1).await;
    let RuntimeClientResult::Capability { capabilities } = result(
        driver
            .request(RuntimeClientRequest::CapabilityGet {
                id: RequestId::new(2),
            })
            .await,
    ) else {
        panic!("capability_get returns the projection");
    };
    let python = capabilities
        .tools
        .iter()
        .find(|tool| tool.name == "py-echo")
        .expect("the Python tool is discovered");
    assert!(matches!(
        &python.origin,
        ToolOrigin::Python { tool_version_id } if !tool_version_id.as_str().is_empty()
    ));
}

/// MCP tools are projected with typed MCP origin metadata carrying the
/// server identity, and no MCP SDK or transport data reaches the wire.
///
/// `test_name` is the exact generated test path, which the fixture uses to
/// re-run this binary as the MCP server child process.
#[cfg(all(unix, feature = "mcp-fixture"))]
pub async fn capability_projection_covers_mcp_origins(
    factory: &dyn DriverFactory,
    test_name: &str,
) {
    use rustx::tools::mcp::fixture;

    // The re-run of this binary in fixture mode *is* the MCP server.
    if fixture::serve_if_fixture_mode(fixture::FixtureServer::from_env()).await {
        return;
    }
    let servers = rustx::tools::mcp::McpServerBindings::from([(
        rustx::runtime::identity::McpServerId::new("fixture"),
        rustx::tools::mcp::McpServerBinding {
            transport: rustx::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture::fixture_spawn_args(test_name),
                cwd: None,
                environment: std::collections::BTreeMap::from([(
                    fixture::FIXTURE_MODE_ENV.to_owned(),
                    "1".to_owned(),
                )]),
            },
            policy: rustx::tools::types::ToolInvocationPolicy::default(),
        },
    )]);
    let fixture = ConformanceFixture::builder(&conversation(factory, "mcp"))
        .mcp_servers(servers)
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    initialize(&mut *driver, 1).await;

    let RuntimeClientResult::Capability { capabilities } = result(
        driver
            .request(RuntimeClientRequest::CapabilityGet {
                id: RequestId::new(2),
            })
            .await,
    ) else {
        panic!("capability_get returns the projection");
    };
    let mcp: Vec<_> = capabilities
        .tools
        .iter()
        .filter(|tool| matches!(tool.origin, ToolOrigin::Mcp { .. }))
        .collect();
    assert_eq!(
        mcp.iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["echo", "mutate", "slow"]
    );
    for tool in mcp {
        assert!(matches!(
            &tool.origin,
            ToolOrigin::Mcp { server_id } if server_id.as_str() == "fixture"
        ));
    }
    let wire = serde_json::to_string(&capabilities).expect("the projection serializes");
    for forbidden in ["transport", "rmcp", "executor"] {
        assert!(
            !wire.contains(forbidden),
            "the wire projection must not carry `{forbidden}`"
        );
    }
}

/// Snapshot and cursor are linearized together, and subscribing after a
/// snapshot cursor observes exactly the later events, contiguously.
pub async fn snapshot_and_cursor_linearize(factory: &dyn DriverFactory) {
    let (release, release_rx) = super::fake::model_release();
    let fixture = ConformanceFixture::builder(&conversation(factory, "cursor"))
        .script(vec![
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ])
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (initial, initial_cursor) = initialize(&mut *driver, 1).await;
    assert!(initial.messages.is_empty());

    submit(&mut *driver, 2, "hello").await;
    await_model_parked(&fixture.model).await;

    // A snapshot taken now contains every Runtime Client state change
    // through its cursor: the inbound message is already committed.
    let (mid, mid_cursor) = snapshot_of(&mut *driver, 3).await;
    assert!(mid_cursor > initial_cursor);
    assert_eq!(mid.messages.len(), 2);

    // Subscribing after that cursor observes only later events, with no gap
    // and no replay of what the snapshot already contains.
    subscribe(&mut *driver, 4, mid_cursor).await;
    release.send_replace(true);
    let events = receive_until(&mut *driver, mid_cursor, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    assert!(!events.is_empty());

    let (settled, settled_cursor) = snapshot_of(&mut *driver, 5).await;
    assert_eq!(settled_cursor, events.last().expect("settled").cursor);
    assert_eq!(settled.messages.len(), 3);
}

/// An unserviceable cursor fails with the typed `resync_required`, a fresh
/// authoritative snapshot repairs the client, and resuming after the
/// repaired cursor continues contiguously.
pub async fn resync_required_and_snapshot_repair(factory: &dyn DriverFactory) {
    let (release, release_rx) = super::fake::model_release();
    let fixture = ConformanceFixture::builder(&conversation(factory, "resync"))
        // A deliberately tiny replay ring: the admission burst evicts the
        // events an old cursor would still need.
        .replay_limit(Some(2))
        .script(vec![
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ])
        .build()
        .await;
    let mut driver = connect(&fixture, factory);
    let (_, stale) = initialize(&mut *driver, 1).await;

    submit(&mut *driver, 2, "go").await;
    // Barrier: the model parked, so the admission burst is complete and
    // no further publication can happen.
    await_model_parked(&fixture.model).await;

    let response = driver
        .request(RuntimeClientRequest::SubscribeEvents {
            id: RequestId::new(3),
            after_cursor: stale,
        })
        .await;
    let RuntimeClientError::ResyncRequired {
        after_cursor,
        earliest_serviceable,
    } = error(response)
    else {
        panic!("an evicted cursor is unserviceable");
    };
    assert_eq!(after_cursor, stale);
    assert!(earliest_serviceable > stale);

    // A fresh authoritative snapshot repairs every externally visible fact,
    // and its cursor is always serviceable.
    let (repaired, repaired_cursor) = snapshot_of(&mut *driver, 4).await;
    assert_eq!(
        repaired.messages.len(),
        2,
        "the inbound message and admitted Agent Status fact committed"
    );
    subscribe(&mut *driver, 5, repaired_cursor).await;

    // Continuing from the repaired cursor is contiguous: one further
    // admission publishes exactly the next event.
    let RuntimeClientResult::InboundAccepted {
        inbound_sequence, ..
    } = submit(&mut *driver, 6, "again").await
    else {
        panic!("accepted");
    };
    let events = receive_until(&mut *driver, repaired_cursor, |event| {
        matches!(event.event, RuntimeClientEvent::InboundEnqueued { .. })
    })
    .await;
    let RuntimeClientEvent::InboundEnqueued { sequence, .. } =
        &events.last().expect("the enqueue").event
    else {
        panic!("enqueued");
    };
    assert_eq!(*sequence, inbound_sequence);

    // Release the parked attempt so the fixture tears down cleanly.
    release.send_replace(true);
}

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// A per-transport conversation identity, so two driver runs of one
/// scenario never share a runtime identity.
#[must_use]
fn conversation(factory: &dyn DriverFactory, scenario: &str) -> String {
    format!("conv-38-{}-{scenario}", factory.name())
}

/// Reads the snapshot, keeping the borrow local.
async fn snapshot_of(
    driver: &mut dyn RuntimeClientProtocolDriver,
    id: u64,
) -> (RuntimeClientSnapshot, RuntimeClientCursor) {
    snapshot(driver, id).await
}
