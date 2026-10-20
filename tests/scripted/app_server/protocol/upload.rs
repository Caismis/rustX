//! Uploads: receipt admission and model projection, binary carriers, lost
//! replies, drain ownership and supervisor capacity.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;
use crate::durable::CanonicalStore;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_upload_receipt_admission_and_model_projection_use_one_owner() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let other = attach(&connection, &f, 1).await;
        let files = binary_upload(&connection, target.clone(), "picture.png", b"NOT_EAGERLY_INJECTED").await;
        let native = f.load(0).await.unwrap().unwrap().inspect_runtime().unwrap();
        assert!(native.tool_runtime().durable_store().load_canonical().unwrap().is_empty());
        rejected(&connection, Method::TurnStart { target: other, content: vec![UserInputBlock::Upload(files[0].receipt.clone())] }).await;
        assert!(native.tool_runtime().durable_store().load_canonical().unwrap().is_empty());
        assert_eq!(std::fs::read(&files[0].path).unwrap(), b"NOT_EAGERLY_INJECTED");
        let body = "request-A\n  exact body  \n";
        call(&connection, 1002, Method::TurnStart { target: target.clone(), content: vec![UserInputBlock::Upload(files[0].receipt.clone()), UserInputBlock::Text(crate::message::content::TextBlock { text: body.into() })] }).await;
        f.gates[0].wait_entered().await;
        let requests = f.provider.request_bodies();
        assert_eq!(requests.len(), 1, "a text-only provider accepts a workspace image");
        let request: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        let user = request["messages"].as_array().unwrap().iter().find(|message| message["content"][0]["text"].as_str().is_some_and(|s| s.contains("user_uploaded_files"))).unwrap();
        assert_eq!(user["content"][0]["text"], format!("<user_uploaded_files>\n  <file name=\"picture.png\" path=\"{}\" />\n</user_uploaded_files>\n\n{body}", files[0].path));
        assert!(!requests[0].contains("NOT_EAGERLY_INJECTED"));
        let snapshots = native.request_history().page(None, 4).unwrap().snapshots;
        let frozen_input = serde_json::to_string(&snapshots[0].input).unwrap();
        assert!(frozen_input.contains(&files[0].path));
        assert!(snapshots[0].input.iter().any(|m| matches!(m, rustx::model::projected::FrozenModelInput::Derived(_))));
        let history = native.tool_runtime().durable_store().load_canonical().unwrap();
        let canonical = serde_json::to_string(&history).unwrap();
        assert!(canonical.contains("uploaded_file"));
        assert!(!canonical.contains(&files[0].path));
        assert!(!canonical.contains("artifact_id"));
        // Replay must not consult current ownership metadata, even if it changes.
        let controller = f.manager.session_controller();
        let mut registry = controller.catalog.lock().await.upload_registry(&target.session_id).unwrap();
        registry.allocations.get_mut(&files[0].file.batch_id).unwrap().workspace = f.workspaces[1].clone();
        controller.catalog.lock().await.commit_uploads(&target.session_id, registry).unwrap();
        let reconstructed = snapshots[0].reconstruct_from_canonical(&history).unwrap();
        let frozen = serde_json::to_string(&reconstructed.messages).unwrap();
        assert!(frozen.contains(&files[0].path));
        assert!(!frozen.contains(f.workspaces[1].to_str().unwrap()));
        f.gates[0].release();
        connection.close(); f.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_upload_waiter_does_not_cancel_or_replay_the_owned_commit() {
    use crate::local_runtime::session::deletion::{DeletionBlocker, SessionDeleteResult};
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let controller = f.manager.session_controller();
        let gate = std::sync::Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        *controller.upload_commit_gate.lock().unwrap() = Some(gate.clone());
        let caller = connection.clone();
        let waiter =
            tokio::spawn(async move { binary_upload(&caller, target, "lost.txt", b"hi").await });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        let claimed = controller
            .catalog
            .lock()
            .await
            .upload_registry(&f.sessions[0].id)
            .unwrap();
        let (batch, allocation) = claimed.allocations.iter().next().unwrap();
        let operation = allocation.operation_id.clone().unwrap();
        assert!(claimed.receipts(&f.sessions[0].id, batch).is_err());
        let SessionDeleteResult::Preview { preview } =
            controller.delete_preview(&f.sessions[0].id).await
        else {
            panic!("preview")
        };
        assert!(matches!(
            controller
                .delete_session(&f.sessions[0].id, &preview.target_revision)
                .await
                .unwrap(),
            SessionDeleteResult::Blocked {
                reason: DeletionBlocker::ResourceConflict,
                ..
            }
        ));
        waiter.abort();
        let _ = waiter.await;
        connection.close();
        *controller.upload_commit_gate.lock().unwrap() = None;
        drop(release);
        // The next allocation acquires the same preparation owner after the
        // abandoned protocol waiter has left; no scheduler timing is evidence.
        controller
            .upload(
                &f.sessions[0].id,
                None,
                vec![crate::local_runtime::session::uploads::UploadFile {
                    name: "fence.txt".into(),
                    bytes: vec![],
                }],
            )
            .await
            .unwrap();
        let registry = controller
            .catalog
            .lock()
            .await
            .upload_registry(&f.sessions[0].id)
            .unwrap();
        assert_eq!(registry.allocations.len(), 2);
        assert!(registry.allocations.values().all(|a| a.ready));
        assert_eq!(
            controller
                .upload_status(&f.sessions[0].id, &operation)
                .await
                .unwrap(),
            crate::local_runtime::session::uploads::UploadOutcome::Ready {
                files: registry.receipts(&f.sessions[0].id, batch).unwrap()
            }
        );
        assert_eq!(
            registry
                .allocations
                .values()
                .filter(|a| a.files[0].name == "lost.txt")
                .count(),
            1
        );
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restored_destination_receipts_admit_a_turn_after_source_deletion() {
    bounded(async {
        use crate::durable::SqliteConversationStore;
        use crate::local_runtime::session::uploads::UploadFile;
        use crate::message::types::{
            InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
        };
        let f = Fixture::with_session_count(None, 1).await;
        let controller = f.manager.session_controller();
        let source = &f.sessions[0];
        let files = controller
            .upload(
                &source.id,
                None,
                vec![
                    UploadFile {
                        name: "A.txt".into(),
                        bytes: b"A".to_vec(),
                    },
                    UploadFile {
                        name: "B.txt".into(),
                        bytes: b"B".to_vec(),
                    },
                ],
            )
            .await
            .unwrap();
        let body = "request-A\n preserve body  \n";
        let access = controller.acquire_session(&source.id, None).await.unwrap();
        let store = SqliteConversationStore::open(
            source.active_conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        store
            .append_canonical(&MessageBlock::User(UserMessageBlock {
                id: crate::runtime::identity::MessageId::new("boundary"),
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
                content: vec![
                    UserContentBlock::UploadedFile(files[0].file.clone()),
                    UserContentBlock::Text(crate::message::content::TextBlock {
                        text: body.into(),
                    }),
                    UserContentBlock::UploadedFile(files[1].file.clone()),
                ],
            }))
            .unwrap();
        let revision = store.load_head().unwrap().revision;
        drop(store);
        drop(access);
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::SessionTransition {
            session: destination,
            editor: Some(editor),
            ..
        } = call(
            &connection,
            1100,
            Method::SessionFork {
                side: crate::local_runtime::session::LineageSide::Before,
                session_id: source.id.clone(),
                node_id: None,
                surface_revision: revision,
                boundary: Some(CanonicalContentLocator::ByMessageId {
                    message_id: crate::runtime::identity::MessageId::new("boundary"),
                }),
            },
        )
        .await
        else {
            panic!("fork with editor receipts");
        };
        let editor = read_editor(&connection, editor).await;
        let crate::local_runtime::session::deletion::SessionDeleteResult::Preview { preview } =
            controller.delete_preview(&source.id).await
        else {
            panic!("preview");
        };
        controller
            .delete_session(&source.id, &preview.target_revision)
            .await
            .unwrap();
        let target = attach_session(&connection, &destination).await;
        rejected(
            &connection,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Upload(files[0].receipt.clone())],
            },
        )
        .await;
        let [
            UserInputBlock::Upload(a),
            UserInputBlock::Text(text),
            UserInputBlock::Upload(b),
        ] = &editor[..]
        else {
            panic!("ordered editor");
        };
        assert_eq!(a.session_id, destination.id);
        assert_eq!(b.session_id, destination.id);
        assert_eq!(text.text, body);
        call(
            &connection,
            1101,
            Method::TurnStart {
                target,
                content: editor,
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let request: serde_json::Value =
            serde_json::from_str(&f.provider.request_bodies()[0]).unwrap();
        let input = request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|m| {
                m["content"][0]["text"]
                    .as_str()
                    .filter(|s| s.contains("user_uploaded_files"))
            })
            .unwrap();
        assert!(input.ends_with(body));
        assert!(input.find("A.txt").unwrap() < input.find("B.txt").unwrap());
        assert!(input.contains(&format!("/uploads/{}/", destination.id.as_str())));
        assert!(!input.contains(&format!("/uploads/{}/", source.id.as_str())));
        f.gates[0].release();
        connection.close();
        f.manager
            .unload(&destination.active_conversation_id)
            .await
            .unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn binary_upload_one_mib_document_repairs_original_receipts_without_model_requests() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut document = b"# Ordinary document\n\n".to_vec();
        document.resize(1024 * 1024 - 1, b'x');
        document.push(b'\n');
        let files = binary_upload(&connection, target.clone(), "document.md", &document).await;
        assert_eq!(std::fs::read(&files[0].path).unwrap(), document);
        let controller = f.manager.session_controller();
        let registry = controller
            .catalog
            .lock()
            .await
            .upload_registry(&target.session_id)
            .unwrap();
        assert_eq!(registry.allocations.len(), 1);
        let operation_id = registry
            .allocations
            .values()
            .next()
            .unwrap()
            .operation_id
            .clone()
            .unwrap();
        for _ in 0..2 {
            let repaired = call(
                &connection,
                9500,
                Method::SessionUploadStatus {
                    target: target.clone(),
                    operation_id: operation_id.clone(),
                },
            )
            .await;
            assert_eq!(
                repaired,
                MethodResult::UploadStatus {
                    outcome: crate::local_runtime::session::uploads::UploadOutcome::Ready {
                        files: files.clone()
                    }
                }
            );
        }
        rejected(
            &connection,
            Method::SessionUploadPrepare {
                target: target.clone(),
                operation_id,
                files: vec![crate::local_runtime::session::uploads::UploadMetadata {
                    name: "document.md".into(),
                    size: document.len(),
                }],
            },
        )
        .await;
        assert_eq!(
            controller
                .catalog
                .lock()
                .await
                .upload_registry(&target.session_id)
                .unwrap()
                .allocations
                .len(),
            1
        );
        assert!(f.provider.request_bodies().is_empty());
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saturated_binary_admission_does_not_hold_control_and_refuses_excess() {
    bounded(async {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let method = |number| Method::SessionUploadPrepare {
            target: target.clone(),
            operation_id: format!("{number:032x}"),
            files: vec![crate::local_runtime::session::uploads::UploadMetadata {
                name: "held.txt".into(),
                size: 1,
            }],
        };
        let mut sockets = Vec::new();
        for number in
            1..=crate::local_runtime::session::uploads::UPLOAD_POLICY.max_concurrent_transfers
        {
            let MethodResult::UploadPrepared { transfer } =
                call(&connection, 9600, method(number)).await
            else {
                panic!("prepared")
            };
            let mut request = format!(
                "ws://127.0.0.1:{}{}",
                transfer.loopback_port.unwrap(),
                transfer.path
            )
            .into_client_request()
            .unwrap();
            request.headers_mut().insert(
                "sec-websocket-protocol",
                "rustx.session-upload.v1".parse().unwrap(),
            );
            let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::Text("next".into())
            );
            sockets.push((socket, transfer.loopback_port.unwrap()));
        }
        rejected(&connection, method(3)).await;
        assert_eq!(
            call(
                &connection,
                9601,
                Method::SessionUploadStatus {
                    target: target.clone(),
                    operation_id: format!("{:032x}", 1)
                }
            )
            .await,
            MethodResult::UploadStatus {
                outcome: crate::local_runtime::session::uploads::UploadOutcome::Unresolved
            }
        );
        assert!(matches!(
            call(&connection, 9602, Method::SessionDetach { target }).await,
            MethodResult::Detached { .. }
        ));
        assert!(f.provider.request_bodies().is_empty());
        for (mut socket, port) in sockets {
            socket.close(None).await.unwrap();
            f.host.uploads().settled.notified().await;
            f.host.uploads().listener_settled.notified().await;
            assert!(
                tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                    .await
                    .is_err(),
                "each owned listener closes after its own transfer, even while another remains held"
            );
        }
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn binary_carrier_rejects_bad_lengths_reuse_expiry_and_paths_before_allocation() {
    bounded(async {
        use crate::local_runtime::session::uploads::{
            UPLOAD_POLICY, UploadMetadata, UploadOutcome,
        };
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        for (index, body) in [
            vec![],
            vec![0],
            vec![0; 3],
            vec![0; UPLOAD_POLICY.max_chunk_bytes + 1],
            vec![0; 2],
        ]
        .into_iter()
        .enumerate()
        {
            let operation_id = format!("{:032x}", index + 1);
            let MethodResult::UploadPrepared { transfer } = call(
                &connection,
                9700,
                Method::SessionUploadPrepare {
                    target: target.clone(),
                    operation_id: operation_id.clone(),
                    files: vec![UploadMetadata {
                        name: "bad.md".into(),
                        size: 2,
                    }],
                },
            )
            .await
            else {
                panic!("prepared")
            };
            let url = format!(
                "ws://127.0.0.1:{}{}",
                transfer.loopback_port.unwrap(),
                transfer.path
            );
            let request = || {
                let mut request = url.clone().into_client_request().unwrap();
                request.headers_mut().insert(
                    "sec-websocket-protocol",
                    "rustx.session-upload.v1".parse().unwrap(),
                );
                request
            };
            let (mut socket, _) = tokio_tungstenite::connect_async(request()).await.unwrap();
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::Text("next".into())
            );
            if body.is_empty() {
                socket.close(None).await.unwrap();
            } else {
                socket.send(Message::Binary(body.into())).await.unwrap();
                if index == 4 {
                    assert_eq!(
                        socket.next().await.unwrap().unwrap(),
                        Message::Text("finish".into())
                    );
                    socket.send(Message::Binary(vec![1].into())).await.unwrap();
                }
                if index == 1 {
                    assert_eq!(
                        socket.next().await.unwrap().unwrap(),
                        Message::Text("next".into())
                    );
                    socket.close(None).await.unwrap();
                }
            }
            f.host.uploads().settled.notified().await;
            assert_eq!(
                call(
                    &connection,
                    9701,
                    Method::SessionUploadStatus {
                        target: target.clone(),
                        operation_id
                    }
                )
                .await,
                MethodResult::UploadStatus {
                    outcome: UploadOutcome::Absent
                }
            );
            assert!(tokio_tungstenite::connect_async(request()).await.is_err());
        }
        let MethodResult::UploadPrepared { transfer } = call(
            &connection,
            9702,
            Method::SessionUploadPrepare {
                target: target.clone(),
                operation_id: "f".repeat(32),
                files: vec![UploadMetadata {
                    name: "expired.md".into(),
                    size: 0,
                }],
            },
        )
        .await
        else {
            panic!("prepared")
        };
        let request = |path: &str| {
            let mut request = format!("ws://127.0.0.1:{}{path}", transfer.loopback_port.unwrap())
                .into_client_request()
                .unwrap();
            request.headers_mut().insert(
                "sec-websocket-protocol",
                "rustx.session-upload.v1".parse().unwrap(),
            );
            request
        };
        assert!(
            tokio_tungstenite::connect_async(request("/wrong"))
                .await
                .is_err()
        );
        let MethodResult::UploadPrepared { transfer: other } = call(
            &connection,
            9703,
            Method::SessionUploadPrepare {
                target: target.clone(),
                operation_id: "e".repeat(32),
                files: vec![UploadMetadata {
                    name: "other.md".into(),
                    size: 0,
                }],
            },
        )
        .await
        else {
            panic!("second preparation")
        };
        // Even a valid capability cannot be moved to another owned-child port.
        assert!(
            tokio_tungstenite::connect_async(request(&other.path))
                .await
                .is_err()
        );
        f.host
            .uploads()
            .expire_prepared(std::time::Instant::now() + std::time::Duration::from_secs(61));
        f.host.uploads().settle_actors().await;
        assert_eq!(f.host.uploads().actor_count(), 0);
        assert!(
            tokio_tungstenite::connect_async(request(&transfer.path))
                .await
                .is_err()
        );
        assert!(
            f.manager
                .session_controller()
                .catalog
                .lock()
                .await
                .upload_registry(&target.session_id)
                .unwrap()
                .allocations
                .is_empty()
        );
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn binary_ready_commit_precedes_lost_reply_and_exact_read_repair() {
    bounded(async {
        use crate::local_runtime::session::uploads::UploadOutcome;
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let gate = std::sync::Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        *f.host.uploads().reply_gate.lock().unwrap() = Some(gate.clone());
        let caller = connection.clone();
        let upload_target = target.clone();
        let waiter = tokio::spawn(async move {
            binary_upload(
                &caller,
                upload_target,
                "lost-ready.md",
                b"# durable before reply",
            )
            .await
        });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        // The reply gate is after native ready, not merely after byte delivery.
        let controller = f.manager.session_controller();
        let registry = controller
            .catalog
            .lock()
            .await
            .upload_registry(&target.session_id)
            .unwrap();
        assert_eq!(registry.allocations.len(), 1);
        let (batch, allocation) = registry.allocations.iter().next().unwrap();
        assert!(allocation.ready);
        let operation_id = allocation.operation_id.clone().unwrap();
        let original = registry.receipts(&target.session_id, batch).unwrap();
        assert!(
            controller
                .uploaded_content(&target.session_id, &[original[0].receipt.clone()])
                .await
                .is_ok()
        );
        waiter.abort();
        let _ = waiter.await;
        for _ in 0..2 {
            assert_eq!(
                call(
                    &connection,
                    9800,
                    Method::SessionUploadStatus {
                        target: target.clone(),
                        operation_id: operation_id.clone()
                    }
                )
                .await,
                MethodResult::UploadStatus {
                    outcome: UploadOutcome::Ready {
                        files: original.clone()
                    }
                }
            );
        }
        *f.host.uploads().reply_gate.lock().unwrap() = None;
        drop(release);
        f.host.uploads().settled.notified().await;
        assert_eq!(
            controller
                .catalog
                .lock()
                .await
                .upload_registry(&target.session_id)
                .unwrap(),
            registry
        );
        assert!(f.provider.request_bodies().is_empty());
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stdio_prepared_upload_is_revoked_when_drain_wins_admission() {
    bounded(async {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let MethodResult::UploadPrepared { transfer } = call(
            &connection,
            9900,
            Method::SessionUploadPrepare {
                target: target.clone(),
                operation_id: "a".repeat(32),
                files: vec![crate::local_runtime::session::uploads::UploadMetadata {
                    name: "revoked.md".into(),
                    size: 1,
                }],
            },
        )
        .await
        else {
            panic!("prepared")
        };
        connection.close();
        f.host.begin_drain();
        assert!(
            f.host.uploads().is_empty(),
            "drain revokes unused capacity synchronously"
        );
        let mut request = format!(
            "ws://127.0.0.1:{}{}",
            transfer.loopback_port.unwrap(),
            transfer.path
        )
        .into_client_request()
        .unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            "rustx.session-upload.v1".parse().unwrap(),
        );
        assert!(tokio_tungstenite::connect_async(request).await.is_err());
        assert!(
            f.manager
                .session_controller()
                .catalog
                .lock()
                .await
                .upload_registry(&target.session_id)
                .unwrap()
                .allocations
                .is_empty()
        );
        connection.close();
        assert!(f.host.drain().await.is_empty());
        f.host.finish_drain().unwrap();
        assert!(f.provider.request_bodies().is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stdio_active_upload_is_owned_until_native_settlement_during_drain() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let controller = f.manager.session_controller();
        let gate = std::sync::Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        *controller.upload_commit_gate.lock().unwrap() = Some(gate.clone());
        let caller = connection.clone();
        let addressed = target.clone();
        let waiter = tokio::spawn(async move {
            binary_upload(&caller, addressed, "accepted.md", b"owned").await
        });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        waiter.abort();
        let _ = waiter.await;
        connection.close();
        let draining = f.host.drain();
        tokio::pin!(draining);
        assert!(futures_util::poll!(&mut draining).is_pending());
        assert!(f.host.finish_drain().is_err());
        assert!(f.host.forced_resources(false).contains("active_uploads=1"));
        drop(release);
        assert!(draining.await.is_empty());
        assert!(f.host.uploads().is_empty());
        f.host.finish_drain().unwrap();
        let registry = controller
            .catalog
            .lock()
            .await
            .upload_registry(&target.session_id)
            .unwrap();
        assert_eq!(registry.allocations.len(), 1);
        assert!(registry.allocations.values().all(|a| a.ready));
        assert!(f.provider.request_bodies().is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_user_input_receipt_collection_enforces_count_bytes_and_authority() {
    bounded(async {
        use crate::local_runtime::session::uploads::{UPLOAD_POLICY, UploadFile};
        for byte_boundary in [false, true] {
            let f = Fixture::new().await;
            let connection = AppServerConnection::new(f.host.clone());
            initialize(&connection).await;
            let target = attach(&connection, &f, 0).await;
            let controller = f.manager.session_controller();
            let native = f.load(0).await.unwrap().unwrap().inspect_runtime().unwrap();
            let mut empty = Vec::new();
            for _ in 0..=UPLOAD_POLICY.max_uploads_per_user_input {
                empty.push(
                    controller
                        .upload(
                            &target.session_id,
                            None,
                            vec![UploadFile {
                                name: "empty".into(),
                                bytes: vec![],
                            }],
                        )
                        .await
                        .unwrap()[0]
                        .receipt
                        .clone(),
                );
            }
            let mut large = Vec::new();
            for size in [
                UPLOAD_POLICY.max_file_bytes,
                UPLOAD_POLICY.max_file_bytes,
                1,
            ] {
                large.push(
                    controller
                        .upload(
                            &target.session_id,
                            None,
                            vec![UploadFile {
                                name: "document".into(),
                                bytes: vec![b'x'; size],
                            }],
                        )
                        .await
                        .unwrap()[0]
                        .receipt
                        .clone(),
                );
            }
            let mut foreign = empty[0].clone();
            foreign.session_id = f.sessions[1].id.clone();
            let mut registry = controller
                .catalog
                .lock()
                .await
                .upload_registry(&target.session_id)
                .unwrap();
            let mut unready = empty[0].clone();
            unready.batch_id = registry
                .claim(
                    registry
                        .allocations
                        .values()
                        .next()
                        .unwrap()
                        .workspace
                        .clone(),
                    &[UploadFile {
                        name: "partial".into(),
                        bytes: vec![],
                    }],
                )
                .unwrap();
            unready.token = registry.allocations[&unready.batch_id].files[0]
                .token
                .clone();
            controller
                .catalog
                .lock()
                .await
                .commit_uploads(&target.session_id, registry.clone())
                .unwrap();
            for receipts in [empty.clone(), large.clone(), vec![foreign], vec![unready]] {
                for steer in [false, true] {
                    let content = receipts
                        .iter()
                        .cloned()
                        .map(UserInputBlock::Upload)
                        .collect();
                    rejected(
                        &connection,
                        if steer {
                            Method::TurnSteer {
                                target: target.clone(),
                                content,
                            }
                        } else {
                            Method::TurnStart {
                                target: target.clone(),
                                content,
                            }
                        },
                    )
                    .await;
                }
            }
            assert!(
                native
                    .tool_runtime()
                    .durable_store()
                    .load_canonical()
                    .unwrap()
                    .is_empty()
            );
            let MethodResult::Checkpoint {
                checkpoint: checkpoint_19,
            } = call(
                &connection,
                9909,
                Method::SessionCheckpoint {
                    target: target.clone(),
                    trace_records: vec![],
                },
            )
            .await
            else {
                panic!("snapshot")
            };
            let snapshot = Box::new(checkpoint_19.snapshot);
            assert!(snapshot.inbound.pending.is_empty());
            assert!(f.provider.request_bodies().is_empty());
            assert_eq!(
                controller
                    .catalog
                    .lock()
                    .await
                    .upload_registry(&target.session_id)
                    .unwrap(),
                registry
            );
            let receipts = if byte_boundary {
                &large[..2]
            } else {
                &empty[..UPLOAD_POLICY.max_uploads_per_user_input]
            };
            let mut content: Vec<_> = receipts
                .iter()
                .cloned()
                .map(UserInputBlock::Upload)
                .collect();
            content.push(UserInputBlock::Text(crate::message::content::TextBlock {
                text: "request-A".into(),
            }));
            call(&connection, 9910, Method::TurnStart { target, content }).await;
            f.gates[0].wait_entered().await;
            assert_eq!(f.provider.request_bodies().len(), 1);
            connection.close();
            f.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_supervisors_settle_after_success_without_accumulating_expiry_actors() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        for _ in 0..6 {
            binary_upload(&connection, target.clone(), "success.md", b"owned").await;
            f.host.uploads().settle_actors().await;
            assert_eq!(f.host.uploads().actor_count(), 0);
            assert!(f.host.uploads().is_empty());
        }
        connection.close();
        assert!(f.host.drain().await.is_empty());
        f.host.finish_drain().unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_drain_waits_for_revoked_loopback_supervisor_physical_exit() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let gate = std::sync::Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        *f.host.uploads().actor_gate.lock().unwrap() = Some(gate.clone());
        let MethodResult::UploadPrepared { transfer } = call(
            &connection,
            9910,
            Method::SessionUploadPrepare {
                target,
                operation_id: "d".repeat(32),
                files: vec![crate::local_runtime::session::uploads::UploadMetadata {
                    name: "unused".into(),
                    size: 1,
                }],
            },
        )
        .await
        else {
            panic!("prepare");
        };
        f.host.begin_drain();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        connection.close();
        assert!(f.manager.drain_all_runtimes().await.is_empty());
        assert!(f.host.uploads().is_empty());
        assert_eq!(f.host.uploads().actor_count(), 1);
        assert!(
            tokio::net::TcpStream::connect((
                std::net::Ipv4Addr::LOCALHOST,
                transfer.loopback_port.unwrap()
            ))
            .await
            .is_err()
        );
        let drain = f.host.drain();
        tokio::pin!(drain);
        assert!(futures_util::poll!(&mut drain).is_pending());
        assert!(f.host.finish_drain().is_err());
        drop(release);
        assert!(drain.await.is_empty());
        assert_eq!(f.host.uploads().actor_count(), 0);
        f.host.finish_drain().unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detached_unconsumed_upload_is_absent_and_releases_supervisor_capacity() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let operation_id = "e".repeat(32);
        let MethodResult::UploadPrepared { transfer } = call(
            &connection,
            9920,
            Method::SessionUploadPrepare {
                target: target.clone(),
                operation_id: operation_id.clone(),
                files: vec![crate::local_runtime::session::uploads::UploadMetadata {
                    name: "unused".into(),
                    size: 1,
                }],
            },
        )
        .await
        else {
            panic!("prepare");
        };
        assert!(matches!(
            call(&connection, 9922, Method::SessionDetach { target }).await,
            MethodResult::Detached {}
        ));
        connection.close();
        f.host.uploads().settle_actors().await;
        assert_eq!(f.host.uploads().actor_count(), 0);
        assert!(f.host.uploads().is_empty());
        assert!(
            tokio::net::TcpStream::connect((
                std::net::Ipv4Addr::LOCALHOST,
                transfer.loopback_port.unwrap()
            ))
            .await
            .is_err()
        );
        let replacement = AppServerConnection::new(f.host.clone());
        initialize(&replacement).await;
        let target = attach(&replacement, &f, 0).await;
        assert_eq!(
            call(
                &replacement,
                9921,
                Method::SessionUploadStatus {
                    target: target.clone(),
                    operation_id
                }
            )
            .await,
            MethodResult::UploadStatus {
                outcome: crate::local_runtime::session::uploads::UploadOutcome::Absent
            }
        );
        assert!(
            f.manager
                .session_controller()
                .catalog
                .lock()
                .await
                .upload_registry(&target.session_id)
                .unwrap()
                .allocations
                .is_empty()
        );
        binary_upload(&replacement, target, "fresh.md", b"new").await;
        f.host.uploads().settle_actors().await;
        replacement.close();
        assert!(f.host.drain().await.is_empty());
        f.host.finish_drain().unwrap();
    })
    .await;
}
