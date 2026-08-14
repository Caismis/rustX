//! Runtime Client Protocol v1: the transport-neutral protocol contract.
//!
//! This module owns the explicit external protocol boundary of Issue #37.
//! It is deliberately **not** the internal runtime fact vocabulary
//! ([`RuntimeEvent`](crate::events::types::RuntimeEvent)) and not the
//! compiled manifest protocol (`crate::protocol`): the Runtime Client
//! projection defines its own external shapes, its own versioning, its own
//! cursor domain, and its own error model. Internal `RuntimeEvent` schema
//! evolution can therefore never break the Runtime Client wire contract by
//! construction.
//!
//! # Protocol version
//!
//! [`RUNTIME_CLIENT_PROTOCOL_VERSION_V1`] is independent from
//! [`EVENT_SCHEMA_VERSION`](crate::events::types::EVENT_SCHEMA_VERSION),
//! [`MANIFEST_SCHEMA_VERSION`](crate::protocol::manifest::MANIFEST_SCHEMA_VERSION),
//! the crate version, and any future Event Journal schema. Attachment
//! initialization performs an explicit version negotiation; v1 rejects
//! every other version explicitly.
//!
//! # Envelope
//!
//! The envelope is a transport-neutral JSON-RPC-style message model:
//!
//! ```text
//! request(id, method + typed params)
//! response(id, result | error)
//! event(cursor + RuntimeClientEvent payload) — no request id exists
//! ```
//!
//! Request ids are scoped to exactly one attachment. Notifications never
//! fabricate request ids: [`RuntimeClientProtocolEvent`] structurally has
//! no `id` field. Every concrete v1 method is client-initiated; the
//! envelope remains structurally capable of peer-initiated requests in a
//! later protocol version.
//!
//! No transport lives here: JSONL/stdio framing is owned by Issue #38 and
//! any WebSocket transport by Issue #36; both consume this semantic layer
//! without redefining it.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::event::RuntimeClientEvent;
use super::snapshot::{CapabilityView, RuntimeClientSnapshot};
use crate::message::types::UserContentBlock;
use crate::model::catalog::ModelCatalogView;
use crate::model::session::{SessionModelConfig, SessionModelView};
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolExecutionId};
use crate::runtime::inbound::InboundSequence;

/// The current Runtime Client Protocol version.
///
/// Deliberately independent from the internal event schema version, the
/// manifest schema version, and the crate version: representing or changing
/// this version never implies anything about internal schemas.
pub const RUNTIME_CLIENT_PROTOCOL_VERSION_V1: u16 = 1;

/// The external cursor of the Runtime Client observation stream.
///
/// A cursor identifies one position in the externally visible
/// `RuntimeClientEvent` sequence: cursor `C` means "every Runtime Client
/// event through event `C` has been applied to the snapshot". It is:
///
/// - monotonic within the Runtime Client observation stream;
/// - **not** an alias of `u64`;
/// - **not** the mailbox [`InboundSequence`](crate::runtime::inbound::InboundSequence);
/// - **not** [`RuntimeEventEnvelope::sequence`](crate::events::types::RuntimeEventEnvelope);
/// - **not** any future Event Journal sequence;
/// - owned by the runtime observation stream, so it survives attachment
///   detach/reconnect;
/// - independent of its numeric representation: protocol versioning never
///   derives anything from cursor encoding.
///
/// Cursor allocation is committed by exactly one linearization owner (the
/// Runtime Client projection) together with event publication; overflow
/// fails explicitly and never wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeClientCursor(u64);

impl RuntimeClientCursor {
    /// Creates a cursor from a raw value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw cursor value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RuntimeClientCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The identity of one Runtime Client attachment.
///
/// Distinct from [`ConversationId`], [`AttemptId`], [`RuntimeClientCursor`],
/// and request ids: one attachment is one client session, and reconnecting
/// always receives a new attachment identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttachmentId(String);

impl AttachmentId {
    /// Creates an attachment identity from a string value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The attachment identity as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AttachmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A request id scoped to exactly one attachment.
///
/// Request ids do not carry across attachments: a fresh attachment starts
/// a fresh request-id scope. The id is opaque correlation data chosen by
/// the client; the runtime never interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(u64);

impl RequestId {
    /// Creates a request id from a raw value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw request id value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One client-initiated Runtime Client Protocol v1 request.
///
/// The `method` tag is the stable protocol discriminator; every method
/// carries its typed params. Unknown fields and unknown methods are
/// rejected on deserialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientRequest {
    /// Negotiate protocol version and admit the attachment.
    Initialize {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The Runtime Client Protocol version the client speaks.
        protocol_version: u16,
    },
    /// Submit one inbound user message.
    ///
    /// The runtime owns authoritative metadata (message identity, inbound
    /// sequencing, timestamp, provenance). A successful response means the
    /// message was accepted/admitted — never that the assistant finished.
    SubmitInbound {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The inbound content blocks.
        content: Vec<UserContentBlock>,
    },
    /// Request cancellation of the current attempt.
    ///
    /// Acceptance is not terminal settlement: actual settlement is owned by
    /// the Agent Loop and observed asynchronously through Runtime Client
    /// events.
    CancelCurrentAttempt {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Read the authoritative snapshot and its cursor.
    SnapshotGet {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Subscribe to Runtime Client events after a cursor.
    ///
    /// A successful subscription observes every subsequently published
    /// event after `after_cursor`; an unserviceable cursor fails explicitly
    /// with `resync_required` (the stream never silently jumps).
    SubscribeEvents {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The last cursor the client already observed.
        after_cursor: RuntimeClientCursor,
    },
    /// Inspect the active capability projection.
    CapabilityGet {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Read the safe public model catalog: which models and reasoning
    /// profiles this runtime can select.
    ///
    /// This exists so a client never reads `models.json` itself. The result
    /// carries no credential, no adapter internal, and no compat
    /// implementation object.
    ModelCatalogGet {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Read the authoritative session model state.
    ModelGet {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Replace the authoritative session model configuration.
    ///
    /// This is a whole-state replacement, never a patch. Validation is
    /// transactional: an invalid update changes nothing and publishes
    /// nothing. A valid update may occur while an attempt is running; it
    /// affects **future admissions only**, and the running attempt keeps the
    /// model it froze at its own admission.
    ModelSet {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The complete desired session model configuration.
        config: Box<SessionModelConfig>,
    },
    /// Inspect one background execution.
    BackgroundStatus {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The detached execution identity.
        execution_id: ToolExecutionId,
    },
    /// Request cancellation of one background execution.
    ///
    /// Acceptance and eventual settlement remain distinct: the response
    /// carries the registry snapshot after the request, never the terminal
    /// result.
    BackgroundCancel {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The detached execution identity.
        execution_id: ToolExecutionId,
    },
    /// Release the attachment.
    Detach {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Request local-runtime shutdown.
    ///
    /// Shutdown is not detach and not cancellation: the current attempt
    /// continues to its settlement, semantic runtime work is never mutated,
    /// and no further inbound admission occurs.
    Shutdown {
        /// Attachment-scoped request id.
        id: RequestId,
    },
}

impl RuntimeClientRequest {
    /// The attachment-scoped request id of the request.
    #[must_use]
    pub fn id(&self) -> RequestId {
        match self {
            Self::Initialize { id, .. }
            | Self::SubmitInbound { id, .. }
            | Self::CancelCurrentAttempt { id, .. }
            | Self::SnapshotGet { id, .. }
            | Self::SubscribeEvents { id, .. }
            | Self::CapabilityGet { id, .. }
            | Self::ModelCatalogGet { id, .. }
            | Self::ModelGet { id, .. }
            | Self::ModelSet { id, .. }
            | Self::BackgroundStatus { id, .. }
            | Self::BackgroundCancel { id, .. }
            | Self::Detach { id, .. }
            | Self::Shutdown { id, .. } => *id,
        }
    }

    /// The stable method discriminator.
    #[must_use]
    pub fn method(&self) -> &'static str {
        match self {
            Self::Initialize { .. } => "initialize",
            Self::SubmitInbound { .. } => "submit_inbound",
            Self::CancelCurrentAttempt { .. } => "cancel_current_attempt",
            Self::SnapshotGet { .. } => "snapshot_get",
            Self::SubscribeEvents { .. } => "subscribe_events",
            Self::CapabilityGet { .. } => "capability_get",
            Self::ModelCatalogGet { .. } => "model_catalog_get",
            Self::ModelGet { .. } => "model_get",
            Self::ModelSet { .. } => "model_set",
            Self::BackgroundStatus { .. } => "background_status",
            Self::BackgroundCancel { .. } => "background_cancel",
            Self::Detach { .. } => "detach",
            Self::Shutdown { .. } => "shutdown",
        }
    }
}

/// One Runtime Client Protocol response.
///
/// Exactly one of `result`/`error` is present. The request id is echoed
/// from the corresponding request, so responses correlate correctly even
/// under request pipelining.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientResponse {
    /// The echoed attachment-scoped request id.
    pub id: RequestId,
    /// The method result, when the request succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<RuntimeClientResult>,
    /// The typed protocol error, when the request failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RuntimeClientError>,
}

/// The typed success payloads of Runtime Client Protocol v1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientResult {
    /// `initialize` succeeded: the attachment is admitted and the initial
    /// authoritative snapshot (linearized with its cursor) is returned.
    Initialized {
        /// The fresh attachment identity.
        attachment_id: AttachmentId,
        /// The conversation this runtime serves.
        conversation_id: ConversationId,
        /// The agent executed by attempts of this runtime.
        agent_id: AgentId,
        /// The authoritative snapshot at the returned cursor.
        snapshot: RuntimeClientSnapshot,
        /// The cursor the snapshot is linearized at.
        cursor: RuntimeClientCursor,
    },
    /// `submit_inbound` succeeded: the message was accepted/admitted.
    InboundAccepted {
        /// The runtime-assigned canonical message identity.
        message_id: MessageId,
        /// The mailbox-assigned inbound sequence.
        inbound_sequence: InboundSequence,
    },
    /// `cancel_current_attempt` succeeded: cancellation was requested. This
    /// is acceptance, never terminal settlement.
    AttemptCancellationAccepted {
        /// The attempt cancellation was requested for.
        attempt_id: AttemptId,
    },
    /// `snapshot_get` succeeded.
    Snapshot {
        /// The authoritative snapshot at the returned cursor.
        snapshot: RuntimeClientSnapshot,
        /// The cursor the snapshot is linearized at.
        cursor: RuntimeClientCursor,
    },
    /// `subscribe_events` succeeded.
    Subscribed {
        /// The cursor the subscription resumes after.
        after_cursor: RuntimeClientCursor,
    },
    /// `capability_get` succeeded: the active capability projection.
    Capability {
        /// The deterministic active capability projection.
        capabilities: CapabilityView,
    },
    /// `model_catalog_get` succeeded: the safe selectable-model view.
    ModelCatalog {
        /// The bounded public catalog view.
        catalog: ModelCatalogView,
    },
    /// `model_get` succeeded: the authoritative session model state.
    Model {
        /// The redacted session model view.
        model: Box<SessionModelView>,
    },
    /// `model_set` succeeded: the update was applied and published.
    ///
    /// The result carries the *session* state after the update. It never
    /// implies that a running attempt changed model.
    ModelSet {
        /// The redacted session model view after the update.
        model: Box<SessionModelView>,
    },
    /// `background_status` succeeded.
    BackgroundStatus {
        /// The canonical registry snapshot of the execution.
        execution: RuntimeClientBackgroundExecution,
    },
    /// `background_cancel` succeeded: the request was processed by the
    /// authoritative registry. Acceptance is never terminal settlement.
    BackgroundCancelAccepted {
        /// The registry snapshot after processing the request.
        execution: RuntimeClientBackgroundExecution,
    },
    /// `detach` succeeded.
    Detached,
    /// `shutdown` succeeded: the request was accepted.
    ShutdownAccepted,
}

/// The typed protocol-visible errors of Runtime Client Protocol v1.
///
/// Every protocol-visible failure maps to one category; provider SDK
/// errors and internal synchronization failures are never exposed as
/// protocol structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientError {
    /// The negotiated protocol version is not supported by this runtime.
    UnsupportedProtocolVersion {
        /// The version this runtime speaks.
        supported: u16,
        /// The version the client requested.
        requested: u16,
    },
    /// An attachment is already active: v1 allows at most one attachment
    /// per runtime instance, and an attach never evicts the active one.
    AttachmentInUse {
        /// The identity of the active attachment.
        existing_attachment_id: AttachmentId,
    },
    /// The request arrived without an admitted attachment.
    NotAttached,
    /// The request payload is malformed or contradictory.
    InvalidRequest {
        /// Human-readable detail.
        message: String,
    },
    /// No attempt is currently cancellable.
    NoCurrentAttempt,
    /// The referenced background execution does not exist in the
    /// authoritative conversation registry.
    UnknownBackgroundExecution {
        /// The referenced execution identity.
        execution_id: ToolExecutionId,
    },
    /// The requested cursor is not serviceable by the bounded pre-M8
    /// replay buffer; the client must take a fresh snapshot.
    ResyncRequired {
        /// The cursor the client asked to resume after.
        after_cursor: RuntimeClientCursor,
        /// The oldest cursor the runtime can still serve.
        earliest_serviceable: RuntimeClientCursor,
    },
    /// The runtime is shutting down and no longer admits inbound work.
    RuntimeShutdown,
    /// The operation is impossible in the current authoritative runtime
    /// state.
    InvalidState {
        /// Human-readable detail.
        message: String,
    },
    /// A model configuration update could not be resolved.
    ///
    /// The update was rejected as a whole: no session state changed and no
    /// model-configuration event was published. The message never carries a
    /// credential.
    InvalidModelConfiguration {
        /// Human-readable detail.
        message: String,
    },
    /// The Runtime Client observation stream is exhausted.
    ProjectionExhausted,
    /// A runtime-internal failure prevented the operation; no provider
    /// detail is exposed.
    RuntimeFailure {
        /// Human-readable detail.
        message: String,
    },
}

/// One Runtime Client event pushed on the observation stream.
///
/// The envelope carries the cursor the event was published at, so a client
/// resuming after a cursor always knows the exact stream position of every
/// observation. Notifications structurally carry no request id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientProtocolEvent {
    /// The cursor this event was published at.
    pub cursor: RuntimeClientCursor,
    /// The typed external event payload.
    pub event: RuntimeClientEvent,
}

// Re-exported for use by the public protocol docs.
pub use super::snapshot::RuntimeClientBackgroundExecution;

#[cfg(test)]
mod tests {
    use super::{
        RUNTIME_CLIENT_PROTOCOL_VERSION_V1, RuntimeClientCursor, RuntimeClientError,
        RuntimeClientProtocolEvent, RuntimeClientRequest, RuntimeClientResponse,
        RuntimeClientResult,
    };
    use crate::events::types::EVENT_SCHEMA_VERSION;
    use crate::message::content::TextBlock;
    use crate::message::types::UserContentBlock;
    use crate::runtime::identity::{AttemptId, ToolExecutionId};
    use crate::runtime_client::event::RuntimeClientEvent;

    /// The Runtime Client protocol version is a distinct constant from the
    /// internal event schema version: representing or changing one never
    /// implies anything about the other.
    #[test]
    fn protocol_version_is_independent_from_event_schema_version() {
        let _ = EVENT_SCHEMA_VERSION;
        assert_eq!(RUNTIME_CLIENT_PROTOCOL_VERSION_V1, 1);
        // Structural independence: no Runtime Client protocol type carries
        // a `schema_version` field, and serialized requests never embed it.
        let request = RuntimeClientRequest::Initialize {
            id: super::RequestId::new(1),
            protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        };
        let value = serde_json::to_value(request).expect("serialize request");
        assert!(value.get("schema_version").is_none());
        assert!(value.get("event_schema_version").is_none());
    }

    /// Requests serialize deterministically with their method discriminator
    /// and round-trip exactly.
    #[test]
    fn requests_round_trip_deterministically() {
        let request = RuntimeClientRequest::Initialize {
            id: super::RequestId::new(7),
            protocol_version: 1,
        };
        let first = serde_json::to_string(&request).expect("serialize");
        let second = serde_json::to_string(&request).expect("serialize again");
        assert_eq!(first, second);
        let value: serde_json::Value = serde_json::from_str(&first).expect("parse");
        assert_eq!(value["method"], "initialize");
        assert_eq!(value["id"], 7);
        let decoded: RuntimeClientRequest =
            serde_json::from_str(&first).expect("deserialize request");
        assert_eq!(decoded, request);

        let request = RuntimeClientRequest::SubmitInbound {
            id: super::RequestId::new(9),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "hello".to_owned(),
            })],
        };
        let value = serde_json::to_value(&request).expect("serialize");
        assert_eq!(value["method"], "submit_inbound");
        let decoded: RuntimeClientRequest =
            serde_json::from_value(value).expect("deserialize request");
        assert_eq!(decoded, request);
    }

    /// Unknown methods and unknown request fields are rejected explicitly.
    #[test]
    fn unknown_methods_and_fields_are_rejected() {
        let unknown_method = r#"{"method": "future_method", "id": 1}"#;
        assert!(serde_json::from_str::<RuntimeClientRequest>(unknown_method).is_err());
        let unknown_field = r#"{"method": "snapshot_get", "id": 1, "extra": true}"#;
        assert!(serde_json::from_str::<RuntimeClientRequest>(unknown_field).is_err());
    }

    /// Responses echo the request id and carry exactly one of result/error.
    #[test]
    fn responses_correlate_request_ids() {
        let response = RuntimeClientResponse {
            id: super::RequestId::new(42),
            result: Some(RuntimeClientResult::Detached),
            error: None,
        };
        let value = serde_json::to_value(&response).expect("serialize");
        assert_eq!(value["id"], 42);
        assert!(value.get("error").is_none());
        let decoded: RuntimeClientResponse = serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded, response);
    }

    /// Notifications structurally carry no request id: an event envelope
    /// serializes as cursor + typed payload only.
    #[test]
    fn notifications_never_fabricate_request_ids() {
        let notification = RuntimeClientProtocolEvent {
            cursor: RuntimeClientCursor::new(5),
            event: RuntimeClientEvent::AttemptStarted {
                attempt_id: AttemptId::new("attempt-1"),
                model: Box::new(attempt_model_view("acme/model-a")),
            },
        };
        let value = serde_json::to_value(&notification).expect("serialize");
        assert!(value.get("id").is_none());
        assert_eq!(value["cursor"], 5);
        assert_eq!(value["event"]["type"], "attempt_started");
        let decoded: RuntimeClientProtocolEvent =
            serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded, notification);
    }

    /// `attempt_started` is self-contained: the frozen attempt model travels
    /// with the event, so a continuously subscribed client never has to
    /// infer the active attempt's model or take a second snapshot.
    #[test]
    fn attempt_started_carries_the_frozen_attempt_model() {
        let event = RuntimeClientEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-a"),
            model: Box::new(attempt_model_view("acme/model-a")),
        };
        let value = serde_json::to_value(&event).expect("serialize");
        assert_eq!(value["type"], "attempt_started");
        assert_eq!(value["model"]["primary"]["model"], "acme/model-a");
        assert_eq!(value["model"]["summary"]["mode"], "session");
        let decoded: RuntimeClientEvent = serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded, event);
    }

    /// The redacted attempt model view of one model reference.
    fn attempt_model_view(reference: &str) -> crate::model::session::AttemptModelView {
        use crate::model::catalog::{ModelCapabilities, ModelRef};
        use crate::model::invocation::{ModelInvocationView, RequestParams};
        use crate::model::session::{AttemptModelView, SummaryModelView};
        use crate::model::types::ModelProtocol;

        let capabilities = ModelCapabilities::text_only(true, false);
        AttemptModelView {
            primary: ModelInvocationView {
                model: ModelRef::parse(reference).expect("a valid model reference"),
                protocol: ModelProtocol::OpenAiChatCompletions,
                context_window: 128_000,
                model_max_output_tokens: 4096,
                max_output_tokens: 4096,
                reasoning_profile: None,
                reasoning_enabled: false,
                request_params: RequestParams::new(),
                capabilities: capabilities.clone(),
                declared_capabilities: capabilities,
            },
            summary: SummaryModelView::Session,
        }
    }

    /// Typed errors carry structured categories, never free-form strings
    /// only, and round-trip exactly.
    #[test]
    fn typed_errors_round_trip() {
        let cases = [
            RuntimeClientError::UnsupportedProtocolVersion {
                supported: 1,
                requested: 9,
            },
            RuntimeClientError::NoCurrentAttempt,
            RuntimeClientError::UnknownBackgroundExecution {
                execution_id: ToolExecutionId::new("exec_1"),
            },
            RuntimeClientError::ResyncRequired {
                after_cursor: RuntimeClientCursor::new(3),
                earliest_serviceable: RuntimeClientCursor::new(100),
            },
            RuntimeClientError::RuntimeShutdown,
        ];
        for error in cases {
            let value = serde_json::to_value(&error).expect("serialize");
            let decoded: RuntimeClientError = serde_json::from_value(value).expect("deserialize");
            assert_eq!(decoded, error);
        }
    }

    /// The cursor is a distinct newtype: it is not `u64`, not the mailbox
    /// `InboundSequence`, and not the internal event sequence.
    #[test]
    fn cursor_is_a_distinct_domain() {
        let cursor = RuntimeClientCursor::new(7);
        assert_eq!(cursor.get(), 7);
        assert_eq!(cursor.to_string(), "7");
        let value = serde_json::to_value(cursor).expect("serialize");
        assert_eq!(value, serde_json::json!(7));
    }
}
