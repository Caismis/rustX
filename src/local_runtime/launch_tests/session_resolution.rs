#[tokio::test]
async fn durable_omission_uses_current_sources_and_removed_explicit_model_fails_closed() {
    let f = super::Fixture::new();
    let (manager, input) = f.request.session_input(&f.host).unwrap();
    let root = tempfile::tempdir().unwrap();
    let catalog =
        crate::local_runtime::session_controller::SessionController::open(root.path()).unwrap();
    let omitted = crate::local_runtime::session::SessionPersistentState::from_input(&input);
    let a = catalog.create_session(omitted).await.unwrap().session;
    let mut explicit = crate::local_runtime::session::SessionPersistentState::from_input(&input);
    explicit.model = Some(crate::model::session::SessionModelConfig::of(
        crate::model::catalog::ModelRef::parse("host/one").unwrap(),
    ));
    let b = catalog.create_session(explicit).await.unwrap().session;
    drop(catalog);
    let catalog =
        crate::local_runtime::session_controller::SessionController::open(root.path()).unwrap();
    let a = catalog.read_settings(&a.id).await.unwrap().1;
    let b = catalog.read_settings(&b.id).await.unwrap().1;
    f.user(serde_json::json!({"agent":{"model":{"model":"host/two"}}}));
    assert_eq!(
        manager
            .resolve_session(&a.input())
            .unwrap()
            .session_model()
            .model
            .to_string(),
        "host/two"
    );
    assert_eq!(
        manager
            .resolve_session(&b.input())
            .unwrap()
            .session_model()
            .model
            .to_string(),
        "host/one"
    );
    let path = f.host.config_directory.join("rustx.toml");
    let bytes = std::fs::read_to_string(&path).unwrap();
    std::fs::write(path, bytes.replace("one", "removed")).unwrap();
    let ((), effects) = crate::local_runtime::static_effects::measure(|| {
        assert!(manager.resolve_session(&b.input()).is_err());
    });
    assert_eq!(effects, [0; 11]);
}
use super::*;
use crate::model::{catalog::ModelRef, session::SessionModelConfig};

#[test]
fn one_manager_isolates_explicit_cwd_and_captured_resources() {
    let f = Fixture::new();
    let (manager, input_a) = f.request.session_input(&f.host).unwrap();
    let b = f.root.path().join("second");
    std::fs::create_dir_all(b.join(".agents/skills/only-b")).unwrap();
    std::fs::write(b.join("rustx.toml"), "[agent.model]\nmodel = 'host/two'\n").unwrap();
    std::fs::write(b.join("AGENTS.md"), "B guidance").unwrap();
    std::fs::write(
        b.join(".agents/skills/only-b/SKILL.md"),
        "---\nname: only-b\ndescription: B resource\n---\nB body\n",
    )
    .unwrap();
    std::fs::write(input_a.cwd.join("AGENTS.md"), "A guidance").unwrap();
    let a = manager.resolve_session(&input_a).unwrap();
    let b = manager
        .resolve_session(&SessionConfigInput::new(b))
        .unwrap();
    assert_eq!(a.session_model().model.to_string(), "host/one");
    assert_eq!(b.session_model().model.to_string(), "host/two");
    assert_eq!(a.project_context_files[0].content, "A guidance");
    assert_eq!(b.project_context_files[0].content, "B guidance");
    assert_eq!(a.skill_provenance(), []);
    assert_eq!(b.skill_provenance().len(), 1);
    assert_ne!(a.workspace, b.workspace);
    assert_eq!(a.runtime_root, b.runtime_root);
    assert_eq!(
        manager.resolve_session(&input_a).unwrap().config(),
        a.config()
    );
}

#[test]
fn fresh_sources_change_only_later_snapshots_and_omission_stays_omission() {
    let f = Fixture::new();
    let (manager, input) = f.request.session_input(&f.host).unwrap();
    let a = manager.resolve_session(&input).unwrap();
    f.user(json!({"agent":{"model":{"model":"host/two", "request_params":"{\"temperature\":0.3}"}, "skills": []}}));
    let b = manager.resolve_session(&input).unwrap();
    assert_eq!(a.session_model().model.to_string(), "host/one");
    assert_eq!(b.session_model().model.to_string(), "host/two");
    assert!(a.input.model.is_none());
    assert!(b.input.model.is_none());
    assert!(input.model.is_none());
    assert_ne!(
        a.skill_sources,
        [] as [crate::skills::source::AutomaticSkillRoot; 0]
    );
    assert_eq!(
        a.skill_sources, b.skill_sources,
        "visibility never relocates collection roots"
    );
    let catalog = f.host.config_directory.join("rustx.toml");
    let bytes = std::fs::read_to_string(&catalog).unwrap();
    std::fs::write(&catalog, bytes.replace("128000", "64000")).unwrap();
    let c = manager.resolve_session(&input).unwrap();
    assert_ne!(format!("{:?}", b.models), format!("{:?}", c.models));
    assert_eq!(b.session_model(), c.session_model());
}

#[test]
fn explicit_session_model_is_whole_state_and_beats_current_defaults() {
    let f = Fixture::new();
    f.user(json!({"agent":{"model":{"model":"host/one", "request_params":"{\"temperature\":0.3}", "max_output_tokens":{"mode":"limit","tokens":1024}}}}));
    let (manager, mut input) = f.request.session_input(&f.host).unwrap();
    let omitted = manager.resolve_session(&input).unwrap();
    assert!(!omitted.session_model().request_params.is_empty());
    let mut selected = SessionModelConfig::of(ModelRef::parse("host/two").unwrap());
    selected.max_output_tokens = Some(2048);
    input.model = Some(selected.clone());
    let explicit = manager.resolve_session(&input).unwrap();
    assert_eq!(explicit.session_model(), &selected);
    assert!(explicit.session_model().request_params.is_empty());
    f.user(
        json!({"agent":{"model":{"model":"host/one", "request_params":"{\"temperature\":0.8}"}}}),
    );
    assert_eq!(
        manager.resolve_session(&input).unwrap().session_model(),
        &selected
    );
    input
        .model
        .as_mut()
        .unwrap()
        .request_params
        .insert("messages".into(), json!([]));
    let (invalid, effects) =
        super::super::static_effects::measure(|| manager.resolve_session(&input));
    assert!(invalid.is_err());
    assert_eq!(effects, [0; 11]);
}

#[test]
fn invalid_sources_and_disabled_external_sources_have_zero_effects() {
    let f = Fixture::new();
    let (manager, input) = f.request.session_input(&f.host).unwrap();
    for source in [
        "secret_unknown = 'secret-value'",
        "invalid = [",
        "[skills]\nsources = ['unknown']",
        "[agent.model]\nmodel = 'host/absent'",
    ] {
        std::fs::write(f.host.config_directory.join("rustx.toml"), source).unwrap();
        let (result, effects) =
            super::super::static_effects::measure(|| manager.resolve_session(&input));
        assert!(result.is_err());
        assert_eq!(effects, [0; 11]);
    }
    f.mcp(
        true,
        json!({"disabled":{"type":"stdio", "command":"/never-execute", "enabled":false}}),
    );
    f.user(json!({"agent":{"model":{"model":"host/one"}}}));
    let (result, effects) =
        super::super::static_effects::measure(|| manager.resolve_session(&input));
    result.unwrap();
    assert_eq!(effects, [0; 11]);

    let prospective = manager.resolve_session(&input).unwrap();
    let admitted = prospective.admit(|| f.credentials.clone()).unwrap();

    assert_eq!(
        admitted.project_context_files,
        [] as [crate::runtime::resources::ProjectContextFile; 0]
    );
}

/// The shared owner never resolves a Session cwd against the process's
/// ambient working directory: a relative cwd is refused before any
/// effect-owning boundary is entered.
#[test]
fn shared_resolver_refuses_relative_cwd_before_any_effect() {
    let f = Fixture::new();
    let (manager, _) = f.request.session_input(&f.host).unwrap();
    let (result, effects) = super::super::static_effects::measure(|| {
        manager.resolve_session(&SessionConfigInput::new(".".into()))
    });
    assert_eq!(
        result.map(|_| ()).unwrap_err().to_string(),
        "Session cwd must be absolute"
    );
    assert_eq!(effects, [0; 11]);
}

/// The CLI adds no configuration semantics of its own: for the same
/// explicit inputs its analysis is exactly the shared Session
/// configuration owner's resolution, and its refusal is the owner's.
#[test]
fn cli_analysis_is_the_shared_resolver_result() {
    let mut f = Fixture::new();
    f.project(json!({"agent": {"model": {"model": "host/two"}}}));
    std::fs::write(
        f.host.launch_directory.join("AGENTS.md"),
        "Project guidance",
    )
    .unwrap();
    f.request.model = Some("host/one".into());
    let (manager, input) = f.request.session_input(&f.host).unwrap();
    let shared = manager.resolve_session(&input).unwrap();
    let cli = analyze(&f.request, &f.host).unwrap();
    assert_eq!(
        shared.session_model(),
        &crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("host/one").unwrap()
        )
    );
    assert_eq!(cli.session_model(), shared.session_model());
    assert_eq!(cli.input.cwd, shared.input.cwd);
    assert_eq!(cli.config, shared.config);
    assert_eq!(cli.provenance, shared.provenance);
    assert_eq!(cli.component_revisions, shared.component_revisions);
    assert_eq!(cli.source_revisions, shared.source_revisions);
    assert_eq!(cli.project_context_files, shared.project_context_files);
    assert_eq!(shared.project_context_files[0].content, "Project guidance");

    f.project(json!({"agent": {"model": {"model": "host/missing"}}}));
    f.request.model = None;
    let (manager, input) = f.request.session_input(&f.host).unwrap();
    let shared = manager.resolve_session(&input).map(|_| ()).unwrap_err();
    let cli = analyze(&f.request, &f.host).map(|_| ()).unwrap_err();
    assert_eq!(cli.to_string(), shared.to_string());
}

/// Source convention, not a behavior proof: the shared configuration
/// owner's source never spells process working-directory or environment
/// mutation calls, nor the CLI-only launch types. Rust cannot restrict
/// imports or calls inside one crate, so this deliberately limited
/// lexical check cannot see an alias; the behavior is proven by the two
/// tests above and by `unsafe_code = "deny"`, which already rejects the
/// edition-2024 `unsafe` environment mutators crate-wide.
#[test]
fn configuration_owner_source_spells_no_ambient_process_state_or_cli_type() {
    let source = include_str!("../configuration.rs");
    for forbidden in [
        "current_dir(",
        "set_current_dir(",
        "set_var(",
        "remove_var(",
        "HostEnvironment",
        "LaunchRequest",
    ] {
        assert!(
            !source.contains(forbidden),
            "shared owner must not contain {forbidden}"
        );
    }
}

#[tokio::test]
async fn captured_guidance_and_skills_survive_source_edits_before_composition() {
    let f = Fixture::new();
    let (manager, input) = f.request.session_input(&f.host).unwrap();
    std::fs::write(input.cwd.join("AGENTS.md"), "Captured guidance").unwrap();
    let skill = input.cwd.join(".agents/skills/frozen");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: frozen\ndescription: Captured Skill\n---\nbody\n",
    )
    .unwrap();
    let a = manager.resolve_session(&input).unwrap();
    std::fs::remove_dir_all(&skill).unwrap();
    std::fs::write(input.cwd.join("AGENTS.md"), "Later guidance").unwrap();
    let b = manager.resolve_session(&input).unwrap();
    assert_eq!(a.project_context_files[0].content, "Captured guidance");
    assert_eq!(b.project_context_files[0].content, "Later guidance");
    let admitted = a.admit(|| f.credentials.clone()).unwrap();
    let product = compose_admitted(&admitted).await.unwrap();
    let resources = product.runtime().runtime_resources();
    assert_eq!(
        resources.project_context_files()[0].content,
        "Captured guidance"
    );
    assert_eq!(resources.inspection().skills.len(), 1);
    assert_eq!(b.skill_provenance(), []);
    drop(product);
    assert_eq!(
        admitted.project_context_files[0].content,
        "Captured guidance"
    );
}
#[test]
fn session_input_and_provenance_redact_provider_native_values() {
    let f = Fixture::new();
    let (manager, mut input) = f.request.session_input(&f.host).unwrap();
    let mut selected = SessionModelConfig::of(ModelRef::parse("host/one").unwrap());
    selected
        .request_params
        .insert("custom".into(), json!("session-secret-value"));
    input.model = Some(selected);
    let prospective = manager.resolve_session(&input).unwrap();
    for text in [
        format!("{input:?}"),
        format!("{prospective:?}"),
        serde_json::to_string(prospective.provenance()).unwrap(),
    ] {
        assert!(!text.contains("session-secret-value"));
    }
}

#[tokio::test]
async fn workspace_agents_md_capture_survives_edit_and_fresh_resolution_refreshes() {
    let f = Fixture::new();
    let file = f.host.launch_directory.join("AGENTS.md");
    std::fs::write(&file, "instruction A").unwrap();
    let (manager, input) = f.request.session_input(&f.host).unwrap();
    let a = manager.resolve_session(&input).unwrap();
    let inspection_a = a.inspection.clone();
    assert_eq!(a.project_context_files[0].content, "instruction A");
    std::fs::write(&file, "instruction B").unwrap();
    let b = manager.resolve_session(&input).unwrap();
    assert_eq!(b.project_context_files[0].content, "instruction B");
    // Removal cannot cause a semantic reread failure at initial composition.
    std::fs::remove_file(&file).unwrap();
    let admitted_a = a.admit(|| f.credentials.clone()).unwrap();
    let controller =
        super::super::session_controller::SessionController::open(&admitted_a.runtime_root)
            .expect("one native catalog owner");
    let product_a = compose_admitted_with_controller(&admitted_a, &controller)
        .await
        .unwrap();
    let resources_a = product_a.runtime().runtime_resources();
    assert_eq!(
        resources_a.project_context_files()[0].content,
        "instruction A"
    );
    assert_eq!(resources_a.inspection(), &inspection_a);
    let old_file = resources_a.project_context_files()[0].clone();
    drop(resources_a);
    product_a.shutdown().await.unwrap();
    drop(product_a);
    let inspection_b = b.inspection.clone();
    let admitted_b = b.admit(|| f.credentials.clone()).unwrap();
    let product_b = compose_admitted_with_controller(&admitted_b, &controller)
        .await
        .unwrap();
    let resources_b = product_b.runtime().runtime_resources();
    assert_eq!(
        resources_b.project_context_files()[0].content,
        "instruction B"
    );
    assert_eq!(resources_b.inspection(), &inspection_b);
    assert_eq!(old_file.content, "instruction A");
}

#[test]
fn whole_session_model_provenance_covers_every_field_and_catalog_default_choice() {
    use crate::model::catalog::ModelProfileId;
    use crate::model::session::AuxiliaryModelPolicy;
    let f = Fixture::new();
    let path = f.host.config_directory.join("rustx.toml");
    let mut catalog: serde_json::Value =
        toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let entry = &mut catalog["models"]["host/one"];
    entry["default_profile"] = json!("off");
    entry["profiles"] = json!({"off":{},"on":{"request_params":"{\"reasoning_effort\":\"high\"}"}});
    std::fs::write(path, toml::to_string_pretty(&catalog).unwrap()).unwrap();
    let (manager, mut input) = f.request.session_input(&f.host).unwrap();
    let mut model = SessionModelConfig::of(ModelRef::parse("host/one").unwrap());
    model.profile = Some(ModelProfileId::parse("on").unwrap());
    model
        .request_params
        .insert("custom".into(), json!("secret-model-parameter"));
    model.max_output_tokens = Some(1024);
    model.auxiliary = AuxiliaryModelPolicy::Explicit {
        model: ModelRef::parse("host/two").unwrap(),
        profile: crate::model::AuxiliaryProfileSelection::ModelDefault,
        request_params: serde_json::Map::default(),
        max_output_tokens: Some(512),
    };
    for selected in [
        model,
        SessionModelConfig::of(ModelRef::parse("host/one").unwrap()),
    ] {
        input.model = Some(selected.clone());
        let prospective = manager.resolve_session(&input).unwrap();
        assert_eq!(prospective.session_model(), &selected);
        assert_eq!(prospective.input.model.as_ref(), Some(&selected));
        assert_eq!(
            prospective.config().initial_model().model.to_string(),
            "host/one"
        );
        assert!(matches!(
            prospective.provenance()["agent.model"],
            Origin::User { .. }
        ));
        for text in [
            format!("{input:?}"),
            format!("{prospective:?}"),
            serde_json::to_string(prospective.provenance()).unwrap(),
        ] {
            assert!(!text.contains("secret-model-parameter"));
        }
    }
}
