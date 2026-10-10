//! A real process/store continuation proof. Provider gates are explicit
//! activation frontiers; no elapsed delay establishes lifecycle order.
use super::*;
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
    let first = snapshot(&mut process).await.agents.remove(0);
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
    assert_eq!(report_count(&settled, "CHILD-ANSWER"), 1);

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
    assert!(first_turn.prompt.contains("count the workspace files"));
    assert!(first_turn.response.contains("CHILD-ANSWER"));

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
        .replace("temperature = 0.11", "temperature = 0.91");
    assert!(changed.contains("temperature = 0.91"));
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
    // Appended turns cannot move a previously captured location/cut.
    let Ok(MethodResult::TranscriptWindow { window: located }) = process
        .call(Method::AgentTranscript {
            target: process.target(),
            agent_id: first.agent_id.clone(),
            at: rustx::durable::reading::ConversationWindowAt::Turn {
                id: first_turn.id.clone(),
                cut: first_outline.cut.clone(),
            },
            limit: 1,
        })
        .await
    else {
        panic!("native child turn location");
    };
    assert_eq!(located.cut, first_outline.cut);
    assert_eq!(located.target.as_ref(), Some(&first_turn.id));
    assert_eq!(located.target_cursor, first_turn.cursor.map(Into::into));
    assert_eq!(located.page.entries.len(), 1);
    assert_eq!(
        located.page.entries[0].cursor,
        located.target_cursor.unwrap()
    );
    let mut foreign_turn = first_turn.id.clone();
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
                    id: first_turn.id.clone(),
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
    let mut recovered = reopen_parent(
        root.path(),
        &models,
        SESSION_TOML,
        "continuation-secret",
        &session_id,
    )
    .await;
    let restored = recovered.attach().await;
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
        panic!("recovered resume: {third:?}");
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
