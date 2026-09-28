//! A real process/store continuation proof. Provider gates are explicit
//! activation frontiers; no elapsed delay establishes lifecycle order.
use super::*;
use rustx::runtime::subagent::{AgentState, SubagentState};
use rustx::runtime_client::RequestId;

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
    let mut process = Process::spawn(root.path(), &models, SESSION_TOML, "continuation-secret");
    let initialized = process
        .request(|id| RuntimeClientRequest::Initialize {
            id: RequestId::new(id),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        })
        .await;
    assert!(matches!(
        initialized.result,
        Some(RuntimeClientResult::Initialized { .. })
    ));
    let submitted = process
        .request(|id| RuntimeClientRequest::SubmitInbound {
            id: RequestId::new(id),
            content: vec![rustx::message::types::UserContentBlock::Text(
                rustx::message::content::TextBlock {
                    text: "please delegate".into(),
                },
            )],
        })
        .await;
    assert!(matches!(
        submitted.result,
        Some(RuntimeClientResult::InboundAccepted { .. })
    ));
    tokio::time::timeout(LIVENESS, first_gate.wait_entered())
        .await
        .unwrap();
    let first = snapshot(&mut process).await.agents.remove(0);
    assert_eq!(first.state, AgentState::Active);
    first_gate.release();
    let waited = process
        .request(|id| RuntimeClientRequest::AgentWait {
            id: RequestId::new(id),
            agent_id: first.agent_id.clone(),
        })
        .await;
    assert!(
        matches!(waited.result, Some(RuntimeClientResult::AgentWait { .. })),
        "{waited:?}"
    );
    let settled = snapshot_with_reports(&mut process, &["CHILD-ANSWER"]).await;
    assert_eq!(settled.agents.len(), 1);
    assert_eq!(settled.agents[0].state, AgentState::Inactive);
    assert_eq!(settled.agents[0].activation_state, SubagentState::Succeeded);
    assert_eq!(report_count(&settled, "CHILD-ANSWER"), 1);

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
        .request(|id| RuntimeClientRequest::AgentSendMessage {
            id: RequestId::new(id),
            agent_id: first.agent_id.clone(),
            message: "CONTINUE-411: use your earlier answer".into(),
        })
        .await;
    let Some(RuntimeClientResult::AgentMessage { accepted }) = resumed.result else {
        panic!("resume must be admitted: {resumed:?}");
    };
    assert!(accepted.resumed);
    assert_eq!(accepted.agent_id, first.agent_id);
    assert_ne!(accepted.activation_id, first.activation_id);
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
        Some(&accepted.activation_id)
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
        .request(|id| RuntimeClientRequest::AgentWait {
            id: RequestId::new(id),
            agent_id: first.agent_id.clone(),
        })
        .await;
    assert!(
        matches!(waited.result, Some(RuntimeClientResult::AgentWait { .. })),
        "{waited:?}"
    );
    let final_snapshot =
        snapshot_with_reports(&mut process, &["CHILD-ANSWER", "SECOND-ANSWER"]).await;
    assert_eq!(final_snapshot.agents.len(), 1);
    assert_eq!(final_snapshot.agents[0].state, AgentState::Inactive);
    assert_eq!(
        final_snapshot.agents[0].activation_id,
        accepted.activation_id
    );
    assert_eq!(report_count(&final_snapshot, "CHILD-ANSWER"), 1);
    assert_eq!(report_count(&final_snapshot, "SECOND-ANSWER"), 1);
    let transcript = process
        .request(|id| RuntimeClientRequest::AgentTranscript {
            id: RequestId::new(id),
            agent_id: first.agent_id.clone(),
            before: None,
            limit: 64,
        })
        .await;
    let Some(RuntimeClientResult::TranscriptPage { page }) = transcript.result else {
        panic!("canonical child transcript: {transcript:?}");
    };
    let history = serde_json::to_string(&page).unwrap();
    assert!(history.contains("CHILD-ANSWER: three files"));
    assert!(history.contains("CONTINUE-411"));
    assert!(history.contains("SECOND-ANSWER: retained history"));
    let session = process
        .request(|id| RuntimeClientRequest::SessionGet {
            id: RequestId::new(id),
        })
        .await;
    let Some(RuntimeClientResult::Session { session }) = session.result else {
        panic!("session identity");
    };
    let shut = process
        .request(|id| RuntimeClientRequest::Shutdown {
            id: RequestId::new(id),
        })
        .await;
    assert!(matches!(
        shut.result,
        Some(RuntimeClientResult::ShutdownCompleted)
    ));
    let (status, stderr) = process.close_and_wait().await;
    assert!(status.success(), "{status}: {stderr}");

    // Recovery reconstructs Agent ownership and the latest activation from
    // durable execution facts; resuming still opens the same child history.
    let mut recovered = Process::reopen(
        root.path(),
        &models,
        SESSION_TOML,
        "continuation-secret",
        session.id.as_str(),
    );
    let initialized = recovered
        .request(|id| RuntimeClientRequest::Initialize {
            id: RequestId::new(id),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        })
        .await;
    let Some(RuntimeClientResult::Initialized {
        snapshot: restored, ..
    }) = initialized.result
    else {
        panic!("recovery: {initialized:?}");
    };
    assert_eq!(restored.agents.len(), 1);
    assert_eq!(restored.agents[0].agent_id, first.agent_id);
    assert_eq!(
        restored.agents[0].child_conversation_id,
        first.child_conversation_id
    );
    assert_eq!(restored.agents[0].activation_id, accepted.activation_id);
    assert_eq!(restored.agents[0].state, AgentState::Inactive);
    let third = recovered
        .request(|id| RuntimeClientRequest::AgentSendMessage {
            id: RequestId::new(id),
            agent_id: first.agent_id.clone(),
            message: "THIRD-411: continue after recovery".into(),
        })
        .await;
    let Some(RuntimeClientResult::AgentMessage { accepted: third }) = third.result else {
        panic!("recovered resume: {third:?}");
    };
    assert!(third.resumed);
    assert_eq!(third.agent_id, first.agent_id);
    assert_ne!(third.activation_id, first.activation_id);
    assert_ne!(third.activation_id, accepted.activation_id);
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
        .request(|id| RuntimeClientRequest::AgentWait {
            id: RequestId::new(id),
            agent_id: first.agent_id.clone(),
        })
        .await;
    assert!(
        matches!(wait.result, Some(RuntimeClientResult::AgentWait { .. })),
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
    assert_eq!(final_snapshot.agents[0].activation_id, third.activation_id);
    assert_eq!(final_snapshot.agents[0].state, AgentState::Inactive);
    for marker in ["CHILD-ANSWER", "SECOND-ANSWER", "THIRD-ANSWER"] {
        assert_eq!(report_count(&final_snapshot, marker), 1);
    }
    let shut = recovered
        .request(|id| RuntimeClientRequest::Shutdown {
            id: RequestId::new(id),
        })
        .await;
    assert!(matches!(
        shut.result,
        Some(RuntimeClientResult::ShutdownCompleted)
    ));
    let (status, stderr) = recovered.close_and_wait().await;
    assert!(status.success(), "{status}: {stderr}");
}

async fn snapshot(process: &mut Process) -> rustx::runtime_client::RuntimeClientSnapshot {
    let response = process
        .request(|id| RuntimeClientRequest::SnapshotGet {
            id: RequestId::new(id),
        })
        .await;
    let Some(RuntimeClientResult::Snapshot { snapshot, .. }) = response.result else {
        panic!("snapshot: {response:?}");
    };
    snapshot
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
    process: &mut Process,
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
