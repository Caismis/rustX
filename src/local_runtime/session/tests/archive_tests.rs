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
    assert_eq!(
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .err()
            .unwrap(),
        crate::session_archive::SessionArchivePrepareError::ArtifactUnavailable,
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

fn claim_child(parent: &SqliteConversationStore, child: &ConversationId) {
    use crate::runtime::identity::{AgentId, SubagentId};
    let subagent = SubagentId::for_conversation(parent.conversation_id(), 1);
    parent
        .append_event(crate::runtime::subagent::ownership_event(
            parent.conversation_id(),
            &subagent,
            &AgentId::new("duplicate-owner"),
            child,
            &ToolCallId::new("duplicate-call"),
            &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
            &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
            &serde_json::from_value(serde_json::json!(format!("sha256:{}", "a".repeat(64))))
                .unwrap(),
            crate::events::types::SubagentOwnershipKind::Normal,
            &crate::runtime::workspace::WorkspaceSnapshot::shared(std::path::PathBuf::from(
                "/authored/workspace",
            )),
            Utc::now(),
        ))
        .unwrap();
}

fn assert_ownership_rejected_before_archive_capture(root: &std::path::Path, session: &SessionId) {
    assert_eq!(
        SessionArchiveProducer::prepare_inner(root, session, &CancellationToken::new(), || {
            panic!("ambiguous ownership reached archive cut capture");
        })
        .err()
        .unwrap(),
        crate::session_archive::SessionArchivePrepareError::CorruptAuthority,
    );
    assert!(
        crate::local_runtime::session_deletion::DeletionTargetSnapshot::inspect(root, session)
            .is_err()
    );
}

#[test]
fn archive_global_ownership_rejects_same_child_across_sessions_before_cut() {
    let (root, mut catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let store = store_for(&catalog, &session, &conversation);
    let child = super::deletion_tests::child(root.path(), &store, 1, false);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let other = catalog.prepare_clone_session(&state(), &source).unwrap();
    catalog
        .publish_session(&other, SessionNodeOrigin::New)
        .unwrap();
    let other_store = store_for(&catalog, &other.session_id, &other.conversation_id);
    claim_child(&other_store, &child);
    // Deliberately materialize both invalid private allocations. Neither export
    // may appear successful merely because its local child bytes exist.
    let foreign_path = crate::runtime::subagent::child_conversation_store_path(
        root.path(),
        &other.session_id,
        &child,
    );
    fs::create_dir_all(foreign_path.parent().unwrap()).unwrap();
    SqliteConversationStore::open(child.clone(), &foreign_path)
        .unwrap()
        .initialize(&[])
        .unwrap();
    for selected in [&session, &other.session_id] {
        assert_ownership_rejected_before_archive_capture(root.path(), selected);
    }
    // Ambiguity still wins if traversal sees an unavailable allocation first.
    fs::remove_file(foreign_path).unwrap();
    for selected in [&session, &other.session_id] {
        assert_ownership_rejected_before_archive_capture(root.path(), selected);
    }
}

#[test]
fn archive_global_ownership_rejects_two_parents_and_cycles_before_cut() {
    let (root, catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let store = store_for(&catalog, &session, &conversation);
    let first = super::deletion_tests::child(root.path(), &store, 1, false);
    let second = super::deletion_tests::child(root.path(), &store, 2, false);
    let first_store = store_for(&catalog, &session, &first);
    let second_store = store_for(&catalog, &session, &second);
    let shared = super::deletion_tests::child(root.path(), &first_store, 1, false);
    claim_child(&second_store, &shared);
    assert_ownership_rejected_before_archive_capture(root.path(), &session);

    let (root, catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &[]);
    let store = store_for(&catalog, &session, &conversation);
    let child = super::deletion_tests::child(root.path(), &store, 1, false);
    claim_child(&store_for(&catalog, &session, &child), &conversation);
    assert_ownership_rejected_before_archive_capture(root.path(), &session);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Real durable events across every Journal status carrier.
async fn archive_tool_status_diagnostics_are_excluded_but_canonical_tool_history_is_exact() {
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::runtime::identity::EventId;
    use crate::tools::invocation::NativeInvocationFact;
    use crate::tools::types::{ToolInvocationId, ToolResultContent};
    let authored = "AUTHORED_TOOL_TEXT_SECRET /tmp/user-visible/example /home/user/project Authorization: Bearer AUTHORED_VALUE";
    let mut history = source_history();
    let MessageBlock::Tool(tool) = &mut history[2] else {
        panic!("Tool fixture")
    };
    tool.result.status = ToolExecutionStatus::Failed {
        error: authored.into(),
    };
    tool.result.content = vec![ToolResultContent::Text(TextBlock {
        text: authored.into(),
    })];
    let canonical_tool = serde_json::to_value(&history[2]).unwrap();
    let (directory, catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &history);
    let store = store_for(&catalog, &session, &conversation);
    let mut node = crate::runtime::workflow::test_instance("archive-test", "candidate");
    node.block.run.conversation_id = conversation.clone();
    let input = crate::runtime::workspace::CandidateReference {
        run: node.block.run.clone(),
        version: 1,
        content: "c".repeat(64),
    };
    // Candidate events require an existing durable run-owned workspace fact.
    let workspace = crate::runtime::workspace::WorkspaceSnapshot {
        borrowed_from: None,
        logical_workspace: directory.path().join("workspaces/candidate"),
        isolation: crate::runtime::workspace::WorkspaceIsolation::GitWorktree(
            crate::runtime::workspace::GitWorktreeSnapshot {
                source_repository_root: directory.path().join("project"),
                repository_relative_workspace: std::path::PathBuf::new(),
                physical_worktree_root: directory.path().join("workspaces/candidate"),
                base_commit: "a".repeat(40),
                branch: "rustx/candidate".into(),
                parent_had_uncommitted_changes: false,
            },
        ),
    };
    store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::workspace::workflow_resource_event_id(
                &node.block.run,
                "owned",
            ),
            sequence: 0,
            conversation_id: conversation.clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: Utc::now(),
            event: RuntimeEvent::WorkflowWorkspaceOwned {
                run_id: node.block.run.clone(),
                workspace,
            },
        })
        .unwrap();
    let mut markers = vec!["ARCHIVE_WORKFLOW_OUTER_DIAGNOSTIC".to_owned()];
    for family in ["NATIVE", "CANDIDATE", "WORKFLOW_FAILED", "TOOL_COMPLETED"] {
        for kind in ["FAILED", "DENIED", "UNKNOWN"] {
            let marker = format!("ARCHIVE_{family}_{kind}_DIAGNOSTIC");
            let status = match kind {
                "FAILED" => ToolExecutionStatus::Failed {
                    error: marker.clone(),
                },
                "DENIED" => ToolExecutionStatus::Denied {
                    reason: marker.clone(),
                },
                "UNKNOWN" => ToolExecutionStatus::OutcomeUnknown {
                    detail: marker.clone(),
                },
                _ => unreachable!(),
            };
            markers.push(marker);
            let event = match family {
                "NATIVE" => RuntimeEvent::NativeToolInvocation {
                    invocation_id: ToolInvocationId::Workflow {
                        node: Box::new(node.clone()),
                    },
                    tool_id: ToolId::new("tool-test"),
                    fact: NativeInvocationFact::Completed { status },
                },
                "CANDIDATE" => RuntimeEvent::WorkflowCandidateInvocation {
                    node: node.clone(),
                    input: input.clone(),
                    result: status,
                    candidate_unchanged: true,
                },
                "WORKFLOW_FAILED" => RuntimeEvent::WorkflowFailed {
                    workflow_id: node.block.definition.workflow_id.clone(),
                    run_id: node.block.run.clone(),
                    diagnostic: "ARCHIVE_WORKFLOW_OUTER_DIAGNOSTIC".into(),
                    status,
                },
                "TOOL_COMPLETED" => RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: ToolCallId::new("source-call"),
                    tool_id: ToolId::new("tool-test"),
                    result: ToolExecutionResult {
                        status,
                        content: vec![ToolResultContent::Text(TextBlock {
                            text: authored.into(),
                        })],
                        duration_ms: 42,
                        exit_code: Some(7),
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    },
                },
                _ => unreachable!(),
            };
            store
                .append_event(RuntimeEventEnvelope {
                    schema_version: EVENT_SCHEMA_VERSION,
                    event_id: EventId::new(format!("{family}-{kind}")),
                    sequence: 0,
                    conversation_id: conversation.clone(),
                    attempt_id: None,
                    turn_id: None,
                    timestamp: Utc::now(),
                    event,
                })
                .unwrap();
        }
    }
    let files = decode(
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .unwrap(),
    )
    .await;
    for bytes in files.values() {
        let text = String::from_utf8_lossy(bytes);
        for marker in &markers {
            assert!(!text.contains(marker), "leaked {marker}");
        }
    }
    let messages = records(&files, &conversation, "messages");
    assert_eq!(
        messages[2], canonical_tool,
        "canonical content and failure feedback remain exact"
    );
    assert_eq!(messages[2]["result"]["content"][0]["text"], authored);
    let mut statuses = Vec::new();
    for envelope in records(&files, &conversation, "journal") {
        let event = &envelope["event"];
        let status = match event["type"].as_str().unwrap() {
            "native_tool_invocation" => &event["fact"]["status"],
            "workflow_candidate_invocation" => {
                assert_eq!(event["node"], serde_json::json!(node));
                assert_eq!(event["input"], serde_json::json!(input));
                assert_eq!(event["candidate_unchanged"], true);
                &event["result"]
            }
            "workflow_failed" => {
                assert_eq!(event["run_id"], serde_json::json!(node.block.run));
                assert!(event.get("diagnostic").is_none());
                &event["status"]
            }
            "tool_execution_completed" => {
                assert_eq!(event["result"]["content"][0]["text"], authored);
                assert_eq!(event["result"]["duration_ms"], 42);
                assert_eq!(event["result"]["exit_code"], 7);
                &event["result"]["status"]
            }
            _ => continue,
        };
        assert_eq!(
            status["diagnostic_unavailable"],
            "executor diagnostic excluded"
        );
        statuses.push(status["type"].as_str().unwrap().to_owned());
    }
    for kind in ["failed", "denied", "outcome_unknown"] {
        assert_eq!(statuses.iter().filter(|s| s.as_str() == kind).count(), 4);
    }
}

#[tokio::test]
async fn archive_managed_output_projects_journal_but_preserves_canonical_tool() {
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::runtime::identity::EventId;
    use crate::tools::types::ManagedOutputContinuation as Continuation;
    use serde_json::json;

    let locator = std::path::PathBuf::from("/home/user/project/tool-output/result.txt");
    let partial_diagnostic = "AUTHORED_OR_CANONICAL_MANAGED_OUTPUT_DETAIL";
    let unavailable_diagnostic = "ARCHIVE_MANAGED_UNAVAILABLE_DIAGNOSTIC";
    let partial = Continuation::Partial {
        locator: locator.clone(),
        diagnostic: partial_diagnostic.into(),
    };
    let mut history = source_history();
    let MessageBlock::Tool(tool) = &mut history[2] else {
        panic!("Tool fixture")
    };
    tool.result.managed_output = Some(partial.clone());
    let original_result = tool.result.clone();
    let canonical_tool = serde_json::to_value(&history[2]).unwrap();
    let (directory, catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &history);
    let store = store_for(&catalog, &session, &conversation);
    let continuations = [
        Continuation::Complete {
            locator: locator.clone(),
        },
        partial,
        Continuation::Unavailable {
            diagnostic: unavailable_diagnostic.into(),
        },
    ];
    for (index, continuation) in continuations.into_iter().enumerate() {
        let mut result = original_result.clone();
        result.managed_output = Some(continuation);
        store
            .append_event(RuntimeEventEnvelope {
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: EventId::new(format!("managed-output-{index}")),
                sequence: 0,
                conversation_id: conversation.clone(),
                attempt_id: None,
                turn_id: None,
                timestamp: Utc::now(),
                event: RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: ToolCallId::new("source-call"),
                    tool_id: ToolId::new("tool-test"),
                    result,
                },
            })
            .unwrap();
    }
    let files = decode(
        SessionArchiveProducer::prepare(directory.path(), &session, &CancellationToken::new())
            .unwrap(),
    )
    .await;
    let messages = records(&files, &conversation, "messages");
    assert_eq!(
        messages[2], canonical_tool,
        "canonical continuation remains exact"
    );
    assert_eq!(
        messages[2]["result"]["managed_output"]["diagnostic"],
        partial_diagnostic
    );
    let journal = records(&files, &conversation, "journal");
    // The Partial diagnostic intentionally survives in canonical history, so
    // exclusion is asserted only against the Journal authority.
    let encoded = serde_json::to_string(&journal).unwrap();
    assert!(!encoded.contains(partial_diagnostic));
    assert!(!encoded.contains(unavailable_diagnostic));
    let projected: Vec<_> = journal
        .iter()
        .filter(|record| record["event"]["type"] == "tool_execution_completed")
        .map(|record| record["event"]["result"]["managed_output"].clone())
        .collect();
    assert_eq!(
        projected,
        vec![
            json!({"type":"complete","locator":locator}),
            json!({"type":"partial","locator":locator,
            "diagnostic_unavailable":"output-storage diagnostic excluded"}),
            json!({"type":"unavailable",
            "diagnostic_unavailable":"output-storage diagnostic excluded"}),
        ]
    );
}
