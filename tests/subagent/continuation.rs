//! A real process/store continuation proof. Provider gates are explicit
//! activation frontiers; no elapsed delay establishes lifecycle order.
use super::*;
use rustx::durable::JournalStore;
use rustx::runtime::subagent::{AgentState, SubagentState};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_child_resumes_same_identity_history_and_frozen_authority() {
    let first_gate = crate::common::HeaderGate::new();
    let second_gate = crate::common::HeaderGate::new();
    let third_gate = crate::common::HeaderGate::new();
    let first_response = Arc::clone(&first_gate);
    let second_response = Arc::clone(&second_gate);
    let third_response = Arc::clone(&third_gate);
    let server = crate::common::FixtureServer::start_with_body(move |_, _, body| {
        if !body.contains("please delegate") && body.contains("count the workspace files") {
            if body.contains("THIRD-411") {
                let answer = include_str!("../fixtures/m2/openai_chat/subagent_child_answer.sse")
                    .replace("CHILD-ANSWER: three files", "THIRD-ANSWER: recovered Agent");
                crate::common::FixtureReply::body(200, "OK", "text/event-stream", answer)
                    .with_header_gate(Arc::clone(&third_response))
            } else if body.contains("CONTINUE-411") {
                let answer = include_str!("../fixtures/m2/openai_chat/subagent_child_answer.sse")
                    .replace(
                        "CHILD-ANSWER: three files",
                        "SECOND-ANSWER: retained history",
                    );
                crate::common::FixtureReply::body(200, "OK", "text/event-stream", answer)
                    .with_header_gate(Arc::clone(&second_response))
            } else {
                crate::common::sse_fixture("openai_chat", "subagent_child_answer.sse")
                    .with_header_gate(Arc::clone(&first_response))
            }
        } else {
            route(body)
        }
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let models = models_json(&server.url("/v1"));
    let mut process = spawn_parent(root.path(), &models, SESSION_TOML, "continuation-secret").await;
    process.attach().await;
    process.start_turn("please delegate").await;
    tokio::time::timeout(LIVENESS, first_gate.wait_entered())
        .await
        .unwrap();
    let mut admitted = snapshot(&mut process).await;
    // Issue #462: the Root admits `explore`, so the whole Agent control group
    // is active, exactly once each.
    let tools = process.capability_tools().await;
    for control in [
        "subagent",
        "list_agents",
        "send_message",
        "wait_agent",
        "interrupt_agent",
    ] {
        assert_eq!(
            tools.iter().filter(|tool| tool.name == control).count(),
            1,
            "{control}"
        );
    }
    let first = admitted.agents.remove(0);
    assert_eq!(first.state, AgentState::Active);
    first_gate.release();
    let waited = process
        .call(Method::AgentWait {
            target: process.target(),
            agent_id: first.agent_id.clone(),
        })
        .await;
    assert!(
        matches!(waited, Ok(MethodResult::AgentWait { .. })),
        "{waited:?}"
    );
    let settled = snapshot_with_reports(&mut process, &["CHILD-ANSWER"]).await;
    assert_eq!(settled.agents.len(), 1);
    assert_eq!(settled.agents[0].state, AgentState::Inactive);
    assert_eq!(settled.agents[0].activation_state, SubagentState::Succeeded);
    assert_agent_page_matches_snapshot(&mut process, &settled.agents[0]).await;
    assert_eq!(report_count(&settled, "CHILD-ANSWER"), 1);
    let first_trace = cold_trace_evidence(&mut process, &first).await;

    let Ok(MethodResult::ConversationTurns {
        page: first_outline,
    }) = process
        .call(Method::AgentTurns {
            target: process.target(),
            agent_id: first.agent_id.clone(),
            offset: None,
            limit: 1,
        })
        .await
    else {
        panic!("first child outline must be readable");
    };
    assert_eq!(first_outline.total, 1);
    let first_turn = first_outline.turns[0].clone();
    assert_eq!(first_turn.id.conversation_id, first.child_conversation_id);
    assert!(first_turn.id.attempt_id_source.is_none());
    let native_first_turn = rustx::durable::reading::ConversationTurnId {
        conversation_id: first_turn.id.conversation_id.clone(),
        attempt_id: first_turn
            .id
            .attempt_id
            .clone()
            .expect("normal native owner"),
    };
    assert!(first_turn.prompt.contains("count the workspace files"));
    assert!(first_turn.response.contains("CHILD-ANSWER"));
    let frozen_location = child_turn_window(
        &mut process,
        &first.agent_id,
        first_turn.ordinal,
        &first_outline.cut,
    )
    .await;
    assert_eq!(frozen_location.cut, first_outline.cut);
    assert_eq!(frozen_location.target.as_ref(), Some(&first_turn.id));
    assert_eq!(
        frozen_location.target_cursor,
        first_turn.cursor.map(Into::into)
    );
    assert_eq!(frozen_location.page.entries.len(), 1);
    assert_eq!(
        frozen_location.page.entries[0].cursor,
        frozen_location.target_cursor.unwrap()
    );

    // Resume must use the admitted Agent authority, never re-read this
    // changed named definition or current model configuration from disk.
    let definition = root.path().join("workspace/.agents/agents/explore.toml");
    let changed = std::fs::read_to_string(&definition).unwrap().replace(
        EXPLORE_INSTRUCTIONS,
        "MUTATED-411 unauthorized new instructions",
    );
    assert!(changed.contains("MUTATED-411"));
    std::fs::write(definition, changed).unwrap();
    let config = root.path().join("rustx.toml");
    let changed = std::fs::read_to_string(&config)
        .unwrap()
        .replace(r#""temperature":0.11"#, r#""temperature":0.91"#);
    assert!(changed.contains(r#""temperature":0.91"#));
    std::fs::write(config, changed).unwrap();
    let resumed = process
        .call(Method::AgentSendMessage {
            target: process.target(),
            agent_id: first.agent_id.clone(),
            message: "CONTINUE-411: use your earlier answer".into(),
            attachments: Vec::new(),
        })
        .await;
    let Ok(MethodResult::AgentMessage {
        agent_id: accepted_agent,
        activation_id: accepted_activation,
        resumed: accepted_resumed,
    }) = resumed
    else {
        panic!("resume must be admitted: {resumed:?}");
    };
    assert!(accepted_resumed);
    assert_eq!(accepted_agent, first.agent_id);
    assert_ne!(accepted_activation, first.activation_id);
    tokio::time::timeout(LIVENESS, second_gate.wait_entered())
        .await
        .unwrap();
    let running = snapshot(&mut process).await;
    assert_eq!(
        running.agents.len(),
        1,
        "one durable Agent, not one row per activation"
    );
    let second = &running.agents[0];
    assert_eq!(second.agent_id, first.agent_id);
    assert_eq!(second.child_conversation_id, first.child_conversation_id);
    assert_eq!(
        second.current_activation.as_ref(),
        Some(&accepted_activation)
    );
    assert_eq!(second.state, AgentState::Active);
    assert_trace_cut_rejected(&mut process, &first, &first_trace, &first_trace.cut).await;
    assert_eq!(second.definition_digest, first.definition_digest);
    assert_eq!(second.profile_digest, first.profile_digest);
    let bodies = server.request_bodies();
    let child = bodies
        .iter()
        .find(|body| body.contains("CONTINUE-411") && !body.contains("please delegate"))
        .unwrap();
    assert!(
        child.contains("CHILD-ANSWER: three files"),
        "canonical first answer must reach resumed model"
    );
    assert!(child.contains(EXPLORE_INSTRUCTIONS));
    assert!(!child.contains("MUTATED-411"));
    let request: serde_json::Value = serde_json::from_str(child).unwrap();
    assert_eq!(request["temperature"], 0.11);
    second_gate.release();
    let waited = process
        .call(Method::AgentWait {
            target: process.target(),
            agent_id: first.agent_id.clone(),
        })
        .await;
    assert!(
        matches!(waited, Ok(MethodResult::AgentWait { .. })),
        "{waited:?}"
    );
    let final_snapshot =
        snapshot_with_reports(&mut process, &["CHILD-ANSWER", "SECOND-ANSWER"]).await;
    assert_eq!(final_snapshot.agents.len(), 1);
    assert_eq!(final_snapshot.agents[0].state, AgentState::Inactive);
    assert_eq!(final_snapshot.agents[0].activation_id, accepted_activation);
    assert_agent_page_matches_snapshot(&mut process, &final_snapshot.agents[0]).await;
    assert_eq!(report_count(&final_snapshot, "CHILD-ANSWER"), 1);
    assert_eq!(report_count(&final_snapshot, "SECOND-ANSWER"), 1);
    let transcript = process
        .call(Method::AgentTranscript {
            target: process.target(),
            agent_id: first.agent_id.clone(),
            at: rustx::durable::reading::ConversationWindowAt::Latest,
            limit: 64,
        })
        .await;
    let Ok(MethodResult::TranscriptWindow { window }) = transcript else {
        panic!("canonical child transcript: {transcript:?}");
    };
    let history = serde_json::to_string(&window.page).unwrap();
    assert!(history.contains("CHILD-ANSWER: three files"));
    assert!(history.contains("CONTINUE-411"));
    assert!(history.contains("SECOND-ANSWER: retained history"));
    // The directory covers every activation even when only one row is loaded.
    let Ok(MethodResult::ConversationTurns {
        page: newest_outline,
    }) = process
        .call(Method::AgentTurns {
            target: process.target(),
            agent_id: first.agent_id.clone(),
            offset: None,
            limit: 1,
        })
        .await
    else {
        panic!("newest child outline");
    };
    assert_eq!(newest_outline.total, 2);
    assert_eq!(newest_outline.offset, 1);
    assert_eq!(newest_outline.turns[0].ordinal, 2);
    assert!(newest_outline.turns[0].prompt.contains("CONTINUE-411"));
    let Ok(MethodResult::ConversationTurns {
        page: older_outline,
    }) = process
        .call(Method::AgentTurns {
            target: process.target(),
            agent_id: first.agent_id.clone(),
            offset: Some(0),
            limit: 1,
        })
        .await
    else {
        panic!("older child outline");
    };
    assert_eq!(older_outline.total, 2);
    assert_eq!(older_outline.turns[0], first_turn);
    // Native pending adoption advances M, so the old full read cut expires;
    // its immutable original Turn/C location remains exactly the same.
    assert!(older_outline.cut.mutation_revision > first_outline.cut.mutation_revision);
    let expired = process
        .call(Method::AgentTranscript {
            target: process.target(),
            agent_id: first.agent_id.clone(),
            at: rustx::durable::reading::ConversationWindowAt::TurnOrdinal {
                ordinal: first_turn.ordinal,
                cut: first_outline.cut.clone(),
            },
            limit: 1,
        })
        .await;
    assert!(
        matches!(expired, Err(failure) if matches!(failure.error.data, Some(ErrorData::ContentSourceUnavailable)))
    );
    let located = child_turn_window(
        &mut process,
        &first.agent_id,
        first_turn.ordinal,
        &older_outline.cut,
    )
    .await;
    assert_eq!(located.cut, older_outline.cut);
    assert_eq!(located.target.as_ref(), Some(&first_turn.id));
    assert_eq!(located.target_cursor, frozen_location.target_cursor);
    assert_eq!(located.page.entries.len(), 1);
    assert_eq!(
        located.page.entries[0].cursor,
        frozen_location.page.entries[0].cursor
    );
    assert_eq!(
        located.page.entries[0].item,
        frozen_location.page.entries[0].item
    );
    assert_eq!(frozen_location.cut, first_outline.cut);
    let mut foreign_turn = native_first_turn.clone();
    foreign_turn.conversation_id = process.target().conversation_id;
    assert!(
        process
            .call(Method::AgentTranscript {
                target: process.target(),
                agent_id: first.agent_id.clone(),
                at: rustx::durable::reading::ConversationWindowAt::Turn {
                    id: foreign_turn,
                    cut: first_outline.cut.clone()
                },
                limit: 1,
            })
            .await
            .is_err(),
        "a parent turn must never select child content"
    );
    let mut foreign_cut = first_outline.cut.clone();
    foreign_cut.conversation_id = process.target().conversation_id;
    assert!(
        process
            .call(Method::AgentTranscript {
                target: process.target(),
                agent_id: first.agent_id.clone(),
                at: rustx::durable::reading::ConversationWindowAt::Turn {
                    id: native_first_turn.clone(),
                    cut: foreign_cut
                },
                limit: 1,
            })
            .await
            .is_err(),
        "a parent cut cannot authorize child history"
    );
    let session_id = process.session_id.clone();
    let (status, stderr) = process.shutdown().await;
    assert!(status.success(), "{status}: {stderr}");

    // Recovery reconstructs Agent ownership and the latest activation from
    // durable execution facts; resuming still opens the same child history.
    // Issue #462 A11: the reopened Root admits no named Agent, so it composes
    // none of the five model-facing controls, yet the existing durable
    // identity keeps its authorized typed controls.
    let unadmitted = SESSION_TOML.replace(r#"agents = ["explore"]"#, "agents = []");
    assert_ne!(unadmitted, SESSION_TOML);
    let mut recovered = reopen_parent(
        root.path(),
        &models,
        &unadmitted,
        "continuation-secret",
        &session_id,
    )
    .await;
    let restored = recovered.attach().await;
    assert!(
        restored.capabilities.tools.iter().all(|tool| ![
            "subagent",
            "list_agents",
            "send_message",
            "wait_agent",
            "interrupt_agent"
        ]
        .contains(&tool.name.as_str())),
        "{:?}",
        restored.capabilities.tools
    );
    assert_eq!(restored.agents.len(), 1);
    assert_eq!(restored.agents[0].agent_id, first.agent_id);
    assert_eq!(
        restored.agents[0].child_conversation_id,
        first.child_conversation_id
    );
    assert_eq!(restored.agents[0].activation_id, accepted_activation);
    assert_eq!(restored.agents[0].state, AgentState::Inactive);
    let Ok(MethodResult::ConversationTurns {
        page: recovered_outline,
    }) = recovered
        .call(Method::AgentTurns {
            target: recovered.target(),
            agent_id: first.agent_id.clone(),
            offset: Some(0),
            limit: 64,
        })
        .await
    else {
        panic!("recovered child directory");
    };
    assert_eq!(recovered_outline.total, 2);
    assert_eq!(recovered_outline.turns[0], first_turn);
    assert_eq!(recovered_outline.turns[1], newest_outline.turns[0]);
    let third = recovered
        .call(Method::AgentSendMessage {
            target: recovered.target(),
            agent_id: first.agent_id.clone(),
            message: "THIRD-411: continue after recovery".into(),
            attachments: Vec::new(),
        })
        .await;
    let Ok(MethodResult::AgentMessage {
        agent_id: third_agent,
        activation_id: third_activation,
        resumed: third_resumed,
    }) = third
    else {
        let diagnostic = recovered_resume_diagnostic(recovered, root.path(), &first.agent_id).await;
        panic!("recovered resume: {third:?}\n{diagnostic}");
    };
    assert!(third_resumed);
    assert_eq!(third_agent, first.agent_id);
    assert_ne!(third_activation, first.activation_id);
    assert_ne!(third_activation, accepted_activation);
    tokio::time::timeout(LIVENESS, third_gate.wait_entered())
        .await
        .unwrap();
    let bodies = server.request_bodies();
    let child = bodies
        .iter()
        .find(|body| body.contains("THIRD-411") && !body.contains("please delegate"))
        .unwrap();
    assert!(child.contains("CHILD-ANSWER: three files"));
    assert!(child.contains("SECOND-ANSWER: retained history"));
    assert!(child.contains(EXPLORE_INSTRUCTIONS));
    third_gate.release();
    let wait = recovered
        .call(Method::AgentWait {
            target: recovered.target(),
            agent_id: first.agent_id.clone(),
        })
        .await;
    assert!(
        matches!(wait, Ok(MethodResult::AgentWait { .. })),
        "{wait:?}"
    );
    let final_snapshot = snapshot_with_reports(
        &mut recovered,
        &["CHILD-ANSWER", "SECOND-ANSWER", "THIRD-ANSWER"],
    )
    .await;
    assert_eq!(final_snapshot.agents.len(), 1);
    assert_eq!(final_snapshot.agents[0].agent_id, first.agent_id);
    assert_eq!(
        final_snapshot.agents[0].child_conversation_id,
        first.child_conversation_id
    );
    assert_eq!(final_snapshot.agents[0].activation_id, third_activation);
    assert_eq!(final_snapshot.agents[0].state, AgentState::Inactive);
    for marker in ["CHILD-ANSWER", "SECOND-ANSWER", "THIRD-ANSWER"] {
        assert_eq!(report_count(&final_snapshot, marker), 1);
    }
    let (status, stderr) = recovered.shutdown().await;
    assert!(status.success(), "{status}: {stderr}");
}

/// Public per-Agent pages and original-value reads must agree with the
/// authoritative lifecycle already published in the parent snapshot.
async fn assert_agent_page_matches_snapshot(
    process: &mut Parent,
    expected: &rustx::runtime_client::snapshot::RuntimeClientAgent,
) {
    let result = process
        .call(Method::AgentStatus {
            target: process.target(),
            agent_id: expected.agent_id.clone(),
        })
        .await;
    let Ok(MethodResult::DomainPage { page }) = result else {
        panic!("native Agent status page: {result:?}");
    };
    assert_eq!(page.domain, format!("agents.{}", expected.agent_id));
    assert_eq!(page.total, 1);
    assert_eq!(page.items.len(), 1);
    let selected = process
        .call(Method::ConversationCheckpointPage {
            target: process.target(),
            agent_id: None,
            cut: page.read_cut.clone(),
            domain: page.domain.clone(),
            offset: 0,
            limit: 1,
        })
        .await;
    let Ok(MethodResult::CheckpointPage { page: selected }) = selected else {
        panic!("per-Agent page at its native cut: {selected:?}");
    };
    assert_eq!(selected, page);
    let original = process
        .selected_value(None, &page.read_cut, &page.domain, 0, &[])
        .await;
    let original: rustx::runtime_client::snapshot::RuntimeClientAgent =
        rustx::observation::bounded::decode_domain_value(original).unwrap();
    assert_eq!(
        &original, expected,
        "domain reads cannot resurrect stale lifecycle or activity"
    );
}

async fn snapshot(process: &mut Parent) -> rustx::runtime_client::RuntimeClientSnapshot {
    process.snapshot().await
}

fn report_count(snapshot: &rustx::runtime_client::RuntimeClientSnapshot, marker: &str) -> usize {
    snapshot.messages.iter().filter(|message| match message {
        rustx::message::types::MessageBlock::User(user) => {
            matches!(user.source, rustx::message::types::UserSource::Agent { .. })
                && user.content.iter().any(|block| matches!(block,
                    rustx::message::types::UserContentBlock::Text(text) if text.text.contains(marker)))
        }
        _ => false,
    }).count()
}

// Terminal publication admits parent inbound; the parent's Agent Loop commits
// its canonical message at its next legal boundary. Snapshot round trips
// observe that separate owner without assuming settlement means consumption.
async fn snapshot_with_reports(
    process: &mut Parent,
    markers: &[&str],
) -> rustx::runtime_client::RuntimeClientSnapshot {
    tokio::time::timeout(LIVENESS, async {
        loop {
            let value = snapshot(process).await;
            if markers
                .iter()
                .all(|marker| report_count(&value, marker) == 1)
            {
                return value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent commits the admitted final reports")
}

/// Failure-only native evidence: no configuration or provider payload is printed.
async fn recovered_resume_diagnostic(
    mut process: Parent,
    root: &std::path::Path,
    agent_id: &rustx::runtime::identity::AgentId,
) -> String {
    let target = process.target();
    let session = process.session_id.clone();
    let result = process
        .call(Method::SessionCheckpoint {
            target: target.clone(),
            trace_records: Vec::new(),
        })
        .await;
    let Ok(MethodResult::Checkpoint { checkpoint }) = result else {
        let (status, stderr) = process.shutdown().await;
        return format!(
            "failure checkpoint={result:?} shutdown={status} stderr tail={}",
            stderr_tail(&stderr)
        );
    };
    let owner = checkpoint
        .snapshot
        .agents
        .iter()
        .find(|agent| &agent.agent_id == agent_id)
        .map(|agent| {
            (
                agent.activation_id.clone(),
                agent.state,
                agent.activation_state,
            )
        });
    let Some(cut) = checkpoint.read_cut.source_cut else {
        let (status, stderr) = process.shutdown().await;
        return format!(
            "failure checkpoint has no native source cut shutdown={status} stderr tail={}",
            stderr_tail(&stderr)
        );
    };
    assert_eq!(cut.read.conversation_id, target.conversation_id);
    let (status, stderr) = process.shutdown().await;
    let shutdown = format!(
        "parent shutdown={status} stderr tail:\n{}",
        stderr_tail(&stderr)
    );
    eprintln!("{shutdown}");
    // The product owner has physically exited before any cold catalog handle.
    let controller = match rustx::local_runtime::session_controller::SessionController::open(
        &root.join("private"),
    ) {
        Ok(controller) => controller,
        Err(error) => return format!("{shutdown}\ncold diagnostic catalog={error}"),
    };
    let access = match controller.acquire_session(&session, None).await {
        Ok(access) => access,
        Err(error) => return format!("{shutdown}\ncold diagnostic identity={error}"),
    };
    assert_eq!(access.node.conversation_id, target.conversation_id);
    let store = match rustx::durable::SqliteConversationStore::open_existing(
        access.node.conversation_id.clone(),
        &access.database_path,
    ) {
        Ok(store) => store,
        Err(error) => return format!("{shutdown}\ncold diagnostic Journal={error}"),
    };
    let mut after = 0;
    let mut admissions = Vec::new();
    while after < cut.read.journal {
        let page = store.read_events(Some(after), 32).unwrap();
        assert!(!page.events.is_empty());
        for envelope in page.events {
            assert!(envelope.sequence > after);
            after = envelope.sequence;
            if after > cut.read.journal {
                break;
            }
            if let rustx::events::types::RuntimeEvent::AgentActivationAdmission {
                agent_id: original_agent,
                activation_id,
                phase,
                ..
            } = envelope.event
                && &original_agent == agent_id
            {
                admissions.push((envelope.sequence, activation_id, phase));
            }
        }
    }
    let native = format!(
        "native Owner={owner:?} RootCut={:?} admissionJ={admissions:?}",
        cut.read
    );
    format!("{native}\n{shutdown}")
}

fn stderr_tail(stderr: &str) -> &str {
    let mut tail = stderr.len().saturating_sub(4096);
    while !stderr.is_char_boundary(tail) {
        tail += 1;
    }
    &stderr[tail..]
}

async fn child_turn_window(
    process: &mut Parent,
    agent_id: &rustx::runtime::identity::AgentId,
    ordinal: u64,
    cut: &rustx::durable::reading::ConversationReadCut,
) -> rustx::runtime_client::snapshot::ConversationWindow {
    let result = process
        .call(Method::AgentTranscript {
            target: process.target(),
            agent_id: agent_id.clone(),
            at: rustx::durable::reading::ConversationWindowAt::TurnOrdinal {
                ordinal,
                cut: cut.clone(),
            },
            limit: 1,
        })
        .await;
    let Ok(MethodResult::TranscriptWindow { window }) = result else {
        panic!("advertised native Child ordinal at its exact cut: {result:?}");
    };
    window
}

struct ColdTraceEvidence {
    cut: rustx::observation::cut::CheckpointReadCut,
    domain: String,
    item_offset: u64,
    path: Vec<String>,
}

/// The actual inactive Child page owns the exact original terminal selection.
async fn cold_trace_evidence(
    process: &mut Parent,
    agent: &rustx::runtime_client::snapshot::RuntimeClientAgent,
) -> ColdTraceEvidence {
    use rustx::observation::trace::{TraceFieldKind, TraceFieldSource, TraceKind, TraceState};
    let trace = process
        .call(Method::AgentTrace {
            target: process.target(),
            agent_id: agent.agent_id.clone(),
            before: None,
            limit: 32,
            records: Vec::new(),
        })
        .await;
    let Ok(MethodResult::Trace { page }) = trace else {
        panic!("inactive Child's original Trace: {trace:?}");
    };
    let cut = page.read_cut.expect("standalone cold Trace read evidence");
    assert_eq!(cut.conversation_id, agent.child_conversation_id);
    assert_eq!(cut.activation_id.as_ref(), Some(&agent.activation_id));
    assert_eq!(cut.revision, 0);
    assert_eq!(cut.source_cut.as_ref().unwrap().read.journal, cut.through);
    let terminal = page
        .records
        .iter()
        .find(|record| record.kind == TraceKind::Attempt && record.state == TraceState::Completed)
        .expect("genuine Child completion");
    let TraceFieldSource::Value {
        domain,
        item_offset,
        path,
        encoded_bytes,
        ..
    } = terminal
        .fields
        .iter()
        .find(|field| {
            matches!(
                field,
                TraceFieldSource::Value {
                    field: TraceFieldKind::Result,
                    ..
                }
            )
        })
        .expect("original terminal finish reason")
    else {
        unreachable!()
    };
    assert!(*encoded_bytes > 0);
    let evidence = ColdTraceEvidence {
        cut,
        domain: domain.clone(),
        item_offset: *item_offset,
        path: path.clone(),
    };
    let original = process
        .selected_value(
            Some(agent.agent_id.clone()),
            &evidence.cut,
            &evidence.domain,
            evidence.item_offset,
            &evidence.path,
        )
        .await;
    assert_eq!(
        original,
        serde_json::to_value(rustx::model::ModelFinishReason::Stop).unwrap()
    );
    let request = page
        .records
        .iter()
        .find(|record| record.kind == TraceKind::Request)
        .unwrap();
    let detail_domain = format!(
        "trace.records.{}.detail",
        request.id.strip_prefix("trace:").unwrap()
    );
    let detail = process
        .call(Method::ConversationCheckpointPage {
            target: process.target(),
            agent_id: Some(agent.agent_id.clone()),
            cut: evidence.cut.clone(),
            domain: detail_domain.clone(),
            offset: 0,
            limit: 32,
        })
        .await;
    let Ok(MethodResult::CheckpointPage { page: detail }) = detail else {
        panic!("cold Trace selected page at its own cut: {detail:?}");
    };
    assert_eq!(detail.read_cut, evidence.cut);
    assert_eq!(detail.domain, detail_domain);
    assert!(!detail.items.is_empty());
    let mut inconsistent = evidence.cut.clone();
    inconsistent.source_cut.as_mut().unwrap().read.journal += 1;
    assert_trace_cut_rejected(process, agent, &evidence, &inconsistent).await;
    evidence
}

async fn assert_trace_cut_rejected(
    process: &mut Parent,
    agent: &rustx::runtime_client::snapshot::RuntimeClientAgent,
    evidence: &ColdTraceEvidence,
    cut: &rustx::observation::cut::CheckpointReadCut,
) {
    let result = process
        .call(Method::ConversationCheckpointValueRead {
            target: process.target(),
            agent_id: Some(agent.agent_id.clone()),
            cut: cut.clone(),
            domain: evidence.domain.clone(),
            item_offset: evidence.item_offset,
            path: evidence.path.clone(),
            offset: 0,
            max_bytes: 65_536,
        })
        .await;
    assert!(
        matches!(result, Err(failure) if matches!(failure.error.data, Some(ErrorData::ReadCutExpired)))
    );
}
