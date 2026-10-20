//! Direct connection contracts. Provider gates prove overlap; no socket timing.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::{Fixture, FixturePhase, bounded, input};
use crate::app_server::connection::AppServerConnection;
use crate::app_server::protocol::*;
use crate::runtime_client::content::CanonicalContentLocator;
use crate::runtime_client::event::RuntimeClientEvent;

use super::app_server_conformance as conformance;

fn trace_content_references(
    value: &serde_json::Value,
    output: &mut Vec<crate::runtime_client::content::ContentRef>,
) {
    match value {
        serde_json::Value::Object(fields) => {
            if let Some(reference) = fields.get("content_ref") {
                output.push(
                    crate::runtime_client::structural_json::decode(reference.clone()).unwrap(),
                );
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
            .find(|record| record.kind == crate::runtime_client::trace::TraceKind::Request)
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
                    == crate::runtime_client::content::ContentRepresentation::Utf8Text
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

// Wait on the catalog's native publication signal, not on elapsed time or retries.
async fn activity_published(f: &Fixture, id: &crate::local_runtime::session::SessionId) {
    let mut changes = f.manager.sessions.summary_invalidations().changes();
    loop {
        let catalog = f.manager.sessions.catalog.lock().await;
        let expected = catalog.activity_subject(id).unwrap().unwrap();
        if catalog.summary(id).unwrap().updated_at >= expected {
            return;
        }
        drop(catalog);
        changes.changed().await.unwrap();
    }
}

async fn binary_upload(
    connection: &AppServerConnection,
    target: AttachmentTarget,
    name: &str,
    bytes: &[u8],
) -> Vec<crate::local_runtime::session::uploads::UploadedFile> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let operation_id = uuid::Uuid::new_v4().simple().to_string();
    let MethodResult::UploadPrepared { transfer } = call(
        connection,
        9000,
        Method::SessionUploadPrepare {
            target: target.clone(),
            operation_id: operation_id.clone(),
            files: vec![crate::local_runtime::session::uploads::UploadMetadata {
                name: name.into(),
                size: bytes.len(),
            }],
        },
    )
    .await
    else {
        panic!("upload preparation")
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
    for chunk in bytes.chunks(crate::local_runtime::session::uploads::UPLOAD_POLICY.max_chunk_bytes)
    {
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Text("next".into())
        );
        socket
            .send(Message::Binary(chunk.to_vec().into()))
            .await
            .unwrap();
    }
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Text("finish".into())
    );
    socket.send(Message::Text("finish".into())).await.unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Text("settled".into())
    );
    let MethodResult::UploadStatus {
        outcome: crate::local_runtime::session::uploads::UploadOutcome::Ready { files },
    } = call(
        connection,
        9001,
        Method::SessionUploadStatus {
            target,
            operation_id,
        },
    )
    .await
    else {
        panic!("ready upload")
    };
    files
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_connection_runs_shared_transport_neutral_conformance() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        conformance::representative_scenario(
            &conformance::DirectDriver(&connection),
            [f.sessions[0].id.clone(), f.sessions[1].id.clone()],
        )
        .await;
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

pub(super) struct AskPolicy;
impl crate::agent::PreToolPolicy for AskPolicy {
    fn evaluate<'a>(
        &'a self,
        _: &'a crate::agent::PreToolView<'a>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<crate::agent::PreToolDecision, crate::agent::LifecycleError>,
    > {
        Box::pin(async {
            Ok(crate::agent::PreToolDecision::Ask {
                reason: "protocol regression".into(),
            })
        })
    }
}

/// Records only the existing fixture operation when its caller drops it.
struct CallDiagnostic {
    id: i64,
    method: std::mem::Discriminant<Method>,
    stage: &'static str,
    started: std::time::Instant,
    stage_started: std::time::Instant,
    schema_was_ready: bool,
    schema_init_us: Option<u128>,
    schema_validation_us: Option<u128>,
    complete: bool,
}
impl CallDiagnostic {
    fn enter(&mut self, stage: &'static str) {
        self.stage = stage;
        self.stage_started = std::time::Instant::now();
    }
}
impl Drop for CallDiagnostic {
    fn drop(&mut self) {
        if !self.complete {
            eprintln!(
                "incomplete fixture call: id={} method={:?} stage={} elapsed_us={} stage_us={} schema_was_ready={} schema_init_us={:?} schema_validation_us={:?}",
                self.id,
                self.method,
                self.stage,
                self.started.elapsed().as_micros(),
                self.stage_started.elapsed().as_micros(),
                self.schema_was_ready,
                self.schema_init_us,
                self.schema_validation_us,
            );
        }
    }
}

pub(super) async fn call(connection: &AppServerConnection, id: i64, call: Method) -> MethodResult {
    static SCHEMA: std::sync::OnceLock<jsonschema::Validator> = std::sync::OnceLock::new();
    let started = std::time::Instant::now();
    let mut diagnostic = CallDiagnostic {
        id,
        method: std::mem::discriminant(&call),
        stage: "handle response",
        started,
        stage_started: started,
        schema_was_ready: SCHEMA.get().is_some(),
        schema_init_us: None,
        schema_validation_us: None,
        complete: false,
    };
    let configuration_target = match &call {
        Method::ConfigurationGet { target } => Some(target.clone()),
        _ => None,
    };
    let response = connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .await;
    diagnostic.enter("schema initialization");
    let validator = SCHEMA.get_or_init(|| {
        jsonschema::validator_for(&crate::app_server::schema::protocol_schema()).unwrap()
    });
    diagnostic.schema_init_us = Some(diagnostic.stage_started.elapsed().as_micros());
    diagnostic.enter("schema validation");
    let wire = serde_json::to_value(&response).unwrap();
    assert!(
        validator.is_valid(&wire),
        "{:?}",
        validator.iter_errors(&wire).collect::<Vec<_>>()
    );
    diagnostic.schema_validation_us = Some(diagnostic.stage_started.elapsed().as_micros());
    let Response::Success(response) = response else {
        panic!("{response:?}")
    };
    assert_eq!(response.id, RequestId::Integer(id));
    if let MethodResult::Attached { target, .. } = &response.result {
        diagnostic.enter("attached checkpoint");
        let checkpoint_response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(id + 10_000_000),
                call: Method::SessionCheckpoint {
                    target: target.clone(),
                    trace_records: vec![],
                },
            })
            .await;
        let Response::Success(checkpoint) = checkpoint_response else {
            panic!("client checkpoint loading: {checkpoint_response:?}")
        };
        let MethodResult::Checkpoint { checkpoint } = checkpoint.result else {
            panic!("checkpoint")
        };
        diagnostic.enter("subscribe");
        assert!(matches!(
            connection
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(id + 20_000_000),
                    call: Method::SessionSubscribe {
                        target: target.clone(),
                        after_cursor: checkpoint.cursor
                    }
                })
                .await,
            Response::Success(_)
        ));
    }
    diagnostic.enter("selected fixture read");
    let result = materialize_source_fixture(connection, id, response.result).await;
    let result = if let Some(target) = configuration_target {
        diagnostic.enter("configuration fixture read");
        materialize_configuration_fixture(connection, id, target, result).await
    } else {
        result
    };
    diagnostic.complete = true;
    result
}

async fn materialize_source_fixture(
    connection: &AppServerConnection,
    id: i64,
    result: MethodResult,
) -> MethodResult {
    use base64::Engine;
    let MethodResult::SourceSettings { mut projection } = result else {
        return result;
    };
    if projection.inline.is_none() {
        let mut object = serde_json::Map::new();
        let mut read_id = id + 60_000_000;
        for field in &projection.fields {
            let value = if let Some(value) = &field.value {
                value.clone()
            } else {
                let reference = field.reference.clone().expect("source descriptor");
                let mut bytes = Vec::new();
                let mut offset = 0;
                loop {
                    let Response::Success(response) = connection
                        .handle_request(Request {
                            jsonrpc: JsonRpcVersion::V2,
                            id: RequestId::Integer(read_id),
                            call: Method::SourcesValueRead {
                                target: projection.target.clone(),
                                reference: reference.clone(),
                                offset,
                                max_bytes: 65_536,
                            },
                        })
                        .await
                    else {
                        panic!("fixture source read");
                    };
                    read_id += 1;
                    let MethodResult::SourceValueChunk { chunk } = response.result else {
                        panic!("source chunk");
                    };
                    bytes.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(chunk.data_base64)
                            .unwrap(),
                    );
                    assert!(
                        bytes.len() <= 16 * 1024 * 1024,
                        "explicit fixture source cache bound"
                    );
                    let Some(next) = chunk.next_offset else {
                        break;
                    };
                    assert!(next > offset);
                    offset = next;
                }
                serde_json::from_slice(&bytes).unwrap()
            };
            let serde_json::Value::String(name) = serde_json::to_value(field.name).unwrap() else {
                panic!("field name");
            };
            assert!(object.insert(name, value).is_none());
        }
        projection.inline = Some(Box::new(
            crate::runtime_client::structural_json::decode(serde_json::Value::Object(object))
                .unwrap(),
        ));
        // This is loaded client fixture state. Only the original bounded
        // server response above is serialized and schema-validated.
    }
    MethodResult::SourceSettings { projection }
}

async fn materialize_configuration_fixture(
    connection: &AppServerConnection,
    id: i64,
    target: AttachmentTarget,
    result: MethodResult,
) -> MethodResult {
    use base64::Engine;
    let MethodResult::EffectiveConfiguration { mut projection } = result else {
        return result;
    };
    if projection.inline.is_none() {
        let mut object = serde_json::Map::new();
        let mut read_id = id + 70_000_000;
        for field in &projection.fields {
            let value = if let Some(value) = &field.value {
                value.clone()
            } else {
                let reference = field.reference.clone().expect("configuration descriptor");
                let mut bytes = Vec::new();
                let mut offset = 0;
                loop {
                    let Response::Success(response) = connection
                        .handle_request(Request {
                            jsonrpc: JsonRpcVersion::V2,
                            id: RequestId::Integer(read_id),
                            call: Method::ConfigurationValueRead {
                                target: target.clone(),
                                reference: reference.clone(),
                                offset,
                                max_bytes: 65_536,
                            },
                        })
                        .await
                    else {
                        panic!("fixture configuration read");
                    };
                    read_id += 1;
                    let MethodResult::ConfigurationValueChunk { chunk } = response.result else {
                        panic!("configuration chunk");
                    };
                    bytes.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(chunk.data_base64)
                            .unwrap(),
                    );
                    assert!(bytes.len() <= 16 * 1024 * 1024, "fixture cache bound");
                    let Some(next) = chunk.next_offset else {
                        break;
                    };
                    assert!(next > offset);
                    offset = next;
                }
                serde_json::from_slice(&bytes).unwrap()
            };
            let serde_json::Value::String(name) = serde_json::to_value(field.name).unwrap() else {
                panic!("field name");
            };
            assert!(object.insert(name, value).is_none());
        }
        projection.inline = Some(Box::new(
            crate::runtime_client::structural_json::decode(serde_json::Value::Object(object))
                .unwrap(),
        ));
    }
    MethodResult::EffectiveConfiguration { projection }
}

pub(super) async fn read_model_result(
    connection: &AppServerConnection,
    target: &AttachmentTarget,
    result: MethodResult,
) -> Box<crate::model::session::SessionModelView> {
    use base64::Engine as _;
    let MethodResult::DomainPage { page } = result else {
        panic!("model domain page");
    };
    assert_eq!(page.domain, "model");
    assert_eq!(page.total, 1);
    assert!(page.next_offset.is_none());
    assert_eq!(page.items.len(), 1);
    let item_offset = page.items[0].offset;
    let mut bytes = Vec::new();
    let mut offset = 0;
    let mut readable_end = None;
    loop {
        let MethodResult::CheckpointValueChunk { chunk } = call(
            connection,
            46_790,
            Method::ConversationCheckpointValueRead {
                target: target.clone(),
                agent_id: None,
                cut: page.read_cut.clone(),
                domain: page.domain.clone(),
                item_offset,
                path: Vec::new(),
                offset,
                max_bytes: 65_536,
            },
        )
        .await
        else {
            panic!("selected original model value");
        };
        assert_eq!(chunk.cut, page.read_cut);
        assert_eq!(chunk.domain, page.domain);
        assert_eq!(chunk.item_offset, item_offset);
        assert!(chunk.path.is_empty());
        assert_eq!(chunk.offset, offset);
        assert_eq!(
            chunk.representation,
            crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1
        );
        assert!(chunk.bytes_base64.len() <= 87_384);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(!part.is_empty() && part.len() <= 65_536);
        if let Some(end) = readable_end {
            assert_eq!(chunk.readable_end, end);
        } else {
            readable_end = Some(chunk.readable_end);
        }
        let next = offset + u64::try_from(part.len()).unwrap();
        assert!(next <= chunk.readable_end);
        assert!(bytes.len() + part.len() <= 16 * 1024 * 1024);
        bytes.extend(part);
        if let Some(value) = chunk.next_offset {
            assert_eq!(value, next);
            assert!(value > offset && value < chunk.readable_end);
            offset = value;
        } else {
            assert_eq!(next, chunk.readable_end);
            break;
        }
    }
    Box::new(
        crate::runtime_client::structural_json::decode(serde_json::from_slice(&bytes).unwrap())
            .unwrap(),
    )
}
pub(super) async fn read_native_model_result(
    client: &crate::runtime_client::RuntimeAttachment,
    result: crate::runtime_client::RuntimeClientResult,
) -> crate::model::session::SessionModelView {
    use base64::Engine as _;
    let crate::runtime_client::RuntimeClientResult::DomainPage { page } = result else {
        panic!("bounded native model page");
    };
    assert_eq!(page.domain, "model");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.total, 1);
    let mut bytes = Vec::new();
    let mut offset = 0;
    let mut end = None;
    loop {
        let response = client
            .handle_request_async(
                crate::runtime_client::RuntimeClientRequest::CheckpointValueRead {
                    id: crate::runtime_client::RequestId::new(46_791),
                    cut: page.read_cut.clone(),
                    domain: page.domain.clone(),
                    item_offset: page.items[0].offset,
                    path: Vec::new(),
                    offset,
                    max_bytes: 65_536,
                },
            )
            .await;
        let Some(crate::runtime_client::RuntimeClientResult::CheckpointValueChunk { chunk }) =
            response.result
        else {
            panic!("original selected model value: {:?}", response.error);
        };
        assert_eq!(chunk.cut, page.read_cut);
        assert_eq!(chunk.domain, page.domain);
        assert_eq!(chunk.item_offset, page.items[0].offset);
        assert!(chunk.path.is_empty());
        assert_eq!(chunk.offset, offset);
        assert_eq!(
            chunk.representation,
            crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1
        );
        assert!(chunk.bytes_base64.len() <= 87_384);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(!part.is_empty() && part.len() <= 65_536);
        if let Some(original) = end {
            assert_eq!(chunk.readable_end, original);
        } else {
            end = Some(chunk.readable_end);
        }
        let next = offset + part.len() as u64;
        assert!(next <= chunk.readable_end);
        assert!(bytes.len() + part.len() <= 16 * 1024 * 1024);
        bytes.extend(part);
        if let Some(value) = chunk.next_offset {
            assert_eq!(value, next);
            assert!(value > offset && value < chunk.readable_end);
            offset = value;
        } else {
            assert_eq!(next, chunk.readable_end);
            break;
        }
    }
    crate::runtime_client::structural_json::decode(serde_json::from_slice(&bytes).unwrap()).unwrap()
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModelCatalogObservation {
    pub models: Vec<CatalogModelObservation>,
}

/// The current finite native catalog header, including its profile continuation facts.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CatalogModelObservation {
    pub model: crate::model::catalog::ModelRef,
    pub protocol: crate::model::types::ModelProtocol,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub context_window: u64,
    pub max_output_tokens: u32,
    pub declared_capabilities: crate::model::catalog::ModelCapabilities,
    pub effective_capabilities: crate::model::catalog::ModelCapabilities,
    pub profiles: Vec<crate::model::catalog::ModelProfileView>,
    pub default_profile: Option<crate::model::catalog::ModelProfileId>,
    pub credential_source: crate::model::catalog::CredentialSourceView,
    pub profile_count: u64,
    pub profiles_deferred: bool,
}
impl PartialEq<crate::model::catalog::ModelCatalogView> for ModelCatalogObservation {
    fn eq(&self, original: &crate::model::catalog::ModelCatalogView) -> bool {
        self.models.len() == original.models.len()
            && self
                .models
                .iter()
                .zip(&original.models)
                .all(|(observed, native)| {
                    observed.model == native.model
                        && observed.protocol == native.protocol
                        && observed.context_window == native.context_window
                        && observed.max_output_tokens == native.max_output_tokens
                        && observed.declared_capabilities == native.declared_capabilities
                        && observed.effective_capabilities == native.effective_capabilities
                        && observed.profiles == native.profiles
                        && observed.default_profile == native.default_profile
                        && observed.credential_source == native.credential_source
                        && observed.profile_count == native.profiles.len() as u64
                        && !observed.profiles_deferred
                })
    }
}
pub(super) fn read_catalog_result(result: MethodResult) -> ModelCatalogObservation {
    let MethodResult::DomainPage { page } = result else {
        panic!("catalog domain page");
    };
    assert_eq!(page.domain, "model_catalog.models");
    assert!(
        page.next_offset.is_none(),
        "this fixture's catalog fits one bounded page"
    );
    assert!(page.items.len() <= 32);
    assert_eq!(page.total, page.items.len() as u64);
    let models = page
        .items
        .into_iter()
        .map(|item| {
            let model: CatalogModelObservation = crate::runtime_client::structural_json::decode(
                item.value.expect("finite inline catalog fixture header"),
            )
            .expect("current typed catalog header");
            assert!(
                !model.profiles_deferred,
                "this small fixture's complete native profile prefix is advertised inline"
            );
            assert_eq!(model.profile_count, model.profiles.len() as u64);
            assert!(model.profiles.len() <= 32);
            if model.profile_count > 0 {
                let frontier = page
                    .collections
                    .iter()
                    .find(|frontier| {
                        frontier.domain == format!("model_catalog.models.{}.profiles", item.offset)
                    })
                    .expect("actual numeric profile source frontier");
                assert_eq!(frontier.total, model.profile_count);
                assert_eq!(frontier.next_offset, Some(0));
            }
            model
        })
        .collect();
    ModelCatalogObservation { models }
}
pub(super) fn read_native_catalog_result(
    result: crate::runtime_client::RuntimeClientResult,
) -> ModelCatalogObservation {
    let crate::runtime_client::RuntimeClientResult::DomainPage { page } = result else {
        panic!("native catalog page");
    };
    read_catalog_result(MethodResult::DomainPage { page })
}

async fn read_editor(
    connection: &AppServerConnection,
    locator: crate::app_server::editor_read::EditorLocator,
) -> Vec<crate::local_runtime::session::uploads::UserInputBlock> {
    use base64::Engine;
    let mut content = vec![];
    let mut block_index = 0;
    let mut offset = 0;
    let mut bytes = vec![];
    loop {
        let MethodResult::EditorPage { page } = call(
            connection,
            30_000_000,
            Method::SessionEditorRead {
                locator: locator.clone(),
                block_index,
                offset,
                max_bytes: 65_536,
            },
        )
        .await
        else {
            panic!("editor read")
        };
        if let Some(chunk) = page.chunk {
            bytes.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(chunk.data_base64)
                    .unwrap(),
            );
            if let Some(next) = chunk.next_offset {
                offset = next;
                continue;
            }
            content.push(
                crate::local_runtime::session::uploads::UserInputBlock::Text(
                    crate::message::content::TextBlock {
                        text: String::from_utf8(std::mem::take(&mut bytes)).unwrap(),
                    },
                ),
            );
        }
        if let Some(upload) = page.upload {
            content.push(crate::local_runtime::session::uploads::UserInputBlock::Upload(upload));
        }
        let Some(next) = page.next_block else {
            break;
        };
        block_index = next;
        offset = 0;
    }
    content
}

pub(super) async fn initialize(connection: &AppServerConnection) {
    call(
        connection,
        0,
        Method::Initialize(InitializeParams {
            protocol_version: crate::app_server::protocol::APP_SERVER_PROTOCOL_VERSION,
            client: ClientIdentity {
                name: "scripted".into(),
                version: "1".into(),
            },
            presentation: PresentationCapabilities::default(),
        }),
    )
    .await;
}

async fn rejected(connection: &AppServerConnection, method: Method) -> ErrorData {
    let Response::Failure(Failure {
        error: RpcError {
            data: Some(data), ..
        },
        ..
    }) = connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(999),
            call: method,
        })
        .await
    else {
        panic!("expected typed rejection")
    };
    data
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_during_unloading_needs_no_operation_lease() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        let probe = f.manager.probe(&old.conversation_id);
        // The attachment helper's independent checkpoint already used this gate.
        // Rearm it: a wrongly admitted detach would now park instead of passing.
        probe.before_operation.arm();
        probe.before_shutdown.arm();
        let unload = super::unload_task(&f, old.conversation_id.clone());
        probe.before_shutdown.entered().await;
        assert_eq!(
            f.manager.residency(&old.conversation_id),
            super::ResidencyState::Unloading
        );
        assert!(matches!(
            call(&connection, 1, Method::SessionDetach { target: old }).await,
            MethodResult::Detached {}
        ));
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(
            !*probe.before_operation.entered.borrow(),
            "detach admits no operation lease"
        );
        probe.before_operation.release();
        probe.before_shutdown.release();
        unload.await.unwrap().unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_detach_cannot_remove_replacement_route() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        call(
            &connection,
            1,
            Method::SessionDetach {
                target: old.clone(),
            },
        )
        .await;
        let new = attach(&connection, &f, 0).await;
        assert_ne!(old.attachment_id, new.attachment_id);
        assert_eq!(old.runtime_incarnation, new.runtime_incarnation);
        assert_eq!(
            rejected(&connection, Method::SessionDetach { target: old }).await,
            ErrorData::StaleAttachment
        );
        assert_eq!(connection.attachment_counts(), (1, 0));
        call(
            &connection,
            2,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: new,
            },
        )
        .await;
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_async_operation_drains_before_delete_releases_resources() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        use std::sync::Arc;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let identity = f.manager.load(&target.session_id, None).await.unwrap();
        let weak_runtime = identity.inspect_runtime().unwrap().weak_inner();
        let weak_host = {
            let resident = identity.resident.upgrade().unwrap();
            let composition = resident.composition.lock().unwrap();
            composition.as_ref().unwrap().host().weak_inner()
        };
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let worker = connection.clone();
        let operation_target = target.clone();
        let operation = tokio::spawn(async move {
            let response = worker
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(200),
                    call: Method::ConfigurationGet {
                        target: operation_target,
                    },
                })
                .await;
            let wire = serde_json::to_value(&response).unwrap();
            assert!(
                jsonschema::validator_for(&crate::app_server::schema::protocol_schema())
                    .unwrap()
                    .is_valid(&wire)
            );
            assert!(
                serde_json::to_vec(&wire).unwrap().len()
                    <= crate::app_server::transport::MAX_MESSAGE_BYTES
            );
            let Response::Success(receipt) = response else {
                panic!("admitted original bounded read: {response:?}");
            };
            assert_eq!(receipt.id, RequestId::Integer(200));
            receipt.result
        });
        probe.before_operation.entered().await;
        let worker = connection.clone();
        let MethodResult::Deletion {
            result: crate::app_server::session_deletion::SessionDeletionResult::Preview { preview },
        } = call(
            &connection,
            199,
            Method::SessionDeletePreview {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("preview")
        };
        let delete_target = target.clone();
        let delete = tokio::spawn(async move {
            call(
                &worker,
                201,
                Method::SessionDelete {
                    session_id: delete_target.session_id,
                    expected_target_revision: preview.target_revision,
                },
            )
            .await
        });
        probe
            .draining_operations
            .subscribe()
            .wait_for(|draining| *draining)
            .await
            .unwrap();
        assert_eq!(
            f.manager.residency(&target.conversation_id),
            super::ResidencyState::Unloading
        );
        assert!(
            !delete.is_finished(),
            "unload is waiting on the admitted operation, not the client"
        );
        assert!(matches!(
            call(
                &connection,
                202,
                Method::SessionRecoverDeletion {
                    session_id: target.session_id.clone()
                }
            )
            .await,
            MethodResult::Deletion {
                result: crate::app_server::session_deletion::SessionDeletionResult::Preview { .. }
            }
        ));
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&target.session_id)
        );
        assert!(weak_runtime.upgrade().is_some());
        assert_eq!(
            rejected(
                &connection,
                Method::TurnStart {
                    target: target.clone(),
                    content: (input("must not run"))
                        .into_iter()
                        .map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) =>
                                crate::app_server::protocol::UserInputBlock::Text(text),
                            _ => panic!("client fixtures must use text or issued receipts"),
                        })
                        .collect()
                }
            )
            .await,
            ErrorData::StaleRuntime
        );
        probe.before_operation.release();
        let MethodResult::EffectiveConfiguration { projection } = operation.await.unwrap() else {
            panic!("the admitted original configuration read succeeds during retirement");
        };
        assert!(!projection.revision.is_empty());
        assert!(
            projection.inline.is_some() || !projection.fields.is_empty(),
            "the bounded receipt retains original selected-field evidence"
        );
        assert!(matches!(
            delete.await.unwrap(),
            MethodResult::Deletion { .. }
        ));
        assert!(weak_host.upgrade().is_none());
        assert!(weak_runtime.upgrade().is_none());
        assert!(f.provider.request_bodies().is_empty());
        assert!(
            matches!(
                f.manager.sessions.delete_preview(&target.session_id).await,
                SessionDeleteResult::NotFound { .. }
            ),
            "deletion committed after all admitted operations drained"
        );
        f.close().await;
    })
    .await;
}

async fn cold_sessions(
    f: &Fixture,
    count: usize,
) -> Vec<crate::local_runtime::session::SessionSnapshot> {
    let mut sessions = Vec::new();
    for _ in 0..count {
        sessions.push(
            f.manager
                .sessions
                .create_session(
                    crate::local_runtime::session::SessionPersistentState::from_input(
                        &crate::local_runtime::configuration::SessionConfigInput::new(
                            f.workspaces[0].clone(),
                        ),
                    ),
                )
                .await
                .unwrap()
                .session,
        );
    }
    sessions
}

async fn attach_session(
    connection: &AppServerConnection,
    session: &crate::local_runtime::session::SessionSnapshot,
) -> AttachmentTarget {
    let MethodResult::Attached { target, .. } = call(
        connection,
        300,
        Method::SessionAttach {
            session_id: session.id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("attached")
    };
    target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachment_capacity_rejection_never_composes_and_detach_reclaims_without_notifications() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new_with_attachment_limit_for_test(f.host.clone(), 2);
        initialize(&connection).await;
        let sessions = cold_sessions(&f, 3).await;
        assert_eq!(connection.attachment_counts(), (0, 0));
        let mut targets = Vec::new();
        for session in &sessions[..2] {
            targets.push(attach_session(&connection, session).await);
        }
        assert_eq!(connection.attachment_counts(), (2, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 2);
        assert_eq!(
            rejected(
                &connection,
                Method::SessionAttach {
                    session_id: sessions[2].id.clone(),
                    node_id: None
                }
            )
            .await,
            ErrorData::AttachmentCapacity
        );
        assert_eq!(
            f.manager
                .probe(&sessions[2].active_conversation_id)
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            f.manager.residency(&sessions[2].active_conversation_id),
            super::ResidencyState::Unloaded
        );
        assert_eq!(connection.attachment_counts(), (2, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 2);
        for target in targets.drain(..1) {
            call(&connection, 301, Method::SessionDetach { target }).await;
        }
        assert_eq!(connection.attachment_counts(), (1, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 1);
        // No next_notification call: unload's response is the terminal acknowledgement.
        for session in &sessions[2..] {
            targets.push(attach_session(&connection, session).await);
        }
        assert_eq!(connection.attachment_counts(), (2, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 2);
        assert_eq!(
            f.manager
                .probe(&sessions[2].active_conversation_id)
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert!(f.provider.request_bodies().is_empty());
        for target in targets {
            call(&connection, 302, Method::SessionDetach { target }).await;
        }
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 0);
        for session in &sessions {
            assert_eq!(
                f.manager.residency(&session.active_conversation_id),
                super::ResidencyState::Loaded
            );
        }
        connection.close();
        assert_eq!(f.host.diagnostics().external_attachments, 0);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_final_slot_is_reserved_before_composition() {
    bounded(async {
        use std::sync::Arc;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new_with_attachment_limit_for_test(
            f.host.clone(),
            2,
        ));
        initialize(&connection).await;
        let sessions = cold_sessions(&f, 3).await;
        for session in &sessions[..1] {
            attach_session(&connection, session).await;
        }
        assert_eq!(connection.attachment_counts(), (1, 0));
        let probes = [
            f.manager.probe(&sessions[1].active_conversation_id),
            f.manager.probe(&sessions[2].active_conversation_id),
        ];
        for probe in &probes {
            probe.before_compose.arm();
        }
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let mut tasks = Vec::new();
        for (index, session) in sessions[1..].iter().enumerate() {
            let worker = connection.clone();
            let barrier = barrier.clone();
            let session_id = session.id.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                worker
                    .handle_request(Request {
                        jsonrpc: JsonRpcVersion::V2,
                        id: RequestId::Integer(400 + i64::try_from(index).unwrap()),
                        call: Method::SessionAttach {
                            session_id,
                            node_id: None,
                        },
                    })
                    .await
            }));
        }
        barrier.wait().await;
        let winner = tokio::select! {
            () = probes[0].before_compose.entered() => 0,
            () = probes[1].before_compose.entered() => 1,
        };
        assert_eq!(connection.attachment_counts(), (1, 1));
        let loser = tasks.remove(1 - winner).await.unwrap();
        assert!(matches!(
            loser,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::AttachmentCapacity),
                    ..
                },
                ..
            })
        ));
        assert_eq!(
            probes[1 - winner]
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            f.manager
                .residency(&sessions[2 - winner].active_conversation_id),
            super::ResidencyState::Unloaded
        );
        assert_eq!(connection.attachment_counts(), (1, 1));
        assert_eq!(f.host.diagnostics().external_attachments, 2);
        probes[winner].before_compose.release();
        assert!(matches!(
            tasks.remove(0).await.unwrap(),
            Response::Success(_)
        ));
        assert_eq!(connection.attachment_counts(), (2, 0));
        assert!(f.provider.request_bodies().is_empty());
        for session in &sessions {
            f.manager
                .unload(&session.active_conversation_id)
                .await
                .unwrap();
        }
        connection.close();
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 0);
        connection.close();
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 0);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_attach_releases_reservation_but_not_manager_owned_load() {
    bounded(async {
        use std::sync::Arc;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let probe = f.manager.probe(&f.id(0).await);
        probe.before_compose.arm();
        let worker = connection.clone();
        let session = f.sessions[0].clone();
        let attaching = tokio::spawn(async move { attach_session(&worker, &session).await });
        probe.before_compose.entered().await;
        assert_eq!(connection.attachment_counts(), (0, 1));
        attaching.abort();
        assert!(attaching.await.unwrap_err().is_cancelled());
        assert_eq!(connection.attachment_counts(), (0, 0));
        // Observe the already-claimed flight without initiating another load.
        let id = f.id(0).await;
        let flight = {
            let registry = f.manager.registry.0.lock().unwrap();
            let super::Entry::Loading(flight) = registry.entries.get(&id).unwrap() else {
                panic!("manager-owned Loading flight")
            };
            flight.clone()
        };
        probe.before_compose.release();
        let resident = flight.wait().await.operation_result().unwrap().unwrap();
        assert_eq!(
            f.manager.residency(resident.conversation_id()),
            super::ResidencyState::Loaded
        );
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(
            probe.compositions.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let target = attach(&connection, &f, 0).await;
        assert_eq!(target.runtime_incarnation, resident.incarnation_id());
        assert_eq!(
            probe.compositions.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        call(
            &connection,
            500,
            Method::SessionDetach {
                target: target.clone(),
            },
        )
        .await;
        let fresh = attach(&connection, &f, 0).await;
        assert_ne!(fresh.attachment_id, target.attachment_id);
        assert_eq!(fresh.runtime_incarnation, target.runtime_incarnation);
        assert_eq!(
            probe.compositions.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let target = fresh;
        probe.before_operation.arm();
        let worker = connection.clone();
        let operation_target = target.clone();
        let operation = tokio::spawn(async move {
            call(
                &worker,
                501,
                Method::ConfigurationGet {
                    target: operation_target,
                },
            )
            .await
        });
        probe.before_operation.entered().await;
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        let unload = super::unload_task(&f, target.conversation_id.clone());
        probe
            .draining_operations
            .subscribe()
            .wait_for(|v| *v)
            .await
            .unwrap();
        assert!(!unload.is_finished());
        probe.before_operation.release();
        unload.await.unwrap().unwrap();
        f.close().await;
    })
    .await;
}

pub(super) async fn attach(
    connection: &AppServerConnection,
    f: &Fixture,
    index: usize,
) -> AttachmentTarget {
    let MethodResult::Attached { target, .. } = call(
        connection,
        10 + i64::try_from(index).unwrap(),
        Method::SessionAttach {
            session_id: f.sessions[index].id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("attached")
    };
    target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn initialize_and_malformed_wire_are_transactional() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        let before = connection.handle_json(r#"{"jsonrpc":"2.0","id":0,"method":"server/info","params":{}}"#).await.unwrap();
        assert!(matches!(before, Response::Failure(Failure { error: RpcError { data: Some(ErrorData::NotInitialized), .. }, .. })));
        let bad_version = connection.handle_json(r#"{"jsonrpc":"2.0","id":"version","method":"initialize","params":{"protocol_version":12,"client":{"name":"test","version":"1"},"presentation":{"images":false,"questionnaires":false,"reviews":false}}}"#).await.unwrap();
        let Response::Failure(failure) = bad_version else { panic!("version mismatch") };
        assert_eq!(failure.id, Some(RequestId::String("version".into())));
        assert!(matches!(failure.error.data, Some(ErrorData::UnsupportedVersion { supported: APP_SERVER_PROTOCOL_VERSION, requested: 12 })));
        for requested in 0..crate::app_server::protocol::APP_SERVER_PROTOCOL_VERSION {
            let request = serde_json::json!({"jsonrpc":"2.0","id":"previous-generation",
                "method":"initialize","params":{"protocol_version":requested,
                "client":{"name":"test","version":"1"},
                "presentation":{"images":false,"questionnaires":false,"reviews":false}}});
            let response = connection.handle_json(&request.to_string()).await.unwrap();
            assert!(matches!(response, Response::Failure(Failure { error: RpcError {
                data: Some(ErrorData::UnsupportedVersion { supported: APP_SERVER_PROTOCOL_VERSION, requested: rejected }), .. }, .. }) if rejected == requested));
            assert_eq!(rejected(&connection, Method::SessionAttach { session_id: f.sessions[0].id.clone(), node_id: None }).await, ErrorData::NotInitialized);
            assert_eq!(rejected(&connection, Method::ServerInfo {}).await, ErrorData::NotInitialized);
            assert_eq!(connection.attachment_counts(), (0, 0));
        }
        initialize(&connection).await;
        for (json, expected_code) in [
            (r#"{"jsonrpc":"2.0","id":1,"method":"missing","params":{}}"#, -32601),
            (r#"{"jsonrpc":"2.0","id":2,"method":"session/create","params":{"settings":{}}}"#, -32602),
            (r#"{"jsonrpc":"1.0","id":3,"method":"session/create","params":{}}"#, -32600),
            (r#"{"jsonrpc":"2.0","id":4,"method":"session/create","params":{},"extra":true}"#, -32600),
            ("{", -32700),
            (r#"{"jsonrpc":"2.0","id":9007199254740993,"method":"session/create","params":{}}"#, -32600),
            (r#"{"jsonrpc":"2.0","id":9,"method":"session/list","params":{"offset":9007199254740993,"limit":32}}"#, -32602),
            (r#"{"jsonrpc":"2.0","id":8,"method":"session/name","params":{"session_id":"s","name":"first","name":"second"}}"#, -32602),
            (r#"{"jsonrpc":"2.0","id":6,"method":"settings/saveDefault","params":{"target":{"session_id":"s","conversation_id":"c","runtime_incarnation":1,"attachment_id":"a"},"scope":"user","expected_revision":"r","setting":"settings/saveDefault"}}"#, -32601),
            (r#"{"jsonrpc":"2.0","id":7,"method":"turn/start","params":{"target":{"session_id":"s","conversation_id":"c","runtime_incarnation":1,"attachment_id":"a"},"content":[{"type":"text","text":"never","extra":true}]}}"#, -32602),
        ] {
            let Response::Failure(failure) = connection.handle_json(json).await.unwrap() else { panic!("invalid request accepted") };
            assert_eq!(failure.error.code, expected_code);
            serde_json::to_value(Response::Failure(failure)).expect("even invalid correlation IDs produce a serializable error");
        }
        assert!(connection.handle_json(r#"{"jsonrpc":"2.0","method":"session/create","params":{}}"#).await.is_none());
        let MethodResult::Sessions { sessions, .. } = call(&connection, 5, Method::SessionList { query: None, offset: 0, limit: 32 }).await else { panic!("list") };
        assert_eq!(sessions.len(), 2);
        assert_eq!(f.manager.probe(&f.id(0).await).compositions.load(std::sync::atomic::Ordering::SeqCst), 0);
        f.close().await;
    }).await;
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
        let MethodResult::SessionTransition { session, .. } =
            call(&connection, 100, Method::SessionCreate { settings }).await
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
async fn one_connection_pipelines_sessions_without_cross_routing_and_detach_keeps_execution() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let (a, b) = tokio::join!(attach(&connection, &f, 0), attach(&connection, &f, 1));
        // Both root runtimes were composed with no ordinary user boundary,
        // so each armed the one-shot display-projection publisher (Issue
        // #386). Awaiting its probe is the deterministic replacement for
        // observing the derived preview at list time.
        let mut projection_probe =
            crate::local_runtime::session_display_projection::display_projection_probe(
                &a.session_id,
            )
            .expect("root composition armed the display-projection publisher");
        let (reply_a, reply_b) = tokio::join!(
            call(
                &connection,
                20,
                Method::TurnStart {
                    target: a.clone(),
                    content: (input("request-A"))
                        .into_iter()
                        .map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) =>
                                crate::app_server::protocol::UserInputBlock::Text(text),
                            _ => panic!("client fixtures must use text or issued receipts"),
                        })
                        .collect()
                }
            ),
            call(
                &connection,
                21,
                Method::TurnStart {
                    target: b.clone(),
                    content: (input("request-B"))
                        .into_iter()
                        .map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) =>
                                crate::app_server::protocol::UserInputBlock::Text(text),
                            _ => panic!("client fixtures must use text or issued receipts"),
                        })
                        .collect()
                }
            ),
        );
        assert!(matches!(reply_a, MethodResult::InboundAccepted { .. }));
        assert!(matches!(reply_b, MethodResult::InboundAccepted { .. }));
        tokio::join!(f.gates[0].wait_entered(), f.gates[1].wait_entered());
        f.manager
            .configuration_host(&a.session_id)
            .unwrap()
            .projection_settled()
            .await;
        let frozen = f.manager.configuration_host(&a.session_id).unwrap();
        frozen.inner.park_projection_worker();
        let before = call(
            &connection,
            22,
            Method::SessionCheckpoint {
                target: a.clone(),
                trace_records: Vec::new(),
            },
        )
        .await;
        let residency_before = f.manager.diagnostics();
        // The preview is published by the armed one-shot publisher after the
        // canonical root-lineage commit, which provably precedes the parked
        // provider request the gates observed above. Await the publisher
        // deterministically — no sleeps.
        projection_probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(
            projection_probe.borrow().published,
            "the first canonical ordinary user commit published the projection"
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            23,
            Method::SessionSummary {
                session_id: a.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary during execution")
        };
        assert_eq!(summary.id, a.session_id);
        assert_eq!(summary.preview.as_deref(), Some("request-A"));
        assert_eq!(f.manager.diagnostics(), residency_before);
        let after = call(
            &connection,
            24,
            Method::SessionCheckpoint {
                target: a.clone(),
                trace_records: Vec::new(),
            },
        )
        .await;
        let (
            MethodResult::Checkpoint {
                checkpoint: checkpoint_1,
            },
            MethodResult::Checkpoint {
                checkpoint: checkpoint_2,
            },
        ) = (before, after)
        else {
            panic!("snapshots");
        };
        let before = Box::new(checkpoint_1.snapshot);
        let before_cursor = checkpoint_1.cursor;
        let after = Box::new(checkpoint_2.snapshot);
        let after_cursor = checkpoint_2.cursor;
        assert_eq!(before, after, "catalog summary changes no Session field");
        assert!(after_cursor >= before_cursor);
        frozen.inner.unpark_projection_worker();
        let mut checked_cursor = before_cursor;
        let mut seen = std::collections::BTreeSet::new();
        while seen.len() != 2 || checked_cursor < after_cursor {
            let notification = connection.next_notification().await.notification;
            // Session metadata invalidation is not a Conversation event: it
            // carries no cursor and no attachment target (Issue #386); nor is
            // a source-scoped repair level (Issue #464).
            if matches!(
                notification,
                NotificationMethod::SummaryInvalidated { .. }
                    | NotificationMethod::SourceRepairChanged { .. }
            ) {
                continue;
            }
            let NotificationMethod::Event {
                target,
                event,
                cursor,
                ..
            } = notification
            else {
                panic!("event")
            };
            assert!(target == a || target == b);
            if target == a && cursor > before_cursor && cursor <= after_cursor {
                assert_eq!(cursor.get(), checked_cursor.get() + 1);
                assert!(event.facts().iter().all(|fact| matches!(
                    fact,
                    RuntimeClientEvent::ReadDomainsUpdated { .. }
                        | RuntimeClientEvent::TraceChanged
                )));

                checked_cursor = cursor;
            }
            if event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
            {
                seen.insert(target.session_id.clone());
            }
            if let RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } = &*event
                && checkpoint.snapshot.attempt.as_ref().is_some_and(|attempt| {
                    matches!(
                        attempt.phase,
                        crate::runtime_client::RuntimeClientAttemptPhase::Admitted
                            | crate::runtime_client::RuntimeClientAttemptPhase::Running
                    )
                })
            {
                assert_eq!(checkpoint.cursor, cursor);
                assert_eq!(checkpoint.read_cut.conversation_id, target.conversation_id);
                seen.insert(target.session_id.clone());
            }
        }
        let rival = AppServerConnection::new(f.host.clone());
        initialize(&rival).await;
        let response = rival
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(30),
                call: Method::SessionAttach {
                    session_id: a.session_id.clone(),
                    node_id: None,
                },
            })
            .await;
        assert!(matches!(
            response,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::ControllerInUse),
                    ..
                },
                ..
            })
        ));
        call(&connection, 31, Method::SessionDetach { target: a.clone() }).await;
        let stale = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(310),
                call: Method::TurnCancel { target: a.clone() },
            })
            .await;
        assert!(matches!(
            stale,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::StaleAttachment),
                    ..
                },
                ..
            })
        ));
        let live_a = f.manager.load(&a.session_id, None).await.unwrap();
        assert_eq!(live_a.incarnation_id(), a.runtime_incarnation);
        assert!(live_a.inspect_runtime().unwrap().has_current_attempt());
        let new_a = attach(&rival, &f, 0).await;
        assert_ne!(new_a.attachment_id, a.attachment_id);
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_3,
        } = call(
            &rival,
            32,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: new_a,
            },
        )
        .await
        else {
            panic!("snapshot")
        };
        let snapshot = Box::new(checkpoint_3.snapshot);
        assert_eq!(snapshot.conversation_id, a.conversation_id);
        f.gates[1].release();
        loop {
            if let NotificationMethod::Event { target, event, .. } =
                connection.next_notification().await.notification
                && event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
            {
                assert_eq!(target, b);
                break;
            }
        }
        assert!(
            live_a.inspect_runtime().unwrap().has_current_attempt(),
            "B settled while A remained blocked"
        );
        f.gates[0].release();
        drop(connection);
        drop(rival);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_connection_cannot_retain_or_control_replacement() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        let identity = f.manager.load(&old.session_id, None).await.unwrap();
        let weak = identity.inspect_runtime().unwrap().weak_inner();
        f.manager.unload(&old.conversation_id).await.unwrap();
        assert!(weak.upgrade().is_none());
        let replacement = f.manager.load(&old.session_id, None).await.unwrap();
        assert_ne!(replacement.incarnation_id(), old.runtime_incarnation);
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(50),
                call: Method::TurnStart {
                    target: old,
                    content: (input("must not execute"))
                        .into_iter()
                        .map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) => {
                                crate::app_server::protocol::UserInputBlock::Text(text)
                            }
                            _ => panic!("client fixtures must use text or issued receipts"),
                        })
                        .collect(),
                },
            })
            .await;
        assert!(matches!(
            response,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::StaleRuntime),
                    ..
                },
                ..
            })
        ));
        assert!(f.provider.request_bodies().is_empty());
        drop(connection);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn headless_approval_and_questionnaire_survive_detach_and_settle_once() {
    bounded(async {
        use crate::runtime::interaction::{
            ApprovalDecision, InteractionResponse, InteractionSettleGate, QuestionnaireResponse,
        };
        use std::sync::Arc;
        for tool in ["read", "ask_user"] {
            for cancel_first in [false, true] {
                let f = Fixture::with_tool(Some(tool)).await;
                let connection = Arc::new(AppServerConnection::new(f.host.clone()));
                initialize(&connection).await;
                let initial = attach(&connection, &f, 0).await;
                let identity = f.manager.load(&initial.session_id, None).await.unwrap();
                let runtime = identity.inspect_runtime().unwrap();
                if tool == "read" {
                    runtime.install_test_pre_tool_policy(Arc::new(AskPolicy));
                }
                let coordinator = runtime.interaction_test_owner();
                call(
                    &connection,
                    60,
                    Method::TurnStart {
                        target: initial.clone(),
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
                call(&connection, 61, Method::SessionDetach { target: initial }).await;
                // No external attachment exists when the real provider releases the tool call.
                f.gates[0].release();
                coordinator.pending_published.notified().await;
                assert_eq!(coordinator.pending_count(), 1);
                let reattached = attach(&connection, &f, 0).await;
                let MethodResult::Checkpoint {
                    checkpoint: checkpoint_4,
                } = call(
                    &connection,
                    62,
                    Method::SessionCheckpoint {
                        trace_records: vec![],
                        target: reattached.clone(),
                    },
                )
                .await
                else {
                    panic!("snapshot")
                };
                let snapshot = Box::new(checkpoint_4.snapshot);
                assert_eq!(snapshot.pending_interactions.len(), 1);
                let pending = snapshot.pending_interactions[0].clone();
                call(
                    &connection,
                    63,
                    Method::SessionDetach { target: reattached },
                )
                .await;
                assert_eq!(
                    coordinator.pending_count(),
                    1,
                    "detach cannot settle a pending interaction"
                );
                let target = attach(&connection, &f, 0).await;
                let response = if tool == "read" {
                    InteractionResponse::Approval {
                        decision: ApprovalDecision::Allow,
                    }
                } else {
                    InteractionResponse::Questionnaire {
                        response: QuestionnaireResponse::Declined,
                    }
                };
                let interaction = pending.interaction.clone();
                let respond = Method::InteractionRespond {
                    target: target.clone(),
                    interaction: interaction.clone(),
                    response,
                };
                let cancel = Method::InteractionCancel {
                    target,
                    interaction,
                };
                let (first, second) = if cancel_first {
                    (cancel, respond)
                } else {
                    (respond, cancel)
                };
                let gate = Arc::new(InteractionSettleGate::default());
                gate.arm();
                coordinator.install_settle_gate(gate.clone());
                let first_connection = connection.clone();
                let winner = tokio::spawn(async move {
                    first_connection
                        .handle_request(Request {
                            jsonrpc: JsonRpcVersion::V2,
                            id: RequestId::Integer(64),
                            call: first,
                        })
                        .await
                });
                let entered = gate.clone();
                tokio::task::spawn_blocking(move || entered.wait_entered())
                    .await
                    .unwrap();
                // The actual coordinator terminal transition is now parked with its winner selected.
                let second_connection = connection.clone();
                let (started, receiver) = tokio::sync::oneshot::channel();
                let loser = tokio::spawn(async move {
                    started.send(()).unwrap();
                    second_connection
                        .handle_request(Request {
                            jsonrpc: JsonRpcVersion::V2,
                            id: RequestId::Integer(65),
                            call: second,
                        })
                        .await
                });
                receiver.await.unwrap();
                // The loser reaches the coordinator while the winning terminal
                // transition is still parked, not merely after task completion.
                let losing_response = loser.await.unwrap();
                gate.release();
                assert!(matches!(winner.await.unwrap(), Response::Success(_)));
                assert!(matches!(losing_response, Response::Failure(_)));
                assert_eq!(coordinator.pending_count(), 0);
                loop {
                    if let NotificationMethod::Event { event, .. } =
                        connection.next_notification().await.notification
                        && event
                            .facts()
                            .iter()
                            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
                    {
                        break;
                    }
                }
                drop(coordinator);
                drop(runtime);
                drop(connection);
                f.close().await;
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn t13_attach_identity_checkpoint_and_subscribe_preserve_the_native_cut() {
    bounded(async {
        use std::sync::Arc;
        let f = Fixture::new().await;
        let identity = f.manager.load(&f.sessions[0].id, None).await.unwrap();
        let runtime = identity.inspect_runtime().unwrap();
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let Response::Success(attached) = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(80),
                call: Method::SessionAttach {
                    session_id: f.sessions[0].id.clone(),
                    node_id: None,
                },
            })
            .await
        else {
            panic!("attach")
        };
        let MethodResult::Attached { target, .. } = attached.result else {
            panic!("identity receipt")
        };
        let MethodResult::Checkpoint { checkpoint } = call(
            &connection,
            81,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("checkpoint")
        };
        let cursor = checkpoint.cursor;
        assert!(checkpoint.snapshot.inbound.pending.is_empty());
        // Publication between C and registration remains in native Replay;
        // explicit subscribe(C) does not jump to latest.
        let accepted = runtime.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        call(
            &connection,
            82,
            Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: cursor,
            },
        )
        .await;
        let mut previous = cursor.get();
        let mut occurrences = 0;
        f.gates[0].release();
        loop {
            let notification = connection.next_notification().await.notification;
            // Session metadata invalidation is not a Conversation event: it
            // carries no cursor, so it never participates in this cursor
            // monotonicity check (Issue #386); nor does a source-scoped repair
            // level (Issue #464).
            if matches!(
                notification,
                NotificationMethod::SummaryInvalidated { .. }
                    | NotificationMethod::SourceRepairChanged { .. }
            ) {
                continue;
            }
            let NotificationMethod::Event {
                target: routed,
                cursor,
                event,
                ..
            } = notification
            else {
                panic!("event")
            };
            assert_eq!(routed, target);
            assert_eq!(cursor.get(), previous + 1);
            previous = cursor.get();
            let RuntimeClientEvent::ProjectionTransition { events, metadata } = *event else {
                panic!("one bounded native transition");
            };
            assert_eq!(metadata.read_cut.revision, cursor.get());
            let mut settled = false;
            for event in events {
                if let RuntimeClientEvent::InboundEnqueued { message, .. } = &event {
                    assert_eq!(message.id, accepted.message_id);
                    occurrences += 1;
                }
                settled |= matches!(event, RuntimeClientEvent::AttemptSettled { .. });
            }
            if settled {
                break;
            }
        }
        assert_eq!(
            occurrences, 1,
            "publication is delivered once after the snapshot"
        );
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(81),
                call: Method::SessionSubscribe {
                    target: target.clone(),
                    after_cursor: crate::runtime_client::types::RuntimeClientCursor::new(u64::MAX),
                },
            })
            .await;
        assert!(matches!(
            response,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::ResyncRequired),
                    ..
                },
                ..
            })
        ));
        drop(runtime);
        call(
            &connection,
            82,
            Method::SessionDetach {
                target: target.clone(),
            },
        )
        .await;
        let replacement = f.manager.replace(&target.session_id, None).await.unwrap();
        assert!(
            f.manager
                .unload_incarnation(&target.conversation_id, target.runtime_incarnation)
                .await
                .is_err()
        );
        assert!(
            f.manager
                .is_current(replacement.conversation_id(), replacement.incarnation_id())
        );
        drop(connection);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_close_is_idempotent_and_revokes_claims_despite_retained_arc() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = std::sync::Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        let retained = connection.clone();
        connection.close();
        assert_eq!(connection.attachment_counts(), (0, 0));
        let replacement = AppServerConnection::new(f.host.clone());
        initialize(&replacement).await;
        let new = attach(&replacement, &f, 0).await;
        assert_eq!(old.runtime_incarnation, new.runtime_incarnation);
        assert_ne!(old.attachment_id, new.attachment_id);
        retained.close();
        assert_eq!(
            rejected(&retained, Method::SessionDetach { target: old }).await,
            ErrorData::StaleAttachment
        );
        call(
            &replacement,
            2,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: new,
            },
        )
        .await;
        replacement.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_linearizes_before_pending_attach_commit() {
    bounded(async {
        let f = Fixture::new().await;
        let probe = f.manager.probe(&f.id(0).await);
        probe.before_compose.arm();
        let connection = std::sync::Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let pending = connection.clone();
        let session_id = f.sessions[0].id.clone();
        let request = tokio::spawn(async move {
            rejected(
                &pending,
                Method::SessionAttach {
                    session_id,
                    node_id: None,
                },
            )
            .await
        });
        probe.before_compose.entered().await;
        assert_eq!(connection.attachment_counts(), (0, 1));
        connection.close();
        probe.before_compose.release();
        assert_eq!(request.await.unwrap(), ErrorData::StaleAttachment);
        assert_eq!(connection.attachment_counts(), (0, 0));
        let replacement = AppServerConnection::new(f.host.clone());
        initialize(&replacement).await;
        attach(&replacement, &f, 0).await;
        replacement.close();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn host_request_owner_outlives_dropped_protocol_waiter() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = std::sync::Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let worker = connection.clone();
        let waiter = tokio::spawn(async move {
            call(
                &worker,
                91,
                Method::SessionCheckpoint {
                    trace_records: vec![],
                    target,
                },
            )
            .await
        });
        probe.before_operation.entered().await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            f.host
                .forced_resources(false)
                .contains("pending_protocol_operations=1")
        );
        f.host.begin_drain();
        assert_eq!(
            rejected(&connection, Method::ServerInfo {}).await,
            ErrorData::ServerDraining
        );
        let host = f.host.clone();
        let mut drain = Box::pin(host.drain());
        assert!(futures_util::poll!(&mut drain).is_pending());
        assert!(f.host.finish_drain().is_err());
        probe.before_operation.release();
        assert!(drain.await.is_empty());
        assert!(
            f.host
                .forced_resources(false)
                .contains("pending_protocol_operations=0")
        );
        connection.close();
        f.host.finish_drain().unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artifact_carrier_is_native_scoped_bounded_and_cold_reopen_safe() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let other = attach(&connection, &f, 1).await;
        let artifact_id = {
            let managed = f.load(0).await.unwrap().unwrap();
            managed
                .inspect_runtime()
                .unwrap()
                .tool_runtime()
                .artifacts()
                .put_bounded(b"canonical bytes")
                .unwrap()
        };
        let read = call(
            &connection,
            801,
            Method::ArtifactRead {
                target: target.clone(),
                artifact_id: artifact_id.clone(),
            },
        )
        .await;
        assert_eq!(
            read,
            MethodResult::ArtifactBytes {
                data: "Y2Fub25pY2FsIGJ5dGVz".into()
            }
        );
        let encoded = serde_json::to_string(&read).unwrap();
        assert!(!encoded.contains(f.workspaces[0].parent().unwrap().to_str().unwrap()));
        rejected(
            &connection,
            Method::ArtifactRead {
                target: other.clone(),
                artifact_id: artifact_id.clone(),
            },
        )
        .await;
        rejected(
            &connection,
            Method::ArtifactRead {
                target: target.clone(),
                artifact_id: crate::runtime::identity::ArtifactId::new("../conversation.sqlite"),
            },
        )
        .await;
        let uploaded = binary_upload(&connection, target.clone(), "hello.txt", b"hi").await;
        assert_eq!(uploaded.len(), 1);
        let product_host = TrustedFileHost::new(&f).await;
        let host_read = |target| crate::app_server::product_host::FileRead {
            target,
            source: crate::app_server::product_host::ReadSource::Artifact {
                agent_id: None,
                artifact_id: artifact_id.clone(),
            },
            roots: vec![],
        };
        assert_eq!(product_host.success(host_read(target.clone())).await, read);
        product_host.rejected(host_read(other.clone())).await;

        rejected(
            &connection,
            Method::SessionUploadPrepare {
                target: target.clone(),
                operation_id: "0".repeat(32),
                files: vec![crate::local_runtime::session::uploads::UploadMetadata {
                    name: "../unsafe.txt".into(),
                    size: 0,
                }],
            },
        )
        .await;
        assert!(
            f.provider.request_bodies().is_empty(),
            "storage-only upload never reaches provider"
        );
        call(
            &connection,
            802,
            Method::SessionDetach {
                target: target.clone(),
            },
        )
        .await;
        product_host.rejected(host_read(target.clone())).await;
        rejected(
            &connection,
            Method::ArtifactRead {
                target,
                artifact_id: artifact_id.clone(),
            },
        )
        .await;
        let reopened = attach(&connection, &f, 0).await;
        assert_eq!(
            call(
                &connection,
                803,
                Method::ArtifactRead {
                    target: reopened,
                    artifact_id: artifact_id.clone()
                }
            )
            .await,
            read
        );
        let next = {
            let managed = f.load(0).await.unwrap().unwrap();
            managed
                .inspect_runtime()
                .unwrap()
                .tool_runtime()
                .artifacts()
                .put_bounded(b"new bytes")
                .unwrap()
        };
        assert_ne!(artifact_id, next);
        product_host.close().await;
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trace_reads_are_read_only_and_reconnect_repairs_the_same_native_facts() {
    bounded(async {
        let f = Fixture::new().await;
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
                .any(|entry| entry.kind == crate::runtime_client::trace::TraceKind::Request)
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
        let f = Fixture::new().await;
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
                    data: vec![].into(),
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
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::local_runtime::session::uploads::UploadFile;
        use crate::message::types::{
            InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
        };
        let f = Fixture::new().await;
        let controller = f.manager.session_controller();
        let source = &f.sessions[0];
        let files = controller
            .upload(
                &source.id,
                None,
                vec![
                    UploadFile {
                        name: "A.txt".into(),
                        data: b"A".to_vec().into(),
                    },
                    UploadFile {
                        name: "B.txt".into(),
                        data: b"B".to_vec().into(),
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
            Method::TurnStart {
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
            Method::TurnSteer {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web08_source_and_session_cas_cross_the_real_protocol_boundary() {
    use crate::local_runtime::configuration::settings::SourceMutation;
    use crate::model::{catalog::ModelRef, session::SessionModelConfig};
    bounded(async {
        let f = Fixture::new().await;
        // Authorized, canonical TOML; only full-runtime Workflow semantics fail.
        std::fs::write(
            f.workspaces[0].join("rustx.toml"),
            "[agent]\nworkflows = ['check', 'check']\n",
        )
        .unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let _id = f.sessions[0].id.clone();
        let MethodResult::SourceSettings {
            projection,
            ..
        } = call(
            &connection,
            1,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        let projection=projection.inline.expect("bounded fixture source");
        assert!(projection.user.authored.is_some());
        assert!(projection.workspace.is_none());
        assert!(projection.user.authored.as_ref().unwrap().models.as_ref().unwrap().contains_key("local/b"));
        let expected = projection.user.revision.clone();
        let selection = Some(SessionModelConfig::of(ModelRef::parse("local/b").unwrap()));
        let MethodResult::SourcesWritten { revision: written_revision, .. } = call(
            &connection,
            2,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
                expected_revision: expected.clone(),
                mutation: SourceMutation::Config { mutation: crate::local_runtime::configuration::settings::ConfigMutation::RootModel {
                    authored: Some(
                        crate::local_runtime::authoring::ModelLayer {
                            model: Some(ModelRef::parse("local/b").unwrap()),
                            ..Default::default()
                        },
                    ),
                } },
            },
        )
        .await
        else {
            panic!()
        };
        let MethodResult::SourceSettings { projection: saved, .. } = call(
            &connection, 1002, Method::SourcesRead { target: crate::local_runtime::configuration::settings::SourceTarget::User },
        ).await else { panic!("explicit source reread after confirmed write") };
        let saved=saved.inline.expect("bounded fixture source");
        assert_eq!(saved.user.revision, written_revision);
        assert_ne!(saved.user.revision, expected);
        assert_eq!(
            saved.user.authored.as_ref().unwrap().agent.as_ref().unwrap().model.as_ref().unwrap().model,
            selection.as_ref().map(|s| s.model.clone())
        );
        assert!(matches!(
            rejected(
                &connection,
                Method::SourcesWrite {
                    target: crate::local_runtime::configuration::settings::SourceTarget::User,
                    expected_revision: expected,
                    mutation: SourceMutation::Config { mutation: crate::local_runtime::configuration::settings::ConfigMutation::RootModel { authored: None } }
                }
            )
            .await,
            ErrorData::SourceConflict {
                ..
            }
        ));
        let MethodResult::SourceSettings {
            projection: fresh, ..
        } = call(
            &connection,
            3,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        let fresh=fresh.inline.expect("bounded fixture source");
        assert_eq!(fresh.user, saved.user);
        let error = f
            .manager
            .configuration
            .resolve_session(
                &crate::local_runtime::configuration::SessionConfigInput::new(
                    f.workspaces[0].clone(),
                ),
            )
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("duplicate"), "{error}");
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web08_catalog_commit_preserves_admitted_attempt_and_updates_cold_resolution() {
    use crate::local_runtime::configuration::settings::SourceMutation;
    bounded(Box::pin(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let initial = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let mut model = initial
            .user
            .authored
            .as_ref()
            .unwrap()
            .models
            .as_ref()
            .unwrap()["local/a"]
            .clone();
        model.request_params = None;
        model.default_profile = Some(crate::model::catalog::ModelProfileId::new("old"));
        model.profiles = Some(
            ["old", "new"]
                .into_iter()
                .map(|name| {
                    (
                        crate::model::catalog::ModelProfileId::new(name),
                        crate::model::authoring::Profile {
                            reasoning_enabled: Some(false),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        );
        f.manager
            .write_source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                initial.user.revision,
                SourceMutation::Config {
                    mutation:
                        crate::local_runtime::configuration::settings::ConfigMutation::Model {
                            id: "local/a".into(),
                            authored: Some(model),
                        },
                },
            )
            .await
            .unwrap();
        let target = attach(&connection, &f, 0).await;
        if let Some(candidate) = f
            .manager
            .configuration_application(&target.session_id)
            .and_then(|application| application.candidate)
        {
            f.manager
                .adopt_configuration(
                    &target.session_id,
                    &candidate.identity,
                    candidate.expected_binding,
                )
                .unwrap();
        }
        call(
            &connection,
            101,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-A".into(),
                })],
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let runtime = f.manager.load(&target.session_id, None).await.unwrap();
        let before = runtime.client().checkpoint().unwrap().snapshot;
        let MethodResult::SourceSettings { projection, .. } = call(
            &connection,
            102,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        let projection = projection.inline.expect("bounded fixture source");
        let mut model = projection
            .user
            .authored
            .as_ref()
            .unwrap()
            .models
            .as_ref()
            .unwrap()["local/a"]
            .clone();
        model.max_output_tokens = 2048;
        // A Profile edit after admission: a new default whose complete
        // native object carries a value the admitted Attempt never froze.
        model.default_profile = Some(crate::model::catalog::ModelProfileId::new("new"));
        model.profiles.as_mut().unwrap().insert(
            crate::model::catalog::ModelProfileId::new("new"),
            crate::model::authoring::Profile {
                reasoning_enabled: Some(false),
                max_output_tokens: Some(1024),
                request_params: crate::toml_authoring::AuthoredRequestParams(
                    serde_json::from_value(serde_json::json!({"temperature": 0.8, "stop": [null]}))
                        .unwrap(),
                ),
            },
        );
        let MethodResult::SourcesWritten { .. } = call(
            &connection,
            103,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
                expected_revision: projection.user.revision,
                mutation: SourceMutation::Config {
                    mutation:
                        crate::local_runtime::configuration::settings::ConfigMutation::Model {
                            id: "local/a".into(),
                            authored: Some(model),
                        },
                },
            },
        )
        .await
        else {
            panic!("confirmed native source-write receipt")
        };
        let after = runtime.client().checkpoint().unwrap().snapshot;
        assert_eq!(before.resources, after.resources);
        assert_eq!(before.capabilities, after.capabilities);
        assert_eq!(
            before.attempt.as_ref().unwrap().execution_settings,
            after.attempt.as_ref().unwrap().execution_settings
        );
        assert_eq!(before.model, after.model);
        assert_eq!(
            before.attempt.as_ref().unwrap().model,
            after.attempt.as_ref().unwrap().model
        );
        let prospective = f
            .manager
            .configuration
            .resolve_session(
                &crate::local_runtime::configuration::SessionConfigInput::new(
                    f.workspaces[0].clone(),
                ),
            )
            .unwrap();
        let next = prospective
            .models
            .model(&crate::model::catalog::ModelRef::parse("local/a").unwrap())
            .unwrap();
        let frozen = after.attempt.as_ref().unwrap().model.as_ref().unwrap();
        assert_eq!(next.id.as_str(), "a");
        assert_eq!(next.max_output_tokens, 2048);
        assert_eq!(frozen.primary.max_output_tokens, 4096);
        assert_eq!(next.default_profile.as_ref().unwrap().as_str(), "new");
        assert_eq!(frozen.primary.profile.as_ref().unwrap().as_str(), "old");
        let new = &next.profiles[&crate::model::catalog::ModelProfileId::new("new")];
        assert_eq!(new.request_params["temperature"], 0.8);
        assert_eq!(new.request_params["stop"][0], serde_json::Value::Null);
        assert_eq!(new.max_output_tokens, Some(1024));
        assert!(frozen.primary.request_params.is_empty());
        // Creation uses the successfully prepared source authority, after
        // native processing acknowledges availability. Existing A stays frozen.
        super::configuration::settled(&f, 0).await;
        let created = f
            .manager
            .create_session(crate::local_runtime::session::SessionPersistentState {
                cwd: f.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap();
        let cold = attach_session(&connection, &created.session).await;
        let catalog = read_catalog_result(
            call(&connection, 104, Method::ModelCatalog { target: cold }).await,
        );
        assert_eq!(
            catalog
                .models
                .iter()
                .find(|m| m.model.to_string() == "local/a")
                .unwrap()
                .max_output_tokens,
            2048
        );
        f.gates[0].release();
        f.close().await;
    }))
    .await;
}

/// Author `local/a` with exactly these Profiles through `sources/write`.
async fn write_profiles(
    connection: &AppServerConnection,
    id: i64,
    default: &str,
    profiles: &[(&str, serde_json::Value)],
) {
    use crate::local_runtime::configuration::settings::{
        ConfigMutation, SourceMutation, SourceTarget,
    };
    use crate::model::catalog::ModelProfileId;
    let MethodResult::SourceSettings { projection, .. } = call(
        connection,
        id,
        Method::SourcesRead {
            target: SourceTarget::User,
        },
    )
    .await
    else {
        panic!("source settings")
    };
    let projection = projection.inline.expect("bounded fixture source");
    let mut model = projection
        .user
        .authored
        .as_ref()
        .unwrap()
        .models
        .as_ref()
        .unwrap()["local/a"]
        .clone();
    model.request_params = None;
    model.default_profile = Some(ModelProfileId::new(default));
    model.profiles = Some(
        profiles
            .iter()
            .map(|(name, params)| {
                (
                    ModelProfileId::new(*name),
                    crate::model::authoring::Profile {
                        request_params: crate::toml_authoring::AuthoredRequestParams(
                            serde_json::from_value(params.clone()).unwrap(),
                        ),
                        ..Default::default()
                    },
                )
            })
            .collect(),
    );
    let MethodResult::SourcesWritten { .. } = call(
        connection,
        id + 1,
        Method::SourcesWrite {
            target: SourceTarget::User,
            expected_revision: projection.user.revision,
            mutation: SourceMutation::Config {
                mutation: ConfigMutation::Model {
                    id: "local/a".into(),
                    authored: Some(model),
                },
            },
        },
    )
    .await
    else {
        panic!("confirmed native source-write receipt")
    };
}

fn profile_ids(catalog: &ModelCatalogObservation) -> Vec<String> {
    catalog
        .models
        .iter()
        .find(|model| model.model.to_string() == "local/a")
        .unwrap()
        .profiles
        .iter()
        .map(|profile| profile.id.to_string())
        .collect()
}

/// Issue #459: catalog publication and invocation adoption are distinct
/// contracts. A Profile published while an attached Session's invocation is
/// unchanged reaches that Session's `session/models` and `session/setModel`
/// without any preparation, adoption candidate or resource rebuild, and the
/// Attempt admitted before it keeps its frozen invocation through a Tool
/// continuation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_published_profile_reaches_attached_session_without_adoption() {
    use crate::local_runtime::configuration::application::UnitApplication;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::model::session::SessionModelConfig;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    bounded(Box::pin(async {
        let f = Fixture::with_tool(Some("read")).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let balanced = ("balanced", json!({"temperature": 0.5}));
        let fast = ("fast", json!({"temperature": 1.0}));
        write_profiles(
            &connection,
            100,
            "balanced",
            &[balanced.clone(), fast.clone()],
        )
        .await;
        let target = attach(&connection, &f, 0).await;
        let pinned = |profile: &str| SessionModelConfig {
            profile: Some(ModelProfileId::new(profile)),
            ..SessionModelConfig::of(ModelRef::parse("local/a").unwrap())
        };
        assert!(matches!(
            call(
                &connection,
                102,
                Method::ModelSet {
                    target: target.clone(),
                    config: Box::new(pinned("balanced")),
                }
            )
            .await,
            MethodResult::ModelSet { .. }
        ));
        let model = read_model_result(
            &connection,
            &target,
            call(
                &connection,
                10_102,
                Method::ModelGet {
                    target: target.clone(),
                },
            )
            .await,
        )
        .await;
        assert_eq!(
            model.effective.as_ref().unwrap().request_params["temperature"],
            json!(0.5)
        );
        // The fixture loads after source authoring; explicitly register its
        // source application. Model selection alone never ingests source files.
        f.manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        super::configuration::settled(&f, 0).await;
        let models = |id| {
            let connection = &connection;
            let target = target.clone();
            async move {
                read_catalog_result(call(connection, id, Method::ModelCatalog { target }).await)
            }
        };
        assert_eq!(profile_ids(&models(103).await), ["balanced", "fast"]);

        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        let runtime = f.manager.configuration_runtime(&target.session_id).unwrap();
        let probe = f.manager.probe(&f.sessions[0].active_conversation_id);
        let invocation = runtime.model_snapshot();
        let resources = runtime.runtime_resources();
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let previous = f
            .manager
            .configuration_application(&target.session_id)
            .unwrap();
        let settlement = runtime.settlement_signal();
        call(
            &connection,
            104,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-A".into(),
                })],
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let admitted = managed
            .client()
            .checkpoint()
            .unwrap()
            .snapshot
            .attempt
            .unwrap()
            .model;

        // `deep` is added; `balanced`, the selected Profile, is untouched.
        let deep = ("deep", json!({"temperature": 0.1, "seed": 42}));
        write_profiles(
            &connection,
            105,
            "balanced",
            &[balanced.clone(), fast.clone(), deep.clone()],
        )
        .await;
        let application = super::configuration::settled(&f, 0).await;
        assert_ne!(application.desired, previous.desired);
        assert!(application.candidate.is_none(), "{application:?}");
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );
        // Nothing was prepared or rebuilt: the same resolved invocations and
        // adapters, the same capability snapshot; only the configuration
        // generation that carries the catalog advanced.
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        assert_eq!(runtime.model_snapshot(), invocation);
        let published = runtime.runtime_resources();
        assert!(std::sync::Arc::ptr_eq(
            resources.capability(),
            published.capability()
        ));
        assert_eq!(published.revision(), resources.revision().next());
        // Native discovery: the App Server and the Runtime Client read one
        // published catalog.
        let catalog = models(107).await;
        assert_eq!(profile_ids(&catalog), ["balanced", "deep", "fast"]);
        let client = read_native_catalog_result(
            f.manager
                .configuration_host(&target.session_id)
                .unwrap()
                .model_catalog()
                .unwrap(),
        );
        assert_eq!(client, catalog);
        assert_eq!(
            managed
                .client()
                .checkpoint()
                .unwrap()
                .snapshot
                .attempt
                .unwrap()
                .model,
            admitted
        );

        // Select the newly published Profile while the admitted Attempt is still
        // physically parked. Only future independent Attempts may use it.
        let MethodResult::ModelSet { .. } = call(
            &connection,
            108,
            Method::ModelSet {
                target: target.clone(),
                config: Box::new(pinned("deep")),
            },
        )
        .await
        else {
            panic!("bounded selection receipt")
        };
        let model = read_model_result(
            &connection,
            &target,
            call(
                &connection,
                10_108,
                Method::ModelGet {
                    target: target.clone(),
                },
            )
            .await,
        )
        .await;
        assert_eq!(
            model.effective.as_ref().unwrap().profile,
            Some(ModelProfileId::new("deep"))
        );
        assert_eq!(
            managed
                .client()
                .checkpoint()
                .unwrap()
                .snapshot
                .attempt
                .unwrap()
                .model,
            admitted
        );

        // The admitted Attempt's Tool continuation still sends its frozen
        // invocation.
        f.gates[0].release();
        settlement.notified().await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 2, "Tool proposal and its continuation");
        for body in &bodies {
            let body: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(body["temperature"], json!(0.5));
            assert!(body.get("seed").is_none());
        }

        let settlement = runtime.settlement_signal();
        call(
            &connection,
            109,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-A deep".into(),
                })],
            },
        )
        .await;
        settlement.notified().await;
        // The history already holds a Tool result, so the fixture answers
        // this Attempt in one request.
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 3);
        let body: serde_json::Value = serde_json::from_str(&bodies[2]).unwrap();
        assert_eq!(
            (&body["temperature"], &body["seed"]),
            (&json!(0.1), &json!(42))
        );
        f.close().await;
    }))
    .await;
}

/// The typed refusal of one App Server request.
async fn refusal(connection: &AppServerConnection, id: i64, call: Method) -> ErrorData {
    let response = connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .await;
    let Response::Failure(Failure {
        error: RpcError {
            data: Some(data), ..
        },
        ..
    }) = response
    else {
        panic!("{response:?}")
    };
    data
}

/// Issue #459: a published catalog that removes the Profile an existing
/// Session pins is published anyway. The Session keeps `fast` configured and
/// reports it unavailable — no silent `balanced` — its admitted Attempt keeps
/// the frozen `fast` invocation through a Tool continuation, new work is
/// refused explicitly, and selecting `deep` through `session/setModel`
/// recovers without preparation, a resource rebuild, a new Session or a
/// reconnect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_removed_selected_profile_is_reported_until_explicit_recovery() {
    use crate::local_runtime::configuration::application::UnitApplication;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::model::session::SessionModelConfig;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    bounded(Box::pin(async {
        let f = Fixture::with_tool(Some("read")).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let fast = ("fast", json!({"temperature": 1.0, "top_p": 0.5}));
        let balanced = ("balanced", json!({"temperature": 0.5}));
        let deep = ("deep", json!({"temperature": 0.1, "seed": 42}));
        write_profiles(
            &connection,
            100,
            "balanced",
            &[fast.clone(), balanced.clone(), deep.clone()],
        )
        .await;
        let target = attach(&connection, &f, 0).await;
        let pinned = |profile: &str| SessionModelConfig {
            profile: Some(ModelProfileId::new(profile)),
            ..SessionModelConfig::of(ModelRef::parse("local/a").unwrap())
        };
        let set = |profile: &str| Method::ModelSet {
            target: target.clone(),
            config: Box::new(pinned(profile)),
        };
        call(&connection, 102, set("fast")).await;
        f.manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        super::configuration::settled(&f, 0).await;
        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        let runtime = f.manager.configuration_runtime(&target.session_id).unwrap();
        let probe = f.manager.probe(&f.sessions[0].active_conversation_id);
        let resources = runtime.runtime_resources();
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let selected = runtime.model_view();
        assert!(selected.unavailable.is_none());

        let settlement = runtime.settlement_signal();
        call(
            &connection,
            103,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-A".into(),
                })],
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let admitted = managed.client().checkpoint().unwrap().snapshot.attempt.unwrap().model;

        // `fast`, the pinned Profile, is removed from a valid catalog.
        write_profiles(&connection, 104, "balanced", &[balanced.clone(), deep.clone()]).await;
        let application = super::configuration::settled(&f, 0).await;
        assert!(application.candidate.is_none(), "{application:?}");
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        let published = runtime.runtime_resources();
        assert!(std::sync::Arc::ptr_eq(
            resources.capability(),
            published.capability()
        ));
        assert_eq!(published.revision(), resources.revision().next());

        // Discovery: both native surfaces advertise the published catalog.
        let catalog = read_catalog_result(call(
            &connection,
            106,
            Method::ModelCatalog {
                target: target.clone(),
            },
        )
        .await);
        assert_eq!(profile_ids(&catalog), ["balanced", "deep"]);
        let client = read_native_catalog_result(f
            .manager
            .configuration_host(&target.session_id)
            .unwrap()
            .model_catalog()
            .unwrap());
        assert_eq!(client, catalog);

        // The configured identity is still `fast`, reported unavailable; the
        // invocation it last resolved is retained, never replaced by a default.
        let model = read_model_result(
            &connection, &target, call(
            &connection,
            107,
            Method::ModelGet {
                target: target.clone(),
            },
        )
        .await).await;
        assert_eq!(model.configured, pinned("fast"));
        let diagnostic = model.unavailable.clone().expect("reported unavailable");
        assert!(diagnostic.contains("fast"), "{diagnostic}");
        assert_eq!(model.effective, selected.effective);
        assert_eq!(model.summary, selected.summary);
        assert_eq!(
            managed.client().checkpoint().unwrap().snapshot.attempt.unwrap().model,
            admitted
        );

        // The admitted Attempt finishes on its frozen `fast` invocation.
        f.gates[0].release();
        settlement.notified().await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 2, "Tool proposal and its continuation");
        for body in &bodies {
            let body: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!((&body["temperature"], &body["top_p"]), (&json!(1.0), &json!(0.5)));
        }

        // New work is refused explicitly and nothing reaches the provider.
        let refused = refusal(
            &connection,
            108,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-B".into(),
                })],
            },
        )
        .await;
        assert!(
            matches!(&refused, ErrorData::ModelUnavailable { diagnostic: refused } if *refused == format!("the configured session model is unavailable: {diagnostic}")),
            "{refused:?}"
        );
        assert!(matches!(
            refusal(
                &connection,
                109,
                Method::CompactContext {
                    target: target.clone(),
                    request_id: "issue459".to_owned().try_into().unwrap(),
                },
            )
            .await,
            ErrorData::ModelUnavailable { .. }
        ));
        assert!(managed.client().checkpoint().unwrap().snapshot.inbound.pending.is_empty());
        assert_eq!(f.provider.request_bodies().len(), 2);
        // Re-selecting the removed Profile is refused by the published catalog.
        assert!(matches!(
            refusal(&connection, 110, set("fast")).await,
            ErrorData::ModelUnavailable { .. }
        ));
        assert_eq!(runtime.model_view(), *model);

        // Explicit recovery uses the published catalog without an idle gate.
        let MethodResult::ModelSet { .. } = call(&connection, 111, set("deep")).await else {
            panic!("bounded selection receipt")
        };
        let model = read_model_result(
            &connection, &target, call(&connection, 10_108, Method::ModelGet {target:target.clone()}).await).await;
        assert!(model.unavailable.is_none());
        assert_eq!(
            model.effective.as_ref().unwrap().profile,
            Some(ModelProfileId::new("deep"))
        );
        let settlement = runtime.settlement_signal();
        call(
            &connection,
            112,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-C".into(),
                })],
            },
        )
        .await;
        settlement.notified().await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 3);
        let body: serde_json::Value = serde_json::from_str(&bodies[2]).unwrap();
        assert_eq!(
            (&body["temperature"], &body["seed"]),
            (&json!(0.1), &json!(42))
        );
        assert!(body.get("top_p").is_none());
        // Recovery rebuilt nothing: the same executable tool registry.
        assert!(std::sync::Arc::ptr_eq(
            resources.capability().tool_registry(),
            runtime.runtime_resources().capability().tool_registry()
        ));
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        f.close().await;
    }))
    .await;
}

/// Issue #459: the acceptance sequence. An existing Session uses `local/a`; a
/// valid published catalog removes it, introduces `local/c` and makes that
/// the new-Session default. The Session keeps `local/a` configured and
/// unavailable — nothing switches it — discovers `local/c`, refuses `local/a`
/// as a new explicit selection, and after `session/setModel` the next Attempt
/// sends `local/c`'s wire identity and exact Profile parameters, all on the
/// same runtime incarnation and attachment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_removed_selected_model_is_replaced_explicitly_in_place() {
    use crate::local_runtime::configuration::application::UnitApplication;
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::model::catalog::ModelRef;
    use crate::model::session::SessionModelConfig;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    bounded(Box::pin(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let model = |id: &str| SessionModelConfig::of(ModelRef::parse(id).unwrap());
        let set = |id: &str| Method::ModelSet {
            target: target.clone(),
            config: Box::new(model(id)),
        };
        // Persist the Session's currently resolved `local/a` as explicit intent.
        call(&connection, 100, set("local/a")).await;
        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        let incarnation = managed.incarnation_id();
        let runtime = f.manager.configuration_runtime(&target.session_id).unwrap();
        let probe = f.manager.probe(&f.sessions[0].active_conversation_id);
        let compositions = probe.compositions.load(Ordering::SeqCst);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let resources = runtime.runtime_resources();
        let selected = runtime.model_view();

        // One valid source generation removes `local/a`, adds `local/c` with
        // its own wire identity and Profile, and moves the default to it.
        let source = f
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
        let models = document["models"].as_table_mut().unwrap();
        let mut replacement = models.remove("local/a").unwrap();
        let table = replacement.as_table_mut().unwrap();
        table.remove("request_params");
        table.insert("id".into(), "c".into());
        table.insert("default_profile".into(), "steady".into());
        table.insert(
            "profiles".into(),
            toml::toml! { steady = { request_params = "{\"temperature\":0.25,\"seed\":7}" } }
                .into(),
        );
        models.insert("local/c".into(), replacement);
        document["agent"]["model"]["model"] = "local/c".into();
        std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
        call(
            &connection,
            101,
            Method::ConfigurationReconcile {
                target: SourceTarget::User,
            },
        )
        .await;
        let application = super::configuration::settled(&f, 0).await;
        assert!(application.candidate.is_none(), "{application:?}");
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        let published = runtime.runtime_resources();
        assert!(std::sync::Arc::ptr_eq(
            resources.capability(),
            published.capability()
        ));
        assert_eq!(published.revision(), resources.revision().next());
        // The new-Session default travels with the catalog it is valid in.
        assert_eq!(
            runtime
                .configuration_view()
                .unwrap()
                .root_agent
                .model
                .unwrap()
                .model
                .to_string(),
            "local/c"
        );

        // The Session still asks for `local/a`, reported unavailable.
        let view = read_model_result(
            &connection,
            &target,
            call(
                &connection,
                102,
                Method::ModelGet {
                    target: target.clone(),
                },
            )
            .await,
        )
        .await;
        assert_eq!(view.configured, model("local/a"));
        assert!(view.unavailable.as_deref().unwrap().contains("local/a"));
        assert_eq!(view.effective, selected.effective);
        // Discovery advertises the replacement, and only the published catalog.
        let catalog = read_catalog_result(
            call(
                &connection,
                103,
                Method::ModelCatalog {
                    target: target.clone(),
                },
            )
            .await,
        );
        let references: Vec<_> = catalog
            .models
            .iter()
            .map(|entry| entry.model.to_string())
            .collect();
        assert_eq!(references, ["local/b", "local/c"]);
        // `local/a` is no longer admitted as an explicit selection, and no
        // turn runs on it.
        assert!(matches!(
            refusal(&connection, 104, set("local/a")).await,
            ErrorData::ModelUnavailable { .. }
        ));
        assert!(matches!(
            refusal(
                &connection,
                105,
                Method::TurnStart {
                    target: target.clone(),
                    content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                        text: "on a".into(),
                    })],
                },
            )
            .await,
            ErrorData::ModelUnavailable { .. }
        ));
        assert!(f.provider.request_bodies().is_empty());

        // Explicit selection of the replacement, then the next Attempt.
        assert!(matches!(
            call(&connection, 106, set("local/c")).await,
            MethodResult::ModelSet { .. }
        ));
        let view = read_model_result(
            &connection,
            &target,
            call(
                &connection,
                10_106,
                Method::ModelGet {
                    target: target.clone(),
                },
            )
            .await,
        )
        .await;
        assert!(view.unavailable.is_none());
        let effective = view.effective.as_ref().unwrap();
        assert_eq!(effective.model.to_string(), "local/c");
        assert_eq!(effective.profile.as_ref().unwrap().as_str(), "steady");
        f.gates[0].release();
        let settlement = runtime.settlement_signal();
        call(
            &connection,
            107,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "on c".into(),
                })],
            },
        )
        .await;
        settlement.notified().await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 1);
        let body: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(
            (&body["model"], &body["temperature"], &body["seed"]),
            (&json!("c"), &json!(0.25), &json!(7))
        );
        // No reload, recomposition or reattachment was involved.
        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        assert_eq!(managed.incarnation_id(), incarnation);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), compositions);
        call(
            &connection,
            108,
            Method::ModelGet {
                target: target.clone(),
            },
        )
        .await;
        f.close().await;
    }))
    .await;
}

fn source_gate(
    f: &Fixture,
    point: &'static str,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, resume) = std::sync::mpsc::channel();
    f.manager.configuration.test_hooks.insert(point, move || {
        entered.send(()).unwrap();
        resume.recv().unwrap();
    });
    (waiting, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c12_source_read_is_independent_of_concurrent_session_revision() {
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let before = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let revision = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(&id)
            .unwrap();
        let document_lock =
            crate::local_runtime::settings::lock_document(std::path::Path::new(&before.user.path))
                .unwrap();
        let (entered, resume) = source_gate(&f, "before_documents");
        let manager = f.manager.clone();
        let _read_id = id.clone();
        let read = tokio::spawn(async move {
            manager
                .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
                .await
        });
        entered.await.unwrap();
        resume.send(()).unwrap();
        // Source worker is about to wait on this held document lock.
        // Unrelated catalog access and a durable same-Session commit both finish.
        let mut catalog = f.manager.sessions.catalog.lock().await;
        catalog.settings_revision(&f.sessions[1].id).unwrap();
        let (_, mut settings) = catalog.lineage(&id, None).unwrap();
        settings.model = Some(crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/b").unwrap(),
        ));
        let next = catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        assert!(
            !read.is_finished(),
            "held document lock prevents source completion"
        );
        drop(document_lock);
        assert!(read.await.unwrap().is_ok());
        let after = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let (current, settings) = f.manager.sessions.read_settings(&id).await.unwrap();
        let selected = settings.model;
        assert_eq!(current, next);
        assert_eq!(selected.unwrap().model.to_string(), "local/b");
        assert_eq!(after.user.revision, before.user.revision);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c12_source_commit_is_independent_of_concurrent_session_revision() {
    use crate::local_runtime::configuration::settings::SourceMutation;
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let before = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let revision = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(&id)
            .unwrap();
        let (entered, resume) = source_gate(&f, "before_publication");
        let mut model = before
            .user
            .authored
            .as_ref()
            .unwrap()
            .models
            .as_ref()
            .unwrap()["local/a"]
            .clone();
        model.max_output_tokens = 2048;
        let manager = f.manager.clone();
        let _write_id = id.clone();
        let write = tokio::spawn(async move {
            manager
                .write_source_settings(
                    &crate::local_runtime::configuration::settings::SourceTarget::User,
                    before.user.revision,
                    SourceMutation::Config {
                        mutation:
                            crate::local_runtime::configuration::settings::ConfigMutation::Model {
                                id: "local/a".into(),
                                authored: Some(model),
                            },
                    },
                )
                .await
        });
        entered.await.unwrap();
        let mut catalog = f.manager.sessions.catalog.lock().await;
        let (_, settings) = catalog.lineage(&id, None).unwrap();
        catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        resume.send(()).unwrap();
        assert!(write.await.unwrap().is_ok());
        let fresh = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        assert_eq!(
            fresh
                .user
                .authored
                .as_ref()
                .unwrap()
                .models
                .as_ref()
                .unwrap()["local/a"]
                .max_output_tokens,
            2048
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c12_mcp_source_commit_is_independent_of_session_revision() {
    use crate::local_runtime::configuration::settings::{McpWrite, SourceMutation};
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let before = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let revision = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(&id)
            .unwrap();
        let mutation = SourceMutation::Mcp {
            id: crate::runtime::identity::McpServerId::new("fixture"),
            authored: Some(McpWrite {
                definition: serde_json::from_value(
                    serde_json::json!({"command":"never-start-unselected-fixture"}),
                )
                .unwrap(),
                retained_env: vec![],
                retained_headers: vec![],
            }),
        };
        let (entered, resume) = source_gate(&f, "before_publication");
        let manager = f.manager.clone();
        let _write_id = id.clone();
        let expected = before.user_mcp.revision.clone();
        let write = tokio::spawn(async move {
            manager
                .write_source_settings(
                    &crate::local_runtime::configuration::settings::SourceTarget::User,
                    expected,
                    mutation,
                )
                .await
        });
        entered.await.unwrap();
        let mut catalog = f.manager.sessions.catalog.lock().await;
        let (_, settings) = catalog.lineage(&id, None).unwrap();
        catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        resume.send(()).unwrap();
        assert!(write.await.unwrap().is_ok());
        let MethodResult::SourceSettings {
            projection: fresh, ..
        } = call(
            &connection,
            1,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        let fresh = fresh.inline.expect("bounded fixture source");
        assert!(
            fresh
                .user_mcp
                .authored
                .as_ref()
                .unwrap()
                .contains_key(&crate::runtime::identity::McpServerId::new("fixture"))
        );
        assert!(matches!(
            rejected(
                &connection,
                Method::SourcesWrite {
                    target: crate::local_runtime::configuration::settings::SourceTarget::User,
                    expected_revision: before.user_mcp.revision,
                    mutation: SourceMutation::Mcp {
                        id: crate::runtime::identity::McpServerId::new("fixture"),
                        authored: None
                    }
                }
            )
            .await,
            ErrorData::SourceConflict { .. }
        ));
        let MethodResult::SourcesWritten {
            revision: written_revision,
            ..
        } = call(
            &connection,
            2,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
                expected_revision: fresh.user_mcp.revision,
                mutation: SourceMutation::Mcp {
                    id: crate::runtime::identity::McpServerId::new("fixture"),
                    authored: None,
                },
            },
        )
        .await
        else {
            panic!()
        };
        let MethodResult::SourceSettings {
            projection: deleted,
            ..
        } = call(
            &connection,
            1002,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!("explicit source reread after confirmed write")
        };
        let deleted = deleted.inline.expect("bounded fixture source");
        assert_eq!(deleted.user_mcp.revision, written_revision);
        assert!(deleted.user_mcp.authored.as_ref().unwrap().is_empty());
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg332_workspace_mcp_definition_and_policy_have_independent_authoring_owners() {
    use crate::local_runtime::configuration::settings::{ConfigMutation, McpWrite, SourceMutation};
    use crate::runtime::identity::McpServerId;
    bounded(async {
        let f = Fixture::new().await;
        let _id = f.sessions[0].id.clone();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let before = f
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        let MethodResult::SourcesWritten {
            revision: written_revision,
            ..
        } = call(
            &connection,
            1,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
                expected_revision: before.workspace_mcp.as_ref().unwrap().revision.clone(),
                mutation: SourceMutation::Mcp {
                    id: McpServerId::new("service"),
                    authored: Some(McpWrite {
                        definition: serde_json::from_value(
                            serde_json::json!({"command":"never-start-unselected-fixture"}),
                        )
                        .unwrap(),
                        retained_env: vec![],
                        retained_headers: vec![],
                    }),
                },
            },
        )
        .await
        else {
            panic!("source projection")
        };
        let MethodResult::SourceSettings {
            projection: definition,
            ..
        } = call(
            &connection,
            1001,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            },
        )
        .await
        else {
            panic!("explicit source reread after confirmed write")
        };
        let definition = definition.inline.expect("bounded fixture source");
        assert_eq!(
            definition.workspace_mcp.as_ref().unwrap().revision,
            written_revision
        );
        assert_eq!(
            definition.workspace.as_ref().unwrap().revision,
            before.workspace.as_ref().unwrap().revision
        );
        let MethodResult::SourcesWritten {
            revision: written_revision,
            ..
        } = call(
            &connection,
            2,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
                expected_revision: definition.workspace.as_ref().unwrap().revision.clone(),
                mutation: SourceMutation::Config {
                    mutation: ConfigMutation::McpPolicy {
                        id: McpServerId::new("service"),
                        authored: Some(crate::local_runtime::config::InvocationPolicyDocument {
                            approval: crate::local_runtime::config::ApprovalPolicyDocument::Always,
                            ..Default::default()
                        }),
                    },
                },
            },
        )
        .await
        else {
            panic!("policy projection")
        };
        let MethodResult::SourceSettings {
            projection: policy, ..
        } = call(
            &connection,
            1002,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            },
        )
        .await
        else {
            panic!("explicit source reread after confirmed write")
        };
        let policy = policy.inline.expect("bounded fixture source");
        assert_eq!(
            policy.workspace.as_ref().unwrap().revision,
            written_revision
        );
        assert_eq!(
            policy.workspace_mcp.as_ref().unwrap().revision,
            definition.workspace_mcp.as_ref().unwrap().revision
        );
        assert_ne!(
            policy.workspace.as_ref().unwrap().revision,
            before.workspace.as_ref().unwrap().revision
        );
        assert_eq!(policy.user.revision, before.user.revision);
        let target = attach(&connection, &f, 0).await;
        let MethodResult::EffectiveConfiguration { projection } =
            call(&connection, 3, Method::ConfigurationGet { target }).await
        else {
            panic!("native effective projection")
        };
        let projection = projection.inline.expect("bounded fixture configuration");
        assert!(projection.root_agent.tools.sources.is_empty());
        assert!(f.provider.request_bodies().is_empty());
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_connection_deletes_an_attached_idle_session_and_closes_its_route() {
    bounded(async {
        use crate::app_server::session_deletion::SessionDeletionResult as Deleted;
        let f = Fixture::new().await;
        let viewer = AppServerConnection::new(f.host.clone());
        let deleter = AppServerConnection::new(f.host.clone());
        initialize(&viewer).await;
        initialize(&deleter).await;
        let target = attach(&viewer, &f, 0).await;
        let MethodResult::Deletion {
            result: Deleted::Preview { preview },
        } = call(
            &deleter,
            900,
            Method::SessionDeletePreview {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("attached preview")
        };
        let result = call(
            &deleter,
            901,
            Method::SessionDelete {
                session_id: target.session_id.clone(),
                expected_target_revision: preview.target_revision,
            },
        )
        .await;
        assert!(matches!(
            result,
            MethodResult::Deletion {
                result: Deleted::Deleted { .. }
            }
        ));
        assert!(
            !f.manager
                .is_current(&target.conversation_id, target.runtime_incarnation)
        );
        assert!(f.manager.load(&target.session_id, None).await.is_err());
        loop {
            let notice = viewer.next_notification().await;
            if matches!(notice.notification, NotificationMethod::Closed { .. }) {
                break;
            }
        }
        assert_eq!(viewer.attachment_counts(), (0, 0));
        assert_eq!(
            rejected(&viewer, Method::TurnCancel { target }).await,
            ErrorData::StaleAttachment
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One native source proves both lineage destinations and Retry through the public protocol.
async fn completed_response_cut_is_shared_by_branch_and_fork_and_distinct_from_retry() {
    Box::pin(bounded(async {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
        use crate::local_runtime::session::LineageSide;
        use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, UserContentBlock, UserSource};
        use crate::message::TextBlock;
        use crate::model::finish::ModelFinishReason;
        use crate::runtime::identity::{AttemptId, EventId, MessageId};
        let f = Fixture::new().await;
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
            crate::runtime_client::response::tests::request(&store, attempt, 0, Some(crate::model::ModelUsage { input_tokens: 100, output_tokens: 20, total_tokens: 120, details: None }));
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
            let mut projected = crate::runtime_client::snapshot::transcript_page_view(copied.load_transcript_page(None, 64).unwrap()).unwrap();
            crate::runtime_client::response::native_oracle_decorate(&copied, &mut projected).unwrap();
            let tail = projected.entries.last().unwrap().completed_response.clone().unwrap();
            assert_eq!(tail.closing_message_id, Some(crate::conversation::message_id_of(&messages[3])));
            assert_eq!(tail.retry_message_id, Some(crate::conversation::message_id_of(&messages[2])));
            assert_eq!(tail.origin.conversation_id, source.active_conversation_id);
            assert_eq!(tail.usage.as_ref().unwrap().total_tokens, 120);
            assert_eq!(tail.timing.as_ref().unwrap().generation_ms, Some(1280));
            assert_eq!(projected.entries.iter().filter(|entry| entry.completed_response.is_some()).count(), 2);
            // As a Harness fork folds its copied prefix, the child's totals and
            // context reading include both inherited turns, without owning them.
            let totals = projected.statistics.unwrap();
            assert_eq!((totals.turns, totals.model_requests, totals.requests_with_usage, totals.completed_responses), (2, 2, 2, 2));
            assert_eq!(totals.reported_usage.unwrap().total_tokens, 240);
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
            let mut reopened_page = crate::runtime_client::snapshot::transcript_page_view(reopened.load_transcript_page(None, 64).unwrap()).unwrap();
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
        let mut retried = crate::runtime_client::snapshot::transcript_page_view(retry.load_transcript_page(None, 64).unwrap()).unwrap();
        crate::runtime_client::response::native_oracle_decorate(&retry, &mut retried).unwrap();
        let totals = retried.statistics.unwrap();
        assert_eq!((totals.turns, totals.model_requests, totals.completed_responses), (1, 1, 1));
        assert_eq!(totals.reported_usage.unwrap().total_tokens, 120);
        rejected(&connection, Method::SessionBranch { session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: crate::conversation::SurfaceRevision::new(revision.get() + 1), boundary: CanonicalContentLocator::ByMessageId {message_id:MessageId::new("assistant-b")}, side: LineageSide::After }).await;
        rejected(&connection, Method::SessionBranch { session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: revision, boundary: CanonicalContentLocator::ByMessageId {message_id:MessageId::new("user-b")}, side: LineageSide::After }).await;
        assert_eq!(store.load_canonical().unwrap(), original);
        drop(destination); drop(retry); drop(store); f.close().await;
    })).await;
}

#[tokio::test]
async fn archive_preflight_failures_reach_the_protocol_without_private_diagnostics() {
    use crate::durable::{ConversationStore, SqliteConversationStore};
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

fn wire_text(text: &str) -> Vec<UserInputBlock> {
    vec![UserInputBlock::Text(crate::message::content::TextBlock {
        text: text.into(),
    })]
}

/// Drains events until the named Session's current attempt settles: the
/// deterministic "turn completed" signal (a settled attempt provably follows
/// the canonical commit of its user boundary).
/// Captures execution ownership before admission and waits for both the native
/// handoff and its correlated display publication. Replacement checkpoints are
/// authoritative only for the same native attempt, never an earlier settlement.
struct AttemptCompletion {
    runtime: crate::runtime::ConversationRuntime,
    after: u64,
    target: AttachmentTarget,
    host: std::sync::Weak<crate::runtime_client::host::ClientInner>,
}

struct AttemptCompletionDiagnostic<'a> {
    completion: &'a AttemptCompletion,
    started: std::time::Instant,
    stage: &'static str,
    receipt_ordinal: Option<u64>,
    last_cursor: Option<crate::runtime_client::RuntimeClientCursor>,
    complete: bool,
}
impl Drop for AttemptCompletionDiagnostic<'_> {
    fn drop(&mut self) {
        if !self.complete {
            let source = self.completion.host.upgrade().map_or_else(
                || "host released".into(),
                |host| host.projection_source_debug(),
            );
            eprintln!(
                "incomplete fixture attempt: stage={} elapsed_us={} captured_after={} current_ordinal={} has_current_attempt={} receipt_ordinal={:?} last_cursor={:?} source={source:.4096}",
                self.stage,
                self.started.elapsed().as_micros(),
                self.completion.after,
                self.completion.runtime.settlement_ordinal_for_test(),
                self.completion.runtime.has_current_attempt(),
                self.receipt_ordinal,
                self.last_cursor,
            );
        }
    }
}

impl AttemptCompletion {
    fn capture(fixture: &Fixture, target: &AttachmentTarget) -> Self {
        let runtime = fixture
            .manager
            .configuration_runtime(&target.session_id)
            .unwrap();
        Self {
            after: runtime.settlement_ordinal_for_test(),
            runtime,
            target: target.clone(),
            host: fixture
                .manager
                .configuration_host(&target.session_id)
                .unwrap()
                .weak_inner(),
        }
    }

    async fn observed(self, connection: &AppServerConnection, expected: &AttachmentTarget) {
        let mut diagnostic = AttemptCompletionDiagnostic {
            completion: &self,
            started: std::time::Instant::now(),
            stage: "native handoff",
            receipt_ordinal: None,
            last_cursor: None,
            complete: false,
        };
        assert_eq!(expected.session_id, self.target.session_id);
        assert_eq!(expected.conversation_id, self.target.conversation_id);
        assert_eq!(
            expected.runtime_incarnation,
            self.target.runtime_incarnation
        );
        let receipt = self.runtime.child_attempt_settlement(self.after).await;
        diagnostic.receipt_ordinal = Some(receipt.ordinal);
        diagnostic.stage = "display publication";
        assert_eq!(receipt.ordinal, self.after + 1, "exact next native handoff");
        let mut last_cursor = None;
        loop {
            let notification = connection.next_notification().await.notification;
            match &notification {
                NotificationMethod::ObservationUnavailable { target, .. }
                | NotificationMethod::ResyncRequired { target, .. }
                | NotificationMethod::Closed { target }
                    if target == expected =>
                {
                    panic!(
                        "native settlement publication failed: receipt={receipt:?}, captured_after={}, last_cursor={last_cursor:?}, notification={notification:?}",
                        self.after,
                    );
                }
                _ => {}
            }
            let NotificationMethod::Event {
                target,
                cursor,
                event,
                ..
            } = notification
            else {
                continue;
            };
            if &target != expected {
                continue;
            }
            last_cursor = Some(cursor);
            diagnostic.last_cursor = last_cursor;
            if event.facts().iter().any(|fact| match fact {
                RuntimeClientEvent::AttemptSettled { attempt_id, .. } => {
                    *attempt_id == receipt.attempt_id
                }
                RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                    assert_eq!(checkpoint.cursor, cursor);
                    assert_eq!(checkpoint.snapshot.conversation_id, target.conversation_id);
                    checkpoint.snapshot.attempt.as_ref().is_some_and(|attempt| {
                        attempt.attempt_id.as_ref() == Some(&receipt.attempt_id)
                            && matches!(attempt.phase,
                                crate::runtime_client::snapshot::RuntimeClientAttemptPhase::Settled { .. })
                    })
                }
                _ => false,
            }) {
                diagnostic.complete = true;
                return;
            }
        }
    }
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

// P05 + P06 (runtime plane): the first turn's canonical commit publishes the
// normalized, 120-character-bounded projection while the turn is still
// parked; a later turn updates activity but never repaints the first-message preview.
#[tokio::test]
async fn the_first_turn_publishes_the_bounded_projection_and_later_turns_never_repaint() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        // Whitespace-heavy, multiline, multi-script, and long enough that the
        // 119th character is a non-space (so the kept prefix is not trimmed).
        let text = format!("  {}\n\n\t {}", "é".repeat(150), "汉".repeat(50));
        assert!(!text.contains("request-B"));
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            50,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text(&text),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        {
            let probe = probe.borrow();
            assert!(probe.attempted && probe.published);
        }
        // The companion rendering: the wire row must equal `preview_of` of
        // the same boundary, normalized to one line and bounded at 120 chars.
        let expected =
            crate::local_runtime::session::preview_of(&crate::message::types::UserMessageBlock {
                id: crate::runtime::identity::MessageId::new("p05-companion"),
                content: input(&text),
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: None,
            })
            .expect("text renders a line");
        assert_eq!(expected.chars().count(), 120);
        assert!(expected.ends_with('\u{2026}'));
        let before = crate::durable::conversation_store_opens_on_this_thread();
        let MethodResult::Sessions { sessions, .. } = call(
            &connection,
            51,
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
        let after = crate::durable::conversation_store_opens_on_this_thread();
        assert_eq!(after, before, "listing reads the persisted projection");
        let row = sessions
            .iter()
            .find(|row| row.id == target.session_id)
            .unwrap();
        assert_eq!(row.preview.as_deref(), Some(expected.as_str()));
        activity_published(&f, &target.session_id).await;
        let generation = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .document_generation();
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        // P06 (runtime plane): a second turn is an ordinary new commit, but
        // the one-shot publisher is spent — the settled first line survives
        // byte-identically while the new human activity commits separately.
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            53,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("a follow-up repaints nothing"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        completion.observed(&connection, &target).await;
        {
            let probe = probe.borrow();
            assert!(probe.finished && probe.published);
        }
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            53,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        activity_published(&f, &target.session_id).await;
        assert_eq!(summary.preview.as_deref(), Some(expected.as_str()));
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .document_generation(),
            generation + 1,
            "a later human turn publishes activity without repainting the preview"
        );
        f.close().await;
    })
    .await;
}

// P07 (runtime plane): a Session named before its first turn keeps the name
// and gains the first turn's projection underneath it.
#[tokio::test]
async fn a_projection_published_underneath_a_name_survives_over_the_wire() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::Session { session } = call(
            &connection,
            60,
            Method::SessionName {
                session_id: f.sessions[0].id.clone(),
                name: "kept through publication".into(),
            },
        )
        .await
        else {
            panic!("name")
        };
        assert_eq!(session.name.as_deref(), Some("kept through publication"));
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            61,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("named session first subject"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(probe.borrow().published);
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            62,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.name.as_deref(), Some("kept through publication"));
        assert_eq!(
            summary.preview.as_deref(),
            Some("named session first subject")
        );
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        f.close().await;
    })
    .await;
}

// P08 (runtime plane): once the root projection is settled, an agent-sourced
// boundary admitted to the same root runtime is just another commit — the
// projection stays byte-identical and the catalog generation never moves.
#[tokio::test]
async fn an_agent_sourced_boundary_never_repaints_the_settled_projection() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            63,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("human root subject"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(probe.borrow().published);
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        activity_published(&f, &target.session_id).await;
        let generation = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .document_generation();
        let live = f.manager.load(&target.session_id, None).await.unwrap();
        let runtime = live.inspect_runtime().expect("the runtime stays resident");
        let completion = AttemptCompletion::capture(&f, &target);
        runtime
            .submit_sourced_inbound(
                crate::message::types::UserSource::Agent {
                    agent_id: crate::runtime::identity::AgentId::new("parent-agent"),
                },
                input("an agent-authored repaint attempt"),
            )
            .unwrap();
        completion.observed(&connection, &target).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            64,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview.as_deref(), Some("human root subject"));
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .document_generation(),
            generation
        );
        f.close().await;
    })
    .await;
}

// P11: a failed projection commit (one-shot catalog write fault armed before
// the first turn) leaves canonical history untouched, is observable on the
// publisher probe, lists as `None`, and the explicit repair seam publishes
// exactly the first line afterwards.
#[tokio::test]
async fn a_failed_projection_commit_preserves_history_and_repairs_exactly_once() {
    bounded(async {
        use crate::durable::ConversationStore as _;
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let gate = crate::local_runtime::session_display_projection::publication_test_support::arm(
            &target.session_id,
        );
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            65,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("recovered after the fault"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        gate.parked(1).await;
        activity_published(&f, &target.session_id).await;
        // The independent activity write is settled; inject the fault into
        // exactly the parked preview publication.
        f.manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_before_rename();
        gate.release();
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        {
            let probe = probe.borrow();
            assert!(probe.attempted, "the commit rendered a line");
            assert!(!probe.published, "the armed fault failed the commit");
        }
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        // Canonical history is exactly the turn: one ordinary user boundary
        // and one assistant response.
        let path = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .database_path(&target.session_id, &target.conversation_id);
        let store =
            crate::durable::SqliteConversationStore::open(target.conversation_id.clone(), &path)
                .unwrap();
        let canonical = store.load_canonical().unwrap();
        assert_eq!(
            canonical
                .iter()
                .filter(|message| matches!(
                    message,
                    crate::message::types::MessageBlock::User(user)
                        if user.kind == crate::message::types::InboundKind::Message
                ))
                .count(),
            1
        );
        assert_eq!(
            canonical
                .iter()
                .filter(|message| matches!(
                    message,
                    crate::message::types::MessageBlock::Assistant(_)
                ))
                .count(),
            1
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            66,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview, None);
        // The fault was one-shot; the explicit repair seam derives and
        // publishes exactly the first line.
        assert_eq!(
            f.manager
                .sessions
                .repair_display_preview(&target.session_id)
                .await
                .unwrap(),
            crate::local_runtime::session_controller::DisplayPreviewRepair::Published
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            67,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(
            summary.preview.as_deref(),
            Some("recovered after the fault")
        );
        f.close().await;
    })
    .await;
}

// Image-only case (extra, app-server plane): a first boundary with no
// renderable text — an upload-only turn — settles the projection as absent
// through the publisher itself, and a later text turn never repaints it:
// the one-shot publisher is spent, repair/report seams treat the settled
// `None` as final.
#[tokio::test]
async fn an_upload_only_first_turn_settles_the_projection_absent_and_never_repaints() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let files = binary_upload(
            &connection,
            target.clone(),
            "pixel.png",
            &[137_u8, 80, 78, 71, 13, 10, 26, 10],
        )
        .await;
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            71,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Upload(files[0].receipt.clone())],
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        {
            let probe = probe.borrow();
            assert!(!probe.attempted, "no renderable text means no attempt");
            assert!(!probe.published);
        }
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            72,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview, None);
        // The one-shot publisher is spent: a later text turn commits, but
        // the settled `None` never gets repainted.
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            73,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("text after the upload"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        completion.observed(&connection, &target).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            74,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview, None);
        f.close().await;
    })
    .await;
}

/// Issue #386 regression A (native half): the real window between the
/// canonical commit and the display-projection commit.
///
/// The one-shot publisher is parked *after* it renders its line and *before*
/// it takes the catalog mutex, so an ordinary `session/summary` read really is
/// served — and really does answer `preview: null` — while publication is
/// still outstanding. Releasing publication then produces the Session-scoped
/// metadata invalidation a live client needs, with no further user turn and no
/// client-initiated retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_publication_serves_a_null_summary_then_invalidates_it() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let gate = crate::local_runtime::session_display_projection::publication_test_support::arm(
            &target.session_id,
        );
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            80,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("live convergence subject"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        // The provider request proves the canonical user boundary committed.
        f.gates[0].wait_entered().await;
        // The publisher has derived its line and is parked before the catalog
        // mutex. It therefore cannot be blocking the read below.
        gate.parked(1).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            81,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(
            summary.preview, None,
            "canonical history exists and the projection is still unpublished"
        );

        // Consume the independently committed activity invalidation while the
        // preview is still parked, so coalescing cannot merge the two facts.
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
        gate.release();
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(probe.borrow().published);
        // The native metadata owner announces the change by Session identity.
        let invalidated = loop {
            if let NotificationMethod::SummaryInvalidated {
                session_id,
                catalog_changed,
            } = connection.next_notification().await.notification
            {
                // The human commit separately invalidates recency. Wait for
                // the preview-only invalidation after releasing its gate.
                if !catalog_changed {
                    break session_id;
                }
            }
        };
        assert_eq!(
            invalidated, target.session_id,
            "the invalidation is routed by Session identity, not by attachment target"
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            82,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(
            summary.preview.as_deref(),
            Some("live convergence subject"),
            "the authoritative reread carries the committed projection"
        );
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        f.close().await;
    })
    .await;
}

/// Issue #386 regression (App Server plane): a Session cold-loaded straight
/// onto a branch repairs its **root** subject, arms no publisher on the branch,
/// and writes nothing the second time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_loading_a_branch_repairs_the_projection_from_the_session_root() {
    bounded(async {
        use crate::durable::ConversationStore as _;
        let f = Fixture::new().await;
        let sessions = &f.manager.sessions;
        let session = f.sessions[0].clone();
        let root_node = session.active_node.clone();
        let text = |id: &str, text: &str| {
            crate::message::types::MessageBlock::User(crate::message::types::UserMessageBlock {
                id: crate::runtime::identity::MessageId::new(id),
                content: input(text),
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: None,
            })
        };
        let store = |access: &crate::local_runtime::session_controller::SessionAccess| {
            crate::durable::SqliteConversationStore::open(
                access.node.conversation_id.clone(),
                &access.database_path,
            )
            .unwrap()
        };
        let boundary = crate::runtime::identity::MessageId::new("root-subject-a");
        let revision = {
            let access = sessions
                .acquire_session(&session.id, Some(&root_node))
                .await
                .unwrap();
            let root = store(&access);
            root.append_canonical(&text("root-subject-a", "root subject A"))
                .unwrap();
            root.load_head().unwrap().revision
        };
        // Cut before the root's only boundary: the branch retains no root
        // message and can disagree with the root about its first message.
        let branch = sessions
            .branch_session_node(&session.id, &root_node, revision, &boundary)
            .await
            .unwrap()
            .session;
        let branch_node = branch.active_node.clone();
        let branch_canonical = {
            let access = sessions
                .acquire_session(&session.id, Some(&branch_node))
                .await
                .unwrap();
            let branch = store(&access);
            branch
                .append_canonical(&text("branch-subject-z", "branch subject Z"))
                .unwrap();
            branch.load_canonical().unwrap()
        };
        assert_eq!(
            sessions
                .read_session_summary(&session.id)
                .await
                .unwrap()
                .preview,
            None,
            "the Session starts with a missing projection"
        );

        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::Attached { target, .. } = call(
            &connection,
            90,
            Method::SessionAttach {
                session_id: session.id.clone(),
                node_id: Some(branch_node.clone()),
            },
        )
        .await
        else {
            panic!("attached")
        };
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            91,
            Method::SessionSummary {
                session_id: session.id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(
            summary.preview.as_deref(),
            Some("root subject A"),
            "the repair derives the Session's root subject, never the composed branch's"
        );
        assert!(
            crate::local_runtime::session_display_projection::display_projection_probe(&session.id)
                .is_none(),
            "a branch runtime is never the projection subject, so no publisher is armed"
        );
        // The branch's canonical history is untouched by Session metadata repair.
        assert_eq!(
            store(
                &sessions
                    .acquire_session(&session.id, Some(&branch_node))
                    .await
                    .unwrap()
            )
            .load_canonical()
            .unwrap(),
            branch_canonical
        );

        // Write-level idempotence across a *second cold composition*, not a
        // detach/reattach cycle: detach only releases an external claim, so the
        // branch runtime is unloaded through its real owner and its retirement
        // is proven before the repair-capable composition runs again.
        let generation = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .document_generation();
        let invalidations = sessions.summary_invalidations();
        let recorded = invalidations.recorded();
        let before = sessions.read_session(&session.id).await.unwrap();
        let revision_before = sessions.read_settings(&session.id).await.unwrap().0;
        let preview_before = sessions
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .preview;
        let root_canonical_before = store(
            &sessions
                .acquire_session(&session.id, Some(&root_node))
                .await
                .unwrap(),
        )
        .load_canonical()
        .unwrap();
        let conversation = target.conversation_id.clone();
        let incarnation = target.runtime_incarnation;
        let probe = f.manager.probe(&conversation);
        let compositions = probe.compositions.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(compositions, 1, "the branch composed exactly once so far");
        assert!(matches!(
            call(&connection, 92, Method::SessionDetach { target }).await,
            MethodResult::Detached {}
        ));
        f.manager.unload(&conversation).await.unwrap();
        assert_eq!(
            f.manager.residency(&conversation),
            crate::local_runtime::session_runtime_manager::ResidencyState::Unloaded,
            "the branch runtime is retired, not merely detached"
        );
        let MethodResult::Attached { target, .. } = call(
            &connection,
            93,
            Method::SessionAttach {
                session_id: session.id.clone(),
                node_id: Some(branch_node.clone()),
            },
        )
        .await
        else {
            panic!("attached")
        };
        assert_ne!(
            target.runtime_incarnation, incarnation,
            "reopening the branch produced a new runtime incarnation"
        );
        assert_eq!(
            probe.compositions.load(std::sync::atomic::Ordering::SeqCst),
            compositions + 1,
            "a second real composition ran the repair-capable startup path again"
        );
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .document_generation(),
            generation,
            "an already-correct projection is a write-level no-op"
        );
        assert_eq!(
            invalidations.recorded(),
            recorded,
            "a no-op repair announces nothing"
        );
        let after = sessions.read_session(&session.id).await.unwrap();
        assert_eq!(after.updated_at, before.updated_at);
        assert_eq!(
            sessions.read_settings(&session.id).await.unwrap().0,
            revision_before
        );
        assert_eq!(
            sessions
                .read_session_summary(&session.id)
                .await
                .unwrap()
                .preview,
            preview_before,
            "the already-correct root preview survives the second composition"
        );
        assert_eq!(
            store(
                &sessions
                    .acquire_session(&session.id, Some(&root_node))
                    .await
                    .unwrap()
            )
            .load_canonical()
            .unwrap(),
            root_canonical_before,
            "the root's canonical history is unchanged by the second repair"
        );
        assert_eq!(
            store(
                &sessions
                    .acquire_session(&session.id, Some(&branch_node))
                    .await
                    .unwrap()
            )
            .load_canonical()
            .unwrap(),
            branch_canonical
        );
        assert!(
            crate::local_runtime::session_display_projection::display_projection_probe(&session.id)
                .is_none(),
            "the reopened branch still arms no live root-preview publisher"
        );
        assert!(matches!(
            call(&connection, 94, Method::SessionDetach { target }).await,
            MethodResult::Detached {}
        ));
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
async fn ux04_initialize_identity_is_owned_by_host_not_connection_or_endpoint() {
    let f = Fixture::new().await;
    let replacement = crate::app_server::host::AppServerHost::new(
        f.manager.clone(),
        crate::local_runtime::app_server_policy::AppServerPolicy::default(),
    );
    let mut identities = Vec::new();
    for host in [f.host.clone(), f.host.clone(), replacement] {
        let connection = AppServerConnection::new(host);
        let result = call(
            &connection,
            0,
            Method::Initialize(InitializeParams {
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                client: ClientIdentity {
                    name: "identity-test".into(),
                    version: "1".into(),
                },
                presentation: PresentationCapabilities::default(),
            }),
        )
        .await;
        let MethodResult::Initialized { authority_id, .. } = result else {
            panic!("initialize result")
        };
        assert!(!authority_id.is_empty());
        identities.push(authority_id);
        connection.close();
    }
    assert_eq!(identities[0], identities[1]);
    assert_ne!(identities[0], identities[2]);
    assert!(f.provider.request_bodies().is_empty());
    f.close().await;
}

// Real transport-authenticated Product Host, never a second ordinary connection.
const FILE_HOST_TOKEN: &str = "host-secret-000000000000000000000000000000000000000";
const FILE_BROWSER_TOKEN: &str = "browser-secret-0000000000000000000000000000000000000";
type FileSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
struct TrustedFileHost {
    endpoint: String,
    stop: tokio_util::sync::CancellationToken,
    serving: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl TrustedFileHost {
    async fn new(f: &Fixture) -> Self {
        use crate::app_server::transport::websocket;
        f.host.bind_product_host(Some(
            websocket::Credential::new(FILE_HOST_TOKEN.into()).unwrap(),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "ws://{}/product-host/file-read",
            listener.local_addr().unwrap()
        );
        let stop = tokio_util::sync::CancellationToken::new();
        let serving = tokio::spawn(websocket::serve(
            listener,
            f.host.clone(),
            websocket::Credential::new(FILE_BROWSER_TOKEN.into()).unwrap(),
            stop.clone(),
        ));
        Self {
            endpoint,
            stop,
            serving,
        }
    }
    async fn open(
        &self,
        credential: &str,
    ) -> Result<FileSocket, tokio_tungstenite::tungstenite::Error> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = self.endpoint.as_str().into_client_request().unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            format!("rustx.product-host.file-read.v4, rustx-product-host.{credential}")
                .parse()
                .unwrap(),
        );
        let (socket, reply) = tokio_tungstenite::connect_async(request).await?;
        assert_eq!(
            reply.headers()["sec-websocket-protocol"],
            "rustx.product-host.file-read.v4"
        );
        Ok(socket)
    }
    async fn response(&self, read: crate::app_server::product_host::FileRead) -> Response {
        use futures_util::{SinkExt, StreamExt};
        let mut socket = self.open(FILE_HOST_TOKEN).await.unwrap();
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&read).unwrap().into(),
            ))
            .await
            .unwrap();
        let frame = socket.next().await.unwrap().unwrap();
        let response = serde_json::from_str(frame.to_text().unwrap()).unwrap();
        let _ = socket.close(None).await;
        response
    }
    async fn success(&self, read: crate::app_server::product_host::FileRead) -> MethodResult {
        match self.response(read).await {
            Response::Success(reply) => reply.result,
            Response::Failure(reply) => panic!("trusted file read failed: {:?}", reply.error),
        }
    }
    async fn rejected(&self, read: crate::app_server::product_host::FileRead) -> ErrorData {
        match self.response(read).await {
            Response::Failure(reply) => reply.error.data.unwrap(),
            Response::Success(_) => panic!("file read unexpectedly succeeded"),
        }
    }
    async fn close(self) {
        self.stop.cancel();
        self.serving.await.unwrap().unwrap();
    }
}

// Shared native fixture scenario; the boundary-suite owner runs it on Linux and macOS.
pub(crate) async fn committed_present_read_boundary_scenario() {
    use crate::tools::session_files::SessionFileReadFailure as FileFailure;
    use base64::Engine;
    use futures_util::{SinkExt, StreamExt};
    bounded(async {
        let mut phases = FixturePhase::new("committed present boundary");
        phases.enter("fixture construction");
        let f = Fixture::with_tool(Some("present")).await;
        phases.enter("product host listener");
        let path = f.workspaces[0].join("报告 file.md");
        let original = b"# Original\r\n\r\nNative bytes\r\n";
        std::fs::write(&path, original).unwrap();
        let browser = AppServerConnection::new(f.host.clone());
        let product_host = TrustedFileHost::new(&f).await;
        phases.enter("initialize and attach");
        initialize(&browser).await;
        let target = attach(&browser, &f, 0).await;
        let read = |message_id| crate::app_server::product_host::FileRead {
            target: target.clone(),
            source: crate::app_server::product_host::ReadSource::SessionFile { agent_id: None, locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId { message_id }, delivery_index: 0 },
            roots: vec![f.workspaces[0].clone()],
        };
        phases.enter("pre-turn file refusal");
        assert_eq!(
            product_host.rejected(
                read(crate::runtime::identity::MessageId::new("invented"))
            )
            .await,
            ErrorData::SessionFileRead {
                reason: FileFailure::Unavailable
            }
        );
        phases.enter("present attempt settlement");
        let completion = AttemptCompletion::capture(&f, &target);
    f.gates[0].release();
        call(
            &browser,
            4100,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("request-A"),
            },
        )
        .await;
        completion.observed(&browser, &target).await;
        phases.enter("committed checkpoint and metadata");
        let MethodResult::Checkpoint { checkpoint: checkpoint_13 } = call(
            &browser,
            4101,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!()
        };
        let snapshot = Box::new(checkpoint_13.snapshot);
        let tool = checkpoint_delivery_metadata(&browser, &target, &snapshot).await;
        assert_eq!(
            tool.result.status,
            crate::tools::ToolExecutionStatus::Success
        );
        assert_eq!(tool.result.deliveries.len(), 1);
        let file = tool.result.deliveries[0].clone();
        assert_eq!(file.scope.conversation_id, target.conversation_id);
        assert_eq!(file.name, "报告 file.md");
        assert!(tool.result.artifacts.is_empty());
        phases.enter("original bytes and authority checks");
        let requests = f.provider.request_bodies().len();
        assert_eq!(requests, 2);
        let MethodResult::SessionFileBytes {
            file: returned,
            data,
        } = product_host.success( read(tool.id.clone())).await
        else {
            panic!()
        };
        assert_eq!(returned, file);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
            original
        );
        assert_eq!(
            browser.attachment_counts(),
            (1, 0),
            "file reads create no new attachment"
        );
        // Browser has every valid coordinate and the exact cwd; neither its
        // shared transport token nor a claimed Product Host name grants bytes.
        let forged = serde_json::json!({"jsonrpc":"2.0", "id":4190, "method":"session/fileRead", "params":{
            "target":target, "locator":{"type":"by_message_id","message_id":tool.id}, "delivery_index":0, "allowed_roots":[f.workspaces[0]], "file":file
        }});
        let Some(Response::Failure(denied)) = browser.handle_json(&forged.to_string()).await else { panic!("ordinary file bypass succeeded") };
        assert_eq!(denied.error.code, -32601);
        assert!(product_host.open(FILE_BROWSER_TOKEN).await.is_err(), "normal transport credential cannot authenticate private file seam");
        // Older paging carries the same typed fact after activity folds.
        let MethodResult::TranscriptWindow { window } = call(&browser, 4120, Method::Transcript {
            target: target.clone(), at: crate::durable::reading::ConversationWindowAt::Latest, limit: 1,
        }).await else { panic!() };
        let MethodResult::TranscriptWindow { window: older } = call(&browser, 4121, Method::Transcript {
            target: target.clone(), at: crate::durable::reading::ConversationWindowAt::Older { cut: None,
                before: window.page.entries[0].cursor.into(),
            }, limit: 64,
        }).await else { panic!() };
        let older_reference = older.page.content.iter().find(|binding| matches!(&binding.content_ref.source,
            crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
                component: crate::runtime_client::content::CanonicalContentComponent::MessageMetadata,
                locator: CanonicalContentLocator::ByMessageId { message_id }, ..
            } if message_id == &tool.id)).expect("older exact canonical Tool metadata source");
        let older_tool = read_delivery_metadata(&browser, &target, &older_reference.content_ref).await;
        assert_eq!(older_tool.id, tool.id);
        assert_eq!(older_tool.result.deliveries.as_slice(), std::slice::from_ref(&file));
        phases.enter("fork and inherited bytes");
        let response_boundary = delivery_response_boundary(&browser, &target).await;
        let MethodResult::SessionTransition { session: fork, .. } = call(&browser, 4122, Method::SessionFork {
            session_id: target.session_id.clone(), node_id: Some(f.sessions[0].active_node.clone()),
            surface_revision: response_boundary.surface_revision,
            boundary: Some(response_boundary.closing_locator.clone()),
            side: crate::local_runtime::session::LineageSide::After,
        }).await else { panic!() };
        let fork_target = attach_session(&browser, &fork).await;
        assert_eq!(fork_target.attachment_id, target.attachment_id, "native local attachment ordinals intentionally collide across Conversations");
        let MethodResult::Checkpoint { checkpoint: checkpoint_14 } = call(&browser, 4123, Method::SessionCheckpoint {
            target: fork_target.clone(), trace_records: vec![],
        }).await else { panic!() };
        let copied = Box::new(checkpoint_14.snapshot);
        let inherited = checkpoint_delivery_metadata(&browser, &fork_target, &copied).await;
        assert_eq!(inherited.result.deliveries.as_slice(), std::slice::from_ref(&file));
        assert_ne!(fork_target.conversation_id, file.scope.conversation_id);
        let MethodResult::SessionFileBytes { file: inherited_file, data } = product_host.success( crate::app_server::product_host::FileRead {
            target: fork_target, source: crate::app_server::product_host::ReadSource::SessionFile { agent_id: None, locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId { message_id: inherited.id.clone() }, delivery_index: 0 }, roots: vec![f.workspaces[0].clone()],
        }).await else { panic!() };
        assert_eq!(inherited_file, file);
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(data).unwrap(), original);
        // A same-named unrelated Session cannot resolve invented source coordinates.
        std::fs::write(f.workspaces[1].join(&file.path), b"UNRELATED").unwrap();
        let unrelated = attach_session(&browser, &f.sessions[1]).await;
        assert_eq!(product_host.rejected( crate::app_server::product_host::FileRead {
            target: unrelated, source: crate::app_server::product_host::ReadSource::SessionFile { agent_id: None, locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId { message_id: tool.id.clone() }, delivery_index: 0 }, roots: vec![f.workspaces[1].clone()],
        }).await, ErrorData::SessionFileRead { reason: FileFailure::Unavailable });
        let mut denied = read(tool.id.clone());
        denied.roots = vec![f.workspaces[1].clone()];
        assert_eq!(
            product_host.rejected( denied).await,
            ErrorData::SessionFileRead {
                reason: FileFailure::Unauthorized
            }
        );
        assert_eq!(
            rejected(
                &browser,
                Method::ArtifactRead {
                    target: target.clone(),
                    artifact_id: crate::runtime::ArtifactId::new(file.path.clone())
                }
            )
            .await,
            ErrorData::InvalidState
        );
        phases.enter("mutable file and capacity checks");
        std::fs::write(&path, vec![b'x'; 300 * 1024]).unwrap();
        let MethodResult::SessionFileBytes { data, .. } =
            product_host.success( read(tool.id.clone())).await
        else {
            panic!()
        };
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap()
                .len(),
            300 * 1024
        );
        let permits = f.host.file_reads();
        let one = permits.clone().acquire_owned().await.unwrap();
        let two = permits.clone().acquire_owned().await.unwrap();
        assert_eq!(
            product_host.rejected( read(tool.id.clone())).await,
            ErrorData::SessionFileRead {
                reason: FileFailure::Capacity
            }
        );
        drop(one);
        drop(two);
        assert_eq!(
            permits.available_permits(),
            crate::tools::session_files::SESSION_FILE_MAX_READS
        );
        std::fs::write(
            &path,
            vec![b'x'; crate::tools::session_files::SESSION_FILE_MAX_BYTES + 1],
        )
        .unwrap();
        assert_eq!(
            product_host.rejected( read(tool.id.clone())).await,
            ErrorData::SessionFileRead {
                reason: FileFailure::TooLarge
            }
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            product_host.rejected( read(tool.id.clone())).await,
            ErrorData::SessionFileRead {
                reason: FileFailure::Missing
            }
        );
        std::fs::write(&path, original).unwrap();
        let probe = f.host.file_read_probe();
        for revoked in ["credential", "socket", "attachment"] {
            phases.enter("revocation open and before-bytes admission");
            let mut completed = probe.completed.subscribe();
            probe.completed.send_replace(None);
            let mut retirement_waiting = probe.retirement_waiting.subscribe();
            probe.retirement_waiting.send_replace(false);
            let gate_release = probe.before_bytes.arm_scoped();
            let mut pending = product_host.open(FILE_HOST_TOKEN).await.unwrap();
            pending.send(tokio_tungstenite::tungstenite::Message::Text(serde_json::to_string(&read(tool.id.clone())).unwrap().into())).await.unwrap();
            let gate = probe.before_bytes.clone();
            tokio::task::spawn_blocking(move || gate.wait_entered()).await.unwrap();
            assert_eq!(*completed.borrow(), None, "open descriptor has not published bytes");
            phases.enter("revoke exact authority");
            if revoked == "credential" {
                f.host.bind_product_host(None);
                assert!(product_host.open(FILE_HOST_TOKEN).await.is_err(), "revoked secret admits no later read");
            } else if revoked == "socket" {
                // This is the exact native fence used when Node aborts after
                // registration removal or Host replacement, not a clock wait.
                pending.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                    reason: "".into(),
                })).await.unwrap();
                let authority = probe.authority.lock().unwrap().clone().unwrap();
                authority.cancelled().await;
                // Observe the exact retirement future being polled Pending
                // while descriptor read is held. The transport cannot ACK a
                // clean close and release Node admission before this settles.
                retirement_waiting.wait_for(|waiting| *waiting).await.unwrap();
                assert_eq!(*completed.borrow(), None);
                assert_eq!(f.host.file_reads().available_permits(), crate::tools::session_files::SESSION_FILE_MAX_READS - 1);
                assert!(futures_util::poll!(pending.next()).is_pending(), "close acknowledgement waits for the native read permit");
            } else {
                call(&browser, 4193, Method::SessionDetach { target: target.clone() }).await;
            }
            phases.enter("physical read settlement");
            drop(gate_release);
            completed.wait_for(Option::is_some).await.unwrap();
            assert_eq!(*completed.borrow(), Some(false), "{revoked}: native read publishes no bytes after revocation");
            if revoked == "socket" {
                let frame = pending.next().await.unwrap().unwrap();
                assert!(matches!(frame, tokio_tungstenite::tungstenite::Message::Close(Some(frame)) if frame.code == tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal));
                assert_eq!(f.host.file_reads().available_permits(), crate::tools::session_files::SESSION_FILE_MAX_READS, "clean native close is a physical permit retirement witness");
                assert!(matches!(product_host.success(read(tool.id.clone())).await, MethodResult::SessionFileBytes { .. }), "a replacement read is admitted after the close acknowledgement");
            }
            phases.enter("socket retirement");
            while let Some(frame) = pending.next().await {
                if let Ok(tokio_tungstenite::tungstenite::Message::Text(text)) = frame {
                    let reply: Response = serde_json::from_str(&text).unwrap();
                    assert!(matches!(reply, Response::Failure(_)), "{revoked}: no successful bytes may cross socket");
                }
            }
            assert_eq!(f.host.file_reads().available_permits(), crate::tools::session_files::SESSION_FILE_MAX_READS);
            if revoked == "credential" { f.host.bind_product_host(Some(crate::app_server::transport::websocket::Credential::new(FILE_HOST_TOKEN.into()).unwrap())); }
        }
        assert_eq!(
            product_host.rejected( read(tool.id.clone())).await,
            ErrorData::StaleAttachment
        );
        assert_eq!(
            f.provider.request_bodies().len(),
            requests,
            "all file reads issue zero model requests"
        );
        phases.enter("close product host");
        browser.close();
        product_host.close().await;
        phases.enter("unload fork");
        f.manager.unload(&fork.active_conversation_id).await.unwrap();
        phases.enter("close fixture");
        f.close().await;
        phases.finish();
    })
    .await;
}

// Transport-granted delivery access on an ordinary connection. The same native
// core as the Product Host lane, without its registered-root policy.
pub(crate) async fn delivery_access_boundary_scenario() {
    use crate::tools::session_files::SessionFileReadFailure as FileFailure;
    use base64::Engine;
    use std::os::unix::fs::MetadataExt;
    async fn initialize_as(connection: &AppServerConnection, name: &str) -> MethodResult {
        call(
            connection,
            0,
            Method::Initialize(InitializeParams {
                protocol_version: crate::app_server::protocol::APP_SERVER_PROTOCOL_VERSION,
                // Client names are metadata; a trusted-looking one grants nothing.
                client: ClientIdentity {
                    name: name.into(),
                    version: "1".into(),
                },
                presentation: PresentationCapabilities::default(),
            }),
        )
        .await
    }
    bounded(async {
        let f = Fixture::with_tool(Some("present")).await;
        let path = f.workspaces[0].join("报告 file.md");
        let original = b"# Original\r\n\r\nNative bytes\r\n";
        std::fs::write(&path, original).unwrap();
        let ordinary = AppServerConnection::new(f.host.clone());
        let MethodResult::Initialized { capabilities, .. } =
            initialize_as(&ordinary, "rustx-tui").await
        else {
            panic!("initialized")
        };
        assert!(!capabilities.delivery_access);
        let authority = tokio_util::sync::CancellationToken::new();
        let trusted = AppServerConnection::with_delivery_access(f.host.clone(), authority.clone());
        let MethodResult::Initialized { capabilities, .. } =
            initialize_as(&trusted, "anything").await
        else {
            panic!("initialized")
        };
        assert!(capabilities.delivery_access);
        let MethodResult::ServerInfo { capabilities } =
            call(&trusted, 1, Method::ServerInfo {}).await
        else {
            panic!("server info")
        };
        assert!(capabilities.delivery_access);
        let target = attach(&ordinary, &f, 0).await;
        // A pre-commit or invented delivery coordinate is not a delivery.
        assert_eq!(
            rejected(
                &ordinary,
                Method::DeliveryRead {
                    target: target.clone(),
                    locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                        message_id: crate::runtime::identity::MessageId::new("invented")
                    },
                    delivery_index: 0,
                }
            )
            .await,
            ErrorData::SessionFileRead {
                reason: FileFailure::Unauthorized
            }
        );
        let completion = AttemptCompletion::capture(&f, &target);
        f.gates[0].release();
        call(
            &ordinary,
            4300,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("request-A"),
            },
        )
        .await;
        completion.observed(&ordinary, &target).await;
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_15,
        } = call(
            &ordinary,
            4301,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!()
        };
        let snapshot = Box::new(checkpoint_15.snapshot);
        let tool = checkpoint_delivery_metadata(&ordinary, &target, &snapshot).await;
        let file = tool.result.deliveries[0].clone();
        let requests = f.provider.request_bodies().len();
        // Every exact coordinate, the exact cwd and a forged trusted client
        // name still grant an ordinary authenticated connection nothing.
        for method in [
            Method::DeliveryRead {
                target: target.clone(),
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id: tool.id.clone(),
                },
                delivery_index: 0,
            },
            Method::DeliveryLocate {
                target: target.clone(),
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id: tool.id.clone(),
                },
                delivery_index: 0,
            },
        ] {
            assert_eq!(
                rejected(&ordinary, method).await,
                ErrorData::SessionFileRead {
                    reason: FileFailure::Unauthorized
                }
            );
        }
        // Access never reaches another connection's attachment.
        assert_eq!(
            rejected(
                &trusted,
                Method::DeliveryRead {
                    target: target.clone(),
                    locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                        message_id: tool.id.clone()
                    },
                    delivery_index: 0,
                }
            )
            .await,
            ErrorData::StaleAttachment
        );
        call(
            &ordinary,
            4302,
            Method::SessionDetach {
                target: target.clone(),
            },
        )
        .await;
        let own = attach(&trusted, &f, 0).await;
        let read = |message_id: crate::runtime::identity::MessageId, delivery_index| {
            Method::DeliveryRead {
                target: own.clone(),
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id,
                },
                delivery_index,
            }
        };
        let MethodResult::SessionFileBytes {
            file: returned,
            data,
        } = call(&trusted, 4303, read(tool.id.clone(), 0)).await
        else {
            panic!("bytes")
        };
        assert_eq!(returned, file);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
            original
        );
        let MethodResult::SessionFileLocation {
            file: located,
            path: native,
            device,
            inode,
        } = call(
            &trusted,
            4304,
            Method::DeliveryLocate {
                target: own.clone(),
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id: tool.id.clone(),
                },
                delivery_index: 0,
            },
        )
        .await
        else {
            panic!("location")
        };
        let leaf = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(located, file);
        assert_eq!(
            std::path::PathBuf::from(native),
            std::fs::canonicalize(&path).unwrap()
        );
        assert_eq!(
            (device, inode),
            (leaf.dev().to_string(), leaf.ino().to_string())
        );
        for (message_id, index, expected) in [
            (
                crate::runtime::identity::MessageId::new("invented"),
                0,
                ErrorData::SessionFileRead {
                    reason: FileFailure::Unavailable,
                },
            ),
            (
                tool.id.clone(),
                1,
                ErrorData::SessionFileRead {
                    reason: FileFailure::Unavailable,
                },
            ),
            (
                tool.id.clone(),
                8,
                ErrorData::SessionFileRead {
                    reason: FileFailure::Unavailable,
                },
            ),
        ] {
            assert_eq!(rejected(&trusted, read(message_id, index)).await, expected);
        }
        if let Ok(index) =
            usize::try_from(crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX + 1)
        {
            assert_eq!(
                rejected(&trusted, read(tool.id.clone(), index)).await,
                ErrorData::InvalidParams
            );
        }
        // A fork addresses its own copied message but resolves the original
        // Conversation and root, never a same-named file in an unrelated root.
        std::fs::write(f.workspaces[1].join(&file.path), b"UNRELATED").unwrap();
        let response_boundary = delivery_response_boundary(&trusted, &own).await;
        let MethodResult::SessionTransition { session: fork, .. } = call(
            &trusted,
            4305,
            Method::SessionFork {
                session_id: own.session_id.clone(),
                node_id: Some(f.sessions[0].active_node.clone()),
                surface_revision: response_boundary.surface_revision,
                boundary: Some(response_boundary.closing_locator.clone()),
                side: crate::local_runtime::session::LineageSide::After,
            },
        )
        .await
        else {
            panic!("fork")
        };
        let fork_target = attach_session(&trusted, &fork).await;
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_16,
        } = call(
            &trusted,
            4310,
            Method::SessionCheckpoint {
                target: fork_target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("fork snapshot")
        };
        let copied = Box::new(checkpoint_16.snapshot);
        let copied = checkpoint_delivery_metadata(&trusted, &fork_target, &copied).await;
        let MethodResult::SessionFileBytes {
            file: inherited,
            data,
        } = call(
            &trusted,
            4306,
            Method::DeliveryRead {
                target: fork_target.clone(),
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id: copied.id,
                },
                delivery_index: 0,
            },
        )
        .await
        else {
            panic!("fork bytes")
        };
        assert_eq!(inherited.scope.conversation_id, target.conversation_id);
        assert_ne!(fork_target.conversation_id, inherited.scope.conversation_id);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
            original
        );
        let unrelated = attach_session(&trusted, &f.sessions[1]).await;
        assert_eq!(
            rejected(
                &trusted,
                Method::DeliveryRead {
                    target: unrelated.clone(),
                    locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                        message_id: tool.id.clone()
                    },
                    delivery_index: 0
                }
            )
            .await,
            ErrorData::SessionFileRead {
                reason: FileFailure::Unavailable
            }
        );
        // Mutable source: reopening observes current bytes; oversize remains
        // declarable and locatable but fails a byte read explicitly.
        std::fs::write(
            &path,
            vec![b'x'; crate::tools::session_files::SESSION_FILE_MAX_BYTES + 1],
        )
        .unwrap();
        assert_eq!(
            rejected(&trusted, read(tool.id.clone(), 0)).await,
            ErrorData::SessionFileRead {
                reason: FileFailure::TooLarge
            }
        );
        assert!(matches!(
            call(
                &trusted,
                4307,
                Method::DeliveryLocate {
                    target: own.clone(),
                    locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                        message_id: tool.id.clone()
                    },
                    delivery_index: 0
                }
            )
            .await,
            MethodResult::SessionFileLocation { .. }
        ));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            rejected(&trusted, read(tool.id.clone(), 0)).await,
            ErrorData::SessionFileRead {
                reason: FileFailure::Missing
            }
        );
        std::fs::write(&path, original).unwrap();
        // Bounded: the two native permits are shared with the Product Host.
        let permits = f.host.file_reads();
        let held = [
            permits.clone().acquire_owned().await.unwrap(),
            permits.clone().acquire_owned().await.unwrap(),
        ];
        assert_eq!(
            rejected(&trusted, read(tool.id.clone(), 0)).await,
            ErrorData::SessionFileRead {
                reason: FileFailure::Capacity
            }
        );
        drop(held);
        // Revocation after descriptor open but before bytes: closing the
        // connection cancels its transport authority; the admitted operation
        // publishes nothing and releases its permit only on settlement.
        let probe = f.host.file_read_probe();
        let mut completed = probe.completed.subscribe();
        probe.completed.send_replace(None);
        let gate_release = probe.before_bytes.arm_scoped();
        let pending = {
            let trusted = trusted.clone();
            let request = read(tool.id.clone(), 0);
            tokio::spawn(async move {
                trusted
                    .handle_request(Request {
                        jsonrpc: JsonRpcVersion::V2,
                        id: RequestId::Integer(4308),
                        call: request,
                    })
                    .await
            })
        };
        let gate = probe.before_bytes.clone();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        assert_eq!(
            *completed.borrow(),
            None,
            "open descriptor has not published bytes"
        );
        trusted.close();
        assert!(
            authority.is_cancelled(),
            "connection close revokes its delivery authority"
        );
        assert_eq!(
            permits.available_permits(),
            crate::tools::session_files::SESSION_FILE_MAX_READS - 1,
            "the blocked descriptor read still owns its permit"
        );
        drop(gate_release);
        completed.wait_for(Option::is_some).await.unwrap();
        assert_eq!(
            *completed.borrow(),
            Some(false),
            "revoked access publishes no bytes"
        );
        assert!(matches!(pending.await.unwrap(), Response::Failure(_)));
        assert_eq!(
            permits.available_permits(),
            crate::tools::session_files::SESSION_FILE_MAX_READS
        );
        assert_eq!(
            f.provider.request_bodies().len(),
            requests,
            "delivery reads and locations issue zero model requests"
        );
        f.manager
            .unload(&fork.active_conversation_id)
            .await
            .unwrap();
        f.close().await;
    })
    .await;
}

async fn delivery_response_boundary(
    connection: &AppServerConnection,
    target: &AttachmentTarget,
) -> crate::runtime_client::response::CompletedResponseView {
    let MethodResult::TranscriptWindow { window } = call(
        connection,
        45_001,
        Method::Transcript {
            target: target.clone(),
            at: crate::durable::reading::ConversationWindowAt::Latest,
            limit: 32,
        },
    )
    .await
    else {
        panic!("native completed response window");
    };
    window
        .page
        .entries
        .iter()
        .rev()
        .find_map(|entry| entry.completed_response.as_ref())
        .expect("native completed response boundary")
        .clone()
}

#[derive(serde::Deserialize)]
struct DeliveryToolMetadata {
    id: crate::runtime::identity::MessageId,
    result: DeliveryResultMetadata,
}
#[derive(serde::Deserialize)]
struct DeliveryResultMetadata {
    status: crate::tools::ToolExecutionStatus,
    #[serde(default)]
    artifacts: Vec<crate::message::content::FileReference>,
    #[serde(default)]
    deliveries: Vec<crate::tools::session_files::SessionFileReference>,
}

async fn read_delivery_metadata(
    connection: &AppServerConnection,
    target: &AttachmentTarget,
    reference: &crate::runtime_client::content::ContentRef,
) -> DeliveryToolMetadata {
    use base64::Engine as _;
    assert!(matches!(
        &reference.source,
        crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
            component: crate::runtime_client::content::CanonicalContentComponent::MessageMetadata,
            ..
        }
    ));
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let MethodResult::ContentChunk { chunk } = call(
            connection,
            45_000,
            Method::ConversationContentRead {
                target: target.clone(),
                agent_id: None,
                content_ref: reference.clone(),
                offset,
                max_bytes: 65_536,
            },
        )
        .await
        else {
            panic!("canonical delivery metadata chunk")
        };
        assert_eq!(&chunk.content_ref, reference);
        assert_eq!(chunk.offset, offset);
        assert_eq!(chunk.readable_end, reference.readable_end);
        assert!(chunk.data_base64.len() <= 87_384);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap();
        assert!(part.len() <= 65_536);
        assert_eq!(
            chunk.end_offset,
            offset + u64::try_from(part.len()).unwrap()
        );
        bytes.extend(part);
        assert!(
            bytes.len() <= 16 * 1024 * 1024,
            "selected fixture metadata cache bound"
        );
        let Some(next) = chunk.next_offset else {
            assert_eq!(chunk.end_offset, reference.readable_end);
            break;
        };
        assert_eq!(next, chunk.end_offset);
        assert!(next > offset);
        offset = next;
    }
    serde_json::from_slice(&bytes).unwrap()
}

fn delivery_metadata_reference(
    descriptor: &crate::runtime_client::snapshot::RuntimeClientMessageDescriptor,
) -> &crate::runtime_client::content::ContentRef {
    assert_eq!(descriptor.role, "tool");
    &descriptor
        .content
        .iter()
        .find(|binding| {
            matches!(
                &binding.content_ref.source,
                crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
                    component:
                        crate::runtime_client::content::CanonicalContentComponent::MessageMetadata,
                    ..
                }
            )
        })
        .expect("native canonical Tool metadata source")
        .content_ref
}

async fn checkpoint_delivery_metadata(
    connection: &AppServerConnection,
    target: &AttachmentTarget,
    snapshot: &crate::runtime_client::RuntimeClientSnapshot,
) -> DeliveryToolMetadata {
    let descriptors: Vec<_> = snapshot
        .message_descriptors
        .iter()
        .filter(|header| header.role == "tool")
        .collect();
    assert_eq!(
        descriptors.len(),
        1,
        "ordinary canonical Tool commit is unique"
    );
    let tool = read_delivery_metadata(
        connection,
        target,
        delivery_metadata_reference(descriptors[0]),
    )
    .await;
    assert_eq!(descriptors[0].identity.as_ref(), Some(&tool.id));
    tool
}

pub(super) const DELIVERED: &[u8] = b"# Original\r\n\r\nNative bytes\r\n";

/// Commits one `present` delivery of `f.sessions[0]` through an ordinary
/// connection, which then closes. Returns the delivering Tool message id.
pub(super) async fn committed_delivery(f: &Fixture) -> crate::runtime::identity::MessageId {
    std::fs::write(f.workspaces[0].join("报告 file.md"), DELIVERED).unwrap();
    let ordinary = AppServerConnection::new(f.host.clone());
    initialize(&ordinary).await;
    let target = attach(&ordinary, f, 0).await;
    let completion = AttemptCompletion::capture(f, &target);
    f.gates[0].release();
    call(
        &ordinary,
        4300,
        Method::TurnStart {
            target: target.clone(),
            content: wire_text("request-A"),
        },
    )
    .await;
    completion.observed(&ordinary, &target).await;
    let MethodResult::Checkpoint {
        checkpoint: checkpoint_17,
    } = call(
        &ordinary,
        4301,
        Method::SessionCheckpoint {
            target: target.clone(),
            trace_records: vec![],
        },
    )
    .await
    else {
        panic!("snapshot")
    };
    let snapshot = Box::new(checkpoint_17.snapshot);
    let tool = checkpoint_delivery_metadata(&ordinary, &target, &snapshot).await;
    assert!(!tool.result.deliveries.is_empty(), "committed delivery");
    ordinary.close();
    tool.id
}

pub(super) fn delivery_request(
    id: i64,
    target: &AttachmentTarget,
    message_id: &crate::runtime::identity::MessageId,
    locate: bool,
) -> Request {
    let (target, message_id) = (target.clone(), message_id.clone());
    Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(id),
        call: if locate {
            Method::DeliveryLocate {
                target,
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id,
                },
                delivery_index: 0,
            }
        } else {
            Method::DeliveryRead {
                target,
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id,
                },
                delivery_index: 0,
            }
        },
    }
}

pub(super) async fn cancel_delivery(connection: &AppServerConnection, request: i64) -> bool {
    let MethodResult::DeliveryCancel { accepted } = call(
        connection,
        9000 + request,
        Method::DeliveryCancel {
            request_id: RequestId::Integer(request),
        },
    )
    .await
    else {
        panic!("delivery cancel")
    };
    accepted
}

#[track_caller]
pub(super) fn delivered_bytes(response: &Response) -> Vec<u8> {
    use base64::Engine;
    let Response::Success(success) = response else {
        panic!("delivery success expected: {response:?}")
    };
    let MethodResult::SessionFileBytes { data, .. } = &success.result else {
        panic!("bytes expected")
    };
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap()
}

#[track_caller]
pub(super) fn failed_with(response: &Response, expected: &ErrorData) {
    let Response::Failure(Failure {
        error: RpcError {
            data: Some(data), ..
        },
        ..
    }) = response
    else {
        panic!("typed failure expected: {response:?}")
    };
    assert_eq!(data, expected);
}

/// Request-scoped cancellation, at every native interleaving.
///
/// Each case pins one boundary with a probe, never a sleep: before native
/// admission, after admission before open, after open before bytes, after
/// physical settlement before publication, and after publication. Terminal
/// response, publication, physical settlement and permit restoration are
/// checked separately.
pub(crate) async fn delivery_cancellation_scenario() {
    use crate::tools::session_files::{SESSION_FILE_MAX_READS, SessionFileReadFailure};
    bounded(async {
        let mut phases = FixturePhase::new("request-scoped delivery cancellation");
        phases.enter("fixture construction");
        let f = Fixture::with_tool(Some("present")).await;
        phases.enter("commit present delivery");
        let tool = committed_delivery(&f).await;
        phases.enter("initialize trusted connection");
        let authority = tokio_util::sync::CancellationToken::new();
        let trusted = AppServerConnection::with_delivery_access(f.host.clone(), authority);
        initialize(&trusted).await;
        let own = attach(&trusted, &f, 0).await;
        let read = |id| delivery_request(id, &own, &tool, false);
        let probe = f.host.file_read_probe();
        let permits = f.host.file_reads();
        let mut completed = probe.completed.subscribe();
        let cancelled = ErrorData::DeliveryCancelled;
        let spawn_read = |id| {
            let trusted = trusted.clone();
            let request = read(id);
            tokio::spawn(async move { trusted.handle_request(request).await })
        };

        phases.enter("cancellation before native admission");
        // 1. Accepted before native admission: nothing is admitted at all.
        probe.completed.send_replace(None);
        probe.before_admission.arm();
        let pending = spawn_read(10);
        probe.before_admission.wait_entered().await;
        assert!(cancel_delivery(&trusted, 10).await);
        assert!(cancel_delivery(&trusted, 10).await, "cancel is idempotent");
        probe.before_admission.release();
        failed_with(&pending.await.unwrap(), &cancelled);
        assert_eq!(*completed.borrow(), None, "never admitted");
        assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);

        // 2./3. Accepted after admission (before open; after open, before
        // bytes): the admitted read keeps its permit until it physically
        // settles, publishes nothing, and answers exactly once.
        for gate in [probe.before_open.clone(), probe.before_bytes.clone()] {
            phases.enter("physical read gate admission");
            probe.completed.send_replace(None);
            let release = gate.arm_scoped();
            let pending = spawn_read(11);
            let entered = gate.clone();
            tokio::task::spawn_blocking(move || entered.wait_entered())
                .await
                .unwrap();
            phases.enter("cancel held physical read");
            assert!(cancel_delivery(&trusted, 11).await);
            assert_eq!(
                permits.available_permits(),
                SESSION_FILE_MAX_READS - 1,
                "the parked read still owns its permit"
            );
            assert!(!pending.is_finished(), "no response before settlement");
            phases.enter("cancelled physical settlement");
            drop(release);
            completed.wait_for(Option::is_some).await.unwrap();
            assert_eq!(*completed.borrow(), Some(false), "no bytes produced");
            failed_with(&pending.await.unwrap(), &cancelled);
            assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
        }

        // 4. Accepted after physical settlement, before the publication
        // commit: the produced bytes and the native path are not published.
        phases.enter("cancellation before publication");
        for locate in [false, true] {
            probe.completed.send_replace(None);
            let reply = trusted
                .reply(delivery_request(12, &own, &tool, locate))
                .await;
            assert_eq!(*completed.borrow(), Some(true), "native work settled");
            assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
            assert!(cancel_delivery(&trusted, 12).await);
            failed_with(&reply.commit(), &cancelled);
        }

        // 5. After the publication commit the response stands; a late cancel
        // is refused rather than misrepresenting it as unpublished.
        phases.enter("late and unknown cancellation");
        let published = trusted.handle_request(read(13)).await;
        assert_eq!(delivered_bytes(&published), DELIVERED);
        assert!(!cancel_delivery(&trusted, 13).await);
        assert!(!cancel_delivery(&trusted, 404).await, "unknown id");

        // 6. Concurrent requests: cancelling one never touches the other,
        // nor can another connection (or an ordinary one) name it.
        phases.enter("independent request ownership");
        let other = AppServerConnection::with_delivery_access(
            f.host.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        initialize(&other).await;
        let ordinary = AppServerConnection::new(f.host.clone());
        initialize(&ordinary).await;
        probe.before_admission.arm();
        let parked = spawn_read(14);
        probe.before_admission.wait_entered().await;
        failed_with(
            &trusted.handle_request(read(14)).await,
            &ErrorData::DuplicateRequestId,
        );
        assert!(
            !cancel_delivery(&other, 14).await,
            "another connection's id"
        );
        assert_eq!(
            rejected(
                &ordinary,
                Method::DeliveryCancel {
                    request_id: RequestId::Integer(14)
                }
            )
            .await,
            ErrorData::SessionFileRead {
                reason: SessionFileReadFailure::Unauthorized
            }
        );
        let sibling = trusted.reply(read(15)).await;
        assert!(cancel_delivery(&trusted, 14).await);
        probe.before_admission.release();
        failed_with(&parked.await.unwrap(), &cancelled);
        assert_eq!(delivered_bytes(&sibling.commit()), DELIVERED);

        // 7. The connection stays healthy: a cancelled id is free again.
        assert_eq!(
            delivered_bytes(&trusted.handle_request(read(10)).await),
            DELIVERED
        );
        assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
        phases.enter("close connections and fixture");
        other.close();
        ordinary.close();
        trusted.close();
        f.close().await;
        phases.finish();
    })
    .await;
}

/// Native admission holds each request before lookup/execution. Unknown Agent
/// identities intentionally test the Observe lane before domain validation;
/// their later exact refusals neither create a Child nor claim an outcome.
fn delivery_capacity_parties(target: &AttachmentTarget) -> Vec<Request> {
    let mut parties = Vec::new();
    for id in 0..5 {
        parties.push(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call: Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        });
    }
    for id in 10..18 {
        parties.push(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call: Method::AgentObserve {
                target: target.clone(),
                agent_id: crate::runtime::identity::AgentId::new(format!("admission-only-{id}")),
                after: None,
            },
        });
    }
    parties.push(Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(90),
        call: Method::TurnCancel {
            target: target.clone(),
        },
    });
    parties
}

fn delivery_capacity_answer(response: Response, parties: &[Request]) -> i64 {
    let (id, result) = match response {
        Response::Success(success) => (success.id, Ok(success.result)),
        Response::Failure(failure) => (
            failure.id.expect("correlated refusal"),
            Err(failure.error.data.expect("typed refusal")),
        ),
    };
    let party = parties
        .iter()
        .find(|request| request.id == id)
        .expect("exact retained request");
    match (&party.call, result) {
        (Method::SessionCheckpoint { .. }, Ok(MethodResult::Checkpoint { .. }))
        | (Method::TurnCancel { .. }, Err(ErrorData::InvalidState)) => {}
        (
            Method::AgentObserve { agent_id, .. },
            Err(ErrorData::UnknownAgent { agent_id: actual }),
        ) => assert_eq!(&actual, agent_id),
        other => panic!("retained native request changed: {other:?}"),
    }
    let RequestId::Integer(id) = id else {
        panic!("integer fixture identity");
    };
    id
}

/// Five checkpoints and one physically parked delivery fill the six ordinary
/// slots. Eight Observe requests and one control request are independently
/// held at native operation admission, making fifteen retained requests. The
/// remaining control slot admits delivery/cancel as the sixteenth, without
/// releasing a read permit or cancelling any other native operation.
pub(crate) async fn delivery_cancellation_capacity_scenario() {
    use crate::app_server::transport::{IN_FLIGHT_REQUESTS, stdio};
    use crate::tools::session_files::SESSION_FILE_MAX_READS;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    bounded(async {
        let mut phases = FixturePhase::new("sixteenth in-flight delivery cancellation");
        phases.enter("fixture construction");
        let f = Fixture::with_tool(Some("present")).await;
        phases.enter("commit present delivery");
        let tool = committed_delivery(&f).await;
        phases.enter("initialize and attach transport");
        let connection = std::sync::Arc::new(AppServerConnection::with_delivery_access(
            f.host.clone(),
            tokio_util::sync::CancellationToken::new(),
        ));
        initialize(&connection).await;
        let own = attach(&connection, &f, 0).await;
        let (mut input, reader) = tokio::io::duplex(1 << 20);
        let (writer, output) = tokio::io::duplex(1 << 20);
        let serving = tokio::spawn(stdio::serve(
            connection.clone(),
            reader,
            writer,
            tokio_util::sync::CancellationToken::new(),
        ));
        let mut output = tokio::io::BufReader::new(output).lines();
        let mut next_response = async || loop {
            let line = output.next_line().await.unwrap().expect("open output");
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            if value.get("id").is_some() {
                break serde_json::from_value::<Response>(value).unwrap();
            }
        };
        let line = |request: &Request| format!("{}\n", serde_json::to_string(request).unwrap());
        let read_probe = f.host.file_read_probe();
        let permits = f.host.file_reads();

        phases.enter("physical read before-bytes gate");
        // A delivery read parked after its leaf open, before its bytes.
        let release_bytes = read_probe.before_bytes.arm_scoped();
        input
            .write_all(line(&delivery_request(100, &own, &tool, false)).as_bytes())
            .await
            .unwrap();
        let entered = read_probe.before_bytes.clone();
        tokio::task::spawn_blocking(move || entered.wait_entered())
            .await
            .unwrap();
        assert_eq!(
            permits.available_permits(),
            SESSION_FILE_MAX_READS - 1,
            "the read is admitted and holds its native permit"
        );
        phases.enter("park fourteen native operations");
        // Fourteen unrelated requests across their declared admission lanes.
        let probe = f.manager.probe(&own.conversation_id);
        probe.before_operation.arm();
        assert_eq!(IN_FLIGHT_REQUESTS, 16);
        let parties = delivery_capacity_parties(&own);
        assert_eq!(parties.len(), IN_FLIGHT_REQUESTS - 2);
        for request in &parties {
            input.write_all(line(request).as_bytes()).await.unwrap();
        }
        probe.before_operation.parked(parties.len()).await;

        phases.enter("sixteenth request cancellation response");
        // The sixteenth request in flight: the cancel.
        let cancel = Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(200),
            call: Method::DeliveryCancel {
                request_id: RequestId::Integer(100),
            },
        };
        input.write_all(line(&cancel).as_bytes()).await.unwrap();
        let Response::Success(answer) = next_response().await else {
            panic!("the cancel is answered")
        };
        assert_eq!(answer.id, RequestId::Integer(200));
        assert!(matches!(
            answer.result,
            MethodResult::DeliveryCancel { accepted: true }
        ));
        assert!(!serving.is_finished(), "sixteen in flight is within capacity");
        assert_eq!(
            permits.available_permits(),
            SESSION_FILE_MAX_READS - 1,
            "the parked read still owns its permit"
        );
        assert_eq!(
            probe.before_operation.parked_now(),
            parties.len(),
            "no unrelated request settled or was dropped"
        );

        phases.enter("cancelled read physical settlement");
        drop(release_bytes);
        let cancelled = next_response().await;
        assert!(matches!(&cancelled, Response::Failure(failure) if failure.id == Some(RequestId::Integer(100))));
        failed_with(&cancelled, &ErrorData::DeliveryCancelled);
        assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
        assert_eq!(probe.before_operation.parked_now(), parties.len());

        phases.enter("release and answer fourteen native operations");
        probe.before_operation.release();
        let mut answered = Vec::new();
        for _ in &parties {
            answered.push(delivery_capacity_answer(next_response().await, &parties));
        }
        answered.sort_unstable();
        let mut expected: Vec<_> = parties.iter().map(|request| match &request.id { RequestId::Integer(id) => *id, RequestId::String(_) => unreachable!("integer fixture identity") }).collect();
        expected.sort_unstable();
        assert_eq!(answered, expected, "each retained request answered exactly once");
        probe.before_operation.parked(0).await;

        phases.enter("post-cancellation checkpoint");
        // Back to baseline, and the same connection still serves requests.
        let after = Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(300),
            call: Method::SessionCheckpoint {
                target: own.clone(),
                trace_records: vec![],
            },
        };
        input.write_all(line(&after).as_bytes()).await.unwrap();
        let Response::Success(snapshot) = next_response().await else {
            panic!("the connection still serves requests")
        };
        assert_eq!(snapshot.id, RequestId::Integer(300));
        assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
        phases.enter("transport EOF and join");
        drop(input);
        serving.await.unwrap().unwrap();
        phases.enter("close fixture");
        f.close().await;
        phases.finish();
    })
    .await;
}

/// Revocation that wins before the publication commit suppresses a produced
/// delivery response with a typed failure for the same id; a response already
/// committed stands.
pub(crate) async fn delivery_publication_revocation_scenario() {
    use crate::tools::session_files::SessionFileReadFailure;
    bounded(async {
        let f = Fixture::with_tool(Some("present")).await;
        let tool = committed_delivery(&f).await;
        let unauthorized = ErrorData::SessionFileRead {
            reason: SessionFileReadFailure::Unauthorized,
        };
        for (locate, revocation) in [
            (false, "credential"),
            (true, "credential"),
            (false, "detach"),
            (true, "detach"),
            (false, "close"),
            (true, "close"),
        ] {
            let authority = tokio_util::sync::CancellationToken::new();
            let trusted =
                AppServerConnection::with_delivery_access(f.host.clone(), authority.clone());
            initialize(&trusted).await;
            let own = attach(&trusted, &f, 0).await;
            let committed = trusted
                .handle_request(delivery_request(20, &own, &tool, locate))
                .await;
            let reply = trusted
                .reply(delivery_request(21, &own, &tool, locate))
                .await;
            assert!(matches!(reply.produced(), Response::Success(_)));
            let expected = match revocation {
                "credential" => {
                    authority.cancel();
                    unauthorized.clone()
                }
                "detach" => {
                    call(&trusted, 22, Method::SessionDetach { target: own }).await;
                    ErrorData::StaleAttachment
                }
                _ => {
                    trusted.close();
                    unauthorized.clone()
                }
            };
            failed_with(&reply.commit(), &expected);
            assert!(
                matches!(committed, Response::Success(_)),
                "a response committed before revocation stands"
            );
            trusted.close();
        }
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compaction_correlation_is_bounded_before_admission() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let schema = crate::app_server::schema::protocol_schema();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let identity = &schema["$defs"]["ManualCompactionRequestId"];
        assert_eq!(identity["maxLength"], 64);
        assert_eq!(identity["minLength"], 1);
        assert_eq!(identity["pattern"], r"^[A-Za-z0-9_-]+$(?![\s\S])");
        let read = || {
            call(
                &connection,
                20,
                Method::SessionCheckpoint {
                    target: target.clone(),
                    trace_records: vec![],
                },
            )
        };
        for (value, valid) in [
            ("550e8400-e29b-41d4-a716-446655440000".to_owned(), true),
            ("a".repeat(64), true),
            ("a".repeat(65), false),
            ("a".repeat(900_000), false),
            (String::new(), false),
            ("has space".to_owned(), false),
            ("非ASCII".to_owned(), false),
            ("trailing\n".to_owned(), false),
        ] {
            f.manager
                .configuration_host(&target.session_id)
                .unwrap()
                .projection_settled()
                .await;
            let native = f.manager.configuration_host(&target.session_id).unwrap();
            if !valid {
                native.inner.park_projection_worker();
            }
            let before = read().await;
            let mut wire = serde_json::to_value(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(30),
                call: Method::CompactContext {
                    target: target.clone(),
                    request_id: "valid".to_owned().try_into().unwrap(),
                },
            })
            .unwrap();
            wire["params"]["request_id"] = serde_json::Value::String(value.clone());
            assert_eq!(validator.is_valid(&wire), valid);
            let response = connection.handle_json(&wire.to_string()).await.unwrap();
            let Response::Failure(failure) = response else {
                panic!("empty context cannot compact")
            };
            if valid {
                assert_ne!(failure.error.code, -32602);
                let MethodResult::Checkpoint {
                    checkpoint: checkpoint_18,
                } = read().await
                else {
                    panic!("snapshot")
                };
                let snapshot = Box::new(checkpoint_18.snapshot);
                let result = snapshot.context.manual_compaction.unwrap();
                assert_eq!(result.request_id.unwrap().as_str(), value);
                assert!(result.released);
                assert!(result.error.is_some());
            } else {
                assert_eq!(failure.error.code, -32602);
                // Equal cursor and full context prove no start or correlation mutation.
                assert_eq!(
                    serde_json::to_value(read().await).unwrap(),
                    serde_json::to_value(before).unwrap()
                );
            }
            if !valid {
                native.inner.unpark_projection_worker();
            }
            assert!(f.provider.request_bodies().is_empty());
        }
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn binary_upload_one_mib_document_repairs_original_receipts_without_model_requests() {
    bounded(async {
        let f = Fixture::new().await;
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
        let f = Fixture::new().await;
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
        let f = Fixture::new().await;
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
        let f = Fixture::new().await;
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
        let f = Fixture::new().await;
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
        let f = Fixture::new().await;
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
async fn native_user_input_receipts_allow_large_collections_and_preserve_authority() {
    bounded(async {
        use crate::local_runtime::session::uploads::UploadFile;
        for byte_boundary in [false, true] {
            let f = Fixture::new().await;
            let connection = AppServerConnection::new(f.host.clone());
            initialize(&connection).await;
            let target = attach(&connection, &f, 0).await;
            let controller = f.manager.session_controller();
            let native = f.load(0).await.unwrap().unwrap().inspect_runtime().unwrap();
            let mut empty = Vec::new();
            for _ in 0..24 {
                empty.push(
                    controller
                        .upload(
                            &target.session_id,
                            None,
                            vec![UploadFile {
                                name: "empty".into(),
                                data: vec![].into(),
                            }],
                        )
                        .await
                        .unwrap()[0]
                        .receipt
                        .clone(),
                );
            }
            let mut large = Vec::new();
            for size in [3 * 1024 * 1024, 3 * 1024 * 1024, 1] {
                large.push(
                    controller
                        .upload(
                            &target.session_id,
                            None,
                            vec![UploadFile {
                                name: "document".into(),
                                data: vec![b'x'; size].into(),
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
                        data: vec![].into(),
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
            for receipts in [vec![foreign], vec![unready]] {
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
                &large[..]
            } else {
                &empty[..]
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
        let f = Fixture::new().await;
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
        let f = Fixture::new().await;
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
        let f = Fixture::new().await;
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
async fn durable_history_is_readable_before_and_during_runtime_preparation() {
    bounded(async {
        let f = Fixture::new().await;
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
async fn explicit_node_switch_commits_aba_ownership_and_coalesces_exact_retirements() {
    bounded(async {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::local_runtime::session::LineageSide;
        use crate::message::TextBlock;
        use crate::message::types::{
            InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
        };
        use crate::runtime::identity::MessageId;
        let f = Fixture::new().await;
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
    use crate::durable::{ConversationStore, SqliteConversationStore};
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
            let f = Fixture::new().await;
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

mod child_delivery {
    use super::*;
    include!("child_delivery.rs");
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

#[path = "live_model.rs"]
mod live_model;
pub(crate) use live_model::live_model_child_inheritance_boundary_scenario;
#[tokio::test]
async fn t01_invalid_identity_precedes_method_and_connection_admission() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        for id in [
            serde_json::json!("x".repeat(255)),
            serde_json::json!(9_007_199_254_740_992_u64),
            serde_json::json!(null),
            serde_json::json!([]),
        ] {
            let record =
                serde_json::json!({"jsonrpc":"2.0", "id":id, "method":"not/a/method", "params":{}})
                    .to_string();
            let Response::Failure(failure) = connection.handle_json(&record).await.unwrap() else {
                panic!("invalid envelope")
            };
            assert_eq!(failure.id, None);
            assert_eq!(failure.error.code, -32600);
            assert_eq!(connection.attachment_counts(), (0, 0));
        }
        let Response::Failure(failure) = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::String("\n".repeat(128)),
                call: Method::Initialize(InitializeParams {
                    protocol_version: APP_SERVER_PROTOCOL_VERSION,
                    client: ClientIdentity {
                        name: "rejected".into(),
                        version: "1".into(),
                    },
                    presentation: PresentationCapabilities::default(),
                }),
            })
            .await
        else {
            panic!("typed invalid identity")
        };
        assert_eq!((failure.id, failure.error.code), (None, -32600));
        // Rejected typed Initialize did not alter connection state.
        initialize(&connection).await;
        for call in [
            Method::DeliveryCancel {
                request_id: RequestId::String("x".repeat(255)),
            },
            Method::AgentObserveCancel {
                request_id: RequestId::Integer(i64::MAX),
            },
        ] {
            let Response::Failure(failure) = connection
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(19),
                    call,
                })
                .await
            else {
                panic!("invalid cancellation parameter")
            };
            assert_eq!(
                (failure.id, failure.error.code),
                (Some(RequestId::Integer(19)), -32602)
            );
        }
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn t20_request_identity_remains_owned_through_response_disposition() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let request = || Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(31),
            call: Method::ServerInfo {},
        };
        let reply = connection.reply(request()).await;
        let Response::Failure(failure) = connection.handle_request(request()).await else {
            panic!("overlapping identity must be refused")
        };
        assert_eq!(failure.error.data, Some(ErrorData::DuplicateRequestId));
        drop(reply); // Unaccepted response abandoned; exact association is released.
        assert!(matches!(
            connection.handle_request(request()).await,
            Response::Success(_)
        ));
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn t20_observation_cancel_after_admission_before_first_poll_never_dispatches() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::Attached { target, .. } = call(
            &connection,
            1,
            Method::SessionAttach {
                session_id: f.sessions[0].id.clone(),
                node_id: None,
            },
        )
        .await
        else {
            panic!("attached")
        };
        let wait = connection.admitted_request_for_test(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(41),
            call: Method::AgentObserve {
                target,
                agent_id: crate::runtime::identity::AgentId::new("never-inspected"),
                after: None,
            },
        });
        // No future has been polled. Cancellation must already find the owner.
        let cancelled = call(
            &connection,
            42,
            Method::AgentObserveCancel {
                request_id: RequestId::Integer(41),
            },
        )
        .await;
        assert_eq!(
            cancelled,
            MethodResult::AgentObserveCancelled { accepted: true }
        );
        let Response::Failure(failure) = wait.await.commit() else {
            panic!("cancelled observation")
        };
        assert_eq!(failure.error.data, Some(ErrorData::ObservationCancelled));
        // Dispatching this nonexistent Child would have produced UnknownAgent.
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t20_aborted_response_waiter_retains_native_operation_reservation_until_termination() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::Attached { target, .. } = call(
            &connection,
            1,
            Method::SessionAttach {
                session_id: f.sessions[0].id.clone(),
                node_id: None,
            },
        )
        .await
        else {
            panic!("attached")
        };
        let runtime = f.manager.load(&target.session_id, None).await.unwrap();
        let resident = runtime.client().current().unwrap();
        let mut operations = resident.operations.subscribe();
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let request = Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(51),
            call: Method::ModelGet { target },
        };
        let waiting = tokio::spawn(connection.admitted_request_for_test(request.clone()));
        probe.before_operation.entered().await;
        waiting.abort();
        assert!(matches!(waiting.await, Err(error) if error.is_cancelled()));
        let Response::Failure(failure) = connection.handle_request(request.clone()).await else {
            panic!("physical work still owns identity")
        };
        assert_eq!(failure.error.data, Some(ErrorData::DuplicateRequestId));
        // Native OperationLease release is the physical termination proof.
        probe.before_operation.release();
        operations.wait_for(|count| *count == 0).await.unwrap();
        assert!(matches!(
            connection.handle_request(request).await,
            Response::Success(_)
        ));
        connection.close();
        f.close().await;
    })
    .await;
}

/// T15: the source configuration owner has bounded projections, advancing
/// aggregate pages, and independent exact large-field reads at one source cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t15_source_fields_and_many_small_revisions_page_with_exact_cut_evidence() {
    bounded(async {
        use crate::local_runtime::configuration::{observation as source, settings::SourceTarget};
        use base64::Engine;
        let f = Fixture::new().await;
        let mut native = f
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        native.prospective_diagnostic = Some("é\n".repeat(140_000));
        for index in 0..3000 {
            native.resource_revisions.insert(
                std::path::PathBuf::from(format!("resource-{index:05}")),
                "revision".repeat(8),
            );
        }
        native.resource_revisions.insert(
            std::path::PathBuf::from("resource-zzzz"),
            "x".repeat(40_000),
        );
        let projection = source::bounded(&native).unwrap();
        assert!(projection.inline.is_none());
        let mut reconstructed = serde_json::Map::new();
        for field in &projection.fields {
            let value = if let Some(value) = &field.value {
                value.clone()
            } else {
                let reference = field.reference.as_ref().unwrap();
                let mut bytes = Vec::new();
                let mut offset = 0;
                loop {
                    let chunk =
                        source::read(&native, reference, offset, source::READ_BYTES).unwrap();
                    bytes.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(&chunk.data_base64)
                            .unwrap(),
                    );
                    let Some(next) = chunk.next_offset else {
                        break;
                    };
                    assert!(next > offset);
                    offset = next;
                }
                serde_json::from_slice(&bytes).unwrap()
            };
            let serde_json::Value::String(name) = serde_json::to_value(field.name).unwrap() else {
                panic!("field name");
            };
            assert!(reconstructed.insert(name, value).is_none());
        }
        let reconstructed: crate::local_runtime::configuration::settings::SourceSettings =
            crate::runtime_client::structural_json::decode(serde_json::Value::Object(
                reconstructed,
            ))
            .unwrap();
        assert_eq!(reconstructed, native);
        let response = Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(315),
            result: MethodResult::SourceSettings {
                projection: Box::new(projection.clone()),
            },
        }));
        assert!(serde_json::to_vec(&response).unwrap().len() < 786_432);
        let reference = projection
            .fields
            .iter()
            .find(|field| field.name == source::SourceFieldName::ProspectiveDiagnostic)
            .unwrap()
            .reference
            .as_ref()
            .unwrap();
        let mut offset = 0;
        let mut bytes = vec![];
        loop {
            let chunk = source::read(&native, reference, offset, source::READ_BYTES).unwrap();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap();
            assert!(decoded.len() <= 65_536);
            assert!(chunk.end_offset > offset);
            bytes.extend(decoded);
            let Some(next) = chunk.next_offset else {
                break;
            };
            offset = next;
        }
        assert_eq!(
            serde_json::from_slice::<String>(&bytes).unwrap(),
            native.prospective_diagnostic.as_deref().unwrap()
        );
        assert!(source::read(&native, reference, reference.byte_length + 1, 1).is_err());
        let mut offset = 0;
        let mut seen = 0;
        loop {
            let page = source::page(
                &native,
                &projection.revision,
                source::SourceFieldName::ResourceRevisions,
                offset,
                32,
            )
            .unwrap();
            assert!(page.items.len() <= 32);
            assert!(serde_json::to_vec(&page).unwrap().len() < 786_432);
            for item in &page.items {
                assert_eq!(item.index, seen);
                seen += 1;
                if let Some(reference) = &item.reference {
                    let chunk = source::read(&native, reference, 0, 65_536).unwrap();
                    let entry: serde_json::Value = serde_json::from_slice(
                        &base64::engine::general_purpose::STANDARD
                            .decode(chunk.data_base64)
                            .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(entry[0], "resource-zzzz");
                    assert_eq!(entry[1], "x".repeat(40_000));
                }
            }
            let Some(next) = page.next_offset else {
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(
            seen,
            u64::try_from(native.resource_revisions.len()).unwrap()
        );
        native.prospective_diagnostic.as_mut().unwrap().push('x');
        assert_eq!(
            source::read(&native, reference, 0, 1).unwrap_err(),
            "source read cut expired"
        );
        native.prospective_diagnostic = None;
        native.resource_revisions.clear();
        for index in 0..33 {
            native
                .resource_revisions
                .insert(std::path::PathBuf::from(format!("r{index}")), "v".into());
        }
        assert!(
            serde_json::to_vec(&native.resource_revisions)
                .unwrap()
                .len()
                < 16_384
        );
        let small_many = source::bounded(&native).unwrap();
        assert!(
            small_many.inline.is_none(),
            "count budget applies below whole-view byte budget"
        );
        let field = small_many
            .fields
            .iter()
            .find(|field| field.name == source::SourceFieldName::ResourceRevisions)
            .unwrap();
        assert!(
            field.value.is_none(),
            "count budget applies below per-field byte budget"
        );
        assert!(field.reference.as_ref().unwrap().byte_length < 16_384);
        let page = source::page(
            &native,
            &small_many.revision,
            source::SourceFieldName::ResourceRevisions,
            0,
            32,
        )
        .unwrap();
        assert_eq!(page.items.len(), 32);
        assert_eq!(page.next_offset, Some(32));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t15_cold_settings_deferred_value_advances_with_exact_catalog_revision() {
    use crate::local_runtime::configuration::observation as source;
    use base64::Engine;
    bounded(async {
        let f = Fixture::new().await;
        let (revision, mut settings) = f
            .manager
            .sessions
            .read_settings(&f.sessions[0].id)
            .await
            .unwrap();
        settings.model = Some(crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/a").unwrap(),
        ));
        settings.model.as_mut().unwrap().request_params.insert(
            "large".into(),
            serde_json::Value::String("é\n".repeat(700_000)),
        );
        let projection = source::session_settings(revision, &settings).unwrap();
        assert!(projection.inline.is_none());
        let reference = projection.reference.as_ref().unwrap();
        assert!(serde_json::to_vec(&projection).unwrap().len() < 786_432);
        let mut bytes = Vec::new();
        let mut offset = 0;
        loop {
            let chunk =
                source::session_settings_read(revision, &settings, reference, offset, 65_536)
                    .unwrap();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(chunk.data_base64)
                .unwrap();
            assert!(decoded.len() <= 65_536);
            assert!(chunk.end_offset > offset);
            bytes.extend(decoded);
            let Some(next) = chunk.next_offset else {
                break;
            };
            offset = next;
        }
        assert_eq!(
            crate::runtime_client::structural_json::decode::<
                crate::local_runtime::session::SessionPersistentState,
            >(serde_json::from_slice(&bytes).unwrap())
            .unwrap(),
            settings
        );
        assert_eq!(
            source::session_settings_read(revision + 1, &settings, reference, 0, 1).unwrap_err(),
            "source read cut expired"
        );
        let mut modified = reference.clone();
        modified.byte_length += 1;
        assert!(source::session_settings_read(revision, &settings, &modified, 0, 1).is_err());
        assert!(
            source::session_settings_read(
                revision,
                &settings,
                reference,
                reference.byte_length + 1,
                1
            )
            .is_err()
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t15_effective_configuration_preserves_all_fields_and_advancing_pages() {
    use crate::local_runtime::configuration::effective_observation as source;
    use base64::Engine;
    bounded(async {
        let mut phases = FixturePhase::new("effective-configuration-pages");
        phases.enter("fixture");
        let f = Fixture::new().await;
        phases.enter("load");
        f.manager.load(&f.sessions[0].id, None).await.unwrap();
        phases.enter("native effective configuration");
        let mut native = f
            .manager
            .effective_configuration_observation(&f.sessions[0].id)
            .unwrap();
        native.effective_model.configured.request_params.insert(
            "large".into(),
            serde_json::Value::String("é\n".repeat(300_000)),
        );
        for index in 0..3000 {
            native.source_revisions.insert(
                std::path::PathBuf::from(format!("source-{index:05}")),
                "revision".repeat(8),
            );
        }
        phases.enter("bounded projection");
        let projection = source::bounded(&native).unwrap();
        assert!(projection.inline.is_none());
        assert_eq!(projection.fields.len(), 18);
        assert!(serde_json::to_vec(&projection).unwrap().len() < 786_432);
        phases.enter("field reconstruction");
        let mut reconstructed = serde_json::Map::new();
        for field in &projection.fields {
            let value = if let Some(value) = &field.value {
                value.clone()
            } else {
                let reference = field.reference.as_ref().unwrap();
                let mut bytes = Vec::new();
                let mut offset = 0;
                loop {
                    let chunk = source::read(&native, reference, offset, 65_536).unwrap();
                    bytes.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(chunk.data_base64)
                            .unwrap(),
                    );
                    let Some(next) = chunk.next_offset else {
                        break;
                    };
                    assert!(next > offset);
                    offset = next;
                }
                serde_json::from_slice(&bytes).unwrap()
            };
            let serde_json::Value::String(name) = serde_json::to_value(field.name).unwrap() else {
                panic!("field");
            };
            assert!(reconstructed.insert(name, value).is_none());
        }
        let reconstructed: crate::local_runtime::configuration::settings::EffectiveConfiguration =
            crate::runtime_client::structural_json::decode(serde_json::Value::Object(
                reconstructed,
            ))
            .unwrap();
        assert_eq!(reconstructed, native);
        phases.enter("source revisions pages");
        let mut offset = 0;
        let mut seen = 0;
        loop {
            let page = source::page(
                &native,
                &projection.revision,
                source::EffectiveFieldName::SourceRevisions,
                offset,
                32,
            )
            .unwrap();
            assert!(page.items.len() <= 32);
            assert!(serde_json::to_vec(&page).unwrap().len() < 786_432);
            seen += page.items.len();
            let Some(next) = page.next_offset else {
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(seen, native.source_revisions.len());
        phases.enter("expired and small collection evidence");
        let reference = projection
            .fields
            .iter()
            .find(|field| field.name == source::EffectiveFieldName::EffectiveModel)
            .unwrap()
            .reference
            .as_ref()
            .unwrap();
        native.adopted_binding += 1;
        assert_eq!(
            source::read(&native, reference, 0, 1).unwrap_err(),
            "source read cut expired"
        );
        native.effective_model.configured.request_params.clear();
        native.source_revisions.clear();
        for index in 0..33 {
            native
                .source_revisions
                .insert(std::path::PathBuf::from(format!("r{index}")), "v".into());
        }
        assert!(serde_json::to_vec(&native.source_revisions).unwrap().len() < 16_384);
        let small_many = source::bounded(&native).unwrap();
        assert!(small_many.inline.is_none());
        let field = small_many
            .fields
            .iter()
            .find(|field| field.name == source::EffectiveFieldName::SourceRevisions)
            .unwrap();
        assert!(field.value.is_none());
        assert!(field.reference.as_ref().unwrap().byte_length < 16_384);
        let page = source::page(
            &native,
            &small_many.revision,
            source::EffectiveFieldName::SourceRevisions,
            0,
            32,
        )
        .unwrap();
        assert_eq!(page.items.len(), 32);
        assert_eq!(page.next_offset, Some(32));
        phases.enter("close");
        f.close().await;
        phases.finish();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t13_t15_editor_and_origin_history_use_finite_native_ledger_identity() {
    bounded(async {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::local_runtime::session::LineageSide;
        use crate::message::types::{InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
        use crate::runtime::identity::MessageId;
        let fixture = Fixture::new().await;
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
        assert!(window.page.content.iter().chain(window.page.deferred_entries.iter().flat_map(|entry| entry.content.iter())).any(|entry| matches!(&entry.content_ref.source, crate::runtime_client::content::ContentSource::CanonicalMessageBlock { locator: found, .. } if found == &locator)));
        let mut foreign = editor;
        foreign.source_node = session.active_node;
        rejected(&connection, Method::SessionEditorRead { locator:foreign, block_index:0, offset:0, max_bytes:1 }).await;
        fixture.close().await;
    }).await;
}

async fn retained_huge_root(fixture: &Fixture) -> Vec<(crate::runtime::identity::MessageId, u64)> {
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::MessageId;
    let session = &fixture.sessions[0];
    let access = fixture
        .manager
        .session_controller()
        .acquire_session(&session.id, None)
        .await
        .unwrap();
    let store = SqliteConversationStore::open(
        session.active_conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    // The model admission limit is per immutable record. Keep the retained
    // aggregate above 2 MiB while each original is a legal native input.
    let mut originals = Vec::new();
    for index in 0..4 {
        let text = "水\n".repeat(150_000);
        let readable_end = u64::try_from(text.len()).unwrap();
        let message_id = MessageId::new(format!("retained-huge-root-{index}"));
        let original = MessageBlock::User(UserMessageBlock {
            id: message_id.clone(),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
            content: vec![UserContentBlock::Text(crate::message::TextBlock { text })],
        });
        assert!(
            serde_json::to_vec(&original).unwrap().len()
                < crate::context::resources::ResourceLimits::default().record_bytes
        );
        store.append_canonical(&original).unwrap();
        originals.push((message_id, readable_end));
    }
    assert!(originals.iter().map(|(_, bytes)| bytes).sum::<u64>() > 2 * 1_048_576);
    drop(store);
    drop(access);
    originals
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t13_lost_huge_root_attach_receipt_is_reconciled_without_second_admission() {
    bounded(async {
        use std::task::Poll;
        let fixture = Fixture::new().await;
        let originals = retained_huge_root(&fixture).await;
        let session = &fixture.sessions[0];
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let reply = connection
            .reply(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(46_710),
                call: Method::SessionAttach {
                    session_id: session.id.clone(),
                    node_id: None,
                },
            })
            .await;
        let mut lost = crate::app_server::transport::Outbound::reply(reply).unwrap();
        let mut receipt_bytes = 0;
        // Attach is committed, but the physical attempt accepts no bytes.
        assert!(matches!(
            lost.poll_hand_off::<()>(|record| {
                receipt_bytes = record.len();
                Poll::Pending
            }),
            Poll::Pending
        ));
        assert!(receipt_bytes > 0 && receipt_bytes < 4096);
        drop(lost);
        let MethodResult::AttachmentStatus {
            target: Some(target),
            ..
        } = call(
            &connection,
            46_711,
            Method::SessionAttachmentStatus {
                session_id: session.id.clone(),
            },
        )
        .await
        else {
            panic!("committed attachment remains resolvable");
        };
        assert_eq!(target.conversation_id, session.active_conversation_id);
        assert_eq!(
            rejected(
                &connection,
                Method::SessionAttach {
                    session_id: session.id.clone(),
                    node_id: None,
                }
            )
            .await,
            ErrorData::ControllerInUse
        );
        let MethodResult::Checkpoint { checkpoint } = call(
            &connection,
            46_712,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("independent bounded checkpoint");
        };
        assert!(serde_json::to_vec(&checkpoint).unwrap().len() <= 786_432);
        for (message_id, readable_end) in &originals {
            let descriptor = checkpoint
                .snapshot
                .message_descriptors
                .iter()
                .find(|descriptor| descriptor.identity.as_ref() == Some(message_id))
                .unwrap();
            assert!(descriptor.content.iter().any(
                |binding| binding.deferred && binding.content_ref.readable_end == *readable_end
            ));
        }
        assert!(
            fixture.provider.request_bodies().is_empty(),
            "reconciliation starts no model work"
        );
        call(&connection, 46_713, Method::SessionDetach { target }).await;
        assert!(matches!(
            call(
                &connection,
                46_714,
                Method::SessionAttachmentStatus {
                    session_id: session.id.clone(),
                }
            )
            .await,
            MethodResult::AttachmentStatus { target: None, .. }
        ));
        connection.close();
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projection_history_and_activation_fork_cross_native_app_server_boundary() {
    bounded(async {
        use crate::context::checkpoint::ProjectionActivationId;
        use crate::context::resources::LedgerFrontier;
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
        use crate::runtime::identity::MessageId;
        let f = Fixture::new().await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t07_t13_source_failure_fences_registration_without_replay_loss_or_cursor_consumption() {
    bounded(async {
        use futures_util::FutureExt;
        use crate::durable::ConversationStore as _;
        struct SourceFault { connection: rusqlite::Connection, original: Vec<u8> }
        impl SourceFault {
            fn restore(&self) {
                assert_eq!(self.connection.execute("UPDATE bootstrap_identity SET occupancy_observation=?1 WHERE id=1",[&self.original]).unwrap(),1);
            }
        }
        impl Drop for SourceFault { fn drop(&mut self) { self.restore(); } }
        let fixture = Fixture::new().await;
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &fixture, 0).await;
        let native = fixture
            .manager
            .configuration_host(&target.session_id)
            .unwrap();
        native.projection_settled().await;
        let inner = native.weak_inner().upgrade().unwrap();
        inner.park_projection_worker();
        let MethodResult::Checkpoint { checkpoint } = call(
            &connection,
            47_800,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("native checkpoint")
        };
        let MethodResult::Subscribed {
            registration_id, ..
        } = call(
            &connection,
            47_801,
            Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: checkpoint.cursor,
            },
        )
        .await
        else {
            panic!("accepted registration")
        };
        let slot = connection.registration_for_test(&target);
        assert_eq!(slot.accepted_cursor(), Some(checkpoint.cursor));
        // Retain a real derived Source fault while explicit checkpoint repair
        // uses the original worker. Canonical/J/operation authority is untouched.
        let access=fixture.manager.session_controller().acquire_read(&target.session_id,Some(&fixture.sessions[0].active_node)).await.unwrap();
        let source=rusqlite::Connection::open(&access.database_path).unwrap();
        let store=crate::durable::SqliteConversationStore::open_existing(target.conversation_id.clone(),&access.database_path).unwrap();
        drop(access);
        let original_cut=store.observation_read_cut().unwrap();
        let original_journal=store.read_events(None,32).unwrap().events;
        let original=source.query_row("SELECT occupancy_observation FROM bootstrap_identity WHERE id=1",[],|row|row.get::<_,Vec<u8>>(0)).unwrap();
        assert_eq!(source.execute("UPDATE bootstrap_identity SET occupancy_observation=NULL WHERE id=1",[]).unwrap(),1);
        let fault=SourceFault {connection:source,original};
        native.fail_projection_read_for_test("same-fence source failure");
        loop {
            match connection.next_notification().await.notification {
                NotificationMethod::ObservationUnavailable {
                    target: found,
                    registration_id: found_registration,
                    message,
                } => {
                    assert_eq!(found, target);
                    assert_eq!(found_registration, registration_id);
                    assert!(message.contains("same-fence source failure"));
                    break;
                }
                NotificationMethod::ResyncRequired { .. }
                | NotificationMethod::Event { .. }
                | NotificationMethod::Closed { .. } => {
                    panic!("source failure is neither replay loss nor an event/close")
                }
                _ => {}
            }
        }
        assert_eq!(slot.accepted_cursor(), Some(checkpoint.cursor));
        inner.unpark_projection_worker();
        assert!(matches!(
            rejected(
                &connection,
                Method::SessionCheckpoint {
                    target: target.clone(),
                    trace_records: vec![]
                }
            )
            .await,
            ErrorData::ObservationUnavailable {diagnostic} if diagnostic.contains("inherited occupancy reading source unavailable")
        ));
        assert!(matches!(
            rejected(
                &connection,
                Method::SessionSubscribe {
                    target: target.clone(),
                    after_cursor: checkpoint.cursor
                }
            )
            .await,
            ErrorData::ObservationUnavailable {diagnostic} if diagnostic.contains("inherited occupancy reading source unavailable")
        ));
        assert!(
            connection
                .next_notification_candidate()
                .now_or_never()
                .is_none(),
            "unavailable registration is fenced, not a notification loop"
        );
        assert_eq!(slot.accepted_cursor(), Some(checkpoint.cursor));
        fault.restore();
        let MethodResult::Checkpoint {checkpoint:repaired}=call(&connection,47_803,Method::SessionCheckpoint {target:target.clone(),trace_records:vec![]}).await else {panic!("explicit public checkpoint repairs the original Source");};
        assert_eq!(repaired.cursor.get(),checkpoint.cursor.get()+1);
        assert_eq!(repaired.read_cut.source_cut,checkpoint.read_cut.source_cut);
        assert_eq!(store.observation_read_cut().unwrap(),original_cut);
        assert_eq!(store.read_events(None,32).unwrap().events,original_journal);
        let MethodResult::Subscribed {
            registration_id: successor,
            ..
        } = call(
            &connection,
            47_802,
            Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: checkpoint.cursor,
            },
        )
        .await
        else {
            panic!("explicit recovery")
        };
        assert_ne!(successor, registration_id);
        loop {
            match connection.next_notification().await.notification {
                NotificationMethod::Event {
                    registration_id: found,
                    cursor,
                    event,
                    ..
                } => {
                    assert_eq!(found, successor);
                    assert_eq!(cursor.get(), checkpoint.cursor.get() + 1);
                    assert!(
                        event.facts().iter().any(|fact| matches!(
                            fact,
                            RuntimeClientEvent::ReadDomainsUpdated { .. }
                        ))
                    );
                    break;
                }
                NotificationMethod::ResyncRequired { .. }
                | NotificationMethod::ObservationUnavailable { .. }
                | NotificationMethod::Closed { .. } => {
                    panic!("same-fence successful repair remains continuously replayable")
                }
                _ => {}
            }
        }
        assert_eq!(
            fixture.provider.request_bodies().len(),
            0,
            "observation recovery starts no execution"
        );
        drop(fault);
        drop(store);
        drop(inner);
        drop(native);
        fixture.close().await;
    })
    .await;
}
