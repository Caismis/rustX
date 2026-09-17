//! Isolated launch fixture for composition tests. All production entry points
//! receive the same resolver output; this helper supplies explicit test intent.
#![allow(dead_code)]
use rustx::local_runtime::{AdmittedSessionConfig, HostEnvironment, LaunchRequest, StartupSession};
use std::path::{Path, PathBuf};

/// Write canonical TOML Agent fixtures from an in-memory fixture builder.
pub fn write_roles(workspace: &Path, subagents: &mut serde_json::Value) {
    let Some(roles) = subagents.as_object_mut().and_then(|s| s.remove("roles")) else {
        return;
    };
    let roles = roles.as_object().expect("fixture Agents");
    let directory = workspace.join(".agents/agents");
    std::fs::create_dir_all(&directory).unwrap();
    // This fixture builder authors the complete desired filesystem resource set.
    // Removing a fixture Agent removes its canonical file, never a settings entry.
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "toml")
            && !roles.contains_key(path.file_stem().unwrap().to_str().unwrap())
        {
            std::fs::remove_file(path).unwrap();
        }
    }
    for (name, metadata) in roles {
        let path = directory.join(format!("{name}.toml"));
        let existing =
            std::fs::read_to_string(&path).unwrap_or_else(|_| "Role instructions.\n".into());
        let body = toml::from_str::<serde_json::Value>(&existing)
            .ok()
            .and_then(|v| {
                v.get("instructions")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .unwrap_or(existing);
        let mut document = metadata.clone();
        document["instructions"] = body.into();
        std::fs::write(path, toml::to_string_pretty(&document).unwrap()).unwrap();
    }
}

/// Author only MCP resource definitions at the selected canonical scope.
pub fn write_mcp(workspace: &Path, definitions: &serde_json::Value) {
    std::fs::create_dir_all(workspace.join(".agents")).unwrap();
    std::fs::write(
        workspace.join(".agents/mcp.toml"),
        toml::to_string_pretty(&serde_json::json!({"mcp_servers": definitions})).unwrap(),
    )
    .unwrap();
}

/// Write a complete authored User document. Resource fixtures are authored separately.
pub fn write_document(config: &Path, source: &str) {
    std::fs::write(config, source).expect("fixture rustx.toml");
}

#[derive(Debug, Clone)]
pub struct LaunchFixture {
    pub config: PathBuf,
    pub workspace: PathBuf,
    pub runtime_root: PathBuf,
    pub startup_session: StartupSession,
    pub session_name: Option<String>,
}

impl LaunchFixture {
    pub async fn compose(
        &self,
        dependencies: &rustx::local_runtime::LocalRuntimeDependencies,
    ) -> Result<rustx::local_runtime::LocalSessionClient, rustx::local_runtime::LocalRuntimeError>
    {
        let dependencies = rustx::local_runtime::LocalRuntimeDependencies {
            startup_session: self.startup_session.clone(),
            session_name: self.session_name.clone(),
            credentials: dependencies.credentials.clone(),
            estimator: dependencies.estimator.clone(),
            child_program: dependencies.child_program.clone(),
        };
        rustx::local_runtime::LocalSessionClient::compose(&self.resolve(), &dependencies).await
    }

    pub fn request(&self) -> LaunchRequest {
        LaunchRequest {
            config: Some(self.config.clone()),
            workspace: Some(self.workspace.clone()),
            runtime_root: Some(self.runtime_root.clone()),
            startup_session: self.startup_session.clone(),
            session_name: self.session_name.clone(),
            ..LaunchRequest::default()
        }
    }

    pub fn try_resolve(&self) -> Result<AdmittedSessionConfig, String> {
        let host = self
            .config
            .parent()
            .expect("fixture configuration parent")
            .join(".test-host");
        std::fs::create_dir_all(&host).expect("isolated host");
        let environment = HostEnvironment::from_paths(std::env::current_dir().unwrap(), host)?;
        let request = self.request();
        rustx::local_runtime::resolve(&request, &environment)
    }

    pub fn resolve(&self) -> AdmittedSessionConfig {
        self.try_resolve().expect("fixture resolves")
    }

    pub fn locations(&self) -> rustx::local_runtime::SessionLocations {
        let host = tempfile::tempdir().expect("isolated host");
        let environment =
            HostEnvironment::from_paths(std::env::current_dir().unwrap(), host.path().into())
                .unwrap();
        rustx::local_runtime::launch::resolve_locations(&self.request(), &environment)
            .unwrap()
            .0
    }

    pub fn artifacts_root(&self) -> PathBuf {
        self.runtime_root.join("artifacts")
    }
    pub fn environment_store_root(&self) -> PathBuf {
        self.runtime_root.join("environments")
    }
    pub fn environment_store_root_for(
        &self,
        conversation: &rustx::runtime::identity::ConversationId,
    ) -> PathBuf {
        self.environment_store_root().join(conversation.as_str())
    }
}
