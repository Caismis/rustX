//! Public delivery discovery remains bounded independently of original Tool C.
use super::*;
use crate::durable::SqliteConversationStore;
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
    ToolCallOccurrenceRef, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::observation::content::CanonicalContentLocator;
use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
use base64::Engine as _;
use rustx::durable::{CanonicalStore, JournalStore, ObservationStore};

const DELIVERY_SECRET: &str = "bounded-delivery-secret-00000000000000000000000000000";

async fn granted_connection(
    fixture: &Fixture,
    ws: bool,
    grant: CancellationToken,
) -> (driver::Driver, tokio::task::JoinHandle<io::Result<()>>) {
    let host = fixture.host.clone();
    if ws {
        host.bind_delivery_access(Some(
            websocket::Credential::new(DELIVERY_SECRET.into()).unwrap(),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            websocket::connection(
                listener.accept().await.unwrap().0,
                host,
                websocket::Credential::new(driver::TOKEN.into()).unwrap(),
                CancellationToken::new(),
            )
            .await
        });
        let offered = format!(
            "rustx.app-server.v57, rustx-token.{}, rustx-delivery-access.{DELIVERY_SECRET}",
            driver::TOKEN
        );
        (driver::websocket_offering(&url, &offered).await, server)
    } else {
        let (client, server) = tokio::io::duplex(65_536);
        let (reader, writer) = tokio::io::split(server);
        let connection = Arc::new(AppServerConnection::with_delivery_access(host, grant));
        let server = tokio::spawn(stdio::serve(
            connection,
            reader,
            writer,
            CancellationToken::new(),
        ));
        let (reader, writer) = tokio::io::split(client);
        (driver::jsonl(reader, writer), server)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t11_t12_t14_t19_numeric_delivery_carrier_keeps_real_stdio_and_websocket_bounded() {
    Box::pin(bounded(async {
        for ws in [false, true] {
            let fixture = Fixture::with_session_count(None, 1).await;
            let session = &fixture.sessions[0];
            let file_path = fixture.workspaces[0].join("原始 report.md");
            let original_bytes = b"# Original protected bytes\r\n";
            std::fs::write(&file_path, original_bytes).unwrap();
            let file = crate::tools::session_files::declare(
                &fixture.workspaces[0], &session.active_conversation_id, "原始 report.md", Some("Original metadata".into()),
            ).unwrap();
            let access = fixture.manager.session_controller().acquire_session(&session.id, None).await.unwrap();
            let store = SqliteConversationStore::open(session.active_conversation_id.clone(), &access.database_path).unwrap();
            // Exercise a genuinely large retained aggregate while every immutable
            // model input remains admissible under the native per-record limit.
            let mut originals = Vec::new();
            for index in 0..5 {
                let message = MessageBlock::User(UserMessageBlock {
                    id: MessageId::new(format!("bounded-delivery-retained-{index}")),
                    source: UserSource::Human, kind: InboundKind::Message, timestamp: None,
                    content: vec![UserContentBlock::Text(crate::message::content::TextBlock {text:"retained original 水🙂".repeat(35_000)})],
                });
                assert!(serde_json::to_vec(&message).unwrap().len() < crate::context::resources::ResourceLimits::default().record_bytes);
                store.append_canonical(&message).unwrap();
                originals.push(message);
            }
            let proposal = MessageId::new("bounded-delivery-proposal");
            let call_id = ToolCallId::new("bounded-delivery-call");
            let tool_id = ToolId::new("native-present");
            store.append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
                id: proposal.clone(),
                content: vec![AssistantContentBlock::ToolCall(crate::tools::ToolCall {
                    id: call_id.clone(), tool_id: tool_id.clone(), name: "present".into(), arguments: serde_json::json!({"files":[{"path":file.path}]}),
                })],
            })).unwrap();
            let original_id = MessageId::new("original-delivery🙂水".repeat(100));
            let original = MessageBlock::Tool(ToolMessageBlock {
                id: original_id.clone(), occurrence: ToolCallOccurrenceRef::new(proposal, ContentBlockIndex::new(0)), tool_call_id: call_id, tool_id,
                result: crate::tools::types::ToolExecutionResult {
                    status: crate::tools::types::ToolExecutionStatus::Success,
                    content: vec![crate::tools::types::ToolResultContent::Text(crate::message::content::TextBlock { text: "original result 水🙂".repeat(30_000) })],
                    duration_ms: 1, exit_code: None, artifacts: vec![], deliveries: vec![file.clone(); 9], truncation: None, workflow: None, managed_output: None,
                },
            });
            assert!(original_id.as_str().len() > 256 && original_id.as_str().len() <= 4096);
            assert!(serde_json::to_vec(&original).unwrap().len() < crate::context::resources::ResourceLimits::default().record_bytes);
            originals.push(original.clone());
            assert!(originals.iter().map(|message|serde_json::to_vec(message).unwrap().len()).sum::<usize>() > 4 * 1_048_576);
            store.append_canonical(&original).unwrap();
            let original_cut = store.observation_read_cut().unwrap();
            let locator = CanonicalContentLocator::ByLedgerPosition { position: original_cut.ledger_through };
            drop(store);
            drop(access);

            let grant = CancellationToken::new();
            let (client, serving) = granted_connection(&fixture, ws, grant.clone()).await;
            initialize(&client).await;
            let target = attach(&client, &fixture).await;
            // The supervisor publishes its genuine idle adoption gate after
            // attachment. Capture the read-purity baseline after that native
            // readiness fact, rather than racing initialization.
            let checkpoint = loop {
                let MethodResult::Checkpoint { checkpoint } = call(&client, 46_790, Method::SessionCheckpoint { target: target.clone(), trace_records: vec![] }).await else { panic!("checkpoint"); };
                if checkpoint.snapshot.configuration_adoption_eligibility
                    == crate::runtime::adoption::AdoptionEligibility::Eligible
                {
                    break checkpoint;
                }
            };
            let descriptor = checkpoint.snapshot.message_descriptors.iter().find(|descriptor| descriptor.locator == locator).unwrap();
            assert!(descriptor.identity.is_none(), "giant original identity is deferred, never replaced");
            let head = descriptor.deliveries.as_ref().expect("positive successful C delivery head");
            assert_eq!((head.total, head.items.len(), head.next_offset), (9, 8, Some(8)));
            assert!(serde_json::to_vec(&head).unwrap().len() < 16_384);
            let MethodResult::CheckpointPage { page } = call(&client, 46_791, Method::ConversationCheckpointPage { target: target.clone(), agent_id: None, cut: checkpoint.read_cut.clone(), domain: head.domain.clone(), offset: 0, limit: 8 }).await else { panic!("delivery page"); };
            assert_eq!(page.read_cut, checkpoint.read_cut);
            assert_eq!((page.total, page.items.len(), page.next_offset), (9, 8, Some(8)));
            let MethodResult::CheckpointPage { page: last } = call(&client, 46_792, Method::ConversationCheckpointPage { target: target.clone(), agent_id: None, cut: checkpoint.read_cut.clone(), domain: head.domain.clone(), offset: page.next_offset.unwrap(), limit: 8 }).await else { panic!("advancing delivery page"); };
            assert_eq!(last.read_cut, checkpoint.read_cut);
            assert_eq!((last.total, last.items.len(), last.next_offset), (9, 1, None));
            let item: crate::observation::delivery::RuntimeClientDeliveryItem = crate::observation::bounded::decode_domain_value(last.items[0].value.clone().unwrap()).unwrap();
            assert_eq!(item.index, 8);
            assert_eq!(item.locator, locator);
            let mut metadata = Vec::new();
            let mut offset = 0;
            let mut request = 46_800;
            loop {
                let MethodResult::CheckpointValueChunk { chunk } = call(&client, request, Method::ConversationCheckpointValueRead {
                    target: target.clone(), agent_id: None, cut: checkpoint.read_cut.clone(), domain: item.metadata_domain.clone(), item_offset: 0, path: vec![], offset, max_bytes: 17,
                }).await else { panic!("selected metadata"); };
                assert_eq!(chunk.cut, checkpoint.read_cut);
                assert_eq!(chunk.offset, offset);
                assert_eq!(chunk.readable_end, item.encoded_bytes);
                let bytes = base64::engine::general_purpose::STANDARD.decode(chunk.bytes_base64).unwrap();
                assert!(!bytes.is_empty() && bytes.len() <= 17);
                metadata.extend(bytes);
                let Some(next) = chunk.next_offset else { break };
                assert!(next > offset);
                offset = next;
                request += 1;
            }
            assert_eq!(serde_json::from_slice::<crate::tools::session_files::SessionFileReference>(&metadata).unwrap(), file);
            let read = Method::DeliveryRead { target: target.clone(), locator: locator.clone(), delivery_index: usize::try_from(item.index).unwrap() };
            let MethodResult::SessionFileBytes { file: selected, data } = call(&client, 46_900, read.clone()).await else { panic!("protected original file"); };
            assert_eq!(selected, file);
            assert_eq!(base64::engine::general_purpose::STANDARD.decode(data).unwrap(), original_bytes);
            if ws { fixture.host.bind_delivery_access(None); } else { grant.cancel(); }
            let Response::Failure(failure) = client.request(Request { jsonrpc: JsonRpcVersion::V2, id: RequestId::Integer(46_901), call: read }).await else { panic!("revoked file authority"); };
            assert_eq!(failure.id, Some(RequestId::Integer(46_901)));
            assert_eq!(failure.error.data, Some(ErrorData::SessionFileRead { reason: crate::tools::session_files::SessionFileReadFailure::Unauthorized }));
            assert!(matches!(call(&client, 46_902, Method::ServerInfo {}).await, MethodResult::ServerInfo { .. }));
            let MethodResult::Checkpoint { checkpoint: after } = call(&client, 46_903, Method::SessionCheckpoint { target: target.clone(), trace_records: vec![] }).await else { panic!("same observation after revoke"); };
            assert_eq!(after, checkpoint);
            assert!(fixture.provider.request_bodies().is_empty(), "observation never starts model work");
            let access = fixture.manager.session_controller().acquire_read(&session.id, Some(&session.active_node)).await.unwrap();
            let retained = SqliteConversationStore::open_existing(session.active_conversation_id.clone(), &access.database_path).unwrap();
            assert_eq!(retained.observation_read_cut().unwrap(), *checkpoint.read_cut.source_cut.as_ref().unwrap());
            for original in &originals {
                assert_eq!(retained.load_messages(std::slice::from_ref(original.id())).unwrap(), vec![original.clone()]);
            }
            assert!(retained.read_events(None, 32).unwrap().events.iter().all(|entry| !matches!(entry.event, crate::events::types::RuntimeEvent::AttemptStarted { .. } | crate::events::types::RuntimeEvent::ToolExecutionStarted { .. } | crate::events::types::RuntimeEvent::ToolExecutionCompleted { .. })), "no fabricated execution facts");
            drop(retained); drop(access);
            client.close().await;
            // The driver deliberately drops its socket rather than negotiating Close.
            match serving.await.unwrap() {
                Ok(()) => {},
                Err(error) if ws && matches!(error.get_ref().and_then(|error| error.downcast_ref::<tokio_tungstenite::tungstenite::Error>()), Some(tokio_tungstenite::tungstenite::Error::Protocol(tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake))) => {},
                Err(error) => panic!("carrier teardown: {error}"),
            }
            fixture.close().await;
        }
    })).await;
}
