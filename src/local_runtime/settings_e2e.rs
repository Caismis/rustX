//! Native CFG3 source authoring and complete generation publication contracts.
use super::configuration::settings::{ConfigMutation, SettingsError, SourceMutation};
use super::launch::{HostEnvironment, LaunchRequest};

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

#[test]
fn root_identity_and_description_are_independent_cas_units() {
    let (_root, host, request) = fixture();
    let (manager, input) = request.session_input(&host).unwrap();
    let source = manager
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap();
    let identity = SourceMutation::Config {
        mutation: ConfigMutation::AgentIdentity {
            authored: Some(crate::runtime::identity::AgentId::new("reviewer")),
        },
    };
    let saved = manager
        .write_source_settings(
            &rustx_target(&input.cwd),
            &source.workspace.as_ref().unwrap().revision,
            identity.clone(),
        )
        .unwrap();
    assert_ne!(
        saved.workspace.as_ref().unwrap().revision,
        source.workspace.as_ref().unwrap().revision
    );
    assert!(matches!(
        manager.write_source_settings(
            &rustx_target(&input.cwd),
            &source.workspace.as_ref().unwrap().revision,
            identity
        ),
        Err(SettingsError::Conflict { .. })
    ));
    let saved = manager
        .write_source_settings(
            &rustx_target(&input.cwd),
            &saved.workspace.as_ref().unwrap().revision,
            SourceMutation::Config {
                mutation: ConfigMutation::Description {
                    authored: Some("Review changes".into()),
                },
            },
        )
        .unwrap();
    let document = saved.workspace.as_ref().unwrap().authored.clone().unwrap();
    assert_eq!(document.agent_id.unwrap().as_str(), "reviewer");
    assert_eq!(
        document.agent.unwrap().description.as_deref(),
        Some("Review changes")
    );
    let removed = manager
        .write_source_settings(
            &rustx_target(&input.cwd),
            &saved.workspace.as_ref().unwrap().revision,
            SourceMutation::Config {
                mutation: ConfigMutation::Description { authored: None },
            },
        )
        .unwrap();
    assert!(
        removed
            .workspace
            .as_ref()
            .unwrap()
            .authored
            .as_ref()
            .unwrap()
            .agent
            .as_ref()
            .unwrap()
            .description
            .is_none()
    );
}

#[test]
fn t08_capture_rejects_external_change_between_layers_and_resource_manifest() {
    let (_root, host, request) = fixture();
    let (manager, input) = request.session_input(&host).unwrap();
    let source = manager
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap();
    let path = source.user.path;
    let before = std::fs::read_to_string(&path).unwrap();
    // This hook is exactly after layer capture, before resource resolution and
    // the final manifest check. No thread scheduling or elapsed time is involved.
    let result = manager.capture_application_at_boundary(&input, || {
        std::fs::write(
            &path,
            format!("{before}\n[environment]\nCAPTURE_REVISION='new'\n"),
        )
        .unwrap();
    });
    assert!(matches!(result, Err(diagnostic) if diagnostic.contains("changed during capture")));
    let captured = manager.capture_application(&input).unwrap();
    assert!(captured.context.is_ok(), "a stable rescan succeeds");
}

fn rustx_target(
    directory: &std::path::Path,
) -> crate::local_runtime::configuration::settings::SourceTarget {
    crate::local_runtime::configuration::settings::SourceTarget::Workspace {
        directory: directory.to_path_buf(),
    }
}
