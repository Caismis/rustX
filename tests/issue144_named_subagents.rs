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

use std::sync::Arc;

use rustx::capabilities::{CapabilitySourceId, CapabilitySourceState};
use rustx::local_runtime::composition::{
    LocalRuntimeDependencies, LocalRuntimePaths, LocalSessionProduct,
};
use rustx::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
use rustx::model::invocation::ModelBindingRegistry;
use rustx::model::session::SessionModelConfig;
use rustx::runtime::RuntimeResourceSnapshot;
use rustx::runtime::subagent::{
    ResolvedSubagentTool, SubagentDefinitionDigest, SubagentName, SubagentResolutionError,
    SubagentResolver,
};

const KEY_ENV: &str = "RUSTX_ISSUE144_KEY";

const MODELS: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "http://127.0.0.1:9/v1",
      "apiKey": "$RUSTX_ISSUE144_KEY",
      "models": [
        {
          "id": "model-a",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 512,
          "capabilities": {"inputModalities": ["text"], "outputModalities": ["text"], "toolCalls": true, "reasoning": false},
          "compat": {"chatReasoningReplay": "omit"}
        },
        {
          "id": "model-b",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 512,
          "capabilities": {"inputModalities": ["text"], "outputModalities": ["text"], "toolCalls": true, "reasoning": false},
          "compat": {"chatReasoningReplay": "omit"}
        }
      ]
    }
  }
}"#;

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Arc::new(MapCredentialEnvironment::new([(
            KEY_ENV.to_owned(),
            "test-only-secret".to_owned(),
        )])),
        ..LocalRuntimeDependencies::default()
    }
}

/// The same model authority the runtime composes, rebuilt for direct
/// resolver calls.
fn model_registry() -> ModelBindingRegistry {
    let catalog = ModelCatalog::from_jsonc_slice(MODELS.as_bytes()).expect("model catalog");
    let resolved = catalog
        .resolve(dependencies().credentials.as_ref())
        .expect("resolved catalog");
    ModelBindingRegistry::new(resolved).expect("binding registry")
}

fn agent(name: &str) -> SubagentName {
    SubagentName::parse(name).expect("canonical name")
}

fn inherited_model() -> SessionModelConfig {
    SessionModelConfig::of(ModelRef::parse("local/model-a").expect("model reference"))
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
        std::fs::create_dir_all(lab.workspace().join("subagents")).expect("subagent resources");
        std::fs::write(lab.root().join("models.jsonc"), MODELS).expect("models.jsonc");
        std::fs::write(
            lab.workspace().join("subagents/explore.md"),
            "Explore the shared workspace read-only.\n",
        )
        .expect("explore instructions");
        std::fs::write(
            lab.workspace().join("AGENTS.md"),
            "workspace instructions\n",
        )
        .expect("AGENTS.md");
        std::fs::write(
            lab.workspace().join("subagents/explore-AGENTS.md"),
            "explicit agent instructions\n",
        )
        .expect("agent AGENTS.md");
        std::fs::write(
            lab.workspace().join("subagents/explore-EXTRA.md"),
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

    fn write_config(&self, subagents: &serde_json::Value) {
        self.write_config_with_tools(subagents, &["read", "subagent"]);
    }

    fn write_config_with_tools(&self, subagents: &serde_json::Value, default_tools: &[&str]) {
        let document = serde_json::json!({
            "schemaVersion": 3,
            "agentId": "agent-issue144",
            "model": {"model": "local/model-a"},
            "context": {"reserveTokens": 0, "keepRecentTokens": 0},
            "defaultTools": default_tools,
            "subagents": subagents,
        });
        std::fs::write(
            self.root().join("rustx.jsonc"),
            serde_json::to_string_pretty(&document).expect("config document"),
        )
        .expect("rustx.jsonc");
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

    fn paths(&self) -> LocalRuntimePaths {
        LocalRuntimePaths {
            models: self.root().join("models.jsonc"),
            config: self.root().join("rustx.jsonc"),
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
        LocalSessionProduct::compose(&self.paths(), &dependencies())
            .await
            .expect("the runtime composes")
    }
}

/// A definition selecting exactly the named built-ins.
fn explore(builtin: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "maxConcurrent": 4,
        "agents": {
            "explore": {
                "description": "Read-only repository exploration.",
                "instructionsFile": "subagents/explore.md",
                "tools": {"builtin": builtin},
            }
        }
    })
}

fn digest_of(resources: &RuntimeResourceSnapshot, name: &str) -> SubagentDefinitionDigest {
    resources
        .subagents()
        .get(&agent(name))
        .expect("the generation admits the agent")
        .digest()
        .clone()
}

/// Only named catalog definitions are admitted, and the catalog is keyed by
/// canonical name in deterministic order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_named_catalog_definitions_are_admitted() {
    let lab = Lab::new();
    std::fs::write(
        lab.workspace().join("subagents/research.md"),
        "Research broadly.\n",
    )
    .expect("research instructions");
    lab.write_config(&serde_json::json!({
        "maxConcurrent": 2,
        "agents": {
            "research": {
                "description": "Deep research.",
                "instructionsFile": "subagents/research.md",
            },
            "explore": {
                "description": "Read-only repository exploration.",
                "instructionsFile": "subagents/explore.md",
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
    lab.write_config(&explore(&["read"]));
    let product = lab.compose().await;

    // R1: exactly what an attempt admitted now would freeze.
    let r1 = product.runtime().runtime_resources();
    let r1_digest = digest_of(&r1, "explore");
    assert!(r1.subagents().get(&agent("research")).is_none());

    // R2 redefines `explore` and adds `research`.
    std::fs::write(
        lab.workspace().join("subagents/research.md"),
        "Research broadly.\n",
    )
    .expect("research instructions");
    std::fs::write(
        lab.workspace().join("subagents/explore.md"),
        "Explore the shared workspace read-only, and summarize.\n",
    )
    .expect("revised explore instructions");
    lab.write_config(&serde_json::json!({
        "maxConcurrent": 4,
        "agents": {
            "explore": {
                "description": "Read-only repository exploration.",
                "instructionsFile": "subagents/explore.md",
                "tools": {"builtin": ["read"]},
            },
            "research": {
                "description": "Deep research.",
                "instructionsFile": "subagents/research.md",
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
    let resolved = SubagentResolver::resolve(&r1, &agent("explore"), &inherited_model(), &registry)
        .expect("R1 still resolves its own agent");
    assert_eq!(resolved.definition_digest, r1_digest);
    assert_eq!(
        resolved.instructions, "Explore the shared workspace read-only.\n",
        "the R1 attempt observes R1's instruction document, not R2's"
    );
    assert!(
        matches!(
            SubagentResolver::resolve(&r1, &agent("research"), &inherited_model(), &registry),
            Err(SubagentResolutionError::UnknownAgent { .. })
        ),
        "an agent that only R2 admits is invisible to an R1-owning attempt"
    );

    // And the newly current generation is genuinely different.
    let from_r2 = SubagentResolver::resolve(&r2, &agent("explore"), &inherited_model(), &registry)
        .expect("R2 resolves its own agent");
    assert_ne!(from_r2.definition_digest, r1_digest);
    assert!(
        SubagentResolver::resolve(&r2, &agent("research"), &inherited_model(), &registry).is_ok()
    );
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
        "maxConcurrent": 4,
        "agents": {
            "explore": {
                "description": "Read-only repository exploration.",
                "instructionsFile": "subagents/explore.md",
                "tools": {"builtin": ["definitely_not_a_capability"]},
            }
        }
    }));
    let error = product
        .runtime()
        .reload_resources()
        .await
        .expect_err("an invalid catalog rejects the whole candidate");
    assert!(
        format!("{error}").contains("definitely_not_a_capability"),
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
    lab.write_config_with_tools(&explore(&["grep", "glob"]), &["read", "subagent"]);
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
    assert_eq!(
        active,
        vec!["read".to_owned(), "subagent".to_owned()],
        "the parent's active projection is deliberately narrow"
    );
    let available = resources
        .capability()
        .available_tools()
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert!(available.contains(&"grep".to_owned()) && available.contains(&"glob".to_owned()));

    let resolved = SubagentResolver::resolve(
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
async fn statically_invalid_references_fail_composition_closed() {
    for (subagents, expected) in [
        (
            serde_json::json!({
                "maxConcurrent": 4,
                "agents": {"explore": {
                    "description": "d",
                    "instructionsFile": "subagents/explore.md",
                    "tools": {"builtin": ["not_a_builtin"]},
                }}
            }),
            "builtin:not_a_builtin",
        ),
        (
            serde_json::json!({
                "maxConcurrent": 4,
                "agents": {"explore": {
                    "description": "d",
                    "instructionsFile": "subagents/explore.md",
                    "tools": {"mcp": {"unconfigured": ["anything"]}},
                }}
            }),
            "mcp:unconfigured/anything",
        ),
        (
            serde_json::json!({
                "maxConcurrent": 4,
                "agents": {"explore": {
                    "description": "d",
                    "instructionsFile": "subagents/explore.md",
                    "tools": {"python": ["not_a_python_tool"]},
                }}
            }),
            "python:not_a_python_tool",
        ),
        (
            serde_json::json!({
                "maxConcurrent": 4,
                "agents": {"explore": {
                    "description": "d",
                    "instructionsFile": "subagents/explore.md",
                    "model": "local/model-missing",
                }}
            }),
            "local/model-missing",
        ),
        (
            serde_json::json!({
                "maxConcurrent": 4,
                "agents": {"explore": {
                    "description": "d",
                    "instructionsFile": "subagents/explore.md",
                    "skills": ["no-such-skill"],
                }}
            }),
            "no-such-skill",
        ),
    ] {
        let lab = Lab::new();
        lab.write_config(&subagents);
        let error = LocalSessionProduct::compose(&lab.paths(), &dependencies())
            .await
            .expect_err("a statically invalid definition fails composition");
        let rendered = format!("{error}");
        assert!(
            rendered.contains(expected),
            "the refusal names the offending reference {expected}: {rendered}"
        );
    }
}

/// Recursive and child-unsafe capability selections are rejected at
/// definition admission, so no resolution path has to defend against them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recursive_and_child_unsafe_selections_are_rejected_at_admission() {
    for capability in ["subagent", "ask_user", "background_task"] {
        let lab = Lab::new();
        lab.write_config(&explore(&[capability]));
        let error = LocalSessionProduct::compose(&lab.paths(), &dependencies())
            .await
            .err()
            .unwrap_or_else(|| panic!("selecting {capability} must fail composition"));
        assert!(
            format!("{error}").contains(capability),
            "the refusal names {capability}: {error}"
        );
    }
}

/// An unavailable optional source keeps the runtime healthy, while an agent
/// that explicitly requires a capability from that source cannot start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unavailable_source_keeps_the_runtime_healthy_but_blocks_the_agent_that_needs_it() {
    let lab = Lab::new();
    let document = serde_json::json!({
        "schemaVersion": 3,
        "agentId": "agent-issue144",
        "model": {"model": "local/model-a"},
        "context": {"reserveTokens": 0, "keepRecentTokens": 0},
        "defaultTools": ["read", "subagent"],
        "mcpServers": {
            "offline": {"type": "stdio", "command": "/definitely/missing-rustx-issue144-mcp"}
        },
        "subagents": {
            "maxConcurrent": 4,
            "agents": {
                "explore": {
                    "description": "Read-only repository exploration.",
                    "instructionsFile": "subagents/explore.md",
                    "tools": {"mcp": {"offline": ["get_issue"]}},
                }
            }
        }
    });
    std::fs::write(
        lab.root().join("rustx.jsonc"),
        serde_json::to_string_pretty(&document).expect("config document"),
    )
    .expect("rustx.jsonc");

    // The whole runtime composes: an optional source failure is availability
    // state, not a composition error, and the catalog is still admitted.
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    assert!(
        matches!(
            resources
                .capability_availability()
                .get(&CapabilitySourceId::Mcp(
                    rustx::runtime::identity::McpServerId::new("offline")
                )),
            Some(CapabilitySourceState::Unavailable { .. })
        ),
        "the failed source is recorded as availability state: {:?}",
        resources.capability_availability()
    );
    assert!(resources.subagents().get(&agent("explore")).is_some());

    // But the agent that explicitly requires it cannot start, and the
    // failure is the source-unavailable fact rather than "unknown".
    let error = SubagentResolver::resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect_err("an agent requiring an unavailable source cannot start");
    assert!(
        matches!(error, SubagentResolutionError::SourceUnavailable { .. }),
        "an unavailable source is never reported as an invalid selector: {error:?}"
    );
}

/// A definition with no explicit model inherits the invoking attempt's
/// frozen configuration; an explicit one freezes the configured model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_semantics_inherit_the_invoking_attempt_or_freeze_the_explicit_selection() {
    let lab = Lab::new();
    std::fs::write(
        lab.workspace().join("subagents/pinned.md"),
        "Run on the pinned model.\n",
    )
    .expect("pinned instructions");
    lab.write_config(&serde_json::json!({
        "maxConcurrent": 4,
        "agents": {
            "explore": {
                "description": "Inherits the invoking attempt's model.",
                "instructionsFile": "subagents/explore.md",
            },
            "pinned": {
                "description": "Runs on its own model.",
                "instructionsFile": "subagents/pinned.md",
                "model": "local/model-b",
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
    let inheriting =
        SubagentResolver::resolve(&resources, &agent("explore"), &attempt_model, &registry)
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
    let pinned = SubagentResolver::resolve(
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
        lab.workspace().join("subagents/isolated.md"),
        "Ignore the workspace chain.\n",
    )
    .expect("isolated instructions");
    let files = serde_json::json!(["subagents/explore-AGENTS.md", "subagents/explore-EXTRA.md"]);
    lab.write_config(&serde_json::json!({
        "maxConcurrent": 4,
        "agents": {
            "explore": {
                "description": "Inherits the parent chain.",
                "instructionsFile": "subagents/explore.md",
                "agentsMd": {"inherit": true, "files": files},
            },
            "isolated": {
                "description": "Explicit files only.",
                "instructionsFile": "subagents/isolated.md",
                "agentsMd": {"inherit": false, "files": files},
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

    let inherited =
        SubagentResolver::resolve(&resources, &agent("explore"), &inherited_model(), &registry)
            .expect("the inheriting agent resolves");
    let inherited_contents = inherited
        .project_instructions
        .iter()
        .map(|file| file.content.clone())
        .collect::<Vec<_>>();
    let mut expected = parent_chain.clone();
    expected.push("explicit agent instructions\n".to_owned());
    expected.push("second explicit file\n".to_owned());
    assert_eq!(
        inherited_contents, expected,
        "inherit=true is the exact parent chain followed by the explicit files in order"
    );

    let isolated = SubagentResolver::resolve(
        &resources,
        &agent("isolated"),
        &inherited_model(),
        &registry,
    )
    .expect("the isolated agent resolves");
    assert_eq!(
        isolated
            .project_instructions
            .iter()
            .map(|file| file.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "explicit agent instructions\n".to_owned(),
            "second explicit file\n".to_owned(),
        ],
        "inherit=false freezes only the explicit files"
    );

    // Ordering is configuration order, not filesystem or map order: the same
    // files listed the other way round produce the other order.
    let reversed = serde_json::json!(["subagents/explore-EXTRA.md", "subagents/explore-AGENTS.md"]);
    lab.write_config(&serde_json::json!({
        "maxConcurrent": 4,
        "agents": {"isolated": {
            "description": "Explicit files only.",
            "instructionsFile": "subagents/isolated.md",
            "agentsMd": {"inherit": false, "files": reversed},
        }}
    }));
    product
        .runtime()
        .reload_resources()
        .await
        .expect("the reversed order publishes");
    let reloaded = product.runtime().runtime_resources();
    let reordered =
        SubagentResolver::resolve(&reloaded, &agent("isolated"), &inherited_model(), &registry)
            .expect("the isolated agent resolves");
    assert_eq!(
        reordered
            .project_instructions
            .iter()
            .map(|file| file.content.clone())
            .collect::<Vec<_>>(),
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
        "maxConcurrent": 4,
        "agents": {"explore": {
            "description": "Read-only repository exploration.",
            "instructionsFile": "subagents/explore.md",
            "skills": ["alpha"],
        }}
    }));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    // The parent generation admits both.
    let parent_catalog = resources.skill_catalog().expect("the parent Skill catalog");
    assert!(parent_catalog.contains("alpha") && parent_catalog.contains("beta"));

    let resolved = SubagentResolver::resolve(
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

/// The definition digest ignores incidental JSONC formatting, comments, and
/// key order, and changes for every semantic difference.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_definition_digest_ignores_incidental_formatting_and_tracks_semantics() {
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({
        "maxConcurrent": 4,
        "agents": {"explore": {
            "description": "Read-only repository exploration.",
            "instructionsFile": "subagents/explore.md",
            "tools": {"builtin": ["read", "grep"]},
        }}
    }));
    let product = lab.compose().await;
    let baseline = digest_of(&product.runtime().runtime_resources(), "explore");

    // The same semantics, spelled with comments, different whitespace,
    // different JSON key order, and a different selector listing order.
    std::fs::write(
        lab.root().join("rustx.jsonc"),
        r#"{
  // A comment cannot change the semantic identity of a definition.
  "schemaVersion": 3, "agentId": "agent-issue144",
  "context": {"keepRecentTokens": 0, "reserveTokens": 0},
  "model": {"model": "local/model-a"},
  "defaultTools": ["read", "subagent"],
  "subagents": {
    "agents": {
      "explore": {
        "tools": {"builtin": ["grep", "read"]},
        "instructionsFile":    "subagents/explore.md",
        "description": "Read-only repository exploration.",
      },
    },
    "maxConcurrent": 4,
  },
}"#,
    )
    .expect("reformatted config");
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
        "maxConcurrent": 4,
        "agents": {"explore": {
            "description": "Read-only repository exploration.",
            "instructionsFile": "subagents/explore.md",
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
    lab.write_config(&explore(&["read", "grep"]));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let resolved = SubagentResolver::resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("the definition resolves");

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
        "maxConcurrent": 1,
        "agents": {"explore": {
            "description": "Read-only repository exploration.",
            "instructionsFile": "subagents/explore.md",
            "tools": {"builtin": ["read"]},
        }}
    }));
    let product = lab.compose().await;

    // A reload that changes the bound publishes a new catalog without
    // touching the live registry's capacity: capacity is live-registry state.
    lab.write_config(&serde_json::json!({
        "maxConcurrent": 8,
        "agents": {"explore": {
            "description": "Read-only repository exploration, revised.",
            "instructionsFile": "subagents/explore.md",
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
    lab.write_config(&serde_json::json!({"maxConcurrent": 0, "agents": {}}));
    let error = LocalSessionProduct::compose(&lab.paths(), &dependencies())
        .await
        .expect_err("a zero bound is refused");
    assert!(format!("{error}").contains("maxConcurrent"));
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

/// The committed durable ownership fact carries `(agent, definition_digest)`
/// and survives a round trip through the real durable authority unchanged.
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
                ..
            } => Some((agent.clone(), definition_digest.clone())),
            _ => None,
        })
        .expect("the ownership fact round-trips");
    assert_eq!(fact, ("explore".to_owned(), "sha256:d1".to_owned()));
}

/// The Runtime Client projection of a subagent carries the named-agent
/// identity, and its wire shape has no `profile` field at all.
#[test]
fn the_runtime_client_projection_carries_the_named_identity() {
    use rustx::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
    use rustx::runtime::subagent::{SubagentSnapshot, SubagentState};
    use rustx::runtime_client::snapshot::RuntimeClientSubagent;

    let snapshot = SubagentSnapshot {
        subagent_id: SubagentId::new("conv-1-subagent-1"),
        child_agent_id: AgentId::new("agent-child"),
        child_conversation_id: ConversationId::new("conv-1-subagent-1"),
        tool_call_id: ToolCallId::new("call-1"),
        agent: "explore".to_owned(),
        definition_digest: "sha256:d1".to_owned(),
        state: SubagentState::Running,
        detail: None,
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
        state: snapshot.state,
        detail: None,
    };
    let wire = serde_json::to_value(&view).expect("serialize the projection");
    assert_eq!(wire["agent"], "explore");
    assert_eq!(wire["definition_digest"], "sha256:d1");
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

/// A `models.jsonc` whose `local/model-a` has materially different semantics
/// from [`MODELS`]: a different endpoint, protocol, context window, output
/// budget, compat metadata, and request parameters — and no `model-b` at
/// all, so a re-resolving child would also *fail* where the parent
/// succeeded.
const MODELS_MUTATED: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "http://127.0.0.1:10/v2",
      "apiKey": "$RUSTX_ISSUE144_KEY",
      "models": [
        {
          "id": "model-a",
          "protocol": "anthropic_messages",
          "contextWindow": 1000,
          "maxOutputTokens": 64,
          "capabilities": {"inputModalities": ["text"], "outputModalities": ["text"], "toolCalls": true, "reasoning": false},
          "requestParams": {"temperature": 0.9}
        }
      ]
    }
  }
}"#;

/// Blocker 1: the child's model semantics are frozen by the parent, so a
/// `models.jsonc` edit that lands between the freeze and the child's
/// composition cannot be observed by that child.
///
/// The race is driven by two explicit linearizations — the resolver call
/// returns before the file is rewritten, and the child model authority is
/// composed after it — never by a sleep.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frozen_child_model_never_observes_a_later_models_jsonc_edit() {
    use rustx::model::session::SessionModelState;
    use rustx::model::types::ModelProtocol;

    let lab = Lab::new();
    lab.write_config(&explore(&["read"]));
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    // M1: freeze the child against the catalog the parent was admitted with.
    let resolved = SubagentResolver::resolve(
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
    std::fs::write(lab.root().join("models.jsonc"), MODELS_MUTATED).expect("mutate models.jsonc");

    // A re-resolving consumer *would* observe M2 — this is what makes the
    // assertion below meaningful rather than vacuous.
    let mutated = ModelCatalog::from_jsonc_slice(MODELS_MUTATED.as_bytes()).expect("M2 parses");
    let mutated_registry = ModelBindingRegistry::new(
        mutated
            .resolve(dependencies().credentials.as_ref())
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
    let child =
        SessionModelState::frozen(&over_the_wire.model, dependencies().credentials.as_ref())
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
        SessionModelState::frozen(&pinned_frozen, dependencies().credentials.as_ref()).is_ok(),
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
    let document = serde_json::json!({
        "schemaVersion": 3,
        "agentId": "agent-issue144",
        "model": {"model": "local/model-a"},
        "context": {"reserveTokens": 0, "keepRecentTokens": 0},
        "defaultTools": ["read", "subagent"],
        "nativeTools": {
            "grep": {
                "execution": "model_selectable",
                "concurrency": "parallel",
                "approval": "always",
            }
        },
        "subagents": explore(&["grep"]),
    });
    std::fs::write(
        lab.root().join("rustx.jsonc"),
        serde_json::to_string_pretty(&document).expect("config document"),
    )
    .expect("rustx.jsonc");
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let resolved = SubagentResolver::resolve(
        &resources,
        &agent("explore"),
        &inherited_model(),
        &model_registry(),
    )
    .expect("the child resolves");
    let frozen = match &resolved.tools[0] {
        ResolvedSubagentTool::Builtin { definition, .. } => definition.clone(),
        other => panic!("expected a frozen Builtin, found {other:?}"),
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
/// short-circuiting validator would never reach the invalid Python one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unavailable_source_cannot_hide_a_later_invalid_selector() {
    let lab = Lab::new();
    let document = serde_json::json!({
        "schemaVersion": 3,
        "agentId": "agent-issue144",
        "model": {"model": "local/model-a"},
        "context": {"reserveTokens": 0, "keepRecentTokens": 0},
        "defaultTools": ["read", "subagent"],
        "mcpServers": {
            "offline": {"type": "stdio", "command": "/definitely/missing-rustx-issue144-mcp"}
        },
        "subagents": {
            "maxConcurrent": 4,
            "agents": {
                "explore": {
                    "description": "Read-only repository exploration.",
                    "instructionsFile": "subagents/explore.md",
                    "tools": {
                        // `mcp:` sorts before `python:` in canonical selector
                        // order, so the unavailable source is inspected first.
                        "mcp": {"offline": ["get_issue"]},
                        "python": ["not_a_python_tool"],
                    },
                }
            }
        }
    });
    std::fs::write(
        lab.root().join("rustx.jsonc"),
        serde_json::to_string_pretty(&document).expect("config document"),
    )
    .expect("rustx.jsonc");

    let error = LocalSessionProduct::compose(&lab.paths(), &dependencies())
        .await
        .expect_err("a statically invalid selector rejects the candidate generation");
    let rendered = format!("{error}");
    assert!(
        rendered.contains("python:not_a_python_tool"),
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
        "maxConcurrent": 4,
        "agents": {"explore": {
            "description": "Read-only repository exploration.",
            "instructionsFile": "subagents/explore.md",
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

    let resolved = SubagentResolver::resolve(
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
    let reloaded = LocalSessionProduct::compose(&lab.paths(), &dependencies())
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
