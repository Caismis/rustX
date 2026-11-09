//! Real browser/native upgrade admission. The clock controls JWT admission only.
use super::{Fixture, bounded};
use crate::app_server::{protocol::*, transport::websocket};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use sha2::Digest;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
async fn rpc(socket: &mut Socket, call: Method) -> MethodResult {
    let id = RequestId::String(uuid::Uuid::new_v4().to_string());
    socket
        .send(Message::Text(
            serde_json::to_string(&Request {
                jsonrpc: JsonRpcVersion::V2,
                id: id.clone(),
                call,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    loop {
        let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
            panic!("RPC text")
        };
        match serde_json::from_str::<ServerOutput>(&text).unwrap() {
            ServerOutput::Response(Response::Success(response)) => {
                assert_eq!(response.id, id);
                return response.result;
            }
            ServerOutput::Notification(_) => {}
            output => panic!("unexpected output: {output:?}"),
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_and_browser_share_validation_and_jwt_expiry_only_affects_new_admission() {
    bounded(async {
        let f=Fixture::with_session_count(None,0).await;
        f.manager.bind_process_policy(crate::local_runtime::app_server_policy::AppServerPolicy {environment_roots:f.workspaces.to_vec(),..Default::default()});
        let secret=b"host-signing-secret-with-at-least-32-bytes";
        let clock=Arc::new(AtomicI64::new(100));
        let signed=jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),&serde_json::json!({"exp":110,"nbf":90,"iss":"host","aud":"rustx","role":"readonly","thread":"unrelated"}),&jsonwebtoken::EncodingKey::from_secret(secret)).unwrap();
        let opaque="host/capability+token==";
        let modes=[(websocket::Credential::new(opaque.into()).unwrap(),opaque.to_owned()),(websocket::Credential::from_sha256(&format!("{:x}",sha2::Sha256::digest(opaque))).unwrap(),opaque.to_owned()),(websocket::Credential::signed(secret,Some("host".into()),Some("rustx".into()),0).unwrap().with_clock(clock.clone()),signed)];
        for (index,(policy,token)) in modes.into_iter().enumerate() {
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let endpoint=format!("ws://{}",listener.local_addr().unwrap());
            let stop=CancellationToken::new(); let server=tokio::spawn(websocket::serve(listener,f.host.clone(),policy,stop.clone()));
            let mut peers=Vec::new();
            for browser in [false,true] {
                let mut request=endpoint.clone().into_client_request().unwrap();
                if browser {request.headers_mut().insert("Origin","https://trusted-product-host.example".parse().unwrap());request.headers_mut().insert("Sec-WebSocket-Protocol",format!("{}, rustx-token.{}",websocket::SUBPROTOCOL,base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&token)).parse().unwrap());}
                else {request.headers_mut().insert("Authorization",format!("Bearer {token}").parse().unwrap());}
                let (mut socket,response)=tokio_tungstenite::connect_async(request).await.unwrap();
                assert_eq!(response.headers().contains_key("Sec-WebSocket-Protocol"),browser);
                let MethodResult::Initialized {capabilities,..}=rpc(&mut socket,Method::Initialize(InitializeParams {protocol_version:APP_SERVER_PROTOCOL_VERSION,client:ClientIdentity {name:if browser {"browser"}else{"native"}.into(),version:"1".into()},presentation:PresentationCapabilities::default()})).await else {panic!("initialized")};
                assert!(capabilities.shared_writable_clients);
                let path=f.workspaces[0].join(format!("trusted-environment-{index}-{browser}"));
                assert!(matches!(rpc(&mut socket,Method::FsWriteFile {path:path.display().to_string(),data_base64:"dHJ1c3RlZA==".into(),expected_revision:None}).await,MethodResult::FileMetadata {..}));
                assert!(matches!(rpc(&mut socket,Method::FsGetMetadata {path:path.display().to_string()}).await,MethodResult::FileMetadata {..}));
                let params=serde_json::from_value(serde_json::json!({"command":["/usr/bin/printf","trusted"],"cwd":f.workspaces[0]})).unwrap();
                let MethodResult::CommandExec {result}=rpc(&mut socket,Method::CommandExec(params)).await else {panic!("equal command eligibility")};
                assert_eq!(result.exit_code,Some(0));
                assert!(matches!(rpc(&mut socket,Method::SkillsList {target:crate::local_runtime::configuration::settings::SourceTarget::User,offset:0,limit:32,expected_revision:None}).await,MethodResult::Skills {..}));
                peers.push(socket);
            }
            let MethodResult::SessionTransition {thread,..}=rpc(&mut peers[0],Method::SessionCreate {settings:crate::local_runtime::session::SessionPersistentState {cwd:f.workspaces[0].clone(),model:None},dynamic_tools:vec![],submission:None}).await else {panic!("ordinary write eligibility")};
            assert!(matches!(rpc(&mut peers[1],Method::ThreadRead {thread_id:thread.id}).await,MethodResult::Thread {..}));
            for credential in [None,Some("wrong")] {
                let mut request=endpoint.clone().into_client_request().unwrap();if let Some(token)=credential {request.headers_mut().insert("Authorization",format!("Bearer {token}").parse().unwrap());}
                assert!(tokio_tungstenite::connect_async(request).await.is_err());
            }
            let mut ambiguous=endpoint.clone().into_client_request().unwrap(); ambiguous.headers_mut().insert("Authorization",format!("Bearer {token}").parse().unwrap());ambiguous.headers_mut().insert("Sec-WebSocket-Protocol",format!("{}, rustx-token.{}",websocket::SUBPROTOCOL,base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&token)).parse().unwrap()); assert!(tokio_tungstenite::connect_async(ambiguous).await.is_err());
            if index==2 {
                clock.store(111,Ordering::Release);
                // Established clients remain trusted after exp; reconnection
                // is a fresh validation against the advanced clock.
                for peer in &mut peers {assert!(matches!(rpc(peer,Method::ServerInfo {}).await,MethodResult::ServerInfo {..}));}
                let mut request=endpoint.into_client_request().unwrap();request.headers_mut().insert("Authorization",format!("Bearer {token}").parse().unwrap());assert!(tokio_tungstenite::connect_async(request).await.is_err());
            }
            drop(peers);stop.cancel();server.await.unwrap().unwrap();
        }
        f.close().await;
    }).await;
}
