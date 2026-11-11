//! Contracts at the real CFG3 source resolution and frozen invocation boundaries.
use rustx::local_runtime::launch::{HostEnvironment, LaunchRequest, analyze};
use rustx::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
use rustx::model::invocation::{ModelBindingRegistry, ModelSelection};
use std::path::Path;

const PROVIDER: &str = r#"
[providers.transport]
protocol = "openai_responses"
base_url = "https://user.invalid/v1"
api_key = "$USER_KEY"
"#;
const MODEL: &str = r#"
[models."arbitrary-name"]
provider = "transport"
id = "wire-model"
context_window = 128000
max_output_tokens = 8192
[models."arbitrary-name".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
"#;
const ROOT: &str = r#"
[agent.model]
model = "arbitrary-name"
"#;

#[test]
fn hidden_providers_and_models_stay_resolvable_but_are_absent_from_model_choices() {
    let second = MODEL.replace("arbitrary-name", "second");
    let hidden_model = second.replace("provider = ", "hidden = true\nprovider = ");
    let hidden_provider = PROVIDER.replace("protocol = ", "hidden = true\nprotocol = ");
    for (catalog, visible) in [
        (
            format!("{PROVIDER}{MODEL}{second}"),
            vec!["arbitrary-name", "second"],
        ),
        (
            format!("{PROVIDER}{MODEL}{hidden_model}"),
            vec!["arbitrary-name"],
        ),
        (format!("{hidden_provider}{MODEL}{second}"), vec![]),
    ] {
        let catalog = ModelCatalog::from_toml_slice(catalog.as_bytes()).unwrap();
        let choices: Vec<_> = catalog
            .view()
            .models
            .iter()
            .map(|model| model.model.to_string())
            .collect();
        assert_eq!(choices, visible);
        for id in ["arbitrary-name", "second"] {
            assert!(catalog.model(&ModelRef::parse(id).unwrap()).is_ok());
        }
    }
}

const WORKFLOW: &str = "description: Return a literal\nblock:\n  input: {type: object, properties: {}, additionalProperties: false}\n  edges:\n  - from: workflow_start\n    to: done\n  nodes:\n    workflow_start:\n      type: start\n    done:\n      type: return\n      outputs: []\n";

#[test]
fn workflow_shadowing_preserves_the_complete_winning_identity_and_native_origin() {
    use rustx::runtime::capability_inspection::ResourceFamily;
    use rustx::runtime::resources::SourceScope;
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let user = host
        .home_directory
        .join("rustx/.agents/workflows/helper.yaml");
    let workspace = host.launch_directory.join(".agents/workflows/helper.yaml");
    for path in [&user, &workspace] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    std::fs::write(&user, WORKFLOW).unwrap();
    for (higher, scope, shadowed, valid) in [
        (None, SourceScope::User, None, true),
        (
            Some(WORKFLOW),
            SourceScope::Workspace,
            Some(user.clone()),
            true,
        ),
        (
            Some("malformed: ["),
            SourceScope::Workspace,
            Some(user.clone()),
            false,
        ),
    ] {
        if let Some(bytes) = higher {
            std::fs::write(&workspace, bytes).unwrap();
        }
        let captured = analyze(&request, &host).unwrap();
        let facts = captured.resource_inspection();
        let definition = facts
            .definitions
            .iter()
            .find(|entry| entry.family == ResourceFamily::Workflow && entry.name == "helper")
            .unwrap();
        assert_eq!(definition.location.scope, scope);
        assert_eq!(definition.location.shadowed, shadowed);
        assert_eq!(definition.valid, valid);
        assert_eq!(facts.resource_diagnostics.is_empty(), valid);
    }
    std::fs::write(
        host.launch_directory.join("rustx.toml"),
        "[agent]\nworkflows = ['helper']\n",
    )
    .unwrap();
    // An enabled malformed Workspace Workflow stays off, and never exposes
    // the valid User program it shadows.
    let captured = analyze(&request, &host).unwrap();
    assert!(
        !captured
            .resource_inspection()
            .main
            .as_ref()
            .unwrap()
            .workflows
            .contains(&rustx::runtime::workflow::WorkflowId::parse("helper").unwrap()),
        "a selected malformed Workspace Workflow cannot expose the valid User program"
    );
    std::fs::remove_file(&user).unwrap();
    std::fs::write(&workspace, WORKFLOW).unwrap();
    let captured = analyze(&request, &host).unwrap();
    let entry = captured
        .resource_inspection()
        .definitions
        .iter()
        .find(|entry| entry.family == ResourceFamily::Workflow)
        .unwrap();
    assert_eq!(entry.location.scope, SourceScope::Workspace);
    assert_eq!(entry.location.shadowed, None);
}

#[tokio::test]
async fn python_shadowing_is_inert_and_a_malformed_higher_file_blocks_selected_admission() {
    use rustx::local_runtime::composition::{LocalConversationCore, LocalRuntimeDependencies};
    use rustx::runtime::capability_inspection::ResourceFamily;
    use rustx::runtime::resources::SourceScope;
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(
        root.path(),
        &format!(
            "{}{}{}",
            PROVIDER.replace("$USER_KEY", "test-key"),
            MODEL,
            ROOT
        ),
        "",
    );
    let user = host.home_directory.join("rustx/.agents/tools/helper");
    let workspace = host.launch_directory.join(".agents/tools/helper");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(workspace.parent().unwrap()).unwrap();
    std::fs::write(
        user.join("server.py"),
        "raise AssertionError('discovery must never run Python')\n",
    )
    .unwrap();
    std::fs::write(user.join("requirements.txt"), "").unwrap();
    let captured = analyze(&request, &host).unwrap();
    let entry = captured
        .resource_inspection()
        .definitions
        .iter()
        .find(|entry| entry.family == ResourceFamily::ManagedPython)
        .unwrap();
    assert_eq!(entry.location.scope, SourceScope::User);
    assert!(entry.valid);
    std::fs::write(&workspace, "a file cannot be a Python package").unwrap();
    let captured = analyze(&request, &host).unwrap();
    let entry = captured
        .resource_inspection()
        .definitions
        .iter()
        .find(|entry| entry.family == ResourceFamily::ManagedPython)
        .unwrap();
    assert_eq!(entry.location.scope, SourceScope::Workspace);
    assert_eq!(entry.location.shadowed.as_ref(), Some(&user));
    assert!(!entry.valid);
    let inert = LocalConversationCore::compose(
        &captured
            .admit(rustx::credentials::CredentialSnapshot::default)
            .unwrap(),
        &LocalRuntimeDependencies::default(),
    )
    .await
    .unwrap();
    inert.runtime().activate();
    inert.shutdown().await.unwrap();
    drop(inert);
    std::fs::write(
        host.launch_directory.join("rustx.toml"),
        "[agent.tools.sources]\n'python:helper' = 'all'\n",
    )
    .unwrap();
    let selected = analyze(&request, &host)
        .unwrap()
        .admit(rustx::credentials::CredentialSnapshot::default)
        .unwrap();
    assert!(
        LocalConversationCore::compose(&selected, &LocalRuntimeDependencies::default())
            .await
            .is_err()
    );
    std::fs::remove_file(&workspace).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("server.py"),
        "raise AssertionError('still inert')\n",
    )
    .unwrap();
    std::fs::write(workspace.join("requirements.txt"), "").unwrap();
    std::fs::write(host.launch_directory.join("rustx.toml"), "").unwrap();
    for lower_exists in [true, false] {
        if !lower_exists {
            std::fs::remove_dir_all(&user).unwrap();
        }
        let captured = analyze(&request, &host).unwrap();
        let entry = captured
            .resource_inspection()
            .definitions
            .iter()
            .find(|entry| entry.family == ResourceFamily::ManagedPython)
            .unwrap();
        assert!(entry.valid);
        assert_eq!(entry.location.scope, SourceScope::Workspace);
        assert_eq!(entry.location.shadowed.is_some(), lower_exists);
    }
}
fn sources(root: &Path, user: &str, workspace: &str) -> (HostEnvironment, LaunchRequest) {
    // Compare published path identities against the same canonical fixture root
    // used by native resolution, including macOS temporary-directory aliases.
    let root = root.canonicalize().unwrap();
    let home = root.join("home");
    let cwd = root.join("workspace");
    std::fs::create_dir_all(home.join("rustx")).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(home.join("rustx/rustx.toml"), user).unwrap();
    std::fs::write(cwd.join("rustx.toml"), workspace).unwrap();
    let host = HostEnvironment::from_paths(cwd.clone(), home).unwrap();
    (
        host,
        LaunchRequest {
            workspace: Some(cwd),
            ..Default::default()
        },
    )
}

#[cfg(unix)]
#[test]
fn source_fixture_uses_canonical_bindings_through_a_parent_alias() {
    let root = tempfile::tempdir().unwrap();
    let canonical = root.path().canonicalize().unwrap();
    let alias = canonical.join("alias");
    std::os::unix::fs::symlink(&canonical, &alias).unwrap();
    let (host, request) = sources(&alias, &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    assert_eq!(host.home_directory, canonical.join("home"));
    assert_eq!(host.launch_directory, canonical.join("workspace"));
    let (manager, input) = request.session_input(&host).unwrap();
    assert_eq!(
        manager.runtime_root(),
        host.home_directory.join("rustx/runtime")
    );
    assert!(manager.resolve_session(&input).is_ok());
}

#[tokio::test]
async fn unused_invalid_resource_collections_warn_without_blocking_unrelated_startup() {
    use rustx::local_runtime::{LocalConversationRuntime, LocalRuntimeDependencies};
    for family in ["agents", "workflows", "tools", "skills"] {
        let root = tempfile::tempdir().unwrap();
        let (host, request) = sources(
            root.path(),
            &format!("{}{MODEL}{ROOT}", PROVIDER.replace("$USER_KEY", "fixture")),
            "",
        );
        let agents = host.launch_directory.join(".agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join(family),
            "invalid collection: file instead of directory",
        )
        .unwrap();
        let captured = analyze(&request, &host)
            .unwrap()
            .admit(rustx::credentials::CredentialSnapshot::default)
            .unwrap();
        let product =
            LocalConversationRuntime::compose(&captured, &LocalRuntimeDependencies::default())
                .await
                .unwrap();
        let resources = product.runtime().runtime_resources();
        let inspection = resources.inspection();
        assert!(
            !inspection.resource_diagnostics.is_empty() || !inspection.skill_diagnostics.is_empty(),
            "{family}"
        );
        assert!(
            resources
                .capability()
                .tool_registry()
                .definitions()
                .is_empty()
        );
        product.shutdown().await.unwrap();
    }
}
#[test]
fn names_do_not_determine_provider_or_wire_identity_and_freezing_preserves_both() {
    let catalog = ModelCatalog::from_toml_slice(format!("{PROVIDER}{MODEL}").as_bytes()).unwrap();
    let reference = ModelRef::parse("arbitrary-name").unwrap();
    assert_eq!(
        catalog.model(&reference).unwrap().provider.as_str(),
        "transport"
    );
    let credentials = MapCredentialEnvironment::new([("USER_KEY".into(), "test-secret".into())]);
    let registry = ModelBindingRegistry::new(catalog.resolve(&credentials).unwrap()).unwrap();
    let frozen = registry.freeze(&ModelSelection::of(reference)).unwrap();
    assert_eq!(frozen.binding.provider.as_str(), "transport");
    assert_eq!(frozen.wire_model, "wire-model");
    assert_eq!(
        frozen
            .materialize(&credentials)
            .unwrap()
            .invocation_config()
            .model,
        "wire-model"
    );
}
#[test]
fn workspace_model_replaces_model_without_replacing_provider() {
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(
        root.path(),
        &format!("{PROVIDER}{MODEL}{ROOT}"),
        &MODEL.replace("8192", "4096"),
    );
    let resolved = analyze(&request, &host).unwrap();
    assert_eq!(
        resolved.config().initial_model().model.to_string(),
        "arbitrary-name"
    );
    assert!(matches!(
        resolved.provenance().get("models.arbitrary-name"),
        Some(rustx::local_runtime::configuration::Origin::Workspace { .. })
    ));
    assert!(matches!(
        resolved.provenance().get("providers.transport"),
        Some(rustx::local_runtime::configuration::Origin::User { .. })
    ));
}
#[test]
fn provider_replacement_cannot_borrow_lower_credential() {
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(
        root.path(),
        &format!("{PROVIDER}{MODEL}{ROOT}"),
        "[providers.transport]\nprotocol = \"openai_responses\"\nbase_url = 'https://workspace.invalid/v1'\n",
    );
    assert!(analyze(&request, &host).is_err());
}
#[test]
fn split_files_are_inert_and_config_rebinds_only_user_document() {
    let root = tempfile::tempdir().unwrap();
    let (host, mut request) = sources(root.path(), "invalid old default", "");
    for name in ["settings.toml", "models.toml"] {
        std::fs::write(host.config_directory.join(name), "invalid obsolete bytes").unwrap();
    }
    let alternative = root.path().join("alternate.toml");
    std::fs::write(&alternative, format!("{PROVIDER}{MODEL}{ROOT}")).unwrap();
    request.config = Some(alternative);
    let (manager, input) = request.session_input(&host).unwrap();
    assert_eq!(
        manager.runtime_root(),
        host.home_directory.join("rustx/runtime")
    );
    assert!(manager.resolve_session(&input).is_ok());
}

#[test]
fn root_defaults_grant_no_native_tools_skills_or_plugins() {
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let resolved = analyze(&request, &host).unwrap();
    let agent = &resolved.config().agent;
    assert!(agent.tools.builtin.is_empty());
    assert!(agent.tools.sources.is_empty());
    assert!(agent.skills.is_none());
    assert!(!agent.extensions.todo.enabled);
    assert!(!agent.extensions.goal.enabled);
    assert!(!agent.extensions.agent_status.enabled);
}

#[test]
fn plugin_replacement_is_atomic_per_identity() {
    let root = tempfile::tempdir().unwrap();
    let user = format!(
        "{PROVIDER}{MODEL}{ROOT}\n[agent.plugins.todo]\nenabled = true\n[agent.plugins.agent_status]\nenabled = true\n[agent.plugins.agent_status.time]\nenabled = false\n[agent.plugins.agent_status.background]\nenabled = false\n"
    );
    let workspace = "[agent.plugins.agent_status]\nenabled = true\n";
    let (host, request) = sources(root.path(), &user, workspace);
    let resolved = analyze(&request, &host).unwrap();
    let plugins = &resolved.config().agent.extensions;
    assert!(plugins.todo.enabled, "unmentioned Plugin identity inherits");
    assert!(
        plugins.agent_status.time.enabled,
        "winning complete object uses its domain default"
    );
    assert!(
        plugins.agent_status.background.enabled,
        "lower Plugin fields cannot splice"
    );
}

#[test]
fn native_whitelist_empty_replaces_while_absence_inherits() {
    for (workspace, expected) in [("", vec!["read"]), ("[agent.tools]\nbuiltin = []", vec![])] {
        let root = tempfile::tempdir().unwrap();
        let (host, request) = sources(
            root.path(),
            &format!("{PROVIDER}{MODEL}{ROOT}\n[agent.tools]\nbuiltin = ['read']"),
            workspace,
        );
        let resolved = analyze(&request, &host).unwrap();
        assert_eq!(resolved.config().agent.tools.builtin, expected);
    }
}

#[test]
fn skills_all_exact_and_empty_are_valid_for_both_agent_kinds() {
    use rustx::local_runtime::config::{AgentProfileDocument, NamedAgentProfileDocument};
    use rustx::runtime::agent_profile::AgentSkillSelection;
    for (text, selection) in [
        ("skills = 'all'", AgentSkillSelection::All),
        (
            "skills = ['rust']",
            AgentSkillSelection::Exact(vec!["rust".into()]),
        ),
        ("skills = []", AgentSkillSelection::Exact(vec![])),
    ] {
        let root: AgentProfileDocument = toml::from_str(text).unwrap();
        let named: NamedAgentProfileDocument = toml::from_str(text).unwrap();
        assert_eq!(root.skills.as_ref(), Some(&selection));
        assert_eq!(named.skills, Some(selection));
    }
}

#[test]
fn stale_source_writes_and_external_edits_do_not_overwrite() {
    use rustx::local_runtime::configuration::settings::{
        ConfigMutation, SettingsError, SourceMutation,
    };
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let (owner, input) = request.session_input(&host).unwrap();
    let before = owner
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap();
    let external = format!("{PROVIDER}{MODEL}{ROOT}\n[environment]\nEXTERNAL = 'won'\n");
    std::fs::write(&before.user.path, &external).unwrap();
    let result = owner.write_source_settings(
        &USER_TARGET,
        &before.user.revision,
        SourceMutation::Config {
            mutation: ConfigMutation::Environment {
                name: "EXTERNAL".into(),
                authored: Some("lost".into()),
            },
        },
    );
    assert!(matches!(result, Err(SettingsError::Conflict { .. })));
    assert_eq!(
        std::fs::read_to_string(&before.user.path).unwrap(),
        external
    );
}

#[test]
fn competing_native_writers_have_exactly_one_commit_winner() {
    use rustx::local_runtime::configuration::settings::{
        ConfigMutation, SettingsError, SourceMutation,
    };
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let (owner, input) = request.session_input(&host).unwrap();
    let revision = owner
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap()
        .workspace
        .as_ref()
        .unwrap()
        .revision
        .clone();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|threads| {
        let workers: Vec<_> = ["first", "second"]
            .into_iter()
            .map(|value| {
                threads.spawn({
                    let owner = &owner;
                    let input = &input;
                    let revision = &revision;
                    let barrier = &barrier;
                    move || {
                        barrier.wait();
                        owner.write_source_settings(
                            &rustx_target(&input.cwd),
                            revision,
                            SourceMutation::Config {
                                mutation: ConfigMutation::Environment {
                                    name: "WINNER".into(),
                                    authored: Some(value.into()),
                                },
                            },
                        )
                    }
                })
            })
            .collect();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        // Both outcomes, and whether a fresh capture succeeds, so a failure
        // says which writer step failed rather than only a count.
        let outcomes = results
            .iter()
            .map(|result| result.as_ref().map(|_| ()))
            .collect::<Vec<_>>();
        let reread = owner
            .read_source_settings(&rustx_target(&input.cwd))
            .map(|_| ());
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "{outcomes:?}; a fresh read: {reread:?}"
        );
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(SettingsError::Conflict { .. })))
                .count(),
            1,
            "{outcomes:?}; a fresh read: {reread:?}"
        );
    });
}

#[test]
fn removing_a_provider_cascades_only_within_the_revision_fenced_source() {
    use rustx::local_runtime::configuration::settings::{
        ConfigMutation, SettingsError, SourceMutation,
    };
    let root = tempfile::tempdir().unwrap();
    let other = MODEL
        .replace("arbitrary-name", "other-model")
        .replace("transport", "other");
    let other_provider = PROVIDER.replace("transport", "other");
    let local = MODEL.replace("arbitrary-name", "workspace-alias");
    let (host, request) = sources(
        root.path(),
        &format!("{PROVIDER}{MODEL}{other_provider}{other}{ROOT}"),
        &format!("{PROVIDER}{local}"),
    );
    let (owner, input) = request.session_input(&host).unwrap();
    let target = rustx_target(&input.cwd);
    let before = owner.read_source_settings(&target).unwrap();
    let mutation = |mutation| SourceMutation::Config { mutation };
    owner
        .write_source_settings(
            &target,
            &before.workspace.as_ref().unwrap().revision,
            mutation(ConfigMutation::RestoreProvider {
                id: "transport".into(),
            }),
        )
        .unwrap();
    let restored = owner.read_source_settings(&target).unwrap();
    let workspace = restored
        .workspace
        .as_ref()
        .unwrap()
        .authored
        .as_ref()
        .unwrap();
    assert!(
        workspace
            .providers
            .as_ref()
            .is_none_or(|providers| !providers.contains_key("transport"))
    );
    assert!(
        workspace
            .models
            .as_ref()
            .unwrap()
            .contains_key("workspace-alias")
    );
    assert!(analyze(&request, &host).is_ok());

    let delete = || {
        mutation(ConfigMutation::Provider {
            id: "transport".into(),
            authored: None,
        })
    };
    assert!(matches!(
        owner.write_source_settings(&USER_TARGET, "stale", delete()),
        Err(SettingsError::Conflict { .. })
    ));
    assert_eq!(
        owner
            .read_source_settings(&USER_TARGET)
            .unwrap()
            .user
            .revision,
        before.user.revision
    );
    owner
        .write_source_settings(&USER_TARGET, &before.user.revision, delete())
        .unwrap();
    let after = owner.read_source_settings(&target).unwrap();
    let user = after.user.authored.as_ref().unwrap();
    assert_eq!(
        user.providers.as_ref().unwrap().keys().collect::<Vec<_>>(),
        ["other"]
    );
    assert_eq!(
        user.models.as_ref().unwrap().keys().collect::<Vec<_>>(),
        ["other-model"]
    );
    assert_eq!(
        after.workspace.as_ref().unwrap().revision,
        restored.workspace.as_ref().unwrap().revision
    );
}

#[test]
fn provider_secrets_are_not_read_back_and_cannot_be_retained_from_lower_scope() {
    use rustx::local_runtime::configuration::settings::{
        ConfigMutation, CredentialEdit, ProviderWrite, SettingsError, SourceMutation,
    };
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(
        root.path(),
        &format!(
            "{}{MODEL}{ROOT}",
            PROVIDER.replace("$USER_KEY", "private-fixture-secret")
        ),
        "",
    );
    let (owner, input) = request.session_input(&host).unwrap();
    let view = owner
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap();
    assert!(
        !serde_json::to_string(&view)
            .unwrap()
            .contains("private-fixture-secret")
    );
    let result = owner.write_source_settings(
        &rustx_target(&input.cwd),
        &view.workspace.as_ref().unwrap().revision,
        SourceMutation::Config {
            mutation: ConfigMutation::Provider {
                id: "transport".into(),
                authored: Some(ProviderWrite {
                    display_name: None,
                    hidden: false,
                    icon: None,
                    protocol: rustx::model::ModelProtocol::OpenAiResponses,
                    base_url: "https://higher.invalid/v1".into(),
                    credential: CredentialEdit::Retain,
                }),
            },
        },
    );
    assert!(matches!(result, Err(SettingsError::Invalid)));
}

#[test]
fn malformed_workspace_agent_shadows_user_without_poisoning_unused_startup() {
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let user = host.home_directory.join("rustx/.agents/agents");
    let workspace = request.workspace.as_ref().unwrap().join(".agents/agents");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        user.join("worker.toml"),
        "description = 'Worker'\ninstructions = 'Perform the assigned work'\n",
    )
    .unwrap();
    std::fs::write(workspace.join("worker.toml"), "description = [\n").unwrap();
    assert!(analyze(&request, &host).is_ok());
    std::fs::write(
        request.workspace.as_ref().unwrap().join("rustx.toml"),
        "[agent]\nagents = ['worker']\n",
    )
    .unwrap();
    assert!(
        analyze(&request, &host).is_err(),
        "invalid higher definition must not fall back to User"
    );
}

#[test]
fn session_persistence_refuses_obsolete_effective_configuration_fields() {
    use rustx::local_runtime::session::SessionPersistentState;
    let state = SessionPersistentState {
        cwd: "/workspace".into(),
        model: None,
    };
    let mut value = serde_json::to_value(state).unwrap();
    assert_eq!(value, serde_json::json!({"cwd":"/workspace"}));
    for key in [
        "config",
        "tools",
        "exclude_tools",
        "skill_paths",
        "no_direct_tools",
        "no_builtin_tools",
        "no_automatic_skills",
    ] {
        value[key] = serde_json::json!([]);
        assert!(
            serde_json::from_value::<SessionPersistentState>(value.clone()).is_err(),
            "{key}"
        );
        value.as_object_mut().unwrap().remove(key);
    }
}

#[test]
fn malformed_workspace_skill_leaves_the_valid_user_definition_available() {
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let user = host.home_directory.join("rustx/.agents/skills/review");
    let workspace = request
        .workspace
        .as_ref()
        .unwrap()
        .join(".agents/skills/review");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        user.join("SKILL.md"),
        "---\nname: review\ndescription: Review code\n---\nRead the code.",
    )
    .unwrap();
    std::fs::write(workspace.join("SKILL.md"), "invalid manifest").unwrap();
    let outcome = rustx::skills::SkillDiscovery::with_config(rustx::skills::SkillDiscoveryConfig {
        automatic: vec![
            rustx::skills::AutomaticSkillRoot {
                source: rustx::skills::SkillSource::User,
                root: user.parent().unwrap().to_owned(),
            },
            rustx::skills::AutomaticSkillRoot {
                source: rustx::skills::SkillSource::Workspace,
                root: workspace.parent().unwrap().to_owned(),
            },
        ],
    })
    .discover();
    assert_eq!(outcome.packages.len(), 1);
    assert_eq!(
        outcome.packages[0].source(),
        rustx::skills::SkillSource::User
    );
    assert!(!outcome.diagnostics.is_empty());
    assert!(
        analyze(&request, &host).is_ok(),
        "unused invalid package does not block startup"
    );
    std::fs::write(
        request.workspace.as_ref().unwrap().join("rustx.toml"),
        "[agent]\nskills = ['review']\n",
    )
    .unwrap();
    assert!(analyze(&request, &host).is_ok());
}

#[test]
fn skill_prompt_exposes_escaped_instruction_paths() {
    use rustx::skills::{SkillCatalogEntry, SkillReadTool, render_skill_catalog};
    let prompt = render_skill_catalog(
        &[SkillCatalogEntry {
            name: "review".into(),
            description: "Review <code> & tests".into(),
            location: "/work/a&b/.agents/skills/team/review/SKILL.md".into(),
        }],
        SkillReadTool::Bash,
    );
    assert!(
        prompt.contains("<location>/work/a&amp;b/.agents/skills/team/review/SKILL.md</location>")
    );
    assert!(prompt.contains("Review &lt;code&gt; &amp; tests"));
    assert!(prompt.contains("Use bash to load"));
    assert!(render_skill_catalog(&[], SkillReadTool::Read).is_empty());
}

#[test]
fn config_and_runtime_root_are_absolute_process_bindings() {
    let root = tempfile::tempdir().unwrap();
    let (host, mut request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    request.config = Some("other.toml".into());
    assert!(request.session_input(&host).is_err());
    request.config = None;
    request.runtime_root = Some("runtime".into());
    assert!(request.session_input(&host).is_err());
}

#[tokio::test]
async fn skill_prompt_requires_an_available_reader() {
    use rustx::local_runtime::composition::{LocalConversationCore, LocalRuntimeDependencies};
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(
        root.path(),
        &format!(
            "{}{MODEL}{ROOT}",
            PROVIDER.replace("$USER_KEY", "fixture-key")
        ),
        "[agent]\n[agent.tools]\nbuiltin = []\n",
    );
    let package = host.launch_directory.join(".agents/skills/review");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("SKILL.md"),
        "---\nname: review\ndescription: Review code\n---\nPRIVATE_BODY_SENTINEL",
    )
    .unwrap();
    let paths = analyze(&request, &host)
        .unwrap()
        .admit(rustx::credentials::CredentialSnapshot::default)
        .unwrap();
    let core = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let runtime = core.runtime();
    runtime.activate();
    let generation = runtime.runtime_resources();
    assert!(generation.capability().tool_registry().is_empty());
    assert_eq!(
        generation.capability().model_skill_entries()[0].name,
        "review"
    );
    assert!(generation.skill_catalog().is_none());
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn headless_composition_allocates_session_owned_uuid_conversation_database() {
    use rustx::local_runtime::composition::{LocalConversationCore, LocalRuntimeDependencies};
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(
        root.path(),
        &format!(
            "{}{}{}",
            PROVIDER.replace("$USER_KEY", "test-key"),
            MODEL,
            ROOT
        ),
        "",
    );
    let paths = analyze(&request, &host)
        .unwrap()
        .admit(rustx::credentials::CredentialSnapshot::capture)
        .unwrap();
    let core = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let sessions = host.home_directory.join("rustx/runtime/sessions");
    assert!(sessions.join("catalog.json").is_file());
    let session = std::fs::read_dir(&sessions)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ses_")
        })
        .unwrap();
    let conversation = core
        .runtime()
        .runtime_resources()
        .capability()
        .conversation_id()
        .clone();
    assert!(
        session
            .join("conversations")
            .join(conversation.as_str())
            .join("conversation.sqlite")
            .is_file()
    );
    core.runtime().activate();
    core.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_workspace_mcp_reserves_shadow_and_unused_definition_warns() {
    use rustx::local_runtime::composition::{LocalConversationCore, LocalRuntimeDependencies};
    for malformed in [
        "[mcp_servers.service]\ncommand = 9\n",
        "[mcp_servers.service\n",
    ] {
        let root = tempfile::tempdir().unwrap();
        let (host, request) = sources(
            root.path(),
            &format!(
                "{}{}{}",
                PROVIDER.replace("$USER_KEY", "test-key"),
                MODEL,
                ROOT
            ),
            "",
        );
        let user = host.home_directory.join("rustx/.agents");
        let workspace = request.workspace.as_ref().unwrap().join(".agents");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            user.join("mcp.toml"),
            "[mcp_servers.service]\ncommand = '/definitely-not-launched'\n",
        )
        .unwrap();
        std::fs::write(workspace.join("mcp.toml"), malformed).unwrap();
        let unused = analyze(&request, &host).unwrap();
        assert!(
            unused.config().mcp_servers.is_empty(),
            "Workspace invalid identity cannot expose User binding"
        );
        let paths = unused
            .admit(rustx::credentials::CredentialSnapshot::capture)
            .unwrap();
        let core = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
            .await
            .unwrap();
        core.runtime().activate();
        assert!(
            !rustx::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                core.runtime()
            )
            .unwrap()
            .resources
            .resource_diagnostics
            .is_empty()
        );
        core.shutdown().await.unwrap();
        std::fs::write(
            request.workspace.as_ref().unwrap().join("rustx.toml"),
            "[agent.tools.sources]\nservice = 'all'\n",
        )
        .unwrap();
        assert!(
            analyze(&request, &host).is_err(),
            "selected invalid MCP resource fails resolution"
        );
    }
}

#[test]
fn named_agent_save_validates_complete_definition_before_committing() {
    use rustx::local_runtime::configuration::settings::{SettingsError, SourceMutation};
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let (owner, input) = request.session_input(&host).unwrap();
    let before = owner
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap();
    let name = rustx::runtime::subagent::SubagentName::parse("reviewer").unwrap();
    let mut profile = rustx::local_runtime::config::NamedAgentProfileDocument {
        description: "Review changes".into(),
        instructions: "x".repeat(64 * 1024 + 1),
        ..Default::default()
    };
    let mutation = |authored| SourceMutation::Agent {
        name: name.clone(),
        authored: Some(authored),
    };
    let path = input.cwd.join(".agents/agents/reviewer.toml");
    assert!(matches!(
        owner.write_source_settings(
            &rustx_target(&input.cwd),
            &before.absent_resource_revision,
            mutation(profile.clone())
        ),
        Err(SettingsError::Invalid)
    ));
    assert!(
        !path.exists(),
        "invalid profile never becomes authored authority"
    );
    profile.instructions = "Read the change and report concrete findings.".into();
    let saved = owner
        .write_source_settings(
            &rustx_target(&input.cwd),
            &before.absent_resource_revision,
            mutation(profile),
        )
        .unwrap();
    assert!(!saved.is_empty(), "bounded committed revision receipt");
    let saved = owner
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap();
    let inventory = saved.prospective_resources.unwrap();
    assert!(
        inventory
            .definitions
            .iter()
            .any(|entry| entry.name == "reviewer" && entry.valid)
    );
}

#[test]
fn agent_permission_projection_uses_native_resolution_and_source_cas() {
    use rustx::local_runtime::configuration::settings::{
        ConfigMutation, SettingsError, SourceMutation,
    };
    use rustx::runtime::ApprovalMode;
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(
        root.path(),
        &format!("approval_mode = \"full_access\"\n{PROVIDER}{MODEL}{ROOT}"),
        "approval_mode = \"plan\"\n",
    );
    let (owner, input) = request.session_input(&host).unwrap();
    let before = owner
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap();
    assert_eq!(before.prospective_approval_mode, Some(ApprovalMode::Plan));
    let mutation = SourceMutation::Config {
        mutation: ConfigMutation::Approval {
            authored: Some(ApprovalMode::FullAccess),
        },
    };
    let committed_revision = owner
        .write_source_settings(
            &rustx_target(&input.cwd),
            &before.workspace.as_ref().unwrap().revision,
            mutation.clone(),
        )
        .unwrap();
    assert!(!committed_revision.is_empty());
    assert_eq!(
        owner
            .read_source_settings(&rustx_target(&input.cwd))
            .unwrap()
            .prospective_approval_mode,
        Some(ApprovalMode::FullAccess)
    );
    assert!(matches!(
        owner.write_source_settings(
            &rustx_target(&input.cwd),
            &before.workspace.as_ref().unwrap().revision,
            mutation
        ),
        Err(SettingsError::Conflict { .. })
    ));
    std::fs::write(
        &owner
            .read_source_settings(&rustx_target(&input.cwd))
            .unwrap()
            .workspace
            .as_ref()
            .unwrap()
            .path,
        "approval_mode = 'unsupported'",
    )
    .unwrap();
    assert_eq!(
        owner
            .read_source_settings(&rustx_target(&input.cwd))
            .unwrap()
            .prospective_approval_mode,
        None
    );
}

#[test]
fn named_agent_optional_text_survives_independent_complete_profile_edits() {
    use rustx::local_runtime::configuration::settings::SourceMutation;
    use rustx::runtime::subagent::SubagentName;
    for text in ["", "description = 'Review'\n", "instructions = 'Inspect'\n"] {
        let root = tempfile::tempdir().unwrap();
        let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
        let (owner, input) = request.session_input(&host).unwrap();
        let path = input.cwd.join(".agents/agents/optional.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let before = owner
            .read_source_settings(&rustx_target(&input.cwd))
            .unwrap();
        let definition_is_valid =
            |source: &rustx::local_runtime::configuration::settings::SourceSettings| {
                source
                    .prospective_resources
                    .as_ref()
                    .unwrap()
                    .definitions
                    .iter()
                    .any(|entry| entry.name == "optional" && entry.valid)
            };
        assert!(definition_is_valid(&before));
        let source = &before
            .agents
            .iter()
            .find(|agent| agent.name.as_str() == "optional")
            .unwrap()
            .source;
        let mut profile = source.authored.clone().unwrap();
        let original_text = (profile.description.clone(), profile.instructions.clone());
        let projected = serde_json::to_value(&profile).unwrap();
        assert_eq!(
            projected.get("description").is_some(),
            !original_text.0.is_empty()
        );
        assert_eq!(
            projected.get("instructions").is_some(),
            !original_text.1.is_empty()
        );
        profile.tools.builtin = vec!["read".into()];
        let saved = owner
            .write_source_settings(
                &rustx_target(&input.cwd),
                &source.revision,
                SourceMutation::Agent {
                    name: SubagentName::parse("optional").unwrap(),
                    authored: Some(profile),
                },
            )
            .unwrap();
        assert!(!saved.is_empty(), "bounded committed receipt");
        let saved = owner
            .read_source_settings(&rustx_target(&input.cwd))
            .unwrap();
        assert!(definition_is_valid(&saved));
        let saved_source = &saved
            .agents
            .iter()
            .find(|agent| agent.name.as_str() == "optional")
            .unwrap()
            .source;
        assert_ne!(saved_source.revision, source.revision);
        let saved_profile = saved_source.authored.as_ref().unwrap();
        assert_eq!(
            (&saved_profile.description, &saved_profile.instructions),
            (&original_text.0, &original_text.1)
        );
        assert_eq!(saved_profile.tools.builtin, ["read"]);
        let saved_text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            saved_text.contains("description ="),
            !original_text.0.is_empty()
        );
        assert_eq!(
            saved_text.contains("instructions ="),
            !original_text.1.is_empty()
        );
    }
}

const USER_TARGET: rustx::local_runtime::configuration::settings::SourceTarget =
    rustx::local_runtime::configuration::settings::SourceTarget::User;
fn rustx_target(
    directory: &std::path::Path,
) -> rustx::local_runtime::configuration::settings::SourceTarget {
    rustx::local_runtime::configuration::settings::SourceTarget::Workspace {
        directory: directory.to_path_buf(),
    }
}

#[test]
fn each_scope_enables_only_the_workflow_files_it_owns() {
    let root = tempfile::tempdir().unwrap();
    let user_config = format!(
        "{PROVIDER}{MODEL}[agent]\nworkflows = ['helper']\n[agent.model]\nmodel = \"arbitrary-name\"\n"
    );
    let (host, request) = sources(root.path(), &user_config, "");
    let user = host
        .home_directory
        .join("rustx/.agents/workflows/helper.yaml");
    let workspace = host.launch_directory.join(".agents/workflows/helper.yaml");
    for path in [&user, &workspace] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, WORKFLOW).unwrap();
    }
    // What the root Agent of a Session in this Workspace may invoke.
    let enabled = || {
        analyze(&request, &host)
            .unwrap()
            .resource_inspection()
            .main
            .as_ref()
            .unwrap()
            .workflows
            .contains(&rustx::runtime::workflow::WorkflowId::parse("helper").unwrap())
    };
    // The Global entry names the Global file; the Workspace file shadowing
    // it is not enabled by it.
    assert!(!enabled());
    std::fs::remove_file(&workspace).unwrap();
    assert!(enabled(), "the Global file is enabled in every Workspace");
    std::fs::write(&workspace, WORKFLOW).unwrap();
    std::fs::write(
        host.launch_directory.join("rustx.toml"),
        "[agent]\nworkflows = ['helper']\n",
    )
    .unwrap();
    assert!(enabled(), "the Workspace enables its own file");
}

#[test]
fn provider_display_metadata_round_trips_without_changing_model_resolution_or_exposing_secrets() {
    use rustx::local_runtime::configuration::settings::{
        ConfigMutation, CredentialEdit, ProviderWrite, SettingsError, SourceMutation,
    };
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let (owner, input) = request.session_input(&host).unwrap();
    let target = rustx_target(&input.cwd);
    for (icon, name) in [
        (Some("deepseek"), Some("模型供应商")),
        (Some("openai"), Some("Another display name")),
        (None, None),
    ] {
        let before = owner.read_source_settings(&target).unwrap();
        owner
            .write_source_settings(
                &target,
                &before.workspace.as_ref().unwrap().revision,
                SourceMutation::Config {
                    mutation: ConfigMutation::Provider {
                        id: "transport".into(),
                        authored: Some(ProviderWrite {
                            display_name: name.map(str::to_owned),
                            hidden: false,
                            icon: icon.map(str::to_owned),
                            protocol: rustx::model::ModelProtocol::OpenAiResponses,
                            base_url: "https://workspace.invalid/v1".into(),
                            credential: CredentialEdit::Literal {
                                value: "private-icon-fixture".into(),
                            },
                        }),
                    },
                },
            )
            .unwrap();
        let after = owner.read_source_settings(&target).unwrap();
        let authored = after.workspace.as_ref().unwrap().authored.as_ref().unwrap();
        let provider = &authored.providers.as_ref().unwrap()["transport"];
        assert_eq!(provider.icon.as_deref(), icon);
        assert_eq!(provider.display_name.as_deref(), name);
        assert!(
            !serde_json::to_string(&after)
                .unwrap()
                .contains("private-icon-fixture")
        );
        assert!(analyze(&request, &host).is_ok());
        let bytes = std::fs::read_to_string(input.cwd.join("rustx.toml")).unwrap();
        assert_eq!(bytes.contains("icon ="), icon.is_some());
        assert_eq!(bytes.contains("display_name ="), name.is_some());
    }
    for (icon, name) in [
        (Some(""), None),
        (Some("https://example.com/icon.svg"), None),
        (Some("../openai"), None),
        (Some("OpenAI"), None),
        (Some("-openai"), None),
        (Some("openai-"), None),
        (Some("a".repeat(65).as_str()), None),
        (None, Some("")),
        (None, Some(" leading space")),
        (None, Some("two\nlines")),
        (None, Some("模".repeat(101).as_str())),
    ] {
        let before = owner.read_source_settings(&target).unwrap();
        let bytes = std::fs::read(input.cwd.join("rustx.toml")).unwrap();
        let result = owner.write_source_settings(
            &target,
            &before.workspace.as_ref().unwrap().revision,
            SourceMutation::Config {
                mutation: ConfigMutation::Provider {
                    id: "transport".into(),
                    authored: Some(ProviderWrite {
                        display_name: name.map(str::to_owned),
                        hidden: false,
                        icon: icon.map(str::to_owned),
                        protocol: rustx::model::ModelProtocol::OpenAiResponses,
                        base_url: "https://workspace.invalid/v1".into(),
                        credential: CredentialEdit::Retain,
                    }),
                },
            },
        );
        assert!(
            matches!(result, Err(SettingsError::Invalid)),
            "{icon:?} {name:?}"
        );
        assert_eq!(std::fs::read(input.cwd.join("rustx.toml")).unwrap(), bytes);
    }
    // Manual source edits obey the same native validation as managed writes.
    std::fs::write(
        input.cwd.join("rustx.toml"),
        format!("{PROVIDER}icon = '../openai'\n"),
    )
    .unwrap();
    let invalid = owner.read_source_settings(&target).unwrap();
    assert!(invalid.workspace.as_ref().unwrap().diagnostic.is_some());
}
