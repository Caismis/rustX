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
        session_id: crate::local_runtime::session::SessionId::new("session-fixture"),
        conversation_id: ConversationId::new("conversation-fixture"),
        runtime_incarnation: serde_json::from_str("9007199254740993").expect("incarnation fixture"),
        attachment_id: crate::runtime_client::types::AttachmentId::new("attachment-fixture"),
    };
    let mut fixtures = vec![
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
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                capabilities: ServerCapabilities::default(),
            },
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
            notification: NotificationMethod::Closed {
                target: target.clone(),
            },
        }),
        ProtocolMessage::Response(Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::String("read".into()),
            result: MethodResult::Session {
                session: crate::local_runtime::session::SessionSnapshot {
                    id: crate::local_runtime::session::SessionId::new("session-fixture"),
                    name: None,
                    created_at: chrono::DateTime::from_timestamp(0, 0).expect("epoch"),
                    updated_at: chrono::DateTime::from_timestamp(1, 0).expect("epoch"),
                    active_node: crate::local_runtime::session::SessionNodeId::new("node-fixture"),
                    active_conversation_id: ConversationId::new("conversation-fixture"),
                    node_count: 1,
                },
            },
        }))),
    ];
    for call in [
        Method::SessionSubscribe {
            target: target.clone(),
            after_cursor: crate::runtime_client::RuntimeClientCursor::new(EXACT),
        },
        Method::Transcript {
            target: target.clone(),
            before: Some(
                crate::runtime_client::snapshot::RuntimeClientTranscriptCursor::new(EXACT),
            ),
            limit: 32,
        },
        Method::SessionFork {
            session_id: target.session_id.clone(),
            node_id: None,
            surface_revision: crate::conversation::surface::SurfaceRevision::new(EXACT),
            boundary: None,
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
        MethodResult::SettingsReplaced { revision: EXACT },
        MethodResult::ResourcesReloaded {
            resource_revision: EXACT,
            capability_revision: crate::runtime::identity::CapabilityRevision::new(EXACT),
        },
        MethodResult::InboundAccepted {
            message_id: crate::runtime::identity::MessageId::new("message-fixture"),
            inbound_sequence: crate::runtime::inbound::InboundSequence::new(EXACT),
        },
        MethodResult::ApprovalMode {
            effective_approval_mode: crate::runtime::ApprovalMode::Policy,
            pending_approval_mode: None,
            revision: EXACT,
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
    let run = crate::runtime::workflow::WorkflowRunId {
        conversation_id: target.conversation_id.clone(),
        attempt_id: crate::runtime::identity::AttemptId::new("attempt-fixture"),
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
                            tool_call_id: crate::runtime::identity::ToolCallId::new("call-fixture"),
                            state: crate::runtime::workflow::read_model::WorkflowState::Running,
                            instances: Vec::new(),
                            omitted_instances: 0,
                            steps_consumed: 1,
                            steps_max: 10,
                            agents_consumed: 0,
                            candidate: Some(crate::runtime::workspace::CandidateReference {
                                run,
                                version: EXACT,
                                content: "content-fixture".into(),
                            }),
                            candidate_users: 0,
                            handoff: None,
                        }],
                        omitted_runs: 0,
                    },
                },
            ),
        },
    }));
    fixtures
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn committed_rust_artifacts_are_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("protocol/app-server");
        assert_eq!(
            std::fs::read_to_string(root.join("v1.schema.json")).unwrap(),
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
