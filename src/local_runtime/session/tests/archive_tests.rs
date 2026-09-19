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
            .unwrap(),
        crate::session_archive::SessionArchivePrepareError::Cancelled
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn archive_request_projection_excludes_infrastructure_and_preserves_authored_history() {
    use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, ReasoningBlock};
    use crate::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
    use crate::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
    let (directory, catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(
        &catalog,
        &[user(
            "authored",
            "Please analyze the literal string Authorization: Bearer AUTHORED_TEXT_SECRET",
        )],
    );
    let store = store_for(&catalog, &session, &conversation);
    let private = |secret: &str| {
        Some(ProviderContinuationState::Anthropic(
            AnthropicContinuation {
                opaque: serde_json::json!(secret),
            },
        ))
    };
    store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("reasoning"),
            content: vec![AssistantContentBlock::Reasoning(ReasoningBlock {
                text: Some("AUTHORED_REASONING_SECRET".into()),
                provider_state: private("ARCHIVE_REASONING_SECRET"),
            })],
        }))
        .unwrap();
    let status_id = MessageId::new("archive-safe-status");
    let mut snapshot = todo_status_start_snapshot(
        store.load_head().unwrap().revision.next(),
        status_id.clone(),
        AgentStatusEmission {
            module_id: AgentStatusModuleId::Todo,
            key: "active_actionable".into(),
            fingerprint: "safe".into(),
        },
    );
    snapshot.invocation.request_params = serde_json::from_value(serde_json::json!({
        "temperature":0.25, "api_key":"ARCHIVE_PROVIDER_SECRET",
        "authorization":"Bearer ARCHIVE_AUTH_SECRET", "executor_env":"ARCHIVE_EXECUTOR_SECRET",
        "some_unknown_future_secret_key":"ARCHIVE_UNKNOWN_SECRET"
    }))
    .unwrap();
    snapshot.continuation = private("ARCHIVE_CONTINUATION_SECRET");
    let receipt = store
        .commit_model_turn_start(&[todo_status(status_id.as_str())], &snapshot, Utc::now())
        .unwrap();
    let mut event = receipt.started;
    event.sequence = 0;
    event.event_id = crate::runtime::identity::EventId::new("safe-model-failed");
    event.event = crate::events::types::RuntimeEvent::ModelRequestFailed {
        request_id: snapshot.request_id.clone(),
        error: ModelError {
            kind: ModelErrorKind::Authentication,
            message: "ARCHIVE_PROVIDER_DIAGNOSTIC_SECRET".into(),
            provider_code: Some("ARCHIVE_PROVIDER_CODE_SECRET".into()),
            retry_disposition: ModelRetryDisposition::Never,
            retry_after_ms: None,
            context_overflow: None,
            malformed_tool_proposal: None,
            timeout_phase: None,
            generation: None,
        },
        usage: None,
        generation: None,
    };
    store.append_event(event).unwrap();
    let files = decode(
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .unwrap(),
    )
    .await;
    let all = files
        .values()
        .map(|v| String::from_utf8_lossy(v))
        .collect::<Vec<_>>()
        .join("\n");
    for secret in [
        "ARCHIVE_PROVIDER_SECRET",
        "ARCHIVE_AUTH_SECRET",
        "ARCHIVE_EXECUTOR_SECRET",
        "ARCHIVE_UNKNOWN_SECRET",
        "ARCHIVE_CONTINUATION_SECRET",
        "ARCHIVE_REASONING_SECRET",
        "ARCHIVE_PROVIDER_DIAGNOSTIC_SECRET",
        "ARCHIVE_PROVIDER_CODE_SECRET",
    ] {
        assert!(!all.contains(secret), "leaked {secret}");
    }
    let messages = records(&files, &conversation, "messages");
    assert!(
        messages[0].to_string().contains(
            "Please analyze the literal string Authorization: Bearer AUTHORED_TEXT_SECRET"
        )
    );
    assert!(
        messages[1]
            .to_string()
            .contains("AUTHORED_REASONING_SECRET")
    );
    let request = &records(&files, &conversation, "requests")[0];
    assert_eq!(
        request["invocation"]["request_options"],
        serde_json::json!({"temperature":0.25})
    );
    assert_eq!(request["invocation"]["omitted_option_count"], 4);
    assert!(request.get("continuation").is_none());
    assert!(request["invocation"].get("request_params").is_none());
    assert_eq!(
        request["request_id"],
        serde_json::json!(snapshot.request_id)
    );
    let failure = records(&files, &conversation, "journal")
        .into_iter()
        .find(|v| v["event"]["type"] == "model_request_failed")
        .unwrap();
    assert_eq!(failure["event"]["error"]["kind"], "authentication");
    assert!(failure["event"]["error"].get("message").is_none());
}

#[tokio::test]
async fn archive_preserves_native_inherited_response_provenance_without_execution() {
    use crate::durable::response::{CompletedResponseProvenance, ResponseOrigin};
    use crate::message::types::{AssistantContentBlock, AssistantMessageBlock};
    let (directory, mut catalog, _) = open_catalog();
    let (conversation, session, node) = append_history(
        &catalog,
        &[
            user("input", "authored"),
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("answer"),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: "inherited answer".into(),
                })],
            }),
        ],
    );
    let store = store_for(&catalog, &session, &conversation);
    let revision = store.load_head().unwrap().revision;
    let mut source = lineage_at(&store, &conversation, revision);
    let origin = ResponseOrigin {
        conversation_id: conversation,
        attempt_id: crate::runtime::identity::AttemptId::new("source-attempt"),
        closing_message_id: MessageId::new("answer"),
    };
    source.completed_responses = vec![CompletedResponseProvenance {
        closing_message_id: MessageId::new("answer"),
        origin: origin.clone(),
        completed_at: Utc::now(),
        retry_message_id: Some(MessageId::new("input")),
        usage: None,
        timing: None,
    }];
    let cloned = catalog.prepare_clone_session(&state(), &source).unwrap();
    catalog
        .publish_session(
            &cloned,
            SessionNodeOrigin::Clone {
                source_session: session,
                source_node: node,
                source_surface_revision: revision,
            },
        )
        .unwrap();
    let cut = SessionArchiveProducer::prepare(
        directory.path(),
        &cloned.session_id,
        &CancellationToken::new(),
    )
    .unwrap();
    let files = decode(cut).await;
    let responses = records(&files, &cloned.conversation_id, "inherited_responses");
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["origin"], serde_json::json!(origin));
    assert!(records(&files, &cloned.conversation_id, "journal").is_empty());
    assert!(records(&files, &cloned.conversation_id, "requests").is_empty());
}
