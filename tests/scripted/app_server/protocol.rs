//! Direct connection contracts. Provider gates prove overlap; no socket timing.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::app_server_conformance as conformance;
use super::{Fixture, FixturePhase, bounded, input};
use crate::app_server::connection::AppServerConnection;
use crate::app_server::protocol::*;
use crate::observation::content::CanonicalContentLocator;
use crate::runtime_client::event::RuntimeClientEvent;
mod admission;
mod attachment;
mod auxiliary;
mod catalog;
mod configuration;
mod control;
pub(crate) mod delivery;
mod history;
mod model_catalog;
mod projection;
mod schedule;
mod upload;
mod workflows;

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
            crate::observation::structural_json::decode(serde_json::Value::Object(object)).unwrap(),
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
            crate::observation::structural_json::decode(serde_json::Value::Object(object)).unwrap(),
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
        crate::observation::structural_json::decode(serde_json::from_slice(&bytes).unwrap())
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
    crate::observation::structural_json::decode(serde_json::from_slice(&bytes).unwrap()).unwrap()
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
            let model: CatalogModelObservation = crate::observation::structural_json::decode(
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

#[path = "child_delivery.rs"]
mod child_delivery;

#[path = "live_model.rs"]
mod live_model;

pub(crate) use live_model::live_model_child_inheritance_boundary_scenario;
