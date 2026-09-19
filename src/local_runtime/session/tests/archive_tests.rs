//! Real durable fixtures; gates are completion/queue boundaries, never sleeps.
use super::*;
use crate::session_archive::{SessionArchiveCut, SessionArchiveProducer};
use crate::tools::artifacts::ArtifactStore;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use tokio_util::sync::CancellationToken;

async fn decode(cut: SessionArchiveCut) -> BTreeMap<String, Vec<u8>> {
    let mut stream = cut.stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.recv().await {
        bytes.extend(chunk.unwrap());
    }
    decode_bytes(bytes)
}
fn decode_bytes(bytes: Vec<u8>) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut files = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        assert!(files.insert(entry.name().to_owned(), bytes).is_none());
    }
    files
}
fn records(
    files: &BTreeMap<String, Vec<u8>>,
    conversation: &ConversationId,
    name: &str,
) -> Vec<serde_json::Value> {
    std::str::from_utf8(&files[&format!("sessions/{conversation}/{name}.jsonl")])
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One gated cross-authority history cut.
async fn archive_cut_excludes_live_writes_and_later_descendants() {
    let (directory, catalog, _) = open_catalog();
    let secret_text = "literal user text: Authorization: Bearer sk-authored-123";
    let (conversation, session, _) = append_history(&catalog, &[user("before", secret_text)]);
    let store = store_for(&catalog, &session, &conversation);
    let child = super::deletion_tests::child(directory.path(), &store, 1, false);
    let status_id = MessageId::new("archive-status");
    let snapshot = todo_status_start_snapshot(
        store.load_head().unwrap().revision.next(),
        status_id.clone(),
        AgentStatusEmission {
            module_id: AgentStatusModuleId::Todo,
            key: "active_actionable".into(),
            fingerprint: "archive-fingerprint".into(),
        },
    );
    let receipt = store
        .commit_model_turn_start(&[todo_status(status_id.as_str())], &snapshot, Utc::now())
        .unwrap();
    let mut terminal = receipt.started;
    terminal.event_id = crate::runtime::identity::EventId::new("archive-terminal");
    terminal.sequence = 0;
    terminal.event = crate::events::types::RuntimeEvent::ModelRequestCompleted {
        request_id: snapshot.request_id.clone(),
        finish_reason: crate::model::ModelFinishReason::Stop,
        usage: None,
        generation: Some(crate::model::generation_evidence::GenerationEvidence {
            dispatch_after_start_ms: Some(2),
            first_output_ms: Some(3),
            last_output_ms: Some(4),
            terminal_ms: 5,
        }),
    };
    store.append_event(terminal).unwrap();
    fs::write(
        directory.path().join("credentials"),
        "INFRASTRUCTURE_SECRET",
    )
    .unwrap();
    let cut =
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .unwrap();
    let (reached, wait_reached) = tokio::sync::oneshot::channel();
    let (release, proceed) = std::sync::mpsc::channel();
    let producing = tokio::task::spawn_blocking(move || {
        let mut writer = GatedWriter {
            reached: Some(reached),
            proceed,
            bytes: Vec::new(),
        };
        cut.write_test(&mut writer).unwrap();
        writer.bytes
    });
    wait_reached.await.unwrap();
    // These synchronous native commits would fail with SQLITE_BUSY if any
    // capture read barrier survived preparation. ZIP writing is parked at its first write.
    store
        .append_canonical(&user("after", "EXCLUDED_AFTER_CUT"))
        .unwrap();
    let later = super::deletion_tests::child(directory.path(), &store, 2, false);
    let child_path =
        crate::runtime::subagent::child_conversation_store_path(directory.path(), &session, &child);
    let child_store = SqliteConversationStore::open(child.clone(), &child_path).unwrap();
    child_store
        .append_canonical(&user("child-later", "CHILD_AFTER_CUT"))
        .unwrap();
    let mut later_request = snapshot.clone();
    later_request.identity.retry_number = 1;
    later_request.request_id = later_request.identity.request_id();
    later_request.provisional_message_id = later_request.identity.provisional_message_id();
    later_request.surface_revision = store.load_head().unwrap().revision;
    later_request.request_context_ids.clear();
    later_request.agent_status = None;
    let later_receipt = store
        .commit_model_turn_start(&[], &later_request, Utc::now())
        .unwrap();
    let mut later_terminal = later_receipt.started;
    later_terminal.sequence = 0;
    later_terminal.event_id = crate::runtime::identity::EventId::new("later-terminal");
    later_terminal.event = crate::events::types::RuntimeEvent::ModelRequestCompleted {
        request_id: later_request.request_id,
        finish_reason: crate::model::ModelFinishReason::Stop,
        usage: None,
        generation: Some(crate::model::generation_evidence::GenerationEvidence {
            dispatch_after_start_ms: None,
            first_output_ms: None,
            last_output_ms: None,
            terminal_ms: 99,
        }),
    };
    store.append_event(later_terminal).unwrap();
    release.send(()).unwrap();
    let files = decode_bytes(producing.await.unwrap());
    let manifest: serde_json::Value = serde_json::from_slice(&files["manifest.json"]).unwrap();
    assert_eq!(manifest["format"], "rustx-session-archive/v1");
    assert_eq!(manifest["conversations"].as_array().unwrap().len(), 2);
    assert!(files.contains_key(&format!("sessions/{child}/journal.jsonl")));
    assert!(!files.contains_key(&format!("sessions/{later}/journal.jsonl")));
    assert_eq!(records(&files, &conversation, "messages").len(), 2);
    assert_eq!(records(&files, &conversation, "surface").len(), 2);
    assert_eq!(
        records(&files, &conversation, "requests")[0]["request_id"],
        serde_json::json!(snapshot.request_id)
    );
    assert_eq!(
        records(&files, &conversation, "generations")[0]["evidence"]["terminal_ms"],
        5
    );
    assert_eq!(records(&files, &conversation, "requests").len(), 1);
    assert_eq!(records(&files, &conversation, "generations").len(), 1);
    assert!(records(&files, &child, "messages").is_empty());
    let all = files
        .values()
        .map(|b| String::from_utf8_lossy(b))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains(secret_text));
    for excluded in [
        "EXCLUDED_AFTER_CUT",
        "CHILD_AFTER_CUT",
        "INFRASTRUCTURE_SECRET",
    ] {
        assert!(!all.contains(excluded));
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Artifact identity, streaming and failure lifecycle.
async fn archive_streams_deduplicated_large_artifact_and_rejects_missing_bytes() {
    let (directory, catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let store = store_for(&catalog, &session, &conversation);
    let root = catalog
        .database_path(&session, &conversation)
        .parent()
        .unwrap()
        .to_path_buf();
    let artifacts = ArtifactStore::new(conversation.clone(), &root).unwrap();
    let id = artifacts.create_artifact().unwrap();
    let mut writer = artifacts.open_writer(&id).unwrap();
    let mut seed = 0x1234_5678_u32;
    let block: Vec<u8> = (0..65536)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed.to_le_bytes()[0]
        })
        .collect();
    for _ in 0..64 {
        writer.write_all(&block).unwrap();
    }
    let reference = UserContentBlock::File(crate::message::content::FileReference {
        artifact_id: id.clone(),
        name: Some("large.bin".into()),
        mime_type: None,
        description: None,
    });
    let mut message = user("artifact-user", "retained");
    if let MessageBlock::User(user) = &mut message {
        user.content.extend([reference.clone(), reference]);
    }
    store.append_canonical(&message).unwrap();
    assert!(
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .is_err(),
        "an unsealed artifact is not historical bytes"
    );
    drop(writer);
    let cut =
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .unwrap();
    let mut streaming = cut.stream();
    let mut received = 0;
    while received < 8 * crate::session_archive::CHUNK_BYTES {
        received += streaming.recv().await.unwrap().unwrap().len();
    }
    // Incompressible artifact bytes have started; cancel waits for the native
    // reader/container worker to exit, including a capacity-blocked writer.
    streaming.cancel().await;
    let cut =
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .unwrap();
    let files = decode(cut).await;
    assert_eq!(
        files.keys().filter(|k| k.starts_with("artifacts/")).count(),
        1
    );
    assert_eq!(
        files[&format!("artifacts/{conversation}/{id}/content")],
        block.repeat(64)
    );
    let cut =
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(root.join(format!("{id}.bin")))
        .unwrap()
        .set_len(1)
        .unwrap();
    let mut damaged = cut.stream();
    let mut failed = false;
    while let Some(result) = damaged.recv().await {
        if result.is_err() {
            failed = true;
            break;
        }
    }
    assert!(
        failed,
        "post-preflight artifact corruption cannot become successful EOF"
    );
    fs::remove_file(root.join(format!("{id}.bin"))).unwrap();
    assert!(
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .is_err()
    );
}

#[test]
fn archive_missing_required_child_and_cancellation_fail_preparation() {
    let (directory, catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let store = store_for(&catalog, &session, &conversation);
    let child = super::deletion_tests::child(directory.path(), &store, 1, false);
    let path =
        crate::runtime::subagent::child_conversation_store_path(directory.path(), &session, &child);
    fs::remove_file(path).unwrap();
    assert!(
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .is_err()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        SessionArchiveProducer::prepare(directory.path(), &session, &cancelled)
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
}

struct GatedWriter {
    reached: Option<tokio::sync::oneshot::Sender<()>>,
    proceed: std::sync::mpsc::Receiver<()>,
    bytes: Vec<u8>,
}
impl Write for GatedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(reached) = self.reached.take() {
            reached.send(()).unwrap();
            self.proceed.recv().unwrap();
        }
        self.bytes.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
