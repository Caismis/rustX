//! Opt-in-by-availability integration for the managed Python package store
//! (Issue #174): the production `uv` backend materializes the isolated
//! `FastMCP` environment of a discovered package, and a second preparation of
//! the unchanged package reuses the published state.

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_uv_materializes_a_managed_package_environment() {
    let uv = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    });
    if uv.is_none() {
        eprintln!("uv unavailable; production uv acceptance not exercised");
        return;
    }
    let directory = tempfile::tempdir().expect("fixture root");
    let package_root = directory.path().join(".agents/tools/local-tool");
    std::fs::create_dir_all(&package_root).expect("package root");
    std::fs::write(
        package_root.join("server.py"),
        "from fastmcp import FastMCP\n\nmcp = FastMCP('local-tool')\n\n\n@mcp.tool\ndef echo(text: str) -> str:\n    return text\n",
    )
    .expect("server source");
    std::fs::write(package_root.join("requirements.txt"), "# none\n").expect("requirements");

    let workspace = rustx::tools::Workspace::new(directory.path()).expect("workspace");
    let discovered = rustx::tools::python::discover_python_packages(&workspace).expect("discover");
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].server_id.as_str(), "python:local-tool");
    let package = discovered[0]
        .outcome
        .as_ref()
        .expect("valid package")
        .clone();

    let store = rustx::tools::python::PythonToolStore::new(directory.path().join("runtime"))
        .expect("store");
    let prepared = store
        .ensure_prepared(&package, &rustx::runtime::CancellationSignal::new())
        .await
        .expect("materialize");
    let interpreter = prepared.state_dir.join("venv/bin/python");
    assert!(
        interpreter.is_file(),
        "the venv interpreter exists: {interpreter:?}"
    );
    assert!(prepared.state_dir.join("manifest.json").is_file());
    assert!(prepared.state_dir.join("uv.lock").is_file());
    assert!(prepared.state_dir.join("source/server.py").is_file());

    let binding = prepared.server_binding();
    let rustx::tools::mcp::McpTransportConfig::Stdio {
        program,
        args,
        cwd,
        environment,
    } = &binding.transport
    else {
        panic!("the managed package binding is a stdio launch");
    };
    assert_eq!(program, &interpreter.display().to_string());
    assert_eq!(
        args,
        &[
            "-m".to_owned(),
            "fastmcp.cli".to_owned(),
            "run".to_owned(),
            format!(
                "{}:mcp",
                prepared.state_dir.join("source/server.py").display()
            ),
            "--skip-env".to_owned(),
            "--no-banner".to_owned(),
        ]
    );
    assert!(cwd.is_none(), "the launch runs from the workspace root");
    assert_eq!(
        environment
            .get("FASTMCP_SHOW_SERVER_BANNER")
            .map(String::as_str),
        Some("false")
    );
    assert_eq!(
        environment
            .get("FASTMCP_CHECK_FOR_UPDATES")
            .map(String::as_str),
        Some("off")
    );
    assert_eq!(
        environment
            .get("PYTHONDONTWRITEBYTECODE")
            .map(String::as_str),
        Some("1")
    );

    // An unchanged package reuses the published state verbatim.
    let reused = store
        .ensure_prepared(&package, &rustx::runtime::CancellationSignal::new())
        .await
        .expect("reuse");
    assert_eq!(reused, prepared);
}
