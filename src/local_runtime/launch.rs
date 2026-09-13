//! Local CLI input translation and trust commands. Configuration semantics live
//! exclusively in `configuration::UserConfigManager`.
use super::composition::StartupSession;
use super::configuration::{
    AdmittedSessionConfig, ProspectiveSessionConfig, SessionConfigInput, SessionLocations,
    UserConfigManager, UserConfigSources, absolute, bind_user_source_path, canonical_directory,
    canonical_settings_source, present_on_disk, trust_root,
};
use super::diagnostics::LaunchFailure;
use crate::bounded_file::read_bounded;
use std::path::{Path, PathBuf};
/// Raw user intent. Absence is preserved until resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchRequest {
    pub models: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub runtime_root: Option<PathBuf>,
    pub model: Option<String>,
    pub trust: Option<TrustAction>,
    pub skill_paths: Vec<PathBuf>,
    pub no_automatic_skills: bool,
    pub no_builtin_tools: bool,
    pub no_direct_tools: bool,
    pub startup_session: StartupSession,
    pub session_name: Option<String>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustAction {
    Grant,
    Revoke,
}

/// Captured once; tests supply isolated snapshots without changing process globals.
#[derive(Debug, Clone)]
pub struct HostEnvironment {
    pub launch_directory: PathBuf,
    /// The captured home directory.
    ///
    /// This is the one owner of the `global` Skill source root
    /// (`<home>/.agents/skills`, Issue #280). It is deliberately separate
    /// from `config_directory`: the global Skill root is a user-owned
    /// Agent-resource location, not rustX configuration state, and it is
    /// never derived by shell-style string expansion at a use site.
    pub home_directory: PathBuf,
    pub config_directory: PathBuf,
    pub state_directory: PathBuf,
}

impl HostEnvironment {
    /// Capture the supported Unix host paths. Relative XDG paths are errors.
    ///
    /// # Errors
    /// Fails if the launch directory or required absolute host paths are unavailable.
    pub fn capture() -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is required to locate user configuration and trust")?;
        let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
        Self::from_paths(
            std::env::current_dir().map_err(|e| e.to_string())?,
            home,
            config,
            state,
        )
    }

    /// Build an isolated host snapshot from explicit paths.
    ///
    /// # Errors
    /// Rejects relative HOME or XDG roots.
    #[allow(clippy::needless_pass_by_value)] // captured path inputs transfer together
    pub fn from_paths(
        launch_directory: PathBuf,
        home: PathBuf,
        config: Option<PathBuf>,
        state: Option<PathBuf>,
    ) -> Result<Self, String> {
        if !home.is_absolute()
            || config.as_ref().is_some_and(|p| !p.is_absolute())
            || state.as_ref().is_some_and(|p| !p.is_absolute())
        {
            return Err("HOME, XDG_CONFIG_HOME and XDG_STATE_HOME must be absolute paths".into());
        }
        // Use the same XDG convention on Linux and macOS; no platform-dependent fallback chain.
        Ok(Self {
            launch_directory,
            config_directory: config.unwrap_or_else(|| home.join(".config")).join("rustx"),
            state_directory: state
                .unwrap_or_else(|| home.join(".local/state"))
                .join("rustx"),
            home_directory: home,
        })
    }
}

impl LaunchRequest {
    /// Translate CLI-relative spellings and discovery into explicit shared inputs.
    /// # Errors
    /// Invalid host paths or workspace discovery fail without effects.
    pub fn session_input(
        &self,
        host: &HostEnvironment,
    ) -> Result<(UserConfigManager, SessionConfigInput), LaunchFailure> {
        let (sources, input) = self.source_inputs(host)?;
        let launch = canonical_directory(&host.launch_directory)?;
        let manager = UserConfigManager::bootstrap(
            sources,
            self.models.as_ref().map(|p| absolute(&launch, p)),
            self.runtime_root.as_ref().map(|p| absolute(&launch, p)),
        )?;
        Ok((manager, input))
    }

    fn source_inputs(
        &self,
        host: &HostEnvironment,
    ) -> Result<(UserConfigSources, SessionConfigInput), String> {
        let launch = canonical_directory(&host.launch_directory)?;
        let cwd = match &self.workspace {
            Some(path) => canonical_directory(&absolute(&launch, path))?,
            None => discover_workspace(&launch)?,
        };
        let sources = UserConfigSources {
            home_directory: host.home_directory.clone(),
            config_directory: host.config_directory.clone(),
            state_directory: host.state_directory.clone(),
            settings: host.config_directory.join("settings.toml"),
            models: host.config_directory.join("models.toml"),
            runtime_root: host
                .state_directory
                .join("workspaces")
                .join(super::configuration::workspace_identity(&cwd)),
        };
        let input = SessionConfigInput {
            cwd,
            config: self.config.as_ref().map(|p| absolute(&launch, p)),
            model: self
                .model
                .as_ref()
                .map(|model| {
                    crate::model::catalog::ModelRef::parse(model)
                        .map(crate::model::session::SessionModelConfig::of)
                })
                .transpose()
                .map_err(|e| e.to_string())?,
            skill_paths: self
                .skill_paths
                .iter()
                .map(|p| absolute(&launch, p))
                .collect(),
            no_automatic_skills: self.no_automatic_skills,
            no_builtin_tools: self.no_builtin_tools,
            no_direct_tools: self.no_direct_tools,
            tools: self.tools.clone(),
            exclude_tools: self.exclude_tools.clone(),
        };
        Ok((sources, input))
    }
}
/// CLI projection of the shared static resolver.
/// # Errors
/// Returns the shared resolver's typed configuration failures.
pub fn analyze(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<ProspectiveSessionConfig, LaunchFailure> {
    let (manager, input) = request.session_input(host)?;
    manager.resolve_session(&input)
}
/// CLI trust/inspection location input, without configuration reads.
/// # Errors
/// Invalid explicit locations are rejected.
pub fn resolve_locations(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<(SessionLocations, String), String> {
    let (mut sources, input) = request.source_inputs(host)?;
    if let Some(root) = &request.runtime_root {
        sources.runtime_root = absolute(&host.launch_directory, root);
    }
    UserConfigManager::new(sources)?.resolve_locations(&input)
}
/// Resolve and admit CLI input through the shared Session configuration owner.
/// # Errors
/// Invalid configuration or absent trust prevents credential capture.
pub fn resolve(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<AdmittedSessionConfig, String> {
    analyze(request, host)?.admit(crate::credentials::CredentialSnapshot::capture)
}
/// User-owned membership directories make grant/revoke atomic and independent per identity.
///
/// # Errors
/// Rejects invalid workspace/store ownership and filesystem failures.
pub fn change_trust(
    request: &LaunchRequest,
    host: &HostEnvironment,
    action: TrustAction,
) -> Result<(), String> {
    #[cfg(test)]
    super::static_effects::observe(super::static_effects::Effect::Trust);
    let (locations, identity) = resolve_locations(request, host)?;
    let sources = UserConfigSources {
        home_directory: host.home_directory.clone(),
        config_directory: host.config_directory.clone(),
        state_directory: host.state_directory.clone(),
        settings: host.config_directory.join("settings.toml"),
        models: host.config_directory.join("models.toml"),
        runtime_root: locations.runtime_root.clone(),
    };
    let root = trust_root(&sources, &locations.workspace)?;
    let record = root.join(identity);
    match action {
        TrustAction::Grant => {
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            match std::fs::create_dir(&record) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && record.is_dir() => {
                    Ok(())
                }
                Err(e) => Err(e.to_string()),
            }
        }
        TrustAction::Revoke => match std::fs::remove_dir(&record) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        },
    }
}

/// Resolve inspection state without loading models or project configuration.
/// Only the host state-location member is relevant; runtime settings stay inert.
///
/// # Errors
/// Rejects invalid filesystem locations or an unreadable/malformed host state reference.
pub fn resolve_inspection_locations(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<SessionLocations, String> {
    let (mut locations, _) = resolve_locations(request, host)?;
    let settings = canonical_settings_source(&host.config_directory.join("settings.toml"))?;
    let authored = if request.runtime_root.is_none() && present_on_disk(&settings)? {
        // Location-only inspection must not validate unrelated model/resource
        // fields. This bounded projection grants no configuration authority.
        #[derive(serde::Deserialize)]
        struct StateLocation {
            runtime_root: Option<PathBuf>,
        }
        let document: StateLocation = crate::toml_authoring::parse(&read_bounded(&settings)?)?;
        document.runtime_root
    } else {
        None
    };
    let explicit = request
        .runtime_root
        .as_ref()
        .map(|p| absolute(&host.launch_directory, p));
    locations.runtime_root = bind_user_source_path(
        &settings,
        authored.as_deref(),
        explicit.as_deref(),
        &locations.runtime_root,
    )?;
    Ok(locations)
}

pub(super) fn discover_workspace(launch: &Path) -> Result<PathBuf, String> {
    for (depth, directory) in launch.ancestors().enumerate() {
        if depth >= 128 {
            return Err("workspace discovery exceeded 128 directories; pass --workspace".into());
        }
        if present_on_disk(&directory.join(".git"))?
            || present_on_disk(&directory.join("rustx.toml"))?
        {
            return Ok(directory.into());
        }
    }
    Ok(launch.into())
}
