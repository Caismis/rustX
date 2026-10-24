//! Artifact and delivered-file reads: native-scoped artifact carriers and the
//! transport-authorized delivery access, cancellation, capacity and
//! revocation scenarios the boundary suite drives over real sockets.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

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

// Real transport-authenticated Product Host, never a second ordinary connection.
pub(super) const FILE_HOST_TOKEN: &str = "host-secret-000000000000000000000000000000000000000";

const FILE_BROWSER_TOKEN: &str = "browser-secret-0000000000000000000000000000000000000";

type FileSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub(super) struct TrustedFileHost {
    endpoint: String,
    stop: tokio_util::sync::CancellationToken,
    serving: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl TrustedFileHost {
    pub(super) async fn new(f: &Fixture) -> Self {
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
    pub(super) async fn open(
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
    pub(super) async fn response(
        &self,
        read: crate::app_server::product_host::FileRead,
    ) -> Response {
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
    pub(super) async fn success(
        &self,
        read: crate::app_server::product_host::FileRead,
    ) -> MethodResult {
        match self.response(read).await {
            Response::Success(reply) => reply.result,
            Response::Failure(reply) => panic!("trusted file read failed: {:?}", reply.error),
        }
    }
    pub(super) async fn rejected(
        &self,
        read: crate::app_server::product_host::FileRead,
    ) -> ErrorData {
        match self.response(read).await {
            Response::Failure(reply) => reply.error.data.unwrap(),
            Response::Success(_) => panic!("file read unexpectedly succeeded"),
        }
    }
    pub(super) async fn close(self) {
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
            source: crate::app_server::product_host::ReadSource::SessionFile { agent_id: None, locator: crate::observation::content::CanonicalContentLocator::ByMessageId { message_id }, delivery_index: 0 },
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
            crate::observation::content::ContentSource::CanonicalMessageBlock {
                component: crate::observation::content::CanonicalContentComponent::MessageMetadata,
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
            target: fork_target, source: crate::app_server::product_host::ReadSource::SessionFile { agent_id: None, locator: crate::observation::content::CanonicalContentLocator::ByMessageId { message_id: inherited.id.clone() }, delivery_index: 0 }, roots: vec![f.workspaces[0].clone()],
        }).await else { panic!() };
        assert_eq!(inherited_file, file);
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(data).unwrap(), original);
        // A same-named unrelated Session cannot resolve invented source coordinates.
        std::fs::write(f.workspaces[1].join(&file.path), b"UNRELATED").unwrap();
        let unrelated = attach_session(&browser, &f.sessions[1]).await;
        assert_eq!(product_host.rejected( crate::app_server::product_host::FileRead {
            target: unrelated, source: crate::app_server::product_host::ReadSource::SessionFile { agent_id: None, locator: crate::observation::content::CanonicalContentLocator::ByMessageId { message_id: tool.id.clone() }, delivery_index: 0 }, roots: vec![f.workspaces[1].clone()],
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
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
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
                locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                    message_id: tool.id.clone(),
                },
                delivery_index: 0,
            },
            Method::DeliveryLocate {
                target: target.clone(),
                locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
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
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
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
                locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
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
                locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
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
            usize::try_from(crate::observation::content::CONTENT_SAFE_INTEGER_MAX + 1)
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
                locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
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
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
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
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
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
) -> crate::observation::turn::CompletedResponseView {
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
    reference: &crate::observation::content::ContentRef,
) -> DeliveryToolMetadata {
    use base64::Engine as _;
    assert!(matches!(
        &reference.source,
        crate::observation::content::ContentSource::CanonicalMessageBlock {
            component: crate::observation::content::CanonicalContentComponent::MessageMetadata,
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
    descriptor: &crate::observation::transcript::RuntimeClientMessageDescriptor,
) -> &crate::observation::content::ContentRef {
    assert_eq!(descriptor.role, "tool");
    &descriptor
        .content
        .iter()
        .find(|binding| {
            matches!(
                &binding.content_ref.source,
                crate::observation::content::ContentSource::CanonicalMessageBlock {
                    component:
                        crate::observation::content::CanonicalContentComponent::MessageMetadata,
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

pub(in super::super) const DELIVERED: &[u8] = b"# Original\r\n\r\nNative bytes\r\n";

/// Commits one `present` delivery of `f.sessions[0]` through an ordinary
/// connection, which then closes. Returns the delivering Tool message id.
pub(in super::super) async fn committed_delivery(
    f: &Fixture,
) -> crate::runtime::identity::MessageId {
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

pub(in super::super) fn delivery_request(
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
                locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                    message_id,
                },
                delivery_index: 0,
            }
        } else {
            Method::DeliveryRead {
                target,
                locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                    message_id,
                },
                delivery_index: 0,
            }
        },
    }
}

pub(in super::super) async fn cancel_delivery(
    connection: &AppServerConnection,
    request: i64,
) -> bool {
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
pub(in super::super) fn delivered_bytes(response: &Response) -> Vec<u8> {
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
pub(in super::super) fn failed_with(response: &Response, expected: &ErrorData) {
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
/// held at native operation admission; two inert metadata reads wait at the
/// catalog, making seventeen retained requests. The remaining control slot
/// admits delivery/cancel as the eighteenth, without
/// releasing a read permit or cancelling any other native operation.
pub(crate) async fn delivery_cancellation_capacity_scenario() {
    use crate::app_server::transport::{IN_FLIGHT_REQUESTS, stdio};
    use crate::tools::session_files::SESSION_FILE_MAX_READS;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    bounded(async {
        let mut phases = FixturePhase::new("eighteenth in-flight delivery cancellation");
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
        assert_eq!(IN_FLIGHT_REQUESTS, 18);
        let parties = delivery_capacity_parties(&own);
        assert_eq!(parties.len(), IN_FLIGHT_REQUESTS - 4);
        for request in &parties {
            input.write_all(line(request).as_bytes()).await.unwrap();
        }
        probe.before_operation.parked(parties.len()).await;

        phases.enter("park two independent metadata reads");
        let catalog = f.manager.sessions.catalog.lock().await;
        let mut operations = f.host.request_operations();
        let before_metadata = *operations.borrow();
        for (id, call) in [
            (80, Method::SessionRead { session_id: own.session_id.clone() }),
            (81, Method::SettingsRead { session_id: own.session_id.clone() }),
        ] {
            input.write_all(line(&Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(id),
                call,
            }).as_bytes()).await.unwrap();
        }
        operations.wait_for(|count| *count == before_metadata + 2).await.unwrap();

        phases.enter("eighteenth request cancellation response");
        // All six ordinary, eight Observe, two metadata and two control slots.
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
        assert!(!serving.is_finished(), "eighteen in flight is within capacity");
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

        phases.enter("release inert metadata reads");
        drop(catalog);
        let mut metadata_answered = std::collections::BTreeSet::new();
        for _ in 0..2 {
            let Response::Success(response) = next_response().await else {
                panic!("metadata reads remain independent of runtime admission");
            };
            match (&response.id, response.result) {
                (RequestId::Integer(80), MethodResult::Session { session }) => assert_eq!(session.id, own.session_id),
                (RequestId::Integer(81), MethodResult::Settings { .. }) => {},
                other => panic!("exact metadata outcome: {other:?}"),
            }
            assert!(metadata_answered.insert(response.id));
        }
        assert_eq!(metadata_answered.len(), 2);

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
