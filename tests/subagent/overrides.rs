//! Issue #258: invocation-scoped tools, Skills, and extension overrides.
//!
//! The named definition is the canonical **default** child profile; a single
//! invocation may replace selected capability dimensions before child
//! ownership commits. This suite owns the contract at the boundary that
//! decides it — the shared resolver, driven against real composed runtime
//! generations — rather than at a serde helper.
//!
//! Complementary coverage lives at the other real owners and is deliberately
//! not duplicated here:
//!
//! - `rustx::runtime::subagent::resolver` unit tests own the delegation
//!   ceiling's exact-identity algebra (role ∪ parent, per dimension, by
//!   `ToolId`), because the identities under test are the point;
//! - `rustx::tools::native::subagent` unit tests own the pre-staging refusal
//!   at the real model-facing Tool, against a real registry;
//! - `rustx::runtime::subagent::invocation` unit tests own the wire
//!   vocabulary (missing vs empty vs null, closed dimensions).
//!
//! Every generation-ordering assertion here is decided by an explicit
//! linearization the test drives — a completed `reload_resources`, or a typed
//! refusal — never by a sleep.

use crate::launch_fixture::LaunchFixture;
use std::sync::Arc;

use rustx::extensions::{NativeAgentExtensionSelection, NativeAgentExtensions};
use rustx::local_runtime::composition::{LocalRuntimeDependencies, LocalSessionProduct};
use rustx::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
use rustx::model::invocation::ModelBindingRegistry;
use rustx::model::session::SessionModelConfig;
use rustx::runtime::RuntimeResourceSnapshot;
use rustx::runtime::subagent::{
    InvokingAgentAuthority, ResolvedSubagentSpec, SubagentDomain, SubagentInvocationOverride,
    SubagentName, SubagentOverrideAuthority, SubagentResolution, SubagentResolutionError,
    SubagentResolver,
};

const KEY_ENV: &str = "RUSTX_ISSUE258_KEY";

const MODELS: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "http://127.0.0.1:9/v1",
      "apiKey": "$RUSTX_ISSUE258_KEY",
      "models": [
        {
          "id": "model-a",
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
        credentials: Some(Arc::new(MapCredentialEnvironment::new([(
            KEY_ENV.to_owned(),
            "test-only-secret".to_owned(),
        )]))),
        ..LocalRuntimeDependencies::default()
    }
}

fn model_registry() -> ModelBindingRegistry {
    let catalog = ModelCatalog::from_jsonc_slice(MODELS.as_bytes()).expect("model catalog");
    let resolved = catalog
        .resolve(dependencies().credentials.as_deref().unwrap())
        .expect("resolved catalog");
    ModelBindingRegistry::new(resolved).expect("binding registry")
}

fn agent(name: &str) -> SubagentName {
    SubagentName::parse(name).expect("canonical name")
}

fn attempt_model() -> SessionModelConfig {
    SessionModelConfig::of(ModelRef::parse("local/model-a").expect("model reference"))
}

/// The invoking Agent's frozen admitted execution profile, captured from the
/// generation the attempt owns — exactly what production captures at attempt
/// admission.
fn parent_of(resources: &RuntimeResourceSnapshot) -> InvokingAgentAuthority {
    InvokingAgentAuthority::frozen(resources.capability(), NativeAgentExtensions::none())
}

fn parent_with_extensions(
    resources: &RuntimeResourceSnapshot,
    extensions: NativeAgentExtensions,
) -> InvokingAgentAuthority {
    InvokingAgentAuthority::frozen(resources.capability(), extensions)
}

/// One main-model (dynamic delegation) resolution.
fn delegate(
    resources: &RuntimeResourceSnapshot,
    name: &str,
    invocation: Option<&SubagentInvocationOverride>,
    parent: &InvokingAgentAuthority,
) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
    SubagentResolver::resolve(&SubagentResolution {
        resources,
        agent: &agent(name),
        attempt_model: &attempt_model(),
        models: &model_registry(),
        domain: SubagentDomain::Main,
        invocation,
        authority: SubagentOverrideAuthority::DelegatedByModel,
        invoking: parent,
    })
}

/// One Workflow (trusted static program) resolution.
fn workflow_resolve(
    resources: &RuntimeResourceSnapshot,
    name: &str,
    invocation: Option<&SubagentInvocationOverride>,
) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
    SubagentResolver::resolve(&SubagentResolution {
        resources,
        agent: &agent(name),
        attempt_model: &attempt_model(),
        models: &model_registry(),
        domain: SubagentDomain::Workflow,
        invocation,
        authority: SubagentOverrideAuthority::TrustedProgram,
        // A trusted static override is not bounded by the invoking main
        // model's narrower profile, so the parent contributes nothing here.
        invoking: &InvokingAgentAuthority::none(),
    })
}

fn parse_override(value: serde_json::Value) -> SubagentInvocationOverride {
    serde_json::from_value(value).expect("the override parses")
}

fn tool_names(spec: &ResolvedSubagentSpec) -> Vec<String> {
    spec.tools
        .iter()
        .map(rustx::runtime::subagent::ResolvedSubagentTool::canonical)
        .collect::<Vec<_>>()
}

fn skill_names(spec: &ResolvedSubagentSpec) -> Vec<String> {
    spec.skills
        .iter()
        .map(|skill| skill.catalog_entry.name.clone())
        .collect::<Vec<_>>()
}

/// One temporary world: models, workspace, roles, Skills, and Workflows.
struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    fn new() -> Self {
        let lab = Self {
            dir: tempfile::tempdir().expect("lab directory"),
        };
        std::fs::create_dir_all(lab.workspace().join(".agents/subagents")).expect("role directory");
        std::fs::create_dir_all(lab.workspace().join(".agents/workflows"))
            .expect("workflow directory");
        std::fs::write(lab.root().join("models.jsonc"), MODELS).expect("models.jsonc");
        std::fs::write(lab.workspace().join("AGENTS.md"), "workspace guidance\n")
            .expect("AGENTS.md");
        lab
    }

    fn root(&self) -> &std::path::Path {
        self.dir.path()
    }

    fn workspace(&self) -> std::path::PathBuf {
        self.root().join("workspace")
    }

    fn write_skill(&self, name: &str, description: &str, body: &str) {
        let directory = self.workspace().join(".agents/skills").join(name);
        std::fs::create_dir_all(&directory).expect("skill directory");
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
        )
        .expect("SKILL.md");
    }

    fn write_workflow(&self, id: &str, yaml: &str) {
        std::fs::write(
            self.workspace()
                .join(".agents/workflows")
                .join(format!("{id}.yaml")),
            yaml,
        )
        .expect("workflow yaml");
    }

    /// Writes one complete configuration.
    ///
    /// `default_tools` is the **invoking model's** frozen model-facing
    /// selection, which is deliberately narrower than the generation's
    /// available catalog: the difference between the two is what makes
    /// "generation-only authority" a real, testable state.
    fn write_config(&self, roles: &serde_json::Value, default_tools: &[&str], workflows: &[&str]) {
        let mut subagents = serde_json::json!({"maxConcurrent": 4, "roles": roles});
        let names = subagents["roles"]
            .as_object()
            .expect("roles")
            .keys()
            .map(|name| serde_json::Value::String(name.clone()))
            .collect::<Vec<_>>();
        subagents["main"] = serde_json::Value::Array(names.clone());
        subagents["workflow"] = serde_json::Value::Array(names);
        crate::launch_fixture::write_roles(&self.workspace(), &mut subagents);
        let document = serde_json::json!({
            "schemaVersion": 8,
            "agentId": "agent-issue258",
            "model": {"model": "local/model-a"},
            "context": {"reserveTokens": 0, "keepRecentTokens": 0},
            "defaultTools": default_tools,
            "subagents": subagents,
            "workflows": {
                "definitions": workflows,
                "main": [],
            },
        });
        std::fs::write(
            self.root().join("rustx.jsonc"),
            serde_json::to_string_pretty(&document).expect("config document"),
        )
        .expect("rustx.jsonc");
    }

    fn paths(&self) -> LaunchFixture {
        LaunchFixture {
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
        LocalSessionProduct::compose(&(self.paths()).resolve(), &dependencies())
            .await
            .expect("the runtime composes")
    }

    /// The offline diagnostic of a statically invalid configuration.
    ///
    /// Offline launch resolution is the owner of statically knowable
    /// reference validation, so an invalid Workflow Agent override is refused
    /// here — before any runtime, provider, MCP connection, Python
    /// environment, process, Session, or worktree exists.
    fn offline_error(&self) -> String {
        self.paths()
            .try_resolve()
            .expect_err("offline inspection rejects the configuration")
    }
}

/// The role every replacement test specializes: read-only, one Skill, and the
/// authored extension defaults (which compose Agent Status).
fn reviewer_roles() -> serde_json::Value {
    serde_json::json!({
        "reviewer": {
            "description": "Review one bounded change.",
            "tools": {"builtin": ["read"]},
            "skills": ["review-guidance"],
        }
    })
}

// =====================================================================
// A. Replacement and isolation
// =====================================================================

/// Omitting the override, and supplying an explicitly empty one, must both
/// reproduce the definition's profile exactly — including its extension
/// composition, which is precisely where a default-bearing document would
/// otherwise leak through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_no_override_and_an_empty_override_both_reproduce_the_definition() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    let none = delegate(&resources, "reviewer", None, &parent).expect("defaults resolve");
    let empty = parse_override(serde_json::json!({}));
    assert!(empty.is_empty());
    let explicit_empty =
        delegate(&resources, "reviewer", Some(&empty), &parent).expect("defaults resolve");

    assert_eq!(tool_names(&none), vec!["builtin:read".to_owned()]);
    assert_eq!(skill_names(&none), vec!["review-guidance".to_owned()]);
    assert!(
        none.extensions.agent_status().is_some(),
        "the role's authored extension defaults compose Agent Status"
    );
    assert_eq!(
        none, explicit_empty,
        "an empty override object is exactly equivalent to omitting it"
    );
    assert_eq!(
        none.profile_digest(),
        explicit_empty.profile_digest(),
        "and the two are one effective execution profile"
    );
}

/// Each present dimension replaces its whole dimension; each missing one
/// still comes from the definition. Proven per dimension and in combination.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_each_present_dimension_replaces_and_missing_dimensions_inherit() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_skill("security-review", "How to review security", "security body");
    lab.write_config(&reviewer_roles(), &["read", "grep", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    // Tools only.
    let tools_only = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(
            serde_json::json!({"tools": {"builtin": ["grep"]}}),
        )),
        &parent,
    )
    .expect("a parent-held capability is delegable");
    assert_eq!(
        tool_names(&tools_only),
        vec!["builtin:grep".to_owned()],
        "the role's read is replaced, not unioned with grep"
    );
    assert_eq!(
        skill_names(&tools_only),
        vec!["review-guidance".to_owned()],
        "the missing Skill dimension still comes from the definition"
    );
    assert!(
        tools_only.extensions.agent_status().is_some(),
        "the missing extension dimension still comes from the definition"
    );

    // Skills only.
    let skills_only = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(
            serde_json::json!({"skills": ["security-review"]}),
        )),
        &parent,
    )
    .expect("a Skill the parent can see is delegable");
    assert_eq!(
        skill_names(&skills_only),
        vec!["security-review".to_owned()],
        "the role's Skill is replaced, not unioned"
    );
    assert_eq!(tool_names(&skills_only), vec!["builtin:read".to_owned()]);

    // All three at once, resolved independently.
    let combined = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({
            "tools": {"builtin": ["grep"]},
            "skills": ["security-review"],
            "extensions": {},
        }))),
        &parent,
    )
    .expect("every dimension is independently authorized");
    assert_eq!(tool_names(&combined), vec!["builtin:grep".to_owned()]);
    assert_eq!(skill_names(&combined), vec!["security-review".to_owned()]);
    assert_eq!(combined.extensions, NativeAgentExtensions::none());
}

/// The documented empty meanings, including the extension-default trap: the
/// authored role document composes Agent Status by default, and an explicit
/// empty extension override must not resurrect it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_explicit_empty_dimensions_have_their_documented_meaning() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    let cleared = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({
            "tools": {},
            "skills": [],
            "extensions": {},
        }))),
        &parent,
    )
    .expect("clearing every dimension is narrowing and always authorized");
    assert!(
        cleared.tools.is_empty(),
        "explicit empty tools means no tools"
    );
    assert!(
        cleared.skills.is_empty(),
        "explicit empty skills means no Skills"
    );
    assert_eq!(
        cleared.extensions,
        NativeAgentExtensions::none(),
        "explicit empty extensions means no composed extension"
    );
    assert!(
        cleared.materialization.is_empty(),
        "an empty selection requires no external materialization"
    );

    // The default really is non-empty, so the assertion above is meaningful.
    let defaults = delegate(&resources, "reviewer", None, &parent).expect("defaults resolve");
    assert!(defaults.extensions.agent_status().is_some());
    assert_ne!(cleared.profile_digest(), defaults.profile_digest());
}

/// Two invocations of one role with different overrides are independent, and
/// neither mutates the shared definition, catalog, or generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_concurrent_invocations_do_not_mutate_each_other_or_the_role() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "grep", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    let role_before = resources
        .subagents()
        .get(&agent("reviewer"))
        .expect("the generation admits the role")
        .clone();

    let (first, second) = tokio::join!(
        {
            let resources = Arc::clone(&resources);
            let parent = parent.clone();
            async move {
                delegate(
                    &resources,
                    "reviewer",
                    Some(&parse_override(
                        serde_json::json!({"tools": {"builtin": ["grep"]}}),
                    )),
                    &parent,
                )
                .expect("first invocation")
            }
        },
        {
            let resources = Arc::clone(&resources);
            let parent = parent.clone();
            async move {
                delegate(
                    &resources,
                    "reviewer",
                    Some(&parse_override(serde_json::json!({"tools": {}}))),
                    &parent,
                )
                .expect("second invocation")
            }
        }
    );

    assert_eq!(tool_names(&first), vec!["builtin:grep".to_owned()]);
    assert!(second.tools.is_empty());
    assert_ne!(first.profile_digest(), second.profile_digest());
    assert_eq!(
        first.definition_digest, second.definition_digest,
        "both children came from one source definition"
    );

    let role_after = resources
        .subagents()
        .get(&agent("reviewer"))
        .expect("the generation still admits the role");
    assert_eq!(
        &role_before, role_after,
        "the shared definition is untouched"
    );
    assert_eq!(
        delegate(&resources, "reviewer", None, &parent)
            .expect("the defaults still resolve")
            .profile_digest(),
        delegate(&resources, "reviewer", None, &parent)
            .expect("the defaults still resolve")
            .profile_digest(),
        "resolution is a pure projection of the generation"
    );
}

// =====================================================================
// B. Dynamic delegation
// =====================================================================

/// The ceiling end to end against a real generation: a role default the
/// parent does not hold, a parent capability the role does not hold, their
/// combination, and a capability only the generation knows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_the_delegation_ceiling_is_role_union_parent_and_nothing_more() {
    let lab = Lab::new();
    lab.write_config(
        &serde_json::json!({
            "reviewer": {
                "description": "Read-only reviewer.",
                "tools": {"builtin": ["read"]},
            },
            "builder": {
                "description": "A different role that holds bash.",
                "tools": {"builtin": ["bash"]},
            }
        }),
        // The invoking model holds grep and glob, and deliberately not read:
        // a role default must stay delegable regardless.
        &["grep", "glob", "subagent"],
        &[],
    );
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    assert!(
        !resources
            .capability()
            .tool_registry()
            .names()
            .contains(&"read"),
        "the invoking model really does not expose read"
    );
    assert!(
        resources
            .capability()
            .available_tools()
            .definitions()
            .iter()
            .any(|definition| definition.name == "write"),
        "the generation really does know write"
    );

    for (dimension, expected) in [
        (serde_json::json!({"builtin": ["read"]}), "builtin:read"),
        (serde_json::json!({"builtin": ["grep"]}), "builtin:grep"),
    ] {
        let resolved = delegate(
            &resources,
            "reviewer",
            Some(&parse_override(serde_json::json!({"tools": dimension}))),
            &parent,
        )
        .expect("authorized by the role or by the invoking model");
        assert_eq!(tool_names(&resolved), vec![expected.to_owned()]);
    }
    let both = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(
            serde_json::json!({"tools": {"builtin": ["read", "grep"]}}),
        )),
        &parent,
    )
    .expect("a valid combination of role and parent authority succeeds");
    assert_eq!(
        tool_names(&both),
        vec!["builtin:grep".to_owned(), "builtin:read".to_owned()]
    );

    // Generation-only authority.
    assert_eq!(
        delegate(
            &resources,
            "reviewer",
            Some(&parse_override(
                serde_json::json!({"tools": {"builtin": ["write"]}})
            )),
            &parent,
        ),
        Err(SubagentResolutionError::UnauthorizedTool {
            selector: "builtin:write".to_owned()
        }),
        "a capability only the generation knows is not delegable"
    );
    // Other-role-only authority.
    assert_eq!(
        delegate(
            &resources,
            "reviewer",
            Some(&parse_override(
                serde_json::json!({"tools": {"builtin": ["bash"]}})
            )),
            &parent,
        ),
        Err(SubagentResolutionError::UnauthorizedTool {
            selector: "builtin:bash".to_owned()
        }),
        "authority held only by another role is not delegable"
    );
    // ...while that other role's own default still resolves, so the refusal
    // above is about the caller and not about the capability.
    assert_eq!(
        tool_names(
            &delegate(&resources, "builder", None, &parent).expect("the role's own default")
        ),
        vec!["builtin:bash".to_owned()]
    );
}

/// Skill delegation is its own authorization domain, gated by the #234
/// visibility rule: a parent with no Read sees no Skills and can delegate
/// none, while the role's own Skills stay delegable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_skill_delegation_follows_frozen_model_visible_authority() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_skill("security-review", "How to review security", "security body");
    lab.write_config(&reviewer_roles(), &["read", "subagent"], &[]);
    let with_read = lab.compose().await;
    let seeing = with_read.runtime().runtime_resources();
    let seeing_parent = parent_of(&seeing);
    assert!(
        !seeing.capability().model_skill_entries().is_empty(),
        "a parent holding read sees the Skill catalog"
    );
    assert_eq!(
        skill_names(
            &delegate(
                &seeing,
                "reviewer",
                Some(&parse_override(
                    serde_json::json!({"skills": ["security-review"]})
                )),
                &seeing_parent,
            )
            .expect("a Skill the parent can see is delegable")
        ),
        vec!["security-review".to_owned()]
    );

    // The same generation, resolved for a caller whose frozen profile has no
    // Read: the Skill catalog is invisible to it, so it may delegate only the
    // role's own Skill.
    let blind = Lab::new();
    blind.write_skill("review-guidance", "How to review", "guidance body");
    blind.write_skill("security-review", "How to review security", "security body");
    blind.write_config(&reviewer_roles(), &["grep", "subagent"], &[]);
    let product = blind.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);
    assert!(
        resources.capability().model_skill_entries().is_empty(),
        "the #234 Read gate really does hide the catalog from this caller"
    );
    assert_eq!(
        skill_names(
            &delegate(
                &resources,
                "reviewer",
                Some(&parse_override(
                    serde_json::json!({"skills": ["review-guidance"]})
                )),
                &parent,
            )
            .expect("the role's own Skill stays delegable")
        ),
        vec!["review-guidance".to_owned()]
    );
    assert_eq!(
        delegate(
            &resources,
            "reviewer",
            Some(&parse_override(
                serde_json::json!({"skills": ["security-review"]})
            )),
            &parent,
        ),
        Err(SubagentResolutionError::UnauthorizedSkill {
            skill: "security-review".to_owned()
        }),
        "a Skill this caller cannot see is not delegable"
    );

    // Selecting a Skill grants no Tool: the effective tool set is exactly the
    // role's, and Read is not silently added.
    let resolved = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(
            serde_json::json!({"skills": ["review-guidance"]}),
        )),
        &parent,
    )
    .expect("resolution");
    assert_eq!(tool_names(&resolved), vec!["builtin:read".to_owned()]);
    let no_tools = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(
            serde_json::json!({"tools": {}, "skills": ["review-guidance"]}),
        )),
        &parent,
    )
    .expect("resolution");
    assert!(
        no_tools.tools.is_empty(),
        "selecting a Skill never implicitly grants Read or any other tool"
    );
    assert_eq!(skill_names(&no_tools), vec!["review-guidance".to_owned()]);
}

/// An unknown or hidden reference keeps its own failure class: it is never
/// reported as an authority verdict, and never silently dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_unknown_references_keep_their_own_failure_class() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    assert_eq!(
        delegate(
            &resources,
            "reviewer",
            Some(&parse_override(
                serde_json::json!({"tools": {"builtin": ["definitely_not_a_capability"]}})
            )),
            &parent,
        ),
        Err(SubagentResolutionError::UnknownCapability {
            selector: "builtin:definitely_not_a_capability".to_owned()
        })
    );
    assert_eq!(
        delegate(
            &resources,
            "reviewer",
            Some(&parse_override(
                serde_json::json!({"skills": ["not-a-skill"]})
            )),
            &parent,
        ),
        Err(SubagentResolutionError::UnknownSkill {
            skill: "not-a-skill".to_owned()
        })
    );
    // Structural child rules survive at the invocation boundary.
    for spelling in [
        serde_json::json!({"tools": {"builtin": ["subagent"]}}),
        serde_json::json!({"tools": {"builtin": ["execution"]}}),
    ] {
        assert!(
            matches!(
                delegate(
                    &resources,
                    "reviewer",
                    Some(&parse_override(spelling.clone())),
                    &parent
                ),
                Err(SubagentResolutionError::InvalidOverride { .. })
            ),
            "accepted {spelling}"
        );
    }
}

// =====================================================================
// C. Workflow
// =====================================================================

const OVERRIDE_WORKFLOW: &str = r"
description: A Workflow whose Agent node specializes the reviewer role.
timeout_ms: 60000
block:
  input: {type: object, properties: {}, required: [], additionalProperties: false}
  output:
    type: object
    properties: {verdict: {type: string}}
    required: [verdict]
    additionalProperties: false
  entry: work
  nodes:
    work:
      type: agent
      profile: reviewer
      task: Review the implementation.
      override:
        tools:
          builtin: [read, grep, bash]
        skills: [security-review]
        extensions:
          agentStatus:
            enabled: true
      output:
        type: object
        properties: {verdict: {type: string}}
        required: [verdict]
        additionalProperties: false
    done:
      type: return
      output: {type: reference, path: [work]}
  edges:
  - {from: work, to: done}
";

/// A trusted static override may legitimately exceed both the role's defaults
/// and the invoking main model's active capabilities, because the Workflow's
/// admitted generation authorizes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_a_trusted_workflow_override_exceeds_the_main_model_capabilities() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_skill("security-review", "How to review security", "security body");
    lab.write_workflow("specialized", OVERRIDE_WORKFLOW);
    // The invoking main model holds neither grep nor bash.
    lab.write_config(&reviewer_roles(), &["read", "subagent"], &["specialized"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    let node_override = parse_override(serde_json::json!({
        "tools": {"builtin": ["read", "grep", "bash"]},
        "skills": ["security-review"],
        "extensions": {"agentStatus": {"enabled": true}},
    }));

    let workflow = workflow_resolve(&resources, "reviewer", Some(&node_override))
        .expect("the Workflow generation authorizes the static override");
    assert_eq!(
        tool_names(&workflow),
        vec![
            "builtin:bash".to_owned(),
            "builtin:grep".to_owned(),
            "builtin:read".to_owned()
        ]
    );
    assert_eq!(skill_names(&workflow), vec!["security-review".to_owned()]);
    assert!(workflow.extensions.agent_status().is_some());

    // The same request from the main model is refused: the two authority
    // modes are genuinely different, and neither is reachable from the other.
    assert!(
        matches!(
            delegate(&resources, "reviewer", Some(&node_override), &parent),
            Err(SubagentResolutionError::UnauthorizedTool { .. })
        ),
        "the dynamic ceiling does not cover what the Workflow generation does"
    );

    // A trusted override still cannot exceed the admitted generation.
    assert_eq!(
        workflow_resolve(
            &resources,
            "reviewer",
            Some(&parse_override(
                serde_json::json!({"skills": ["not-a-skill"]})
            )),
        ),
        Err(SubagentResolutionError::UnknownSkill {
            skill: "not-a-skill".to_owned()
        })
    );
}

/// Equivalent authorized inputs resolve to one specification on both paths.
/// This is the "one resolver" property stated as an assertion rather than as
/// a comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_equivalent_workflow_and_tool_inputs_resolve_equivalently() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "grep", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    let requested = parse_override(serde_json::json!({
        "tools": {"builtin": ["grep", "read"]},
        "skills": ["review-guidance"],
    }));
    let from_tool = delegate(&resources, "reviewer", Some(&requested), &parent)
        .expect("the main model holds both capabilities");
    let from_workflow = workflow_resolve(&resources, "reviewer", Some(&requested))
        .expect("the Workflow generation authorizes the same request");
    assert_eq!(
        from_tool, from_workflow,
        "one resolver, one frozen specification"
    );
    assert_eq!(from_tool.profile_digest(), from_workflow.profile_digest());

    // A differently spelled but semantically identical request agrees too.
    let respelled = parse_override(serde_json::json!({
        "tools": {"builtin": ["read", "grep", "read"]},
        "skills": ["review-guidance", "review-guidance"],
    }));
    assert_eq!(
        delegate(&resources, "reviewer", Some(&respelled), &parent)
            .expect("resolution")
            .profile_digest(),
        from_tool.profile_digest(),
        "order and repetition are normalization, not identity"
    );
}

/// A Workflow Agent node's override is validated during resource preparation
/// against the same candidate generation that will publish it, with a precise
/// nested authored path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_a_nested_workflow_override_is_validated_with_a_precise_path() {
    let nested = r"
description: A Workflow whose nested Agent node carries an invalid override.
timeout_ms: 60000
block:
  input: {type: object, properties: {}, required: [], additionalProperties: false}
  output:
    type: object
    properties: {verdict: {type: string}}
    required: [verdict]
    additionalProperties: false
  entry: fan
  nodes:
    fan:
      type: parallel
      branches:
        only:
          input: {type: literal, value: {}}
          block:
            input: {type: object, properties: {}, required: [], additionalProperties: false}
            output:
              type: object
              properties: {verdict: {type: string}}
              required: [verdict]
              additionalProperties: false
            entry: work
            nodes:
              work:
                type: agent
                profile: reviewer
                task: Review the implementation.
                override:
                  tools:
                    builtin: [definitely_not_a_capability]
                output:
                  type: object
                  properties: {verdict: {type: string}}
                  required: [verdict]
                  additionalProperties: false
              done:
                type: return
                output: {type: reference, path: [work]}
            edges:
            - {from: work, to: done}
    done:
      type: return
      output: {type: reference, path: [fan, only]}
  edges:
  - {from: fan, to: done}
";
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_workflow("nested", nested);
    lab.write_config(&reviewer_roles(), &["read", "subagent"], &["nested"]);
    let error = lab.offline_error();
    assert!(
        error.contains("definitely_not_a_capability"),
        "the refusal names the offending selector: {error}"
    );
    assert!(
        error.contains("branches.only.block.nodes.work.override"),
        "the refusal names the precise nested authored path: {error}"
    );
    // Offline inspection is side-effect free: no runtime state was created
    // in order to produce that diagnostic.
    assert!(
        !lab.root().join("runtime").exists(),
        "static validation creates no runtime state"
    );
}

/// Structural child rules apply to a Workflow override at compilation, with
/// the offending dimension's authored path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_a_workflow_override_cannot_smuggle_nested_delegation() {
    let recursive =
        OVERRIDE_WORKFLOW.replace("builtin: [read, grep, bash]", "builtin: [read, subagent]");
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_skill("security-review", "How to review security", "security body");
    lab.write_workflow("specialized", &recursive);
    lab.write_config(&reviewer_roles(), &["read", "subagent"], &["specialized"]);
    let error = lab.offline_error();
    assert!(
        error.contains("nested subagent delegation is unsupported"),
        "structural child rules survive into Workflow authoring: {error}"
    );
    assert!(
        error.contains("override.tools"),
        "the diagnostic names the offending dimension: {error}"
    );
}

// =====================================================================
// D. Freeze, identity, and materialization
// =====================================================================

/// An R1-resolved specification and the R1 caller's authority both stay R1
/// after R2 publishes, and a later invocation resolves against its own
/// admitted generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_r1_resolution_and_authority_survive_r2_publication() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "R1 guidance");
    lab.write_config(&reviewer_roles(), &["read", "grep", "subagent"], &[]);
    let product = lab.compose().await;
    let r1 = product.runtime().runtime_resources();
    let r1_parent = parent_of(&r1);

    let requested = parse_override(serde_json::json!({
        "tools": {"builtin": ["grep"]},
        "skills": ["review-guidance"],
    }));
    let frozen = delegate(&r1, "reviewer", Some(&requested), &r1_parent).expect("R1 resolution");
    let frozen_digest = frozen.profile_digest();

    // R2 narrows the role, rewrites the Skill, and shrinks the model-facing
    // selection out from under the already-frozen R1 caller.
    lab.write_skill("review-guidance", "How to review", "R2 guidance");
    lab.write_config(
        &serde_json::json!({
            "reviewer": {
                "description": "Review one bounded change.",
                "tools": {"builtin": ["read"]},
                "skills": [],
            }
        }),
        &["read", "subagent"],
        &[],
    );
    product
        .runtime()
        .reload_resources()
        .await
        .expect("R2 publishes");
    let r2 = product.runtime().runtime_resources();
    assert!(r2.revision().get() > r1.revision().get());

    // The already-frozen specification is unchanged, and still resolves the
    // same way from its own generation.
    let again = delegate(&r1, "reviewer", Some(&requested), &r1_parent).expect("R1 still resolves");
    assert_eq!(again, frozen);
    assert_eq!(again.profile_digest(), frozen_digest);
    assert_eq!(
        frozen.skills[0].catalog_entry.description, "How to review",
        "the R1 child keeps the Skill binding R1 admitted"
    );

    // Frozen authority cannot be widened by live state: an R2 caller no
    // longer holds grep at all.
    let r2_parent = parent_of(&r2);
    assert_eq!(
        delegate(&r2, "reviewer", Some(&requested), &r2_parent),
        Err(SubagentResolutionError::UnauthorizedTool {
            selector: "builtin:grep".to_owned()
        }),
        "a later generation's caller no longer holds grep"
    );

    // Conversely, an R1-frozen authority does not travel *into* R2 either.
    // Its Tool identities are generation-independent, so grep stays
    // delegable...
    let tools_only = parse_override(serde_json::json!({"tools": {"builtin": ["grep"]}}));
    let in_r2 = delegate(&r2, "reviewer", Some(&tools_only), &r1_parent)
        .expect("an exact ToolId the R1 caller holds is still that capability");
    assert_eq!(tool_names(&in_r2), vec!["builtin:grep".to_owned()]);
    assert!(
        in_r2.skills.is_empty(),
        "resolution reads the generation it was given: R2 selects no Skill"
    );

    // ...while its Skill authority is *version*-exact, so the rewritten
    // package is a different identity that the R1 authority cannot cover.
    // A matching Skill name is not authorization.
    assert_eq!(
        delegate(&r2, "reviewer", Some(&requested), &r1_parent),
        Err(SubagentResolutionError::UnauthorizedSkill {
            skill: "review-guidance".to_owned()
        }),
        "frozen Skill authority names an exact version, not a name"
    );
    assert_ne!(
        in_r2.profile_digest(),
        frozen_digest,
        "a materially different effective profile has a different identity"
    );
}

/// A default that the invocation replaced away is not a dependency any more:
/// resolution requires only the effective selection's sources, and the role's
/// separate catalog admission is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_a_replaced_away_default_is_no_longer_a_requirement() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "grep", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    // Only the effective Skill is frozen for materialization; the role's
    // default Skill is absent from the specification entirely.
    let replaced = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({"skills": []}))),
        &parent,
    )
    .expect("resolution");
    assert!(
        replaced.skills.is_empty(),
        "a replaced-away Skill is not a materialization requirement"
    );
    assert!(replaced.materialization.is_empty());
    assert!(
        !serde_json::to_string(&replaced)
            .expect("serialize")
            .contains("guidance body"),
        "no Skill body ever enters the frozen specification"
    );

    // The role's own admission is unchanged: its default still resolves.
    assert_eq!(
        skill_names(&delegate(&resources, "reviewer", None, &parent).expect("defaults")),
        vec!["review-guidance".to_owned()]
    );
}

/// The effective digest's documented contract: materially different profiles
/// differ, equivalent profiles agree, and nothing incidental participates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_the_effective_profile_digest_follows_its_documented_contract() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_skill("security-review", "How to review security", "security body");
    lab.write_config(&reviewer_roles(), &["read", "grep", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    let profile = |value: Option<serde_json::Value>| {
        let requested = value.map(parse_override);
        delegate(&resources, "reviewer", requested.as_ref(), &parent)
            .expect("resolution")
            .profile_digest()
    };

    let defaults = profile(None);
    // Every dimension materially changes the identity.
    let distinct = [
        defaults.clone(),
        profile(Some(serde_json::json!({"tools": {"builtin": ["grep"]}}))),
        profile(Some(serde_json::json!({"tools": {}}))),
        profile(Some(serde_json::json!({"skills": ["security-review"]}))),
        profile(Some(serde_json::json!({"skills": []}))),
        profile(Some(serde_json::json!({"extensions": {}}))),
        profile(Some(serde_json::json!({
            "extensions": {"agentStatus": {"time": {"enabled": false}}}
        }))),
    ];
    let mut unique = distinct.to_vec();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        distinct.len(),
        "every materially different effective profile has its own identity"
    );

    // Equivalence: an explicit override reproducing the defaults, however it
    // is spelled, is the same profile.
    let role = resources
        .subagents()
        .get(&agent("reviewer"))
        .expect("the role");
    let restated = SubagentInvocationOverride {
        tools: Some(rustx::capabilities::selection::ToolSelectionDocument {
            builtin: vec!["read".to_owned(), "read".to_owned()],
            mcp: std::collections::BTreeMap::new(),
        }),
        skills: Some(vec!["review-guidance".to_owned()]),
        extensions: Some(NativeAgentExtensionSelection::of(role.extensions())),
    };
    assert_eq!(
        delegate(&resources, "reviewer", Some(&restated), &parent)
            .expect("resolution")
            .profile_digest(),
        defaults,
        "no override and an override restating the defaults are one profile"
    );

    // The identity is a value, not an authority token, and survives the wire.
    let frozen = delegate(&resources, "reviewer", None, &parent).expect("resolution");
    let over_the_wire: ResolvedSubagentSpec =
        serde_json::from_slice(&serde_json::to_vec(&frozen).expect("encode"))
            .expect("the frozen contract round-trips");
    assert_eq!(over_the_wire, frozen);
    assert_eq!(
        over_the_wire.profile_digest(),
        defaults,
        "the child recomputes the same identity from the same frozen bytes"
    );
    assert!(defaults.as_str().starts_with("sha256:"));
    assert!(
        !serde_json::to_string(&frozen)
            .expect("encode")
            .contains("test-only-secret"),
        "no credential material rides in the frozen specification"
    );
}

// =====================================================================
// E. Unchanged behavior
// =====================================================================

/// An extension-provided capability is governed by extension composition and
/// is not filtered by the ordinary tools allowlist — and, symmetrically, the
/// ordinary allowlist is not widened by a composed extension.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_extension_composition_is_not_an_ordinary_tool_selection() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_with_extensions(
        &resources,
        rustx::extensions::NativeAgentExtensionsDocument::default().resolve(),
    );

    // Clearing every ordinary tool leaves the composed extension untouched.
    let no_tools = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({"tools": {}}))),
        &parent,
    )
    .expect("resolution");
    assert!(no_tools.tools.is_empty());
    assert!(
        no_tools.extensions.agent_status().is_some(),
        "an enabled extension is never silently stripped by the tools allowlist"
    );

    // And composing an extension adds no ordinary capability.
    let with_extension = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({
            "tools": {},
            "extensions": {"agentStatus": {"enabled": true}},
        }))),
        &parent,
    )
    .expect("the caller's own composition authorizes this");
    assert!(
        with_extension.tools.is_empty(),
        "extension composition never enters the ordinary tool set"
    );
}

/// Nothing about an override changes the invoking Agent's own execution
/// capabilities or its frozen generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_a_child_override_never_mutates_the_parent() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "grep", "subagent"], &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let parent = parent_of(&resources);

    let before_tools = resources.capability().tool_registry().names().join(",");
    let before_revision = resources.revision();
    let before_extensions = product.runtime().native_extensions();

    let _ = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({
            "tools": {"builtin": ["grep"]},
            "skills": [],
            "extensions": {},
        }))),
        &parent,
    )
    .expect("resolution");

    assert_eq!(
        resources.capability().tool_registry().names().join(","),
        before_tools,
        "the parent's tool registry is untouched"
    );
    assert_eq!(resources.revision(), before_revision);
    assert_eq!(
        product.runtime().native_extensions(),
        before_extensions,
        "the parent's extension composition is untouched"
    );
    assert_eq!(
        parent_of(&resources),
        parent,
        "the caller's frozen authority is a value, not mutable state"
    );
}
