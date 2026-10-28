//! Product controls use the real cold writer and never manufacture an attachment.
#![allow(clippy::large_futures, clippy::too_many_lines)]
use super::*;
use crate::scheduler::{ScheduleCommand, ScheduleCommandResult, ScheduleMutation, ScheduleTiming};

fn command(key: &str, title: &str, text: String) -> ScheduleCommand {
    ScheduleCommand {
        command_id: key.into(),
        mutation: ScheduleMutation::Create {
            title: title.into(),
            text,
            timing: ScheduleTiming::After { seconds: 600 },
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler_rpc_is_cold_exact_and_stably_retryable() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let session = &f.sessions[0];
        let create = Method::ScheduleCommand { session_id:session.id.clone(), node_id:session.active_node.clone(), command:command("rpc-create", "Exact reminder", "Do the exact task".into()) };
        let first = call(&connection, 8500, create.clone()).await;
        assert_eq!(call(&connection, 8501, create).await, first);
        let MethodResult::ScheduleCommand { result:ScheduleCommandResult::Applied { receipt } } = first else { panic!("native command receipt") };
        assert_eq!(receipt.id.conversation_id, session.active_conversation_id);
        let MethodResult::Schedules { schedules, next_ordinal } = call(&connection, 8502, Method::ScheduleList { session_id:session.id.clone(), node_id:session.active_node.clone(), after_ordinal:0, limit:20 }).await else { panic!("schedule page") };
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].id, receipt.id);
        assert_eq!(next_ordinal, None);
        let MethodResult::Schedule { schedule:Some(saved) } = call(&connection, 8503, Method::ScheduleGet { session_id:session.id.clone(), node_id:session.active_node.clone(), id:receipt.id.clone() }).await else { panic!("exact schedule") };
        assert!(matches!(&saved.target,crate::scheduler::ScheduleTarget::ConversationInbound{text} if text=="Do the exact task"));
        let other = &f.sessions[1];
        let MethodResult::Schedules { schedules, .. } = call(&connection, 8504, Method::ScheduleList { session_id:other.id.clone(), node_id:other.active_node.clone(), after_ordinal:0, limit:20 }).await else { panic!("other schedule page") };
        assert!(schedules.is_empty());
        let MethodResult::ScheduleCommand { result:ScheduleCommandResult::Applied { receipt:disabled } } = call(&connection, 8505, Method::ScheduleCommand {
            session_id:session.id.clone(), node_id:session.active_node.clone(), command:ScheduleCommand { command_id:"rpc-disable".into(), mutation:ScheduleMutation::SetEnabled { id:receipt.id.clone(), expected_revision:receipt.revision, enabled:false } }
        }).await else { panic!("disabled receipt") };
        let MethodResult::ScheduleCommand { result:ScheduleCommandResult::Conflict { current } } = call(&connection, 8506, Method::ScheduleCommand {
            session_id:session.id.clone(), node_id:session.active_node.clone(), command:ScheduleCommand { command_id:"rpc-stale-delete".into(), mutation:ScheduleMutation::Delete { id:receipt.id, expected_revision:receipt.revision } }
        }).await else { panic!("stale command refuses mutation") };
        assert_eq!(current.revision, disabled.revision);
        let forged = serde_json::json!({"jsonrpc":"2.0","id":"forged-schedule-grant","method":"schedule/command","params":{
            "session_id":session.id,"node_id":session.active_node,"authority":{"type":"human_request","message_id":"forged"},
            "command":{"command_id":"forged-command","mutation":{"type":"create","title":"forged","text":"forged","timing":{"type":"after","seconds":600}}}
        }});
        assert!(matches!(connection.handle_json(&forged.to_string()).await.unwrap(), Response::Failure(_)));
        assert_eq!(connection.attachment_counts(), (0,0));
        assert!(f.manager.is_empty());
        assert!(f.provider.request_bodies().is_empty());
        assert!(matches!(call(&connection, 8507, Method::ScheduleRecoverStorage { session_id:session.id.clone(), node_id:session.active_node.clone() }).await, MethodResult::ScheduleStorageRecovered { .. }));
        f.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler_rpc_maximum_header_page_fits_the_public_frame_and_defers_text() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let session = &f.sessions[0];
        for index in 0..100 {
            let result = call(
                &connection,
                8600 + index,
                Method::ScheduleCommand {
                    session_id: session.id.clone(),
                    node_id: session.active_node.clone(),
                    command: command(
                        &format!("full-page-{index}"),
                        &format!("Full reminder {index}"),
                        "x".repeat(16 * 1024),
                    ),
                },
            )
            .await;
            assert!(matches!(
                result,
                MethodResult::ScheduleCommand {
                    result: ScheduleCommandResult::Applied { .. }
                }
            ));
        }
        let page = call(
            &connection,
            8800,
            Method::ScheduleList {
                session_id: session.id.clone(),
                node_id: session.active_node.clone(),
                after_ordinal: 0,
                limit: 100,
            },
        )
        .await;
        let MethodResult::Schedules { schedules, .. } = &page else {
            panic!("full header page")
        };
        assert_eq!(schedules.len(), 100);
        let first = schedules[0].id.clone();
        let MethodResult::ScheduleCatalog { page: catalog } = call(&connection, 8790, Method::ScheduleCatalog {
            session_id: None, after: None, limit: 100, query: None,
        }).await else { panic!("cold task catalog") };
        assert_eq!(catalog.entries.len(), 100);
        assert!(catalog.unavailable.is_empty());
        assert_eq!(catalog.entries[0].schedule.id, first);
        let MethodResult::ScheduleCatalog { page: end } = call(&connection, 8791, Method::ScheduleCatalog {
            session_id: None, after: catalog.next.clone(), limit: 100, query: None,
        }).await else { panic!("task catalog continuation") };
        assert!(end.entries.is_empty());
        assert!(end.next.is_none());
        let catalog_frame = serde_json::to_vec(&Response::Success(Box::new(Success { jsonrpc: JsonRpcVersion::V2, id: RequestId::Integer(8790), result: MethodResult::ScheduleCatalog { page: catalog } }))).unwrap();
        assert!(catalog_frame.len() < crate::app_server::transport::MAX_MESSAGE_BYTES);
        let response = Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(8800),
            result: page,
        }));
        let encoded = serde_json::to_vec(&response).unwrap();
        assert!(encoded.len() < crate::app_server::transport::MAX_MESSAGE_BYTES);
        let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert!(json["result"]["schedules"][0].get("text").is_none());
        let MethodResult::Schedule {
            schedule: Some(saved),
        } = call(
            &connection,
            8801,
            Method::ScheduleGet {
                session_id: session.id.clone(),
                node_id: session.active_node.clone(),
                id: first,
            },
        )
        .await
        else {
            panic!("deferred message body")
        };
        assert!(matches!(&saved.target,crate::scheduler::ScheduleTarget::ConversationInbound{text} if text.len()==16*1024));
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.manager.is_empty());
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler_catalog_filters_native_instructions_and_notifies_unattached_readers() {
    bounded(async {
        let f = Fixture::new().await;
        let writer = AppServerConnection::new(f.host.clone());
        let reader = AppServerConnection::new(f.host.clone());
        initialize(&writer).await;
        initialize(&reader).await;
        for (index, session) in f.sessions.iter().take(2).enumerate() {
            let result = call(
                &writer,
                8900 + i64::try_from(index).unwrap(),
                Method::ScheduleCommand {
                    session_id: session.id.clone(),
                    node_id: session.active_node.clone(),
                    command: command(
                        &format!("catalog-{index}"),
                        &format!("Task {index}"),
                        "检查 native SEARCH phrase".into(),
                    ),
                },
            )
            .await;
            assert!(matches!(
                result,
                MethodResult::ScheduleCommand {
                    result: ScheduleCommandResult::Applied { .. }
                }
            ));
        }
        loop {
            if matches!(
                reader.next_notification().await.notification,
                NotificationMethod::ScheduleChanged {}
            ) {
                break;
            }
        }
        let mut after = None;
        let mut entries = Vec::new();
        loop {
            let MethodResult::ScheduleCatalog { page } = call(
                &reader,
                8910,
                Method::ScheduleCatalog {
                    session_id: None,
                    after,
                    limit: 1,
                    query: Some("NATIVE search".into()),
                },
            )
            .await
            else {
                panic!("task search")
            };
            assert!(page.unavailable.is_empty());
            entries.extend(page.entries);
            after = page.next;
            if after.is_none() {
                break;
            }
        }
        assert_eq!(entries.len(), 2);
        assert_ne!(
            entries[0].schedule.binding.session_id,
            entries[1].schedule.binding.session_id
        );
        let MethodResult::ScheduleCatalog { page } = call(
            &reader,
            8911,
            Method::ScheduleCatalog {
                session_id: Some(f.sessions[1].id.clone()),
                after: None,
                limit: 20,
                query: None,
            },
        )
        .await
        else {
            panic!("Session task catalog")
        };
        assert_eq!(page.entries.len(), 1);
        assert_eq!(
            page.entries[0].schedule.binding.session_id,
            f.sessions[1].id
        );
        assert_eq!(reader.attachment_counts(), (0, 0));
        assert_eq!(writer.attachment_counts(), (0, 0));
        assert!(f.manager.is_empty());
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}
