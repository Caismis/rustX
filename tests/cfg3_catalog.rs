//! Contracts at the real CFG3 source resolution and frozen invocation boundaries.
use rustx::local_runtime::launch::{HostEnvironment, LaunchRequest, analyze};
use rustx::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
use rustx::model::invocation::{ModelBindingRegistry, ModelSelection};
use std::path::Path;

const PROVIDER: &str = r#"
[providers.transport]
base_url = "https://user.invalid/v1"
api_key = "$USER_KEY"
"#;
const MODEL: &str = r#"
[models."arbitrary-name"]
provider = "transport"
id = "wire-model"
protocol = "openai_responses"
context_window = 128000
max_output_tokens = 8192
[models."arbitrary-name".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false
"#;
const ROOT: &str = r#"
[agent.model]
model = "arbitrary-name"
"#;

const WORKFLOW: &str = "description: Return a literal\nblock:\n  input: {type: object, properties: {}, additionalProperties: false}\n  output: {type: object, properties: {}, additionalProperties: false}\n  entry: done\n  nodes:\n    done:\n      type: return\n      output: {type: literal, value: {}}\n";

#[test]
fn workflow_shadowing_preserves_the_complete_winning_identity_and_native_origin() {
    use rustx::local_runtime::configuration::settings::SourceScope;
    use rustx::runtime::capability_inspection::ResourceFamily;
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
    assert!(
        analyze(&request, &host).is_err(),
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
    use rustx::local_runtime::configuration::settings::SourceScope;
    use rustx::runtime::capability_inspection::ResourceFamily;
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
    inert.runtime().shutdown().await.unwrap();
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

#[tokio::test]
async fn unused_invalid_resource_collections_warn_without_blocking_unrelated_startup() {
    use rustx::local_runtime::{LocalRuntimeDependencies, LocalSessionClient};
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
        let product = LocalSessionClient::compose(&captured, &LocalRuntimeDependencies::default())
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
        product.runtime().shutdown().await.unwrap();
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
        "[providers.transport]\nbase_url = 'https://workspace.invalid/v1'\n",
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
    use rustx::runtime::agent_profile::{AgentProfile, AgentProfileKind, AgentSkillSelection};
    for (text, selection) in [
        ("skills = 'all'", AgentSkillSelection::All),
        (
            "skills = ['rust']",
            AgentSkillSelection::Exact(vec!["rust".into()]),
        ),
        ("skills = []", AgentSkillSelection::Exact(vec![])),
    ] {
        let document: rustx::local_runtime::config::AgentProfileDocument =
            toml::from_str(text).unwrap();
        for kind in [AgentProfileKind::Root, AgentProfileKind::Named] {
            assert_eq!(
                AgentProfile::from_document(&document, kind, vec![])
                    .unwrap()
                    .skills,
                selection
            );
        }
    }
}

#[test]
fn stale_source_writes_and_external_edits_do_not_overwrite() {
    use rustx::local_runtime::configuration::settings::{
        ConfigMutation, SettingsError, SourceMutation, SourceScope,
    };
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let (owner, input) = request.session_input(&host).unwrap();
    let before = owner.read_source_settings(&input).unwrap();
    let external = format!("{PROVIDER}{MODEL}{ROOT}\n[environment]\nEXTERNAL = 'won'\n");
    std::fs::write(&before.user.path, &external).unwrap();
    let result = owner.write_source_settings(
        &input,
        &before.user.revision,
        SourceMutation::Config {
            scope: SourceScope::User,
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
        ConfigMutation, SettingsError, SourceMutation, SourceScope,
    };
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let (owner, input) = request.session_input(&host).unwrap();
    let revision = owner
        .read_source_settings(&input)
        .unwrap()
        .workspace
        .revision;
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
                            input,
                            revision,
                            SourceMutation::Config {
                                scope: SourceScope::Workspace,
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
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(SettingsError::Conflict { .. })))
                .count(),
            1
        );
    });
}

#[test]
fn provider_secrets_are_not_read_back_and_cannot_be_retained_from_lower_scope() {
    use rustx::local_runtime::configuration::settings::{
        ConfigMutation, CredentialEdit, ProviderWrite, SettingsError, SourceMutation, SourceScope,
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
    let view = owner.read_source_settings(&input).unwrap();
    assert!(
        !serde_json::to_string(&view)
            .unwrap()
            .contains("private-fixture-secret")
    );
    let result = owner.write_source_settings(
        &input,
        &view.workspace.revision,
        SourceMutation::Config {
            scope: SourceScope::Workspace,
            mutation: ConfigMutation::Provider {
                id: "transport".into(),
                authored: Some(ProviderWrite {
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
fn malformed_workspace_skill_reserves_identity_and_selected_invalid_fails() {
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
    let boundary =
        rustx::tools::workspace::Workspace::new(request.workspace.as_ref().unwrap()).unwrap();
    let outcome = rustx::skills::SkillDiscovery::with_config(
        &boundary,
        rustx::skills::SkillDiscoveryConfig {
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
        },
    )
    .discover();
    assert!(
        outcome.packages.is_empty(),
        "invalid Workspace identity cannot fall back"
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
    assert!(
        analyze(&request, &host).is_err(),
        "selected invalid package fails resolution"
    );
}

#[test]
fn skill_prompt_exposes_collection_roots_without_enumerating_absolute_package_paths() {
    use rustx::skills::{AutomaticSkillRoot, SkillCatalogEntry, SkillSource, render_skill_catalog};
    let roots = vec![
        AutomaticSkillRoot {
            source: SkillSource::User,
            root: "/home/example/rustx/.agents/skills".into(),
        },
        AutomaticSkillRoot {
            source: SkillSource::Workspace,
            root: "/work/project/.agents/skills".into(),
        },
    ];
    let location = "/work/project/.agents/skills/review/SKILL.md";
    let prompt = render_skill_catalog(
        &[SkillCatalogEntry {
            name: "review".into(),
            description: "Review code".into(),
            location: location.into(),
        }],
        &roots,
    );
    assert!(prompt.contains("User Skill root (root0): /home/example/rustx/.agents/skills"));
    assert!(prompt.contains("Workspace Skill root (root1): /work/project/.agents/skills"));
    assert!(prompt.contains("<root>root1</root>"));
    assert!(!prompt.contains(location));
    assert!(prompt.contains("not filesystem access"));
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
async fn selected_skill_visibility_is_independent_of_native_tool_selection() {
    use rustx::local_runtime::composition::{LocalConversationCore, LocalRuntimeDependencies};
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(
        root.path(),
        &format!(
            "{}{MODEL}{ROOT}",
            PROVIDER.replace("$USER_KEY", "fixture-key")
        ),
        "[agent]\nskills = ['review']\n[agent.tools]\nbuiltin = []\n",
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
    let prompt = generation.skill_catalog().unwrap();
    assert!(prompt.contains("Review code"));
    assert!(!prompt.contains("PRIVATE_BODY_SENTINEL"));
    assert!(
        prompt.contains(
            host.home_directory
                .join("rustx/.agents/skills")
                .to_str()
                .unwrap()
        )
    );
    assert!(
        prompt.contains(
            host.launch_directory
                .join(".agents/skills")
                .to_str()
                .unwrap()
        )
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn live_generation_changes_only_on_complete_reload_and_preserves_explicit_model() {
    use rustx::local_runtime::composition::{LocalConversationCore, LocalRuntimeDependencies};
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let authored = format!(
        "{}{}{}",
        PROVIDER.replace("$USER_KEY", "test-key"),
        MODEL,
        ROOT
    );
    let (host, request) = sources(root.path(), &authored, "");
    let paths = analyze(&request, &host)
        .unwrap()
        .admit(rustx::credentials::CredentialSnapshot::capture)
        .unwrap();
    let core = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let runtime = core.runtime();
    runtime.activate();
    let before = runtime.runtime_resources();
    let initial_model = runtime.model_view().configured;
    runtime.model_set(initial_model.clone()).unwrap();
    let workspace_file = request.workspace.as_ref().unwrap().join("rustx.toml");
    std::fs::write(
        &workspace_file,
        "[agent.plugins.todo]\nenabled = true\n[environment]\nGENERATION = 'next'\n",
    )
    .unwrap();
    assert!(
        Arc::ptr_eq(&before, &runtime.runtime_resources()),
        "disk edits cannot mutate publication"
    );
    let published = runtime.reload_configuration().await.unwrap();
    let after = runtime.runtime_resources();
    assert_eq!(after.revision(), published.resource_revision);
    assert_eq!(after.revision().get(), before.revision().get() + 1);
    assert!(!Arc::ptr_eq(&before, &after));
    assert!(
        !before
            .configuration()
            .unwrap()
            .config
            .agent
            .extensions
            .todo
            .enabled
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
    assert!(
        after
            .capability()
            .tool_registry()
            .definitions()
            .iter()
            .any(|tool| tool.name == "todo")
    );
    assert_eq!(runtime.model_view().configured, initial_model);
    std::fs::write(&workspace_file, "[broken").unwrap();
    assert!(runtime.reload_configuration().await.is_err());
    assert!(
        Arc::ptr_eq(&after, &runtime.runtime_resources()),
        "failed candidate retains the exact published Arc"
    );
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
    core.runtime().shutdown().await.unwrap();
}

#[tokio::test]
async fn an_uninvoked_named_agent_does_not_connect_its_mcp_source() {
    use rustx::local_runtime::composition::{LocalConversationCore, LocalRuntimeDependencies};
    let root = tempfile::tempdir().unwrap();
    let authored = format!(
        "{}{}{}",
        PROVIDER.replace("$USER_KEY", "test-key"),
        MODEL,
        ROOT
    );
    let (host, request) = sources(root.path(), &authored, "[agent]\nagents = ['researcher']\n");
    let resources = request.workspace.as_ref().unwrap().join(".agents");
    std::fs::create_dir_all(resources.join("agents")).unwrap();
    std::fs::write(
        resources.join("agents/researcher.toml"),
        "description = 'Research'\ninstructions = 'Research the supplied task'\n[tools.sources]\nunused = 'all'\n",
    )
    .unwrap();
    let counter = root.path().join("connect-count");
    let command = format!("printf 'connect\\n' >> '{}'", counter.display());
    let mcp = format!(
        "[mcp_servers.unused]\ncommand = '/bin/sh'\nargs = ['-c', {}]\n",
        toml::Value::String(command)
    );
    std::fs::write(resources.join("mcp.toml"), mcp).unwrap();
    let paths = analyze(&request, &host)
        .unwrap()
        .admit(rustx::credentials::CredentialSnapshot::capture)
        .unwrap();
    let core = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    core.runtime().activate();
    assert!(
        !counter.exists(),
        "completed Root preparation must perform zero child-only connects"
    );
    core.runtime().reload_configuration().await.unwrap();
    assert!(!counter.exists(), "reload does not admit a child");
    core.runtime().shutdown().await.unwrap();
    assert!(
        !counter.exists(),
        "no deferred connect may escape composition ownership"
    );
}

#[tokio::test]
async fn named_profile_is_independent_and_child_inherits_the_frozen_invoking_model() {
    use rustx::local_runtime::composition::{LocalConversationCore, LocalRuntimeDependencies};
    use rustx::runtime::subagent::{AttemptSubagentContext, SubagentName};
    let root = tempfile::tempdir().unwrap();
    let authored = format!(
        "{}{}{}{}",
        PROVIDER.replace("$USER_KEY", "test-key"),
        MODEL,
        MODEL
            .replace("arbitrary-name", "session-choice")
            .replace("wire-model", "session-wire"),
        ROOT
    );
    let (host, mut request) = sources(root.path(), &authored, "[agent]\nagents = ['writer']\n");
    request.model = Some("session-choice".into());
    let agents = request.workspace.as_ref().unwrap().join(".agents/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("writer.toml"), "description = 'Writer'\ninstructions = 'Write requested files'\n[tools]\nbuiltin = ['read']\n[plugins.todo]\nenabled = true\n").unwrap();
    let paths = analyze(&request, &host)
        .unwrap()
        .admit(rustx::credentials::CredentialSnapshot::capture)
        .unwrap();
    assert_eq!(
        paths
            .config()
            .agent
            .model
            .as_ref()
            .unwrap()
            .model
            .to_string(),
        "arbitrary-name"
    );
    let core = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let runtime = core.runtime();
    runtime.activate();
    let resources = runtime.runtime_resources();
    let context = AttemptSubagentContext::new(
        core.capability().clone(),
        rustx::runtime::identity::AttemptId::new("attempt-fixture"),
        resources.clone(),
        runtime.model_view().configured,
        resources.configuration().unwrap().models.clone(),
        rustx::runtime::ApprovalMode::default(),
    );
    let name = SubagentName::parse("writer").unwrap();
    let child = context.resolve(&name, None).unwrap();
    assert_eq!(child.model.primary.model.to_string(), "session-choice");
    assert!(
        child
            .tools
            .iter()
            .any(|tool| tool.definition().name == "read")
    );
    assert!(child.extensions.todo().is_some());
    assert!(resources.root_profile().unwrap().tools.is_empty());
    assert!(
        resources
            .root_profile()
            .unwrap()
            .extensions
            .todo()
            .is_none()
    );
    std::fs::write(
        agents.join("writer.toml"),
        "description = 'Writer'\ninstructions = 'New instructions'\n",
    )
    .unwrap();
    runtime.reload_configuration().await.unwrap();
    let frozen_again = context.resolve(&name, None).unwrap();
    assert_eq!(child, frozen_again);
    assert_eq!(child.generation, resources.revision());
    assert_ne!(child.generation, runtime.runtime_resources().revision());
    runtime.shutdown().await.unwrap();
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
            !core
                .runtime()
                .configuration_view()
                .unwrap()
                .resources
                .resource_diagnostics
                .is_empty()
        );
        core.runtime().shutdown().await.unwrap();
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

#[tokio::test]
async fn resource_revisions_cover_both_complete_roots_without_implicit_publication() {
    use rustx::local_runtime::composition::{LocalConversationCore, LocalRuntimeDependencies};
    use std::sync::Arc;
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
    let (manager, input) = request.session_input(&host).unwrap();
    let paths = analyze(&request, &host)
        .unwrap()
        .admit(rustx::credentials::CredentialSnapshot::capture)
        .unwrap();
    let core = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let runtime = core.runtime();
    runtime.activate();
    for (scope, relative, bytes) in [
        (
            "user",
            "skills/unused/SKILL.md",
            "---\nname: unused\ndescription: Invisible resource\n---\nRead on demand.",
        ),
        ("workspace", "workflows/unused.yaml", "malformed: ["),
        ("user", "tools/unused/tool.py", "# inert package source"),
        (
            "workspace",
            "mcp.toml",
            "[mcp_servers.unused]\ncommand = '/never/run'\n",
        ),
    ] {
        let before = runtime.runtime_resources();
        let root_path = if scope == "user" {
            host.home_directory.join("rustx/.agents")
        } else {
            input.cwd.join(".agents")
        };
        let old_revision = before.configuration().unwrap().source_revisions[&root_path].clone();
        let file = root_path.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
        let authored = manager.read_source_settings(&input).unwrap();
        assert_ne!(authored.resource_revisions[&root_path], old_revision);
        assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
        runtime.reload_configuration().await.unwrap();
        let after = runtime.runtime_resources();
        assert_eq!(after.revision().get(), before.revision().get() + 1);
        assert_eq!(
            after.configuration().unwrap().source_revisions[&root_path],
            authored.resource_revisions[&root_path]
        );
    }
    runtime.shutdown().await.unwrap();
}

#[test]
fn named_agent_save_validates_complete_definition_before_committing() {
    use rustx::local_runtime::configuration::settings::{
        SettingsError, SourceMutation, SourceScope,
    };
    let root = tempfile::tempdir().unwrap();
    let (host, request) = sources(root.path(), &format!("{PROVIDER}{MODEL}{ROOT}"), "");
    let (owner, input) = request.session_input(&host).unwrap();
    let before = owner.read_source_settings(&input).unwrap();
    let name = rustx::runtime::subagent::SubagentName::parse("reviewer").unwrap();
    let mut profile = rustx::local_runtime::config::AgentProfileDocument {
        description: "Review changes".into(),
        ..Default::default()
    };
    let mutation = |authored| SourceMutation::Agent {
        scope: SourceScope::Workspace,
        name: name.clone(),
        authored: Some(authored),
    };
    let path = input.cwd.join(".agents/agents/reviewer.toml");
    assert!(matches!(
        owner.write_source_settings(
            &input,
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
        .write_source_settings(&input, &before.absent_resource_revision, mutation(profile))
        .unwrap();
    let inventory = saved.prospective_resources.unwrap();
    assert!(
        inventory
            .definitions
            .iter()
            .any(|entry| entry.name == "reviewer" && entry.valid)
    );
}
