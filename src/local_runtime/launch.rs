//! One host-owned launch boundary. No provider, Session, or runtime is composed here.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::composition::StartupSession;
use super::config::CurrentRuntimeConfig;
use crate::model::catalog::ModelCatalog;

/// Filesystem locations and controls produced by the launch resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchLocations {
    /// Repeatable explicit Skill package/root paths from the command line.
    pub skill_paths: Vec<PathBuf>,
    /// Disable automatic/default Skill roots while retaining explicit paths.
    pub no_skills: bool,
    /// Disable optional native/built-in tools from startup activation;
    /// mandatory native Read remains active.
    pub no_builtin_tools: bool,
    /// Disable every optional Tool while retaining available metadata;
    /// mandatory native Read remains active.
    pub no_tools: bool,
    /// The Session this launch binds. Startup never resumes on its own;
    /// `--continue` is the explicit request behind
    /// [`StartupSession::ContinueActive`] and `--session` the one behind
    /// [`StartupSession::Select`].
    pub startup_session: StartupSession,
    /// The display name to give the Session this launch binds, from
    /// `--name`. A Session is otherwise unnamed and `/resume` shows it by
    /// its first message; naming one at startup is the same metadata
    /// operation `/name` performs, moved to the command line.
    pub session_name: Option<String>,
    /// Strict startup Tool allowlist, when supplied.
    pub tools: Option<Vec<String>>,
    /// Final startup Tool exclusions.
    pub exclude_tools: Vec<String>,
    /// The model-visible workspace root.
    pub workspace: PathBuf,
    /// The exact runtime-private root from which disjoint private
    /// subdirectories are derived. Child conversation stores live below its
    /// stable `subagents/<conversation-id>` semantic directory; child
    /// execution incarnations remain private and disposable.
    pub runtime_root: PathBuf,
}

impl LaunchLocations {
    /// The runtime-private artifact root.
    #[must_use]
    pub fn artifacts_root(&self) -> PathBuf {
        self.runtime_root.join("artifacts")
    }

    /// The runtime-private capability environment store root.
    #[must_use]
    pub fn environment_store_root(&self) -> PathBuf {
        self.runtime_root.join("environments")
    }

    /// The capability environment store of one independent conversation
    /// lineage. Branches do not share mutable environment materialization.
    #[must_use]
    pub fn environment_store_root_for(
        &self,
        conversation_id: &crate::runtime::identity::ConversationId,
    ) -> PathBuf {
        self.environment_store_root().join(conversation_id.as_str())
    }
}

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
    pub no_skills: bool,
    pub no_builtin_tools: bool,
    pub no_tools: bool,
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
        })
    }
}

/// Values are never included: provenance cannot expose credentials or environment values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    User { document: PathBuf, base: PathBuf },
    Project { document: PathBuf, base: PathBuf },
    Cli { base: PathBuf },
}

/// Validated launch authority consumed by the existing native composition owner.
#[derive(Debug, Clone)]
pub struct ResolvedLaunch {
    pub(crate) locations: LaunchLocations,
    pub(crate) config: std::sync::Arc<CurrentRuntimeConfig>,
    pub(crate) models: ModelCatalog,
    pub(crate) provenance: BTreeMap<String, Origin>,
    pub(crate) identity: String,
    pub(crate) skill_roots: Vec<PathBuf>,
    /// Fixed document slots for explicit resource reload; never discovery.
    documents: Vec<(PathBuf, bool, Origin)>,
    model_override: Option<String>,
    /// Project-origin paths retain their authority even after becoming absolute.
    project_resources: Vec<PathBuf>,
}

impl ResolvedLaunch {
    /// Recheck physical targets immediately before resource preparation. This
    /// never rereads launch documents and is not an execution filesystem sandbox.
    pub(crate) fn validate_resource_authority(&self) -> Result<(), String> {
        for path in &self.project_resources {
            crate::runtime::resources::validate_project_resource_path(&self.workspace, path)
                .map_err(|e| e.to_string())?;
        }
        self.validate_workspace_resource_roots()
    }

    pub(crate) fn validate_workspace_resource_roots(&self) -> Result<(), String> {
        for relative in [".agents/tools", ".agents/skills"] {
            if relative == ".agents/skills" && self.no_skills {
                continue;
            }
            crate::runtime::resources::validate_project_resource_path(
                &self.workspace,
                &self.workspace.join(relative),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// The immutable validated settings this launch resolved.
    #[must_use]
    pub fn config(&self) -> &CurrentRuntimeConfig {
        &self.config
    }

    /// Safe field origins, without configuration values.
    #[must_use]
    pub const fn provenance(&self) -> &BTreeMap<String, Origin> {
        &self.provenance
    }

    /// Stable identity of the canonical workspace path.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The existing resource-generation owner may reread only the slots this
    /// launch authorized. Catalog, workspace, state and trust remain frozen.
    pub(crate) fn reload_resource_config(
        &self,
    ) -> Result<(CurrentRuntimeConfig, BTreeMap<String, Origin>), String> {
        self.validate_workspace_resource_roots()?;
        let mut merged = Map::new();
        let mut provenance = BTreeMap::new();
        for (path, required, origin) in &self.documents {
            let mut layer = read_layer(path, *required, matches!(origin, Origin::Project { .. }))?;
            layer.remove("models");
            layer.remove("runtimeRoot");
            rebase_paths(&mut layer, origin, &self.workspace)?;
            merge_fields(&mut merged, layer, "", origin, &mut provenance);
        }
        if let Some(model) = &self.model_override {
            merged.insert("model".into(), serde_json::json!({"model":model}));
        }
        if !self.skill_paths.is_empty() {
            merged.insert(
                "skills".into(),
                serde_json::to_value(&self.skill_paths).map_err(|e| e.to_string())?,
            );
        }
        apply_defaults(&mut merged, &mut provenance);
        let config: CurrentRuntimeConfig =
            serde_json::from_value(Value::Object(merged)).map_err(|e| e.to_string())?;
        config.validate().map_err(|e| e.to_string())?;
        Ok((config, provenance))
    }
}

impl std::ops::Deref for ResolvedLaunch {
    type Target = LaunchLocations;
    fn deref(&self) -> &Self::Target {
        &self.locations
    }
}

/// Read-only location resolution is also used by inspection and trust commands.
/// It neither reads model/settings documents nor creates runtime state.
///
/// # Errors
/// Rejects invalid directories or an exhausted discovery bound.
pub fn resolve_locations(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<(LaunchLocations, String), String> {
    let launch = canonical_directory(&host.launch_directory)?;
    let workspace = match &request.workspace {
        Some(path) => canonical_directory(&absolute(&launch, path))?,
        None => discover_workspace(&launch)?,
    };
    let identity = format!(
        "{:x}",
        Sha256::digest(workspace.as_os_str().as_encoded_bytes())
    );
    let runtime_root = absolute(
        &launch,
        &request
            .runtime_root
            .clone()
            .unwrap_or_else(|| host.state_directory.join("workspaces").join(&identity)),
    );
    Ok((
        LaunchLocations {
            workspace,
            runtime_root,
            skill_paths: request
                .skill_paths
                .iter()
                .map(|p| absolute(&launch, p))
                .collect(),
            no_skills: request.no_skills,
            no_builtin_tools: request.no_builtin_tools,
            no_tools: request.no_tools,
            startup_session: request.startup_session.clone(),
            session_name: request.session_name.clone(),
            tools: request.tools.clone(),
            exclude_tools: request.exclude_tools.clone().unwrap_or_default(),
        },
        identity,
    ))
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
    let (locations, identity) = resolve_locations(request, host)?;
    let root = trust_root(host, &locations.workspace)?;
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

fn trust_root(host: &HostEnvironment, workspace: &Path) -> Result<PathBuf, String> {
    let root = normalize_missing(&host.state_directory.join("trust"))?;
    if root.starts_with(workspace) || workspace.starts_with(&root) {
        return Err("host trust store must be outside the project workspace".into());
    }
    Ok(root)
}

fn present_on_disk(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
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
) -> Result<LaunchLocations, String> {
    let (mut locations, _) = resolve_locations(request, host)?;
    let settings = host.config_directory.join("settings.jsonc");
    if request.runtime_root.is_none() && present_on_disk(&settings)? {
        let document: Value = crate::config_format::parse(&read_bounded(&settings)?)?;
        if let Some(value) = document.get("runtimeRoot") {
            let path = value
                .as_str()
                .filter(|p| !p.is_empty())
                .ok_or("user runtimeRoot must be a non-empty path")?;
            locations.runtime_root =
                normalize_missing(&absolute(&host.config_directory, Path::new(path)))?;
        }
    }
    Ok(locations)
}

/// Resolve once, before any native composition or Session publication.
///
/// # Errors
/// Rejects invalid documents, unauthorized fields, absent trust, invalid paths
/// and semantic configuration failures, without publishing runtime state.
#[allow(
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::unnecessary_debug_formatting
)] // derived paths always have parents; debug escapes diagnostic paths
pub fn resolve(request: &LaunchRequest, host: &HostEnvironment) -> Result<ResolvedLaunch, String> {
    let (mut locations, identity) = resolve_locations(request, host)?;
    let launch = canonical_directory(&host.launch_directory)?;
    let user_path = host.config_directory.join("settings.jsonc");
    let project_path = request.config.as_ref().map_or_else(
        || locations.workspace.join("rustx.jsonc"),
        |p| absolute(&launch, p),
    );
    let mut user = read_layer(&user_path, false, false)?;
    let project = read_layer(&project_path, request.config.is_some(), true)?;
    // Even an empty workspace requires trust: files added before composition or reload
    // must never turn a previously inert launch into project activation.
    if !trust_root(host, &locations.workspace)?
        .join(&identity)
        .is_dir()
    {
        return Err(format!(
            "project {} is not trusted; run rustx --workspace {:?} --trust grant (revoke with --trust revoke)",
            locations.workspace.display(),
            locations.workspace
        ));
    }
    let user_models = user.remove("models");
    let user_selected_catalog = user_models.is_some();
    let models_path = request
        .models
        .as_ref()
        .map(|p| absolute(&launch, p))
        .or_else(|| {
            user_models.and_then(|v| {
                v.as_str()
                    .map(|p| absolute(user_path.parent().expect("parent"), Path::new(p)))
            })
        })
        .unwrap_or_else(|| host.config_directory.join("models.jsonc"));
    let state = user.remove("runtimeRoot");
    let state_origin = if state.is_some() {
        Origin::User {
            document: user_path.clone(),
            base: user_path.parent().expect("parent").into(),
        }
    } else {
        Origin::Builtin
    };
    if request.runtime_root.is_none()
        && let Some(value) = state
    {
        locations.runtime_root = absolute(
            user_path.parent().expect("parent"),
            Path::new(value.as_str().expect("validated path")),
        );
    }
    locations.runtime_root = normalize_missing(&locations.runtime_root)?;
    let trust_directory = trust_root(host, &locations.workspace)?;
    if locations.runtime_root.starts_with(&trust_directory)
        || trust_directory.starts_with(&locations.runtime_root)
    {
        return Err("runtimeRoot must be disjoint from host trust authority".into());
    }
    if locations.runtime_root.starts_with(&locations.workspace)
        || locations.workspace.starts_with(&locations.runtime_root)
    {
        return Err("runtimeRoot must be disjoint from the workspace".into());
    }
    let models =
        ModelCatalog::from_jsonc_slice(&read_bounded(&models_path)?).map_err(|e| e.to_string())?;
    let documents = vec![
        (
            user_path.clone(),
            false,
            Origin::User {
                document: user_path.clone(),
                base: user_path.parent().expect("parent").into(),
            },
        ),
        (
            project_path.clone(),
            request.config.is_some(),
            Origin::Project {
                document: project_path.clone(),
                base: project_path.parent().expect("parent").into(),
            },
        ),
    ];
    let mut merged = Map::new();
    let mut provenance = BTreeMap::new();
    let mut project_resources = Vec::new();
    for (mut layer, origin) in [
        (
            user,
            Origin::User {
                document: user_path.clone(),
                base: user_path.parent().expect("parent").into(),
            },
        ),
        (
            project,
            Origin::Project {
                document: project_path.clone(),
                base: project_path.parent().expect("parent").into(),
            },
        ),
    ] {
        project_resources.extend(rebase_paths(&mut layer, &origin, &locations.workspace)?);
        merge_fields(&mut merged, layer, "", &origin, &mut provenance);
    }
    if !request.skill_paths.is_empty() {
        merged.insert(
            "skills".into(),
            serde_json::to_value(&locations.skill_paths).map_err(|e| e.to_string())?,
        );
        provenance.insert(
            "skills".into(),
            Origin::Cli {
                base: launch.clone(),
            },
        );
    }
    if let Some(model) = &request.model {
        let value = serde_json::json!({"model": model});
        merged.insert("model".into(), value);
        provenance.retain(|key, _| !key.starts_with("model."));
        provenance.insert(
            "model.model".into(),
            Origin::Cli {
                base: launch.clone(),
            },
        );
    }
    if !merged.contains_key("model") || merged["model"].get("model").is_none() {
        return Err("no unambiguous default model selected; set model.model to provider/model in user settings.jsonc or pass --model provider/model".into());
    }
    apply_defaults(&mut merged, &mut provenance);
    let config: CurrentRuntimeConfig =
        serde_json::from_value(Value::Object(merged)).map_err(|e| e.to_string())?;
    config.validate().map_err(|e| e.to_string())?;
    config
        .tool_deadline_policy
        .to_policy()
        .map_err(|e| e.clone())?;
    config.tool_environment().map_err(|e| e.to_string())?;
    models
        .model(&config.model.model)
        .map_err(|e| e.to_string())?;
    record_default_origins(
        &serde_json::to_value(&config).map_err(|e| e.to_string())?,
        "",
        &mut provenance,
    );
    for (key, present) in [
        ("skills.cli", !request.skill_paths.is_empty()),
        ("noSkills", request.no_skills),
        ("noTools", request.no_tools),
        ("noBuiltinTools", request.no_builtin_tools),
        ("tools", request.tools.is_some()),
        ("excludeTools", request.exclude_tools.is_some()),
    ] {
        provenance.insert(
            key.into(),
            if present {
                Origin::Cli {
                    base: launch.clone(),
                }
            } else {
                Origin::Builtin
            },
        );
    }
    for (key, explicit, fallback) in [
        ("workspace", request.workspace.is_some(), Origin::Builtin),
        ("runtimeRoot", request.runtime_root.is_some(), state_origin),
        (
            "models",
            request.models.is_some(),
            Origin::User {
                document: if user_selected_catalog {
                    user_path.clone()
                } else {
                    models_path.clone()
                },
                base: if user_selected_catalog {
                    user_path.parent().expect("parent").into()
                } else {
                    models_path.parent().expect("parent").into()
                },
            },
        ),
    ] {
        provenance.insert(
            key.into(),
            if explicit {
                Origin::Cli {
                    base: launch.clone(),
                }
            } else {
                fallback
            },
        );
    }
    let skill_roots = if request.no_skills {
        Vec::new()
    } else {
        vec![
            host.config_directory.join("skills"),
            locations.workspace.join(".agents/skills"),
        ]
    };
    Ok(ResolvedLaunch {
        locations,
        config: std::sync::Arc::new(config),
        models,
        provenance,
        identity,
        skill_roots,
        documents,
        model_override: request.model.clone(),
        project_resources,
    })
}

fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path)
        .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(path)
}

fn normalize_missing(path: &Path) -> Result<PathBuf, String> {
    if present_on_disk(path)? {
        return std::fs::canonicalize(path).map_err(|e| e.to_string());
    }
    let parent = path.parent().ok_or("path has no existing ancestor")?;
    let name = path.file_name().ok_or("invalid path component")?;
    Ok(normalize_missing(parent)?.join(name))
}

fn discover_workspace(launch: &Path) -> Result<PathBuf, String> {
    for (depth, directory) in launch.ancestors().enumerate() {
        if depth >= 128 {
            return Err("workspace discovery exceeded 128 directories; pass --workspace".into());
        }
        if present_on_disk(&directory.join(".git"))?
            || present_on_disk(&directory.join("rustx.jsonc"))?
        {
            return Ok(directory.into());
        }
    }
    Ok(launch.into())
}

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!("{} exceeds 1 MiB", path.display()));
    }
    Ok(bytes)
}

// This finite schema is deliberately not an arbitrary recursive merge framework.
// Records listed here merge their explicit members. All other objects are whole
// declared entries, except the named maps below (an empty map clears the map).
fn record(path: &str) -> bool {
    matches!(
        path,
        "model"
            | "context"
            | "modelTimeoutPolicy"
            | "toolDeadlinePolicy"
            | "agentStatus"
            | "agentStatus.time"
            | "agentStatus.background"
            | "subagents"
            | "workflows"
    )
}
fn named_map(path: &str) -> bool {
    matches!(
        path,
        "mcpServers" | "mcpToolPolicies" | "environment" | "nativeTools" | "subagents.definitions"
    )
}

fn record_default_origins(value: &Value, prefix: &str, origins: &mut BTreeMap<String, Origin>) {
    if (prefix.is_empty() || record(prefix) || named_map(prefix))
        && let Some(entries) = value.as_object()
        && !entries.is_empty()
    {
        for (key, value) in entries {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            record_default_origins(value, &path, origins);
        }
        return;
    }
    origins.entry(prefix.into()).or_insert(Origin::Builtin);
}

fn merge_fields(
    target: &mut Map<String, Value>,
    layer: Map<String, Value>,
    prefix: &str,
    origin: &Origin,
    provenance: &mut BTreeMap<String, Origin>,
) {
    for (key, value) in layer {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if (record(&path) || named_map(&path))
            && let Value::Object(entries) = &value
            && (record(&path) || !entries.is_empty())
        {
            let destination = target
                .entry(key.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(destination) = destination {
                merge_fields(destination, entries.clone(), &path, origin, provenance);
                continue;
            }
        }
        provenance.retain(|old, _| old != &path && !old.starts_with(&format!("{path}.")));
        provenance.insert(path, origin.clone());
        target.insert(key, value);
    }
}

fn apply_defaults(target: &mut Map<String, Value>, provenance: &mut BTreeMap<String, Origin>) {
    let defaults = serde_json::json!({"context": super::config::ContextPolicyDocument::default()});
    for (key, value) in defaults.as_object().expect("object") {
        if !target.contains_key(key) {
            target.insert(key.clone(), value.clone());
            provenance.insert(key.clone(), Origin::Builtin);
        } else if key == "context"
            && let Some(context) = target.get_mut(key).and_then(Value::as_object_mut)
        {
            for (name, value) in value.as_object().expect("context") {
                if !context.contains_key(name) {
                    context.insert(name.clone(), value.clone());
                    provenance.insert(format!("context.{name}"), Origin::Builtin);
                }
            }
        }
    }
}

fn read_layer(path: &Path, required: bool, project: bool) -> Result<Map<String, Value>, String> {
    if !required && !present_on_disk(path)? {
        return Ok(Map::new());
    }
    let bytes = read_bounded(path)?;
    let value: Value =
        crate::config_format::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut object = value
        .as_object()
        .cloned()
        .ok_or("configuration must be an object")?;
    for name in [
        "models",
        "providers",
        "credentials",
        "trust",
        "trusted",
        "trustStore",
        "stateDirectory",
        "runtimeRoot",
        "workspace",
    ] {
        if object.contains_key(name) && (project || !matches!(name, "models" | "runtimeRoot")) {
            return Err(format!(
                "{}: field {name} is forbidden in {} settings (host-owned authority)",
                path.display(),
                if project { "project" } else { "user" }
            ));
        }
    }
    for name in ["models", "runtimeRoot"] {
        if let Some(value) = object.get(name)
            && value.as_str().is_none_or(str::is_empty)
        {
            return Err(format!(
                "{}: {name} must be a non-empty path",
                path.display()
            ));
        }
    }
    // Approval-bearing objects are host-only, including empty objects and
    // non-approval members. Reject before merging: precedence cannot hide this.
    if project {
        for name in ["approvalMode", "nativeTools", "mcpToolPolicies"] {
            if object.contains_key(name) {
                return Err(format!(
                    "{}: field {name} is forbidden in project settings (host-owned Tool approval authority)",
                    path.display()
                ));
            }
        }
    }
    // Validate partial syntax independently, so an invalid lower layer cannot be
    // hidden by an upper one. The typed partial document never inserts defaults.
    let _: PartialRuntime =
        crate::config_format::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(std::mem::take(&mut object))
}

fn rebase_paths(
    layer: &mut Map<String, Value>,
    origin: &Origin,
    workspace: &Path,
) -> Result<Vec<PathBuf>, String> {
    let mut resources = Vec::new();
    let mut path = |value: &mut Value, base: &Path| -> Result<(), String> {
        let raw = value.as_str().ok_or("path must be a string")?;
        if raw.is_empty() {
            return Err("path must be non-empty".into());
        }
        let resolved = absolute(base, Path::new(raw));
        if matches!(origin, Origin::Project { .. }) {
            crate::runtime::resources::validate_project_resource_path(workspace, &resolved)
                .map_err(|e| e.to_string())?;
            resources.push(resolved.clone());
        }
        *value = serde_json::to_value(resolved).map_err(|e| e.to_string())?;
        Ok(())
    };
    let base = match origin {
        Origin::User { base, .. } | Origin::Project { base, .. } | Origin::Cli { base } => base,
        Origin::Builtin => return Ok(resources),
    };
    if let Some(Value::Array(skills)) = layer.get_mut("skills") {
        for skill in skills {
            path(skill, base)?;
        }
    }
    if let Some(Value::Object(servers)) = layer.get_mut("mcpServers") {
        for server in servers.values_mut() {
            if let Some(cwd) = server.get_mut("cwd")
                && !cwd.is_null()
            {
                path(cwd, base)?;
            }
            if let Some(command) = server.get_mut("command")
                && command.as_str().is_some_and(|s| s.contains('/'))
            {
                path(command, base)?;
            }
        }
    }
    if let Some(Value::Object(definitions)) = layer
        .get_mut("subagents")
        .and_then(|s| s.get_mut("definitions"))
    {
        for definition in definitions.values_mut() {
            if let Some(instructions) = definition.get_mut("instructionsFile") {
                path(instructions, base)?;
            }
            if let Some(Value::Array(files)) = definition
                .get_mut("agentsMd")
                .and_then(|a| a.get_mut("files"))
            {
                for file in files {
                    path(file, base)?;
                }
            }
        }
    }
    Ok(resources)
}

// Option + deserialize_with rejects explicit null for nonnullable fields while
// leaving missing fields absent. Whole declared entries retain domain serde schemas.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
macro_rules! partial {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, Deserialize, Serialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct $name { $(#[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")] $field: Option<$ty>),* }
    };
}
partial!(PartialRuntime {
    models: PathBuf, runtime_root: PathBuf,
    schema_version: u32, agent_id: crate::runtime::identity::AgentId,
    model: PartialModel, approval_mode: crate::runtime::ApprovalMode,
    agent_status: crate::context::AgentStatusConfig, context: PartialContext,
    model_timeout_policy: PartialTimeout, tool_deadline_policy: PartialToolDeadline,
    mcp_servers: BTreeMap<crate::runtime::identity::McpServerId, super::config::McpServerDocument>,
    mcp_tool_policies: BTreeMap<crate::runtime::identity::McpServerId, super::config::InvocationPolicyDocument>,
    native_tools: super::config::NativeToolPoliciesDocument, environment: BTreeMap<String, String>,
    default_tools: Vec<String>, skills: Vec<PathBuf>, subagents: PartialSubagents, workflows: PartialWorkflows,
});
partial!(PartialContext { reserve_tokens: u64, keep_recent_tokens: u64, summary_output_cap: Option<u32> });
partial!(PartialTimeout {
    response_start_timeout_ms: u64,
    stream_idle_timeout_ms: u64
});
partial!(PartialToolDeadline { hard_deadline_ms: u64, idle_liveness_ms: Option<u64> });
partial!(PartialModel { model: crate::model::catalog::ModelRef, reasoning_profile: Option<crate::model::catalog::ReasoningProfileId>, request_params: crate::model::invocation::RequestParams, max_output_tokens: Option<u32>, summary_model: crate::model::session::SummaryModelPolicy });
#[derive(Debug, Deserialize, Serialize)]
struct UniqueDefinitions(
    #[serde(deserialize_with = "super::config::deserialize_unique_map")]
    BTreeMap<crate::runtime::subagent::SubagentName, super::config::SubagentDocument>,
);
partial!(PartialSubagents { max_concurrent: usize, definitions: UniqueDefinitions, main: Vec<crate::runtime::subagent::SubagentName>, workflow: Vec<crate::runtime::subagent::SubagentName> });
partial!(PartialWorkflows { definitions: Vec<crate::runtime::workflow::WorkflowId>, main: Vec<crate::runtime::workflow::WorkflowId> });
