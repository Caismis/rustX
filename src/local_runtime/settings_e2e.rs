//! Native CFG3 source authoring and complete generation publication contracts.
use super::composition::{LocalConversationCore, LocalRuntimeDependencies};
use super::configuration::settings::{ConfigMutation, SettingsError, SourceMutation, SourceScope};
use super::launch::{HostEnvironment, LaunchRequest};
use std::sync::Arc;

fn fixture() -> (tempfile::TempDir, HostEnvironment, LaunchRequest) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(home.join("rustx")).unwrap();
    std::fs::write(
        home.join("rustx/rustx.toml"),
        r#"
[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "SECRET_SENTINEL"
[models.a]
provider = "local"
id = "wire-a"
protocol = "openai_responses"
context_window = 128000
max_output_tokens = 8192
[models.a.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false
[agent.model]
model = "a"
"#,
    )
    .unwrap();
    let host = HostEnvironment::from_paths(workspace, home).unwrap();
    (root, host, LaunchRequest::default())
}

#[tokio::test]
async fn cfg332_save_is_cas_only_reload_publishes_and_cold_resolution_rereads() {
    let (_root, host, request) = fixture();
    let (manager, input) = request.session_input(&host).unwrap();
    let paths = manager
        .resolve_session(&input)
        .unwrap()
        .admit(crate::credentials::CredentialSnapshot::default)
        .unwrap();
    let core = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let runtime = core.runtime();
    runtime.activate();
    let before = runtime.runtime_resources();
    let loaded = runtime.configuration_view().unwrap();
    let source = manager
        .read_source_settings(&input)
        .unwrap()
        .with_loaded(&loaded);
    assert!(!source.loaded.as_ref().unwrap().pending_reload);
    assert!(
        !serde_json::to_string(&source)
            .unwrap()
            .contains("SECRET_SENTINEL")
    );
    let mutation = SourceMutation::Config {
        scope: SourceScope::User,
        mutation: ConfigMutation::Instructions {
            authored: Some("Saved instructions".into()),
        },
    };
    let saved = manager
        .write_source_settings(&input, &source.user.revision, mutation.clone())
        .unwrap()
        .with_loaded(&loaded);
    assert!(saved.loaded.as_ref().unwrap().pending_reload);
    assert_eq!(saved.loaded.as_ref().unwrap().generation, before.revision());
    assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
    assert!(matches!(
        manager.write_source_settings(&input, &source.user.revision, mutation),
        Err(SettingsError::Conflict { .. })
    ));
    let cold = manager.resolve_session(&input).unwrap();
    assert_eq!(cold.config().agent.instructions, "Saved instructions");
    assert_ne!(
        before.configuration().unwrap().config.agent.instructions,
        "Saved instructions"
    );
    runtime.reload_configuration().await.unwrap();
    let after = runtime.runtime_resources();
    assert_eq!(after.revision().get(), before.revision().get() + 1);
    assert_eq!(
        after.configuration().unwrap().config.agent.instructions,
        "Saved instructions"
    );
    let repaired = manager
        .read_source_settings(&input)
        .unwrap()
        .with_loaded(&runtime.configuration_view().unwrap());
    assert!(!repaired.loaded.unwrap().pending_reload);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn cfg332_complete_candidate_stays_offside_cancellation_keeps_old_and_publication_advances_once()
 {
    let (_root, host, request) = fixture();
    let (manager, input) = request.session_input(&host).unwrap();
    let paths = manager
        .resolve_session(&input)
        .unwrap()
        .admit(crate::credentials::CredentialSnapshot::default)
        .unwrap();
    let core = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let runtime = core.runtime();
    runtime.activate();
    let before = runtime.runtime_resources();
    std::fs::write(
        host.launch_directory.join("rustx.toml"),
        "[agent]\ninstructions = 'Workspace candidate'\n[agent.plugins.todo]\nenabled = true\n",
    )
    .unwrap();
    for (root, name) in [
        (host.home_directory.join("rustx/.agents"), "user-agent"),
        (host.launch_directory.join(".agents"), "workspace-agent"),
    ] {
        std::fs::create_dir_all(root.join("agents")).unwrap();
        std::fs::write(
            root.join("agents").join(format!("{name}.toml")),
            "description = 'candidate'\ninstructions = 'frozen resource'",
        )
        .unwrap();
    }
    let gate = super::agent_resources::test_support::arm(&host.launch_directory);
    let reload_runtime = runtime.clone();
    let reload = tokio::spawn(async move { reload_runtime.reload_configuration().await });
    gate.entered().await;
    assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
    assert!(matches!(
        runtime.reload_configuration().await,
        Err(crate::runtime::RuntimeResourceReloadError::Busy { .. })
    ));
    reload.abort();
    assert!(reload.await.unwrap_err().is_cancelled());
    assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
    drop(gate);
    let gate = super::agent_resources::test_support::arm(&host.launch_directory);
    let reload_runtime = runtime.clone();
    let reload = tokio::spawn(async move { reload_runtime.reload_configuration().await });
    gate.entered().await;
    assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
    gate.release();
    reload.await.unwrap().unwrap();
    let after = runtime.runtime_resources();
    assert_eq!(after.revision().get(), before.revision().get() + 1);
    assert_eq!(
        after.configuration().unwrap().config.agent.instructions,
        "Workspace candidate"
    );
    assert!(
        after
            .configuration()
            .unwrap()
            .config
            .agent
            .extensions
            .todo
            .enabled
    );
    for name in ["user-agent", "workspace-agent"] {
        let name = crate::runtime::subagent::SubagentName::parse(name).unwrap();
        assert!(before.subagents().get(&name).is_none());
        assert!(after.subagents().get(&name).is_some());
    }
    std::fs::write(host.launch_directory.join("rustx.toml"), "[invalid").unwrap();
    assert!(runtime.reload_configuration().await.is_err());
    assert!(Arc::ptr_eq(&after, &runtime.runtime_resources()));
    runtime.shutdown().await.unwrap();
}

#[test]
fn root_identity_and_description_are_independent_cas_units() {
    let (_root, host, request) = fixture();
    let (manager, input) = request.session_input(&host).unwrap();
    let source = manager.read_source_settings(&input).unwrap();
    let identity = SourceMutation::Config {
        scope: SourceScope::Workspace,
        mutation: ConfigMutation::AgentIdentity {
            authored: Some(crate::runtime::identity::AgentId::new("reviewer")),
        },
    };
    let saved = manager
        .write_source_settings(&input, &source.workspace.revision, identity.clone())
        .unwrap();
    assert_ne!(saved.workspace.revision, source.workspace.revision);
    assert!(matches!(
        manager.write_source_settings(&input, &source.workspace.revision, identity),
        Err(SettingsError::Conflict { .. })
    ));
    let saved = manager
        .write_source_settings(
            &input,
            &saved.workspace.revision,
            SourceMutation::Config {
                scope: SourceScope::Workspace,
                mutation: ConfigMutation::Description {
                    authored: Some("Review changes".into()),
                },
            },
        )
        .unwrap();
    let document = saved.workspace.authored.unwrap();
    assert_eq!(document.agent_id.unwrap().as_str(), "reviewer");
    assert_eq!(
        document.agent.unwrap().description.as_deref(),
        Some("Review changes")
    );
    let removed = manager
        .write_source_settings(
            &input,
            &saved.workspace.revision,
            SourceMutation::Config {
                scope: SourceScope::Workspace,
                mutation: ConfigMutation::Description { authored: None },
            },
        )
        .unwrap();
    assert!(
        removed
            .workspace
            .authored
            .unwrap()
            .agent
            .unwrap()
            .description
            .is_none()
    );
}
