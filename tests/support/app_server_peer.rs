//! Small public-protocol peer for real socket and process boundary scenarios.
#![allow(dead_code)]
use futures_util::{SinkExt, StreamExt};
use rustx::{app_server::protocol::*, runtime_client::RuntimeClientEvent};
use std::collections::VecDeque;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
pub struct Peer {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    callbacks: VecDeque<ServerMessage>,
    notifications: VecDeque<Notification>,
    next: i64,
}
impl Peer {
    pub async fn connect(endpoint: &str) -> Self {
        let mut request = endpoint.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Authorization", "Bearer trusted-host-test".parse().unwrap());
        let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let mut peer = Self {
            socket,
            callbacks: VecDeque::new(),
            notifications: VecDeque::new(),
            next: 0,
        };
        peer.rpc(Method::Initialize(InitializeParams {
            protocol_version: APP_SERVER_PROTOCOL_VERSION,
            client: ClientIdentity {
                name: "external-host-handler".into(),
                version: "1".into(),
            },
            presentation: PresentationCapabilities::default(),
        }))
        .await;
        peer
    }
    pub async fn receive(&mut self) -> ServerOutput {
        let Message::Text(text) = self.socket.next().await.unwrap().unwrap() else {
            panic!("JSON-RPC text")
        };
        serde_json::from_str(&text).unwrap()
    }
    pub async fn response(&mut self, call: Method) -> Response {
        self.next += 1;
        let id = RequestId::Integer(self.next);
        self.socket
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
            match self.receive().await {
                ServerOutput::Response(response) => {
                    let received = match &response {
                        Response::Success(value) => &value.id,
                        Response::Failure(value) => {
                            value.id.as_ref().expect("correlated RPC failure")
                        }
                    };
                    assert_eq!(received, &id);
                    return response;
                }
                ServerOutput::Callback(callback) => self.callbacks.push_back(callback),
                ServerOutput::Notification(notification) => {
                    self.notifications.push_back(notification);
                }
            }
        }
    }
    pub async fn rpc(&mut self, call: Method) -> MethodResult {
        match self.response(call).await {
            Response::Success(value) => value.result,
            Response::Failure(value) => panic!("RPC failed: {value:?}"),
        }
    }
    pub async fn follow(
        &mut self,
        thread_id: rustx::runtime::identity::ConversationId,
    ) -> AttachmentTarget {
        let MethodResult::Attached { target, .. } =
            self.rpc(Method::SessionAttach { thread_id }).await
        else {
            panic!("resume")
        };
        let MethodResult::Checkpoint { checkpoint } = self
            .rpc(Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            })
            .await
        else {
            panic!("checkpoint")
        };
        self.rpc(Method::SessionSubscribe {
            target: target.clone(),
            after_cursor: checkpoint.cursor,
        })
        .await;
        target
    }
    pub async fn callback(&mut self) -> ServerMessage {
        if let Some(callback) = self.callbacks.pop_front() {
            return callback;
        }
        loop {
            match self.receive().await {
                ServerOutput::Callback(callback) => return callback,
                ServerOutput::Notification(notification) => {
                    self.notifications.push_back(notification);
                }
                ServerOutput::Response(_) => panic!("unsolicited response"),
            }
        }
    }
    pub async fn reply(&mut self, value: serde_json::Value) {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
    }
    pub async fn settled(&mut self, expected: &rustx::runtime::identity::AttemptId) {
        loop {
            let notification = if let Some(notification) = self.notifications.pop_front() {
                notification
            } else {
                match self.receive().await {
                    ServerOutput::Notification(notification) => notification,
                    ServerOutput::Callback(callback) => {
                        self.callbacks.push_back(callback);
                        continue;
                    }
                    ServerOutput::Response(_) => panic!("unsolicited response"),
                }
            };
            if let NotificationMethod::Event { event, .. } = notification.notification
                && event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { attempt_id,.. } if attempt_id==expected))
            {
                return;
            }
        }
    }
}
