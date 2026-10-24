//! Deterministic Rust-produced wire examples for schema/client conformance.
/// Rust-derived public schema with App Server lossless numeric wire rules.
#[must_use]
pub fn protocol_schema() -> serde_json::Value {
    super::wire::protocol_schema()
}
use super::protocol::{
    APP_SERVER_PROTOCOL_VERSION, AttachmentTarget, ClientIdentity, Failure, InitializeParams,
    JsonRpcVersion, Method, MethodResult, Notification, NotificationMethod,
    PresentationCapabilities, ProtocolMessage, Request, RequestId, Response, RpcError,
    ServerCapabilities, Success,
};

#[must_use]
#[allow(clippy::too_many_lines)] // Typed fixture constructors, not protocol logic.
/// Construct canonical wire fixtures without environment-dependent values.
/// # Panics
/// Panics if a constant fixture violates its native value constructor.
pub fn fixtures() -> Vec<ProtocolMessage> {
    const EXACT: u64 = 9_007_199_254_740_993;
    use crate::events::interaction::{
        ExactInteger, FiniteNumber, IntegerAnswer, NumberAnswer, QuestionnaireAnswer,
        QuestionnaireAnswerEntry, QuestionnaireResponse, QuestionnaireSubmission,
    };
    use crate::runtime::identity::{ConversationId, InteractionId};
    let target = AttachmentTarget {
        session_id: crate::local_runtime::session::SessionId::new(
            "ses_00000000-0000-7000-8000-000000000001",
        ),
        conversation_id: ConversationId::new("conv_00000000-0000-7000-8000-000000000001"),
        runtime_incarnation: serde_json::from_str("9007199254740993").expect("incarnation fixture"),
        attachment_id: crate::runtime_client::types::AttachmentId::new("attachment-fixture"),
    };
    let reading_cut = crate::durable::reading::ConversationReadCut {
        conversation_id: target.conversation_id.clone(),
        journal: EXACT,
        transcript: EXACT,
        mutation_revision: EXACT,
        ledger_through: 0,
        surface_revision: crate::conversation::SurfaceRevision::INITIAL,
        transcript_total: 0,
        pending_through: 0,
        pending_total: 0,
    };
    let reading_id = crate::observation::turn::ObservedTurnIdentity {
        conversation_id: target.conversation_id.clone(),
        attempt_id: Some(crate::runtime::identity::AttemptId::new("attempt-fixture")),
        attempt_id_source: None,
    };
    let mut fixtures = vec![
        ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("exact-node".into()),
            call: Method::SessionNode {
                session_id: target.session_id.clone(),
                node_id: crate::local_runtime::session::SessionNodeId::new(
                    "node_00000000-0000-7000-8000-000000000001",
                ),
            },
        })),
        ProtocolMessage::Response(Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("exact-node".into()),
            result: MethodResult::Node {
                node: crate::local_runtime::session::observation::SessionNodeObservation {
                    id: crate::local_runtime::session::SessionNodeId::new(
                        "node_00000000-0000-7000-8000-000000000001",
                    ),
                    conversation_id: target.conversation_id.clone(),
                    ordinal: EXACT,
                    parent: None,
                    origin: crate::local_runtime::session::observation::SessionNodeOriginObservation::New,
                },
            },
        }))),
        ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("manual-compaction-correlation".into()),
            call: Method::CompactContext {
                target: target.clone(),
                request_id: "550e8400-e29b-41d4-a716-446655440000"
                    .to_owned()
                    .try_into()
                    .expect("bounded UUID correlation"),
            },
        })),
        ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("create-with-initial-model".into()),
            call: Method::SessionCreate {
                settings: crate::local_runtime::session::SessionPersistentState {
                    cwd: "/workspace".into(),
                    model: Some(crate::model::session::SessionModelConfig::of(
                        crate::model::catalog::ModelRef::parse("provider/selected").unwrap(),
                    )),
                },
            },
        })),
        ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("create-with-native-default".into()),
            call: Method::SessionCreate {
                settings: crate::local_runtime::session::SessionPersistentState {
                    cwd: "/workspace".into(),
                    model: None,
                },
            },
        })),
        ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("exact-session-summary".into()),
            call: Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        })),
        ProtocolMessage::Response(Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("exact-session-summary".into()),
            result: MethodResult::SessionSummary {
                summary: crate::local_runtime::session::SessionSummary {
                    ownership_generation: "1".into(),
                    id: target.session_id.clone(),
                    cwd: "/workspace".into(),
                    name: None,
                    generated_title: None,
                    preview: Some("Native first user message".into()),
                    updated_at: chrono::DateTime::from_timestamp(0, 0).expect("fixture date"),
                    active_node: crate::local_runtime::session::SessionNodeId::new(
                        "node_00000000-0000-7000-8000-000000000001",
                    ),
                },
            },
        }))),
        ProtocolMessage::Response(Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(291),
            result: MethodResult::Diagnostics {
                snapshot: crate::app_server::host::ServerDiagnostics {
                    lifecycle: crate::app_server::host::ServerLifecycle::Accepting,
                    policy: crate::local_runtime::app_server_policy::AppServerPolicy::default(),
                    loaded: 0,
                    loading: 0,
                    unloading: 0,
                    active_roots: 0,
                    external_attachments: 0,
                    sessions: Vec::new(),
                    admission_refusals: std::collections::BTreeMap::default(),
                    unload_failures: 0,
                    shutdown_failures: 0,
                    shutdown_timeouts: 0,
                    transport: super::transport::resources::TransportResources::default()
                        .snapshot(),
                },
            },
        }))),
        ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(291),
            call: Method::ServerDiagnostics {},
        })),
        ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("initialize-fixture".into()),
            call: Method::Initialize(InitializeParams {
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                client: ClientIdentity {
                    name: "fixture-client".into(),
                    version: "1".into(),
                },
                presentation: PresentationCapabilities::default(),
            }),
        })),
        ProtocolMessage::Response(Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(7),
            result: MethodResult::Initialized {
                authority_id: "fixture-app-server-authority".into(),
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                capabilities: ServerCapabilities::default(),
            },
        }))),
        ProtocolMessage::Response(Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("tool-without-execution".into()),
            result: MethodResult::TraceToolLocation { location: None },
        }))),
        ProtocolMessage::Response(Response::Failure(Failure {
            jsonrpc: JsonRpcVersion::V2,
            id: None,
            error: RpcError {
                code: -32700,
                message: "Parse error".into(),
                data: None,
            },
        })),
        ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(-3),
            call: Method::InteractionRespond {
                target: target.clone(),
                interaction: crate::runtime::interaction::InteractionRef {
                    conversation_id: target.conversation_id.clone(),
                    interaction_id: InteractionId::new("interaction-fixture"),
                },
                response: crate::runtime::interaction::InteractionResponse::Questionnaire {
                    response: QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                        answers: vec![
                            QuestionnaireAnswerEntry {
                                question_index: 0,
                                answer: QuestionnaireAnswer::Integer(IntegerAnswer {
                                    value: ExactInteger::new(9_007_199_254_740_993),
                                }),
                            },
                            QuestionnaireAnswerEntry {
                                question_index: 1,
                                answer: QuestionnaireAnswer::Number(NumberAnswer {
                                    value: FiniteNumber::try_new(1.25).expect("finite"),
                                }),
                            },
                        ],
                    }),
                },
            },
        })),
        ProtocolMessage::Notification(Notification {
            jsonrpc: JsonRpcVersion::V2,
            notification: NotificationMethod::Event {
                registration_id: "registration_fixture".into(),
                target: target.clone(),
                cursor: crate::runtime_client::RuntimeClientCursor::new(9_007_199_254_740_993),
                event: Box::new(crate::runtime_client::RuntimeClientEvent::AttemptStarted {
                    attempt_id: crate::runtime::identity::AttemptId::new("attempt-fixture"),
                    model: None,
                    execution_settings: None,
                }),
            },
        }),
        ProtocolMessage::Notification(Notification {
            jsonrpc: JsonRpcVersion::V2,
            notification: NotificationMethod::SummaryInvalidated {
                session_id: target.session_id.clone(),
                catalog_changed: false,
            },
        }),
        ProtocolMessage::Notification(Notification {
            jsonrpc: JsonRpcVersion::V2,
            notification: NotificationMethod::SummaryInvalidated {
                session_id: target.session_id.clone(),
                catalog_changed: true,
            },
        }),
        ProtocolMessage::Notification(Notification {
            jsonrpc: JsonRpcVersion::V2,
            notification: NotificationMethod::Closed {
                target: target.clone(),
            },
        }),
        ProtocolMessage::Response(Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("read".into()),
            result: MethodResult::Session {
                session: crate::local_runtime::session::SessionSnapshot {
                    id: crate::local_runtime::session::SessionId::new(
                        "ses_00000000-0000-7000-8000-000000000001",
                    ),
                    name: None,
                    generated_title: None,
                    created_at: chrono::DateTime::from_timestamp(0, 0).expect("epoch"),
                    updated_at: chrono::DateTime::from_timestamp(1, 0).expect("epoch"),
                    active_node: crate::local_runtime::session::SessionNodeId::new(
                        "node_00000000-0000-7000-8000-000000000001",
                    ),
                    active_conversation_id: ConversationId::new(
                        "conv_00000000-0000-7000-8000-000000000001",
                    ),
                    node_count: 1,
                },
            },
        }))),
    ];
    // The same serialized bounded Tool contract is consumed by Web and TUI.
    fixtures.push(
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "id": "trace-tool-summary", "result": {
                "type": "trace", "page": { "records": [{
                    "id": "trace:7", "position": "trace:7", "fields": [], "collections": [],
                    "location": { "attempt_id": "attempt-a", "step_id": "1" },
                    "kind": "tool", "state": "completed",
                    "timing": { "started_at": "2026-09-15T00:00:00Z" },
                    "preview": { "text": "bounded result", "truncated": true },
                    "request": null,
                    "tool": { "call_id": "reused-call", "tool_id": "tool-bash", "name": "bash", "fields": [],
                        "arguments": { "text": "{\"command\":\"bounded input", "truncated": true },
                        "started": true, "outcome": "success", "detail": null },
                    "calls": [], "attachments": [], "has_detail": true, "truncated": false
                }], "next_cursor": null }
            }
        }))
        .expect("bounded Tool protocol fixture"),
    );
    for call in vec![
        Method::ArtifactRead {
            target: target.clone(),
            artifact_id: crate::runtime::identity::ArtifactId::new("artifact_1"),
        },
        Method::SessionUploadStatus {
            target: target.clone(),
            operation_id: "0".repeat(32),
        },
        Method::SessionUploadPrepare {
            operation_id: "0".repeat(32),
            target: target.clone(),
            files: vec![crate::local_runtime::session::uploads::UploadMetadata {
                name: "hello.txt".into(),
                size: 2,
            }],
        },
        Method::SessionSubscribe {
            target: target.clone(),
            after_cursor: crate::runtime_client::RuntimeClientCursor::new(EXACT),
        },
        Method::Trace {
            records: Vec::new(),
            target: target.clone(),
            before: Some(
                serde_json::from_str("\"trace:9007199254740993\"").expect("Trace cursor fixture"),
            ),
            limit: 32,
        },
        Method::TraceLocateTool {
            target: target.clone(),
            locator: crate::runtime_client::trace::TraceToolLocator {
                occurrence: crate::observation::tool::RuntimeClientToolOccurrence {
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                        message_id: crate::runtime::identity::MessageId::new("assistant-fixture"),
                    },
                    block_index: crate::message::types::ContentBlockIndex::new(2),
                },
                call_id: Some(crate::runtime::identity::ToolCallId::new("call-fixture")),
                tool_id: Some(crate::runtime::identity::ToolId::new("tool-bash")),
            },
        },
        Method::AgentTraceLocateTool {
            target: target.clone(),
            agent_id: crate::runtime::identity::AgentId::new("agent-fixture"),
            locator: crate::runtime_client::trace::TraceToolLocator {
                occurrence: crate::observation::tool::RuntimeClientToolOccurrence {
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                        message_id: crate::runtime::identity::MessageId::new("assistant-fixture"),
                    },
                    block_index: crate::message::types::ContentBlockIndex::new(2),
                },
                call_id: Some(crate::runtime::identity::ToolCallId::new("call-fixture")),
                tool_id: Some(crate::runtime::identity::ToolId::new("tool-bash")),
            },
        },
        Method::Transcript {
            target: target.clone(),
            at: crate::durable::reading::ConversationWindowAt::Older {
                cut: None,
                before: crate::durable::TranscriptCursor::new(EXACT),
            },
            limit: 32,
        },
        Method::Transcript {
            target: target.clone(),
            at: crate::durable::reading::ConversationWindowAt::Message {
                id: crate::runtime::identity::MessageId::new("source-message-fixture"),
                cut: None,
            },
            limit: 32,
        },
        Method::ModelSelect {
            target: target.clone(),
            choice: crate::model::session::CatalogModelSelection {
                binding_revision: EXACT,
                model_position: 0,
                profile_position: None,
            },
        },
        Method::ConversationTurns {
            target: target.clone(),
            offset: Some(64),
            limit: 32,
        },
        Method::SessionFork {
            session_id: target.session_id.clone(),
            node_id: None,
            surface_revision: crate::conversation::surface::SurfaceRevision::new(EXACT),
            boundary: None,
            side: crate::local_runtime::session::LineageSide::Before,
        },
        Method::InboundEdit {
            target: target.clone(),
            expected: crate::durable::inbox::PendingInboundRef {
                sequence: crate::runtime::inbound::InboundSequence::new(EXACT),
                revision: EXACT,
            },
            text: "edited pending input".into(),
        },
        Method::InboundRemove {
            target: target.clone(),
            expected: crate::durable::inbox::PendingInboundRef {
                sequence: crate::runtime::inbound::InboundSequence::new(EXACT),
                revision: EXACT,
            },
        },
        Method::AgentStatistics {
            target: target.clone(),
            agent_id: crate::runtime::identity::AgentId::new("agent-fixture"),
        },
        Method::AgentTrace {
            target: target.clone(),
            agent_id: crate::runtime::identity::AgentId::new("agent-fixture"),
            before: None,
            limit: 32,
            records: Vec::new(),
        },
        Method::AgentTraceDetail {
            target: target.clone(),
            agent_id: crate::runtime::identity::AgentId::new("agent-fixture"),
            record_id: "trace:1".into(),
        },
        Method::AgentTranscript {
            target: target.clone(),
            agent_id: crate::runtime::identity::AgentId::new("agent-fixture"),
            at: crate::durable::reading::ConversationWindowAt::Older {
                before: crate::durable::TranscriptCursor::new(EXACT),
                cut: None,
            },
            limit: 32,
        },
        Method::AgentTurns {
            target: target.clone(),
            agent_id: crate::runtime::identity::AgentId::new("agent-fixture"),
            offset: None,
            limit: 32,
        },
        Method::Goal {
            target: target.clone(),
            control: crate::goal::GoalControl::Mutate {
                expected: crate::goal::GoalRef {
                    id: "goal-fixture".into(),
                    revision: EXACT,
                },
                mutation: crate::goal::GoalMutation::Pause,
            },
        },
    ] {
        fixtures.push(ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("exact-u64".into()),
            call,
        })));
    }
    for result in [
        MethodResult::UploadPrepared {
            transfer: super::upload_transfer::UploadDescriptor {
                path: format!("/session-upload/{}", "a".repeat(43)),
                loopback_port: None,
                expires_in_seconds: 60,
            },
        },
        MethodResult::UploadStatus {
            outcome: crate::local_runtime::session::uploads::UploadOutcome::Absent,
        },
        MethodResult::UploadStatus {
            outcome: crate::local_runtime::session::uploads::UploadOutcome::Unresolved,
        },
        MethodResult::UploadStatus {
            outcome: crate::local_runtime::session::uploads::UploadOutcome::Failed,
        },
        MethodResult::UploadStatus {
            outcome: crate::local_runtime::session::uploads::UploadOutcome::Ready {
                files: Vec::new(),
            },
        },
        MethodResult::SessionFileBytes {
            file: crate::tools::session_files::SessionFileReference {
                scope: crate::tools::session_files::SessionFileScope {
                    conversation_id: target.conversation_id.clone(),
                    device: "1".into(),
                    inode: "2".into(),
                },
                path: "报告 file.md".into(),
                name: "报告 file.md".into(),
                description: Some("Report".into()),
                mime_type: "text/markdown".into(),
            },
            data: "IyBSZXBvcnQK".into(),
        },
        MethodResult::DomainPage {
            page: crate::runtime_client::checkpoint::CheckpointPage {
                collections: Vec::new(),
                cursor: crate::runtime_client::types::RuntimeClientCursor::new(EXACT),
                read_cut: crate::observation::cut::CheckpointReadCut {
                    canonical_foreground: None,
                    live_publication: None,
                    collection_cuts: std::collections::BTreeMap::new(),
                    conversation_id: target.conversation_id.clone(),
                    activation_id: None,
                    revision: EXACT,
                    through: EXACT,
                    source_cut: None,
                    domain_revisions: std::collections::BTreeMap::default(),
                },
                domain: "jobs".into(),
                total: 0,
                items: Vec::new(),
                next_offset: None,
            },
        },
        MethodResult::AgentStatistics {
            metrics: crate::observation::statistics::AgentStatistics::default(),
        },
        MethodResult::InboundMutation {
            outcome: crate::durable::inbox::PendingMutationOutcome::Conflict,
        },
        MethodResult::ConfigurationReceipt {
            scope: "session-fixture".into(),
            version: EXACT,
        },
        MethodResult::ConversationTurns {
            page: crate::observation::turn::ObservedTurnPage {
                cut: reading_cut,
                total: 1,
                offset: 0,
                turns: vec![crate::observation::turn::ObservedTurn {
                    id: reading_id,
                    ordinal: 1,
                    cursor: Some(crate::durable::TranscriptCursor::new(EXACT)),
                    prompt: "Native prompt".into(),
                    response: "Native response".into(),
                }],
            },
        },
        MethodResult::TranscriptWindow {
            window: crate::runtime_client::snapshot::ConversationWindow {
                cut: crate::durable::reading::ConversationReadCut {
                    conversation_id: target.conversation_id.clone(),
                    journal: 0,
                    transcript: 0,
                    mutation_revision: 0,
                    ledger_through: 0,
                    surface_revision: crate::conversation::SurfaceRevision::INITIAL,
                    transcript_total: 0,
                    pending_through: 0,
                    pending_total: 0,
                },
                newer_cursor: None,
                target: None,
                target_cursor: None,
                page: crate::observation::transcript::RuntimeClientTranscriptPage {
                    source_cut: None,
                    content: Vec::new(),
                    collections: Vec::new(),
                    deferred_entries: Vec::new(),
                    inherited_through: Some(
                        crate::observation::transcript::RuntimeClientTranscriptCursor::new(0),
                    ),
                    entries: Vec::new(),
                    next_cursor: None,
                    statistics: None,
                },
            },
        },
        MethodResult::InboundAccepted {
            message_id: crate::runtime::identity::MessageId::new("message-fixture"),
            inbound_sequence: crate::runtime::inbound::InboundSequence::new(EXACT),
        },
    ] {
        fixtures.push(ProtocolMessage::Response(Response::Success(Box::new(
            Success {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::String("exact-u64".into()),
                result,
            },
        ))));
    }
    fixtures.push(ProtocolMessage::Response(Response::Failure(Failure {
        jsonrpc: JsonRpcVersion::V2,
        id: Some(RequestId::String("job-publication-failed".into())),
        error: RpcError {
            code: -32000,
            message: "Job terminal publication was abandoned".into(),
            data: Some(super::protocol::ErrorData::JobPublicationAbandoned {
                job_id: crate::runtime::identity::ToolExecutionId::new(
                    "exec_0199c989-03a0-7000-8000-000000000001",
                ),
            }),
        },
    })));
    let run = crate::runtime::workflow::WorkflowRunId {
        conversation_id: target.conversation_id.clone(),
        origin: (crate::runtime::identity::AttemptId::new("attempt-fixture")).into(),
        invocation: EXACT,
    };
    fixtures.push(ProtocolMessage::Response(Response::Failure(Failure {
        jsonrpc: JsonRpcVersion::V2,
        id: Some(RequestId::String("stale-settings".into())),
        error: RpcError {
            code: -32000,
            message: "Stale settings".into(),
            data: Some(super::protocol::ErrorData::StaleSettings {
                expected: EXACT,
                actual: EXACT + 1,
            }),
        },
    })));
    fixtures.push(ProtocolMessage::Notification(Notification {
        jsonrpc: JsonRpcVersion::V2,
        notification: NotificationMethod::Event {
            registration_id: "registration_fixture".into(),
            target,
            cursor: crate::runtime_client::RuntimeClientCursor::new(EXACT),
            event: Box::new(
                crate::runtime_client::RuntimeClientEvent::WorkflowsUpdated {
                    workflows: crate::runtime::workflow::read_model::WorkflowSnapshot {
                        revision: crate::runtime::workflow::read_model::WorkflowRevision(EXACT),
                        runs: vec![crate::runtime::workflow::read_model::WorkflowRunView {
                            id: run.clone(),
                            workflow_id: crate::runtime::WorkflowId::parse("workflow-fixture")
                                .expect("workflow fixture"),
                            program_digest: "digest-fixture".into(),
                            resource_revision:
                                crate::runtime::identity::RuntimeResourceRevision::new(EXACT),
                            tool_call_id: Some(crate::runtime::identity::ToolCallId::new(
                                "call-fixture",
                            )),
                            state: crate::runtime::workflow::read_model::WorkflowState::Running,
                            instances: Vec::new(),
                            omitted_instances: 0,
                            steps_consumed: 1,
                            steps_max: 10,
                            agents_consumed: 0,
                            model_calls_consumed: 0,
                        }],
                        omitted_runs: 0,
                    },
                },
            ),
        },
    }));
    for data in [
        super::protocol::ErrorData::SessionFileRead {
            reason: crate::tools::session_files::SessionFileReadFailure::Unavailable,
        },
        super::protocol::ErrorData::UnknownAgent {
            agent_id: crate::runtime::identity::AgentId::new("agent-fixture"),
        },
        super::protocol::ErrorData::AgentHistoryUnavailable {
            agent_id: crate::runtime::identity::AgentId::new("agent-fixture"),
        },
        super::protocol::ErrorData::ResidencyCapacity,
        super::protocol::ErrorData::AttachmentCapacity,
        super::protocol::ErrorData::RequestCapacity,
        super::protocol::ErrorData::ServerDraining,
    ] {
        fixtures.push(ProtocolMessage::Response(Response::Failure(Failure {
            jsonrpc: JsonRpcVersion::V2,
            id: Some(RequestId::Integer(291)),
            error: RpcError {
                code: -32000,
                message: "Operation rejected".into(),
                data: Some(data),
            },
        })));
    }
    fixtures.push(ProtocolMessage::Request(Box::new(Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::String("archive-fixture".into()),
        call: Method::SessionExportPrepare {
            session_id: crate::local_runtime::session::SessionId::new(
                "ses_00000000-0000-7000-8000-000000000001",
            ),
        },
    })));
    fixtures.push(ProtocolMessage::Response(Response::Success(Box::new(
        Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("archive-fixture".into()),
            result: MethodResult::SessionArchive {
                download: super::archive_download::ArchiveDownloadDescriptor {
                    path: format!("/session-archive/{}", "a".repeat(43)),
                    filename: "rustx-session-fixture.zip".into(),
                    expires_in_seconds: 60,
                    loopback_port: None,
                },
            },
        },
    ))));
    for reason in [
        crate::session_archive::SessionArchivePrepareError::DescendantUnavailable,
        crate::session_archive::SessionArchivePrepareError::ArtifactUnavailable,
    ] {
        fixtures.push(ProtocolMessage::Response(Response::Failure(Failure {
            jsonrpc: JsonRpcVersion::V2,
            id: Some(RequestId::String("archive-failure-fixture".into())),
            error: RpcError {
                code: -32000,
                message: reason.to_string(),
                data: Some(super::protocol::ErrorData::ArchivePreparationFailed { reason }),
            },
        })));
    }
    workflow_fixtures(&mut fixtures);
    schedule_fixtures(&mut fixtures);
    fixtures
}

/// The six native Workflow examples as exact scoped authoring messages
/// (Issue #491): parsed, compiled and explained by the native owner, never
/// by a client.
#[allow(clippy::too_many_lines)] // Typed fixture constructors, not protocol logic.
fn workflow_fixtures(fixtures: &mut Vec<ProtocolMessage>) {
    use crate::local_runtime::configuration::settings::{SourceMutation, SourceTarget};
    use crate::local_runtime::workflow_authoring::{
        WorkflowCheckReport, WorkflowDependencies, WorkflowDiagnostic, WorkflowEdit,
        WorkflowFileSummary, WorkflowLibrary, WorkflowSource, WorkflowStructure,
        WorkflowVocabulary,
    };
    use crate::runtime::workflow::{WorkflowDefinition, WorkflowId, WorkflowProgram};
    use sha2::Digest;
    const EXAMPLES: [(&str, &str); 8] = [
        (
            "typed_agent",
            include_str!(
                "../../examples/local-runtime/workflow-templates/.agents/workflows/typed_agent.yaml"
            ),
        ),
        (
            "specialized_agent",
            include_str!(
                "../../examples/local-runtime/workflow-templates/.agents/workflows/specialized_agent.yaml"
            ),
        ),
        (
            "parallel_checks",
            include_str!(
                "../../examples/local-runtime/workflow-templates/.agents/workflows/parallel_checks.yaml"
            ),
        ),
        (
            "human_plan",
            include_str!(
                "../../examples/local-runtime/workflow-templates/.agents/workflows/human_plan.yaml"
            ),
        ),
        (
            "parallel_review",
            include_str!("../../examples/local-runtime/.agents/workflows/parallel_review.yaml"),
        ),
        (
            "implement_and_review",
            include_str!(
                "../../examples/local-runtime/.agents/workflows/implement_and_review.yaml"
            ),
        ),
        (
            "model_text",
            include_str!(
                "../../examples/local-runtime/workflow-templates/.agents/workflows/model_text.yaml"
            ),
        ),
        (
            "model_json",
            include_str!(
                "../../examples/local-runtime/workflow-templates/.agents/workflows/model_json.yaml"
            ),
        ),
    ];
    let directory = std::path::PathBuf::from("/workspace/fixture/.agents/workflows");
    let target = SourceTarget::Workspace {
        directory: "/workspace/fixture".into(),
    };
    let success = |id: &str, result: MethodResult| {
        ProtocolMessage::Response(Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String(id.into()),
            result,
        })))
    };
    let request = |id: &str, call: Method| {
        ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String(id.into()),
            call,
        }))
    };
    let examples: Vec<_> = EXAMPLES
        .iter()
        .map(|(name, yaml)| {
            let definition: WorkflowDefinition =
                serde_yaml::from_str(yaml).expect("native example parses");
            let revision = format!("sha256:{:x}", sha2::Sha256::digest(yaml.as_bytes()));
            (
                WorkflowId::parse(name).expect("example identity"),
                definition,
                revision,
                yaml.len(),
            )
        })
        .collect();
    fixtures.push(request(
        "workflow-list",
        Method::WorkflowList {
            target: target.clone(),
        },
    ));
    fixtures.push(request(
        "workflow-model-choices",
        Method::WorkflowModelChoices {
            target: target.clone(),
            query: crate::local_runtime::workflow_authoring::WorkflowModelChoicesQuery::Models {
                offset: 0,
            },
            expected_revision: None,
        },
    ));
    fixtures.push(success(
        "workflow-model-choices",
        MethodResult::WorkflowModelChoices {
            choices: crate::local_runtime::workflow_authoring::WorkflowModelChoices::Models {
                revision: "source-model-cut".into(),
                models: vec![
                    crate::model::ModelRef::parse("fixture/model").expect("model fixture"),
                ],
                next_offset: None,
            },
        },
    ));
    fixtures.push(success(
        "workflow-list",
        MethodResult::WorkflowLibrary {
            library: WorkflowLibrary {
                target: target.clone(),
                directory: directory.clone(),
                absent_revision: "missing".into(),
                files: examples
                    .iter()
                    .map(|(id, definition, revision, length)| WorkflowFileSummary {
                        file_name: format!("{id}.yaml"),
                        id: Some(id.clone()),
                        relative_path: format!(".agents/workflows/{id}.yaml"),
                        revision: revision.clone(),
                        byte_length: *length as u64,
                        modified_ms: Some(1_790_000_000_000),
                        description: Some(definition.description.clone()),
                        nodes: Some(definition.block.nodes.len()),
                        diagnostic: None,
                        shadows_global: false,
                    })
                    .collect(),
                diagnostic: None,
            },
        },
    ));
    for (id, definition, revision, _) in &examples {
        fixtures.push(success(
            &format!("workflow-source-{id}"),
            MethodResult::WorkflowSource {
                source: Box::new(WorkflowSource {
                    target: target.clone(),
                    id: id.clone(),
                    path: directory.join(format!("{id}.yaml")),
                    relative_path: format!(".agents/workflows/{id}.yaml"),
                    revision: revision.clone(),
                    present: true,
                    modified_ms: Some(1_790_000_000_000),
                    document: Some(Box::new(definition.clone())),
                    diagnostic: None,
                    vocabulary: WorkflowVocabulary {
                        profiles: vec![
                            crate::runtime::subagent::SubagentName::parse("reviewer")
                                .expect("profile fixture"),
                        ],
                        builtin_tools: crate::runtime::workflow::selectable_builtin_tools(),
                    },
                }),
            },
        ));
    }
    let (id, definition, revision, _) = &examples[4];
    let program =
        WorkflowProgram::compile(id.clone(), definition.clone()).expect("example compiles");
    fixtures.push(request(
        "workflow-check",
        Method::WorkflowCheck {
            target: target.clone(),
            id: id.clone(),
            document: Box::new(definition.clone()),
            context: None,
            explain: true,
        },
    ));
    // The wire omits the local report's runtime requirements.
    let mut explanation = program.inspect();
    explanation.runtime_requirements.clear();
    fixtures.push(success(
        "workflow-check",
        MethodResult::WorkflowCheck {
            report: Box::new(WorkflowCheckReport {
                structure: WorkflowStructure::Valid {
                    outputs: program.node_outputs(),
                    revision: revision.clone(),
                },
                dependencies: WorkflowDependencies::Admitted {
                    context: "/workspace/fixture".into(),
                },
                explanation: Some(Box::new(explanation)),
            }),
        },
    ));
    fixtures.push(request(
        "workflow-save",
        Method::SourcesWrite {
            target,
            expected_revision: revision.clone(),
            mutation: SourceMutation::Workflow {
                id: id.clone(),
                edit: WorkflowEdit::Replace {
                    document: Box::new(definition.clone()),
                },
            },
        },
    ));
    fixtures.push(ProtocolMessage::Response(Response::Failure(Failure {
        jsonrpc: JsonRpcVersion::V2,
        id: Some(RequestId::String("workflow-save".into())),
        error: RpcError {
            code: -32000,
            message: "invalid Workflow".into(),
            data: Some(super::protocol::ErrorData::WorkflowInvalid {
                diagnostic: Box::new(WorkflowDiagnostic {
                    path: "block.entry".into(),
                    category: "workflow_language".into(),
                    reason: "Workflow graph, control structure, or configured bound is invalid"
                        .into(),
                    correction: None,
                    detail: None,
                    line: None,
                    column: None,
                }),
            }),
        },
    })));
}

#[allow(clippy::too_many_lines)] // Native Scheduler wire examples include exact integer domains.
fn schedule_fixtures(fixtures: &mut Vec<ProtocolMessage>) {
    use crate::scheduler::{
        ScheduleBinding, ScheduleCommand, ScheduleId, ScheduleMutation, ScheduleSnapshot,
        ScheduleSpec, ScheduleState, ScheduleTiming,
    };
    let session_id =
        crate::runtime::identity::SessionId::new("ses_00000000-0000-7000-8000-000000000001");
    let node_id =
        crate::runtime::identity::SessionNodeId::new("node_00000000-0000-7000-8000-000000000001");
    let conversation_id =
        crate::runtime::identity::ConversationId::new("conv_00000000-0000-7000-8000-000000000001");
    let id = ScheduleId {
        conversation_id: conversation_id.clone(),
        ordinal: 9_007_199_254_740_993,
    };
    let due = chrono::DateTime::parse_from_rfc3339("2026-10-24T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let snapshot = ScheduleSnapshot {
        id: id.clone(),
        binding: ScheduleBinding {
            session_id: session_id.clone(),
            node_id: node_id.clone(),
            conversation_id,
        },
        revision: 9_007_199_254_740_993,
        title: "Native reminder".into(),
        target: crate::scheduler::ScheduleTarget::ConversationInbound {
            text: "Read this reminder separately".into(),
        },
        spec: ScheduleSpec::At { at_utc: due },
        state: ScheduleState::Enabled,
        next_due_utc: Some(due),
        last_trigger: None,
        completed_at: None,
        completion_reason: None,
        last_failure: None,
    };
    for (name, method) in [
        (
            "schedule-list",
            Method::ScheduleList {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
                after_ordinal: id.ordinal,
                limit: 100,
            },
        ),
        (
            "schedule-get",
            Method::ScheduleGet {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
                id: id.clone(),
            },
        ),
        (
            "schedule-command",
            Method::ScheduleCommand {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
                command: ScheduleCommand {
                    command_id: "stable-reminder-command".into(),
                    mutation: ScheduleMutation::Create {
                        title: "Native reminder".into(),
                        text: "Read this reminder separately".into(),
                        timing: ScheduleTiming::After { seconds: 600 },
                    },
                },
            },
        ),
        (
            "schedule-cas",
            Method::ScheduleCommand {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
                command: ScheduleCommand {
                    command_id: "stable-disable-command".into(),
                    mutation: ScheduleMutation::SetEnabled {
                        id,
                        expected_revision: 9_007_199_254_740_993,
                        enabled: false,
                    },
                },
            },
        ),
        (
            "schedule-recovery",
            Method::ScheduleRecoverStorage {
                session_id,
                node_id,
            },
        ),
    ] {
        fixtures.push(ProtocolMessage::Request(Box::new(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String(name.into()),
            call: method,
        })));
    }
    for (name, result) in [
        (
            "schedule-list",
            MethodResult::Schedules {
                schedules: vec![crate::scheduler::ScheduleHeader::from(snapshot.clone())],
                next_ordinal: Some(snapshot.id.ordinal),
            },
        ),
        (
            "schedule-get",
            MethodResult::Schedule {
                schedule: Some(snapshot),
            },
        ),
        (
            "schedule-command",
            MethodResult::ScheduleCommand {
                result: crate::scheduler::ScheduleCommandResult::Capacity,
            },
        ),
        (
            "schedule-recovery",
            MethodResult::ScheduleStorageRecovered {
                generation: 9_007_199_254_740_993,
            },
        ),
    ] {
        fixtures.push(ProtocolMessage::Response(Response::Success(Box::new(
            Success {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::String(name.into()),
                result,
            },
        ))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Closed scalar schema acceptance must equal authoritative serde acceptance;
    // every accepted spelling must serialize back identically (canonicality).
    fn scalar_agrees<T: serde::de::DeserializeOwned + serde::Serialize>(
        validator: &jsonschema::Validator,
        text: &str,
        expected: bool,
    ) {
        let wire = serde_json::json!(text);
        assert_eq!(validator.is_valid(&wire), expected, "schema: {text:?}");
        let parsed = serde_json::from_value::<T>(wire.clone());
        assert_eq!(parsed.is_ok(), expected, "serde: {text:?}");
        if let Ok(parsed) = parsed {
            assert_eq!(serde_json::to_value(parsed).unwrap(), wire);
        }
    }

    #[test]
    fn child_reading_has_only_exact_parent_agent_read_authority() {
        let requests = fixtures()
            .into_iter()
            .filter_map(|fixture| match fixture {
                super::ProtocolMessage::Request(request)
                    if matches!(
                        request.call,
                        super::Method::AgentTranscript { .. } | super::Method::AgentTurns { .. }
                    ) =>
                {
                    Some(*request)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        for request in requests {
            let value = serde_json::to_value(&request).unwrap();
            let validator = jsonschema::validator_for(&protocol_schema()).unwrap();
            assert!(validator.is_valid(&value));
            let mut arbitrary = value.clone();
            arbitrary["params"]["conversation_id"] =
                serde_json::json!("conv_00000000-0000-7000-8000-000000000002");
            assert!(serde_json::from_value::<super::Request>(arbitrary.clone()).is_err());
            assert!(!validator.is_valid(&arbitrary));
            for method in [
                "subagent/attach",
                "subagent/turnStart",
                "subagent/steer",
                "subagent/interactionRespond",
                "subagent/setModel",
            ] {
                let mut write = value.clone();
                write["method"] = serde_json::json!(method);
                assert!(serde_json::from_value::<super::Request>(write.clone()).is_err());
                assert!(!validator.is_valid(&write));
            }
        }
    }

    #[test]
    fn finite_number_scalar_schema_equals_serde_domain() {
        use crate::events::interaction::FiniteNumber;
        let schema = protocol_schema();
        let validator = jsonschema::validator_for(&schema["$defs"]["FiniteNumber"]).unwrap();
        for text in [
            "0000000000000000",
            "0000000000000001",
            "3ff0000000000000",
            "7fefffffffffffff",
            "ffefffffffffffff",
        ] {
            scalar_agrees::<FiniteNumber>(&validator, text, true);
            assert_eq!(FiniteNumber::from_wire(text).unwrap().to_wire(), text);
        }
        for text in [
            "7ff0000000000000",
            "fff0000000000000",
            "7ff8000000000000",
            "7fffffffffffffff",
            "fff8000000000000",
            "ffffffffffffffff",
            "8000000000000000",
            "3FF0000000000000",
            "3ff000000000000",
            "03ff0000000000000",
            "3ff0000000000000\n",
            "",
        ] {
            scalar_agrees::<FiniteNumber>(&validator, text, false);
            assert!(FiniteNumber::from_wire(text).is_err());
        }
        // Every sign/exponent combination, with zero, subnormal/payload, and
        // maximal mantissas, exercises the schema's exponent exclusion.
        for sign_exponent in 0_u64..4096 {
            for mantissa in [0, 1, (1_u64 << 52) - 1] {
                let text = format!("{:016x}", (sign_exponent << 52) | mantissa);
                scalar_agrees::<FiniteNumber>(
                    &validator,
                    &text,
                    FiniteNumber::from_wire(&text).is_ok(),
                );
            }
        }
    }

    #[test]
    fn exact_integer_scalar_schema_equals_serde_domain() {
        use crate::events::interaction::ExactInteger;
        let schema = protocol_schema();
        let validator = jsonschema::validator_for(&schema["$defs"]["ExactInteger"]).unwrap();
        for text in [
            "0",
            "1",
            "-1",
            "9007199254740993",
            "9223372036854775807",
            "-9223372036854775808",
        ] {
            scalar_agrees::<ExactInteger>(&validator, text, true);
            assert_eq!(ExactInteger::parse(text).unwrap().to_string(), text);
        }
        for text in [
            "9223372036854775808",
            "-9223372036854775809",
            "9999999999999999999",
            "-9999999999999999999",
            "+1",
            "01",
            "-01",
            "-0",
            "1.0",
            "1e3",
            "",
            "1\n",
            "0\n",
            " 1",
        ] {
            scalar_agrees::<ExactInteger>(&validator, text, false);
            assert!(ExactInteger::parse(text).is_err());
        }
        // Probe both sides of every decimal-prefix boundary in both signs.
        for bound in [i128::from(i64::MIN), i128::from(i64::MAX)] {
            for digits in 0..19 {
                let scale = 10_i128.pow(digits);
                for delta in -1..=1 {
                    let value = (bound / scale) * scale + delta;
                    scalar_agrees::<ExactInteger>(
                        &validator,
                        &value.to_string(),
                        i64::try_from(value).is_ok(),
                    );
                }
            }
        }
    }

    #[test]
    fn public_questionnaire_requests_reject_impossible_scalar_values() {
        let schema = protocol_schema();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let request = fixtures()
            .into_iter()
            .find_map(|fixture| match fixture {
                ProtocolMessage::Request(request)
                    if matches!(request.call, Method::InteractionRespond { .. }) =>
                {
                    Some(*request)
                }
                _ => None,
            })
            .unwrap();
        let wire = serde_json::to_value(request).unwrap();
        assert!(validator.is_valid(&wire));
        assert!(serde_json::from_value::<Request>(wire.clone()).is_ok());
        for (index, invalid) in [
            (0, "9223372036854775808"),
            (0, "-0"),
            (1, "7ff0000000000000"),
            (1, "8000000000000000"),
        ] {
            let mut wire = wire.clone();
            wire["params"]["response"]["response"]["value"]["answers"][index]["answer"]["value"]
                ["value"] = serde_json::json!(invalid);
            assert!(!validator.is_valid(&wire), "schema accepted {invalid}");
            assert!(
                serde_json::from_value::<Request>(wire).is_err(),
                "serde accepted {invalid}"
            );
        }
    }
    #[test]
    fn committed_rust_artifacts_are_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("protocol/app-server");
        let mut generations: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| {
                name.starts_with('v') && name.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
            })
            .collect();
        generations.sort();
        assert_eq!(generations, ["v57.schema.json", "v57.ts"]);
        assert_eq!(
            std::fs::read_to_string(root.join("v57.schema.json")).unwrap(),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&protocol_schema()).unwrap()
            )
        );
        assert_eq!(
            std::fs::read_to_string(root.join("fixtures.json")).unwrap(),
            format!("{}\n", serde_json::to_string_pretty(&fixtures()).unwrap())
        );
    }

    #[test]
    fn mandatory_tool_arguments_round_trip_with_truthful_unavailability() {
        let wire = fixtures()
            .into_iter()
            .map(|fixture| serde_json::to_value(fixture).unwrap())
            .find(|wire| wire["id"] == "trace-tool-summary")
            .unwrap();
        let arguments = &wire["result"]["page"]["records"][0]["tool"]["arguments"];
        assert!(arguments["text"].as_str().unwrap().len() <= 512);
        assert_eq!(arguments["truncated"], true);
        let decoded: ProtocolMessage = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
        let mut unavailable = wire;
        unavailable["result"]["page"]["records"][0]["tool"]["arguments"] = serde_json::Value::Null;
        let decoded: ProtocolMessage = serde_json::from_value(unavailable.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), unavailable);
    }

    #[test]
    fn generated_wire_fixtures_round_trip_and_validate() {
        let schema = protocol_schema();
        let validator = jsonschema::validator_for(&schema).unwrap();
        for fixture in fixtures() {
            let json = serde_json::to_value(&fixture).unwrap();
            let decoded: ProtocolMessage = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(fixture, decoded);
            assert!(
                validator.is_valid(&json),
                "schema rejected {json}: {:?}",
                validator.iter_errors(&json).collect::<Vec<_>>()
            );
        }
    }
}
