//! Deterministic Rust-produced wire examples for schema/client conformance.
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
    use crate::events::interaction::{
        ExactInteger, FiniteNumber, IntegerAnswer, NumberAnswer, QuestionnaireAnswer,
        QuestionnaireAnswerEntry, QuestionnaireResponse, QuestionnaireSubmission,
    };
    use crate::runtime::identity::{ConversationId, InteractionId};
    let target = AttachmentTarget {
        session_id: crate::local_runtime::session::SessionId::new("session-fixture"),
        conversation_id: ConversationId::new("conversation-fixture"),
        runtime_incarnation: serde_json::from_str("1").expect("incarnation fixture"),
        attachment_id: crate::runtime_client::types::AttachmentId::new("attachment-fixture"),
    };
    vec![
        ProtocolMessage::Request(Request {
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
        }),
        ProtocolMessage::Response(Response::Success(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(7),
            result: MethodResult::Initialized {
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                capabilities: ServerCapabilities::default(),
            },
        })),
        ProtocolMessage::Response(Response::Failure(Failure {
            jsonrpc: JsonRpcVersion::V2,
            id: None,
            error: RpcError {
                code: -32700,
                message: "Parse error".into(),
                data: None,
            },
        })),
        ProtocolMessage::Request(Request {
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
        }),
        ProtocolMessage::Notification(Notification {
            jsonrpc: JsonRpcVersion::V2,
            notification: NotificationMethod::Event {
                target: target.clone(),
                cursor: crate::runtime_client::RuntimeClientCursor::new(1),
                event: Box::new(crate::runtime_client::RuntimeClientEvent::AttemptStarted {
                    attempt_id: crate::runtime::identity::AttemptId::new("attempt-fixture"),
                    model: None,
                    execution_settings: None,
                }),
            },
        }),
        ProtocolMessage::Notification(Notification {
            jsonrpc: JsonRpcVersion::V2,
            notification: NotificationMethod::Closed { target },
        }),
        ProtocolMessage::Response(Response::Success(Success {
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
        })),
    ]
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
                serde_json::to_string_pretty(&schemars::schema_for!(ProtocolMessage)).unwrap()
            )
        );
        assert_eq!(
            std::fs::read_to_string(root.join("fixtures.json")).unwrap(),
            format!("{}\n", serde_json::to_string_pretty(&fixtures()).unwrap())
        );
    }

    #[test]
    fn generated_wire_fixtures_round_trip_and_validate() {
        let schema = serde_json::to_value(schemars::schema_for!(ProtocolMessage)).unwrap();
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
