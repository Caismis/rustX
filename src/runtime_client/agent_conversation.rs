//! Child conversations reuse their own native projection and cursor stream.
//! A watch read waits for a real projection event, never polls SQLite for text.
//! Each response is a complete cut: late opens and reconnects repair output,
//! including canonical replacement of an in-flight message, without replaying
//! text deltas into a second conversation owner.
use super::{
    host::ClientInner,
    snapshot::RuntimeClientSnapshot,
    types::{
        RUNTIME_CLIENT_PROTOCOL_VERSION, RequestId, RuntimeClientCursor, RuntimeClientError,
        RuntimeClientProtocolEvent, RuntimeClientRequest, RuntimeClientResponse,
        RuntimeClientResult,
    },
};
use crate::runtime::identity::{AgentId, SubagentId};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentConversationCursor {
    pub activation_id: SubagentId,
    pub cursor: RuntimeClientCursor,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentConversation {
    pub agent_id: AgentId,
    pub activation_id: SubagentId,
    /// None for a durable, settled read, which has no live observation stream.
    pub cursor: Option<AgentConversationCursor>,
    pub snapshot: Box<RuntimeClientSnapshot>,
}
fn failed(error: impl std::fmt::Display) -> RuntimeClientError {
    RuntimeClientError::RuntimeFailure {
        message: format!("child conversation observation: {error}"),
    }
}
struct Peer {
    stream: BufReader<UnixStream>,
    id: u64,
}
impl Peer {
    async fn observe(
        path: std::path::PathBuf,
        conversation_id: crate::runtime::identity::ConversationId,
        activation: SubagentId,
        after: Option<AgentConversationCursor>,
    ) -> Result<(RuntimeClientSnapshot, RuntimeClientCursor), RuntimeClientError> {
        let mut peer = Peer {
            stream: BufReader::new(UnixStream::connect(path).await.map_err(failed)?),
            id: 0,
        };
        let request_id = peer.next_id();
        let RuntimeClientResult::Initialized {
            mut snapshot,
            mut cursor,
            ..
        } = peer
            .request(RuntimeClientRequest::Initialize {
                id: request_id,
                protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
            })
            .await?
        else {
            return Err(failed("unexpected initialization response"));
        };
        if snapshot.conversation_id != conversation_id {
            return Err(failed("child conversation identity changed"));
        }
        if after
            .as_ref()
            .is_some_and(|cut| cut.activation_id == activation && cut.cursor == cursor)
        {
            let request_id = peer.next_id();
            peer.request(RuntimeClientRequest::SubscribeEvents {
                id: request_id,
                after_cursor: cursor,
            })
            .await?;
            (snapshot, cursor) = peer.snapshot().await?;
            if after.as_ref().is_some_and(|cut| cut.cursor == cursor) {
                // Bounded waiter lifetime also permits admission/close fences to run.
                if let Ok(result) =
                    tokio::time::timeout(std::time::Duration::from_secs(10), peer.record()).await
                {
                    let _: RuntimeClientProtocolEvent =
                        serde_json::from_value(result?).map_err(failed)?;
                }
                (snapshot, cursor) = peer.snapshot().await?;
            }
        }
        Ok::<_, RuntimeClientError>((snapshot, cursor))
    }
    async fn record(&mut self) -> Result<serde_json::Value, RuntimeClientError> {
        let mut record = Vec::new();
        loop {
            let bytes = self.stream.fill_buf().await.map_err(failed)?;
            if bytes.is_empty() {
                return Err(failed("observation endpoint closed"));
            }
            let end = bytes.iter().position(|b| *b == b'\n');
            let count = end.map_or(bytes.len(), |p| p + 1);
            if record.len() + count > super::transport::stdio::STDIO_JSONL_MAX_RECORD_BYTES + 1 {
                return Err(failed("observation record exceeds native bound"));
            }
            record.extend_from_slice(&bytes[..count]);
            self.stream.consume(count);
            if end.is_some() {
                return serde_json::from_slice(&record).map_err(failed);
            }
        }
    }
    async fn request(
        &mut self,
        request: RuntimeClientRequest,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let expected = request.id();
        let mut bytes = serde_json::to_vec(&request).map_err(failed)?;
        bytes.push(b'\n');
        self.stream
            .get_mut()
            .write_all(&bytes)
            .await
            .map_err(failed)?;
        loop {
            let record = self.record().await?;
            if record.get("id").is_none() {
                // The subsequent authoritative snapshot includes these events.
                let _: RuntimeClientProtocolEvent =
                    serde_json::from_value(record).map_err(failed)?;
                continue;
            }
            let response: RuntimeClientResponse = serde_json::from_value(record).map_err(failed)?;
            if response.id != expected {
                return Err(failed("unexpected response identity"));
            }
            return response
                .result
                .ok_or_else(|| response.error.unwrap_or_else(|| failed("missing result")));
        }
    }
    fn next_id(&mut self) -> RequestId {
        self.id += 1;
        RequestId::new(self.id)
    }
    async fn snapshot(
        &mut self,
    ) -> Result<(RuntimeClientSnapshot, RuntimeClientCursor), RuntimeClientError> {
        let id = self.next_id();
        match self
            .request(RuntimeClientRequest::SnapshotGet { id })
            .await?
        {
            RuntimeClientResult::Snapshot { snapshot, cursor } => Ok((snapshot, cursor)),
            _ => Err(failed("unexpected snapshot response")),
        }
    }
}
impl ClientInner {
    fn durable_agent_conversation(
        &self,
        id: &AgentId,
        activation: &SubagentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let store = self
            .agent_registry()?
            .transcript_store(activation)
            .map_err(|e| failed(format!("{e:?}")))?;
        let host = super::host::RuntimeClientHost::new_durable(std::sync::Arc::new(store), None)
            .map_err(failed)?;
        let (snapshot, _) = host.snapshot()?;
        let current = self.agent_view(id)?;
        if current.activation_id != *activation
            || current.child_conversation_id != snapshot.conversation_id
        {
            return Err(failed("child activation changed; reopen its conversation"));
        }
        Ok(RuntimeClientResult::AgentConversation {
            conversation: AgentConversation {
                agent_id: id.clone(),
                activation_id: activation.clone(),
                cursor: None,
                snapshot: Box::new(snapshot),
            },
        })
    }
    pub(crate) async fn agent_conversation(
        &self,
        id: &AgentId,
        after: Option<AgentConversationCursor>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let registry = self.agent_registry()?;
        let mut agent = self.agent_view(id)?;
        if agent.state == crate::runtime::subagent::AgentState::Admitting {
            tokio::time::timeout(
                std::time::Duration::from_secs(15),
                registry.wait_for_agent_state(id, |state| {
                    state != crate::runtime::subagent::AgentState::Admitting
                }),
            )
            .await
            .map_err(|_| failed("activation admission is still pending"))?
            .ok_or_else(|| RuntimeClientError::UnknownAgent {
                agent_id: id.clone(),
            })?;
            agent = self.agent_view(id)?;
        }
        let activation = agent.activation_id.clone();
        if agent.state == crate::runtime::subagent::AgentState::Inactive
            || agent.state == crate::runtime::subagent::AgentState::Unavailable
        {
            return self.durable_agent_conversation(id, &activation);
        }
        let observed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let path = registry.agent_inspection_path(id).ok_or_else(|| {
                RuntimeClientError::UnknownAgent {
                    agent_id: id.clone(),
                }
            })?;
            Peer::observe(
                path,
                agent.child_conversation_id.clone(),
                activation.clone(),
                after,
            )
            .await
        })
        .await
        .map_err(|_| failed("live observation exceeded its deadline"))
        .and_then(|result| result);
        let (snapshot, cursor) = match observed {
            Ok(cut) => cut,
            Err(error) => {
                let current = self.agent_view(id)?;
                if current.activation_id != activation {
                    return Err(error);
                }
                if current.state == crate::runtime::subagent::AgentState::Stopping {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        registry.wait_for_agent_state(id, |state| {
                            matches!(
                                state,
                                crate::runtime::subagent::AgentState::Inactive
                                    | crate::runtime::subagent::AgentState::Unavailable
                            )
                        }),
                    )
                    .await;
                }
                let current = self.agent_view(id)?;
                if current.activation_id == activation
                    && matches!(
                        current.state,
                        crate::runtime::subagent::AgentState::Inactive
                            | crate::runtime::subagent::AgentState::Unavailable
                    )
                {
                    return self.durable_agent_conversation(id, &activation);
                }
                return Err(error);
            }
        };
        let current = self.agent_view(id)?;
        if current.activation_id != activation {
            return Err(failed("child activation changed; reopen its conversation"));
        }
        Ok(RuntimeClientResult::AgentConversation {
            conversation: AgentConversation {
                agent_id: id.clone(),
                activation_id: activation.clone(),
                cursor: Some(AgentConversationCursor {
                    activation_id: activation,
                    cursor,
                }),
                snapshot: Box::new(snapshot),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn peer_requires_bounded_complete_jsonl_records() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut peer = Peer {
            stream: BufReader::new(client),
            id: 0,
        };
        server.write_all(b"{\"cut\":1}\n{\"cut\":2}").await.unwrap();
        drop(server);
        assert_eq!(peer.record().await.unwrap(), serde_json::json!({"cut":1}));
        assert!(
            peer.record().await.is_err(),
            "unterminated EOF is never a cut"
        );
    }

    #[tokio::test]
    async fn peer_refuses_oversized_native_records_before_deserialization() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let writing = tokio::spawn(async move {
            let bytes =
                vec![b'x'; super::super::transport::stdio::STDIO_JSONL_MAX_RECORD_BYTES + 2];
            let _ = server.write_all(&bytes).await;
        });
        let mut peer = Peer {
            stream: BufReader::new(client),
            id: 0,
        };
        assert!(
            matches!(peer.record().await, Err(RuntimeClientError::RuntimeFailure { message }) if message.contains("exceeds native bound"))
        );
        drop(peer);
        writing.await.unwrap();
    }
}
