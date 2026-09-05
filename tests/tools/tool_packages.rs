//! Deterministic managed-Python-package contract coverage (Issue #174) that
//! does not require a public registry.

use std::sync::Arc;

use rustx::runtime::identity::ToolId;
use rustx::tools::ToolProgressCapability;
use rustx::tools::Workspace;
use rustx::tools::executor::{
    ToolExecutionContext, ToolExecutionHandle, ToolExecutor, ToolRegistry,
};
use rustx::tools::python::discover_python_packages;
use rustx::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolExecutionResult, ToolInvocation, ToolInvocationPolicy, ToolOrigin, ToolReplayPolicy,
};

struct NoopExecutor;

impl ToolExecutor for NoopExecutor {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        ToolExecutionHandle::settled_by_operation(
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
                    managed_output: None,
                }
            }),
            context.cancellation.clone(),
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

fn write_python_package(root: &std::path::Path, name: &str) {
    let package = root.join(".agents/tools").join(name);
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("server.py"),
        format!("from fastmcp import FastMCP\nmcp = FastMCP({name:?})\n"),
    )
    .expect("server source");
    std::fs::write(package.join("requirements.txt"), "# none\n").expect("requirements");
}

#[test]
fn python_discovery_is_sorted_and_tracks_current_package_bytes() {
    let directory = tempfile::tempdir().expect("workspace");
    write_python_package(directory.path(), "zeta");
    write_python_package(directory.path(), "alpha");
    let workspace = Workspace::new(directory.path()).expect("workspace");
    let discovered = discover_python_packages(&workspace).expect("discover");
    assert_eq!(
        discovered
            .iter()
            .map(|entry| entry.server_id.as_str().to_owned())
            .collect::<Vec<_>>(),
        ["python:alpha", "python:zeta"]
    );
    let original = discovered[0]
        .outcome
        .as_ref()
        .expect("alpha package")
        .clone();

    std::fs::write(
        directory.path().join(".agents/tools/alpha/server.py"),
        "from fastmcp import FastMCP\nmcp = FastMCP('changed')\n",
    )
    .expect("change server source");
    let changed = discover_python_packages(&workspace).expect("rediscover")[0]
        .outcome
        .as_ref()
        .expect("alpha package")
        .clone();
    assert_ne!(
        original.files, changed.files,
        "the frozen package snapshot tracks the current package bytes"
    );
}

/// A package that declares the rustX-managed `fastmcp` dependency is
/// rejected in place with a diagnostic naming both the package and the
/// managed pin (Issue #174).
#[test]
fn python_discovery_rejects_a_self_pinning_package_in_place() {
    let directory = tempfile::tempdir().expect("workspace");
    let package = directory.path().join(".agents/tools/selfpinning");
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("server.py"),
        "from fastmcp import FastMCP\nmcp = FastMCP('selfpinning')\n",
    )
    .expect("server source");
    std::fs::write(
        package.join("requirements.txt"),
        format!(
            "fastmcp=={}\n",
            rustx::tools::python::MANAGED_FASTMCP_VERSION
        ),
    )
    .expect("requirements");
    let workspace = Workspace::new(directory.path()).expect("workspace");
    let discovered = discover_python_packages(&workspace).expect("discover");
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].server_id.as_str(), "python:selfpinning");
    let Err(rustx::tools::python::PythonToolError::InvalidPackage(message)) =
        &discovered[0].outcome
    else {
        panic!(
            "the self-pinning package is rejected: {:?}",
            discovered[0].outcome
        );
    };
    assert!(
        message.contains("\"selfpinning\""),
        "the diagnostic names the package: {message}"
    );
    assert!(
        message.contains(rustx::tools::python::MANAGED_FASTMCP_VERSION),
        "the diagnostic names the managed pin: {message}"
    );
}

#[cfg(unix)]
#[test]
fn python_discovery_rejects_symlinked_package_content() {
    let directory = tempfile::tempdir().expect("workspace");
    write_python_package(directory.path(), "alpha");
    std::os::unix::fs::symlink(
        directory.path().join(".agents/tools/alpha/server.py"),
        directory.path().join(".agents/tools/alpha/linked.py"),
    )
    .expect("symlink");
    let workspace = Workspace::new(directory.path()).expect("workspace");
    let discovered = discover_python_packages(&workspace).expect("discover");
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].server_id.as_str(), "python:alpha");
    assert!(
        matches!(
            &discovered[0].outcome,
            Err(rustx::tools::python::PythonToolError::InvalidPackage(message))
                if message.contains("symlink")
        ),
        "the symlinked package is rejected in place: {:?}",
        discovered[0].outcome
    );
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
        approval_policy: ToolApprovalPolicy::Never,
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
fn external_policy_and_progress_are_provider_neutral() {
    let policy = ToolInvocationPolicy::new(
        ToolExecutionPolicy::BackgroundOnly,
        ToolConcurrencyPolicy::Parallel,
        ToolApprovalPolicy::Never,
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
