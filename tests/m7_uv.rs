//! Opt-in-by-availability integration for the production uv backend.

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::items_after_statements, clippy::too_many_lines)]
async fn production_uv_materializes_a_local_tool_environment() {
    let uv = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    });
    let Some(uv) = uv else {
        eprintln!("uv unavailable; production uv acceptance not exercised");
        return;
    };
    let directory = tempfile::tempdir().expect("fixture root");
    let package_root = directory.path().join(".agents/tools/local-tool");
    std::fs::create_dir_all(&package_root).expect("package root");
    std::fs::write(
        package_root.join("TOOL.toml"),
        "schema_version = 1\nname = \"local-tool\"\ndescription = \"local\"\nentrypoint = \"tool:main\"\nexecution = \"foreground_only\"\nconcurrency = \"sequential\"\n",
    )
    .expect("manifest");
    std::fs::write(
        package_root.join("input.schema.json"),
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
    )
    .expect("schema");
    std::fs::write(
        package_root.join("pyproject.toml"),
        "[project]\nname = \"local-tool\"\nversion = \"0.1.0\"\nrequires-python = \">=3.11\"\n",
    )
    .expect("project");
    std::fs::write(
        package_root.join("tool.py"),
        "def main(arguments):\n    return arguments\n",
    )
    .expect("source");

    let lock = std::process::Command::new(&uv)
        .args(["lock", "--offline", "--no-config"])
        .current_dir(&package_root)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", directory.path())
        .env("UV_NO_PYTHON_DOWNLOADS", "1")
        .output()
        .expect("run fixture uv lock");
    assert!(
        lock.status.success(),
        "fixture lock failed: {}",
        String::from_utf8_lossy(&lock.stderr)
    );

    let workspace = rustx::tools::Workspace::new(directory.path()).expect("workspace");
    let package = rustx::tools::python::PythonToolDiscovery::new(&workspace)
        .discover()
        .expect("discover")
        .pop()
        .expect("package");
    let store = rustx::tools::python::PythonToolStore::new(directory.path().join("runtime"))
        .expect("store");
    let published = store.publish(&package).expect("publish source");
    let environment = store
        .ensure_environment(&published)
        .await
        .expect("materialize");
    assert!(environment.root.join("RUSTX_ENV_MANIFEST.json").is_file());
    assert!(environment.root.join("bin/python").is_file());
    let marker = std::fs::read_to_string(environment.root.join("RUSTX_ENV_MANIFEST.json"))
        .expect("environment marker");
    assert!(marker.contains(environment.digest.as_str()));

    struct NoProgress;
    impl rustx::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
    }
    let executor = rustx::tools::python::PythonToolExecutor::new(&store, published, environment)
        .expect("executor");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let runtime = rustx::tools::runtime::ConversationToolRuntime::new(
        rustx::runtime::identity::ConversationId::new("m7-python"),
        directory.path(),
        artifacts.path(),
    )
    .expect("tool runtime");
    let progress = NoProgress;
    let result = rustx::tools::executor::ToolExecutor::execute(
        &executor,
        rustx::tools::types::ToolInvocation {
            call_id: rustx::runtime::identity::ToolCallId::new("call-python"),
            tool_id: rustx::runtime::identity::ToolId::new(rustx::tools::python::python_tool_id(
                "local-tool",
            )),
            tool_name: "local-tool".to_owned(),
            mode: rustx::tools::types::ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"answer": 42}),
        },
        rustx::tools::executor::ToolExecutionContext {
            conversation_id: runtime.conversation_id(),
            execution_id: None,
            cancellation: rustx::runtime::CancellationSignal::new(),
            workspace: runtime.workspace(),
            progress: &progress,
            artifacts: runtime.artifacts(),
            environment: runtime.environment(),
        },
    )
    .await;
    assert!(matches!(
        result.status,
        rustx::tools::types::ToolExecutionStatus::Success
    ));
}
