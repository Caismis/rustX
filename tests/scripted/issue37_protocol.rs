//! Issue #37/#130/#136: Runtime Client wire-contract tests.
//!
//! These tests exercise the protocol boundary exclusively through the
//! public Runtime Client surface: deterministic serialization of every
//! envelope, request-id correlation, notification structure, version
//! negotiation, and attachment lifecycle. No host-side race is asserted
//! here (the in-crate host tests own the synchronization proofs).

use super::support;

use rustx::runtime::identity::{AttemptId, ConversationId, InteractionId};
use rustx::runtime::interaction::{
    InteractionKind, InteractionOutcome, InteractionRequest, InteractionResponse,
    OptionSpecification, QuestionSpecification, QuestionnaireAnswer, QuestionnaireAnswerEntry,
    QuestionnaireResponse, QuestionnaireSpecification, QuestionnaireSubmission, SingleOptionAnswer,
};
use rustx::runtime_client::RuntimeClientHost;
use rustx::runtime_client::{
    RuntimeClientCursor, RuntimeClientError, RuntimeClientEvent, RuntimeClientProtocolEvent,
    RuntimeClientRequest, RuntimeClientResponse, RuntimeClientResult,
};

fn request_id(value: u64) -> rustx::runtime_client::RequestId {
    rustx::runtime_client::RequestId::new(value)
}

/// A host over an empty conversation: no adapter is ever invoked.
///
/// Construction is the shared Runtime Client fixture.
async fn host() -> RuntimeClientHost {
    support::runtime_client_fixture::RuntimeClientFixture::builder("conv-37-protocol")
        .build()
        .await
        .into_parts()
        .1
}

/// Every envelope kind serializes deterministically and round-trips
/// exactly: requests carry their method tag, responses echo request ids,
/// and events carry cursor + typed payload without a request id.
#[test]
fn protocol_envelopes_round_trip_deterministically() {
    let request = RuntimeClientRequest::SubmitInbound {
        id: request_id(5),
        content: vec![rustx::message::types::UserContentBlock::Text(
            rustx::message::content::TextBlock {
                text: "hello".to_owned(),
            },
        )],
    };
    let first = serde_json::to_string(&request).expect("serialize request");
    let second = serde_json::to_string(&request).expect("serialize request again");
    assert_eq!(first, second, "serialization is deterministic");
    let decoded: RuntimeClientRequest = serde_json::from_str(&first).expect("deserialize");
    assert_eq!(decoded, request);
    let value: serde_json::Value = serde_json::from_str(&first).expect("json");
    assert_eq!(value["method"], "submit_inbound");
    assert_eq!(value["id"], 5);

    let response = RuntimeClientResponse {
        id: request_id(5),
        result: Some(RuntimeClientResult::Detached),
        error: None,
    };
    let json = serde_json::to_string(&response).expect("serialize response");
    let decoded: RuntimeClientResponse = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, response);
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(value["id"], 5);

    let event = RuntimeClientProtocolEvent {
        cursor: RuntimeClientCursor::new(9),
        event: RuntimeClientEvent::AttemptStarted {
            attempt_id: rustx::runtime::identity::AttemptId::new("attempt-1"),
            model: Box::new(support::attempt_model_view("fixture/model-a")),
        },
    };
    let json = serde_json::to_string(&event).expect("serialize event");
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert!(
        value.get("id").is_none(),
        "notifications never fabricate request ids"
    );
    assert_eq!(value["cursor"], 9);
    // The start notification is self-contained: the frozen attempt model
    // travels with it, so an incremental client never infers it.
    assert_eq!(
        value["event"]["model"]["primary"]["model"],
        "fixture/model-a"
    );
    let decoded: RuntimeClientProtocolEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, event);
}

/// Every typed protocol error serializes with its stable category and
/// round-trips exactly.
#[test]
fn protocol_errors_round_trip_with_stable_categories() {
    let cases = [
        RuntimeClientError::UnsupportedProtocolVersion {
            supported: 10,
            requested: 4,
        },
        RuntimeClientError::AttachmentInUse {
            existing_attachment_id: rustx::runtime_client::AttachmentId::new("attachment-1"),
        },
        RuntimeClientError::NotAttached,
        RuntimeClientError::InvalidRequest {
            message: "empty content".to_owned(),
        },
        RuntimeClientError::NoCurrentAttempt,
        RuntimeClientError::UnknownBackgroundExecution {
            execution_id: rustx::runtime::identity::ToolExecutionId::new("exec_1"),
        },
        RuntimeClientError::ResyncRequired {
            after_cursor: RuntimeClientCursor::new(1),
            earliest_serviceable: RuntimeClientCursor::new(5),
        },
        RuntimeClientError::RuntimeShutdown,
        RuntimeClientError::InvalidState {
            message: "mailbox full".to_owned(),
        },
        RuntimeClientError::ProjectionExhausted,
        RuntimeClientError::RuntimeFailure {
            message: "boom".to_owned(),
        },
    ];
    for error in cases {
        let json = serde_json::to_string(&error).expect("serialize error");
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert!(value.get("type").is_some(), "typed category: {json}");
        let decoded: RuntimeClientError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, error);
    }
}

fn questionnaire() -> QuestionnaireSpecification {
    QuestionnaireSpecification {
        questions: vec![QuestionSpecification {
            question: "Which direction?".to_owned(),
            header: "Direction".to_owned(),
            options: vec![
                OptionSpecification {
                    label: "First".to_owned(),
                    description: "The first authored option.".to_owned(),
                    preview: Some("# First".to_owned()),
                },
                OptionSpecification {
                    label: "Second".to_owned(),
                    description: "The second authored option.".to_owned(),
                    preview: None,
                },
            ],
            multi_select: false,
        }],
    }
}

#[test]
fn v3_questionnaire_pending_response_decline_and_settlement_round_trip() {
    let questionnaire = questionnaire();
    let interaction_id = InteractionId::new("interaction-questionnaire-v3");
    let request = InteractionRequest {
        id: interaction_id.clone(),
        conversation_id: ConversationId::new("conv-questionnaire-v3"),
        attempt_id: AttemptId::new("attempt-questionnaire-v3"),
        turn: 1,
        kind: InteractionKind::Questionnaire {
            questionnaire: questionnaire.clone(),
        },
    };
    let submitted = QuestionnaireResponse::Submitted(QuestionnaireSubmission {
        answers: vec![QuestionnaireAnswerEntry {
            question_index: 0,
            answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                label: "First".to_owned(),
            }),
        }],
    });
    let submitted_request = RuntimeClientRequest::InteractionRespond {
        id: request_id(20),
        interaction_id: interaction_id.clone(),
        response: InteractionResponse::Questionnaire {
            response: submitted.clone(),
        },
    };
    let declined_request = RuntimeClientRequest::InteractionRespond {
        id: request_id(21),
        interaction_id: interaction_id.clone(),
        response: InteractionResponse::Questionnaire {
            response: QuestionnaireResponse::Declined,
        },
    };
    let pending = RuntimeClientProtocolEvent {
        cursor: RuntimeClientCursor::new(20),
        event: RuntimeClientEvent::InteractionPending {
            interaction: request.clone(),
        },
    };
    let submitted_settled = RuntimeClientProtocolEvent {
        cursor: RuntimeClientCursor::new(21),
        event: RuntimeClientEvent::InteractionSettled {
            interaction_id: interaction_id.clone(),
            outcome: InteractionOutcome::Responded {
                response: InteractionResponse::Questionnaire {
                    response: submitted,
                },
            },
        },
    };
    let declined_settled = RuntimeClientProtocolEvent {
        cursor: RuntimeClientCursor::new(22),
        event: RuntimeClientEvent::InteractionSettled {
            interaction_id,
            outcome: InteractionOutcome::Responded {
                response: InteractionResponse::Questionnaire {
                    response: QuestionnaireResponse::Declined,
                },
            },
        },
    };

    let pending_json = serde_json::to_value(&pending).expect("pending questionnaire JSON");
    assert_eq!(pending_json["event"]["type"], "interaction_pending");
    assert_eq!(
        pending_json["event"]["interaction"]["kind"]["type"],
        "questionnaire"
    );
    assert_eq!(
        pending_json["event"]["interaction"]["kind"]["questionnaire"],
        serde_json::to_value(&questionnaire).expect("questionnaire JSON")
    );
    assert_eq!(
        serde_json::from_value::<RuntimeClientProtocolEvent>(pending_json)
            .expect("pending round trip"),
        pending
    );

    for request in [submitted_request, declined_request] {
        let json = serde_json::to_value(&request).expect("questionnaire response JSON");
        assert_eq!(json["method"], "interaction_respond");
        assert_eq!(json["response"]["type"], "questionnaire");
        let decoded: RuntimeClientRequest =
            serde_json::from_value(json).expect("questionnaire response round trip");
        assert_eq!(decoded, request);
    }
    for event in [submitted_settled, declined_settled] {
        let json = serde_json::to_value(&event).expect("settled questionnaire JSON");
        assert_eq!(json["event"]["type"], "interaction_settled");
        assert_eq!(
            serde_json::from_value::<RuntimeClientProtocolEvent>(json).expect("settled round trip"),
            event
        );
    }

    let old_question_response = serde_json::json!({
        "method": "interaction_respond",
        "id": 30,
        "interaction_id": "interaction-questionnaire-v3",
        "response": {"type": "question", "answer": "pasted text"}
    });
    assert!(
        serde_json::from_value::<RuntimeClientRequest>(old_question_response).is_err(),
        "the obsolete Question response is not a valid response"
    );
}

/// The snapshot and its sections round-trip exactly; no internal executor
/// or path data exists on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_dto_round_trips() {
    let host = host().await;
    let (attachment, initialized) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let RuntimeClientResult::Initialized {
        snapshot, cursor, ..
    } = initialized
    else {
        panic!("initialized");
    };
    assert_eq!(cursor, RuntimeClientCursor::new(0));
    let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
    let decoded: rustx::runtime_client::RuntimeClientSnapshot =
        serde_json::from_str(&json).expect("deserialize snapshot");
    assert_eq!(decoded, snapshot);
    assert!(
        !json.contains("executor"),
        "no executor data appears on the wire"
    );
    assert!(
        !json.contains("environment_store"),
        "no environment internals appear on the wire"
    );
    let response =
        attachment.handle_request(RuntimeClientRequest::SnapshotGet { id: request_id(1) });
    let Some(RuntimeClientResult::Snapshot { snapshot, cursor }) = response.result else {
        panic!("snapshot result");
    };
    assert_eq!(cursor, RuntimeClientCursor::new(0));
    assert_eq!(snapshot.conversation_id().as_str(), "conv-37-protocol");
    let _ = attachment;
}

/// Attachment request handling correlates ids, negotiates the version,
/// and scopes request ids per attachment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachment_request_correlation_and_version_negotiation() {
    let host = host().await;
    let (attachment, initialized) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let RuntimeClientResult::Initialized {
        attachment_id,
        conversation_id,
        agent_id,
        ..
    } = initialized
    else {
        panic!("initialized");
    };
    assert_eq!(conversation_id.as_str(), "conv-37-protocol");
    assert_eq!(agent_id.as_str(), "agent-a");
    assert!(!attachment_id.as_str().is_empty());

    // Multiple pipelined requests correlate by id.
    let responses: Vec<RuntimeClientResponse> = (1..=3)
        .map(|id| {
            attachment.handle_request(RuntimeClientRequest::SnapshotGet { id: request_id(id) })
        })
        .collect();
    for (index, response) in responses.iter().enumerate() {
        assert_eq!(response.id.get(), u64::try_from(index + 1).expect("fits"));
        assert!(response.error.is_none());
    }

    // Incompatible version negotiation fails explicitly, in both
    // directions and including every superseded wire contract.
    let incompatible = host.attach(11);
    assert!(matches!(
        incompatible,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 10,
            requested: 11,
        })
    ));
    let old_protocol = host.attach(7);
    assert!(matches!(
        old_protocol,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 10,
            requested: 7,
        })
    ));
    // v6 carried the obsolete profile-shaped subagent projection (Issue
    // #144). It is refused rather than served a renamed payload.
    let profile_shaped = host.attach(6);
    assert!(matches!(
        profile_shaped,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 10,
            requested: 6,
        })
    ));

    // The initialize method cannot re-initialize an admitted attachment.
    let reinit = attachment.handle_request(RuntimeClientRequest::Initialize {
        id: request_id(9),
        protocol_version: 10,
    });
    assert!(matches!(
        reinit.error,
        Some(RuntimeClientError::InvalidRequest { .. })
    ));
}

/// Request ids are scoped to one attachment: after detach + reattach, a
/// fresh attachment reuses request ids without any cross-attachment
/// state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_ids_are_attachment_scoped() {
    let host = host().await;
    let (first, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("first attach");
    let first_response =
        first.handle_request(RuntimeClientRequest::SnapshotGet { id: request_id(1) });
    assert!(first_response.error.is_none());
    first.detach();
    let (second, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("second attach");
    let second_response =
        second.handle_request(RuntimeClientRequest::SnapshotGet { id: request_id(1) });
    assert!(
        second_response.error.is_none(),
        "request id 1 is fresh in the new attachment scope"
    );
    assert_ne!(
        first.attachment_id(),
        second.attachment_id(),
        "reconnect receives a distinct attachment identity"
    );
}

/// The second concurrent attachment fails deterministically and never
/// evicts the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_attachment_never_evicts_the_first() {
    let host = host().await;
    let (first, initialized) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("first attach");
    let RuntimeClientResult::Initialized {
        attachment_id: first_id,
        ..
    } = initialized
    else {
        panic!("initialized");
    };
    let second = host.attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION);
    assert!(matches!(
        second,
        Err(RuntimeClientError::AttachmentInUse {
            existing_attachment_id,
        }) if existing_attachment_id == first_id
    ));
    let still_works = first.handle_request(RuntimeClientRequest::SnapshotGet { id: request_id(2) });
    assert!(still_works.error.is_none());
}

/// Detach is a pure attachment operation: it never cancels anything and
/// the runtime keeps serving new attachments afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_releases_the_attachment_exactly() {
    let host = host().await;
    let (first, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    // Idempotent double detach.
    first.detach();
    first.detach();
    let (second, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach after detach");
    let response = second.handle_request(RuntimeClientRequest::SnapshotGet { id: request_id(3) });
    assert!(response.error.is_none());
}

/// The `RuntimeAttachment` RAII handle detaches on drop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachment_raii_drop_detaches() {
    let host = host().await;
    {
        let (attachment, _) = host
            .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let _ = attachment;
    }
    let (_, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach after drop");
}
