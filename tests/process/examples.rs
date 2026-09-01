//! The committed `examples/local-runtime/` configuration composes its real
//! resources through the production composition path.
//!
//! This is a local-composition boundary test: composing the example
//! registers its Python `echo` tool through the real tool pipeline, which
//! requires the `uv` toolchain. It therefore lives in the `process` target
//! (whose jobs install Python + uv), not the pure `contracts` target.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustx::local_runtime::{
    HeadlessConversationRuntime, LocalRuntimeDependencies, LocalRuntimePaths, StartupSession,
};
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime::RuntimeResourceSnapshot;
use rustx::runtime::workflow::{WorkflowId, WorkflowNodeProgram};

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime")
}

const REQUIRED_EXAMPLE_FILES: &[&str] = &[
    "AGENTS.md",
    ".agents/skills/review-guidance/SKILL.md",
    ".agents/tools/echo/TOOL.toml",
    ".agents/tools/echo/input.schema.json",
    ".agents/tools/echo/pyproject.toml",
    ".agents/tools/echo/uv.lock",
    ".agents/tools/echo/tool.py",
    ".agents/subagents/navigator/instructions.md",
    ".agents/subagents/navigator/AGENTS.md",
    ".agents/subagents/reviewer/instructions.md",
    ".agents/subagents/reviewer/AGENTS.md",
    ".agents/workflows/review_pr.yaml",
    ".agents/workflows/parallel_review.yaml",
];

fn assert_example_files_exist(workspace: &Path) {
    for relative in REQUIRED_EXAMPLE_FILES {
        assert!(
            workspace.join(relative).is_file(),
            "missing example file {relative}"
        );
    }
}

fn assert_example_resource_snapshot(resources: &RuntimeResourceSnapshot) {
    assert_eq!(
        resources
            .subagents()
            .names()
            .into_iter()
            .map(rustx::runtime::subagent::SubagentName::as_str)
            .collect::<Vec<_>>(),
        vec!["navigator", "reviewer"]
    );
    assert_eq!(
        resources
            .subagent_main_admission()
            .iter()
            .map(rustx::runtime::subagent::SubagentName::as_str)
            .collect::<Vec<_>>(),
        vec!["navigator"]
    );
    assert_eq!(
        resources
            .subagent_workflow_admission()
            .iter()
            .map(rustx::runtime::subagent::SubagentName::as_str)
            .collect::<Vec<_>>(),
        vec!["reviewer"]
    );
    assert!(
        resources
            .skill_catalog()
            .expect("project Skill catalog")
            .contains("review-guidance")
    );

    let review = resources
        .workflows()
        .get(&WorkflowId::parse("review_pr").expect("workflow id"))
        .expect("review_pr is registered");
    assert!(
        review
            .nodes()
            .values()
            .any(|node| matches!(node, WorkflowNodeProgram::Agent(_)))
    );
    assert!(
        review
            .nodes()
            .values()
            .any(|node| matches!(node, WorkflowNodeProgram::Branch { .. }))
    );
    assert!(
        review
            .nodes()
            .values()
            .any(|node| matches!(node, WorkflowNodeProgram::Return { .. }))
    );

    let parallel = resources
        .workflows()
        .get(&WorkflowId::parse("parallel_review").expect("workflow id"))
        .expect("parallel_review is registered");
    assert!(
        parallel
            .nodes()
            .values()
            .any(|node| matches!(node, WorkflowNodeProgram::Parallel { .. }))
    );
    assert!(
        parallel
            .nodes()
            .values()
            .any(|node| matches!(node, WorkflowNodeProgram::Return { .. }))
    );

    let tool_names = resources.capability().tool_registry().names();
    assert!(tool_names.contains(&"review_pr"));
    assert!(tool_names.contains(&"parallel_review"));
    assert!(tool_names.contains(&"subagent"));
    assert!(tool_names.contains(&"echo"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checked_in_local_runtime_example_composes_its_real_resources() {
    let root = tempfile::tempdir().expect("temporary runtime root");
    let examples = examples_root();
    let workspace = examples.join("workspace");
    assert_example_files_exist(&workspace);
    let runtime = HeadlessConversationRuntime::compose(
        &LocalRuntimePaths {
            models: examples.join("models.jsonc"),
            config: examples.join("rustx.jsonc"),
            // Keep this test independent of the developer's home directory
            // while exercising the actual checked-in project Skill root.
            skill_paths: vec![workspace.join(".agents/skills")],
            no_skills: true,
            no_builtin_tools: false,
            no_tools: false,
            startup_session: StartupSession::Empty,
            session_name: None,
            tools: None,
            exclude_tools: Vec::new(),
            workspace,
            runtime_root: root.path().join("runtime-root"),
        },
        &LocalRuntimeDependencies {
            credentials: Arc::new(MapCredentialEnvironment::new([(
                "RUSTX_EXAMPLE_API_KEY".to_owned(),
                "smoke-test-secret".to_owned(),
            )])),
            ..LocalRuntimeDependencies::default()
        },
    )
    .await
    .expect("checked-in local-runtime example must compose");

    assert_example_resource_snapshot(runtime.runtime().runtime_resources().as_ref());
}
