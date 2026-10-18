//! Queue capacity is delivery backpressure, independent of native control work.
use super::*;
use crate::runtime::conversation_runtime::NativeAttemptOutcome;
use tokio::sync::{mpsc, oneshot};

fn turn(target: AttachmentTarget, text: &str) -> Method {
    Method::TurnStart {
        target,
        content: input(text)
            .into_iter()
            .map(|block| match block {
                crate::message::types::UserContentBlock::Text(text) => UserInputBlock::Text(text),
                _ => panic!("text-only fixture"),
            })
            .collect(),
    }
}

async fn second_target(connection: &AppServerConnection, fixture: &Fixture) -> AttachmentTarget {
    let direct = app_server_conformance::DirectDriver(connection);
    let MethodResult::Attached { target, .. } = call(
        &direct,
        3,
        Method::SessionAttach {
            session_id: fixture.sessions[1].id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("independent native attachment on the same carrier");
    };
    target
}

async fn send_request(input: &mpsc::Sender<String>, id: i64, call: Method) {
    input
        .send(
            serde_json::to_string(&Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(id),
                call,
            })
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn settle_second_conversation(
    fixture: &Fixture,
    input: &mpsc::Sender<String>,
    target: AttachmentTarget,
) {
    let runtime = fixture
        .manager
        .configuration_runtime(&target.session_id)
        .unwrap();
    let after = runtime.settlement_ordinal_for_test();
    send_request(input, 98, turn(target, "request-B")).await;
    fixture.gates[1].wait_entered().await;
    fixture.gates[1].release();
    let settlement = runtime.child_attempt_settlement(after).await;
    assert_eq!(settlement.ordinal, after + 1);
    assert!(matches!(
        settlement.outcome,
        NativeAttemptOutcome::Completed
    ));
}

async fn accept_record(
    outbound: &mut transport::Outbound,
    replies: &mpsc::Sender<Response>,
) -> io::Result<()> {
    let (record, accepted) =
        std::future::poll_fn(|_| outbound.poll_hand_off(|_| std::task::Poll::Ready(Ok(()))))
            .await?;
    if accepted.is_some() {
        assert!(record.len() <= transport::MAX_MESSAGE_BYTES);
        if let Ok(response) = serde_json::from_str::<Response>(&record) {
            replies
                .try_send(response)
                .expect("only three correlated replies");
        } else {
            serde_json::from_str::<Notification>(&record).expect("original native notification");
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transient_full_queue_preserves_admitted_control_and_other_conversation_execution() {
    bounded(async {
        let fixture = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
        let direct = app_server_conformance::DirectDriver(&connection);
        initialize(&direct).await;
        let target = attach(&direct, &fixture).await;
        let second = second_target(&connection, &fixture).await;
        let runtime = fixture.manager.configuration_runtime(&target.session_id).unwrap();
        let after = runtime.settlement_ordinal_for_test();
        call(&direct, 2, turn(target.clone(), "request-A")).await;
        fixture.gates[0].wait_entered().await;
        for _ in 0..64 {
            super::replenish_observation(&runtime);
        }
        let (input, incoming) = mpsc::channel::<String>(2);
        let incoming = futures_util::stream::unfold(incoming, |mut incoming| async move {
            incoming.recv().await.map(|record| (Ok(record), incoming))
        });
        let (saturated, full) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let (reply_send, mut replies) = mpsc::channel(3);
        let shutdown = CancellationToken::new();
        let serving = tokio::spawn(transport::serve_for_test(
            connection.clone(),
            incoming,
            move |mut outgoing| async move {
                let mut first = outgoing.next().await.expect("native queue head");
                outgoing.wait_full().await;
                saturated.send(()).unwrap();
                released.await.unwrap();
                accept_record(&mut first, &reply_send).await?;
                while let Some(mut outbound) = outgoing.next().await {
                    accept_record(&mut outbound, &reply_send).await?;
                }
                Ok(())
            },
            shutdown.clone(),
        ));
        full.await.unwrap();
        assert!(!connection.closed_for_test());
        settle_second_conversation(&fixture, &input, second).await;
        send_request(&input, 99, Method::TurnCancel { target }).await;
        let settled = runtime.child_attempt_settlement(after).await;
        assert_eq!(settled.ordinal, after + 1);
        assert!(matches!(settled.outcome, NativeAttemptOutcome::Cancelled));
        assert!(replies.try_recv().is_err(), "no output accepted before recovery");
        assert!(!connection.closed_for_test());
        release.send(()).unwrap();
        let mut ids = std::collections::BTreeSet::new();
        for _ in 0..2 {
            let Response::Success(response) = replies.recv().await.unwrap() else {
                panic!("the original native response survives saturation");
            };
            assert!(ids.insert(response.id.clone()), "one response per request");
            if response.id == RequestId::Integer(99) {
                assert!(matches!(response.result, MethodResult::CancellationAccepted { attempt_id } if attempt_id == settled.attempt_id));
            } else {
                assert_eq!(response.id, RequestId::Integer(98));
                assert!(matches!(response.result, MethodResult::InboundAccepted { .. }));
            }
        }
        send_request(&input, 100, Method::ServerInfo {}).await;
        let Response::Success(response) = replies.recv().await.unwrap() else {
            panic!("the same connection remains serviceable");
        };
        assert_eq!(response.id, RequestId::Integer(100));
        assert_eq!(fixture.provider.request_bodies().len(), 2);
        assert_eq!(runtime.settlement_ordinal_for_test(), settled.ordinal);
        shutdown.cancel();
        serving.await.unwrap().unwrap();
        fixture.gates[0].release();
        fixture.close().await;
    }).await;
}
