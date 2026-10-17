//! Actual WS notification overflow preserves the native effect and requires CP repair.
use super::super::{Fixture, app_server_conformance::AppServerConformanceDriver, bounded};
use super::{call, connected, initialize};
use crate::app_server::protocol::*;
use crate::runtime_client::{RuntimeClientCursor, event::RuntimeClientEvent};

struct Following {
    target: AttachmentTarget,
    registration: String,
    /// Complete decoded native records; this is not a client reducer's Applied cursor.
    received: RuntimeClientCursor,
}

async fn checkpoint(
    client: &impl AppServerConformanceDriver,
    target: &AttachmentTarget,
    id: i64,
) -> Box<crate::runtime_client::checkpoint::RuntimeClientCheckpoint> {
    let MethodResult::Checkpoint { checkpoint } = call(
        client,
        id,
        Method::SessionCheckpoint {
            target: target.clone(),
            trace_records: vec![],
        },
    )
    .await
    else {
        panic!("bounded authoritative checkpoint")
    };
    checkpoint
}

async fn subscribe(
    client: &impl AppServerConformanceDriver,
    target: AttachmentTarget,
    after_cursor: RuntimeClientCursor,
    id: i64,
) -> Following {
    let MethodResult::Subscribed {
        registration_id,
        target: confirmed,
        after_cursor: confirmed_cursor,
    } = call(
        client,
        id,
        Method::SessionSubscribe {
            target: target.clone(),
            after_cursor,
        },
    )
    .await
    else {
        panic!("subscription receipt")
    };
    assert_eq!(confirmed, target);
    assert_eq!(confirmed_cursor, after_cursor);
    Following {
        target,
        registration: registration_id,
        received: after_cursor,
    }
}

async fn attach(
    client: &impl AppServerConformanceDriver,
    fixture: &Fixture,
    index: usize,
    id: i64,
) -> Following {
    let MethodResult::Attached { target, .. } = call(
        client,
        id,
        Method::SessionAttach {
            session_id: fixture.sessions[index].id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("identity attachment")
    };
    let initial = checkpoint(client, &target, id + 1).await;
    subscribe(client, target, initial.cursor, id + 2).await
}

async fn start(
    client: &impl AppServerConformanceDriver,
    target: &AttachmentTarget,
    text: &str,
    id: i64,
) -> crate::runtime::identity::MessageId {
    let MethodResult::InboundAccepted { message_id, .. } = call(
        client,
        id,
        Method::TurnStart {
            target: target.clone(),
            content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                text: text.into(),
            })],
        },
    )
    .await
    else {
        panic!("native inbound accepted")
    };
    message_id
}

fn complete_record(
    following: &mut Following,
    target: &AttachmentTarget,
    registration: &str,
    cursor: RuntimeClientCursor,
) {
    assert_eq!(target, &following.target);
    assert_eq!(registration, following.registration);
    assert_eq!(cursor.get(), following.received.get() + 1);
    following.received = cursor;
}

async fn await_overflow(
    client: &impl AppServerConformanceDriver,
    following: &mut Following,
    sibling: &mut Following,
) -> RuntimeClientCursor {
    loop {
        match client.next_notification().await.notification {
            NotificationMethod::Event {
                target,
                registration_id,
                cursor,
                ..
            } if target == following.target => {
                complete_record(following, &target, &registration_id, cursor);
            }
            NotificationMethod::Event {
                target,
                registration_id,
                cursor,
                ..
            } if target == sibling.target => {
                complete_record(sibling, &target, &registration_id, cursor);
            }
            NotificationMethod::ResyncRequired {
                target,
                registration_id,
                reason,
                checkpoint_required,
                failed_at_cursor,
                after_cursor,
                earliest_serviceable,
            } if target == following.target => {
                assert_eq!(registration_id, following.registration);
                assert_eq!(reason, ResyncReason::RecordTooLarge);
                assert!(checkpoint_required);
                assert_eq!(after_cursor, following.received);
                let failed = failed_at_cursor.expect("exact failed native cursor");
                assert_eq!(failed.get(), following.received.get() + 1);
                assert_eq!(earliest_serviceable, failed);
                return failed;
            }
            _ => {}
        }
    }
}

async fn await_native_fact(
    client: &impl AppServerConformanceDriver,
    following: &mut Following,
    selected: impl Fn(&RuntimeClientEvent) -> bool,
) {
    loop {
        match client.next_notification().await.notification {
            NotificationMethod::Event {
                target,
                registration_id,
                cursor,
                event,
            } if target == following.target => {
                complete_record(following, &target, &registration_id, cursor);
                if event.facts().iter().any(&selected) {
                    return;
                }
            }
            NotificationMethod::ResyncRequired { target, .. } if target == following.target => {
                panic!("a repaired native cut must not loop on the same oversized candidate")
            }
            _ => {}
        }
    }
}

async fn await_both_settlements(
    client: &impl AppServerConformanceDriver,
    affected: &mut Following,
    sibling: &mut Following,
) {
    let mut completed = [false; 2];
    while completed != [true, true] {
        match client.next_notification().await.notification {
            NotificationMethod::Event {
                target,
                registration_id,
                cursor,
                event,
            } => {
                let index = if target == affected.target {
                    0
                } else if target == sibling.target {
                    1
                } else {
                    continue;
                };
                let following = if index == 0 {
                    &mut *affected
                } else {
                    &mut *sibling
                };
                complete_record(following, &target, &registration_id, cursor);
                for fact in event.facts() {
                    if let RuntimeClientEvent::AttemptSettled { outcome, .. } = fact {
                        assert!(matches!(
                            outcome,
                            crate::runtime_client::event::RuntimeClientOutcome::Completed { .. }
                        ));
                        assert!(
                            !completed[index],
                            "one native execution terminal per Session"
                        );
                        completed[index] = true;
                    }
                }
            }
            NotificationMethod::ResyncRequired { target, .. }
                if target == affected.target || target == sibling.target =>
            {
                panic!("recovery must resume without a same-object resync loop");
            }
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t06_native_notification_overflow_repairs_at_checkpoint_without_applying_failed_cursor() {
    bounded(Box::pin(async {
        let fixture = Fixture::new().await;
        let (client, serving) = connected(&fixture, true).await;
        initialize(&client).await;
        let mut affected = attach(&client, &fixture, 0, 10).await;
        let mut sibling = attach(&client, &fixture, 1, 20).await;
        let fault =
            crate::app_server::transport::notification_fault::once(&affected.registration, 128);
        let message = start(
            &client,
            &affected.target,
            "request-A oversize native effect",
            30,
        )
        .await;
        fixture.gates[0].wait_entered().await;
        let failed = await_overflow(&client, &mut affected, &mut sibling).await;
        assert!(fault.fired());
        assert!(
            affected.received < failed,
            "the failed event was never received or foldable"
        );
        let native = fixture
            .manager
            .load(&affected.target.session_id, None)
            .await
            .unwrap();
        assert!(native.inspect_runtime().unwrap().has_current_attempt());
        assert_eq!(native.incarnation_id(), affected.target.runtime_incarnation);
        let sibling_message =
            start(&client, &sibling.target, "request-B healthy sibling", 40).await;
        fixture.gates[1].wait_entered().await;
        await_native_fact(&client, &mut sibling, |fact| {
            matches!(fact,
            RuntimeClientEvent::MessageDescriptorCommitted { descriptor, .. }
                if descriptor.identity.as_ref() == Some(&sibling_message))
        })
        .await;
        let recovered = checkpoint(&client, &affected.target, 50).await;
        assert!(recovered.cursor >= failed);
        assert!(
            recovered
                .snapshot
                .message_descriptors
                .iter()
                .any(|header| header.identity.as_ref() == Some(&message))
        );
        assert!(
            recovered
                .snapshot
                .messages
                .iter()
                .any(|entry| entry.id() == &message)
        );
        let mut resumed = subscribe(&client, affected.target.clone(), recovered.cursor, 51).await;
        assert_ne!(resumed.registration, affected.registration);
        fixture.gates[0].release();
        fixture.gates[1].release();
        await_both_settlements(&client, &mut resumed, &mut sibling).await;
        assert!(resumed.received > recovered.cursor);
        assert!(
            fault.fired(),
            "the one-shot error did not rewrite native history"
        );
        assert!(
            !serving.is_finished(),
            "the healthy socket remains connected after recovery"
        );
        // Driver.close aborts its pumps; fixture teardown may reset the socket
        // without a WebSocket close handshake after every functional assertion.
        client.close().await;
        let _ = serving.await.unwrap();
        fixture.close().await;
    }))
    .await;
}
