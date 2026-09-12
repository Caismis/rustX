//! Issue #144: named attempt-scoped subagent definitions.
//!
//! The suite proves the architecture rather than any particular
//! implementation: catalog admission, resolution against the invoking
//! generation, the authority subset relationships, the prepare/commit
//! boundary, durable identity, and the frozen child specification.
//!
//! Every generation-ordering assertion is decided by an explicit
//! linearization the test drives — a completed `reload_resources` call, or a
//! typed refusal — never by a sleep.

use crate::launch_fixture::LaunchFixture;
use std::sync::Arc;

use rustx::capabilities::{CapabilitySourceState, ToolSourceId};
use rustx::local_runtime::composition::{LocalRuntimeDependencies, LocalSessionProduct};
use rustx::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
use rustx::model::invocation::ModelBindingRegistry;
use rustx::model::session::SessionModelConfig;
use rustx::runtime::RuntimeResourceSnapshot;
use rustx::runtime::subagent::{
    InvokingAgentAuthority, NamedAgentDefinitionDigest, ResolvedSubagentSpec, ResolvedSubagentTool,
    SubagentDomain, SubagentName, SubagentOverrideAuthority, SubagentResolution,
    SubagentResolutionError, SubagentResolver,
};
use rustx::runtime_client::settings::EffectiveNativeAgentExtensions;

const KEY_ENV: &str = "RUSTX_ISSUE144_KEY";
const EXPLORE_AGENTS: &str = ".agents/agents/explore/AGENTS.md";
const EXPLORE_EXTRA: &str = ".agents/agents/explore/EXTRA.md";

const MODELS: &str = r#"[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_ISSUE144_KEY"

[[providers.local.models]]
id = "model-a"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[providers.local.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[providers.local.models.compat]
chat_reasoning_replay = "omit"

[[providers.local.models]]
id = "model-b"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[providers.local.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[providers.local.models.compat]
chat_reasoning_replay = "omit"
"#;

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Some(Arc::new(MapCredentialEnvironment::new([(
            KEY_ENV.to_owned(),
            "test-only-secret".to_owned(),
        )]))),
        ..LocalRuntimeDependencies::default()
    }
}

/// The same model authority the runtime composes, rebuilt for direct
/// resolver calls.
fn model_registry() -> ModelBindingRegistry {
    let catalog = ModelCatalog::from_toml_slice(MODELS.as_bytes()).expect("model catalog");
    let resolved = catalog
        .resolve(dependencies().credentials.as_deref().unwrap())
        .expect("resolved catalog");
    ModelBindingRegistry::new(resolved).expect("binding registry")
}

fn agent(name: &str) -> SubagentName {
    SubagentName::parse(name).expect("canonical name")
}

/// Resolves one named agent with **no** invocation override, through the one
/// shared resolution entry point.
///
/// The helper exists so the historical suites read the way they always did
/// while still going through the single Issue #258 contract: no override, the
/// Main admission domain, the dynamic delegation authority, and a caller that
/// contributes nothing of its own.
fn resolve(
    resources: &RuntimeResourceSnapshot,
    name: &SubagentName,
    attempt_model: &SessionModelConfig,
    models: &ModelBindingRegistry,
) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
    SubagentResolver::resolve(&SubagentResolution {
        resources,
        agent: name,
        attempt_model,
        models,
        domain: SubagentDomain::Main,
        invocation: None,
        authority: SubagentOverrideAuthority::DelegatedByModel,
        invoking: &InvokingAgentAuthority::none(),
    })
}

fn inherited_model() -> SessionModelConfig {
    SessionModelConfig::of(ModelRef::parse("local/model-a").expect("model reference"))
}

fn project_instruction_contents(spec: &ResolvedSubagentSpec) -> Vec<String> {
    spec.project_instructions
        .iter()
        .map(|file| file.content.clone())
        .collect()
}

/// One temporary world: models, workspace, config, and subagent resources.
struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    fn new() -> Self {
        let lab = Self {
            dir: tempfile::tempdir().expect("lab directory"),
        };
        for profile in ["explore", "research", "pinned", "isolated"] {
            std::fs::create_dir_all(lab.workspace().join(format!(".agents/agents/{profile}")))
                .expect("subagent resources");
        }
        std::fs::write(lab.root().join("models.toml"), MODELS).expect("models.toml");
        std::fs::write(
            lab.subagent_file("explore", "instructions.md"),
            "Explore the shared workspace read-only.\n",
        )
        .expect("explore instructions");
        std::fs::write(
            lab.workspace().join("AGENTS.md"),
            "workspace instructions\n",
        )
        .expect("AGENTS.md");
        std::fs::write(
            lab.subagent_file("explore", "AGENTS.md"),
            "explicit agent instructions\n",
        )
        .expect("agent AGENTS.md");
        std::fs::write(
            lab.subagent_file("explore", "EXTRA.md"),
            "second explicit file\n",
        )
        .expect("second agent AGENTS.md");
        lab
    }

    fn root(&self) -> &std::path::Path {
        self.dir.path()
    }

    fn workspace(&self) -> std::path::PathBuf {
        self.root().join("workspace")
    }

    fn subagent_file(&self, profile: &str, file: &str) -> std::path::PathBuf {
        if file == "instructions.md" {
            self.workspace()
                .join(".agents/agents")
                .join(format!("{profile}.toml"))
        } else {
            self.workspace()
                .join(".agents/agents")
                .join(profile)
                .join(file)
        }
    }

    fn write_config(&self, subagents: &serde_json::Value) {
        self.write_config_with_tools(subagents, &["read"]);
    }

    fn write_config_with_tools(&self, subagents: &serde_json::Value, builtin_tools: &[&str]) {
        let mut subagents = subagents.clone();
        let definition_names = subagents
            .get("roles")
            .and_then(serde_json::Value::as_object)
            .map(|definitions| {
                definitions
                    .keys()
                    .map(|name| serde_json::Value::String(name.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(document) = subagents.as_object_mut() {
            document
                .entry("agents".to_owned())
                .or_insert_with(|| serde_json::Value::Array(definition_names.clone()));
            document
                .entry("workflow".to_owned())
                .or_insert_with(|| serde_json::Value::Array(definition_names));
        }
        let root_agents = subagents.as_object_mut().unwrap().remove("agents").unwrap();
        crate::launch_fixture::write_roles(&self.workspace(), &mut subagents);
        let document = serde_json::json!({"schema_version": 8, "agent_id": "agent-issue144", "context": {"reserve_tokens": 0, "keep_recent_tokens": 0}, "subagents": subagents, "agent": {"model": {"model": "local/model-a"}, "tools": {"builtin": builtin_tools}, "skills": ["alpha", "beta"], "agents": root_agents}});
        std::fs::write(
            self.root().join("rustx.toml"),
            toml::to_string_pretty(&document).expect("config document"),
        )
        .expect("rustx.toml");
    }

    fn write_skill(&self, name: &str, description: &str) {
        let directory = self.workspace().join(".agents/skills").join(name);
        std::fs::create_dir_all(&directory).expect("skill directory");
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n{name} body\n"),
        )
        .expect("SKILL.md");
    }

    fn paths(&self) -> LaunchFixture {
        LaunchFixture {
            models: self.root().join("models.toml"),
            config: self.root().join("rustx.toml"),
            skill_paths: Vec::new(),
            no_skills: false,
            no_builtin_tools: false,
            no_tools: false,
            startup_session: rustx::local_runtime::StartupSession::Empty,
            session_name: None,
            tools: None,
            exclude_tools: Vec::new(),
            workspace: self.workspace(),
            runtime_root: self.root().join("runtime"),
        }
    }

    async fn compose(&self) -> LocalSessionProduct {
        LocalSessionProduct::compose(&(self.paths()).resolve(), &dependencies())
            .await
            .expect("the runtime composes")
    }
}

/// A model-visible Tool must have at least one satisfiable invocation in the
/// same frozen resource generation. Owning a subagent runtime is insufficient
/// when that generation admits no named agent.
#[tokio::test]
async fn an_empty_named_agent_catalog_exposes_no_subagent_tool() {
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {},
    }));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let capability = resources.capability();

    assert!(
        capability
            .available_tools()
            .definitions()
            .iter()
            .all(|definition| definition.name != "subagent"),
        "an unsatisfiable intrinsic is absent from availability, not merely inactive"
    );
    assert!(
        capability
            .tool_registry()
            .definitions()
            .iter()
            .all(|definition| definition.name != "subagent"),
        "the frozen model-facing registry cannot advertise an invocation that always fails"
    );
}

/// A capability reload owns the subagent registration per resource
/// generation: R2 may remove an empty catalog without mutating an already
/// frozen R1 lease.
#[tokio::test]
async fn reloading_non_empty_catalog_to_empty_removes_only_the_current_subagent_tool() {
    let lab = Lab::new();
    lab.write_config(&explore(&["read"]));
    let product = lab.compose().await;
    let r1 = product.runtime().runtime_resources();
    let r1_definition = r1
        .capability()
        .tool_registry()
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "subagent")
        .expect("R1 registers subagent for the non-empty catalog");
    assert!(
        !r1.capability()
            .available_tools()
            .definitions()
            .iter()
            .any(|definition| definition.name == "subagent")
    );

    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {},
    }));
    product
        .runtime()
        .reload_resources()
        .await
        .expect("the empty R2 publishes");
    let r2 = product.runtime().runtime_resources();
    assert!(r2.revision().get() > r1.revision().get());
    assert!(
        r2.capability()
            .available_tools()
            .definitions()
            .iter()
            .all(|definition| definition.name != "subagent"),
        "R2 availability omits the unsatisfiable native tool"
    );
    assert!(
        r2.capability()
            .tool_registry()
            .definitions()
            .iter()
            .all(|definition| definition.name != "subagent"),
        "R2's frozen model-facing registry omits the unsatisfiable native tool"
    );

    let r1_again = r1
        .capability()
        .tool_registry()
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "subagent")
        .expect("the frozen R1 lease retains subagent");
    assert_eq!(
        r1_again, r1_definition,
        "R1 keeps the exact catalog-derived definition after R2 publishes"
    );
}

/// A definition selecting exactly the named built-ins.
fn explore(builtin: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "max_concurrent": 4,
        "roles": {
            "explore": {
                "description": "Read-only repository exploration.",

                "tools": {"builtin": builtin},
            }
        },
        "agents": ["explore"],
        "workflow": []
    })
}

fn digest_of(resources: &RuntimeResourceSnapshot, name: &str) -> NamedAgentDefinitionDigest {
    resources
        .subagents()
        .get(&agent(name))
        .expect("the generation admits the agent")
        .digest()
        .clone()
}

/// Freeze at the native resolver frontier, park before child materialization,
/// publish R2, then resume the R1 specification across its real serialization contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg236_gated_frozen_child_retains_r1_after_canonical_role_r2_publication() {
    let lab = Lab::new();
    lab.write_skill("old", "Old guidance");
    lab.write_skill("new", "New guidance");
    lab.write_config(&serde_json::json!({"roles":{"explore":{
        "description":"R1", "model": {"model": "local/model-a"}, "timeout_ms":100,
        "tools":{"builtin":["read"]}, "skills":["old"],
        "agents_md":{"inherit":false,"files":[EXPLORE_AGENTS]}
    }}}));
    let product = lab.compose().await;
    let r1 = product.runtime().runtime_resources();
    let (admitted_tx, admitted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let child = tokio::spawn(async move {
        let frozen = resolve(
            &r1,
            &agent("explore"),
            &inherited_model(),
            &model_registry(),
        )
        .unwrap();
        admitted_tx.send(frozen.clone()).unwrap();
        release_rx.await.unwrap();
        serde_json::from_slice::<ResolvedSubagentSpec>(&serde_json::to_vec(&frozen).unwrap())
            .unwrap()
    });
    let frozen = admitted_rx.await.unwrap();
    std::fs::write(
        lab.workspace().join(".agents/agents/explore.toml"),
        "R2 body\n",
    )
    .unwrap();
    std::fs::write(lab.workspace().join(EXPLORE_AGENTS), "R2 supplemental").unwrap();
    lab.write_config(&serde_json::json!({"roles":{"explore":{
        "description":"R2", "model": {"model": "local/model-b"}, "timeout_ms":200,
        "tools":{"builtin":["grep"]}, "skills":["new"],
        "agents_md":{"inherit":true}, "worktree":{"enabled":true}
    }}}));
    product.runtime().reload_resources().await.unwrap();
    let r2 = product.runtime().runtime_resources();
    let next = resolve(
        &r2,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .unwrap();
    release_tx.send(()).unwrap();
    let retained = child.await.unwrap();
    assert_eq!(retained, frozen);
    assert_eq!(
        retained.instructions,
        "Explore the shared workspace read-only.\n"
    );
    assert_eq!(retained.tool_names(), ["read"]);
    assert_eq!(retained.execution_deadline.unwrap().as_millis(), 100);
    assert_eq!(
        retained.project_instructions[0].content,
        "explicit agent instructions\n"
    );
    assert_eq!(
        retained.workspace_policy,
        rustx::runtime::workspace::WorkspacePolicy::SharedWorkspace
    );
    assert_eq!(next.instructions, "R2 body\n");
    assert_eq!(next.tool_names(), ["grep"]);
    assert_eq!(next.execution_deadline.unwrap().as_millis(), 200);
    assert_ne!(next.model, retained.model);
    assert_ne!(next.skills, retained.skills);
    assert_eq!(
        next.workspace_policy,
        rustx::runtime::workspace::WorkspacePolicy::GitWorktree {
            require_clean_parent: true
        }
    );
    product.runtime().shutdown().await.unwrap();
}

/// Only named catalog definitions are admitted, and the catalog is keyed by
/// canonical name in deterministic order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_named_catalog_definitions_are_admitted() {
    let lab = Lab::new();
    std::fs::write(
        lab.workspace().join(".agents/agents/research.toml"),
        "Research broadly.\n",
    )
    .expect("research instructions");
    lab.write_config(&serde_json::json!({
        "max_concurrent": 2,
        "roles": {
            "research": {
                "description": "Deep research.",

            },
            "explore": {
                "description": "Read-only repository exploration.",

                "tools": {"builtin": ["read", "grep"]},
            }
        }
    }));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let catalog = resources.subagents();

    assert_eq!(
        catalog
            .names()
            .into_iter()
            .map(SubagentName::as_str)
            .collect::<Vec<_>>(),
        vec!["explore", "research"],
        "the catalog is keyed by canonical name, in canonical order"
    );
    let explore = catalog.get(&agent("explore")).expect("explore is admitted");
    assert_eq!(
        explore.instructions(),
        "Explore the shared workspace read-only.\n"
    );
    assert!(
        explore.digest().as_str().starts_with("sha256:"),
        "every admitted definition carries a deterministic digest"
    );
    assert!(
        catalog.get(&agent("nonexistent")).is_none(),
        "nothing but an admitted definition is reachable"
    );

    // The model-facing tool description is derived from this exact catalog.
    let description = product
        .runtime()
        .runtime_resources()
        .capability()
        .tool_registry()
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "subagent")
        .expect("the subagent intrinsic is active")
        .description;
    assert!(description.contains("- explore: Read-only repository exploration."));
    assert!(description.contains("- research: Deep research."));
    assert!(
        !description.contains("profile"),
        "no hard-coded profile prose survives: {description}"
    );
}

/// The invariant this whole issue exists for: an attempt admitted under R1
/// resolves R1, even after a reload has made R2 runtime-current.
///
/// Ordering is established by two explicit linearizations, not by timing:
/// `reload_resources` returns only after R2 is published, and the attempt's
/// own generation handle was taken before that call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_attempt_frozen_on_r1_resolves_r1_after_r2_becomes_current() {
    let lab = Lab::new();
    let mut r1_config = explore(&["read"]);
    r1_config["roles"]["explore"]["timeout_ms"] = serde_json::json!(100);
    lab.write_config(&r1_config);
    let product = lab.compose().await;

    // R1: exactly what an attempt admitted now would freeze.
    let r1 = product.runtime().runtime_resources();
    let r1_digest = digest_of(&r1, "explore");
    assert!(r1.subagents().get(&agent("research")).is_none());

    // R2 redefines `explore` and adds `research`.
    std::fs::write(
        lab.workspace().join(".agents/agents/research.toml"),
        "Research broadly.\n",
    )
    .expect("research instructions");
    std::fs::write(
        lab.workspace().join(".agents/agents/explore.toml"),
        "Explore the shared workspace read-only, and summarize.\n",
    )
    .expect("revised explore instructions");
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {
            "explore": {
                "description": "Read-only repository exploration.",

                "tools": {"builtin": ["read"]},
                "timeout_ms": 200,
            },
            "research": {
                "description": "Deep research.",

            }
        }
    }));
    product
        .runtime()
        .reload_resources()
        .await
        .expect("the reload publishes R2");
    let r2 = product.runtime().runtime_resources();
    assert!(r2.revision().get() > r1.revision().get());

    let registry = model_registry();
    // The R1-owning attempt still sees exactly R1.
    let resolved = resolve(&r1, &agent("explore"), &inherited_model(), &registry)
        .expect("R1 still resolves its own agent");
    assert_eq!(resolved.definition_digest, r1_digest);
    assert_eq!(
        resolved
            .execution_deadline
            .expect("R1 deadline")
            .as_millis(),
        100,
        "the R1 launch keeps its frozen deadline"
    );
    assert_eq!(
        resolved.instructions, "Explore the shared workspace read-only.\n",
        "the R1 attempt observes R1's instruction document, not R2's"
    );
    assert!(
        matches!(
            resolve(&r1, &agent("research"), &inherited_model(), &registry),
            Err(SubagentResolutionError::UnknownAgent { .. })
        ),
        "an agent that only R2 admits is invisible to an R1-owning attempt"
    );

    // And the newly current generation is genuinely different.
    let from_r2 = resolve(&r2, &agent("explore"), &inherited_model(), &registry)
        .expect("R2 resolves its own agent");
    assert_eq!(
        from_r2.execution_deadline.expect("R2 deadline").as_millis(),
        200,
        "the current R2 definition has its independent deadline"
    );
    assert_ne!(from_r2.definition_digest, r1_digest);
    assert!(resolve(&r2, &agent("research"), &inherited_model(), &registry).is_ok());
}

/// A reload whose subagent catalog is invalid leaves the previous complete
/// generation authoritative: catalog, capabilities, project instructions,
/// Skills, and the active generation identity are all unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_reload_leaves_the_previous_generation_completely_authoritative() {
    let lab = Lab::new();
    lab.write_skill("alpha", "the first skill");
    lab.write_config(&explore(&["read"]));
    let product = lab.compose().await;
    let before = product.runtime().runtime_resources();
    let before_digest = digest_of(&before, "explore");

    // Everything about the candidate is wrong at once, and every observable
    // half of the generation is also rewritten on disk: a partial commit
    // would show up as any one of these having moved.
    std::fs::write(
        lab.workspace().join("AGENTS.md"),
        "rewritten instructions\n",
    )
    .expect("AGENTS.md");
    lab.write_skill("beta", "the second skill");
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {
            "explore": {
                "description": "Read-only repository exploration.",

                "tools": {"builtin": ["read", "read"]},
            }
        }
    }));
    let error = product
        .runtime()
        .reload_resources()
        .await
        .expect_err("an invalid catalog rejects the whole candidate");
    assert!(
        format!("{error}").contains("duplicate"),
        "the refusal names the offending selector: {error}"
    );

    let after = product.runtime().runtime_resources();
    assert_eq!(after.revision(), before.revision());
    assert_eq!(digest_of(&after, "explore"), before_digest);
    assert_eq!(
        after.project_instructions(),
        Some("workspace instructions\n"),
        "the retired project instruction chain is untouched"
    );
    assert!(
        after
            .skill_catalog()
            .is_some_and(|catalog| catalog.contains("alpha") && !catalog.contains("beta")),
        "the retired Skill catalog is untouched"
    );
    assert_eq!(after.capability_revision(), before.capability_revision());

    // The runtime is still healthy and a corrected reload still works.
    lab.write_config(&explore(&["read"]));
    product
        .runtime()
        .reload_resources()
        .await
        .expect("a corrected catalog publishes normally");
}

/// A named subagent is an independent projection of the invoking
/// generation's authority: the parent's active tools may be just
/// `{read, subagent}` while a definition selects an available-but-inactive
/// capability of the same generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_may_select_a_capability_that_is_available_but_inactive_for_the_parent() {
    let lab = Lab::new();
    lab.write_config_with_tools(&explore(&["grep", "glob"]), &["read"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let mut active = resources
        .capability()
        .tool_registry()
        .names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    active.sort();
    let available = resources
        .capability()
        .available_tools()
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(
        active,
        // `todo` is the parent's extension-provided Tool, which ordinary
        // `agent.tools.builtin` narrowing does not reach (Issue #259).
        vec!["read".to_owned(), "subagent".to_owned(), "todo".to_owned()],
        "the parent's active ordinary projection is deliberately narrow"
    );
    assert!(
        !available.iter().any(|name| name == "todo"),
        "and the extension Tool is not an ordinary available capability, which is why \
         no selection surface can name it"
    );
    assert!(available.contains(&"grep".to_owned()) && available.contains(&"glob".to_owned()));

    let resolved = resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("an inactive-but-available capability is legal for a named child");
    let mut names = resolved.tool_names();
    names.sort_unstable();
    assert_eq!(names, vec!["glob", "grep"]);
    assert!(
        !names.contains(&"read"),
        "a definition narrows authority; it never inherits the parent's active set"
    );
}

/// Statically invalid references — capability, model, or Skill — reject
/// resource-generation preparation deterministically.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn statically_invalid_references_fail_launch_analysis_closed() {
    for (subagents, expected) in [
        (
            serde_json::json!({
                "max_concurrent": 4,
                "roles": {"explore": {
                    "description": "d",

                    "tools": {"builtin": ["not_a_builtin"]},
                }}
            }),
            "builtin:not_a_builtin",
        ),
        (
            serde_json::json!({
                "max_concurrent": 4,
                "roles": {"explore": {
                    "description": "d",

                    "tools": {"sources": {"unconfigured": ["anything"]}},
                }}
            }),
            "source:unconfigured/anything",
        ),
        (
            serde_json::json!({
                "max_concurrent": 4,
                "roles": {"explore": {
                    "description": "d",

                    // A managed Python package (Issue #174) crosses as its
                    // synthesized MCP server (`python:<folder>`); one that
                    // does not exist is statically invalid.
                    "tools": {"sources": {"python:symbols": ["not_a_real_tool"]}},
                }}
            }),
            "source:python:symbols/not_a_real_tool",
        ),
        (
            serde_json::json!({
                "max_concurrent": 4,
                "roles": {"explore": {
                    "description": "d",

                    "model": {"model": "local/model-missing"},
                }}
            }),
            "local/model-missing",
        ),
        (
            serde_json::json!({
                "max_concurrent": 4,
                "roles": {"explore": {
                    "description": "d",

                    "skills": ["no-such-skill"],
                }}
            }),
            "no-such-skill",
        ),
    ] {
        let lab = Lab::new();
        lab.write_config(&subagents);
        let result = lab.paths().try_resolve();
        if expected == "local/model-missing" {
            assert!(result.unwrap_err().contains(expected));
        } else {
            assert!(
                result.is_ok(),
                "valid unavailable selection stays usable: {expected}"
            );
        }
    }
}

/// Recursive and execution capability selections are rejected at definition
/// admission, while `ask_user` is a valid explicit child capability.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recursive_and_execution_selections_are_rejected_at_admission() {
    {
        let capability = "subagent";
        let lab = Lab::new();
        lab.write_config(&explore(&[capability]));
        let error = lab.paths().try_resolve().unwrap_err();
        assert!(
            error.contains(capability),
            "the refusal names {capability}: {error}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_ask_user_selection_is_admitted_for_a_child() {
    let lab = Lab::new();
    lab.write_config(&explore(&["ask_user"]));
    LocalSessionProduct::compose(&(lab.paths()).resolve(), &dependencies())
        .await
        .expect("ask_user is a routed child capability when explicitly selected");
}

/// An unavailable optional source keeps the runtime healthy, while an agent
/// that explicitly requires a capability from that source cannot start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unavailable_source_warns_and_suppresses_without_blocking_the_agent() {
    let lab = Lab::new();
    let mut document = serde_json::json!({"schema_version": 8, "agent_id": "agent-issue144", "context": {"reserve_tokens": 0, "keep_recent_tokens": 0}, "mcp_servers": {
            "offline": {"enabled": true, "type": "stdio", "command": "missing-rustx-issue144-mcp"}
        }, "subagents": {"max_concurrent": 4, "roles": {"explore": {"description": "Read-only repository exploration.", "tools": {"sources": {"offline": ["get_issue"]}}}}, "workflow": []}, "agent": {"model": {"model": "local/model-a"}, "tools": {"builtin": ["read"]}, "agents": ["explore"]}});
    crate::launch_fixture::write_roles(&lab.workspace(), &mut document["subagents"]);
    std::fs::write(
        lab.root().join("rustx.toml"),
        toml::to_string_pretty(&document).unwrap(),
    )
    .expect("rustx.toml");

    // The whole runtime composes: an optional source failure is availability
    // state, not a composition error, and the catalog is still admitted.
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    assert!(
        matches!(
            resources.capability_availability().get(&ToolSourceId::Mcp(
                rustx::runtime::identity::McpServerId::new("offline")
            )),
            Some(CapabilitySourceState::Unavailable { .. })
        ),
        "the failed source is recorded as availability state: {:?}",
        resources.capability_availability()
    );
    assert!(resources.subagents().get(&agent("explore")).is_some());

    let child = resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("valid unavailable selection leaves a usable child");
    assert!(child.tools.is_empty());
    assert!(matches!(
        resources
            .resolved_agent(&agent("explore"))
            .unwrap()
            .diagnostics
            .as_slice(),
        [rustx::runtime::agent_profile::AgentProfileDiagnostic::Tool(
            rustx::capabilities::selection::ToolSelectionError::SourceUnavailable { .. }
        )]
    ));
}

/// A definition with no explicit model inherits the invoking attempt's
/// frozen configuration; an explicit one freezes the configured model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_semantics_inherit_the_invoking_attempt_or_freeze_the_explicit_selection() {
    let lab = Lab::new();
    std::fs::write(
        lab.workspace().join(".agents/agents/pinned.toml"),
        "Run on the pinned model.\n",
    )
    .expect("pinned instructions");
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {
            "explore": {
                "description": "Inherits the invoking attempt's model.",

            },
            "pinned": {
                "description": "Runs on its own model.",

                "model": {"model": "local/model-b"},
            }
        }
    }));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let registry = model_registry();

    // The runtime was *composed* while the configured default was model-a;
    // the invoking attempt is frozen on model-b. The inheriting agent must
    // follow the attempt, not the composition-time capture.
    let attempt_model =
        SessionModelConfig::of(ModelRef::parse("local/model-b").expect("model reference"));
    let inheriting = resolve(&resources, &agent("explore"), &attempt_model, &registry)
        .expect("the inheriting agent resolves");
    assert_eq!(
        inheriting.model.primary.model.to_string(),
        "local/model-b",
        "a default child model is the invoking attempt's frozen model"
    );
    assert_eq!(
        inheriting.model.configured.model.to_string(),
        "local/model-b"
    );

    // And an explicit selection is independent of the invoking attempt.
    let pinned = resolve(
        &resources,
        &agent("pinned"),
        &SessionModelConfig::of(ModelRef::parse("local/model-a").expect("model reference")),
        &registry,
    )
    .expect("the pinned agent resolves");
    assert_eq!(pinned.model.primary.model.to_string(), "local/model-b");
}

/// Project-instruction policy: `inherit = true` produces the generation's
/// exact frozen chain followed by the explicit files in configured order;
/// `inherit = false` produces only the explicit files.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_instruction_policy_freezes_a_deterministic_chain() {
    let lab = Lab::new();
    std::fs::write(
        lab.subagent_file("isolated", "instructions.md"),
        "Ignore the workspace chain.\n",
    )
    .expect("isolated instructions");
    let files = serde_json::json!([
        lab.workspace().join(EXPLORE_AGENTS),
        lab.workspace().join(EXPLORE_EXTRA)
    ]);
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {
            "explore": {
                "description": "Inherits the parent chain.",

                "agents_md": {"inherit": true, "files": files},
            },
            "isolated": {
                "description": "Explicit files only.",

                "agents_md": {"inherit": false, "files": files},
            }
        }
    }));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let registry = model_registry();

    let parent_chain = resources
        .project_context_files()
        .iter()
        .map(|file| file.content.clone())
        .collect::<Vec<_>>();
    assert!(parent_chain.contains(&"workspace instructions\n".to_owned()));

    let inherited = resolve(&resources, &agent("explore"), &inherited_model(), &registry)
        .expect("the inheriting agent resolves");
    let inherited_contents = project_instruction_contents(&inherited);
    let mut expected = parent_chain.clone();
    expected.push("explicit agent instructions\n".to_owned());
    expected.push("second explicit file\n".to_owned());
    assert_eq!(
        inherited_contents, expected,
        "inherit=true is the exact parent chain followed by the explicit files in order"
    );

    let isolated = resolve(
        &resources,
        &agent("isolated"),
        &inherited_model(),
        &registry,
    )
    .expect("the isolated agent resolves");
    assert_eq!(
        project_instruction_contents(&isolated),
        vec![
            "explicit agent instructions\n".to_owned(),
            "second explicit file\n".to_owned(),
        ],
        "inherit=false freezes only the explicit files"
    );

    // Ordering is configuration order, not filesystem or map order: the same
    // files listed the other way round produce the other order.
    let reversed = serde_json::json!([
        lab.workspace().join(EXPLORE_EXTRA),
        lab.workspace().join(EXPLORE_AGENTS)
    ]);
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {"isolated": {
            "description": "Explicit files only.",

            "agents_md": {"inherit": false, "files": reversed},
        }}
    }));
    product
        .runtime()
        .reload_resources()
        .await
        .expect("the reversed order publishes");
    let reloaded = product.runtime().runtime_resources();
    let reordered = resolve(&reloaded, &agent("isolated"), &inherited_model(), &registry)
        .expect("the isolated agent resolves");
    assert_eq!(
        project_instruction_contents(&reordered),
        vec![
            "second explicit file\n".to_owned(),
            "explicit agent instructions\n".to_owned(),
        ]
    );
    assert_ne!(
        reordered.definition_digest, isolated.definition_digest,
        "explicit project-instruction order is semantic and changes the digest"
    );
}

/// The per-agent Skill list is an exact allowlist over the admitted Skill
/// catalog, and only catalog metadata is frozen: progressive disclosure is
/// preserved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_skill_allowlist_is_exact_and_preserves_progressive_disclosure() {
    let lab = Lab::new();
    lab.write_skill("alpha", "the first skill");
    lab.write_skill("beta", "the second skill");
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {"explore": {
            "description": "Read-only repository exploration.",

            "skills": ["alpha"],
        }}
    }));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    // The parent generation admits both.
    let parent_catalog = resources.skill_catalog().expect("the parent Skill catalog");
    assert!(parent_catalog.contains("alpha") && parent_catalog.contains("beta"));

    let resolved = resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("the allowlist resolves");
    assert_eq!(
        resolved
            .skills
            .iter()
            .map(|skill| skill.catalog_entry.name.clone())
            .collect::<Vec<_>>(),
        vec!["alpha".to_owned()],
        "an unselected Skill is absent from the child-visible catalog"
    );
    let entry = &resolved.skills[0].catalog_entry;
    assert_eq!(entry.description, "the first skill");
    assert!(
        entry.location.ends_with("SKILL.md"),
        "the child receives the Skill's host location, not its body"
    );
    assert!(
        !serde_json::to_string(&resolved.skills)
            .expect("serialize the frozen catalog")
            .contains("alpha body"),
        "no SKILL.md body is preloaded into the frozen specification"
    );
}

/// The definition digest ignores incidental TOML formatting, comments, and
/// key order, and changes for every semantic difference.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_definition_digest_ignores_incidental_formatting_and_tracks_semantics() {
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {"explore": {
            "description": "Read-only repository exploration.",

            "tools": {"builtin": ["read", "grep"]},
        }}
    }));
    let product = lab.compose().await;
    let baseline = digest_of(&product.runtime().runtime_resources(), "explore");

    // TOML comments, field order and selector order do not change the native digest.
    std::fs::write(
        lab.workspace().join(".agents/agents/explore.toml"),
        "tools = {builtin = [\"grep\", \"read\"]}\ndescription = \"Read-only repository exploration.\"\ninstructions = \"Explore the shared workspace read-only.\\n\"\n",
    ).unwrap();
    product
        .runtime()
        .reload_resources()
        .await
        .expect("the reformatted config publishes");
    assert_eq!(
        digest_of(&product.runtime().runtime_resources(), "explore"),
        baseline,
        "comments, whitespace, key order, and selector order are not semantics"
    );

    // A genuine semantic change does move the digest.
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {"explore": {
            "description": "Read-only repository exploration.",

            "tools": {"builtin": ["read"]},
        }}
    }));
    product
        .runtime()
        .reload_resources()
        .await
        .expect("the narrowed config publishes");
    assert_ne!(
        digest_of(&product.runtime().runtime_resources(), "explore"),
        baseline,
        "dropping a capability is a semantic change"
    );
}

/// The frozen specification keeps exact source-qualified identity for every
/// origin, and that identity survives the IPC serialization boundary
/// unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_frozen_specification_preserves_exact_builtin_identity_through_serialization() {
    let lab = Lab::new();
    let mut config = explore(&["read", "grep"]);
    config["roles"]["explore"]["timeout_ms"] = serde_json::json!(30_000);
    lab.write_config(&config);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let resolved = resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("the definition resolves");
    assert_eq!(
        resolved
            .execution_deadline
            .expect("resolved deadline")
            .as_millis(),
        30_000,
        "resolution freezes the admitted definition deadline"
    );

    for tool in &resolved.tools {
        let ResolvedSubagentTool::Builtin {
            tool_id,
            name,
            definition,
        } = tool
        else {
            panic!("a builtin selector freezes a builtin identity: {tool:?}");
        };
        assert_eq!(tool_id, &definition.id);
        assert_eq!(name, &definition.name);
        assert_eq!(definition.origin, rustx::tools::types::ToolOrigin::Builtin);
    }
    assert!(
        resolved.materialization.is_empty(),
        "a Builtin-only agent needs no externally sourced materialization plane"
    );

    let encoded = serde_json::to_vec(&resolved).expect("encode the frozen specification");
    let decoded: rustx::runtime::subagent::ResolvedSubagentSpec =
        serde_json::from_slice(&encoded).expect("decode the frozen specification");
    assert_eq!(decoded, resolved);
}

/// The launch-scoped capacity comes from configuration and is deliberately
/// not resized by a reload.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn max_concurrent_is_launch_scoped() {
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({
        "max_concurrent": 1,
        "roles": {"explore": {
            "description": "Read-only repository exploration.",

            "tools": {"builtin": ["read"]},
        }}
    }));
    let product = lab.compose().await;

    // A reload that changes the bound publishes a new catalog without
    // touching the live registry's capacity: capacity is live-registry state.
    lab.write_config(&serde_json::json!({
        "max_concurrent": 8,
        "roles": {"explore": {
            "description": "Read-only repository exploration, revised.",

            "tools": {"builtin": ["read"]},
        }}
    }));
    product
        .runtime()
        .reload_resources()
        .await
        .expect("the reload publishes");
    assert_eq!(
        product
            .runtime()
            .runtime_resources()
            .subagents()
            .get(&agent("explore"))
            .expect("explore is admitted")
            .description(),
        "Read-only repository exploration, revised.",
        "the catalog half of the generation did change"
    );
    // A zero or oversized bound is refused at the configuration boundary.
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({"max_concurrent": 0, "roles": {}}));
    let error = lab
        .paths()
        .try_resolve()
        .expect_err("a zero bound is refused");
    assert!(error.contains("max_concurrent"));
}

/// An invalid agent name is rejected deterministically at the configuration
/// boundary rather than normalized into something else.
#[test]
fn invalid_agent_names_are_rejected_deterministically() {
    for spelling in ["Explore", "2explore", "explore/child", "explore agent", ""] {
        assert!(
            SubagentName::parse(spelling).is_err(),
            "{spelling:?} must not be a canonical agent name"
        );
    }
    assert_eq!(agent("deep-research_2").as_str(), "deep-research_2");
}

/// The committed durable ownership fact carries
/// `(agent, definition_digest, profile_digest)` and survives a round trip
/// through the real durable authority unchanged.
#[test]
fn the_committed_identity_survives_a_durable_round_trip() {
    use rustx::durable::{ConversationStore, SqliteConversationStore};
    use rustx::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use rustx::runtime::identity::{AgentId, ConversationId, EventId, SubagentId, ToolCallId};

    let dir = tempfile::tempdir().expect("durable directory");
    let conversation_id = ConversationId::new("conv-issue144");
    let store = SqliteConversationStore::open(
        conversation_id.clone(),
        &dir.path().join("conversation.sqlite"),
    )
    .expect("durable store");
    store.initialize(&[]).expect("bootstrap");

    let subagent_id = SubagentId::for_conversation(&conversation_id, 1);
    let committed = RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(format!("subagent-committed-event:{subagent_id}")),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp: chrono::Utc::now(),
        event: RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id: subagent_id.clone(),
            child_agent_id: AgentId::new(format!("agent-{subagent_id}")),
            child_conversation_id: ConversationId::new(subagent_id.as_str()),
            tool_call_id: ToolCallId::new("call-sub"),
            agent: "explore".to_owned(),
            definition_digest: "sha256:d1".to_owned(),
            profile_digest: "sha256:profile".to_owned(),
            ownership: rustx::events::types::SubagentOwnershipKind::Normal,
            workspace: rustx::runtime::workspace::WorkspaceSnapshot::shared(
                std::path::PathBuf::from("<shared-workspace>"),
            ),
        },
    };
    store
        .append_event(committed)
        .expect("durable ownership commit");

    let events = store.read_events(None, 64).expect("read events").events;
    let fact = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            RuntimeEvent::SubagentOwnershipCommitted {
                agent,
                definition_digest,
                profile_digest,
                ..
            } => Some((
                agent.clone(),
                definition_digest.clone(),
                profile_digest.clone(),
            )),
            _ => None,
        })
        .expect("the ownership fact round-trips");
    assert_eq!(
        fact,
        (
            "explore".to_owned(),
            "sha256:d1".to_owned(),
            "sha256:profile".to_owned()
        ),
        "both the source-definition identity and the effective execution-profile identity are \
         durable facts"
    );
}

/// The Runtime Client projection of a subagent carries the named-agent
/// identity, and its wire shape has no `profile` field at all.
#[test]
fn the_runtime_client_projection_carries_the_named_identity() {
    use rustx::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
    use rustx::runtime::subagent::{
        SubagentSnapshot, SubagentState, SubagentWorkspaceResourceState,
    };
    use rustx::runtime_client::snapshot::RuntimeClientSubagent;

    let snapshot = SubagentSnapshot {
        subagent_id: SubagentId::new("conv-1-subagent-1"),
        child_agent_id: AgentId::new("agent-child"),
        child_conversation_id: ConversationId::new("conv-1-subagent-1"),
        tool_call_id: ToolCallId::new("call-1"),
        agent: "explore".to_owned(),
        definition_digest: "sha256:d1".to_owned(),
        profile_digest: "sha256:p1".to_owned(),
        workspace: rustx::runtime::workspace::WorkspaceSnapshot::shared(std::path::PathBuf::from(
            "<shared-workspace>",
        )),
        handoff: None,
        workspace_resource_state: SubagentWorkspaceResourceState::None,
        state: SubagentState::Running,
        cancel_reason: None,
        detail: None,
        observation: rustx::runtime::subagent::SubagentObservation::default(),
        profile: None,
        publication_abandoned: false,
        settled: false,
        started_at: chrono::Utc::now(),
    };
    let view = RuntimeClientSubagent {
        subagent_id: snapshot.subagent_id.clone(),
        child_agent_id: snapshot.child_agent_id.clone(),
        child_conversation_id: snapshot.child_conversation_id.clone(),
        agent: snapshot.agent.clone(),
        definition_digest: snapshot.definition_digest.clone(),
        profile_digest: snapshot.profile_digest.clone(),
        state: snapshot.state,
        detail: None,
        observation: snapshot.observation.clone(),
        execution_profile: None,
        started_at: snapshot.started_at,
        workspace: rustx::runtime_client::snapshot::RuntimeClientSubagentWorkspace {
            borrowed_from: None,
            logical_workspace: snapshot.workspace.logical_workspace.clone(),
            isolation: rustx::runtime_client::snapshot::RuntimeClientWorkspaceIsolation::Shared,
            resource_state: snapshot.workspace_resource_state,
            handoff: None,
        },
    };
    let wire = serde_json::to_value(&view).expect("serialize the projection");
    assert_eq!(wire["agent"], "explore");
    assert_eq!(wire["definition_digest"], "sha256:d1");
    assert_eq!(
        wire["profile_digest"], "sha256:p1",
        "the effective execution profile identity is projected beside the definition identity"
    );
    assert!(
        wire.get("profile").is_none(),
        "the obsolete profile field is absent from the wire shape: {wire}"
    );

    // The obsolete profile-shaped payload is rejected outright rather than
    // decoded with an invented agent identity.
    let obsolete = serde_json::json!({
        "subagent_id": "conv-1-subagent-1",
        "child_agent_id": "agent-child",
        "child_conversation_id": "conv-1-subagent-1",
        "profile": "explore",
        "state": "running"
    });
    assert!(
        serde_json::from_value::<RuntimeClientSubagent>(obsolete).is_err(),
        "the profile-shaped contract must fail"
    );
}

// ---------------------------------------------------------------------------
// The four freeze-contract regressions.
//
// Each one proves that a decision Issue #144 claims to freeze is really
// decided by the parent and merely *materialized* by the child, rather than
// re-resolved against mutable state the child can observe changing.
// ---------------------------------------------------------------------------

/// A `models.toml` whose `local/model-a` has materially different semantics
/// from [`MODELS`]: a different endpoint, protocol, context window, output
/// budget, compat metadata, and request parameters — and no `model-b` at
/// all, so a re-resolving child would also *fail* where the parent
/// succeeded.
const MODELS_MUTATED: &str = r#"[providers.local]
base_url = "http://127.0.0.1:10/v2"
api_key = "$RUSTX_ISSUE144_KEY"

[[providers.local.models]]
id = "model-a"
protocol = "anthropic_messages"
context_window = 1000
max_output_tokens = 64
request_params = { temperature = 0.9 }

[providers.local.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false
"#;

/// Blocker 1: the child's model semantics are frozen by the parent, so a
/// `models.toml` edit that lands between the freeze and the child's
/// composition cannot be observed by that child.
///
/// The race is driven by two explicit linearizations — the resolver call
/// returns before the file is rewritten, and the child model authority is
/// composed after it — never by a sleep.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frozen_child_model_never_observes_a_later_models_toml_edit() {
    use rustx::model::session::SessionModelState;
    use rustx::model::types::ModelProtocol;

    let lab = Lab::new();
    lab.write_config(&explore(&["read"]));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    // M1: freeze the child against the catalog the parent was admitted with.
    let resolved = resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("the child resolves under M1");
    let frozen = resolved.model.clone();
    assert_eq!(
        frozen.primary.protocol,
        ModelProtocol::OpenAiChatCompletions
    );
    assert_eq!(frozen.primary.context_window, 128_000);
    assert_eq!(frozen.primary.max_output_tokens, 512);
    assert_eq!(frozen.primary.binding.base_url, "http://127.0.0.1:9/v1");
    assert!(frozen.primary.request_params.is_empty());

    // The frozen specification survives the real IPC wire unchanged.
    let over_the_wire: rustx::runtime::subagent::ResolvedSubagentSpec =
        serde_json::from_slice(&serde_json::to_vec(&resolved).expect("encode"))
            .expect("decode the frozen specification");
    assert_eq!(over_the_wire, resolved);

    // M2: the catalog now says something materially different for the very
    // same model reference, and drops `model-b` entirely.
    std::fs::write(lab.root().join("models.toml"), MODELS_MUTATED).expect("mutate models.toml");

    // A re-resolving consumer *would* observe M2 — this is what makes the
    // assertion below meaningful rather than vacuous.
    let mutated = ModelCatalog::from_toml_slice(MODELS_MUTATED.as_bytes()).expect("M2 parses");
    let mutated_registry = ModelBindingRegistry::new(
        mutated
            .resolve(dependencies().credentials.as_deref().unwrap())
            .expect("M2 resolves"),
    )
    .expect("M2 binds");
    let reresolved = mutated_registry
        .resolve(&inherited_model().selection())
        .expect("M2 still has local/model-a");
    assert_eq!(reresolved.protocol(), ModelProtocol::AnthropicMessages);
    assert_eq!(reresolved.context_window(), 1_000);
    assert!(
        mutated_registry
            .resolve(
                &SessionModelConfig::of(ModelRef::parse("local/model-b").expect("reference"))
                    .selection()
            )
            .is_err(),
        "M2 really did remove a model the parent could resolve"
    );

    // The child composes its model authority from the frozen specification
    // — exactly what `compose_subagent_child` does — and observes M1.
    let child = SessionModelState::frozen(
        &over_the_wire.model,
        dependencies().credentials.as_deref().unwrap(),
    )
    .expect("the child materializes the frozen authority");
    let attempt = child.snapshot();
    assert_eq!(
        attempt.primary().protocol(),
        ModelProtocol::OpenAiChatCompletions,
        "the child speaks the protocol the parent froze, not M2's"
    );
    assert_eq!(attempt.primary().context_window(), 128_000);
    assert_eq!(attempt.primary().max_output_tokens(), 512);
    assert_eq!(attempt.primary().model_ref().to_string(), "local/model-a");
    assert!(
        attempt.primary().request_params().is_empty(),
        "M2's model default request parameters are never observed: {:?}",
        attempt.primary().request_params()
    );
    assert_eq!(
        child.catalog_view().models.len(),
        1,
        "a frozen authority publishes exactly the model it froze"
    );
    assert!(
        child.registry().is_none(),
        "a frozen authority owns no mutable catalog to re-resolve against"
    );

    // And a model the parent froze under M1 stays resolvable for the child
    // even though M2 deleted it.
    let pinned_frozen = rustx::model::frozen::FrozenModelSpec::freeze(
        &model_registry(),
        &SessionModelConfig::of(ModelRef::parse("local/model-b").expect("reference")),
    )
    .expect("the parent froze model-b under M1");
    assert!(
        SessionModelState::frozen(
            &pinned_frozen,
            dependencies().credentials.as_deref().unwrap()
        )
        .is_ok(),
        "a child frozen on a model M2 removed still starts"
    );
}

/// Blocker 2: the exact parent-frozen Builtin `ToolDefinition` — including a
/// non-default invocation policy on all three axes — is what the child's
/// `ToolRegistry` ends up holding, across the serialization boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_default_builtin_policy_survives_child_materialization_exactly() {
    use rustx::tools::executor::ToolRegistry;
    use rustx::tools::native::register_subagent_child_tools;
    use rustx::tools::types::{ToolApprovalPolicy, ToolConcurrencyPolicy, ToolExecutionPolicy};

    let lab = Lab::new();
    // The generation admits `grep` with a non-default policy on every axis.
    let mut document = serde_json::json!({"schema_version": 8, "agent_id": "agent-issue144", "context": {"reserve_tokens": 0, "keep_recent_tokens": 0}, "native_tools": {
            "grep": {
                "execution": "model_selectable",
                "concurrency": "parallel",
                "approval": "always",
            }
        }, "subagents": explore(&["grep"]), "agent": {"model": {"model": "local/model-a"}, "tools": {"builtin": ["read"]}}});
    document["agent"]["agents"] = document["subagents"]
        .as_object_mut()
        .unwrap()
        .remove("agents")
        .unwrap();
    crate::launch_fixture::write_documents(
        &lab.root().join("rustx.toml"),
        &toml::to_string_pretty(&document).unwrap(),
        &["native_tools"],
    );
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let resolved = resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("the child resolves");
    let frozen = match &resolved.tools[0] {
        ResolvedSubagentTool::Builtin { definition, .. } => definition.clone(),
        other @ ResolvedSubagentTool::Source { .. } => {
            panic!("expected a frozen Builtin, found {other:?}")
        }
    };
    assert_eq!(
        frozen.execution_policy,
        ToolExecutionPolicy::ModelSelectable,
        "the generation really admitted a non-default policy"
    );
    assert_eq!(frozen.concurrency_policy, ToolConcurrencyPolicy::Parallel);
    assert_eq!(frozen.approval_policy, ToolApprovalPolicy::Always);

    // Cross the real wire, then materialize the child registry.
    let over_the_wire: rustx::runtime::subagent::ResolvedSubagentSpec =
        serde_json::from_slice(&serde_json::to_vec(&resolved).expect("encode")).expect("decode");
    let wire_definitions: Vec<_> = over_the_wire
        .tools
        .iter()
        .map(|tool| tool.definition().clone())
        .collect();
    let mut registry = ToolRegistry::new();
    register_subagent_child_tools(&mut registry, &wire_definitions)
        .expect("the child materializes the frozen definitions");

    let child_definition = registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "grep")
        .expect("the child registered grep")
        .clone();
    assert_eq!(
        child_definition, frozen,
        "the child holds the whole parent-frozen definition, not a default-policy rebuild"
    );
}

/// Blocker 3: an unavailable optional source may block one invocation, but
/// it must never stop admission from validating the rest of a definition's
/// selectors. The offline MCP selector sorts first in canonical order, so a
/// short-circuiting validator would never reach the invalid one naming a
/// server that does not exist (a `python:<folder>` id without any managed
/// package, Issue #174).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unavailable_source_cannot_hide_a_later_invalid_selector() {
    let lab = Lab::new();
    let mut document = serde_json::json!({"schema_version": 8, "agent_id": "agent-issue144", "context": {"reserve_tokens": 0, "keep_recent_tokens": 0}, "mcp_servers": {
            "offline": {"type": "stdio", "command": "missing-rustx-issue144-mcp"}
        }, "subagents": {"max_concurrent": 4, "roles": {"explore": {"description": "Read-only repository exploration.", "tools": {
                        // `offline` sorts before `python:ghost` in canonical
                        // selector order, so the unavailable source is
                        // inspected first.
                        "sources": {"offline": ["get_issue"], "python:ghost": ["duplicate", "duplicate"]},
                    }}}, "workflow": []}, "agent": {"model": {"model": "local/model-a"}, "tools": {"builtin": ["read"]}, "agents": ["explore"]}});
    crate::launch_fixture::write_roles(&lab.workspace(), &mut document["subagents"]);
    std::fs::write(
        lab.root().join("rustx.toml"),
        toml::to_string_pretty(&document).expect("config document"),
    )
    .expect("rustx.toml");

    let error = lab
        .paths()
        .try_resolve()
        .expect_err("a statically invalid selector rejects shared launch analysis");
    let rendered = error;
    assert!(
        rendered.contains("duplicate"),
        "the selector after the unavailable source is still validated: {rendered}"
    );
}

/// Blocker 4: the exact `SkillId` + `SkillVersionId` the invoking generation
/// admitted crosses the parent/child boundary, catalog metadata still
/// crosses with it, no body is preloaded, and a later filesystem change
/// never reinterprets an already-frozen specification.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skill_version_identity_is_frozen_across_the_boundary() {
    let lab = Lab::new();
    lab.write_skill("alpha", "the first skill");
    lab.write_skill("beta", "the second skill");
    lab.write_config(&serde_json::json!({
        "max_concurrent": 4,
        "roles": {"explore": {
            "description": "Read-only repository exploration.",

            "skills": ["alpha"],
        }}
    }));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    // The generation's own immutable binding for `alpha`.
    let admitted = resources
        .capability()
        .skills()
        .packages()
        .iter()
        .find(|package| package.name() == "alpha")
        .expect("the generation admitted alpha")
        .clone();

    let resolved = resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("the allowlist resolves");
    assert_eq!(resolved.skills.len(), 1);
    assert_eq!(&resolved.skills[0].binding.skill_id, admitted.id());
    assert_eq!(
        &resolved.skills[0].binding.version_id,
        admitted.version_id()
    );

    // 1. The exact identity survives the IPC serialization boundary.
    let over_the_wire: rustx::runtime::subagent::ResolvedSubagentSpec =
        serde_json::from_slice(&serde_json::to_vec(&resolved).expect("encode")).expect("decode");
    assert_eq!(over_the_wire.skills, resolved.skills);
    assert_eq!(&over_the_wire.skills[0].binding.skill_id, admitted.id());
    assert_eq!(
        &over_the_wire.skills[0].binding.version_id,
        admitted.version_id()
    );

    // 2. The model-visible catalog is exactly the selected metadata.
    assert_eq!(over_the_wire.skills[0].catalog_entry.name, "alpha");
    assert_eq!(
        over_the_wire.skills[0].catalog_entry.description,
        "the first skill"
    );
    let encoded = serde_json::to_string(&over_the_wire.skills).expect("encode the frozen skills");
    assert!(
        !encoded.contains("beta"),
        "an unselected Skill stays invisible: {encoded}"
    );

    // 3. No SKILL.md body is preloaded.
    assert!(
        !encoded.contains("alpha body"),
        "progressive disclosure is preserved: no body crosses the boundary"
    );

    // 4. Rewriting the Skill on disk does not reinterpret the frozen spec.
    lab.write_skill("alpha", "a completely different description");
    std::fs::write(
        lab.workspace()
            .join(".agents/skills/alpha")
            .join("SKILL.md"),
        "---\nname: alpha\ndescription: a completely different description\n---\n\nrewritten body\n",
    )
    .expect("rewrite SKILL.md");
    product
        .runtime()
        .shutdown()
        .await
        .expect("settle the original owner");
    drop(resources);
    drop(product);
    let reloaded = LocalSessionProduct::compose(&(lab.paths()).resolve(), &dependencies())
        .await
        .expect("the rewritten workspace composes");
    let rewritten = reloaded
        .runtime()
        .runtime_resources()
        .capability()
        .skills()
        .packages()
        .iter()
        .find(|package| package.name() == "alpha")
        .expect("alpha is still admitted")
        .clone();
    assert_ne!(
        rewritten.version_id(),
        admitted.version_id(),
        "the rewritten Skill really is a different version"
    );
    assert_eq!(
        &over_the_wire.skills[0].binding.version_id,
        admitted.version_id(),
        "the old frozen specification still names the version it froze"
    );
    assert_eq!(
        over_the_wire.skills[0].catalog_entry.description, "the first skill",
        "and its frozen metadata is not reinterpreted either"
    );
}

// ---------------------------------------------------------------------------
// Issue #256: independently authored native Agent Extension compositions
// ---------------------------------------------------------------------------

/// Writes a launch document whose root declares its own extension
/// composition, alongside the named roles.
fn write_config_with_root_extensions(
    lab: &Lab,
    subagents: &serde_json::Value,
    root_extensions: &serde_json::Value,
) {
    let mut subagents = subagents.clone();
    let definition_names = subagents
        .get("roles")
        .and_then(serde_json::Value::as_object)
        .map(|definitions| {
            definitions
                .keys()
                .map(|name| serde_json::Value::String(name.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(document) = subagents.as_object_mut() {
        document
            .entry("agents".to_owned())
            .or_insert_with(|| serde_json::Value::Array(definition_names.clone()));
        document
            .entry("workflow".to_owned())
            .or_insert_with(|| serde_json::Value::Array(definition_names));
    }
    let root_agents = subagents.as_object_mut().unwrap().remove("agents").unwrap();
    crate::launch_fixture::write_roles(&lab.workspace(), &mut subagents);
    let document = serde_json::json!({"schema_version": 8, "agent_id": "agent-issue256", "context": {"reserve_tokens": 0, "keep_recent_tokens": 0}, "subagents": subagents, "agent": {"model": {"model": "local/model-a"}, "extensions": root_extensions.clone(), "tools": {"builtin": ["read"]}, "agents": root_agents}});
    std::fs::write(
        lab.root().join("rustx.toml"),
        toml::to_string_pretty(&document).expect("config document"),
    )
    .expect("rustx.toml");
}

fn frozen_extensions(
    resources: &RuntimeResourceSnapshot,
    name: &str,
) -> rustx::extensions::NativeAgentExtensions {
    resolve(
        resources,
        &agent(name),
        &inherited_model(),
        &model_registry(),
    )
    .expect("the generation resolves the role")
    .extensions
}

/// Issue #256 regression 6.
///
/// Root Agent extensions and named-Subagent extensions are independently
/// authored compositions. The root here declares a distinctive Agent Status
/// configuration; neither a role that declares its own, nor a role that
/// declares none, is widened by it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ext256_root_extension_configuration_never_reaches_a_named_role() {
    let lab = Lab::new();
    write_config_with_root_extensions(
        &lab,
        &serde_json::json!({"roles": {
            "explore": {
                "description": "Declares its own composition.",
                "tools": {"builtin": ["read"]},
                "extensions": {"agent_status": {"time": {"enabled": false}}},
            },
            "research": {
                "description": "Declares no composition at all.",
                "tools": {"builtin": ["read"]},
            },
        }}),
        &serde_json::json!({"agent_status": {
            "enabled": true,
            "time": {"enabled": true, "timezone": "America/New_York"},
            "background": {"enabled": false}
        }}),
    );
    let product = lab.compose().await;
    let runtime = product.runtime();
    let resources = runtime.runtime_resources();

    // The root composed exactly what the root document declared.
    let root = runtime
        .context_config()
        .status_engine
        .as_ref()
        .expect("the root composes its declared Agent Status extension")
        .config()
        .clone();
    assert_eq!(root.time.timezone, Some(chrono_tz::America::New_York));
    assert!(!root.background.enabled);

    // The declaring role composed exactly what *it* declared.
    let declared = frozen_extensions(&resources, "explore");
    let declared = declared
        .agent_status()
        .expect("the role declares Agent Status");
    assert!(!declared.time.enabled, "the role's own Time decision wins");
    assert_eq!(
        declared.time.timezone, None,
        "the root's timezone never reaches the child"
    );
    assert!(
        declared.background.enabled,
        "the root's disabled Background never narrows the child either"
    );

    let silent_frozen = frozen_extensions(&resources, "research");
    assert!(
        silent_frozen.is_empty(),
        "omission selects no extensions in every profile"
    );

    // Issue #256 regression 8: root extension configuration cannot leak
    // into a child's Runtime Client projection. The root and each child
    // project different values, and neither child's projection carries the
    // root's timezone or its disabled Background.
    let root_projection = EffectiveNativeAgentExtensions::project(&runtime.native_extensions());
    assert_eq!(
        root_projection
            .agent_status
            .expect("the root composes Agent Status")
            .time
            .timezone,
        Some(chrono_tz::America::New_York)
    );
    for child in [
        frozen_extensions(&resources, "explore"),
        silent_frozen.clone(),
    ] {
        let projected = EffectiveNativeAgentExtensions::project(&child);
        assert_ne!(
            projected, root_projection,
            "a child projection is never the root's composition"
        );
        let Some(status) = projected.agent_status else {
            continue;
        };
        assert_eq!(
            status.time.timezone, None,
            "the root's frozen timezone never appears in a child projection"
        );
        assert!(
            status.background.enabled,
            "nor does the root's disabled Background"
        );
    }

    product.runtime().shutdown().await.unwrap();
}

/// Issue #256 regression 7: two role definitions that differ only in their
/// Agent Status extension settings are semantically different definitions —
/// different digests and different frozen child specifications.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ext256_extension_settings_are_part_of_a_role_identity() {
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({"roles": {
        "explore": {
            "description": "Same in every respect but its extensions.",
            "tools": {"builtin": ["read"]},
            "extensions": {"agent_status": {"time": {"timezone": "Asia/Shanghai"}}},
        },
        "research": {
            "description": "Same in every respect but its extensions.",
            "tools": {"builtin": ["read"]},
            "extensions": {"agent_status": {"enabled": false}, "todo": {"enabled": false}},
        },
        "pinned": {
            "description": "Same in every respect but its extensions.",
            "tools": {"builtin": ["read"]},
            "extensions": {"agent_status": {"time": {"timezone": "America/New_York"}}},
        },
    }}));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let shanghai = frozen_extensions(&resources, "explore");
    let absent = frozen_extensions(&resources, "research");
    let new_york = frozen_extensions(&resources, "pinned");
    assert_eq!(
        shanghai
            .agent_status()
            .expect("composed")
            .time
            .timezone
            .map(|zone| zone.name().to_owned()),
        Some("Asia/Shanghai".to_owned())
    );
    assert!(
        absent.is_empty(),
        "a role may compose no native Agent Extension at all"
    );
    // Issue #259: a role that composes only Todo is a fourth distinct
    // identity, so the extension vocabulary as a whole — not just Agent
    // Status — participates in a role's semantic identity.
    assert!(shanghai.todo().is_none() && new_york.todo().is_none());
    assert_eq!(
        new_york
            .agent_status()
            .expect("composed")
            .time
            .timezone
            .map(|zone| zone.name().to_owned()),
        Some("America/New_York".to_owned())
    );
    assert_ne!(shanghai, absent);
    assert_ne!(shanghai, new_york);

    // Extension settings participate in the role's semantic identity: the
    // three definitions differ *only* by them, and their digests differ.
    let mut digests = vec![
        digest_of(&resources, "explore"),
        digest_of(&resources, "research"),
        digest_of(&resources, "pinned"),
    ];
    digests.sort();
    digests.dedup();
    assert_eq!(
        digests.len(),
        3,
        "an extension-only difference is a semantic difference"
    );

    product.runtime().shutdown().await.unwrap();
}

/// Issue #256 regression 8: a child resolved from R1 keeps its R1 extension
/// semantics after R2 publishes different Agent Status settings.
///
/// The interleaving is decided by two explicit channels: the resolving task
/// hands its frozen specification back the instant resolution completes, and
/// parks until this test has published R2 through the real
/// `reload_resources` boundary. Nothing waits on a clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ext256_a_child_frozen_on_r1_keeps_r1_extensions_after_r2_publishes() {
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({"roles": {"explore": {
        "description": "R1",
        "tools": {"builtin": ["read"]},
        "extensions": {
            "agent_status": {
                "enabled": true,
                "time": {"enabled": true, "timezone": "Asia/Shanghai"},
                "background": {"enabled": true}
            },
            // Issue #259 regression 11: the Todo extension crosses the same
            // freeze boundary, and R2 flips it the other way below.
            "todo": {"enabled": true}
        },
    }}}));
    let product = lab.compose().await;
    let r1 = product.runtime().runtime_resources();
    let r1_digest = digest_of(&r1, "explore");

    let (admitted_tx, admitted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let child = tokio::spawn(async move {
        let frozen = resolve(
            &r1,
            &agent("explore"),
            &inherited_model(),
            &model_registry(),
        )
        .expect("R1 resolves");
        admitted_tx.send(frozen.clone()).expect("hand back R1");
        release_rx.await.expect("R2 has published");
        // The frozen decision crosses its real serialization contract, the
        // same way it reaches a child process.
        serde_json::from_slice::<ResolvedSubagentSpec>(
            &serde_json::to_vec(&frozen).expect("encode"),
        )
        .expect("decode")
    });
    let frozen = admitted_rx.await.expect("R1 froze before R2");

    // R2 changes the role's extension composition entirely.
    lab.write_config(&serde_json::json!({"roles": {"explore": {
        "description": "R1",
        "tools": {"builtin": ["read"]},
        "extensions": {"agent_status": {"enabled": false}, "todo": {"enabled": false}},
    }}}));
    product
        .runtime()
        .reload_resources()
        .await
        .expect("R2 publishes");
    let r2 = product.runtime().runtime_resources();
    assert!(r2.revision().get() > 1);
    let next = resolve(
        &r2,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("R2 resolves");
    release_tx.send(()).expect("release the parked resolution");
    let retained = child.await.expect("resolution task");

    assert_eq!(retained, frozen, "the frozen decision is unchanged by R2");
    let agent_status = retained
        .extensions
        .agent_status()
        .expect("R1 composed Agent Status");
    assert_eq!(
        agent_status.time.timezone,
        Some(chrono_tz::Asia::Shanghai),
        "the R1-resolved child keeps its R1 extension semantics"
    );
    assert!(agent_status.background.enabled);
    assert!(
        retained.extensions.todo().is_some(),
        "the R1-resolved child keeps its R1 Todo composition after R2 removed it"
    );
    assert!(
        next.extensions.todo().is_none(),
        "and a child resolved after R2 composes no Todo"
    );
    assert_eq!(
        retained.definition_digest, r1_digest,
        "the committed child keeps the R1 semantic identity it started with"
    );
    assert!(
        next.extensions.is_empty(),
        "R2 is authoritative for children resolved after it"
    );
    assert_ne!(next.definition_digest, r1_digest);

    // Issue #256 regression 7: the Runtime Client effective-extension
    // projection of that committed child is R1's, taken after R2 published.
    // The projection has exactly one input — the frozen specification the
    // child carries — so a child staged from `retained` reports R1 while a
    // child staged from `next` reports absence.
    let projected = EffectiveNativeAgentExtensions::project(&retained.extensions);
    assert_eq!(
        projected
            .agent_status
            .expect("R1 composed Agent Status")
            .time
            .timezone,
        Some(chrono_tz::Asia::Shanghai),
        "the projection of a frozen child is its own R1 composition"
    );
    assert_eq!(
        EffectiveNativeAgentExtensions::project(&next.extensions),
        EffectiveNativeAgentExtensions {
            goal: None,
            agent_status: None,
            todo: None,
        },
        "and a child frozen on R2 projects R2's composition, not R1's"
    );
    assert_ne!(
        EffectiveNativeAgentExtensions::project(&retained.extensions),
        EffectiveNativeAgentExtensions::project(&next.extensions)
    );

    product.runtime().shutdown().await.unwrap();
}
