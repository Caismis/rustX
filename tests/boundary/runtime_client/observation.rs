//! #467 real Unix IPC closure. Native initialization carries identity only;
//! the large canonical source is reconstructed through bounded range reads.
#![cfg(unix)]
use crate::durable::SqliteConversationStore;
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime_client::{
    RUNTIME_CLIENT_PROTOCOL_VERSION, RequestId, RuntimeClientHost, RuntimeClientRequest,
    RuntimeClientResponse, RuntimeClientResult,
};
use rustx::durable::CanonicalStore;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::test]
async fn t04_native_unix_initialize_and_checkpoint_never_carry_an_eight_mib_snapshot() {
    let conversation = ConversationId::new("conv_3815b3a1-e438-7d28-90ac-2bd07745291b");
    let store = Arc::new(SqliteConversationStore::in_memory(conversation).unwrap());
    let text = "🙂\"\\\n".repeat(2_000_000);
    let expected_len = text.len() as u64;
    let user = crate::message::types::MessageBlock::User(crate::message::types::UserMessageBlock {
        id: MessageId::new("enormous-user"),
        content: vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock { text },
        )],
        source: crate::message::types::UserSource::Human,
        kind: crate::message::types::InboundKind::Message,
        timestamp: None,
    });
    store.initialize(&[user]).unwrap();
    let host = RuntimeClientHost::new_durable(store, None).unwrap();
    let endpoint = crate::runtime_client::RuntimeClientEndpoint::new_read_only(&host);
    let (client, server) = UnixStream::pair().unwrap();
    let (server_read, server_write) = server.into_split();
    let serving = tokio::spawn(
        crate::runtime_client::transport::stdio::serve_stdio_jsonl_with_io(
            endpoint,
            server_read,
            server_write,
        ),
    );
    let (client_read, mut client_write) = client.into_split();
    let mut read = BufReader::new(client_read);
    let initialize = RuntimeClientRequest::Initialize {
        id: RequestId::new(1),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
    };
    client_write
        .write_all(&serde_json::to_vec(&initialize).unwrap())
        .await
        .unwrap();
    client_write.write_all(b"\n").await.unwrap();
    let mut record = String::new();
    read.read_line(&mut record).await.unwrap();
    assert!(
        record.len() < 1024,
        "identity receipt is independent of history bytes"
    );
    let initialized: RuntimeClientResponse = serde_json::from_str(&record).unwrap();
    assert!(matches!(
        initialized.result,
        Some(RuntimeClientResult::Initialized {
            read_only: true,
            ..
        })
    ));
    assert!(!record.contains("snapshot"));
    record.clear();
    client_write
        .write_all(
            &serde_json::to_vec(&RuntimeClientRequest::CheckpointGet {
                id: RequestId::new(2),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    client_write.write_all(b"\n").await.unwrap();
    read.read_line(&mut record).await.unwrap();
    assert!(record.len() <= crate::runtime_client::checkpoint::CHECKPOINT_TARGET_BYTES);
    let response: RuntimeClientResponse = serde_json::from_str(&record).unwrap();
    let Some(RuntimeClientResult::Checkpoint { checkpoint }) = response.result else {
        panic!("bounded native checkpoint")
    };
    assert!(
        checkpoint
            .content
            .iter()
            .any(|binding| binding.content_ref.readable_end == expected_len)
    );
    assert!(checkpoint.snapshot.messages.len() <= 32);
    client_write.shutdown().await.unwrap();
    drop(read);
    serving.await.unwrap().unwrap();
}
