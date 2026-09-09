//! Isolated launch fixture for composition tests. All production entry points
//! receive the same resolver output; this helper supplies explicit test intent.
#![allow(dead_code)]
use rustx::local_runtime::{HostEnvironment, LaunchRequest, ResolvedLaunch, StartupSession};

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
use std::path::PathBuf;

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
        let environment = HostEnvironment::from_paths(
            std::env::current_dir().unwrap(),
            host.path().into(),
            None,
            None,
        )?;
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
