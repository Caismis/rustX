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
        .ensure_environment(&published, &rustx::runtime::CancellationSignal::new())
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
    let executor = rustx::tools::python::PythonToolExecutor::new(&store, published, environment);
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
        rustx::tools::executor::ToolExecutionContext::new(
            runtime.conversation_id(),
            None,
            rustx::runtime::ExecutionCancellation::detached(
                rustx::runtime::CancellationSignal::new(),
                rustx::runtime::types::CancellationReason::UserRequested,
            ),
            runtime.workspace(),
            &progress,
            runtime.artifacts(),
            runtime.tool_output(),
            runtime.environment(),
        ),
    )
    .await;
    assert!(matches!(
        result.status,
        rustx::tools::types::ToolExecutionStatus::Success
    ));
}

/// Issue #10 acceptance: conflicting Python dependencies across two tools.
///
/// The fixture is fully local and offline: each tool vendors its own
/// prebuilt wheel of `dep-x` and commits a `uv.lock` generated with
/// `uv lock --offline`. The store materialization (`lock --check` +
/// `sync --frozen` with `UV_PYTHON` pinned) resolves nothing and touches no
/// public index: the lock files contain only local path sources.
///
/// Proofs: distinct environment digests, both environments materialize,
/// both tools execute, tool A observes dep-x v1, tool B observes dep-x v2,
/// no `PyPI` source appears in any lock.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::items_after_statements, clippy::too_many_lines)]
async fn conflicting_local_dependencies_isolate_versions_and_materialize_offline() {
    let uv = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    });
    let Some(uv) = uv else {
        eprintln!("uv unavailable; conflicting-dependency acceptance not exercised");
        return;
    };
    let directory = tempfile::tempdir().expect("fixture root");
    let tools_root = directory.path().join(".agents/tools");

    // Build the two dep-x wheels (v1 and v2) and lay out both tool packages
    // with their own vendored wheel.
    for (tool, version) in [("tool-a", "1.0.0"), ("tool-b", "2.0.0")] {
        let package = tools_root.join(tool);
        let dep_x = package.join("deps/dep-x");
        let dep_x_module = dep_x.join("dep_x");
        std::fs::create_dir_all(&dep_x_module).expect("dep-x module dir");
        std::fs::write(
            dep_x.join("pyproject.toml"),
            format!(
                "[project]\nname = \"dep-x\"\nversion = \"{version}\"\nrequires-python = \">=3.11\"\n"
            ),
        )
        .expect("dep-x project");
        std::fs::write(
            dep_x_module.join("__init__.py"),
            format!("VERSION = \"{version}\"\n"),
        )
        .expect("dep-x source");
        let build = std::process::Command::new(&uv)
            .args(["build", "--wheel", "--no-sources", "--no-config"])
            .current_dir(&dep_x)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", directory.path())
            .env("UV_NO_PYTHON_DOWNLOADS", "1")
            .output()
            .expect("build dep-x wheel");
        assert!(
            build.status.success(),
            "dep-x wheel build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        std::fs::write(
            package.join("TOOL.toml"),
            format!(
                "schema_version = 1\nname = {tool:?}\ndescription = \"{tool}\"\nentrypoint = \"tool:main\"\nexecution = \"foreground_only\"\nconcurrency = \"sequential\"\n"
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
            format!(
                "[project]\nname = \"{tool}\"\nversion = \"0.1.0\"\nrequires-python = \">=3.11\"\ndependencies = [\"dep-x\"]\n\n[tool.uv.sources]\ndep-x = {{ path = \"deps/dep-x/dist/dep_x-{version}-py3-none-any.whl\" }}\n"
            ),
        )
        .expect("project");
        std::fs::write(
            package.join("tool.py"),
            "import dep_x\n\ndef main(arguments):\n    return {\"dep_x_version\": dep_x.VERSION}\n",
        )
        .expect("tool source");
        let lock = std::process::Command::new(&uv)
            .args(["lock", "--offline", "--no-config"])
            .current_dir(&package)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", directory.path())
            .env("UV_NO_PYTHON_DOWNLOADS", "1")
            .output()
            .expect("fixture uv lock");
        assert!(
            lock.status.success(),
            "fixture lock failed for {tool}: {}",
            String::from_utf8_lossy(&lock.stderr)
        );
        let lock_bytes = std::fs::read(package.join("uv.lock")).expect("uv.lock");
        let lock_text = String::from_utf8_lossy(&lock_bytes);
        assert!(
            !lock_text.contains("pypi.org") && !lock_text.contains("index-url"),
            "the {tool} lock must reference only local sources"
        );
    }

    let workspace = rustx::tools::Workspace::new(directory.path()).expect("workspace");
    let packages = rustx::tools::python::PythonToolDiscovery::new(&workspace)
        .discover()
        .expect("discover");
    assert_eq!(packages.len(), 2);
    let store = rustx::tools::python::PythonToolStore::new(directory.path().join("runtime"))
        .expect("store");

    let mut environments = std::collections::BTreeMap::new();
    for package in &packages {
        let published = store.publish(package).expect("publish source");
        let environment = store
            .ensure_environment(&published, &rustx::runtime::CancellationSignal::new())
            .await
            .expect("materialize offline from local wheels");
        assert!(environment.root.join("bin/python").is_file());
        environments.insert(package.name.clone(), (published, environment));
    }

    let (published_a, environment_a) = environments.get("tool-a").expect("tool-a env");
    let (published_b, environment_b) = environments.get("tool-b").expect("tool-b env");
    assert_ne!(
        environment_a.digest, environment_b.digest,
        "conflicting dependency versions must produce distinct environment identities"
    );

    let artifacts = tempfile::tempdir().expect("artifacts");
    let runtime = rustx::tools::runtime::ConversationToolRuntime::new(
        rustx::runtime::identity::ConversationId::new("m7-conflict"),
        directory.path(),
        artifacts.path(),
    )
    .expect("tool runtime");

    async fn observe(
        store: &rustx::tools::python::PythonToolStore,
        published: &rustx::tools::python::PublishedPythonTool,
        environment: &rustx::tools::python::PythonToolEnvironment,
        runtime: &rustx::tools::runtime::ConversationToolRuntime,
        tool_name: &str,
    ) -> serde_json::Value {
        struct NoProgress;
        impl rustx::tools::executor::ProgressReporter for NoProgress {
            fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
        }
        let executor = rustx::tools::python::PythonToolExecutor::new(
            store,
            published.clone(),
            environment.clone(),
        );
        let result = rustx::tools::executor::ToolExecutor::execute(
            &executor,
            rustx::tools::types::ToolInvocation {
                call_id: rustx::runtime::identity::ToolCallId::new(format!("{tool_name}-call")),
                tool_id: rustx::runtime::identity::ToolId::new(
                    rustx::tools::python::python_tool_id(tool_name),
                ),
                tool_name: tool_name.to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            rustx::tools::executor::ToolExecutionContext::new(
                runtime.conversation_id(),
                None,
                rustx::runtime::ExecutionCancellation::detached(
                    rustx::runtime::CancellationSignal::new(),
                    rustx::runtime::types::CancellationReason::UserRequested,
                ),
                runtime.workspace(),
                &NoProgress,
                runtime.artifacts(),
                runtime.tool_output(),
                runtime.environment(),
            ),
        )
        .await;
        assert!(
            matches!(
                result.status,
                rustx::tools::types::ToolExecutionStatus::Success
            ),
            "tool {tool_name} failed: {result:?}"
        );
        let Some(rustx::tools::types::ToolResultContent::Json { value }) = result.content.first()
        else {
            panic!("tool {tool_name} returned no JSON result");
        };
        value.clone()
    }

    let value_a = observe(&store, published_a, environment_a, &runtime, "tool-a").await;
    let value_b = observe(&store, published_b, environment_b, &runtime, "tool-b").await;
    assert_eq!(
        value_a
            .get("dep_x_version")
            .and_then(serde_json::Value::as_str),
        Some("1.0.0"),
        "tool A must observe dep-x v1"
    );
    assert_eq!(
        value_b
            .get("dep_x_version")
            .and_then(serde_json::Value::as_str),
        Some("2.0.0"),
        "tool B must observe dep-x v2"
    );
}
