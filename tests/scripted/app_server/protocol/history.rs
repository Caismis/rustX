//! Bounded historical reads: cold trace pages, durable Session reads that never
//! compose a runtime, editor and origin history, and lineage cuts shared by
//! branch and fork.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;
use crate::durable::CanonicalStore;
use crate::durable::JournalStore;
use crate::durable::ObservationStore;
use crate::durable::ProjectionStore;
use crate::durable::inbox::ConversationInboundCapability;

fn trace_content_references(
    value: &serde_json::Value,
    output: &mut Vec<crate::observation::content::ContentRef>,
) {
    match value {
        serde_json::Value::Object(fields) => {
            if let Some(reference) = fields.get("content_ref") {
                output
                    .push(crate::observation::structural_json::decode(reference.clone()).unwrap());
            }
            for field in fields.values() {
                trace_content_references(field, output);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                trace_content_references(item, output);
            }
        }
        _ => {}
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t11_t15_cold_trace_pages_read_exact_deferred_request_and_fence_foreign_cut() {
    bounded(async {
        use base64::Engine as _;
        let fixture = Fixture::new().await;
        let identity = fixture
            .manager
            .load(&fixture.sessions[0].id, None)
            .await
            .unwrap();
        let runtime = identity.inspect_runtime().unwrap();
        let text = "é\\\"\n".repeat(8_000);
        runtime.submit_inbound(input(&text)).unwrap();
        fixture.gates[0].wait_entered().await;
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let session_id = fixture.sessions[0].id.clone();
        let MethodResult::SessionTraceHistory { page, .. } = call(
            &connection,
            46_701,
            Method::SessionTraceHistory {
                session_id: session_id.clone(),
                node_id: None,
                before: None,
                limit: 32,
            },
        )
        .await
        else {
            panic!("native trace summaries");
        };
        let record = page
            .records
            .iter()
            .find(|record| record.kind == crate::observation::trace::TraceKind::Request)
            .unwrap();
        let MethodResult::SessionTraceHistoryDetail {
            page,
            conversation_id,
        } = call(
            &connection,
            46_702,
            Method::SessionTraceHistoryDetail {
                session_id: session_id.clone(),
                node_id: None,
                record_id: record.id.clone(),
            },
        )
        .await
        else {
            panic!("bounded cold native detail");
        };
        assert_eq!(conversation_id, fixture.sessions[0].active_conversation_id);
        assert_eq!(page.total, 1);
        assert!(page.items[0].deferred || !page.items[0].content.is_empty());
        assert!(
            serde_json::to_string(&MethodResult::DomainPage { page: page.clone() })
                .unwrap()
                .len()
                < 786_432
        );
        let mut bytes = Vec::new();
        let mut offset = 0;
        let mut chunks = 0;
        loop {
            let MethodResult::CheckpointValueChunk { chunk } = call(
                &connection,
                46_703,
                Method::SessionTraceValueRead {
                    session_id: session_id.clone(),
                    node_id: None,
                    cut: page.read_cut.clone(),
                    domain: page.domain.clone(),
                    item_offset: 0,
                    path: vec![],
                    offset,
                    max_bytes: 8192,
                },
            )
            .await
            else {
                panic!("bounded cold native value");
            };
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(chunk.bytes_base64)
                .unwrap();
            assert!(!decoded.is_empty() && decoded.len() <= 8192);
            bytes.extend(decoded);
            chunks += 1;
            match chunk.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert!(
            chunks > 1,
            "deferred native request requires advancing reads"
        );
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut references = Vec::new();
        trace_content_references(&value, &mut references);
        let reference = references
            .into_iter()
            .find(|reference| {
                reference.representation
                    == crate::observation::content::ContentRepresentation::Utf8Text
                    && reference.readable_end == text.len() as u64
            })
            .expect("frozen canonical input content descriptor");
        let mut content = Vec::new();
        let mut offset = 0;
        loop {
            let MethodResult::ContentChunk { chunk } = call(
                &connection,
                46_705,
                Method::SessionTraceContentRead {
                    session_id: session_id.clone(),
                    node_id: None,
                    cut: page.read_cut.clone(),
                    content_ref: reference.clone(),
                    offset,
                    max_bytes: 8192,
                },
            )
            .await
            else {
                panic!("cold canonical source chunk");
            };
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(chunk.data_base64)
                .unwrap();
            assert!(!decoded.is_empty() && decoded.len() <= 8192);
            content.extend(decoded);
            match chunk.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(String::from_utf8(content).unwrap(), text);
        // The same historical native cut discovers deferred canonical fields
        // without attaching this reader or loading a complete conversation.
        let source_cut = page.read_cut.source_cut.clone().unwrap();
        let MethodResult::CheckpointPage { page: messages } = call(
            &connection,
            46_706,
            Method::SessionHistoryPage {
                session_id: session_id.clone(),
                node_id: None,
                cut: source_cut.clone(),
                domain: "messages".into(),
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("cold canonical descriptors")
        };
        assert_eq!(messages.read_cut.source_cut.as_ref(), Some(&source_cut));
        assert!(messages.items.len() <= 32);
        let item = messages.items.first().expect("adopted user message header");
        let MethodResult::CheckpointPage { page: blocks } = call(
            &connection,
            46_707,
            Method::SessionHistoryPage {
                session_id: session_id.clone(),
                node_id: None,
                cut: source_cut.clone(),
                domain: format!("messages.{}.blocks", item.identity),
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("cold canonical block descriptors")
        };
        assert_eq!(blocks.total, 1);
        assert!(
            blocks
                .items
                .iter()
                .flat_map(|item| &item.content)
                .any(|binding| binding.content_ref == reference)
        );
        let mut foreign_source = source_cut;
        foreign_source.read.conversation_id = fixture.sessions[1].active_conversation_id.clone();
        assert!(matches!(
            connection
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(46_708),
                    call: Method::SessionHistoryPage {
                        session_id: session_id.clone(),
                        node_id: None,
                        cut: foreign_source,
                        domain: "messages".into(),
                        offset: 0,
                        limit: 32
                    }
                })
                .await,
            Response::Failure(_)
        ));
        let mut foreign = page.read_cut;
        foreign.conversation_id = fixture.sessions[1].active_conversation_id.clone();
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(46_704),
                call: Method::SessionTraceValueRead {
                    session_id,
                    node_id: None,
                    cut: foreign,
                    domain: page.domain,
                    item_offset: 0,
                    path: vec![],
                    offset: 0,
                    max_bytes: 65_536,
                },
            })
            .await;
        assert!(matches!(
            response,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::ReadCutExpired),
                    ..
                },
                ..
            })
        ));
        fixture.gates[0].release();
        connection.close();
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_session_operations_never_compose_a_runtime() {
    bounded(async {
        use crate::app_server::session_deletion::SessionDeletionResult as Deletion;
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let settings = crate::local_runtime::session::SessionPersistentState::from_input(
            &crate::local_runtime::configuration::SessionConfigInput::new(f.workspaces[0].clone()),
        );
        let MethodResult::SessionTransition { session, .. } = call(
            &connection,
            100,
            Method::SessionCreate {
                submission: None,
                settings,
            },
        )
        .await
        else {
            panic!("create");
        };
        let id = session.id;
        let MethodResult::Sessions { sessions, .. } = call(
            &connection,
            110,
            Method::SessionList {
                query: None,
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("list");
        };
        assert_eq!(
            sessions.iter().find(|row| row.id == id).unwrap().cwd,
            f.workspaces[0]
        );
        let residency_before = f.manager.diagnostics();
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            111,
            Method::SessionSummary {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!("exact summary")
        };
        assert_eq!(&summary, sessions.iter().find(|row| row.id == id).unwrap());
        assert_eq!(f.manager.diagnostics(), residency_before);
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.provider.request_bodies().is_empty());
        let MethodResult::Session { session: unchanged } = call(
            &connection,
            112,
            Method::SessionRead {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!("read")
        };
        assert_eq!(unchanged.active_node, summary.active_node);
        assert_eq!(unchanged.updated_at, summary.updated_at);
        let missing = crate::local_runtime::session::SessionId::new(
            "ses_00000000-0000-7000-8000-000000000099",
        );
        let absent = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(113),
                call: Method::SessionSummary {
                    session_id: missing.clone(),
                },
            })
            .await;
        assert!(matches!(absent, Response::Failure(Failure {
            error: RpcError { data: Some(ErrorData::UnknownSession { session_id }), .. }, ..
        }) if session_id == missing));
        assert_eq!(f.manager.diagnostics(), residency_before);
        let MethodResult::Session { session } = call(
            &connection,
            101,
            Method::SessionName {
                session_id: id.clone(),
                name: "durable only".into(),
            },
        )
        .await
        else {
            panic!("name");
        };
        assert_eq!(session.name.as_deref(), Some("durable only"));
        let MethodResult::Settings { projection } = call(
            &connection,
            102,
            Method::SettingsRead {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!("settings");
        };
        let settings = projection.inline.unwrap();
        assert!(
            settings.model.is_some(),
            "Session creation resolves its model"
        );
        let MethodResult::Tree { nodes, .. } = call(
            &connection,
            105,
            Method::SessionTree {
                session_id: id.clone(),
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("tree");
        };
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            f.manager
                .probe(&session.active_conversation_id)
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let MethodResult::Deletion {
            result: Deletion::Preview { preview },
        } = call(
            &connection,
            106,
            Method::SessionDeletePreview {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!("preview");
        };
        assert!(matches!(
            call(
                &connection,
                107,
                Method::SessionDelete {
                    session_id: id.clone(),
                    expected_target_revision: preview.target_revision
                }
            )
            .await,
            MethodResult::Deletion {
                result: Deletion::Deleted { .. }
            }
        ));
        let absent = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(108),
                call: Method::SessionRead { session_id: id },
            })
            .await;
        assert!(matches!(
            absent,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::UnknownSession { .. }),
                    ..
                },
                ..
            })
        ));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trace_reads_are_read_only_and_reconnect_repairs_the_same_native_facts() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        call(
            &connection,
            900,
            Method::TurnStart {
                target: target.clone(),
                content: (input("request-A"))
                    .into_iter()
                    .map(|block| match block {
                        crate::message::types::UserContentBlock::Text(text) => {
                            crate::app_server::protocol::UserInputBlock::Text(text)
                        }
                        _ => panic!("client fixtures must use text or issued receipts"),
                    })
                    .collect(),
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        // Join the real source-preparation boundary. The provider is parked,
        // so no execution or synchronous publication owner can advance while
        // we prove that the read routes themselves preserve the native cut.
        let native = f.manager.configuration_host(&target.session_id).unwrap();
        native.inner.park_projection_worker();
        let before = call(
            &connection,
            901,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: target.clone(),
            },
        )
        .await;
        let read = call(
            &connection,
            902,
            Method::Trace {
                records: Vec::new(),
                target: target.clone(),
                before: None,
                limit: 32,
            },
        )
        .await;
        let MethodResult::Trace { page } = read else {
            panic!("Trace page");
        };
        assert!(
            page.records
                .iter()
                .any(|entry| entry.kind == crate::observation::trace::TraceKind::Request)
        );
        assert!(matches!(
            rejected(
                &connection,
                Method::Trace {
                    records: Vec::new(),
                    target: target.clone(),
                    before: None,
                    limit: 0
                }
            )
            .await,
            ErrorData::InvalidParams
        ));
        let after = call(
            &connection,
            903,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: target.clone(),
            },
        )
        .await;
        let (
            MethodResult::Checkpoint {
                checkpoint: checkpoint_5,
            },
            MethodResult::Checkpoint {
                checkpoint: checkpoint_6,
            },
        ) = (&before, &after)
        else {
            panic!("snapshots");
        };
        let before_view = &checkpoint_5.snapshot;
        let before_cursor = &checkpoint_5.cursor;
        let after_view = &checkpoint_6.snapshot;
        let after_cursor = &checkpoint_6.cursor;
        assert_eq!(
            before_view, after_view,
            "Trace reads change no Session or Trace field"
        );
        assert_eq!(before_cursor, after_cursor, "Trace reads publish no cursor");
        connection.close();
        let repaired = AppServerConnection::new(f.host.clone());
        initialize(&repaired).await;
        let repaired_target = attach(&repaired, &f, 0).await;
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_7,
        } = after
        else {
            panic!("snapshot");
        };
        let continuous = Box::new(checkpoint_7.snapshot);
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_8,
        } = call(
            &repaired,
            904,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: repaired_target.clone(),
            },
        )
        .await
        else {
            panic!("snapshot");
        };
        let reconnected = Box::new(checkpoint_8.snapshot);
        assert_eq!(
            continuous, reconnected,
            "reconnect preserves the parked native cut"
        );
        native.inner.unpark_projection_worker();
        f.gates[0].release();
        loop {
            if let NotificationMethod::Event { event, .. } =
                repaired.next_notification().await.notification
                && event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
            {
                break;
            }
        }
        native.inner.park_projection_worker();
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_9,
        } = call(
            &repaired,
            905,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: repaired_target,
            },
        )
        .await
        else {
            panic!("snapshot");
        };
        let settled = Box::new(checkpoint_9.snapshot);
        repaired.close();
        let final_connection = AppServerConnection::new(f.host.clone());
        initialize(&final_connection).await;
        let final_target = attach(&final_connection, &f, 0).await;
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_10,
        } = call(
            &final_connection,
            906,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: final_target,
            },
        )
        .await
        else {
            panic!("snapshot");
        };
        let final_snapshot = Box::new(checkpoint_10.snapshot);
        assert_eq!(
            settled, final_snapshot,
            "terminal reconnect preserves the parked native cut"
        );
        native.inner.unpark_projection_worker();
        final_connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One native source proves both lineage destinations and Retry through the public protocol.
async fn completed_response_cut_is_shared_by_branch_and_fork_and_distinct_from_retry() {
    Box::pin(bounded(async {
        use crate::durable::{SqliteConversationStore};
        use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
        use crate::local_runtime::session::LineageSide;
        use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, UserContentBlock, UserSource};
        use crate::message::TextBlock;
        use crate::model::finish::ModelFinishReason;
        use crate::runtime::identity::{AttemptId, EventId, MessageId};
        let f = Fixture::with_session_count(None, 1).await;
        let source = &f.sessions[0];
        let controller = f.manager.session_controller();
        let access = controller.acquire_session(&source.id, None).await.unwrap();
        let store = SqliteConversationStore::open(source.active_conversation_id.clone(), &access.database_path).unwrap();
        let event = |attempt: &str, kind| RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
 schema_version: 1, event_id: EventId::new(format!("lineage-event-{}", store.presentation_frontier().unwrap() + 1)), sequence: 0,
            conversation_id: source.active_conversation_id.clone(), attempt_id: Some(AttemptId::new(attempt)), turn_id: None,
            timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(), event: kind,
        };
        for (input, output, attempt) in [("user-a", "assistant-a", "a"), ("user-b", "assistant-b", "b")] {
            let accepted = store.accept_inbound(crate::durable::InboundDraft {
                message_id: Some(MessageId::new(input)), source: UserSource::Human, kind: InboundKind::Message,
                content: vec![UserContentBlock::Text(TextBlock { text: input.into() })],
                timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(), correlation: None,
            }).unwrap();
            store.append_event(event(attempt, RuntimeEvent::AttemptStarted { attempt_id: AttemptId::new(attempt) })).unwrap();
            store.adopt_pending_batch(accepted.sequence, Some(AttemptId::new(attempt))).unwrap();
            crate::runtime_client::response::tests::request(&store, attempt, 0, Some(crate::model::ModelUsage { input_tokens: Some(100), output_tokens: Some(20), total_tokens: Some(120), details: None }));
            store.append_canonical_with_event(&MessageBlock::Assistant(AssistantMessageBlock { id: MessageId::new(output),
                content: vec![AssistantContentBlock::Text(TextBlock { text: output.into() })] }),
                event(attempt, RuntimeEvent::AssistantMessageCommitted { message_id: MessageId::new(output) })).unwrap();
            store.append_event(event(attempt, RuntimeEvent::AttemptCompleted { attempt_id: AttemptId::new(attempt), finish_reason: ModelFinishReason::Stop })).unwrap();
        }
        let revision = store.message_append_revision(&MessageId::new("assistant-b")).unwrap().unwrap();
        let original = store.load_canonical().unwrap();
        drop(access);
        let connection = AppServerConnection::new(f.host.clone()); initialize(&connection).await;
        for (index, fork) in [false, true].into_iter().enumerate() {
            let method = if fork { Method::SessionFork { session_id: source.id.clone(), node_id: Some(source.active_node.clone()), surface_revision: revision, boundary: Some(CanonicalContentLocator::ByMessageId {message_id:MessageId::new("assistant-b")}), side: LineageSide::After } }
                else { Method::SessionBranch { session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: revision, boundary: CanonicalContentLocator::ByMessageId {message_id:MessageId::new("assistant-b")}, side: LineageSide::After } };
            let MethodResult::SessionTransition { session, editor, .. } = call(&connection, 4000 + i64::try_from(index).unwrap(), method).await else { panic!("lineage transition") };
            assert_eq!(session.id == source.id, !fork);
            assert!(editor.is_none());
            let destination = controller.acquire_session(&session.id, Some(&session.active_node)).await.unwrap();
            let copied = SqliteConversationStore::open_existing(session.active_conversation_id, &destination.database_path).unwrap();
            let messages = copied.load_canonical().unwrap(); assert_eq!(messages.len(), 4);
            assert!(matches!(&messages[3], MessageBlock::Assistant(a) if a.content == match &original[3] { MessageBlock::Assistant(a) => a.content.clone(), _ => unreachable!() }));
            let mut projected = crate::observation::transcript::transcript_page_view(copied.load_transcript_page(None, 64).unwrap()).unwrap();
            crate::runtime_client::response::native_oracle_decorate(&copied, &mut projected).unwrap();
            let tail = projected.entries.last().unwrap().completed_response.clone().unwrap();
            assert_eq!(tail.closing_message_id, Some(crate::conversation::message_id_of(&messages[3])));
            assert_eq!(tail.retry_message_id, Some(crate::conversation::message_id_of(&messages[2])));
            assert_eq!(tail.origin.conversation_id, source.active_conversation_id);
            assert_eq!(tail.usage.as_ref().unwrap().total_tokens, Some(120));
            assert_eq!(tail.timing.as_ref().unwrap().generation_ms, Some(1280));
            assert_eq!(projected.entries.iter().filter(|entry| entry.completed_response.is_some()).count(), 2);
            // As a Harness fork folds its copied prefix, the child's totals and
            // context reading include both inherited turns, without owning them.
            let totals = projected.statistics.unwrap();
            assert_eq!((totals.turns, totals.model_requests, totals.requests_with_usage, totals.completed_responses), (2, 2, 2, 2));
            assert_eq!(totals.reported_usage.unwrap().total_tokens, Some(240));
            assert!(totals.latest_turn.is_none());
            assert_eq!(crate::context::occupancy::read(&copied, 0).unwrap().unwrap().input_tokens, 100);
            assert!(copied.read_events(None, 128).unwrap().events.is_empty());
            let child_id = session.id.clone(); let child_node = session.active_node.clone();
            drop(copied); drop(destination);
            let MethodResult::Attached { target: child_target, .. } = call(&connection, 4198, Method::SessionAttach { session_id: child_id.clone(), node_id: Some(child_node.clone()) }).await else { panic!("attach inherited lineage") };
            let MethodResult::Checkpoint {checkpoint:attached}=call(&connection,41980,Method::SessionCheckpoint{target:child_target.clone(),trace_records:vec![]}).await else {panic!("checkpoint")};
            assert_eq!(attached.snapshot.transcript.entries.last().unwrap().completed_response.as_ref(), Some(&tail));
            call(&connection, 4199, Method::SessionDetach { target: child_target }).await;
            // Both independent and in-Session children remain native anchors on another copy.
            for again_fork in [false, true] {
                let method = if again_fork { Method::SessionFork { session_id: child_id.clone(), node_id: Some(child_node.clone()), surface_revision: tail.surface_revision, boundary: Some(tail.closing_locator.clone()), side: LineageSide::After } }
                    else { Method::SessionBranch { session_id: child_id.clone(), node_id: child_node.clone(), surface_revision: tail.surface_revision, boundary: tail.closing_locator.clone(), side: LineageSide::After } };
                let MethodResult::SessionTransition { editor, .. } = call(&connection, 4200, method).await else { panic!("inherited continuation") };
                assert!(editor.is_none());
            }
            let MethodResult::SessionTransition { session: retry, editor: Some(input), .. } = call(&connection, 4201, Method::SessionBranch { session_id: child_id.clone(), node_id: child_node.clone(), surface_revision: tail.surface_revision, boundary: tail.retry_locator.clone().unwrap(), side: LineageSide::Before }).await else { panic!("inherited retry") };
            let input=read_editor(&connection,input).await;
        assert_eq!(input, vec![UserInputBlock::Text(TextBlock { text: "user-b".into() })]);
            let retry_access = controller.acquire_session(&retry.id, Some(&retry.active_node)).await.unwrap();
            let retry_store = SqliteConversationStore::open_existing(retry.active_conversation_id, &retry_access.database_path).unwrap();
            assert_eq!(retry_store.load_canonical().unwrap().len(), 2);
            let child = controller.acquire_session(&child_id, Some(&child_node)).await.unwrap();
            let reopened = SqliteConversationStore::open_existing(child.node.conversation_id.clone(), &child.database_path).unwrap();
            assert_eq!(reopened.load_canonical().unwrap(), messages);
            let mut reopened_page = crate::observation::transcript::transcript_page_view(reopened.load_transcript_page(None, 64).unwrap()).unwrap();
            crate::runtime_client::response::native_oracle_decorate(&reopened, &mut reopened_page).unwrap();
            assert_eq!(reopened_page.entries.last().unwrap().completed_response.as_ref(), Some(&tail));
            rejected(&connection, Method::SessionBranch { session_id: child_id, node_id: child_node, surface_revision: crate::conversation::SurfaceRevision::new(tail.surface_revision.get() + 1), boundary: tail.closing_locator, side: LineageSide::After }).await;

        }
        let MethodResult::SessionTransition { session, editor: Some(input), .. } = call(&connection, 4010, Method::SessionBranch {
            session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: revision, boundary: CanonicalContentLocator::ByMessageId {message_id:MessageId::new("user-b")}, side: LineageSide::Before,
        }).await else { panic!("retry cut") };
        let input=read_editor(&connection,input).await;
        assert_eq!(input, vec![UserInputBlock::Text(TextBlock { text: "user-b".into() })]);
        let destination = controller.acquire_session(&session.id, Some(&session.active_node)).await.unwrap();
        let retry = SqliteConversationStore::open_existing(session.active_conversation_id, &destination.database_path).unwrap();
        assert_eq!(retry.load_canonical().unwrap().len(), 2);
        // Regenerate keeps only the prefix before the retried input: one turn.
        let mut retried = crate::observation::transcript::transcript_page_view(retry.load_transcript_page(None, 64).unwrap()).unwrap();
        crate::runtime_client::response::native_oracle_decorate(&retry, &mut retried).unwrap();
        let totals = retried.statistics.unwrap();
        assert_eq!((totals.turns, totals.model_requests, totals.completed_responses), (1, 1, 1));
        assert_eq!(totals.reported_usage.unwrap().total_tokens, Some(120));
        rejected(&connection, Method::SessionBranch { session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: crate::conversation::SurfaceRevision::new(revision.get() + 1), boundary: CanonicalContentLocator::ByMessageId {message_id:MessageId::new("assistant-b")}, side: LineageSide::After }).await;
        rejected(&connection, Method::SessionBranch { session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: revision, boundary: CanonicalContentLocator::ByMessageId {message_id:MessageId::new("user-b")}, side: LineageSide::After }).await;
        assert_eq!(store.load_canonical().unwrap(), original);
        drop(destination); drop(retry); drop(store); f.close().await;
    })).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_history_is_readable_before_and_during_runtime_preparation() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = std::sync::Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let read = || Method::SessionHistory {
            session_id: f.sessions[0].id.clone(),
            node_id: None,
            at: crate::durable::reading::ConversationWindowAt::Latest,
            limit: 32,
        };
        let MethodResult::SessionHistory {
            conversation_id,
            window,
        } = call(&connection, 500, read()).await
        else {
            panic!("history");
        };
        assert_eq!(conversation_id, id);
        assert_eq!(window.cut.conversation_id, id);
        assert!(window.page.entries.is_empty());
        let captured = window.clone();
        assert_eq!(
            probe.compositions.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(connection.attachment_counts(), (0, 0));
        probe.before_compose.arm();
        let worker = connection.clone();
        let session = f.sessions[0].clone();
        let attaching = tokio::spawn(async move { attach_session(&worker, &session).await });
        probe.before_compose.entered().await;
        assert_eq!(
            rejected(&connection, read()).await,
            ErrorData::StaleRuntime,
            "an allocated Loading owner cannot silently become a cold Store-head read"
        );
        assert_eq!(
            window, captured,
            "the already-captured nonactivated native page remains unchanged"
        );
        assert_eq!(window.cut.conversation_id, id);
        assert!(!attaching.is_finished());
        assert_eq!(connection.attachment_counts(), (0, 1));
        probe.before_compose.release();
        let target = attaching.await.unwrap();
        assert_eq!(target.conversation_id, id);
        let MethodResult::SessionHistory {
            conversation_id,
            window: installed,
        } = call(&connection, 502, read()).await
        else {
            panic!("genuine installed-owner history cut");
        };
        assert_eq!(conversation_id, id);
        assert_eq!(installed.cut.conversation_id, id);
        assert!(installed.page.entries.is_empty());
        assert!(f.provider.request_bodies().is_empty());
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t13_t15_editor_and_origin_history_use_finite_native_ledger_identity() {
    bounded(async {
        use crate::durable::SqliteConversationStore;
        use crate::local_runtime::session::LineageSide;
        use crate::message::types::{InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
        use crate::runtime::identity::MessageId;
        let fixture = Fixture::with_session_count(None, 1).await;
        let source = &fixture.sessions[0];
        let controller = fixture.manager.session_controller();
        let access = controller.acquire_session(&source.id, None).await.unwrap();
        let store = SqliteConversationStore::open(source.active_conversation_id.clone(), &access.database_path).unwrap();
        let original_id = MessageId::new("original-id:".to_owned() + &"水".repeat(400_000));
        let text = "🙂water\n".repeat(40_000);
        store.append_canonical(&MessageBlock::User(UserMessageBlock {
            id: original_id.clone(), source:UserSource::Human, kind:InboundKind::Message, timestamp:None,
            content:vec![UserContentBlock::Text(crate::message::TextBlock { text:text.clone() })],
        })).unwrap();
        let header = store.message_header(&original_id).unwrap().unwrap();
        let locator = CanonicalContentLocator::ByLedgerPosition { position:header.ledger_position.unwrap() };
        let revision = store.observation_read_cut().unwrap().surface_revision;
        drop(store);
        drop(access);
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let receipt = call(&connection, 46_700, Method::SessionFork {
            session_id:source.id.clone(), node_id:None, surface_revision:revision,
            boundary:Some(locator.clone()), side:LineageSide::Before,
        }).await;
        assert!(serde_json::to_vec(&receipt).unwrap().len() < 8192);
        let MethodResult::SessionTransition { session, editor:Some(editor), .. } = receipt else { panic!("bounded fork receipt"); };
        let (_retained, origin) = controller.acquire_read_origin(&session.id, &session.active_node).await.unwrap();
        assert_eq!(origin.source_locator, locator);
        let recovered = read_editor(&connection, editor.clone()).await;
        assert_eq!(recovered, vec![crate::local_runtime::session::uploads::UserInputBlock::Text(crate::message::TextBlock {text})]);
        let MethodResult::SessionHistory { window, .. } = call(&connection, 46_701, Method::SessionNodeOriginHistory {
            session_id:session.id.clone(), node_id:session.active_node.clone(), limit:32,
        }).await else { panic!("finite original history anchor"); };
        assert!(serde_json::to_vec(&window).unwrap().len() <= 786_432);
        assert!(window.page.content.iter().chain(window.page.deferred_entries.iter().flat_map(|entry| entry.content.iter())).any(|entry| matches!(&entry.content_ref.source, crate::observation::content::ContentSource::CanonicalMessageBlock { locator: found, .. } if found == &locator)));
        let mut foreign = editor;
        foreign.source_node = session.active_node;
        rejected(&connection, Method::SessionEditorRead { locator:foreign, block_index:0, offset:0, max_bytes:1 }).await;
        fixture.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projection_history_and_activation_fork_cross_native_app_server_boundary() {
    bounded(async {
        use crate::context::checkpoint::ProjectionActivationId;
        use crate::context::resources::LedgerFrontier;
        use crate::durable::SqliteConversationStore;
        use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
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
                id: MessageId::new("canonical-source"),
                content: input("immutable execution content"),
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            }))
            .unwrap();
        for (id, body) in [
            ("P1", "first projected summary"),
            ("P2", "second projected summary"),
        ] {
            store
                .commit_projection(
                    crate::scripted_suites::support::context::checkpoint_commit_fixture(
                        &store,
                        crate::scripted_suites::support::context::checkpoint_fixture(
                            &store, id, 1, body,
                        ),
                        None,
                    ),
                )
                .unwrap();
        }
        drop(store);
        drop(access);
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let MethodResult::Projections { page } = call(
            &connection,
            30,
            Method::Projections {
                target: target.clone(),
                after: ProjectionActivationId(0),
                limit: 16,
            },
        )
        .await
        else {
            panic!("native timeline")
        };
        assert_eq!(page.records.len(), 2);
        assert!(
            page.next_activation.is_none(),
            "the terminal historical page has no phantom continuation"
        );
        assert_eq!(
            page.records[0].activation_ledger,
            page.records[1].activation_ledger
        );
        assert!(
            page.records
                .iter()
                .all(|p| p.destination_installation.is_none())
        );
        for (index, expected) in [
            (0, "first projected summary"),
            (1, "second projected summary"),
        ] {
            let MethodResult::ProjectionDetail { detail } = call(
                &connection,
                40,
                Method::ProjectionDetail {
                    target: target.clone(),
                    checkpoint_id: page.records[index].checkpoint_id.clone(),
                    offset: 0,
                    limit: 16,
                },
            )
            .await
            else {
                panic!("typed detail")
            };
            assert_eq!(detail.items.len(), 1);
            assert_eq!(detail.items[0].occurrence.item_index, 0);
            assert!(serde_json::to_string(&detail).unwrap().contains(expected));
            let MethodResult::SessionTransition { session, .. } = call(
                &connection,
                50,
                Method::SessionForkProjection {
                    session_id: source.id.clone(),
                    node_id: Some(source.active_node.clone()),
                    ledger: LedgerFrontier(1),
                    activation: page.records[index].activation_id,
                },
            )
            .await
            else {
                panic!("activation fork")
            };
            let access = controller.acquire_session(&session.id, None).await.unwrap();
            let child = SqliteConversationStore::open_existing(
                session.active_conversation_id.clone(),
                &access.database_path,
            )
            .unwrap();
            let inherited = child.projection_head().unwrap().unwrap();
            assert!(inherited.activation.destination_installation.is_some());
            assert_eq!(
                inherited.activation.origin.conversation_id,
                source.active_conversation_id
            );
            assert!(
                serde_json::to_string(&child.projected_history().unwrap().messages.to_vec())
                    .unwrap()
                    .contains(expected)
            );
            assert!(
                child
                    .read_events(None, 64)
                    .unwrap()
                    .events
                    .iter()
                    .all(|e| !matches!(
                        e.event,
                        crate::events::RuntimeEvent::CompactionCompleted { .. }
                    ))
            );
        }
        assert!(
            f.provider.request_bodies().is_empty(),
            "historical reads and forks start no model request"
        );
        f.close().await;
    })
    .await;
}
