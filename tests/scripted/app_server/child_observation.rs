//! T02/T03: real Child process, Unix inspection, and authenticated TCP WebSocket.
use super::super::super::{app_server_conformance::AppServerConformanceDriver, transports};
use super::*;
use crate::runtime_client::agent_observation::{AgentObservation, ObservationSource};
use crate::runtime_client::snapshot::RuntimeClientAttemptPhase;

async fn observed(
    client: &impl AppServerConformanceDriver,
    id: i64,
    target: &AttachmentTarget,
    agent: &AgentId,
    after: Option<crate::runtime_client::agent_observation::AgentObservationCursor>,
) -> AgentObservation {
    let result = transports::call(
        client,
        id,
        Method::AgentObserve {
            target: target.clone(),
            agent_id: agent.clone(),
            after,
        },
    )
    .await;
    let response = Response::Success(Box::new(Success {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(id),
        result: result.clone(),
    }));
    assert!(
        serde_json::to_vec(&response).unwrap().len()
            <= crate::app_server::transport::MAX_MESSAGE_BYTES
    );
    let MethodResult::AgentObserve { observation } = result else {
        panic!("bounded Child observation")
    };
    observation
}

async fn attach_other_session(
    client: &impl AppServerConformanceDriver,
    fixture: &Fixture,
) -> AttachmentTarget {
    let MethodResult::Attached { target, .. } = transports::call(
        client,
        20,
        Method::SessionAttach {
            session_id: fixture.sessions[1].id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("unrelated Session attachment")
    };
    let MethodResult::Checkpoint { checkpoint } = transports::call(
        client,
        21,
        Method::SessionCheckpoint {
            target: target.clone(),
            trace_records: vec![],
        },
    )
    .await
    else {
        panic!("independent checkpoint")
    };
    transports::call(
        client,
        22,
        Method::SessionSubscribe {
            target: target.clone(),
            after_cursor: checkpoint.cursor,
        },
    )
    .await;
    target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t02_t03_large_child_observation_keeps_parent_websocket_and_other_session_following() {
    Box::pin(bounded(async {
        // The provider header gate proves the original Child Attempt remains
        // physically running throughout the observation and unrelated traffic.
        let child = ChildDelivery::with_delivery(false).await;
        child.connection.close();
        let (client, serving) = transports::connected(&child.fixture, true).await;
        transports::initialize(&client).await;
        let MethodResult::Attached { target, .. } = transports::call(
            &client,
            1,
            Method::SessionAttach {
                session_id: child.target.session_id.clone(),
                node_id: None,
            },
        )
        .await
        else {
            panic!("same Runtime reattachment")
        };
        assert_eq!(target.runtime_incarnation, child.target.runtime_incarnation);
        let other = attach_other_session(&client, &child.fixture).await;
        let AgentObservation::Checkpoint {
            source: ObservationSource::Live,
            cursor: Some(start),
            checkpoint: first,
            ..
        } = observed(&client, 30, &target, &child.agent, None).await
        else {
            panic!("live Child checkpoint")
        };
        assert_eq!(first.read_cut.conversation_id, start.conversation_id);
        assert_eq!(
            first.read_cut.activation_id.as_ref(),
            Some(&start.activation_id)
        );
        let attempt = first.snapshot.attempt.as_ref().unwrap();
        assert!(matches!(attempt.phase, RuntimeClientAttemptPhase::Running));
        let attempt_id = attempt
            .attempt_id
            .clone()
            .expect("the live Child has its bounded native Attempt identity");
        let native_before = child.evidence();
        let guidance = "water 水🙂 ".repeat(2_100);
        assert!(guidance.len() * 40 > 1_048_576);
        for ordinal in 0..40 {
            let MethodResult::AgentMessage {
                activation_id,
                resumed,
                ..
            } = transports::call(
                &client,
                100 + ordinal,
                Method::AgentSendMessage {
                    target: target.clone(),
                    agent_id: child.agent.clone(),
                    message: guidance.clone(),
                    attachments: vec![],
                },
            )
            .await
            else {
                panic!("native guidance receipt")
            };
            assert!(!resumed);
            assert_eq!(activation_id, start.activation_id);
        }
        let AgentObservation::Checkpoint {
            source: ObservationSource::Live,
            cursor: Some(end),
            checkpoint: large,
            ..
        } = observed(&client, 200, &target, &child.agent, None).await
        else {
            panic!("bounded large Child checkpoint")
        };
        assert_eq!(large.read_cut.conversation_id, end.conversation_id);
        assert_eq!(
            large.read_cut.activation_id.as_ref(),
            Some(&end.activation_id)
        );
        assert_eq!(
            large
                .read_cut
                .source_cut
                .as_ref()
                .unwrap()
                .read
                .pending_total,
            40
        );
        assert!(large.snapshot.inbound.pending.len() <= 32);
        assert_eq!(
            large.snapshot.attempt.as_ref().unwrap().attempt_id.as_ref(),
            Some(&attempt_id)
        );
        assert!(matches!(
            large.snapshot.attempt.as_ref().unwrap().phase,
            RuntimeClientAttemptPhase::Running
        ));
        assert_eq!(
            child.evidence().1,
            native_before.1,
            "observation never starts another model request"
        );
        assert_eq!(
            child.evidence().3,
            native_before.3,
            "the activation remains the original owner"
        );
        let mut consumed = start;
        let mut saw_checkpoint_cursor = consumed.cursor == end.cursor;
        let mut applied_cut = first.read_cut.clone();
        while consumed.cursor < end.cursor {
            let AgentObservation::Events {
                after,
                cursor,
                events,
            } = observed(&client, 201, &target, &child.agent, Some(consumed.clone())).await
            else {
                panic!("continuous native Child events")
            };
            assert_eq!(after, consumed);
            assert!(!events.is_empty() && events.len() <= 32);
            assert!(cursor.cursor > consumed.cursor);
            for event in &events {
                assert_eq!(event.cursor.get(), consumed.cursor.get() + 1);
                let cut = match &event.event {
                    RuntimeClientEvent::ProjectionTransition { metadata, .. } => &metadata.read_cut,
                    RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                        &checkpoint.read_cut
                    }
                    _ => panic!("one complete native observation transition owns each Cursor"),
                };
                assert_eq!(cut.conversation_id, consumed.conversation_id);
                assert_eq!(cut.activation_id.as_ref(), Some(&consumed.activation_id));
                applied_cut = cut.clone();
                saw_checkpoint_cursor |= event.cursor == end.cursor;
                consumed.cursor = event.cursor;
            }
            assert_eq!(consumed, cursor);
        }
        // Replay has no "until" parameter; a complete batch may include
        // further native facts published after the independently captured cut.
        assert!(saw_checkpoint_cursor);
        assert_eq!(consumed.agent_id, end.agent_id);
        assert_eq!(consumed.conversation_id, end.conversation_id);
        assert_eq!(consumed.activation_id, end.activation_id);
        assert!(consumed.cursor >= end.cursor);
        // The cut carried by a completely folded native event must remain
        // usable through the public Parent route after actual Unix replay.
        let MethodResult::CheckpointPage { page } = transports::call(
            &client,
            202,
            Method::ConversationCheckpointPage {
                target: target.clone(),
                agent_id: Some(child.agent.clone()),
                cut: applied_cut.clone(),
                domain: "inbound.pending".into(),
                offset: 0,
                limit: 1,
            },
        )
        .await
        else {
            panic!("bounded Child content page remains readable after Events")
        };
        assert_eq!(page.read_cut, applied_cut);
        assert_eq!(page.items.len(), 1);
        // The initial inbound at native sequence 1 was already adopted before
        // the gated request. Pending continuations are exclusive native sequences.
        assert_eq!(page.items[0].offset, 2);
        assert_eq!(page.next_offset, Some(2));
        let MethodResult::CheckpointPage { page: next } = transports::call(
            &client,
            203,
            Method::ConversationCheckpointPage {
                target: target.clone(),
                agent_id: Some(child.agent.clone()),
                cut: applied_cut.clone(),
                domain: "inbound.pending".into(),
                offset: page.next_offset.unwrap(),
                limit: 1,
            },
        )
        .await
        else {
            panic!("advancing bounded Child page")
        };
        assert_eq!(next.read_cut, applied_cut);
        assert_eq!(next.items.len(), 1);
        assert_eq!(next.items[0].offset, 3);
        assert_eq!(next.next_offset, Some(3));
        assert_ne!(next.items[0].identity, page.items[0].identity);
        transports::call(
            &client,
            300,
            Method::TurnStart {
                target: other.clone(),
                content: wire_text("request-A independent Session"),
            },
        )
        .await;
        loop {
            let notification = client.next_notification().await.notification;
            if let NotificationMethod::Event { target, event, .. } = notification
                && target == other
                && event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
            {
                break;
            }
        }
        let AgentObservation::Checkpoint {
            checkpoint: unchanged,
            ..
        } = observed(&client, 301, &target, &child.agent, None).await
        else {
            panic!("same connection remains usable")
        };
        assert_eq!(
            unchanged
                .snapshot
                .attempt
                .as_ref()
                .unwrap()
                .attempt_id
                .as_ref(),
            Some(&attempt_id)
        );
        assert!(matches!(
            unchanged.snapshot.attempt.as_ref().unwrap().phase,
            RuntimeClientAttemptPhase::Running
        ));
        assert!(
            !serving.is_finished(),
            "bounded reads keep the carrier alive"
        );
        client.close().await;
        // Driver::close aborts the physical pumps after the correlated reads;
        // join server cleanup without requiring a graceful Close handshake.
        let _ = serving.await.unwrap();
        child.close().await;
    }))
    .await;
}
