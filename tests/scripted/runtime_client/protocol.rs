//! Issue #37/#130/#136: Runtime Client wire-contract tests.
//!
//! These tests exercise the protocol boundary exclusively through the
//! public Runtime Client surface: deterministic serialization of every
//! envelope, request-id correlation, notification structure, version
//! negotiation, and attachment lifecycle. No host-side race is asserted
//! here (the in-crate host tests own the synchronization proofs).

use super::super::support;

use rustx::runtime::identity::{AttemptId, ConversationId, InteractionId};
use rustx::runtime::interaction::{
    AnswerSpecification, FiniteNumber, InteractionKind, InteractionOutcome, InteractionRef,
    InteractionRequest, InteractionRequester, InteractionResponse, InteractionSource,
    NumberAnswerSpecification, OptionAnswer, OptionSpecification, QuestionSpecification,
    QuestionnaireAnswer, QuestionnaireAnswerEntry, QuestionnaireResponse,
    QuestionnaireSpecification, QuestionnaireSubmission, RoutedInteraction,
    SingleChoiceSpecification,
};
use rustx::runtime_client::RuntimeClientHost;
use rustx::runtime_client::{
    RuntimeClientCursor, RuntimeClientError, RuntimeClientEvent, RuntimeClientProtocolEvent,
    RuntimeClientRequest, RuntimeClientResponse, RuntimeClientResult,
    RuntimeClientSubagentWorkspace, RuntimeClientWorkspaceHandoff, RuntimeClientWorkspaceIsolation,
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
            model: Some(Box::new(support::attempt_model_view("fixture/model-a"))),
            execution_settings: None,
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
            supported: 13,
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
            answer: AnswerSpecification::SingleChoice(SingleChoiceSpecification {
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
                allow_custom: true,
            }),
        }],
    }
}

#[test]
fn v3_questionnaire_pending_response_decline_and_settlement_round_trip() {
    let questionnaire = questionnaire();
    // An MCP-served tool asked, so the projection must carry the canonical
    // server identity rather than a display string the client would have to
    // infer.
    let requester = InteractionRequester {
        tool_id: crate::runtime::identity::ToolId::new("mcp:github:create_issue"),
        tool_name: "create_issue".to_owned(),
        origin: crate::tools::types::ToolOrigin::Mcp {
            server_id: crate::runtime::identity::McpServerId::new("github"),
        },
    };
    let interaction_id = InteractionId::new("interaction-questionnaire-v3");
    let request = InteractionRequest {
        id: interaction_id.clone(),
        conversation_id: ConversationId::new("conv-questionnaire-v3"),
        attempt_id: AttemptId::new("attempt-questionnaire-v3"),
        turn: 1,
        kind: InteractionKind::Questionnaire {
            invocation_id: crate::tools::types::ToolInvocationId::Agent {
                call_id: crate::runtime::identity::ToolCallId::new("questionnaire-call"),
            },
            requester: requester.clone(),
            questionnaire: questionnaire.clone(),
        },
    };
    let submitted = QuestionnaireResponse::Submitted(QuestionnaireSubmission {
        answers: vec![QuestionnaireAnswerEntry {
            question_index: 0,
            answer: QuestionnaireAnswer::Option(OptionAnswer { option_index: 0 }),
        }],
    });
    let submitted_request = RuntimeClientRequest::InteractionRespond {
        id: request_id(20),
        interaction: InteractionRef::new(
            ConversationId::new("conv-questionnaire-v3"),
            interaction_id.clone(),
        ),
        response: InteractionResponse::Questionnaire {
            response: submitted.clone(),
        },
    };
    let declined_request = RuntimeClientRequest::InteractionRespond {
        id: request_id(21),
        interaction: InteractionRef::new(
            ConversationId::new("conv-questionnaire-v3"),
            interaction_id.clone(),
        ),
        response: InteractionResponse::Questionnaire {
            response: QuestionnaireResponse::Declined,
        },
    };
    let pending = RuntimeClientProtocolEvent {
        cursor: RuntimeClientCursor::new(20),
        event: RuntimeClientEvent::InteractionPending {
            interaction: RoutedInteraction {
                interaction: InteractionRef::new(
                    request.conversation_id.clone(),
                    request.id.clone(),
                ),
                source: InteractionSource::Primary,
                request: request.clone(),
            },
        },
    };
    let submitted_settled = RuntimeClientProtocolEvent {
        cursor: RuntimeClientCursor::new(21),
        event: RuntimeClientEvent::InteractionSettled {
            interaction: InteractionRef::new(
                ConversationId::new("conv-questionnaire-v3"),
                interaction_id.clone(),
            ),
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
            interaction: InteractionRef::new(
                ConversationId::new("conv-questionnaire-v3"),
                interaction_id,
            ),
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
        pending_json["event"]["interaction"]["request"]["kind"]["type"],
        "questionnaire"
    );
    assert_eq!(
        pending_json["event"]["interaction"]["request"]["kind"]["questionnaire"],
        serde_json::to_value(&questionnaire).expect("questionnaire JSON")
    );
    // Finding 1: the requester identity is projected as canonical facts, so a
    // Runtime Client can name the MCP server without inferring anything.
    assert_eq!(
        pending_json["event"]["interaction"]["request"]["kind"]["requester"],
        serde_json::json!({
            "tool_id": "mcp:github:create_issue",
            "tool_name": "create_issue",
            "origin": {"mcp": {"server_id": "github"}},
        })
    );
    // The two dimensions stay independent: the routed source says where the
    // interaction came from, the requester says who asked.
    assert_eq!(
        pending_json["event"]["interaction"]["source"]["type"],
        "primary"
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
    assert!(
        matches!(
            host.attach(16),
            Err(RuntimeClientError::UnsupportedProtocolVersion {
                supported: 27,
                requested: 16,
            })
        ),
        "v16 cannot represent background denial"
    );
    assert!(matches!(
        host.attach(24),
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 24,
        })
    ));
    let incompatible = host.attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION + 1);
    assert!(matches!(
        incompatible,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 28,
        })
    ));
    // v26 is the immediately previous contract: its projected subagents carry
    // only `definition_digest` and no effective execution-profile identity
    // (Issue #258). It is refused rather than served a projection with the
    // new field removed; there is no v26 -> v27 conversion.
    let pre_profile_digest = host.attach(26);
    assert!(matches!(
        pre_profile_digest,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 26,
        })
    ));
    // v25 additionally predates the `effective_extensions` projection and the
    // `settings_lifetimes.extensions` boundary (Issue #256), and is refused
    // for the same reason.
    let pre_effective_extensions = host.attach(25);
    assert!(matches!(
        pre_effective_extensions,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 25,
        })
    ));
    let old_protocol = host.attach(7);
    assert!(matches!(
        old_protocol,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 7,
        })
    ));
    // v15 carried the pre-#202 tool status vocabulary: `interrupted` with no
    // bounded `detail`, and a `timed_out` that covered deadline expiry
    // whether or not terminal settlement was proven. Issue #202 replaced
    // that vocabulary under v16, so a v15 client is refused rather than
    // served a projection it would misread. There is no v15 -> v16
    // conversion.
    let interrupted_status = host.attach(15);
    assert!(matches!(
        interrupted_status,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 15,
        })
    ));
    // v14 is the pre-#190 contract: it already carries Issue #187's
    // workspace authority projection and Issue #194's Agent Status window.
    // The v15 disposal/resource shape has no v14 conversion path, so the
    // older client is refused rather than served a projection it would
    // misread.
    let pre_disposal = host.attach(14);
    assert!(matches!(
        pre_disposal,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 14,
        })
    ));
    // v13 carried Issue #187's workspace authority projection together with
    // the pre-#194 latest-only Agent Status shape (`status`, no published
    // placement, no window transition). Issue #194 replaced that shape under
    // v14, so a v13 client is refused rather than served a projection it
    // would misread. There is no v13 -> v14 conversion.
    let latest_only_status = host.attach(13);
    assert!(matches!(
        latest_only_status,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 13,
        })
    ));
    // v6 carried the obsolete profile-shaped subagent projection (Issue
    // #144). It is refused rather than served a renamed payload.
    let profile_shaped = host.attach(6);
    assert!(matches!(
        profile_shaped,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 6,
        })
    ));
    // v12 carried the pre-#187 workspace wire shape (flat `workspace`,
    // `isolated`, handoff `workspace`) as well as the pre-#194 status shape.
    // Both were replaced by later breaking versions, so a v12 client is
    // rejected explicitly rather than decoded into either new shape.
    let pre_workspace_boundary = host.attach(12);
    assert!(matches!(
        pre_workspace_boundary,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            supported: 27,
            requested: 12,
        })
    ));

    // The initialize method cannot re-initialize an admitted attachment.
    let reinit = attachment.handle_request(RuntimeClientRequest::Initialize {
        id: request_id(9),
        protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
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

/// The subagent workspace projection serializes exactly as the shared
/// `tests/fixtures/runtime-client/*.json` fixtures the TUI protocol mirror
/// is validated against. This is the cross-language regression for the
/// Issue #187 wire shape, carried into v15 with the authoritative retained
/// handoff identity fields and unchanged into v16: if either side drifts
/// back to the pre-#187 flat `workspace`/`isolated` schema, or Issue #190
/// silently drops a workspace field, or the Issue #202 renumbering drops
/// one, one of the two fixture assertions fails.
#[test]
fn v15_workspace_wire_shape_matches_the_shared_fixtures() {
    let shared = RuntimeClientSubagentWorkspace {
        borrowed_from: None,
        logical_workspace: std::path::PathBuf::from("/repo"),
        isolation: RuntimeClientWorkspaceIsolation::Shared,
        resource_state: rustx::runtime::subagent::SubagentWorkspaceResourceState::None,
        handoff: None,
    };
    let isolated_subdirectory = RuntimeClientSubagentWorkspace {
        borrowed_from: None,
        logical_workspace: std::path::PathBuf::from("/runtime-root/worktrees/subagent-1/backend"),
        isolation: RuntimeClientWorkspaceIsolation::GitWorktree {
            source_repository_root: std::path::PathBuf::from("/repo"),
            repository_relative_workspace: std::path::PathBuf::from("backend"),
            physical_worktree_root: std::path::PathBuf::from("/runtime-root/worktrees/subagent-1"),
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            branch: "rustx/subagent-1".to_owned(),
            parent_had_uncommitted_changes: true,
        },
        resource_state: rustx::runtime::subagent::SubagentWorkspaceResourceState::Retained,
        handoff: Some(RuntimeClientWorkspaceHandoff {
            logical_workspace: std::path::PathBuf::from(
                "/runtime-root/worktrees/subagent-1/backend",
            ),
            physical_worktree_root: std::path::PathBuf::from("/runtime-root/worktrees/subagent-1"),
            branch: "rustx/subagent-1".to_owned(),
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            head_commit: "89abcdef012345670123456789abcdef01234567".to_owned(),
            dirty: false,
        }),
    };
    let preserved_unresolved = RuntimeClientSubagentWorkspace {
        borrowed_from: None,
        logical_workspace: isolated_subdirectory.logical_workspace.clone(),
        isolation: isolated_subdirectory.isolation.clone(),
        resource_state:
            rustx::runtime::subagent::SubagentWorkspaceResourceState::PreservedUnresolved,
        handoff: None,
    };

    for (fixture, workspace) in [
        (
            "tests/fixtures/runtime-client/workspace-shared-v15.json",
            &shared,
        ),
        (
            "tests/fixtures/runtime-client/workspace-git-worktree-v15.json",
            &isolated_subdirectory,
        ),
        (
            "tests/fixtures/runtime-client/workspace-git-worktree-preserved-unresolved-v15.json",
            &preserved_unresolved,
        ),
    ] {
        let expected = std::fs::read_to_string(fixture).expect("read fixture");
        let serialized = serde_json::to_string_pretty(workspace).expect("serialize workspace");
        assert_eq!(
            serialized,
            expected.trim_end(),
            "{fixture}: the serialized v15 workspace shape drifted from the \
             fixture the TUI mirror is validated against"
        );
        let decoded: RuntimeClientSubagentWorkspace =
            serde_json::from_str(&expected).expect("deserialize fixture");
        assert_eq!(&decoded, workspace, "{fixture}: fixture round-trip");
    }
}

#[test]
fn review_v21_shared_fixture_pins_complete_subject_and_response_identity() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../fixtures/runtime-client/review-v21.json"
    ))
    .unwrap();
    let request: InteractionRequest = serde_json::from_value(fixture.clone()).unwrap();
    assert_eq!(serde_json::to_value(&request).unwrap(), fixture);
    let InteractionKind::Review {
        review,
        subject_digest,
    } = request.kind
    else {
        panic!("Review")
    };
    review.validate().unwrap();
    assert_eq!(review.digest(), subject_digest);
    let response = rustx::events::review::ReviewResponse {
        instance: review.instance.clone(),
        subject_digest,
        decision: rustx::events::review::ReviewDecision::Accepted,
    };
    review.validate_response(&response).unwrap();
    let value = serde_json::to_value(InteractionResponse::Review { response }).unwrap();
    let mut forged = value.clone();
    forged["response"]["subject"] = serde_json::json!({"plan":"replacement"});
    assert!(serde_json::from_value::<InteractionResponse>(forged).is_err());
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<InteractionResponse>(value.clone()).unwrap())
            .unwrap(),
        value
    );
}

#[test]
fn workflow_v22_fixture_pins_native_tree_and_revision_contract() {
    let source: serde_json::Value = serde_json::from_str(include_str!(
        "../../fixtures/runtime-client/workflow-v22.json"
    ))
    .unwrap();
    let event: rustx::runtime_client::RuntimeClientEvent =
        serde_json::from_value(source.clone()).unwrap();
    assert_eq!(serde_json::to_value(event).unwrap(), source);
}

#[test]
fn workflow_result_identity_is_bounded_history_not_model_content() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../fixtures/runtime-client/workflow-result-v22.json"
    ))
    .unwrap();
    let mut result: rustx::tools::types::ToolExecutionResult =
        serde_json::from_value(fixture.clone()).unwrap();
    assert_eq!(serde_json::to_value(&result).unwrap(), fixture);
    assert!(
        serde_json::to_vec(result.workflow.as_ref().unwrap())
            .unwrap()
            .len()
            <= 192
    );
    let projected = result.model_facing_projection();
    result.workflow = None;
    assert_eq!(
        result.model_facing_projection(),
        projected,
        "identity adds no model-visible content"
    );
    let maximum = rustx::runtime::workflow::WorkflowToolIdentity {
        workflow_id: rustx::runtime::workflow::WorkflowId::parse(&"a".repeat(64)).unwrap(),
        program_digest: "a".repeat(64),
    };
    assert!(serde_json::to_vec(&maximum).unwrap().len() <= 192);
}

/// The Number question the cross-language fixtures are built around.
///
/// Its bounds pin the single admissible answer to `2^63` — an exact binary64
/// (a power of two) whose shortest round-tripping decimal,
/// `9223372036854776000`, is a *different* mathematical integer. That is the
/// value a JSON number could not carry across this protocol.
fn number_question_at_two_pow_63() -> InteractionRequest {
    let bound = FiniteNumber::try_new(9_223_372_036_854_775_808.0).expect("2^63 is finite");
    InteractionRequest {
        id: InteractionId::new("interaction-number-v24"),
        conversation_id: ConversationId::new("conv-number-v24"),
        attempt_id: AttemptId::new("attempt-number-v24"),
        turn: 1,
        kind: InteractionKind::Questionnaire {
            invocation_id: crate::tools::types::ToolInvocationId::Agent {
                call_id: crate::runtime::identity::ToolCallId::new("number-call"),
            },
            requester: InteractionRequester {
                tool_id: crate::runtime::identity::ToolId::new("mcp:ledger:post_entry"),
                tool_name: "post_entry".to_owned(),
                origin: crate::tools::types::ToolOrigin::Mcp {
                    server_id: crate::runtime::identity::McpServerId::new("ledger"),
                },
            },
            questionnaire: QuestionnaireSpecification {
                questions: vec![QuestionSpecification {
                    question: "How much?".to_owned(),
                    header: "Amount".to_owned(),
                    answer: AnswerSpecification::Number(NumberAnswerSpecification {
                        minimum: Some(bound),
                        maximum: Some(bound),
                    }),
                }],
            },
        },
    }
}

/// The published `Number` bound survives the Runtime Client protocol exactly.
///
/// This is the request direction of the cross-language contract: the fixture
/// is the byte-for-byte shape the TypeScript client is validated against in
/// `tui/test/protocol-questionnaire-number.test.ts`, so a bound that started
/// rounding, or a wire form that drifted back to a JSON number, fails on both
/// sides at once.
#[test]
fn number_v24_shared_fixture_pins_the_exact_binary64_bound() {
    let fixture = "tests/fixtures/runtime-client/questionnaire-number-v24.json";
    let request = number_question_at_two_pow_63();
    let expected = std::fs::read_to_string(fixture).expect("read fixture");
    assert_eq!(
        serde_json::to_string_pretty(&request).expect("serialize"),
        expected.trim_end(),
        "{fixture}: the serialized v24 Number shape drifted from the fixture \
         the TUI mirror is validated against"
    );
    let decoded: InteractionRequest = serde_json::from_str(&expected).expect("deserialize");
    assert_eq!(decoded, request, "{fixture}: fixture round-trip");

    // The bound on the wire is canonical binary64 text, and it names `2^63`
    // exactly rather than the decimal a JSON number would have printed.
    let projected = serde_json::to_value(&request).expect("project");
    let answer = &projected["kind"]["questionnaire"]["questions"][0]["answer"];
    assert_eq!(answer["minimum"], serde_json::json!("43e0000000000000"));
    assert_eq!(answer["maximum"], serde_json::json!("43e0000000000000"));
    let InteractionKind::Questionnaire { questionnaire, .. } = &decoded.kind else {
        panic!("Questionnaire")
    };
    let AnswerSpecification::Number(number) = &questionnaire.questions[0].answer else {
        panic!("Number")
    };
    assert_eq!(
        number.minimum.expect("a minimum").to_string(),
        "9223372036854775808"
    );
    assert_eq!(number.minimum, number.maximum);
}

/// **The cross-language regression.** The bytes in this fixture are the bytes
/// a JavaScript client actually wrote:
///
/// ```text
/// QuestionnaireOverlay draft "9223372036854775808"
///   -> readNumberDraft            (the exact binary64 2^63)
///   -> finiteNumberToWire         ("43e0000000000000")
///   -> encodeRecord / JSON.stringify
///   -> JSONL bytes                (this fixture)
///   -> serde_json                 (here)
///   -> FiniteNumber(2^63)
///   -> the authoritative range check against the published bounds
/// ```
///
/// The fixture is regenerated by the TypeScript side, which asserts it is
/// byte-identical to what `encodeRecord` produces, so this test consumes the
/// real serialized record rather than a Rust re-implementation of it.
#[test]
fn a_javascript_number_answer_crosses_the_real_jsonl_boundary_intact() {
    let fixture = "tests/fixtures/runtime-client/questionnaire-number-response-v24.jsonl";
    let record = std::fs::read(fixture).expect("read fixture");

    // One JSONL record: LF-terminated, with no interior LF to split it.
    assert_eq!(record.last(), Some(&b'\n'), "{fixture}: LF-terminated");
    let payload = &record[..record.len() - 1];
    assert!(!payload.contains(&b'\n'), "{fixture}: exactly one record");
    // The value crosses as text, so `JSON.stringify` had no number to reformat.
    let bytes = std::str::from_utf8(payload).expect("UTF-8");
    assert!(bytes.contains(r#""value":"43e0000000000000""#), "{bytes}");
    assert!(!bytes.contains("9223372036854776000"), "{bytes}");

    let decoded: RuntimeClientRequest =
        serde_json::from_slice(payload).expect("the runtime decodes the bytes the client wrote");
    let RuntimeClientRequest::InteractionRespond {
        interaction,
        response,
        ..
    } = decoded
    else {
        panic!("interaction_respond")
    };
    let request = number_question_at_two_pow_63();
    assert_eq!(interaction, request.interaction_ref());
    let InteractionResponse::Questionnaire { response } = response else {
        panic!("questionnaire")
    };
    let QuestionnaireResponse::Submitted(submission) = &response else {
        panic!("submitted")
    };
    let QuestionnaireAnswer::Number(number) = &submission.answers[0].answer else {
        panic!("number")
    };

    // decode(encode(FiniteNumber(2^63))) == FiniteNumber(2^63), across the
    // language boundary and through the real transport framing.
    let two_pow_63 = FiniteNumber::try_new(9_223_372_036_854_775_808.0).expect("finite");
    assert_eq!(number.value, two_pow_63);
    assert_eq!(number.value.get().to_bits(), two_pow_63.get().to_bits());
    assert_eq!(number.value.to_string(), "9223372036854775808");

    // And the runtime's own authority accepts it against the bounds it
    // published: a question pinned to `2^63` is answerable, not merely
    // well-formed.
    let InteractionKind::Questionnaire { questionnaire, .. } = &request.kind else {
        panic!("Questionnaire")
    };
    let settled = rustx::events::normalize_questionnaire_response(questionnaire, &response)
        .expect("2^63 satisfies a minimum and maximum of 2^63");
    let QuestionnaireResponse::Submitted(settled) = settled else {
        panic!("submitted")
    };
    assert_eq!(
        settled.answers[0].answer,
        QuestionnaireAnswer::Number(rustx::runtime::interaction::NumberAnswer {
            value: two_pow_63
        })
    );
}
