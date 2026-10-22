//! Subagent child composition regressions: frozen resource replay, selected
//! capability materialization, preparation cancellation and parent loss.

use crate::durable::CanonicalStore;
use crate::durable::JournalStore;
use crate::durable::inbox::ConversationInboundCapability;
use std::sync::Arc;

use super::super::{ChildPreparation, LocalConversationCore, LocalRuntimeDependencies};
use crate::model::catalog::MapCredentialEnvironment;
use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolId};
use crate::runtime::resources::ProjectContextFile;
use crate::runtime::subagent::catalog::SubagentName;
use crate::runtime::subagent::ipc::{SUBAGENT_IPC_VERSION, SubagentChildSpec};
use crate::runtime::subagent::resolver::{ResolvedSubagentSpec, ResolvedSubagentTool};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolInvocationPolicy, ToolOrigin, ToolReplayPolicy,
};

const MODELS: &str = r#"[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_CHILD_KEY"

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
[models."local/model-a".compat]
chat_reasoning_replay = "omit"
"#;

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Some(Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_CHILD_KEY".to_owned(),
            "test-only-secret".to_owned(),
        )]))),
        ..LocalRuntimeDependencies::default()
    }
}

/// A frozen Builtin capability carrying the **real** admitted native
/// definition under the given policy: the child plane materializes the
/// exact frozen definition, so a synthetic stand-in would (correctly)
/// fail to materialize.
fn builtin_with_policy(name: &str, policy: ToolInvocationPolicy) -> ResolvedSubagentTool {
    let definition = crate::tools::native::subagent_child_definition(name, policy)
        .expect("the child plane implements this native capability");
    ResolvedSubagentTool::Builtin {
        tool_id: definition.id.clone(),
        name: definition.name.clone(),
        definition,
    }
}

fn builtin(name: &str) -> ResolvedSubagentTool {
    builtin_with_policy(name, ToolInvocationPolicy::default())
}

#[tokio::test]
async fn child_model_inventory_contains_present_only_when_frozen_admission_selects_it() {
    let directory = lab();
    let core = LocalConversationCore::compose_subagent_child(
        &spec(
            directory.path(),
            vec![builtin("read")],
            Vec::new(),
            Vec::new(),
        ),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .unwrap();
    let snapshot = core.capability().current_snapshot();
    assert!(
        snapshot
            .tool_registry()
            .model_definitions()
            .iter()
            .all(|tool| tool.name != "present")
    );
    let present = crate::tools::native::definitions(
        crate::tools::native::NativeToolPolicies::default(),
        &crate::runtime::subagent::AgentCatalog::empty(),
    )
    .into_iter()
    .map(|(tool, _)| tool)
    .find(|tool| tool.name == "present")
    .unwrap();
    let forced = ResolvedSubagentTool::Builtin {
        tool_id: present.id.clone(),
        name: present.name.clone(),
        definition: present,
    };
    let result = LocalConversationCore::compose_subagent_child(
        &spec(directory.path(), vec![forced], Vec::new(), Vec::new()),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await;
    assert!(
        result.is_ok(),
        "an admitted child can declare files through its own conversation"
    );
}

fn spec(
    root: &std::path::Path,
    tools: Vec<ResolvedSubagentTool>,
    project_instructions: Vec<ProjectContextFile>,
    skills: Vec<crate::runtime::subagent::ResolvedSubagentSkill>,
) -> SubagentChildSpec {
    spec_with_extensions(
        root,
        tools,
        project_instructions,
        skills,
        crate::local_runtime::config::builtin_root_profile()
            .extensions
            .resolve(),
    )
}

fn spec_with_extensions(
    root: &std::path::Path,
    tools: Vec<ResolvedSubagentTool>,
    project_instructions: Vec<ProjectContextFile>,
    skills: Vec<crate::runtime::subagent::ResolvedSubagentSkill>,
    extensions: crate::extensions::NativeAgentExtensions,
) -> SubagentChildSpec {
    // Initialize the supported local-root format (the private reservation
    // namespace) before this fixture fabricates the child allocation tree,
    // so the allocation path is exercised on an accepted root.
    crate::runtime::local_storage::ProductRoot::create(root).unwrap();
    std::fs::create_dir_all(root.join("sessions/ses_01900000-0000-7000-8000-000000000001/conversations/conv_c89c766b-0004-76cb-8baf-0d5b1a677c1e/incarnation-test")).unwrap();
    SubagentChildSpec {
        session_id: crate::runtime::identity::SessionId::new(
            "ses_01900000-0000-7000-8000-000000000001",
        ),

        protocol_version: SUBAGENT_IPC_VERSION,
        product_root: root.to_path_buf(),
        subagent_id: SubagentId::new("conv_c89c766b-0004-76cb-8baf-0d5b1a677c1e"),
        child_conversation_id: ConversationId::new("conv_c89c766b-0004-76cb-8baf-0d5b1a677c1e"),
        child_agent_id: AgentId::new("agent-child"),
        parent_agent_id: AgentId::new("agent-parent"),
        resolved: ResolvedSubagentSpec {
            environment: Vec::new(),
            generation: crate::runtime::identity::RuntimeResourceRevision::new(1),
            skill_roots: Vec::new(),

            selection: crate::runtime::agent_profile::FrozenAgentSelection {
                tools: tools
                    .iter()
                    .map(|tool| match tool {
                        ResolvedSubagentTool::Builtin { name, .. } => {
                            crate::capabilities::selection::AgentToolSelection::Builtin {
                                name: name.clone(),
                            }
                        }
                        ResolvedSubagentTool::Source {
                            source_id, name, ..
                        } => crate::capabilities::selection::AgentToolSelection::Source {
                            source_id: source_id.clone(),
                            name: name.clone(),
                        },
                    })
                    .collect(),
                ..Default::default()
            },
            agent: SubagentName::parse("explore").expect("canonical name"),
            definition_digest: serde_json::from_value(serde_json::json!("sha256:frozen"))
                .expect("digest"),
            execution_deadline: None,
            instructions: "frozen child instructions".to_owned(),
            model: crate::model::frozen::test_frozen_model_spec(
                serde_json::from_value(serde_json::json!("local/model-a")).expect("model ref"),
            ),
            tools,
            skills,
            project_instructions,
            materialization:
                crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
            extensions,
        },
        approval_mode: crate::runtime::ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: crate::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        workspace: root.join("workspace"),
        incarnation: "incarnation-test".to_owned(),
        terminal: crate::runtime::subagent::ipc::ChildTerminalMode::Normal,
    }
}

/// The production child binding installs its existing turn permit gate
/// before activate's synchronous admission attempt. Historical guidance
/// is durable pending input, but no Delegate has released a turn permit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_binding_gates_historical_guidance_before_activation() {
    use crate::message::{content::TextBlock, types::UserContentBlock};

    let dir = lab();
    let child_spec = spec(dir.path(), Vec::new(), Vec::new(), Vec::new());
    let path = crate::runtime::subagent::child_conversation_store_path(
        dir.path(),
        &child_spec.session_id,
        &child_spec.child_conversation_id,
    );
    let store = crate::durable::SqliteConversationStore::open(
        child_spec.child_conversation_id.clone(),
        &path,
    )
    .unwrap();
    let accepted = crate::durable::ConversationInboundCapability::accept_inbound(
        &store,
        crate::durable::InboundDraft {
            message_id: None,
            source: crate::message::types::UserSource::Agent {
                agent_id: child_spec.parent_agent_id.clone(),
            },
            kind: crate::message::types::InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "historical guidance".into(),
            })],
            timestamp: chrono::Utc::now(),
            correlation: None,
        },
    )
    .unwrap();
    let core = LocalConversationCore::compose_subagent_child(
        &child_spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .unwrap();
    let (_parent, control) = tokio::net::UnixStream::pair().unwrap();
    let (_observation_parent, observation) = tokio::net::UnixStream::pair().unwrap();
    let dispatcher =
        crate::local_runtime::dispatcher::ChildControlDispatcher::start(control, observation);
    let child = core
        .into_subagent_child_with_route(
            child_spec.subagent_id.clone(),
            Arc::new(
                crate::local_runtime::subagent_child::ChildInteractionRoute::new(
                    dispatcher.handle(),
                ),
            ),
        )
        .unwrap();
    // activate already attempted admission synchronously. Exercise another
    // wake synchronously too: neither boundary may pass without Delegate.
    child.runtime().admit_now_for_test();
    assert!(child.runtime().is_activated());
    assert!(!child.runtime().has_current_attempt());
    assert_eq!(
        store.load_pending().unwrap()[0].message_id,
        accepted.message_id
    );
    assert!(store.load_canonical().unwrap().is_empty());
    assert!(
        !store
            .read_events(None, 128)
            .unwrap()
            .events
            .iter()
            .any(|event| matches!(
                event.event,
                crate::events::types::RuntimeEvent::AttemptStarted { .. }
            ))
    );
    child.shutdown().await.unwrap();
    dispatcher.shutdown().await;
    assert_eq!(store.load_pending().unwrap().len(), 1);
}

/// Issue #259 regressions 7 and 8: a Todo-enabled child composes **its
/// own** conversation's list, and two concurrently composed Todo-enabled
/// children compose two.
///
/// The isolation is structural rather than enforced: a list belongs to a
/// `ConversationToolRuntime`, a child owns its own runtime over its own
/// Ledger, and there is no cross-conversation reader anywhere. This proves
/// the structure holds by mutating each list through its real batch
/// authority and reading every other list back — no sleep, no ordering
/// assumption, and no shared handle to alias through in the first place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // one coherent three-owner isolation proof
async fn ext259_parent_and_concurrent_child_todo_lists_never_alias() {
    use crate::tools::todo::TodoCreate;

    let dir = lab();
    let parent_dir = tempfile::tempdir().expect("parent lab");
    std::fs::create_dir_all(parent_dir.path().join("workspace")).expect("parent workspace");

    // The parent conversation's own list, composed exactly as a root
    // launch composes it.
    let parent = crate::tools::runtime::ConversationToolRuntime::from_config(
        ConversationId::new("conv_8248c2d2-79e3-70b0-8093-f6299e1fc8be"),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            parent_dir.path().join("workspace"),
            parent_dir.path().join("artifacts"),
        )
        .with_extensions(crate::extensions::NativeAgentExtensions::with_todo()),
    )
    .expect("the parent tool runtime composes Todo");

    // Two children, composed from two frozen specifications that both
    // compose the Todo extension, over two distinct child conversations.
    let mut children = Vec::new();
    for index in 0..2u8 {
        let mut child_spec = spec_with_extensions(
            dir.path(),
            vec![builtin("read")],
            Vec::new(),
            Vec::new(),
            crate::extensions::NativeAgentExtensions::with_todo(),
        );
        // Distinct child conversation identities. The runtime-private
        // root, and with it the child's durable conversation store, is
        // *derived* from that identity, so two children can no more share
        // a Ledger than they can share an id.
        let id = ConversationId::generate();
        child_spec.subagent_id = SubagentId::new(format!("child-{index}"));
        child_spec.child_conversation_id = id.clone();
        std::fs::create_dir_all(
            crate::runtime::subagent::child_conversation_store_path(
                dir.path(),
                &child_spec.session_id,
                &id,
            )
            .parent()
            .unwrap()
            .join(&child_spec.incarnation),
        )
        .expect("child incarnation allocation");
        children.push(
            LocalConversationCore::compose_subagent_child(
                &child_spec,
                &dependencies(),
                &ChildPreparation::detached(),
            )
            .await
            .expect("the child composes its frozen Todo extension"),
        );
    }

    // Every runtime here composes a list, and a freshly composed child's
    // list is empty: it was rebuilt from the child's own canonical
    // history, which is empty at birth. Nothing was inherited.
    let lists = |core: &LocalConversationCore| {
        core.tool_runtime()
            .todo_snapshot()
            .expect("a Todo-enabled child composes a list")
    };
    assert!(
        parent
            .todo_snapshot()
            .expect("parent list")
            .tasks
            .is_empty()
    );
    for child in &children {
        assert!(
            lists(child).tasks.is_empty(),
            "a child's list is its own conversation's, never its parent's"
        );
    }

    // Write one distinct task into each of the three lists, through the
    // real batch authority, and settle each batch the way the Agent Loop
    // does when its ToolResult batch becomes canonical.
    let write = |todos: &crate::tools::todo::ConversationTodoList, subject: &str| {
        let batch = todos.open_batch().expect("a fresh list opens one batch");
        let writer = batch.writer();
        let (_, snapshot) = writer
            .create(TodoCreate {
                subject: subject.to_owned(),
                ..TodoCreate::default()
            })
            .expect("create");
        batch.settle(&[todo_result_message(subject, &snapshot)]);
    };
    write(parent.todos().expect("parent list"), "parent task");
    for (index, child) in children.iter().enumerate() {
        write(
            child
                .tool_runtime()
                .todos()
                .expect("a Todo-enabled child composes a list"),
            &format!("child {index} task"),
        );
    }

    // Each list holds exactly its own task. A single shared list would
    // show three; an aliased pair would show each other's.
    let subjects = |snapshot: &crate::tools::todo::TodoSnapshot| {
        snapshot
            .tasks
            .iter()
            .map(|task| task.subject.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        subjects(&parent.todo_snapshot().expect("parent list")),
        vec!["parent task".to_owned()],
        "no child snapshot merges into the parent's list"
    );
    for (index, child) in children.iter().enumerate() {
        assert_eq!(
            subjects(&lists(child)),
            vec![format!("child {index} task")],
            "two concurrent Todo-enabled children observe neither each other nor the parent"
        );
    }

    // And the two children really are two owners: the ids they allocated
    // are independent sequences over independent lists, not one sequence
    // over one list.
    for child in &children {
        assert_eq!(lists(child).tasks[0].id, 1);
    }
}

/// One canonical `todo` result message, as the Agent Loop commits it.
fn todo_result_message(
    id: &str,
    snapshot: &crate::tools::todo::TodoSnapshot,
) -> crate::message::types::MessageBlock {
    crate::message::types::MessageBlock::Tool(crate::message::types::ToolMessageBlock {
        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
            crate::runtime::identity::MessageId::new("assistant"),
            crate::message::types::ContentBlockIndex::new(0),
        ),
        id: crate::runtime::identity::MessageId::new(format!("message-{id}")),
        tool_call_id: crate::runtime::identity::ToolCallId::new(format!("call-{id}")),
        tool_id: crate::runtime::identity::ToolId::new(crate::tools::todo::TODO_TOOL_ID),
        result: crate::tools::types::ToolExecutionResult {
            deliveries: Vec::new(),
            status: crate::tools::types::ToolExecutionStatus::Success,
            content: vec![crate::tools::types::ToolResultContent::Json {
                value: serde_json::to_value(snapshot).expect("a Todo snapshot serializes"),
            }],
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    })
}

/// A lab whose workspace ancestry is *full* of project instructions and
/// Skills a discovering child would pick up.
fn lab() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("lab");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(workspace.join(".agents/skills/ambient")).expect("skills");
    std::fs::write(dir.path().join("rustx.toml"), MODELS).expect("rustx.toml");
    std::fs::write(
        workspace.join("AGENTS.md"),
        "ambient workspace instructions\n",
    )
    .expect("AGENTS.md");
    std::fs::write(
        workspace.join(".agents/skills/ambient/SKILL.md"),
        "---\nname: ambient\ndescription: an ambient skill\n---\n\nambient body\n",
    )
    .expect("SKILL.md");
    dir
}

/// The effective-extension projection one composed child publishes over
/// its own Runtime Client host — the exact surface a live child
/// inspection connects to.
fn child_projection(
    child: &super::super::LocalConversationRuntime,
) -> Option<crate::runtime_client::settings::EffectivePlugins> {
    child
        .host()
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("the child host projects its snapshot")
        .0
        .effective_plugins
}

/// Issue #256 regressions 6, 7 and 8: a child's Runtime Client
/// projection is its own frozen `ResolvedSubagentSpec::extensions`, and
/// nothing else.
///
/// The lab deliberately carries a *root-shaped* `rustx.toml` beside the
/// child, declaring a conflicting Agent Status composition. That
/// document stands in for every rereadable authority at once — root
/// launch configuration, the child workspace's own configuration, and a
/// newer role/resource generation published after the child was frozen:
/// the child never opens any of them, so a projection that agreed with
/// this file could only have got there by rereading configuration.
///
/// Three frozen specifications are composed and projected:
///
/// - one frozen with an explicit Asia/Shanghai Time and Background off
///   (the "R1" decision) — projected exactly, even though the document
///   beside it says `America/New_York` with Background on ("R2");
/// - one frozen with no extension at all — projected as
///   `agent_status = None`, never widened by the enabled document;
/// - the R1 specification after crossing its real serialization
///   contract, which is how it actually reaches a child process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn ext256_a_child_projects_only_its_frozen_extension_composition() {
    use crate::runtime_client::settings::{
        EffectiveAgentStatusExtension, EffectiveBackgroundStatus, EffectivePlugins,
        EffectiveTimeStatus,
    };

    let dir = lab();
    // The conflicting ambient authority: a root document that both
    // enables the extension and configures it differently.
    std::fs::write(
        dir.path().join("workspace/rustx.toml"),
        r#"schema_version = 10
agent_id = "agent-host"

[context]
reserve_tokens = 0
keep_recent_tokens = 0


[agent]
[agent.model]
model = "local/model-a"


[agent.plugins]
[agent.plugins.agent_status]
enabled = true

[agent.plugins.agent_status.time]
enabled = true
timezone = "America/New_York"


[agent.plugins.agent_status.background]
enabled = true
"#,
    )
    .expect("an ambient root configuration a rereading child would observe");

    let r1 = serde_json::from_value::<crate::extensions::NativeAgentExtensionsDocument>(
        serde_json::json!({"todo": {"enabled": true}, "agent_status": {
            "enabled": true,
            "time": {"enabled": true, "timezone": "Asia/Shanghai"},
            "background": {"enabled": false}
        }}),
    )
    .expect("the role extension document parses")
    .resolve();
    let expected_r1 = EffectivePlugins {
        goal: None,
        agent_status: Some(EffectiveAgentStatusExtension {
            time: EffectiveTimeStatus {
                enabled: true,
                timezone: Some(chrono_tz::Asia::Shanghai),
            },
            background: EffectiveBackgroundStatus { enabled: false },
        }),
        // The R1 role document authors no `todo` member, so the closed
        // document's own default composes it — and it crosses the child
        // boundary frozen, exactly like every other member (Issue #259).
        todo: Some(crate::runtime_client::settings::EffectiveTodoExtension {}),
    };

    // A child frozen on R1, composed and projected while R2 sits on disk.
    let child = LocalConversationCore::compose_subagent_child(
        &spec_with_extensions(
            dir.path(),
            vec![builtin("read")],
            Vec::new(),
            Vec::new(),
            r1.clone(),
        ),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the child composes")
    .into_bound()
    .expect("the child binds its own Runtime Client host");
    let (snapshot, _) = child
        .host()
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("child snapshot");
    assert_eq!(
        snapshot.effective_plugins,
        Some(expected_r1.clone()),
        "the child projects the composition its invoking generation froze"
    );
    // One source of truth on the child side too.
    assert_eq!(
        snapshot.effective_plugins,
        Some(EffectivePlugins::project(
            &child.runtime().native_extensions()
        ))
    );
    // Root configuration cannot leak in: neither the enabled Background
    // nor the America/New_York timezone beside the child appears.
    let projected = snapshot
        .effective_plugins
        .clone()
        .expect("a composed child always projects one");
    let status = projected.agent_status.expect("Agent Status is composed");
    assert_eq!(status.time.timezone, Some(chrono_tz::Asia::Shanghai));
    assert!(!status.background.enabled);
    // A child's composition is a frozen execution profile, not a launch
    // capture, and the lifetime vocabulary says exactly that.
    assert_eq!(
        snapshot.settings_evidence,
        crate::runtime_client::settings::SettingsEvidence::FrozenChild
    );

    // A child frozen with no extension is not widened by the enabled
    // document beside it.
    let bare = LocalConversationCore::compose_subagent_child(
        &spec_with_extensions(
            dir.path(),
            vec![builtin("read")],
            Vec::new(),
            Vec::new(),
            crate::extensions::NativeAgentExtensions::none(),
        ),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the child composes")
    .into_bound()
    .expect("the child binds its own Runtime Client host");
    assert_eq!(
        child_projection(&bare),
        Some(EffectivePlugins {
            goal: None,
            agent_status: None,
            todo: None,
        }),
        "an absent frozen extension stays absent in the projection"
    );

    // And the R1 decision survives its real cross-process encoding.
    let decoded = serde_json::from_slice::<crate::extensions::NativeAgentExtensions>(
        &serde_json::to_vec(&r1).expect("encode"),
    )
    .expect("decode");
    let staged = LocalConversationCore::compose_subagent_child(
        &spec_with_extensions(
            dir.path(),
            vec![builtin("read")],
            Vec::new(),
            Vec::new(),
            decoded,
        ),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the child composes")
    .into_bound()
    .expect("the child binds its own Runtime Client host");
    assert_eq!(child_projection(&staged), Some(expected_r1));
}

/// Issue #256 regression 9: child materialization never rereads host,
/// project, or root configuration to widen or reinterpret its native
/// Agent Extension set.
///
/// The lab's workspace carries a `rustx.toml` that enables the Agent
/// Status extension with a distinctive timezone. A child whose invoking
/// generation froze *no* extension composes no status engine, and a
/// child whose invoking generation froze a *different* Agent Status
/// configuration composes exactly that one. Neither observes the
/// document on disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext256_child_materialization_never_rereads_configuration_for_its_extension_set() {
    let dir = lab();
    std::fs::write(
        dir.path().join("workspace/rustx.toml"),
        r#"schema_version = 10
agent_id = "agent-host"

[context]
reserve_tokens = 0
keep_recent_tokens = 0


[agent]
[agent.model]
model = "local/model-a"


[agent.plugins]
[agent.plugins.agent_status]
enabled = true

[agent.plugins.agent_status.time]
enabled = true
timezone = "America/New_York"


[agent.plugins.agent_status.background]
enabled = true
"#,
    )
    .expect("an ambient host configuration a rereading child would observe");

    let empty = LocalConversationCore::compose_subagent_child(
        &spec_with_extensions(
            dir.path(),
            vec![builtin("read")],
            Vec::new(),
            Vec::new(),
            crate::extensions::NativeAgentExtensions::none(),
        ),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the child composes");
    assert!(
        empty.runtime().context_config().status_engine.is_none(),
        "a child frozen with no extension cannot acquire one from the \
             configuration document beside it"
    );

    let frozen = serde_json::from_value::<crate::extensions::NativeAgentExtensionsDocument>(
        serde_json::json!({"agent_status": {"enabled": true, "time": {"timezone": "Asia/Shanghai"}}}),
    )
    .expect("role extension document")
    .resolve();
    let composed = LocalConversationCore::compose_subagent_child(
        &spec_with_extensions(
            dir.path(),
            vec![builtin("read")],
            Vec::new(),
            Vec::new(),
            frozen.clone(),
        ),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the child composes");
    assert_eq!(
        composed
            .runtime()
            .context_config()
            .status_engine
            .as_ref()
            .expect("the frozen extension is materialized")
            .config(),
        frozen
            .agent_status()
            .expect("the frozen composition carries Agent Status"),
        "the child materializes exactly the frozen decision, never the \
             configuration document's"
    );
}

/// Issue #144: a child observes only what its invoking generation froze.
/// The workspace deliberately contains an `AGENTS.md` and a discoverable
/// Skill; a child that ran discovery would show both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_performs_no_project_instruction_or_skill_discovery() {
    let dir = lab();
    let frozen = vec![
        ProjectContextFile {
            path: dir.path().join("frozen-global/AGENTS.md"),
            content: "frozen global guidance".to_owned(),
        },
        ProjectContextFile {
            path: dir.path().join("frozen/AGENTS.md"),
            content: "frozen parent chain".to_owned(),
        },
    ];
    let core = LocalConversationCore::compose_subagent_child(
        &spec(
            dir.path(),
            vec![builtin("read")],
            frozen.clone(),
            Vec::new(),
        ),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the child composes");
    let resources = core.runtime().runtime_resources();

    assert_eq!(
        resources.project_context_files(),
        frozen,
        "the child observes exactly the frozen chain and never walks ancestors"
    );
    assert_eq!(
        resources.project_instructions(),
        Some(format!(
            "Project-specific instructions and guidelines:\n\n<project_instructions path=\"{}\">\nfrozen global guidance\n</project_instructions>\n\n<project_instructions path=\"{}\">\nfrozen parent chain\n</project_instructions>",
            dir.path().join("frozen-global/AGENTS.md").display(),
            dir.path().join("frozen/AGENTS.md").display()
        ).as_str())
    );
    assert_eq!(
        resources.skill_catalog(),
        None,
        "an empty frozen allowlist means an empty child Skill catalog, \
             whatever the workspace contains"
    );
    assert_eq!(
        resources.agent_profile(),
        Some(
            format!(
                "frozen child instructions\n\n{}",
                crate::runtime::subagent::SUBAGENT_FINAL_REPORT_INSTRUCTION
            )
            .as_str()
        ),
        "the definition's instructions, composed with the runtime-owned final-report rule, \
             are the child's AgentProfile authority"
    );
    assert_eq!(
        core.capability().current_snapshot().tool_registry().names(),
        // The frozen ordinary selection is `read`; `todo` is the second
        // contribution — the child's frozen Todo extension (Issue #259).
        vec!["read"],
        "the child's active set is exactly its authorized ordinary selection plus its \
             frozen extension Tools"
    );
}

/// Issue #192: the runtime-owned final-report handoff rule is generic
/// subagent execution semantics — a normal one-shot child receives it
/// appended to its definition's instructions even when the user-authored
/// document never mentions the handoff. A Workflow-owned child keeps its
/// structured `workflow_output` terminal protocol instead: the free-form
/// final-report rule would contradict it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_final_report_rule_is_runtime_owned_and_terminal_mode_aware() {
    let dir = lab();

    // The user-authored document says nothing about the handoff.
    let normal = LocalConversationCore::compose_subagent_child(
        &spec(dir.path(), vec![builtin("read")], Vec::new(), Vec::new()),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the normal child composes");
    let profile = normal
        .runtime()
        .runtime_resources()
        .agent_profile()
        .expect("agent profile")
        .to_owned();
    assert!(
        profile.starts_with("frozen child instructions\n\n"),
        "the user-authored instructions are preserved first: {profile}"
    );
    assert!(
        profile.contains("Your final response is the complete handoff to the parent agent."),
        "the runtime-owned handoff rule is composed for a normal child: {profile}"
    );

    let mut workflow_spec = spec(dir.path(), vec![builtin("read")], Vec::new(), Vec::new());
    workflow_spec.terminal = crate::runtime::subagent::ipc::ChildTerminalMode::WorkflowOutput {
        output_schema: serde_json::json!({"type": "object"}),
    };
    let workflow = LocalConversationCore::compose_subagent_child(
        &workflow_spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the Workflow child composes");
    assert_eq!(
        workflow.runtime().runtime_resources().agent_profile(),
        Some("frozen child instructions"),
        "a Workflow-owned child keeps its structured terminal protocol, unamended"
    );
}

/// Issue #146: the execution binding is not project-instruction
/// authority. The unrelated AGENTS.md is deliberately above a nested
/// binding (for example an externally created Git worktree); the child
/// still consumes only the parent-frozen chain carried in its typed
/// specification.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_binding_child_never_discovers_ancestor_project_instructions() {
    let dir = lab();
    let binding = dir.path().join("checkouts/feature");
    std::fs::create_dir_all(&binding).expect("nested workspace binding");
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "unrelated ancestor instructions\n",
    )
    .expect("ancestor AGENTS.md");
    let mut child_spec = spec(
        dir.path(),
        vec![builtin("read")],
        vec![ProjectContextFile {
            path: dir.path().join("frozen/AGENTS.md"),
            content: "frozen parent chain".to_owned(),
        }],
        Vec::new(),
    );
    child_spec.workspace = binding;

    let core = LocalConversationCore::compose_subagent_child(
        &child_spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the nested-binding child composes");
    let resources = core.runtime().runtime_resources();
    assert_eq!(
        resources.project_context_files(),
        &[ProjectContextFile {
            path: dir.path().join("frozen/AGENTS.md"),
            content: "frozen parent chain".to_owned(),
        }],
        "the execution binding is not project-instruction authority"
    );
    assert_eq!(
        resources.project_instructions(),
        Some(format!(
            "Project-specific instructions and guidelines:\n\n<project_instructions path=\"{}\">\nfrozen parent chain\n</project_instructions>",
            dir.path().join("frozen/AGENTS.md").display()
        ).as_str())
    );
    assert_eq!(resources.skill_catalog(), None);
    // Issue #462 (B02/B03): every child Tool, including Write, Edit and
    // Bash, is rooted at exactly the frozen Session binding.
    assert_eq!(
        core.runtime().tool_runtime().workspace().root(),
        child_spec
            .workspace
            .canonicalize()
            .expect("canonical binding")
    );
    assert_ne!(
        core.runtime().tool_runtime().workspace().root(),
        dir.path().canonicalize().expect("canonical ancestor"),
        "the ancestor cannot substitute for the frozen nested binding"
    );
}

/// A frozen Skill allowlist is **materialized** into the child-private
/// root, re-proven against its frozen `SkillVersionId`, and rendered as
/// metadata only, so progressive disclosure survives the boundary
/// (Issue #145).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // complete child composition and ownership fixture
async fn a_frozen_skill_allowlist_is_materialized_and_rendered_without_bodies() {
    let dir = lab();
    let source = dir.path().join("selected-source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: selected\ndescription: the selected skill\n---\nsecret body\n",
    )
    .expect("SKILL.md");
    let files = vec![std::path::PathBuf::from("SKILL.md")];
    let markdown = std::fs::read(source.join("SKILL.md")).expect("read");
    let version_id = crate::skills::identity::package_version_id(&source, &files, &markdown)
        .expect("frozen identity");
    let child_spec = spec(
        dir.path(),
        vec![builtin("read"), builtin("grep")],
        Vec::new(),
        vec![crate::runtime::subagent::ResolvedSubagentSkill {
            provenance: crate::skills::SkillProvenance {
                name: "selected".into(),
                source: crate::skills::SkillSource::Workspace,
                location: source.join("SKILL.md").display().to_string(),
                shadowed: Vec::new(),
            },
            binding: crate::protocol::manifest::SkillBinding {
                skill_id: crate::runtime::identity::SkillId::new("selected"),
                version_id,
            },
            catalog_entry: crate::skills::SkillCatalogEntry {
                name: "selected".to_owned(),
                description: "the selected skill".to_owned(),
                location: source.join("SKILL.md").display().to_string(),
            },
            source_root: source.clone(),
            files,
        }],
    );
    let core = LocalConversationCore::compose_subagent_child(
        &child_spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the child composes");
    let catalog = core
        .runtime()
        .runtime_resources()
        .skill_catalog()
        .expect("the frozen catalog")
        .to_owned();
    let materialized = child_spec
        .runtime_root()
        .unwrap()
        .join("skills/workspace/selected/SKILL.md")
        .display()
        .to_string();
    assert!(catalog.contains("selected"));
    assert!(
        catalog.contains(
            &child_spec
                .runtime_root()
                .unwrap()
                .join("skills/workspace")
                .display()
                .to_string()
        ),
        "the model-visible root is remapped onto the child's own copy: {catalog}"
    );
    assert!(
        std::path::Path::new(&materialized).is_file(),
        "the frozen bytes are materialized under the child-private root"
    );
    assert!(
        !catalog.contains("ambient"),
        "an unselected, discoverable Skill is absent from the child catalog: {catalog}"
    );
    assert!(
        !catalog.contains("secret body"),
        "only catalog metadata crosses the boundary: {catalog}"
    );
    // Skill visibility is independent of a child's Native Tool selection.
    let incarnation_parent = child_spec
        .runtime_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut without_read = child_spec;
    without_read.incarnation = "incarnation-without-read".to_owned();
    std::fs::create_dir_all(incarnation_parent.join(&without_read.incarnation)).unwrap();
    without_read.resolved.tools = vec![builtin("grep")];
    let core_without_read = LocalConversationCore::compose_subagent_child(
        &without_read,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("child without Read still materializes admitted packages");
    assert!(
        core_without_read
            .runtime()
            .runtime_resources()
            .skill_catalog()
            .is_some()
    );
    assert_eq!(
        core_without_read
            .capability
            .current_snapshot()
            .tool_registry()
            .names(),
        ["grep"]
    );
    assert!(
        without_read
            .runtime_root()
            .unwrap()
            .join("skills/workspace/selected/SKILL.md")
            .is_file()
    );
}

/// Skill bytes that changed after the parent froze them fail child
/// composition rather than executing a different Skill version.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_skill_source_fails_child_composition() {
    let dir = lab();
    let source = dir.path().join("selected-source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: selected\ndescription: the selected skill\n---\noriginal\n",
    )
    .expect("SKILL.md");
    let files = vec![std::path::PathBuf::from("SKILL.md")];
    let markdown = std::fs::read(source.join("SKILL.md")).expect("read");
    let version_id = crate::skills::identity::package_version_id(&source, &files, &markdown)
        .expect("frozen identity");
    // The source moves on after the freeze.
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: selected\ndescription: the selected skill\n---\nrewritten\n",
    )
    .expect("rewrite");
    let child_spec = spec(
        dir.path(),
        vec![builtin("read")],
        Vec::new(),
        vec![crate::runtime::subagent::ResolvedSubagentSkill {
            provenance: crate::skills::SkillProvenance {
                name: "selected".into(),
                source: crate::skills::SkillSource::Workspace,
                location: source.join("SKILL.md").display().to_string(),
                shadowed: Vec::new(),
            },
            binding: crate::protocol::manifest::SkillBinding {
                skill_id: crate::runtime::identity::SkillId::new("selected"),
                version_id,
            },
            catalog_entry: crate::skills::SkillCatalogEntry {
                name: "selected".to_owned(),
                description: "the selected skill".to_owned(),
                location: source.join("SKILL.md").display().to_string(),
            },
            source_root: source,
            files,
        }],
    );
    let error = LocalConversationCore::compose_subagent_child(
        &child_spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect_err("a changed Skill source fails closed");
    assert!(
        format!("{error}").contains("source bytes changed"),
        "unexpected failure: {error}"
    );
}

/// Issue #144: the child's model authority comes from the frozen
/// specification, so there is no model catalog read to race with a
/// later edit.
///
/// The proof is structural rather than a timing argument: the catalog
/// file is deleted before composition, and the child still composes with
/// exactly the frozen semantics. There is no code path left that could
/// observe a `rustx.toml` at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_composes_with_no_model_catalog_on_disk() {
    let dir = lab();
    std::fs::remove_file(dir.path().join("rustx.toml")).expect("remove the catalog");
    let spec = spec(dir.path(), vec![builtin("read")], Vec::new(), Vec::new());
    let core = LocalConversationCore::compose_subagent_child(
        &spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the child composes from the frozen model authority alone");

    let model = core.runtime().model_view();
    assert_eq!(
        model.effective.as_ref().unwrap().model,
        spec.resolved.model.primary.model,
        "the child runs exactly the model its parent froze"
    );
    assert_eq!(
        model.effective.as_ref().unwrap().protocol,
        spec.resolved.model.primary.protocol
    );
    assert_eq!(
        model.effective.as_ref().unwrap().context_window,
        spec.resolved.model.primary.context_window
    );
    assert_eq!(
        core.runtime().model_catalog().models.len(),
        1,
        "a frozen authority publishes exactly the model it froze"
    );
}

// ---- Issue #145: child preparation is cancellable owned work ----

/// Cancellation derived from the invoking spawn attempt settles a long
/// preparation — and the in-flight step is **not merely dropped**: the
/// guard fires the one preparation signal and awaits the step's own
/// settlement, so preparatory physical work settles before the
/// preparation reports settled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attempt_cancellation_settles_a_long_preparation() {
    let cancellation = crate::runtime::cancellation::CancellationSignal::new();
    let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
    let (_observation_parent, observation_child) =
        tokio::net::UnixStream::pair().expect("observation pair");
    let dispatcher =
        crate::local_runtime::dispatcher::ChildControlDispatcher::start(child, observation_child);
    let preparation = ChildPreparation::new(cancellation.clone(), dispatcher.handle());

    // A cancellation-aware step that would never finish on its own: it
    // parks on the preparation's one authority and records its
    // settlement before returning — exactly the contract of an MCP
    // connect or a uv build.
    let settled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let step = {
        let signal = preparation.cancellation();
        let settled = settled.clone();
        async move {
            signal.cancelled().await;
            settled.store(true, std::sync::atomic::Ordering::SeqCst);
            Err::<(), _>(
                crate::capabilities::CapabilityPreparationError::PreparationSettled(
                    "the step settled its owned work".to_owned(),
                ),
            )
        }
    };
    cancellation.cancel();
    let outcome = preparation.guard(step).await;
    assert!(
        matches!(
            outcome,
            Err(crate::capabilities::CapabilityPreparationError::PreparationSettled(_))
        ),
        "cancellation settles the preparation: {outcome:?}"
    );
    assert!(
        settled.load(std::sync::atomic::Ordering::SeqCst),
        "the guard awaited the step's settlement before reporting settled"
    );
    drop(parent);
}

/// Parent control-channel EOF is a **physical cancellation
/// authority**, observable **while** composition is in progress: it
/// fires the one preparation cancellation signal (so every preparatory
/// supervised unit is cancelled), and the guard awaits the step's
/// settlement rather than finishing work for a parent that no longer
/// exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_loss_settles_a_long_preparation() {
    let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
    let (_observation_parent, observation_child) =
        tokio::net::UnixStream::pair().expect("observation pair");
    let dispatcher =
        crate::local_runtime::dispatcher::ChildControlDispatcher::start(child, observation_child);
    let handle = dispatcher.handle();
    let preparation = ChildPreparation::new(
        crate::runtime::cancellation::CancellationSignal::new(),
        handle.clone(),
    );
    // The step parks on the preparation's one authority: only the
    // guard's parent-loss arm can release it, by firing the signal.
    let settled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let step = {
        let signal = preparation.cancellation();
        let settled = settled.clone();
        async move {
            signal.cancelled().await;
            settled.store(true, std::sync::atomic::Ordering::SeqCst);
            Err::<(), _>(
                crate::capabilities::CapabilityPreparationError::PreparationSettled(
                    "the step settled its owned work".to_owned(),
                ),
            )
        }
    };
    // The EOF is published by the dispatcher's reader before the guard
    // is entered, so the ordering is a fact rather than a race.
    drop(parent);
    handle.parent_lost_signal().await;

    let outcome = preparation.guard(step).await;
    assert!(
        matches!(
            outcome,
            Err(crate::capabilities::CapabilityPreparationError::PreparationSettled(_))
        ),
        "parent loss settles the preparation: {outcome:?}"
    );
    assert!(
        settled.load(std::sync::atomic::Ordering::SeqCst),
        "parent loss fired the physical cancellation authority and the step settled"
    );
    assert!(
        preparation.is_settled(),
        "parent loss leaves the preparation settled"
    );
}

/// A preparation that completes before either settlement authority fires
/// returns its ordinary result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_uninterrupted_preparation_returns_its_result() {
    let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
    let (_observation_parent, observation_child) =
        tokio::net::UnixStream::pair().expect("observation pair");
    let dispatcher =
        crate::local_runtime::dispatcher::ChildControlDispatcher::start(child, observation_child);
    let preparation = ChildPreparation::new(
        crate::runtime::cancellation::CancellationSignal::new(),
        dispatcher.handle(),
    );
    let outcome = preparation.guard(std::future::ready(Ok(7u32))).await;
    assert_eq!(outcome.expect("an uninterrupted preparation"), 7);
    drop(parent);
}

/// A step that completes **after** the cancellation won is not
/// publishable: the biased cancellation arm plus the `is_settled`
/// post-check make `Ready` impossible once pre-commit cancellation has
/// won.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_step_completing_after_cancellation_is_not_publishable() {
    let cancellation = crate::runtime::cancellation::CancellationSignal::new();
    let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
    let (_observation_parent, observation_child) =
        tokio::net::UnixStream::pair().expect("observation pair");
    let dispatcher =
        crate::local_runtime::dispatcher::ChildControlDispatcher::start(child, observation_child);
    let preparation = ChildPreparation::new(cancellation.clone(), dispatcher.handle());
    cancellation.cancel();
    // The cancellation already fired; the step completes anyway. The
    // guard's biased cancellation arm wins this poll...
    let outcome = preparation.guard(std::future::ready(Ok(7u32))).await;
    assert!(
        matches!(
            outcome,
            Err(crate::capabilities::CapabilityPreparationError::PreparationSettled(_))
        ),
        "a cancelled preparation never publishes: {outcome:?}"
    );
    // ...and where a step's completion wins a race elsewhere, the
    // caller-side `is_settled` check refuses to publish its output.
    assert!(preparation.is_settled());
    drop(parent);
}

// ---- Issue #145: selected-only MCP materialization ----

/// The frozen MCP binding of the shared stdio fixture, re-executing this
/// test binary as its own MCP server.
#[cfg(feature = "mcp-fixture")]
fn fixture_binding(test_name: &str, prefix: &str) -> crate::tools::mcp::McpServerBinding {
    crate::tools::mcp::McpServerBinding {
        credentials: crate::credentials::SourceCredentials::default(),
        transport: crate::tools::mcp::McpTransportConfig::Stdio {
            program: std::env::current_exe()
                .expect("test executable")
                .display()
                .to_string(),
            args: crate::tools::mcp::fixture::fixture_spawn_args(test_name),
            cwd: None,
            environment: std::collections::BTreeMap::from([
                (
                    crate::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                    "1".to_owned(),
                ),
                (
                    crate::tools::mcp::fixture::TOOL_PREFIX_ENV.to_owned(),
                    prefix.to_owned(),
                ),
            ]),
        },
        policy: ToolInvocationPolicy::default(),
    }
}

/// Connects the fixture once, exactly as a parent generation would, and
/// returns the canonical definition of one published tool.
#[cfg(feature = "mcp-fixture")]
async fn frozen_fixture_tool(
    server_id: &crate::runtime::identity::McpServerId,
    binding: &crate::tools::mcp::McpServerBinding,
    workspace: &crate::tools::Workspace,
    name: &str,
) -> ToolDefinition {
    let runtime = crate::tools::mcp::McpServerRuntime::connect(
        server_id,
        binding,
        workspace,
        std::sync::Arc::new(crate::tools::mcp::McpInvalidationState::new()),
    )
    .await
    .expect("the fixture connects");
    let tools = runtime.list_tools().await.expect("tools/list");
    let tool = tools
        .into_iter()
        .find(|tool| tool.name == name)
        .expect("the fixture publishes the tool");
    runtime.close().await.expect("the fixture runtime closes");
    ToolDefinition {
        id: ToolId::new(format!("mcp:{server_id}:{name}")),
        name: tool.name,
        description: tool.description,
        input_schema: tool.input_schema,
        execution_policy: binding.policy.execution,
        concurrency_policy: binding.policy.concurrency,
        approval_policy: binding.policy.approval,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Mcp {
            server_id: server_id.clone(),
        },
    }
}

/// A named agent that selects one **inactive-but-available** MCP tool
/// starts, and its child Tool Plane contains exactly the frozen Builtin
/// plus that one MCP tool — nothing the fixture also publishes, and
/// nothing from the workspace.
#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_materializes_exactly_the_selected_mcp_tool() {
    if crate::tools::mcp::fixture::serve_if_fixture_mode(
        crate::tools::mcp::fixture::FixtureServer::from_env(),
    )
    .await
    {
        return;
    }
    let dir = lab();
    let workspace = crate::tools::Workspace::new(dir.path().join("workspace")).expect("workspace");
    let server_id = crate::runtime::identity::McpServerId::new("alpha");
    let binding = fixture_binding(
        "local_runtime::composition::subagent_child::tests::a_child_materializes_exactly_the_selected_mcp_tool",
        "alpha_",
    );
    let definition = frozen_fixture_tool(&server_id, &binding, &workspace, "alpha_echo").await;
    let identity = crate::tools::mcp::identity::definition_identity(&definition)
        .expect("an MCP definition has an MCP identity");
    let mcp = ResolvedSubagentTool::Source {
        source_id: crate::capabilities::ToolSourceId::Mcp(server_id.clone()),
        tool_id: definition.id.clone(),
        name: definition.name.clone(),
        identity,
        definition,
    };
    let mut child_spec = spec(
        dir.path(),
        vec![builtin("read"), mcp],
        Vec::new(),
        Vec::new(),
    );
    child_spec
        .resolved
        .materialization
        .sources
        .insert(crate::capabilities::ToolSourceId::Mcp(server_id), binding);

    let core = LocalConversationCore::compose_subagent_child(
        &child_spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("the child materializes its frozen MCP selection");
    let snapshot = core.capability().current_snapshot();
    let mut names = snapshot.tool_registry().names();
    names.sort_unstable();
    assert_eq!(
        names,
        // `todo` is here because the fixture's frozen extension set
        // composes the Todo extension, and extension-provided Tools are a
        // second, independent contribution to the model Tool set
        // (Issue #259). It is deliberately *not* part of the frozen
        // ordinary selection: no `tools.builtin` entry names it.
        vec!["alpha_echo", "read"],
        "the child Tool Plane is exactly the frozen ordinary selection plus its \
             frozen extension Tools: the fixture also publishes alpha_mutate and \
             alpha_slow, and neither is materialized"
    );
}

/// A definition whose canonical cross-process identity is not the one the
/// parent generation froze fails child preparation — before `Ready`, and
/// therefore before any durable ownership commit.
#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_mcp_definition_fails_before_the_child_is_ready() {
    if crate::tools::mcp::fixture::serve_if_fixture_mode(
        crate::tools::mcp::fixture::FixtureServer::from_env(),
    )
    .await
    {
        return;
    }
    let dir = lab();
    let workspace = crate::tools::Workspace::new(dir.path().join("workspace")).expect("workspace");
    let server_id = crate::runtime::identity::McpServerId::new("beta");
    let binding = fixture_binding(
        "local_runtime::composition::subagent_child::tests::a_changed_mcp_definition_fails_before_the_child_is_ready",
        "beta_",
    );
    let mut definition = frozen_fixture_tool(&server_id, &binding, &workspace, "beta_echo").await;
    // The parent froze a description the server no longer publishes.
    definition.description = format!("{} (as the parent froze it)", definition.description);
    let identity = crate::tools::mcp::identity::definition_identity(&definition)
        .expect("an MCP definition has an MCP identity");
    let mcp = ResolvedSubagentTool::Source {
        source_id: crate::capabilities::ToolSourceId::Mcp(server_id.clone()),
        tool_id: definition.id.clone(),
        name: definition.name.clone(),
        identity,
        definition,
    };
    let mut child_spec = spec(
        dir.path(),
        vec![builtin("read"), mcp],
        Vec::new(),
        Vec::new(),
    );
    child_spec
        .resolved
        .materialization
        .sources
        .insert(crate::capabilities::ToolSourceId::Mcp(server_id), binding);

    let error = LocalConversationCore::compose_subagent_child(
        &child_spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect_err("a changed definition must not be executed");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("SourceIdentityMismatch"),
        "the failure is a typed cross-process identity mismatch: {rendered}"
    );
    assert!(
        rendered.contains("beta_echo"),
        "the failure names the exact definition: {rendered}"
    );
}

/// A frozen tool the server no longer publishes fails child preparation
/// rather than silently starting a weaker child.
#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_mcp_definition_fails_before_the_child_is_ready() {
    if crate::tools::mcp::fixture::serve_if_fixture_mode(
        crate::tools::mcp::fixture::FixtureServer::from_env(),
    )
    .await
    {
        return;
    }
    let dir = lab();
    let server_id = crate::runtime::identity::McpServerId::new("gamma");
    let binding = fixture_binding(
        "local_runtime::composition::subagent_child::tests::a_missing_mcp_definition_fails_before_the_child_is_ready",
        "gamma_",
    );
    let definition = ToolDefinition {
        id: ToolId::new("mcp:gamma:gamma_absent"),
        name: "gamma_absent".to_owned(),
        description: "a tool the server does not publish".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Mcp {
            server_id: server_id.clone(),
        },
    };
    let mcp = ResolvedSubagentTool::Source {
        source_id: crate::capabilities::ToolSourceId::Mcp(server_id.clone()),
        tool_id: definition.id.clone(),
        name: definition.name.clone(),
        identity: crate::tools::mcp::identity::definition_identity(&definition)
            .expect("an MCP definition has an MCP identity"),
        definition,
    };
    let mut child_spec = spec(
        dir.path(),
        vec![builtin("read"), mcp],
        Vec::new(),
        Vec::new(),
    );
    child_spec
        .resolved
        .materialization
        .sources
        .insert(crate::capabilities::ToolSourceId::Mcp(server_id), binding);

    let error = LocalConversationCore::compose_subagent_child(
        &child_spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect_err("a missing definition must not be silently omitted");
    assert!(
        format!("{error:?}").contains("SourceToolMissing"),
        "the failure is a typed missing-definition refusal: {error:?}"
    );
}

/// A frozen MCP requirement is **required**, not optional: a server the
/// child cannot connect fails composition before `Ready` rather than
/// degrading into an availability state and starting weaker than the
/// child was authorized (Issue #145).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_required_mcp_server_fails_child_composition() {
    let dir = lab();
    let server_id = crate::runtime::identity::McpServerId::new("github");
    let definition = ToolDefinition {
        id: ToolId::new("tool-get-issue"),
        name: "get_issue".to_owned(),
        description: "issue".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Mcp {
            server_id: server_id.clone(),
        },
    };
    let mcp = ResolvedSubagentTool::Source {
        source_id: crate::capabilities::ToolSourceId::Mcp(server_id.clone()),
        tool_id: definition.id.clone(),
        name: definition.name.clone(),
        identity: crate::tools::mcp::identity::definition_identity(&definition)
            .expect("an MCP definition has an MCP identity"),
        definition,
    };
    let mut child_spec = spec(
        dir.path(),
        vec![builtin("read"), mcp],
        Vec::new(),
        Vec::new(),
    );
    child_spec.resolved.materialization.sources.insert(
        crate::capabilities::ToolSourceId::Mcp(server_id),
        crate::tools::mcp::McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: "rustx-no-such-mcp-server".to_owned(),
                args: Vec::new(),
                cwd: None,
                environment: std::collections::BTreeMap::new(),
            },
            policy: ToolInvocationPolicy::default(),
        },
    );
    let error = LocalConversationCore::compose_subagent_child(
        &child_spec,
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect_err("a required MCP source that cannot be connected fails composition");
    assert!(
        format!("{error:?}").contains("Mcp"),
        "the failure is an explicit MCP preparation failure: {error:?}"
    );
}

/// A frozen MCP selection whose materialization plane carries no binding
/// for the required server can never be widened by the child: it fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_frozen_mcp_binding_is_never_rediscovered() {
    let dir = lab();
    let server_id = crate::runtime::identity::McpServerId::new("github");
    let definition = ToolDefinition {
        id: ToolId::new("tool-get-issue"),
        name: "get_issue".to_owned(),
        description: "issue".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Mcp {
            server_id: server_id.clone(),
        },
    };
    let mcp = ResolvedSubagentTool::Source {
        source_id: crate::capabilities::ToolSourceId::Mcp(server_id),
        tool_id: definition.id.clone(),
        name: definition.name.clone(),
        identity: crate::tools::mcp::identity::definition_identity(&definition)
            .expect("an MCP definition has an MCP identity"),
        definition,
    };
    let error = LocalConversationCore::compose_subagent_child(
        &spec(
            dir.path(),
            vec![builtin("read"), mcp],
            Vec::new(),
            Vec::new(),
        ),
        &dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect_err("a selection with no frozen binding cannot be materialized");
    assert!(
        format!("{error:?}").contains("was not given a binding"),
        "the child refuses rather than looking a server up for itself: {error:?}"
    );
}
