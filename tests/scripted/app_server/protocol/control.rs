//! Runtime control through an attachment: headless interactions and exact
//! pending-inbound mutations, settled once and never cancelling the attempt.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn headless_interactions_broadcast_survive_disconnect_and_select_one_response() {
    bounded(async {
        use crate::runtime::interaction::{ApprovalDecision, InteractionResponse, InteractionSettleGate, QuestionnaireResponse};
        use std::sync::Arc;
        struct ReleaseSettlement(Arc<InteractionSettleGate>);
        impl Drop for ReleaseSettlement {fn drop(&mut self){self.0.release();}}
        for tool in ["read", "ask_user"] {
            for first in ["response", "error", "invalid"] {
                let f = Fixture::with_session_count(Some(tool), 1).await;
                let connection = Arc::new(AppServerConnection::new(f.host.clone()));
                initialize(&connection).await;
                let initial = attach(&connection, &f, 0).await;
                let identity = f.manager.load(&initial.session_id, None).await.unwrap();
                let runtime = identity.inspect_runtime().unwrap();
                if tool == "read" { runtime.install_test_pre_tool_policy(Arc::new(AskPolicy)); }
                let coordinator = runtime.interaction_test_owner();
                call(&connection, 60, Method::TurnStart { input_id: uuid::Uuid::new_v4().to_string(),
                    target: initial.clone(), content: vec![UserInputBlock::Text(crate::message::content::TextBlock { text:"request-A".into() })],
                }).await;
                f.gates[0].wait_entered().await;
                call(&connection, 61, Method::SessionDetach { target:initial.clone() }).await;
                f.gates[0].release();
                coordinator.pending_published.notified().await;
                assert_eq!(coordinator.pending_count(), 1);
                let waiting = crate::app_server::thread_status::ThreadStatus::Active { active_flags: vec![if tool == "read" {crate::app_server::thread_status::ThreadActiveFlag::WaitingOnApproval} else {crate::app_server::thread_status::ThreadActiveFlag::WaitingOnUserInput}] };
                assert_eq!(super::status::read(&connection, &initial.conversation_id).await.status, waiting);
                let target = attach(&connection, &f, 0).await;
                let MethodResult::Checkpoint { checkpoint } = call(&connection, 62, Method::SessionCheckpoint { trace_records:vec![],target:target.clone() }).await else { panic!("checkpoint") };
                let interaction = checkpoint.snapshot.pending_interactions[0].interaction.clone();
                call(&connection, 63, Method::SessionSubscribe {target:target.clone(),after_cursor:checkpoint.cursor}).await;
                let ServerMessage::Request(request) = connection.next_server_message().await.unwrap() else { panic!("request") };
                let rival = AppServerConnection::new(f.host.clone()); initialize(&rival).await;
                let target = attach(&rival, &f, 0).await;
                let MethodResult::Checkpoint { checkpoint } = call(&rival, 62, Method::SessionCheckpoint {trace_records:vec![],target:target.clone()}).await else { panic!("checkpoint") };
                call(&rival, 63, Method::SessionSubscribe {target,after_cursor:checkpoint.cursor}).await;
                let ServerMessage::Request(replayed) = rival.next_server_message().await.unwrap() else { panic!("request replay") };
                assert_eq!(request, replayed);
                let third = AppServerConnection::new(f.host.clone()); initialize(&third).await;
                let third_target = attach(&third, &f, 0).await;
                let MethodResult::Checkpoint {checkpoint:third_checkpoint}=call(&third,64,Method::SessionCheckpoint{target:third_target.clone(),trace_records:vec![]}).await else {panic!("third checkpoint")};
                call(&third,65,Method::SessionSubscribe{target:third_target,after_cursor:third_checkpoint.cursor}).await;
                let ServerMessage::Request(third_request)=third.next_server_message().await.unwrap() else {panic!("third subscriber")};
                assert_eq!(third_request,request);
                assert_eq!(coordinator.pending_count(),1);
                assert_eq!(super::status::read(&third,&initial.conversation_id).await.status,waiting);
                // A disconnected subscriber never settles shared native work.
                connection.close(); assert_eq!(coordinator.pending_count(),1);
                let response = if tool == "read" { InteractionResponse::Approval {decision:ApprovalDecision::Allow} }
                    else { InteractionResponse::Questionnaire {response:QuestionnaireResponse::Declined} };
                let result = serde_json::to_value(response).unwrap();
                let winner = match first {
                    "response" => serde_json::json!({"jsonrpc":"2.0","id":request.id,"result":result}),
                    "invalid" => serde_json::json!({"jsonrpc":"2.0","id":request.id,"result":"invalid domain payload"}),
                    _ => serde_json::json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32000,"message":"declined"}}),
                };
                let gate = Arc::new(InteractionSettleGate::default()); gate.arm();let _release=ReleaseSettlement(gate.clone()); coordinator.install_before_settle_gate(gate.clone());
                assert!(rival.handle_json(&winner.to_string()).await.is_none());
                let entered = gate.clone(); tokio::task::spawn_blocking(move || entered.wait_entered()).await.unwrap();
                let loser = serde_json::json!({"jsonrpc":"2.0","id":request.id,"result":result});
                assert!(rival.handle_json(&loser.to_string()).await.is_none());
                let ServerMessage::Resolved {params,..} = rival.next_server_message().await.unwrap() else { panic!("resolved") };
                assert_eq!(params.request_id, request.id);
                assert_eq!(super::status::read(&rival,&initial.conversation_id).await.status,waiting,"claiming the transport reply does not settle the native wait");
                gate.release();
                loop {
                    if let NotificationMethod::Event {event,..} = rival.next_notification().await.notification
                        && event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled {..})) {break;}
                }
                assert_eq!(coordinator.pending_count(),0);
                assert!(runtime.with_pending_interaction_header(&interaction, |_| ()).is_none());
                super::status::next_status(&third,&initial.conversation_id,&crate::app_server::thread_status::ThreadStatus::Idle).await;
                drop(coordinator); drop(runtime); drop(connection); drop(rival); drop(third); f.close().await;
            }
        }
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_pending_mutations_are_routed_cas_bound_and_do_not_cancel_attempts() {
    use crate::durable::inbox::{PendingInboundRef, PendingMutationOutcome as Outcome};
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let a = attach(&connection, &f, 0).await;
        let b = attach(&connection, &f, 1).await;
        let content = |text: &str| {
            vec![UserInputBlock::Text(crate::message::content::TextBlock {
                text: text.into(),
            })]
        };
        call(
            &connection,
            700,
            Method::TurnStart { input_id: uuid::Uuid::new_v4().to_string(),
                target: a.clone(),
                content: content("request-A"),
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let MethodResult::InboundAccepted {
            message_id,
            inbound_sequence,
        } = call(
            &connection,
            701,
            Method::InboundQueue { input_id: uuid::Uuid::new_v4().to_string(),
                target: a.clone(),
                content: content("pending original"),
            },
        )
        .await
        else {
            panic!("accepted")
        };
        let expected = PendingInboundRef {
            sequence: inbound_sequence,
            revision: 0,
        };
        assert!(matches!(
            call(
                &connection,
                702,
                Method::InboundRemove {
                    target: b,
                    expected: expected.clone()
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::NotPending
            }
        ));
        let mut wrong = a.clone();
        wrong.conversation_id = crate::runtime::identity::ConversationId::new(
            "conv_8810ad58-1e59-72bc-8928-b261707a7130",
        );
        assert_eq!(
            rejected(
                &connection,
                Method::InboundRemove {
                    target: wrong,
                    expected: expected.clone()
                }
            )
            .await,
            ErrorData::StaleAttachment
        );
        let mut wrong_incarnation = a.clone();
        wrong_incarnation.runtime_incarnation = serde_json::from_str("0").unwrap();
        let mut wrong_attachment = a.clone();
        wrong_attachment.attachment_id = crate::runtime_client::AttachmentId::new("obsolete");
        let mut wrong_session = a.clone();
        wrong_session.session_id = f.sessions[1].id.clone();
        for target in [wrong_incarnation, wrong_attachment, wrong_session] {
            assert_eq!(
                rejected(
                    &connection,
                    Method::InboundEdit {
                        target,
                        expected: expected.clone(),
                        text: "must not commit".into(),
                    }
                )
                .await,
                ErrorData::StaleAttachment
            );
        }
        assert!(matches!(
            call(
                &connection,
                703,
                Method::InboundEdit {
                    target: a.clone(),
                    expected: expected.clone(),
                    text: "edited pending".into()
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::Applied { .. }
            }
        ));
        assert!(matches!(
            call(
                &connection,
                704,
                Method::InboundRemove {
                    target: a.clone(),
                    expected: expected.clone()
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::Conflict
            }
        ));
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_11,
        } = call(
            &connection,
            705,
            Method::SessionCheckpoint {
                target: a.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("snapshot")
        };
        let snapshot = Box::new(checkpoint_11.snapshot);
        assert_eq!(snapshot.inbound.pending.len(), 1);
        assert_eq!(snapshot.inbound.pending[0].revision, 1);
        assert_eq!(snapshot.inbound.pending[0].message.id, message_id,
            "native edit retains the authoritative Message identity without requiring it in the bounded CAS locator");
        assert_eq!(
            snapshot.inbound.pending[0].message.content,
            input("edited pending")
        );
        let expected = PendingInboundRef {
            revision: 1,
            ..expected
        };
        assert!(matches!(
            call(
                &connection,
                706,
                Method::InboundRemove {
                    target: a.clone(),
                    expected: expected.clone()
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::Applied { .. }
            }
        ));
        assert!(matches!(
            call(
                &connection,
                707,
                Method::InboundRemove {
                    target: a.clone(),
                    expected
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::NotPending
            }
        ));
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_12,
        } = call(
            &connection,
            708,
            Method::SessionCheckpoint {
                target: a.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("snapshot")
        };
        let snapshot = Box::new(checkpoint_12.snapshot);
        assert!(snapshot.inbound.pending.is_empty());
        assert!(
            f.manager
                .load(&a.session_id, None)
                .await
                .unwrap()
                .inspect_runtime()
                .unwrap()
                .has_current_attempt()
        );
        assert_eq!(f.provider.request_bodies().len(), 1);
        f.close().await;
    })
    .await;
}
