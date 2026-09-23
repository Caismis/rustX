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
    let mut fixtures = vec![
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
                    id: target.session_id.clone(),
                    cwd: "/workspace".into(),
                    name: None,
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
                    id: crate::local_runtime::session::SessionId::new(
                        "ses_00000000-0000-7000-8000-000000000001",
                    ),
                    name: None,
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
    for call in [
        Method::ArtifactRead {
            target: target.clone(),
            artifact_id: crate::runtime::identity::ArtifactId::new("artifact_1"),
        },
        Method::SessionUpload {
            target: target.clone(),
            files: vec![super::protocol::UploadBytes {
                name: "hello.txt".into(),
                data: "aGk=".into(),
            }],
        },
        Method::SessionSubscribe {
            target: target.clone(),
            after_cursor: crate::runtime_client::RuntimeClientCursor::new(EXACT),
        },
        Method::Trace {
            target: target.clone(),
            before: Some(
                serde_json::from_str("\"trace:9007199254740993\"").expect("Trace cursor fixture"),
            ),
            limit: 32,
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
            side: crate::local_runtime::session::LineageSide::Before,
        },
        Method::InboundEdit {
            target: target.clone(),
            expected: crate::durable::inbox::PendingInboundRef {
                sequence: crate::runtime::inbound::InboundSequence::new(EXACT),
                message_id: crate::runtime::identity::MessageId::new("message-fixture"),
                revision: EXACT,
            },
            text: "edited pending input".into(),
        },
        Method::InboundRemove {
            target: target.clone(),
            expected: crate::durable::inbox::PendingInboundRef {
                sequence: crate::runtime::inbound::InboundSequence::new(EXACT),
                message_id: crate::runtime::identity::MessageId::new("message-fixture"),
                revision: EXACT,
            },
        },
        Method::SubagentTranscript {
            target: target.clone(),
            subagent_id: crate::runtime::identity::SubagentId::new("subagent-fixture"),
            before: Some(
                crate::runtime_client::snapshot::RuntimeClientTranscriptCursor::new(EXACT),
            ),
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
        MethodResult::InboundMutation {
            outcome: crate::durable::inbox::PendingMutationOutcome::Conflict,
        },
        MethodResult::ConfigurationApplication {
            application:
                crate::local_runtime::configuration::application::ConfigurationApplication {
                    scope: "session-fixture".into(),
                    // A Session application scope is a Session identity; its
                    // authored owners are named separately and never parsed
                    // out of that key.
                    sources: vec![
                        crate::local_runtime::configuration::settings::SourceTarget::User,
                        crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                            directory: "/workspace/fixture".into(),
                        },
                    ],
                    version: EXACT,
                    desired:
                        crate::local_runtime::configuration::application::ApplicationIdentity {
                            input_revision: Some("input".into()),
                            attempt: EXACT,
                        },
                    units: std::collections::BTreeMap::default(),
                    candidate: None,
                    eligibility: crate::local_runtime::configuration::application::AdoptionEligibility::Unavailable,
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
    for data in [
        super::protocol::ErrorData::UnknownSubagent {
            subagent_id: crate::runtime::identity::SubagentId::new("subagent-fixture"),
        },
        super::protocol::ErrorData::SubagentHistoryUnavailable {
            subagent_id: crate::runtime::identity::SubagentId::new("subagent-fixture"),
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
    fixtures
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
    fn child_transcript_has_only_exact_parent_subagent_read_authority() {
        let request = fixtures()
            .into_iter()
            .find_map(|fixture| match fixture {
                super::ProtocolMessage::Request(request)
                    if matches!(request.call, super::Method::SubagentTranscript { .. }) =>
                {
                    Some(*request)
                }
                _ => None,
            })
            .unwrap();
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
        assert_eq!(generations, ["v18.schema.json", "v18.ts"]);
        assert_eq!(
            std::fs::read_to_string(root.join("v18.schema.json")).unwrap(),
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
