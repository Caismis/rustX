//! Headless configuration contracts. Watch edges acknowledge native commits;
//! no delay or polling interval is used to establish ordering.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;
use crate::local_runtime::configuration::application::{
    ApplyUnit, ConfigurationApplication, UnitApplication,
};
use crate::local_runtime::configuration::settings::{ConfigMutation, SourceMutation};
mod automatic_commit;
mod available;
mod catalog_agents;
mod catalog_generations;
mod catalog_publication;
mod catalog_recovery;
mod desired_sources;
#[cfg(feature = "mcp-fixture")]
mod mcp_readiness;
mod preparation;
mod session_creation;
mod source_documents;
mod sources;

/// Captures the native execution frontier before admitting or releasing work.
/// The coordinator retains that exact receipt, so no late Notify waiter can
/// lose a completed handoff.
struct SettlementReceiptWait {
    runtime: crate::runtime::ConversationRuntime,
    after: u64,
}

impl SettlementReceiptWait {
    fn capture(runtime: &crate::runtime::ConversationRuntime) -> Self {
        Self {
            runtime: runtime.clone(),
            after: runtime.settlement_ordinal_for_test(),
        }
    }
    async fn wait(&mut self) {
        let receipt = self.runtime.child_attempt_settlement(self.after).await;
        assert_eq!(
            receipt.ordinal,
            self.after + 1,
            "exact next native execution handoff"
        );
        self.after = receipt.ordinal;
    }
}

async fn source_settled(
    f: &Fixture,
    target: &crate::local_runtime::configuration::settings::SourceTarget,
) -> ConfigurationApplication {
    let mut changes = f.manager.configuration_changes();
    loop {
        if let Some(application) = f
            .manager
            .applications
            .lock()
            .view(&target.application_scope())
            && application
                .units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Preparing))
        {
            return application;
        }
        changes.changed().await.unwrap();
    }
}

pub(super) async fn settled(fixture: &Fixture, index: usize) -> ConfigurationApplication {
    let mut changed = fixture.manager.configuration_changes();
    loop {
        if let Some(view) = fixture
            .manager
            .configuration_application(&fixture.sessions[index].id)
            && view
                .units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Preparing))
        {
            return view;
        }
        changed.changed().await.unwrap();
    }
}

/// Waits until the Session's runtime has committed every prepared unit: no
/// unit is still preparing or staged for the runtime's safe boundary.
pub(super) async fn applied(fixture: &Fixture, index: usize) -> ConfigurationApplication {
    let mut changed = fixture.manager.configuration_changes();
    loop {
        if let Some(view) = fixture
            .manager
            .configuration_application(&fixture.sessions[index].id)
            && view
                .units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Preparing | UnitApplication::Ready))
        {
            return view;
        }
        changed.changed().await.unwrap();
    }
}

/// [`applied`] for a Session addressed by identity.
pub(super) async fn session_applied(
    fixture: &Fixture,
    id: &crate::local_runtime::session::SessionId,
) -> ConfigurationApplication {
    let mut changed = fixture.manager.configuration_changes();
    loop {
        if let Some(view) = fixture.manager.configuration_application(id)
            && view
                .units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Preparing | UnitApplication::Ready))
        {
            return view;
        }
        changed.changed().await.unwrap();
    }
}

/// The Session-level configuration change facts the runtime recorded.
pub(super) fn configuration_changes(
    runtime: &crate::runtime::ConversationRuntime,
) -> Vec<crate::runtime::configuration_commit::ConfigurationChange> {
    events(runtime)
        .into_iter()
        .filter_map(|event| match event {
            crate::events::types::RuntimeEvent::ConfigurationApplied { change } => Some(change),
            _ => None,
        })
        .collect()
}

async fn write(fixture: &Fixture, _index: usize, mutation: ConfigMutation) {
    let source = fixture
        .manager
        .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
        .await
        .unwrap();
    fixture
        .manager
        .write_source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            source.user.revision,
            SourceMutation::Config { mutation },
        )
        .await
        .unwrap();
}

async fn session_settled(
    fixture: &Fixture,
    id: &crate::local_runtime::session::SessionId,
) -> ConfigurationApplication {
    let mut changed = fixture.manager.configuration_changes();
    loop {
        if let Some(view) = fixture.manager.configuration_application(id)
            && view
                .units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Preparing))
        {
            return view;
        }
        changed.changed().await.unwrap();
    }
}

/// Author `local/a` with exactly these Profiles (name, request parameters).
fn issue459_profiles(
    authored: &crate::model::authoring::Model,
    default: &str,
    profiles: &[(&str, serde_json::Value)],
) -> ConfigMutation {
    use crate::model::catalog::ModelProfileId;
    let mut model = authored.clone();
    model.request_params = None;
    model.default_profile = Some(ModelProfileId::new(default));
    model.profiles = Some(
        profiles
            .iter()
            .map(|(name, params)| {
                (
                    ModelProfileId::new(*name),
                    crate::model::authoring::Profile {
                        request_params: crate::toml_authoring::AuthoredRequestParams(
                            serde_json::from_value(params.clone()).unwrap(),
                        ),
                        ..Default::default()
                    },
                )
            })
            .collect(),
    );
    ConfigMutation::Model {
        id: "local/a".into(),
        authored: Some(model),
    }
}

async fn issue459_authored_a(fixture: &Fixture) -> crate::model::authoring::Model {
    fixture
        .manager
        .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
        .await
        .unwrap()
        .user
        .authored
        .unwrap()
        .models
        .unwrap()["local/a"]
        .clone()
}

fn issue459_pinned(profile: Option<&str>) -> crate::model::session::SessionModelConfig {
    crate::model::session::SessionModelConfig {
        profile: profile.map(crate::model::catalog::ModelProfileId::new),
        ..crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/a").unwrap(),
        )
    }
}

fn issue459_text(text: &str) -> Vec<crate::message::types::UserContentBlock> {
    vec![crate::message::types::UserContentBlock::Text(
        crate::message::content::TextBlock { text: text.into() },
    )]
}

/// Apply `edit` to the User source document as one source generation and
/// reconcile it.
async fn issue459_edit(fixture: &Fixture, edit: impl FnOnce(&mut toml::Value)) {
    use crate::local_runtime::configuration::settings::SourceTarget;
    let path = fixture
        .manager
        .source_settings(&SourceTarget::User)
        .await
        .unwrap()
        .user
        .path;
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut document);
    std::fs::write(&path, toml::to_string(&document).unwrap()).unwrap();
    fixture
        .manager
        .reconcile_configuration(&SourceTarget::User)
        .await
        .unwrap();
}

fn issue459_set_instructions(document: &mut toml::Value, text: &str) {
    document["agent"]
        .as_table_mut()
        .unwrap()
        .insert("instructions".into(), text.into());
}

/// Remove `local/a`; add `local/c` (wire id `c`, Profile `steady` with
/// distinguishable parameters) and make it the new-Session default.
fn issue459_replace_a_with_c(document: &mut toml::Value) {
    let models = document["models"].as_table_mut().unwrap();
    let mut replacement = models.remove("local/a").unwrap();
    let table = replacement.as_table_mut().unwrap();
    table.remove("request_params");
    table.insert("id".into(), "c".into());
    table.insert("default_profile".into(), "steady".into());
    table.insert(
        "profiles".into(),
        toml::toml! { steady = { request_params = "{\"temperature\":0.25,\"seed\":7}" } }.into(),
    );
    models.insert("local/c".into(), replacement);
    document["agent"]
        .as_table_mut()
        .unwrap()
        .insert("model".into(), toml::toml! { model = "local/c" }.into());
}

/// Commit `text` as the root instructions and wait for the runtime commit.
async fn issue459_apply_instructions(fixture: &Fixture, text: &str) {
    write(
        fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some(text.into()),
        },
    )
    .await;
    applied(fixture, 0).await;
}

fn issue459_model_refs(
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
) -> Vec<String> {
    runtime
        .model_catalog()
        .models
        .iter()
        .map(|model| model.model.to_string())
        .collect()
}

fn issue459_body(fixture: &Fixture, index: usize) -> serde_json::Value {
    serde_json::from_str(&fixture.provider.request_bodies()[index]).unwrap()
}

/// Write the Named Agent `reviewer` and the Workflow `review` whose one Agent
/// node runs it. Root admits neither until a source names them.
fn issue459_reviewer_sources(fixture: &Fixture) {
    issue459_write_reviewer(fixture, "issue459 reviewer", None);
    let workflow = fixture.workspaces[0].join(".agents/workflows/review.yaml");
    std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
    let schema = serde_json::json!({
        "type": "object", "properties": {}, "required": [], "additionalProperties": false
    });
    let program = serde_json::json!({
        "description": "Review",
        "block": {
            "input": schema, "nodes": {"workflow_start":{"type":"start"},
                "agent": {"type": "agent", "profile": "reviewer", "task": [{"type":"literal","value":"Review"}], "output": {"type":"json_schema","schema":schema}},
                "done": {"type": "return", "outputs": []}
            },"edges": [{"from":"workflow_start","to":"agent"},{"from": "agent", "to": "done"}],}
    });
    std::fs::write(&workflow, serde_json::to_string(&program).unwrap()).unwrap();
}

/// Make `review` a Global Workflow, so the User document that
/// `issue459_admit_reviewer` edits enables it.
fn issue459_global_review(fixture: &Fixture) {
    let user = fixture
        .manager
        .configuration
        .resource_root(&crate::local_runtime::configuration::settings::SourceTarget::User)
        .join("workflows");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::rename(
        fixture.workspaces[0].join(".agents/workflows/review.yaml"),
        user.join("review.yaml"),
    )
    .unwrap();
}

/// Author `reviewer` with `instructions`. Without `model` it names none and
/// inherits the invoking Attempt's frozen model; with one it names its own.
fn issue459_write_reviewer(fixture: &Fixture, instructions: &str, model: Option<&str>) {
    let agent = fixture.workspaces[0].join(".agents/agents/reviewer.toml");
    std::fs::create_dir_all(agent.parent().unwrap()).unwrap();
    let model = model.map_or_else(String::new, |model| format!("[model]\nmodel='{model}'\n"));
    std::fs::write(
        &agent,
        format!("description='Review'\ninstructions='{instructions}'\n{model}"),
    )
    .unwrap();
}

/// Admit `reviewer` to Root: by name, and through the `review` Workflow that
/// drives it, when `review` is Global (`issue459_global_review`).
fn issue459_admit_reviewer(document: &mut toml::Value) {
    let agent = document["agent"].as_table_mut().unwrap();
    agent.insert("agents".into(), vec![toml::Value::from("reviewer")].into());
    agent.insert("workflows".into(), vec![toml::Value::from("review")].into());
}

fn issue459_remove_builtin_tool(document: &mut toml::Value, tool: &str) {
    document["agent"]["tools"]["builtin"]
        .as_array_mut()
        .unwrap()
        .retain(|name| name.as_str() != Some(tool));
}

fn issue459_tool_names(body: &serde_json::Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
        .collect()
}
