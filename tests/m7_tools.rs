//! Deterministic M7 contract coverage that does not require a public registry.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use rustx::runtime::identity::ToolId;
use rustx::tools::Workspace;
use rustx::tools::environment::{ToolEnvironment, ToolEnvironmentOverlay};
use rustx::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
use rustx::tools::python::PythonToolDiscovery;
use rustx::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolInvocation, ToolInvocationPolicy, ToolOrigin, ToolReplayPolicy,
};

struct NoopExecutor;

impl ToolExecutor for NoopExecutor {
    fn execute<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async {
            ToolExecutionResult {
                status: rustx::tools::types::ToolExecutionStatus::Failed {
                    error: "noop".to_owned(),
                },
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
            }
        })
    }
}

fn write_python_package(root: &std::path::Path, name: &str, description: &str) {
    let package = root.join(".agents/tools").join(name);
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("TOOL.toml"),
        format!(
            "schema_version = 1\nname = {name:?}\ndescription = {description:?}\nentrypoint = \"tool:main\"\nexecution = \"foreground_only\"\nconcurrency = \"sequential\"\n"
        ),
    )
    .expect("manifest");
    std::fs::write(
        package.join("input.schema.json"),
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
    )
    .expect("schema");
    std::fs::write(
        package.join("pyproject.toml"),
        "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\nrequires-python = \">=3.11\"\n",
    )
    .expect("project");
    std::fs::write(package.join("uv.lock"), "version = 1\nrevision = 1\n").expect("lock");
    std::fs::write(
        package.join("tool.py"),
        "def main(arguments):\n    return arguments\n",
    )
    .expect("source");
}

#[test]
fn python_discovery_is_sorted_and_tool_version_tracks_complete_snapshot() {
    let directory = tempfile::tempdir().expect("workspace");
    write_python_package(directory.path(), "zeta", "Zeta");
    write_python_package(directory.path(), "alpha", "Alpha");
    let workspace = Workspace::new(directory.path()).expect("workspace");
    let packages = PythonToolDiscovery::new(&workspace)
        .discover()
        .expect("discover");
    assert_eq!(
        packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
    let original = packages[0].tool_version_id.clone();

    std::fs::write(
        directory.path().join(".agents/tools/alpha/TOOL.toml"),
        "schema_version = 1\nname = \"alpha\"\ndescription = \"changed\"\nentrypoint = \"tool:main\"\nexecution = \"foreground_only\"\nconcurrency = \"sequential\"\n",
    )
    .expect("change description");
    let changed = PythonToolDiscovery::new(&workspace)
        .discover()
        .expect("rediscover")[0]
        .tool_version_id
        .clone();
    assert_ne!(original, changed);
}

#[cfg(unix)]
#[test]
fn python_discovery_rejects_symlinked_package_content() {
    let directory = tempfile::tempdir().expect("workspace");
    write_python_package(directory.path(), "alpha", "Alpha");
    std::os::unix::fs::symlink(
        directory.path().join(".agents/tools/alpha/tool.py"),
        directory.path().join(".agents/tools/alpha/linked.py"),
    )
    .expect("symlink");
    let workspace = Workspace::new(directory.path()).expect("workspace");
    assert!(PythonToolDiscovery::new(&workspace).discover().is_err());
}

#[test]
fn composed_registry_rejects_global_name_collisions() {
    let schema =
        serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false});
    let definition = |id: &str, name: &str| ToolDefinition {
        id: ToolId::new(id),
        name: name.to_owned(),
        description: String::new(),
        input_schema: schema.clone(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    };
    let base = ToolRegistry::new()
        .compose([(
            definition("native", "same"),
            Arc::new(NoopExecutor) as Arc<dyn ToolExecutor>,
        )])
        .expect("base");
    let collision = base.compose([(
        definition("mcp", "same"),
        Arc::new(NoopExecutor) as Arc<dyn ToolExecutor>,
    )]);
    assert!(matches!(
        collision,
        Err(rustx::tools::ToolRegistryError::DuplicateName(_))
    ));
}

#[test]
fn replacement_python_overlay_excludes_skill_overlay() {
    let base = ToolEnvironment::new().with_overlay(&ToolEnvironmentOverlay::node(
        std::path::Path::new("/skill-node"),
    ));
    let replacement = base.with_replacement_overlay(&ToolEnvironmentOverlay::python(
        std::path::Path::new("/tool-env"),
    ));
    let entries = replacement.child_environment(std::path::Path::new("/workspace"));
    assert_eq!(entries[0].1, "/tool-env/bin:/usr/local/bin:/usr/bin:/bin");
    assert!(
        entries
            .iter()
            .any(|(key, value)| key == "VIRTUAL_ENV" && value == "/tool-env")
    );
    assert!(!entries.iter().any(|(key, _)| key == "NODE_PATH"));
}

#[test]
fn external_policy_and_progress_are_provider_neutral() {
    let policy = ToolInvocationPolicy::new(
        ToolExecutionPolicy::BackgroundOnly,
        ToolConcurrencyPolicy::Parallel,
    );
    assert_eq!(policy.execution, ToolExecutionPolicy::BackgroundOnly);
    let progress = rustx::tools::types::ToolProgress {
        message: Some("fractional".to_owned()),
        completed: Some(1.5),
        total: Some(2.25),
    };
    let round_trip: rustx::tools::types::ToolProgress =
        serde_json::from_value(serde_json::to_value(progress).expect("progress json"))
            .expect("progress round trip");
    assert_eq!(round_trip.completed, Some(1.5));
    assert_eq!(round_trip.total, Some(2.25));
}
