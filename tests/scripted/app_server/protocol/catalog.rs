//! Session catalog over the protocol: listing and search, archive preflight,
//! multi-client deletion and activity, explicit node switches and exact node
//! reads.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;
use crate::durable::CanonicalStore;
use crate::durable::JournalStore;

#[tokio::test]
async fn archive_preflight_failures_reach_the_protocol_without_private_diagnostics() {
    use crate::durable::SqliteConversationStore;
    use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
    use crate::session_archive::SessionArchivePrepareError;
    bounded(async {
        let f = Fixture::new().await;
        let root = crate::runtime::local_storage::ProductRoot::existing(&f.archive_root).unwrap();
        let catalog = crate::local_runtime::session::SessionCatalog::read_under_guard(&root).unwrap().unwrap();
        let session = &f.sessions[0];
        let store = SqliteConversationStore::open(session.active_conversation_id.clone(), &catalog.database_path(&session.id, &session.active_conversation_id)).unwrap();
        let child = ConversationId::generate();
        let (event, authority) = crate::local_runtime::session::tests::deletion_tests::admit_agent(crate::runtime::subagent::ownership_event(
            &AgentId::new("archive-parent"),
            &session.active_conversation_id, &SubagentId::for_conversation(&session.active_conversation_id, 1),
            &AgentId::new("archive-child"), &child, &crate::runtime::subagent::AgentActivationOrigin::CreationTool { tool_call_id: ToolCallId::new("archive-child-call") },
            &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
            &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
            &serde_json::from_value(serde_json::json!(format!("sha256:{}", "a".repeat(64)))).unwrap(),
            crate::events::types::SubagentOwnershipKind::Normal,
            &f.workspaces[0].clone(), chrono::Utc::now(),
        ));
        store.append_agent_admission(event, &authority).unwrap(); // Required child deliberately has no allocation.
        let session = &f.sessions[1];
        let store = SqliteConversationStore::open(session.active_conversation_id.clone(), &catalog.database_path(&session.id, &session.active_conversation_id)).unwrap();
        let mut message = crate::message::types::UserMessageBlock {
            id: crate::runtime::identity::MessageId::new("archive-artifact"), content: input("authored"),
            source: crate::message::types::UserSource::Human, kind: crate::message::types::InboundKind::default(), timestamp: None,
        };
        message.content.push(crate::message::types::UserContentBlock::File(crate::message::content::FileReference {
            artifact_id: crate::runtime::identity::ArtifactId::new("artifact_1"), name: None, mime_type: None, description: None,
        }));
        store.append_canonical(&crate::message::types::MessageBlock::User(message)).unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        for (index, reason) in [SessionArchivePrepareError::DescendantUnavailable, SessionArchivePrepareError::ArtifactUnavailable].into_iter().enumerate() {
            let response = connection.handle_request(Request { jsonrpc: JsonRpcVersion::V2, id: RequestId::Integer(2),
                call: Method::SessionExportPrepare { session_id: f.sessions[index].id.clone() } }).await;
            let Response::Failure(failure) = response else { panic!("preflight returned a descriptor") };
            assert_eq!(failure.error.data, Some(ErrorData::ArchivePreparationFailed { reason }));
            assert_eq!(failure.error.message, reason.to_string());
            assert!(!failure.error.message.contains(f.archive_root.to_str().unwrap()));
            let encoded = serde_json::to_value(Response::Failure(failure.clone())).unwrap();
            assert!(jsonschema::validator_for(&crate::app_server::schema::protocol_schema()).unwrap().is_valid(&encoded));
            // These exact native errors are generated into the fixtures used by both clients.
            assert!(crate::app_server::schema::fixtures().iter().any(|fixture| {
                matches!(fixture, ProtocolMessage::Response(Response::Failure(item)) if item.error == failure.error)
            }));
            if index == 0 {
                // Global ownership is required even when exporting another Session.
                // Repair the missing child before independently exercising artifacts.
                let path = catalog.database_path(&f.sessions[0].id, &child);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                SqliteConversationStore::open(child.clone(), &path).unwrap().initialize(&[]).unwrap();
            }
        }
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    }).await;
}

// P03 (app-server plane): catalog searches by identity, name, and persisted
// projection are served from the catalog alone — zero conversation-store
// opens on the serving thread.
#[tokio::test]
async fn session_list_searches_by_identity_name_and_projection_open_no_store() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let session = &f.sessions[0];
        f.manager
            .sessions
            .rename_session(&session.id, "projection search name")
            .await
            .unwrap();
        assert!(
            f.manager
                .sessions
                .publish_display_preview(&session.id, "projection search line")
                .await
                .unwrap()
        );
        for query in [
            session.id.as_str().to_owned(),
            "projection search name".to_owned(),
            "projection search line".to_owned(),
        ] {
            let before = crate::durable::conversation_store_opens_on_this_thread();
            let MethodResult::Sessions { sessions, .. } = call(
                &connection,
                40,
                Method::SessionList {
                    query: Some(query),
                    offset: 0,
                    limit: 32,
                },
            )
            .await
            else {
                panic!("list")
            };
            let after = crate::durable::conversation_store_opens_on_this_thread();
            assert_eq!(
                after, before,
                "a catalog search never opens a conversation store"
            );
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].id, session.id);
            assert_eq!(sessions[0].name.as_deref(), Some("projection search name"));
            assert_eq!(
                sessions[0].preview.as_deref(),
                Some("projection search line")
            );
        }
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue422_multi_client_deletion_membership_converges_at_commit() {
    use crate::app_server::session_deletion::SessionDeletionResult as Deletion;
    bounded(async {
        for uncertain in [false, true] {
            let f = Fixture::with_session_count(None, 1).await;
            let a = AppServerConnection::new(f.host.clone());
            let b = AppServerConnection::new(f.host.clone());
            initialize(&a).await;
            initialize(&b).await;
            let id = f.sessions[0].id.clone();
            let MethodResult::Sessions { sessions, .. } = call(
                &a,
                1,
                Method::SessionList {
                    query: None,
                    offset: 0,
                    limit: 32,
                },
            )
            .await
            else {
                panic!()
            };
            assert_eq!(sessions[0].id, id);
            let MethodResult::Deletion {
                result: Deletion::Preview { preview },
            } = call(
                &b,
                2,
                Method::SessionDeletePreview {
                    session_id: id.clone(),
                },
            )
            .await
            else {
                panic!()
            };
            if uncertain {
                f.manager
                    .sessions
                    .catalog
                    .lock()
                    .await
                    .arm_write_fault_after_rename();
            }
            let result = call(
                &b,
                3,
                Method::SessionDelete {
                    session_id: id.clone(),
                    expected_target_revision: preview.target_revision,
                },
            )
            .await;
            assert_eq!(
                matches!(
                    result,
                    MethodResult::Deletion {
                        result: Deletion::CommittedDurabilityUncertain { .. }
                    }
                ),
                uncertain
            );
            if !uncertain {
                assert!(matches!(
                    result,
                    MethodResult::Deletion {
                        result: Deletion::Deleted { .. }
                    }
                ));
            }
            let mut membership = false;
            let mut retired = false;
            while !membership || !retired {
                match a.next_notification().await.notification {
                    NotificationMethod::SummaryInvalidated {
                        session_id,
                        catalog_changed: true,
                    } => {
                        assert_eq!(session_id, id);
                        membership = true;
                    }
                    NotificationMethod::OwnershipRetired {
                        session_id,
                        retired_through,
                    } => {
                        assert_eq!(session_id, id);
                        assert_eq!(retired_through, sessions[0].ownership_generation);
                        retired = true;
                    }
                    _ => {}
                }
            }
            let MethodResult::Sessions { sessions, .. } = call(
                &a,
                4,
                Method::SessionList {
                    query: None,
                    offset: 0,
                    limit: 32,
                },
            )
            .await
            else {
                panic!()
            };
            assert!(sessions.is_empty());
            f.close().await;
        }
    })
    .await;
}

#[tokio::test]
async fn human_message_activity_reorders_live_catalog_without_focus_or_rename_activity() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let before = f
            .manager
            .sessions
            .read_session_summary(&target.session_id)
            .await
            .unwrap()
            .updated_at;
        let completion = AttemptCompletion::capture(&f, &target);
        call(
            &connection,
            801,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("activity moves this conversation"),
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        loop {
            if let NotificationMethod::SummaryInvalidated {
                session_id,
                catalog_changed: true,
            } = connection.next_notification().await.notification
                && session_id == target.session_id
            {
                break;
            }
        }
        let summary = f
            .manager
            .sessions
            .read_session_summary(&target.session_id)
            .await
            .unwrap();
        assert!(summary.updated_at > before);
        let MethodResult::Sessions { sessions, .. } = call(
            &connection,
            802,
            Method::SessionList {
                query: None,
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("list")
        };
        assert_eq!(sessions[0].id, target.session_id);
        f.manager
            .sessions
            .rename_session(&target.session_id, "renamed without activity")
            .await
            .unwrap();
        assert_eq!(
            f.manager
                .sessions
                .read_session_summary(&target.session_id)
                .await
                .unwrap()
                .updated_at,
            summary.updated_at
        );
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        assert_eq!(
            f.manager
                .sessions
                .read_session_summary(&target.session_id)
                .await
                .unwrap()
                .updated_at,
            summary.updated_at
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_node_switch_commits_aba_ownership_and_coalesces_exact_retirements() {
    bounded(async {
        use crate::durable::SqliteConversationStore;
        use crate::local_runtime::session::LineageSide;
        use crate::message::TextBlock;
        use crate::message::types::{
            InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
        };
        use crate::runtime::identity::MessageId;
        let f = Fixture::with_session_count(None, 1).await;
        let source = &f.sessions[0];
        let controller = f.manager.session_controller();
        let access = controller.acquire_session(&source.id, None).await.unwrap();
        let store = SqliteConversationStore::open(
            source.active_conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        store
            .append_canonical(&MessageBlock::User(UserMessageBlock {
                id: MessageId::new("ownership-seed"),
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "ownership seed".into(),
                })],
            }))
            .unwrap();
        let revision = store.load_head().unwrap().revision;
        drop(store);
        drop(access);
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let a1 = controller.read_session_summary(&source.id).await.unwrap();
        let target_a = attach(&connection, &f, 0).await;
        let MethodResult::SessionTransition {
            session: branch, ..
        } = call(
            &connection,
            8000,
            Method::SessionBranch {
                session_id: source.id.clone(),
                node_id: source.active_node.clone(),
                surface_revision: revision,
                boundary: CanonicalContentLocator::ByMessageId {
                    message_id: MessageId::new("ownership-seed"),
                },
                side: LineageSide::Before,
            },
        )
        .await
        else {
            panic!("branch committed")
        };
        let b = controller.read_session_summary(&source.id).await.unwrap();
        assert!(
            b.ownership_generation.parse::<u64>().unwrap()
                > a1.ownership_generation.parse::<u64>().unwrap()
        );
        call(
            &connection,
            8001,
            Method::SessionSwitchNode {
                target: target_a,
                node_id: branch.active_node,
            },
        )
        .await;
        assert_eq!(
            controller
                .read_session_summary(&source.id)
                .await
                .unwrap()
                .ownership_generation,
            b.ownership_generation
        );
        let target_b = attach(&connection, &f, 0).await;
        let MethodResult::Session { session: selected } = call(
            &connection,
            8002,
            Method::SessionSwitchNode {
                target: target_b,
                node_id: source.active_node.clone(),
            },
        )
        .await
        else {
            panic!("selection committed")
        };
        assert_eq!(selected.active_node, source.active_node);
        let MethodResult::SessionSummary { summary: a2 } = call(
            &connection,
            8003,
            Method::SessionSummary {
                session_id: source.id.clone(),
            },
        )
        .await
        else {
            panic!("native ownership")
        };
        assert_eq!(a2.active_node, a1.active_node);
        assert!(
            a2.ownership_generation.parse::<u64>().unwrap()
                > b.ownership_generation.parse::<u64>().unwrap()
        );
        // No notification was read during either commit: the native log must
        // coalesce through B while the newer A2 read is already authoritative.
        loop {
            if let NotificationMethod::OwnershipRetired {
                session_id,
                retired_through,
            } = connection.next_notification().await.notification
            {
                assert_eq!(session_id, source.id);
                assert_eq!(retired_through, b.ownership_generation);
                break;
            }
        }
        assert!(f.provider.request_bodies().is_empty());
        connection.close();
        f.close().await;
    })
    .await;
}

// Real Route, Controller and Runtime Manager evidence for browser switch policy.
async fn switch_fixture(
    f: &Fixture,
) -> (
    std::sync::Arc<AppServerConnection>,
    AttachmentTarget,
    crate::local_runtime::session::SessionSnapshot,
) {
    use crate::durable::SqliteConversationStore;
    use crate::local_runtime::session::LineageSide;
    use crate::message::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::MessageId;
    let source = &f.sessions[0];
    let controller = f.manager.session_controller();
    let access = controller.acquire_session(&source.id, None).await.unwrap();
    let store =
        SqliteConversationStore::open(source.active_conversation_id.clone(), &access.database_path)
            .unwrap();
    store
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: MessageId::new("ownership-seed"),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "ownership seed".into(),
            })],
        }))
        .unwrap();
    let revision = store.load_head().unwrap().revision;
    drop(store);
    drop(access);
    let connection = std::sync::Arc::new(AppServerConnection::new(f.host.clone()));
    initialize(&connection).await;
    let target_a = attach(&connection, f, 0).await;
    let MethodResult::SessionTransition {
        session: branch, ..
    } = call(
        &connection,
        8000,
        Method::SessionBranch {
            session_id: source.id.clone(),
            node_id: source.active_node.clone(),
            surface_revision: revision,
            boundary: CanonicalContentLocator::ByMessageId {
                message_id: MessageId::new("ownership-seed"),
            },
            side: LineageSide::Before,
        },
    )
    .await
    else {
        panic!("branch committed")
    };
    // Branch authors B and selects it. Restore A before switch so this test
    // proves switch itself commits selection, not merely an earlier branch.
    controller
        .set_current_node(&source.id, &source.active_node)
        .await
        .unwrap();
    (connection, target_a, branch)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switch_residency_settlement_survives_failure_and_lost_acknowledgement() {
    bounded(async {
        use crate::local_runtime::session_runtime_manager::ResidencyState;
        use std::sync::atomic::Ordering;
        for outcome in ["success", "composition_failure", "lost_ack"] {
            let f = Fixture::with_session_count(None, 1).await;
            let (connection, target_a, branch) = switch_fixture(&f).await;
            let retiring = f.manager.probe(&target_a.conversation_id);
            let probe = f.manager.probe(&branch.active_conversation_id);
            retiring.before_shutdown.arm();
            probe.before_compose.arm();
            if outcome == "composition_failure" {
                probe.fail_compose_once.store(true, Ordering::SeqCst);
            }
            let pending = connection.clone();
            let original = target_a.clone();
            let node_id = branch.active_node.clone();
            let work = tokio::spawn(async move {
                pending
                    .handle_request(Request {
                        jsonrpc: JsonRpcVersion::V2,
                        id: RequestId::Integer(8100),
                        call: Method::SessionSwitchNode {
                            target: original,
                            node_id,
                        },
                    })
                    .await
            });
            retiring.before_shutdown.entered().await;
            assert_eq!(
                f.manager.residency(&target_a.conversation_id),
                ResidencyState::Unloading
            );
            assert_eq!(
                f.manager.residency(&branch.active_conversation_id),
                ResidencyState::Unloaded
            );
            assert_eq!(connection.attachment_counts(), (1, 0));
            assert_eq!(f.host.diagnostics().external_attachments, 1);
            assert_eq!(
                rejected(
                    &connection,
                    Method::TurnStart {
                        target: target_a.clone(),
                        content: vec![UserInputBlock::Text(crate::message::TextBlock {
                            text: "must not execute".into()
                        })],
                    }
                )
                .await,
                ErrorData::StaleRuntime
            );
            if outcome == "lost_ack" {
                connection.close();
            }
            retiring.before_shutdown.release();
            let response = work.await.unwrap();
            let Response::Success(receipt) = response else {
                panic!("native switch selection receipt: {response:?}");
            };
            let MethodResult::Session { session } = receipt.result else {
                panic!("native switch selection result");
            };
            assert_eq!(session.active_node, branch.active_node);
            assert_eq!(
                session.active_conversation_id,
                branch.active_conversation_id
            );
            assert_eq!(
                f.manager.residency(&target_a.conversation_id),
                ResidencyState::Unloaded
            );
            assert_eq!(
                f.manager.residency(&branch.active_conversation_id),
                ResidencyState::Unloaded
            );
            assert_eq!(
                probe.compositions.load(Ordering::SeqCst),
                0,
                "selection never composes a successor implicitly"
            );
            assert_eq!(connection.attachment_counts(), (0, 0));
            assert_eq!(f.host.diagnostics().external_attachments, 0);
            let inspector = AppServerConnection::new(f.host.clone());
            initialize(&inspector).await;
            let MethodResult::Session { session } = call(
                &inspector,
                8101,
                Method::SessionRead {
                    session_id: branch.id.clone(),
                },
            )
            .await
            else {
                panic!("native selection");
            };
            assert_eq!(session.active_node, branch.active_node);
            assert_eq!(
                session.active_conversation_id,
                branch.active_conversation_id
            );
            if outcome != "lost_ack" {
                assert_eq!(
                    rejected(
                        &connection,
                        Method::SessionCheckpoint {
                            target: target_a.clone(),
                            trace_records: vec![]
                        }
                    )
                    .await,
                    ErrorData::StaleAttachment
                );
            }
            // Explicit attachment owns composition; failure cannot roll back the confirmed switch.
            let attaching = inspector.clone();
            let selected = branch.clone();
            let preparation = tokio::spawn(async move {
                attaching
                    .handle_request(Request {
                        jsonrpc: JsonRpcVersion::V2,
                        id: RequestId::Integer(8103),
                        call: Method::SessionAttach {
                            session_id: selected.id,
                            node_id: Some(selected.active_node),
                        },
                    })
                    .await
            });
            probe.before_compose.entered().await;
            assert_eq!(
                f.manager.residency(&branch.active_conversation_id),
                ResidencyState::Loading
            );
            assert_eq!(inspector.attachment_counts(), (0, 1));
            let competing = AppServerConnection::new(f.host.clone());
            initialize(&competing).await;
            assert_eq!(
                rejected(
                    &competing,
                    Method::SessionAttach {
                        session_id: branch.id.clone(),
                        node_id: Some(f.sessions[0].active_node.clone())
                    }
                )
                .await,
                ErrorData::OperationFailed
            );
            probe.before_compose.release();
            let prepared = preparation.await.unwrap();
            if outcome == "composition_failure" {
                assert!(matches!(prepared, Response::Failure(_)), "{prepared:?}");
                assert_eq!(
                    f.manager.residency(&branch.active_conversation_id),
                    ResidencyState::Unloaded
                );
            } else {
                let Response::Success(receipt) = prepared else {
                    panic!("explicit successor attachment: {prepared:?}");
                };
                let MethodResult::Attached { target, .. } = receipt.result else {
                    panic!("explicit successor attachment result");
                };
                assert_eq!(target.conversation_id, branch.active_conversation_id);
                assert_eq!(
                    f.manager.residency(&branch.active_conversation_id),
                    ResidencyState::Loaded
                );
                call(&inspector, 8104, Method::SessionDetach { target }).await;
            }
            assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
            assert_eq!(inspector.attachment_counts(), (0, 0));
            assert_eq!(f.host.diagnostics().external_attachments, 0);
            let selected = f
                .manager
                .session_controller()
                .read_session_summary(&branch.id)
                .await
                .unwrap();
            assert_eq!(
                selected.active_node, branch.active_node,
                "failed composition cannot restore A"
            );
            let target_b = attach(&inspector, &f, 0).await;
            assert_eq!(target_b.conversation_id, branch.active_conversation_id);
            assert_eq!(inspector.attachment_counts(), (1, 0));
            assert_eq!(f.host.diagnostics().external_attachments, 1);
            call(&inspector, 8102, Method::SessionDetach { target: target_b }).await;
            assert_eq!(f.host.diagnostics().external_attachments, 0);
            assert_eq!(
                f.manager.residency(&branch.active_conversation_id),
                ResidencyState::Loaded
            );
            assert!(f.provider.request_bodies().is_empty());
            competing.close();
            inspector.close();
            connection.close();
            f.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_node_read_retains_origin_and_never_selects_or_composes() {
    bounded(async {
        use crate::local_runtime::session::{LineageSide, SessionNodeId};
        use crate::local_runtime::session::observation::SessionNodeOriginObservation as SessionNodeOrigin;
        use crate::runtime::identity::MessageId;
        let f = Fixture::new().await;
        let (connection, target, branch) = switch_fixture(&f).await;
        let source = &f.sessions[0];
        f.manager.session_controller().set_current_node(&source.id, &branch.active_node).await.unwrap();
        let MethodResult::Node { node: root } = call(&connection, 8200, Method::SessionNode { session_id: source.id.clone(), node_id: source.active_node.clone() }).await else { panic!("exact root") };
        assert_eq!(root.id, source.active_node);
        assert_eq!(root.conversation_id, source.active_conversation_id);
        assert_eq!(root.origin, SessionNodeOrigin::New);
        let MethodResult::Node { node } = call(&connection, 8201, Method::SessionNode { session_id: source.id.clone(), node_id: branch.active_node.clone() }).await else { panic!("exact fork") };
        assert!(matches!(node.origin, SessionNodeOrigin::Fork { source_session, source_node, source_message, side: LineageSide::Before, .. } if source_session == source.id && source_node == source.active_node && source_message == Some(MessageId::new("ownership-seed"))));
        let missing = SessionNodeId::generate();
        assert_eq!(rejected(&connection, Method::SessionNode { session_id: source.id.clone(), node_id: missing.clone() }).await, ErrorData::UnknownNode { session_id: source.id.clone(), node_id: missing });
        assert_eq!(rejected(&connection, Method::SessionNode { session_id: f.sessions[1].id.clone(), node_id: root.id }).await, ErrorData::UnknownNode { session_id: f.sessions[1].id.clone(), node_id: source.active_node.clone() });
        assert_eq!(f.manager.session_controller().read_session(&source.id).await.unwrap().active_node, branch.active_node);
        assert_eq!(connection.attachment_counts(), (1, 0));
        assert_eq!(f.manager.probe(&branch.active_conversation_id).compositions.load(std::sync::atomic::Ordering::SeqCst), 0);
        let MethodResult::Tree { nodes, .. } = call(&connection, 8202, Method::SessionTree { session_id: source.id.clone(), offset: 0, limit: 32 }).await else { panic!("tree") };
        assert_eq!(nodes.len(), 2);
        let MethodResult::TranscriptWindow { window } = call(&connection, 8205, Method::Transcript {
            target: target.clone(), at: crate::durable::reading::ConversationWindowAt::Message { id: MessageId::new("ownership-seed"), cut: None }, limit: 1,
        }).await else { panic!("exact Message through matching peers") };
        assert_eq!(window.target_cursor.unwrap().get(), 1);
        assert!(window.target.is_none());
        assert_eq!(window.page.entries.len(), 1);
        let MethodResult::Boundaries { surface_revision, .. } = call(&connection, 8206, Method::SessionBoundaries { target: target.clone(), offset: 0, limit: 32 }).await else { panic!("cut") };
        let MethodResult::SessionTransition { session: inherited, .. } = call(&connection, 8207, Method::SessionFork {
            session_id: source.id.clone(), node_id: Some(source.active_node.clone()), surface_revision, boundary: Some(CanonicalContentLocator::ByMessageId {message_id:MessageId::new("ownership-seed")}), side: LineageSide::Before,
        }).await else { panic!("inherited lineage") };
        let MethodResult::SessionHistory { window, .. } = call(&connection, 8208, Method::SessionHistory {
            session_id: inherited.id, node_id: Some(inherited.active_node), at: crate::durable::reading::ConversationWindowAt::Latest, limit: 1,
        }).await else { panic!("inherited boundary through matching peers") };
        assert_eq!(window.page.inherited_through.unwrap().get(), 0);
        call(&connection, 8203, Method::SessionDetach { target }).await;
        let MethodResult::Node { node } = call(&connection, 8204, Method::SessionNode { session_id: source.id.clone(), node_id: branch.active_node.clone() }).await else { panic!("unattached read") };
        assert_eq!(node.conversation_id, branch.active_conversation_id);
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    }).await;
}
