//! Isolated launch fixture for composition tests. All production entry points
//! receive the same resolver output; this helper supplies explicit test intent.
#![allow(dead_code)]
use rustx::local_runtime::{HostEnvironment, LaunchRequest, ResolvedLaunch, StartupSession};
use std::path::{Path, PathBuf};

/// Write canonical Markdown fixtures from an in-memory collection of frontmatters.
/// `roles` is a test-builder input, removed before any runtime configuration is written.
pub fn write_roles(workspace: &Path, subagents: &mut serde_json::Value) {
    let Some(roles) = subagents.as_object_mut().and_then(|s| s.remove("roles")) else {
        return;
    };
    let roles = roles.as_object().expect("fixture role frontmatters");
    let directory = workspace.join(".agents/subagents");
    std::fs::create_dir_all(&directory).unwrap();
    for (name, metadata) in roles {
        let path = directory.join(format!("{name}.md"));
        let existing =
            std::fs::read_to_string(&path).unwrap_or_else(|_| "Role instructions.\n".into());
        let body = existing
            .strip_prefix("---\n")
            .and_then(|text| text.split_once("\n---\n"))
            .map_or(existing.as_str(), |(_, body)| body);
        std::fs::write(
            &path,
            format!(
                "---\n{}---\n{body}",
                serde_yaml::to_string(metadata).unwrap()
            ),
        )
        .unwrap();
    }
    subagents["definitions"] = serde_json::json!(roles.keys().collect::<Vec<_>>());
}

/// Author the two fixture authorities explicitly. Callers name which fixture
/// members belong to the host; the production resolver never relocates fields.
pub fn write_documents(config: &Path, source: &str, host_fields: &[&str]) {
    let mut project: serde_json::Value = rustx::config_format::parse(source.as_bytes()).unwrap();
    if let Some(subagents) = project.get_mut("subagents") {
        write_roles(&config.parent().unwrap().join("workspace"), subagents);
    }
    let mut user = serde_json::Map::new();
    for field in host_fields {
        if let Some(value) = project.as_object_mut().unwrap().remove(*field) {
            user.insert((*field).into(), value);
        }
    }
    std::fs::write(
        config.parent().unwrap().join("settings.jsonc"),
        serde_json::to_vec(&user).unwrap(),
    )
    .unwrap();
    std::fs::write(config, serde_json::to_vec(&project).unwrap()).unwrap();
}

pub fn grant(root: &std::path::Path, workspace: &std::path::Path) -> PathBuf {
    let home = root.join("host");
    let host = HostEnvironment::from_paths(workspace.into(), home.clone(), None, None).unwrap();
    rustx::local_runtime::launch::change_trust(
        &LaunchRequest {
            workspace: Some(workspace.into()),
            ..Default::default()
        },
        &host,
        rustx::local_runtime::TrustAction::Grant,
    )
    .unwrap();
    home
}

#[derive(Debug, Clone)]
pub struct LaunchFixture {
    pub models: PathBuf,
    pub config: PathBuf,
    pub workspace: PathBuf,
    pub runtime_root: PathBuf,
    pub skill_paths: Vec<PathBuf>,
    pub no_skills: bool,
    pub no_builtin_tools: bool,
    pub no_tools: bool,
    pub startup_session: StartupSession,
    pub session_name: Option<String>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Vec<String>,
}

impl LaunchFixture {
    pub fn request(&self) -> LaunchRequest {
        LaunchRequest {
            models: Some(self.models.clone()),
            config: Some(self.config.clone()),
            workspace: Some(self.workspace.clone()),
            runtime_root: Some(self.runtime_root.clone()),
            skill_paths: self.skill_paths.clone(),
            no_skills: self.no_skills,
            no_builtin_tools: self.no_builtin_tools,
            no_tools: self.no_tools,
            startup_session: self.startup_session.clone(),
            session_name: self.session_name.clone(),
            tools: self.tools.clone(),
            exclude_tools: (!self.exclude_tools.is_empty()).then(|| self.exclude_tools.clone()),
            ..LaunchRequest::default()
        }
    }

    pub fn try_resolve(&self) -> Result<ResolvedLaunch, String> {
        let host = tempfile::tempdir().expect("isolated host");
        let mut environment = HostEnvironment::from_paths(
            std::env::current_dir().unwrap(),
            host.path().into(),
            None,
            None,
        )?;
        // Tests may author host policies alongside their project document.
        environment.config_directory = self.config.parent().unwrap().to_path_buf();
        let request = self.request();
        rustx::local_runtime::launch::change_trust(
            &request,
            &environment,
            rustx::local_runtime::TrustAction::Grant,
        )?;
        rustx::local_runtime::resolve(&request, &environment)
    }

    pub fn resolve(&self) -> ResolvedLaunch {
        self.try_resolve().expect("fixture resolves")
    }

    pub fn locations(&self) -> rustx::local_runtime::LaunchLocations {
        let host = tempfile::tempdir().expect("isolated host");
        let environment = HostEnvironment::from_paths(
            std::env::current_dir().unwrap(),
            host.path().into(),
            None,
            None,
        )
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
