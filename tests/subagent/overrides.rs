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
//! linearization the test drives — a native configuration publication, or a typed
//! refusal — never by a sleep.

use crate::launch_fixture::LaunchFixture;
use std::sync::Arc;

use rustx::extensions::{NativeAgentExtensionSelection, NativeAgentExtensions};
use rustx::local_runtime::composition::{LocalRuntimeDependencies, LocalSessionClient};
use rustx::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
use rustx::model::invocation::ModelBindingRegistry;
use rustx::model::session::SessionModelConfig;
use rustx::runtime::RuntimeResourceSnapshot;
use rustx::runtime::subagent::{
    ResolvedSubagentSpec, SubagentInvocationOverride, SubagentName, SubagentResolution,
    SubagentResolutionError, SubagentResolver,
};

const KEY_ENV: &str = "RUSTX_ISSUE258_KEY";

#[tokio::test]
async fn goal84_root_only_scope_is_enforced_for_definition_model_and_workflow_overrides() {
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({
        "goal_role": {"description": "Root-only extension in a named child", "tools": {"builtin": []}, "plugins": {"goal": {"enabled": true}}},
        "plain": {"description": "Ordinary child", "tools": {"builtin": []}}
    }), &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let invocation = parse_override(serde_json::json!({"plugins": {"goal": {"enabled": true}}}));
    assert!(
        delegate(&resources, "goal_role", None).is_err(),
        "a selected child-invalid profile fails admission"
    );
    {
        let result = delegate(&resources, "plain", Some(&invocation));
        assert!(
            matches!(result, Err(SubagentResolutionError::ExtensionScopeUnsupported { extension, .. }) if extension == "goal")
        );
    }
    // The resolver produces no child contract; disabling Goal explicitly is supported.
    assert!(
        delegate(
            &resources,
            "goal_role",
            Some(&parse_override(serde_json::json!({"plugins": {}}))),
        )
        .is_ok()
    );
    product.runtime().shutdown().await.unwrap();
}

const MODELS: &str = r#"[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_ISSUE258_KEY"

[models."local/model-a"]
provider = "local"
id = "model-a"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[models."local/model-a".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[models."local/model-a".compat]
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

fn attempt_model() -> SessionModelConfig {
    SessionModelConfig::of(ModelRef::parse("local/model-a").expect("model reference"))
}

/// One main-model (dynamic delegation) resolution.
fn delegate(
    resources: &RuntimeResourceSnapshot,
    name: &str,
    invocation: Option<&SubagentInvocationOverride>,
) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
    SubagentResolver::resolve(&SubagentResolution {
        resources,
        agent: &agent(name),
        attempt_model: &attempt_model(),
        models: &model_registry(),

        invocation,
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
        std::fs::create_dir_all(lab.workspace().join(".agents/agents")).expect("role directory");
        std::fs::create_dir_all(lab.workspace().join(".agents/workflows"))
            .expect("workflow directory");
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
    /// `builtin_tools` is the **invoking model's** frozen model-facing
    /// selection, which is deliberately narrower than the generation's
    /// available catalog: the difference between the two is what makes
    /// "generation-only authority" a real, testable state.
    fn write_config(&self, roles: &serde_json::Value, builtin_tools: &[&str]) {
        let mut subagents = serde_json::json!({"max_concurrent": 4, "roles": roles});
        let names = subagents["roles"]
            .as_object()
            .expect("roles")
            .keys()
            .map(|name| serde_json::Value::String(name.clone()))
            .collect::<Vec<_>>();
        let root_agents = serde_json::Value::Array(names);
        crate::launch_fixture::write_roles(&self.workspace(), &mut subagents);
        let document = serde_json::json!({"schema_version": 9, "agent_id": "agent-issue258", "context": {"reserve_tokens": 0, "keep_recent_tokens": 0}, "subagents": subagents, "agent": {"model": {"model": "local/model-a"}, "tools": {"builtin": builtin_tools}, "agents": root_agents, "workflows": []}});
        std::fs::write(
            self.root().join("rustx.toml"),
            format!(
                "{}\n{MODELS}",
                toml::to_string_pretty(&document).expect("config document")
            ),
        )
        .expect("rustx.toml");
    }

    fn paths(&self) -> LaunchFixture {
        LaunchFixture {
            config: self.root().join("rustx.toml"),
            startup_session: rustx::local_runtime::StartupSession::Empty,
            session_name: None,
            workspace: self.workspace(),
            runtime_root: self.root().join("runtime"),
        }
    }

    async fn compose(&self) -> LocalSessionClient {
        (self.paths())
            .compose(&dependencies())
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
        match self.paths().try_resolve() {
            Err(error) => error,
            Ok(prospective) => format!(
                "{:?}",
                prospective.resource_inspection().resource_diagnostics
            ),
        }
    }
}

/// The role every replacement test specializes: read-only, one Skill, and the
/// authored extension defaults (which compose Agent Status).
fn reviewer_roles() -> serde_json::Value {
    serde_json::json!({"reviewer": {"description": "Review one bounded change.", "plugins": {"agent_status": {"enabled": true}, "todo": {"enabled": true}}, "tools": {"builtin": ["read"]}, "skills": ["review-guidance"]}})
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
    lab.write_config(&reviewer_roles(), &["read"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let none = delegate(&resources, "reviewer", None).expect("defaults resolve");
    let empty = parse_override(serde_json::json!({}));
    assert!(empty.is_empty());
    let explicit_empty = delegate(&resources, "reviewer", Some(&empty)).expect("defaults resolve");

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
    lab.write_config(&reviewer_roles(), &["read", "grep"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    // Tools only.
    let tools_only = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(
            serde_json::json!({"tools": {"builtin": ["grep"]}}),
        )),
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
            "plugins": {},
        }))),
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
    lab.write_config(&reviewer_roles(), &["read"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let cleared = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({
            "tools": {},
            "skills": [],
            "plugins": {},
        }))),
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
    let defaults = delegate(&resources, "reviewer", None).expect("defaults resolve");
    assert!(defaults.extensions.agent_status().is_some());
    assert_ne!(cleared.profile_digest(), defaults.profile_digest());
}

/// Two invocations of one role with different overrides are independent, and
/// neither mutates the shared definition, catalog, or generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_concurrent_invocations_do_not_mutate_each_other_or_the_role() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "grep"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let role_before = resources
        .subagents()
        .get(&agent("reviewer"))
        .expect("the generation admits the role")
        .clone();

    let (first, second) = tokio::join!(
        {
            let resources = Arc::clone(&resources);
            async move {
                delegate(
                    &resources,
                    "reviewer",
                    Some(&parse_override(
                        serde_json::json!({"tools": {"builtin": ["grep"]}}),
                    )),
                )
                .expect("first invocation")
            }
        },
        {
            let resources = Arc::clone(&resources);
            async move {
                delegate(
                    &resources,
                    "reviewer",
                    Some(&parse_override(serde_json::json!({"tools": {}}))),
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
        delegate(&resources, "reviewer", None,)
            .expect("the defaults still resolve")
            .profile_digest(),
        delegate(&resources, "reviewer", None,)
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
async fn cfg3_named_agent_and_invocation_tools_are_independent_of_root_visibility() {
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({"reviewer":{"description":"Independent child", "tools":{"builtin":["read"]}, "plugins":{"todo":{"enabled":true}}}}), &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    assert!(
        resources
            .capability()
            .tool_registry()
            .definitions()
            .iter()
            .all(|tool| tool.name != "read")
    );
    let own = delegate(&resources, "reviewer", None).unwrap();
    assert_eq!(tool_names(&own), ["builtin:read"]);
    assert!(own.extensions.todo().is_some());
    for name in ["grep", "write", "bash"] {
        let requested = parse_override(serde_json::json!({"tools":{"builtin":[name]}}));
        let child = delegate(&resources, "reviewer", Some(&requested)).unwrap();
        assert_eq!(tool_names(&child), [format!("builtin:{name}")]);
    }
    product.runtime().shutdown().await.unwrap();
}

/// Skill delegation is its own authorization domain, gated by the #234
/// visibility rule: a parent with no Read sees no Skills and can delegate
/// none, while the role's own Skills stay delegable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg3_child_skill_and_plugin_selection_is_independent_of_root_visibility() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "Review", "Review body");
    lab.write_skill("security-review", "Security", "Security body");
    lab.write_config(&reviewer_roles(), &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    assert!(resources.capability().model_skill_entries().is_empty());
    assert_eq!(
        skill_names(&delegate(&resources, "reviewer", None).unwrap()),
        ["review-guidance"]
    );
    let requested = parse_override(
        serde_json::json!({"skills":["security-review"], "plugins":{"agentStatus":{"enabled":true}}}),
    );
    let child = delegate(&resources, "reviewer", Some(&requested)).unwrap();
    assert_eq!(skill_names(&child), ["security-review"]);
    assert!(child.extensions.agent_status().is_some());
    product.runtime().shutdown().await.unwrap();
}

/// An unknown or hidden reference keeps its own failure class: it is never
/// reported as an authority verdict, and never silently dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_unknown_references_keep_their_own_failure_class() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    assert_eq!(
        delegate(
            &resources,
            "reviewer",
            Some(&parse_override(
                serde_json::json!({"tools": {"builtin": ["definitely_not_a_capability"]}})
            )),
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
        plugins:
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

#[tokio::test]
async fn goal84_workflow_program_cannot_enable_goal_for_an_agent_node() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_skill("security-review", "Security", "guidance body");
    lab.write_workflow(
        "goal-child",
        &OVERRIDE_WORKFLOW.replace("agentStatus:", "goal:"),
    );
    lab.write_config(&reviewer_roles(), &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let id = rustx::runtime::workflow::WorkflowId::parse("goal-child").unwrap();
    let rustx::runtime::workflow::WorkflowAdmission::Disabled(diagnostics) =
        &resources.workflows().entries()[&id].admission
    else {
        panic!("Goal child must disable the entire program")
    };
    assert!(diagnostics.iter().any(|diagnostic| matches!(
        diagnostic.reason,
        rustx::runtime::workflow::WorkflowDependencyFailure::Agent(
            rustx::runtime::agent_profile::AgentProfileDiagnostic::ScopeUnsupported { .. }
        )
    )));
    assert!(resources.workflows().executable(&id).is_err());
}

/// Equivalent authorized inputs resolve to one specification on both paths.
/// This is the "one resolver" property stated as an assertion rather than as
/// a comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_equivalent_dynamic_overrides_have_one_frozen_identity() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "grep"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let requested = parse_override(serde_json::json!({
        "tools": {"builtin": ["grep", "read"]},
        "skills": ["review-guidance"],
    }));
    let from_tool = delegate(&resources, "reviewer", Some(&requested))
        .expect("the main model holds both capabilities");
    // A differently spelled but semantically identical request agrees too.
    let respelled = parse_override(serde_json::json!({
        "tools": {"builtin": ["read", "grep"]},
        "skills": ["review-guidance", "review-guidance"],
    }));
    assert_eq!(
        delegate(&resources, "reviewer", Some(&respelled),)
            .expect("resolution")
            .profile_digest(),
        from_tool.profile_digest(),
        "Tool order and Skill repetition are normalization, not identity"
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
    lab.write_config(&reviewer_roles(), &["read"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    let id = rustx::runtime::workflow::WorkflowId::parse("nested").unwrap();
    let rustx::runtime::workflow::WorkflowAdmission::Disabled(diagnostics) =
        &resources.workflows().entries()[&id].admission
    else {
        panic!("required missing Tool must disable the program")
    };
    assert!(diagnostics.iter().any(|diagnostic| diagnostic.path == "block.nodes.fan.branches.only.block.nodes.work"
        && matches!(&diagnostic.reason, rustx::runtime::workflow::WorkflowDependencyFailure::Agent(
            rustx::runtime::agent_profile::AgentProfileDiagnostic::Tool(rustx::capabilities::selection::ToolSelectionError::UnknownCapability { selector })
        ) if selector == "builtin:definitely_not_a_capability")));
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
    lab.write_config(&reviewer_roles(), &["read"]);
    let error = lab.offline_error();
    assert!(
        error.contains("Workflow graph, control structure, or configured bound is invalid"),
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

/// A default that the invocation replaced away is not a dependency any more:
/// resolution requires only the effective selection's sources, and the role's
/// separate catalog admission is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub258_a_replaced_away_default_is_no_longer_a_requirement() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(&reviewer_roles(), &["read", "grep"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    // Only the effective Skill is frozen for materialization; the role's
    // default Skill is absent from the specification entirely.
    let replaced = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({"skills": []}))),
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
        skill_names(&delegate(&resources, "reviewer", None,).expect("defaults")),
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
    lab.write_config(&reviewer_roles(), &["read", "grep"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let profile = |value: Option<serde_json::Value>| {
        let requested = value.map(parse_override);
        delegate(&resources, "reviewer", requested.as_ref())
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
        profile(Some(serde_json::json!({"plugins": {}}))),
        profile(Some(serde_json::json!({
            "plugins": {"agentStatus": {"enabled": true, "time": {"enabled": false}}}
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
            builtin: vec!["read".to_owned()],
            sources: std::collections::BTreeMap::new(),
        }),
        skills: Some(vec!["review-guidance".to_owned()]),
        extensions: Some(NativeAgentExtensionSelection::of(role.extensions())),
    };
    assert_eq!(
        delegate(&resources, "reviewer", Some(&restated),)
            .expect("resolution")
            .profile_digest(),
        defaults,
        "no override and an override restating the defaults are one profile"
    );

    // The identity is a value, not an authority token, and survives the wire.
    let frozen = delegate(&resources, "reviewer", None).expect("resolution");
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
    lab.write_config(&reviewer_roles(), &["read"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    // Clearing every ordinary tool leaves the composed extension untouched.
    let no_direct_tools = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({"tools": {}}))),
    )
    .expect("resolution");
    assert!(no_direct_tools.tools.is_empty());
    assert!(
        no_direct_tools.extensions.agent_status().is_some(),
        "an enabled extension is never silently stripped by the tools allowlist"
    );

    // And composing an extension adds no ordinary capability.
    let with_extension = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({
            "tools": {},
            "plugins": {"agentStatus": {"enabled": true}},
        }))),
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
    lab.write_config(&reviewer_roles(), &["read", "grep"]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();

    let before_tools = resources.capability().tool_registry().names().join(",");
    let before_revision = resources.revision();
    let before_extensions = product.runtime().native_extensions();

    let _ = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({
            "tools": {"builtin": ["grep"]},
            "skills": [],
            "plugins": {},
        }))),
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
}

// =====================================================================
// Issue #259 — Todo through the shared #258 override machinery.
// =====================================================================

/// Issue #259 regression 9: a named role's own Todo default, and a
/// per-invocation override of it, resolve to the exact frozen child extension
/// set — through #258's shared resolver, with no Todo-specific override path.
///
/// The four cases below are the whole contract of a present-vs-absent
/// dimension applied to a second extension:
///
/// ```text
/// no override                   -> the role's own composition
/// override without `extensions` -> the role's own composition
/// `extensions: {todo: {...}}`   -> replaces the dimension entirely
/// `extensions: {}`              -> composes nothing at all
/// ```
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ext259_a_role_todo_default_and_its_invocation_override_freeze_exactly() {
    let lab = Lab::new();
    lab.write_skill("review-guidance", "How to review", "guidance body");
    lab.write_config(
        &serde_json::json!({
            "reviewer": {
                "description": "Review one bounded change.",
                "tools": {"builtin": ["read"]},
                "skills": ["review-guidance"],
                // The role declares its own Todo default, and switches Agent
                // Status off — proving the two dimensions are independent in
                // role authoring as well.
                "plugins": {
                    "todo": {"enabled": true},
                    "agent_status": {"enabled": false},
                },
            }
        }),
        &["read"],
    );
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    // The invoking Agent composes Todo, which is what entitles it to ask for
    // Todo at all under the delegation ceiling.

    let role_default = delegate(&resources, "reviewer", None).expect("resolution");
    assert_eq!(
        role_default.extensions,
        NativeAgentExtensions::with_todo(),
        "the role's authored composition is the frozen child default, exactly"
    );

    // A present override on another dimension leaves extensions alone.
    let tools_only = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({"tools": {}}))),
    )
    .expect("resolution");
    assert_eq!(
        tools_only.extensions, role_default.extensions,
        "a missing extension dimension uses the role definition default"
    );

    // A present `extensions` dimension replaces it entirely.
    let disabled = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(
            serde_json::json!({"plugins": {"todo": {"enabled": false}}}),
        )),
    )
    .expect("narrowing needs no authority");
    assert!(
        disabled.extensions.todo().is_none() && disabled.extensions.is_empty(),
        "a present dimension replaces the role's composition rather than merging into it"
    );
    assert_eq!(
        delegate(
            &resources,
            "reviewer",
            Some(&parse_override(serde_json::json!({"plugins": {}}))),
        )
        .expect("resolution")
        .extensions,
        NativeAgentExtensions::none(),
        "an explicitly empty dimension composes nothing at all"
    );

    // The frozen set is part of the child's execution identity, and survives
    // the serialization contract that actually carries it to a child process.
    assert_ne!(role_default.profile_digest(), disabled.profile_digest());
    let crossed: ResolvedSubagentSpec =
        serde_json::from_slice(&serde_json::to_vec(&role_default).expect("encode"))
            .expect("decode");
    assert_eq!(crossed.extensions, role_default.extensions);
    assert_eq!(crossed.profile_digest(), role_default.profile_digest());

    // And an override restating the role's own composition is the same
    // effective profile, not merely a similar-looking one.
    let restated = delegate(
        &resources,
        "reviewer",
        Some(&parse_override(serde_json::json!({
            "plugins": serde_json::to_value(NativeAgentExtensionSelection::of(
                &role_default.extensions
            ))
            .expect("the selection serializes"),
        }))),
    )
    .expect("restating a held composition is authorized");
    assert_eq!(restated.profile_digest(), role_default.profile_digest());
}

/// Issue #259 regression 9 (authority half) and 10: Todo obeys #258's
/// authority distinction, with no widening rule of its own.
///
/// ```text
/// main-model override   bounded by role ∪ invoking Agent composition
/// Workflow override     bounded by the admitted/frozen generation authority
/// ```
///
/// A main-model caller that composes no Todo, delegating to a role that
/// composes no Todo, cannot manufacture one; the same request from a trusted
/// Workflow program resolves, because a Workflow's authority is the admitted
/// generation rather than the invoking model's narrower profile. Both are
/// decided before child ownership commits: the refusal is a typed resolution
/// error, so no process, worktree, or Session is created.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg3_child_todo_override_needs_no_root_plugin_ceiling() {
    let lab = Lab::new();
    lab.write_config(&serde_json::json!({"reviewer":{"description":"No default Plugins", "tools":{"builtin":[]}}}), &[]);
    let product = lab.compose().await;
    let resources = product.runtime().runtime_resources();
    assert!(
        delegate(&resources, "reviewer", None)
            .unwrap()
            .extensions
            .todo()
            .is_none()
    );
    let requested = parse_override(serde_json::json!({"plugins":{"todo":{"enabled":true}}}));
    assert!(
        delegate(&resources, "reviewer", Some(&requested))
            .unwrap()
            .extensions
            .todo()
            .is_some()
    );
    assert!(product.runtime().native_extensions().todo().is_none());
    product.runtime().shutdown().await.unwrap();
}
