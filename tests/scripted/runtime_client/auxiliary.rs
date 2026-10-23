//! Named external consumers through real native attachment and SQLite owners.
use super::super::support::fake::{FakeStep, model_release};
use crate::model::{ModelEvent, ModelFinishReason, session::SessionModelConfig};
use crate::runtime::auxiliary::{
    delivery::{AuxiliaryResult, ConsumerResult},
    submission::{BtwSubmission, ClassificationInput, ClassificationSubmission, CorrelationId},
};
use crate::runtime_client::{RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeAttachment};
fn correlation(value: &str) -> CorrelationId {
    CorrelationId::new(value.into()).unwrap()
}
fn answer(text: &str) -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            text: text.into(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}
async fn result(
    attachment: &RuntimeAttachment,
    call: &crate::runtime::identity::AuxiliaryCallId,
) -> AuxiliaryResult {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(copy) = attachment.auxiliary_result(call).unwrap() {
                return copy.result().unwrap().clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("native completion")
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn classification_is_strict_ephemeral_and_duplicate_semantics_exclude_current_model() {
    let (provider, host) = super::model::runtime(vec![
        answer(r#"{"label":"yes"}"#),
        answer(r#"{"label":"yes","label":"yes"}"#),
    ])
    .await;
    let (attachment, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    let submission = ClassificationSubmission {
        correlation: correlation("classification"),
        input: ClassificationInput::Text {
            text: "Exact bytes\n".into(),
        },
        labels: vec!["yes".into(), "no".into()],
    };
    let receipt = attachment.classify(submission.clone()).await.unwrap();
    assert_eq!(
        result(&attachment, &receipt.call_id).await.value,
        ConsumerResult::Classification {
            label: "yes".into()
        }
    );
    attachment
        .model_set(SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("beta/model-b").unwrap(),
        ))
        .unwrap();
    assert_eq!(
        attachment.classify(submission.clone()).await.unwrap(),
        receipt
    );
    let mut changed = submission.clone();
    changed.labels.reverse();
    assert!(attachment.classify(changed).await.is_err());
    assert_eq!(provider.requests().len(), 1);
    let mut next = submission;
    next.correlation = correlation("reject-duplicate-property");
    let rejected = attachment.classify(next).await.unwrap();
    let rejected = result(&attachment, &rejected.call_id).await;
    assert_eq!(
        rejected.execution,
        Some(crate::model::AuxiliaryCallOutcome::Completed)
    );
    assert!(matches!(
        rejected.value,
        ConsumerResult::Rejected {
            reason: crate::runtime::auxiliary::delivery::ConsumerFailure::InvalidOutput
        }
    ));
    assert!(
        provider
            .requests()
            .iter()
            .all(|request| request.tools.is_empty() && request.continuation.is_none())
    );
    assert!(
        host.checkpoint()
            .unwrap()
            .snapshot
            .transcript
            .entries
            .is_empty()
    );
    host.shutdown().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_btw_survives_detach_and_successor_cancels_without_observation() {
    let (release, parked) = model_release();
    let (provider, host) = super::model::runtime(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::ParkUntilReleased(parked),
        FakeStep::ParkUntilCancelled,
    ]])
    .await;
    let (old, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    let receipt = old
        .btw(BtwSubmission {
            correlation: correlation("handoff"),
            question: "What is committed?".into(),
        })
        .await
        .unwrap();
    provider.parked().wait_for(|parked| *parked).await.unwrap();
    let (observer, _) = host
        .attach_read_only(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    assert!(
        observer
            .btw(BtwSubmission {
                correlation: correlation("readonly"),
                question: "no".into()
            })
            .await
            .is_err()
    );
    assert!(observer.auxiliary_cancel(&receipt.call_id).is_err());
    assert!(observer.auxiliary_discard(&receipt.call_id).is_err());
    old.detach();
    assert!(old.auxiliary_cancel(&receipt.call_id).is_err());
    assert_eq!(provider.streams_exited_count(), 0);
    let (successor, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    assert!(matches!(
        successor
            .auxiliary_lookup(&receipt.correlation, Some(&receipt.scope))
            .unwrap(),
        crate::runtime::conversation_runtime::auxiliary::AuxiliarySubmissionLookup::Known { .. }
    ));
    assert!(successor.auxiliary_cancel(&receipt.call_id).unwrap());
    release.send_replace(true);
    assert!(matches!(
        result(&observer, &receipt.call_id).await.value,
        ConsumerResult::Rejected {
            reason: crate::runtime::auxiliary::delivery::ConsumerFailure::Cancelled
        }
    ));
    assert!(successor.auxiliary_discard(&receipt.call_id).unwrap());
    assert!(
        observer
            .auxiliary_result(&receipt.call_id)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        successor
            .auxiliary_lookup(&receipt.correlation, None)
            .unwrap(),
        crate::runtime::conversation_runtime::auxiliary::AuxiliarySubmissionLookup::Known { .. }
    ));
    host.shutdown().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_result_capacity_refuses_before_another_provider_request() {
    let (provider, host) =
        super::model::runtime((0..16).map(|_| answer("bounded answer")).collect()).await;
    let (attachment, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    let mut calls = Vec::new();
    for number in 0..16 {
        let receipt = attachment
            .btw(BtwSubmission {
                correlation: correlation(&format!("bounded-{number}")),
                question: "question".into(),
            })
            .await
            .unwrap();
        let _ = result(&attachment, &receipt.call_id).await;
        calls.push(receipt);
    }
    assert!(
        attachment
            .btw(BtwSubmission {
                correlation: correlation("full"),
                question: "question".into()
            })
            .await
            .is_err()
    );
    assert_eq!(provider.requests().len(), 16);
    assert!(attachment.auxiliary_discard(&calls[0].call_id).unwrap());
    // The original known command remains known after discard; it never reexecutes.
    assert_eq!(
        attachment
            .btw(BtwSubmission {
                correlation: calls[0].correlation.clone(),
                question: "question".into()
            })
            .await
            .unwrap(),
        calls[0]
    );
    assert_eq!(provider.requests().len(), 16);
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_preparation_publishes_no_receipt_or_provider_request_and_releases_slots() {
    let (provider, host) = super::model::runtime(vec![answer(r#"{"label":"yes"}"#)]).await;
    let (attachment, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    for index in 0..17 {
        let submission = ClassificationSubmission {
            correlation: correlation(&format!("missing-history-{index}")),
            input: ClassificationInput::History {
                message_id: crate::runtime::identity::MessageId::new("absent"),
            },
            labels: vec!["yes".into()],
        };
        assert!(attachment.classify(submission.clone()).await.is_err());
        assert!(!matches!(
            attachment
                .auxiliary_lookup(&submission.correlation, None)
                .unwrap(),
            crate::runtime::conversation_runtime::auxiliary::AuxiliarySubmissionLookup::Known { .. }
        ));
    }
    assert!(provider.requests().is_empty());
    let receipt = attachment
        .classify(ClassificationSubmission {
            correlation: correlation("valid-after-rejection"),
            input: ClassificationInput::Text {
                text: "input".into(),
            },
            labels: vec!["yes".into()],
        })
        .await
        .unwrap();
    assert_eq!(
        result(&attachment, &receipt.call_id).await.value,
        ConsumerResult::Classification {
            label: "yes".into()
        }
    );
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_and_new_submission_follow_the_same_authority_lock_order() {
    let (provider, host) = super::model::runtime(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilCancelled,
        ],
        answer("second"),
    ])
    .await;
    let (attachment, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    let attachment = std::sync::Arc::new(attachment);
    let receipt = attachment
        .btw(BtwSubmission {
            correlation: correlation("lock-order-running"),
            question: "first".into(),
        })
        .await
        .unwrap();
    provider.parked().wait_for(|parked| *parked).await.unwrap();
    let (entered, observing) = tokio::sync::oneshot::channel();
    let (release, paused) = std::sync::mpsc::channel();
    crate::runtime::auxiliary::submission::install_transfer_hook(
        "lock-order-new".into(),
        Box::new(move || {
            entered.send(()).unwrap();
            if paused.recv().unwrap() {
                Ok(())
            } else {
                Err("negative control releases registration".into())
            }
        }),
    );
    let next = attachment.clone();
    let submitting = tokio::spawn(async move {
        next.btw(BtwSubmission {
            correlation: correlation("lock-order-new"),
            question: "second".into(),
        })
        .await
    });
    observing.await.unwrap();
    let controller = attachment.clone();
    let mut cancelling =
        tokio::task::spawn_blocking(move || controller.auxiliary_cancel(&receipt.call_id));
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), &mut cancelling).await;
    release.send(outcome.is_ok()).unwrap();
    let cancelled = if let Ok(value) = outcome {
        value.unwrap().unwrap()
    } else {
        let _ = cancelling.await;
        false
    };
    let _ = submitting.await.unwrap();
    assert!(
        cancelled,
        "cancellation must finish while new registration is parked"
    );
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn classification_history_is_bounded_data_and_never_a_tool_continuation() {
    let sources = super::super::support::fake::classification_history_messages();
    let fixture = super::super::support::runtime_client_fixture::RuntimeClientFixture::builder(
        "conv_85b1a03d-ab33-7d8c-98d3-934561e7386b",
    )
    .initial_messages(sources.clone())
    .scripts((0..3).map(|_| answer(r#"{"label":"yes"}"#)).collect())
    .build()
    .await;
    let (attachment, _) = fixture
        .host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    for (index, source) in sources.iter().enumerate() {
        let admission = attachment
            .classify(ClassificationSubmission {
                correlation: correlation(&format!("history-{index}")),
                input: ClassificationInput::History {
                    message_id: source.id().clone(),
                },
                labels: vec!["yes".into(), "no".into()],
            })
            .await;
        if index == 2 {
            assert!(
                admission.is_err(),
                "ToolCall-bearing input is unsupported before dispatch"
            );
            continue;
        }
        let receipt = admission.unwrap();
        assert!(matches!(
            result(&attachment, &receipt.call_id).await.value,
            ConsumerResult::Classification { .. }
        ));
        let request = fixture.model.requests().last().unwrap().clone();
        assert_eq!(request.messages.len(), 1);
        assert!(request.messages.tool_indices().is_empty());
        assert!(request.tools.is_empty() && request.continuation.is_none());
        let crate::model::ModelInputMessage::RequestOnly(question) = &request.messages[0] else {
            panic!("classification data")
        };
        assert!(question.render().contains(source.id().as_str()));
    }
    assert_eq!(fixture.model.requests().len(), 3);
    fixture.host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn btw_runs_while_the_primary_tool_is_physically_parked_without_changing_its_continuation() {
    use super::super::{
        common,
        support::fake::{FakeTool, ScriptedCall, await_started, success_result, tool_call_events},
    };
    use crate::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy};
    let call = ScriptedCall {
        id: "primary-call",
        tool_id: "primary-tool",
        name: "primary_tool",
        arguments: serde_json::json!({}),
    };
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    first.extend(tool_call_events(0, &call).into_iter().map(FakeStep::Emit));
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let (tool, release) = FakeTool::parking(
        common::tool_policies(
            "primary_tool",
            "primary-tool",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("Primary Tool complete"),
    );
    let mut started = tool.started();
    let mut tools = crate::tools::executor::ToolRegistry::new();
    tool.register(&mut tools);
    let fixture = super::super::support::runtime_client_fixture::RuntimeClientFixture::builder(
        "conv_75b1a03d-ab33-7d8c-98d3-934561e7386b",
    )
    .tools(tools)
    .scripts(vec![first, answer("Side answer"), answer("Primary answer")])
    .build()
    .await;
    let (attachment, _) = fixture
        .host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    attachment
        .submit_inbound(vec![crate::message::types::UserContentBlock::Text(
            crate::message::TextBlock {
                text: "Run primary tool".into(),
            },
        )])
        .unwrap();
    await_started(
        &mut started,
        "primary Tool has started after canonical ToolCall commit",
    )
    .await;
    fixture.host.projection_settled().await;
    let before = fixture.host.checkpoint().unwrap().snapshot;
    let receipt = attachment
        .btw(BtwSubmission {
            correlation: correlation("tool-window"),
            question: "What is committed?".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        result(&attachment, &receipt.call_id).await.value,
        ConsumerResult::Btw {
            text: "Side answer".into()
        }
    );
    let requests = fixture.model.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].messages.tool_indices().is_empty());
    assert!(requests[1].continuation.is_none() && requests[1].tools.is_empty());
    fixture.host.projection_settled().await;
    assert_eq!(
        fixture.host.checkpoint().unwrap().snapshot.attempt,
        before.attempt
    );
    release.send_replace(true);
    let terminal = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fixture.runtime.child_attempt_settlement(0),
    )
    .await
    .unwrap();
    assert!(matches!(
        terminal.outcome,
        crate::runtime::conversation_runtime::NativeAttemptOutcome::Completed
    ));
    assert_eq!(fixture.model.requests().len(), 3);
    let continued = fixture.model.requests().last().unwrap().clone();
    assert!(!continued.messages.tool_indices().is_empty());
    assert!(
        !serde_json::to_string(&continued.messages)
            .unwrap()
            .contains("Side answer")
    );
    fixture.host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_detach_revokes_auxiliary_handles_but_preserves_calls_for_the_successor() {
    use crate::runtime_client::RuntimeClientError::NotAttached;
    let (provider, host) = super::model::runtime(vec![
        answer("retained"),
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilCancelled,
        ],
    ])
    .await;
    let (old, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    let retained = old
        .btw(BtwSubmission {
            correlation: correlation("host-detach-retained"),
            question: "first".into(),
        })
        .await
        .unwrap();
    let _ = result(&old, &retained.call_id).await;
    let active = old
        .btw(BtwSubmission {
            correlation: correlation("host-detach-active"),
            question: "second".into(),
        })
        .await
        .unwrap();
    provider.parked().wait_for(|parked| *parked).await.unwrap();
    let (observer, _) = host
        .attach_read_only(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    host.detach(old.attachment_id());
    host.detach(observer.attachment_id());
    let (successor, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    assert!(matches!(
        old.btw(BtwSubmission {
            correlation: correlation("host-detach-rejected"),
            question: "third".into()
        })
        .await,
        Err(NotAttached)
    ));
    assert!(matches!(
        old.classify(ClassificationSubmission {
            correlation: correlation("host-detach-classify"),
            input: ClassificationInput::Text {
                text: "input".into()
            },
            labels: vec!["yes".into()]
        })
        .await,
        Err(NotAttached)
    ));
    assert!(matches!(
        old.auxiliary_cancel(&active.call_id),
        Err(NotAttached)
    ));
    assert!(matches!(
        old.auxiliary_discard(&retained.call_id),
        Err(NotAttached)
    ));
    for revoked in [&old, &observer] {
        assert!(matches!(
            revoked.auxiliary_lookup(&active.correlation, Some(&active.scope)),
            Err(NotAttached)
        ));
        assert!(matches!(
            revoked.auxiliary_status(&active.call_id),
            Err(NotAttached)
        ));
        assert!(matches!(
            revoked.auxiliary_result(&retained.call_id),
            Err(NotAttached)
        ));
        assert!(matches!(revoked.model_get(), Err(NotAttached)));
    }
    // A later handle detach/drop must not revoke the successor's registration.
    old.detach();
    drop(old);
    assert_eq!(provider.requests().len(), 2);
    assert_eq!(provider.streams_exited_count(), 1);
    assert!(
        successor
            .auxiliary_status(&active.call_id)
            .unwrap()
            .executing
    );
    assert!(
        matches!(successor.auxiliary_lookup(&active.correlation, Some(&active.scope)).unwrap(), crate::runtime::conversation_runtime::auxiliary::AuxiliarySubmissionLookup::Known { receipt } if receipt == active)
    );
    assert_eq!(
        result(&successor, &retained.call_id).await.value,
        ConsumerResult::Btw {
            text: "retained".into()
        }
    );
    assert!(successor.auxiliary_cancel(&active.call_id).unwrap());
    assert!(matches!(
        result(&successor, &active.call_id).await.value,
        ConsumerResult::Rejected {
            reason: crate::runtime::auxiliary::delivery::ConsumerFailure::Cancelled
        }
    ));
    assert!(successor.auxiliary_discard(&retained.call_id).unwrap());
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_detach_before_final_transfer_prevents_call_started_and_provider_dispatch() {
    for classify in [false, true] {
        let fixture = super::super::support::runtime_client_fixture::RuntimeClientFixture::builder(
            "conv_35b1a03d-ab33-7d8c-98d3-934561e7386b",
        )
        .scripts(vec![answer("must not execute")])
        .build()
        .await;
        let store = fixture.runtime.tool_runtime().durable_store();
        let before = store.auxiliary_audit_cut().unwrap();
        let name = if classify {
            "host-detach-classify-transfer"
        } else {
            "host-detach-btw-transfer"
        };
        let (old, _) = fixture
            .host
            .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let old = std::sync::Arc::new(old);
        let (entered, observing) = tokio::sync::oneshot::channel();
        let (release, paused) = std::sync::mpsc::channel();
        crate::runtime::auxiliary::submission::install_transfer_hook(
            name.into(),
            Box::new(move || {
                entered.send(()).unwrap();
                paused.recv().map_err(|error| error.to_string())
            }),
        );
        let caller = old.clone();
        let submitting = tokio::spawn(async move {
            if classify {
                caller
                    .classify(ClassificationSubmission {
                        correlation: correlation(name),
                        input: ClassificationInput::Text {
                            text: "input".into(),
                        },
                        labels: vec!["yes".into()],
                    })
                    .await
            } else {
                caller
                    .btw(BtwSubmission {
                        correlation: correlation(name),
                        question: "question".into(),
                    })
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), observing)
            .await
            .unwrap()
            .unwrap();
        fixture.host.detach(old.attachment_id());
        let (successor, _) = fixture
            .host
            .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        release.send(()).unwrap();
        assert!(submitting.await.unwrap().is_err());
        assert!(fixture.model.requests().is_empty());
        assert_eq!(store.auxiliary_audit_cut().unwrap(), before);
        assert!(store.load_auxiliary_submission(name).unwrap().is_none());
        assert!(!matches!(
            successor
                .auxiliary_lookup(&correlation(name), None)
                .unwrap(),
            crate::runtime::conversation_runtime::auxiliary::AuxiliarySubmissionLookup::Known { .. }
        ));
        fixture.host.shutdown().await.unwrap();
    }
}
