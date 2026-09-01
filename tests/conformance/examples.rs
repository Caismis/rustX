//! The committed `examples/local-runtime/` configuration composes and runs
//! through the real provider-emulator boundary.
//!
//! This is composed conformance, not a pure contract check: the example's
//! workflow model is pointed at the emulator, the real
//! `LocalConversationRuntime` composes over it, and the checked-in review
//! Workflow runs parent → child → parent continuation against strict
//! provider-side scripts.

use std::sync::Arc;

use rustx::local_runtime::{
    LocalConversationRuntime, LocalRuntimeDependencies, LocalRuntimePaths, StartupSession,
};
use rustx::message::content::TextBlock;
use rustx::message::types::UserContentBlock;
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime_client::host::EventDelivery;
use rustx::runtime_client::types::RuntimeClientResult;
use rustx::runtime_client::{
    RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientEvent, RuntimeClientOutcome,
};

use crate::common::provider_emulator::ProviderEmulator;

fn examples_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime")
}

fn read_example(name: &str) -> Vec<u8> {
    std::fs::read(examples_root().join(name)).expect("committed example file")
}

fn example_models_for_emulator(emulator: &ProviderEmulator) -> String {
    String::from_utf8(read_example("models.jsonc"))
        .expect("example models are UTF-8")
        .replace("\"example\": {", "\"emulator\": {")
        .replace("\"id\": \"demo-model\"", "\"id\": \"workflow-model\"")
        .replace(
            "https://api.example.invalid/v1",
            &emulator.openai_base_url(),
        )
        .replace("$RUSTX_EXAMPLE_API_KEY", "smoke-test-secret")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn checked_in_review_workflow_runs_through_the_existing_provider_emulator() {
    let Some(emulator) = ProviderEmulator::start("workflow_output").await else {
        return;
    };
    let root = tempfile::tempdir().expect("temporary runtime root");
    let examples = examples_root();
    let workspace = examples.join("workspace");
    let config = String::from_utf8(read_example("rustx.jsonc"))
        .expect("example config is UTF-8")
        .replace("example/demo-model", "emulator/workflow-model");
    std::fs::write(
        root.path().join("models.jsonc"),
        example_models_for_emulator(&emulator),
    )
    .expect("emulator model catalog");
    std::fs::write(root.path().join("rustx.jsonc"), config).expect("emulator runtime config");

    let runtime = LocalConversationRuntime::compose(
        &LocalRuntimePaths {
            models: root.path().join("models.jsonc"),
            config: root.path().join("rustx.jsonc"),
            skill_paths: vec![workspace.join(".agents/skills")],
            no_skills: true,
            no_builtin_tools: false,
            no_tools: false,
            startup_session: StartupSession::Empty,
            session_name: None,
            tools: None,
            exclude_tools: Vec::new(),
            workspace,
            runtime_root: root.path().join("runtime-root"),
        },
        &LocalRuntimeDependencies {
            credentials: Arc::new(MapCredentialEnvironment::new([(
                "RUSTX_EXAMPLE_API_KEY".to_owned(),
                "smoke-test-secret".to_owned(),
            )])),
            child_program: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_rustx"))),
            ..LocalRuntimeDependencies::default()
        },
    )
    .await
    .expect("example runtime composes against the local emulator");
    let (attachment, initialized) = runtime
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let RuntimeClientResult::Initialized { cursor, .. } = initialized else {
        panic!("initialize returns the initial snapshot");
    };
    let (events, _) = runtime
        .host()
        .subscribe_events(attachment.attachment_id(), cursor)
        .expect("subscribe");
    runtime
        .host()
        .submit_inbound(vec![UserContentBlock::Text(TextBlock {
            text: "workflow conformance request".to_owned(),
        })])
        .expect("inbound accepted");

    let stream = events;
    let outcome = loop {
        let delivery = tokio::time::timeout(std::time::Duration::from_secs(30), stream.next())
            .await
            .expect("checked-in Workflow settles");
        match delivery {
            EventDelivery::Event(published) => {
                if let RuntimeClientEvent::AttemptSettled { outcome, .. } = published.event {
                    break outcome;
                }
            }
            other => panic!("Runtime Client event stream ended: {other:?}"),
        }
    };
    let requests = emulator.requests().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "checked-in Workflow outcome: {outcome:?}"
    );
    let snapshot = runtime.host().snapshot().expect("snapshot");
    let snapshot_json = serde_json::to_string(&snapshot).expect("snapshot JSON");
    assert!(snapshot_json.contains("native workflow child committed"));
    assert_eq!(requests.len(), 3, "parent, child, then parent continuation");
    emulator.finish().await;
}
