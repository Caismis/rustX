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
