use super::*;
use crate::local_runtime::launch::{HostEnvironment, LaunchRequest};
use crate::runtime::agent_profile::AgentSkillSelection;

fn fixture() -> (tempfile::TempDir, UserConfigManager, SourceTarget) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let home = root.join("home");
    let workspace = root.join("workspace");
    fs::create_dir_all(home.join("rustx")).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::write(
        home.join("rustx/rustx.toml"),
        r#"
[providers.local]
protocol = "openai_responses"
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_SKILL_TRANSFER_UNUSED"
[models.local]
provider = "local"
id = "local"
context_window = 128000
max_output_tokens = 8192
[models.local.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
[agent.model]
model = "local"
"#,
    )
    .unwrap();
    let host = HostEnvironment::from_paths(workspace.clone(), home).unwrap();
    let request = LaunchRequest {
        workspace: Some(workspace.clone()),
        ..LaunchRequest::default()
    };
    let manager = request.session_input(&host).unwrap().0;
    assert!(
        manager
            .capture_authored_settings(&SourceTarget::User, None)
            .unwrap()
            .0
            .is_some(),
        "the transfer fixture must have a valid authored model catalog"
    );
    (
        temporary,
        manager,
        SourceTarget::Workspace {
            directory: workspace,
        },
    )
}

#[test]
fn skill_copy_rejects_names_already_selected_by_a_named_agent() {
    for from_user in [true, false] {
        for global_profile in [true, false] {
            let (_temporary, manager, workspace) = fixture();
            let user = SourceTarget::User;
            let (source, destination) = if from_user {
                (&user, &workspace)
            } else {
                (&workspace, &user)
            };
            let collection = manager.skill_collection(source);
            let original = collection.join("original");
            fs::create_dir_all(&original).unwrap();
            fs::write(
                original.join("SKILL.md"),
                "---\nname: original\ndescription: Original skill\n---\nInstructions\n",
            )
            .unwrap();
            let package = manager
                .discover_skill_scope(source, &collection)
                .packages
                .pop()
                .unwrap();
            let revision = operation_revision(&package, &collection).unwrap();
            let profile_root = manager
                .resource_root(if global_profile { &user } else { &workspace })
                .join("agents");
            fs::create_dir_all(&profile_root).unwrap();
            let profile = profile_root.join("reviewer.toml");
            fs::write(
                &profile,
                "description = 'Review'\ninstructions = 'Review input'\nskills = ['copied']\n",
            )
            .unwrap();

            let result = manager.transfer_skill(
                source,
                destination,
                TransferOperation::Copy,
                "original",
                "copied",
                &revision,
            );
            assert!(
                matches!(
                    &result,
                    Err(SettingsError::TransferRejected(
                        TransferRejection::NameEnabled
                    ))
                ),
                "from_user={from_user}, global_profile={global_profile}: {result:?}"
            );
            assert!(original.join("SKILL.md").is_file());
            assert!(
                !manager
                    .skill_collection(destination)
                    .join("copied")
                    .exists()
            );

            fs::write(
                &profile,
                "description = 'Review'\ninstructions = 'Review input'\nskills = []\n",
            )
            .unwrap();
            manager
                .transfer_skill(
                    source,
                    destination,
                    TransferOperation::Copy,
                    "original",
                    "copied",
                    &revision,
                )
                .unwrap();
            let copied = manager
                .discover_skill_scope(destination, &manager.skill_collection(destination))
                .packages
                .into_iter()
                .find(|package| package.name() == "copied")
                .unwrap();
            assert!(!copied.enabled_by_default());
            assert!(!AgentSkillSelection::All.selects_package(&copied));
            assert!(
                AgentSkillSelection::Exact(vec!["copied".into()]).selects_package(&copied),
                "explicit enabling still overrides the default"
            );
        }
    }
}
