//! Bounded canonical named Agent discovery. Project overrides user as a whole resource.
use super::config::AgentProfileDocument;
use crate::runtime::resources::{
    ProjectContextFile, RuntimeResourceLoadError, validate_project_resource_path,
};
use crate::runtime::subagent::{AgentCatalog, NamedAgentDefinition, SubagentName};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct AgentSource {
    pub revision: String,
    pub identity: SubagentName,
    pub selected: PathBuf,
    pub layer: &'static str,
    pub overridden: Option<PathBuf>,
    pub overridden_revision: Option<String>,
}

pub(crate) fn parse(text: &str) -> Result<AgentProfileDocument, String> {
    if text.len() > 1024 * 1024 {
        return Err("Agent resource exceeds 1 MiB".into());
    }
    let document: AgentProfileDocument = crate::toml_authoring::parse(text.as_bytes())?;
    crate::runtime::agent_profile::AgentProfile::from_document(
        &document,
        crate::runtime::agent_profile::AgentProfileKind::Named,
        Vec::new(),
    )?;
    document.execution_deadline()?;
    Ok(document)
}

pub(crate) fn load_profile_files(
    paths: &[PathBuf],
) -> Result<Vec<ProjectContextFile>, RuntimeResourceLoadError> {
    if paths.len() > crate::runtime::subagent::catalog::MAX_SUBAGENT_PROJECT_FILES {
        return Err(RuntimeResourceLoadError::new(
            "Agent project file count exceeds native bound",
        ));
    }
    paths
        .iter()
        .map(|path| {
            let bytes = crate::bounded_file::read_bounded(path).map_err(|error| {
                RuntimeResourceLoadError::new(error).at(path, "agent.agents_md.files")
            })?;
            let content = String::from_utf8(bytes).map_err(|error| {
                RuntimeResourceLoadError::new(error.to_string()).at(path, "agent.agents_md.files")
            })?;
            Ok(ProjectContextFile {
                path: path.clone(),
                content,
            })
        })
        .collect()
}

fn candidates(
    boundary: &Path,
    root: &Path,
    diagnostics: &mut Vec<RuntimeResourceLoadError>,
) -> Result<BTreeMap<SubagentName, PathBuf>, RuntimeResourceLoadError> {
    let mut result = BTreeMap::new();
    for path in super::resource_directory::files(boundary, root, "toml")? {
        let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
            diagnostics.push(
                RuntimeResourceLoadError::new("Agent filename must be UTF-8").at(&path, "agents"),
            );
            continue;
        };
        let Ok(name) = SubagentName::parse(name) else {
            diagnostics.push(
                RuntimeResourceLoadError::new("invalid Agent filename identity")
                    .at(&path, "agents"),
            );
            continue;
        };
        result.insert(name, path);
    }
    Ok(result)
}

#[allow(clippy::too_many_lines)] // One bounded, shadow-before-parse catalog pass.
pub(crate) fn load_authorized(
    workspace: &Path,
    user_root: &Path,
) -> Result<(AgentCatalog, BTreeMap<SubagentName, AgentSource>), RuntimeResourceLoadError> {
    let mut diagnostics = Vec::new();
    let mut users = candidates(user_root, user_root, &mut diagnostics).unwrap_or_else(|error| {
        diagnostics.push(error);
        BTreeMap::new()
    });
    let projects = candidates(
        workspace,
        &workspace.join(".agents/agents"),
        &mut diagnostics,
    )
    .unwrap_or_else(|error| {
        // An unreadable higher collection cannot authorize lower identities
        // whose potential shadowing cannot be determined.
        users.clear();
        diagnostics.push(error);
        BTreeMap::new()
    });
    let mut selected = users.clone();
    selected.extend(projects.clone());
    if selected.len() > crate::runtime::subagent::MAX_SUBAGENT_DEFINITIONS {
        diagnostics.push(
            RuntimeResourceLoadError::new("Agent catalog exceeds native count bound")
                .at(&workspace.join(".agents/agents"), "agents"),
        );
        selected.clear();
    }
    let mut definitions = Vec::new();
    let mut invalid = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for (name, path) in &selected {
        let project_exists = projects.contains_key(name);
        let user_exists = users.contains_key(name);
        let user = user_root.join(format!("{name}.toml"));
        let (boundary, layer) = if project_exists {
            (workspace, "workspace")
        } else {
            (user_root, "user")
        };
        let field = format!("agents.{name}");
        let error = |message: String| RuntimeResourceLoadError::new(message).at(path, &field);
        let mut revision = "unreadable".to_owned();
        let result = (|| -> Result<NamedAgentDefinition, RuntimeResourceLoadError> {
            let bytes = super::resource_directory::read_resource(boundary, path)?;
            revision = super::settings::revision(Some(&bytes));
            let text = std::str::from_utf8(&bytes).map_err(|e| error(e.to_string()))?;
            let agent = parse(text).map_err(|e| {
                error(e)
                    .because("canonical Agent TOML violates its strict typed authoring contract")
            })?;
            if agent.agents_md.files.len()
                > crate::runtime::subagent::catalog::MAX_SUBAGENT_PROJECT_FILES
            {
                return Err(error(
                    "agents_md.files exceeds the native file-count bound".into(),
                ));
            }
            let mut files = Vec::new();
            for file in &agent.agents_md.files {
                let resolved = boundary.join(file);
                validate_project_resource_path(boundary, &resolved)
                    .map_err(|e| e.at(path, format!("{field}.agents_md.files")))?;
                let bytes = crate::bounded_file::read_bounded(&resolved).map_err(error)?;
                let content = String::from_utf8(bytes).map_err(|e| error(e.to_string()))?;
                files.push(ProjectContextFile {
                    path: resolved,
                    content,
                });
            }
            let profile = crate::runtime::agent_profile::AgentProfile::from_document(
                &agent,
                crate::runtime::agent_profile::AgentProfileKind::Named,
                files,
            )
            .map_err(error)?;
            NamedAgentDefinition::new(name.clone(), profile, path.clone())
                .map_err(|e| error(e.to_string()))
        })();
        match result {
            Ok(definition) => definitions.push(definition),
            Err(error) => {
                invalid.insert(name.clone(), error);
            }
        }
        sources.insert(
            name.clone(),
            AgentSource {
                revision,
                identity: name.clone(),
                selected: path.clone(),
                layer,
                overridden: (project_exists && user_exists).then_some(user.clone()),
                overridden_revision: (project_exists && user_exists).then(|| {
                    super::settings::read_document(&user).map_or_else(
                        |_| "unreadable".into(),
                        |bytes| super::settings::revision(bytes.as_deref()),
                    )
                }),
            },
        );
    }
    let mut catalog =
        AgentCatalog::new(definitions).map_err(|e| RuntimeResourceLoadError::new(e.to_string()))?;
    catalog.set_invalid(invalid);
    catalog.discovery_diagnostics = diagnostics;
    Ok((catalog, sources))
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, Weak};
    use tokio::sync::watch;

    pub(crate) struct Gate {
        entered: watch::Sender<bool>,
        release: watch::Sender<bool>,
    }
    static GATES: Mutex<Vec<(PathBuf, Weak<Gate>)>> = Mutex::new(Vec::new());
    pub(crate) fn arm(workspace: &Path) -> Arc<Gate> {
        let gate = Arc::new(Gate {
            entered: watch::channel(false).0,
            release: watch::channel(false).0,
        });
        let mut gates = GATES.lock().unwrap();
        gates.retain(|(path, weak)| path != workspace && weak.strong_count() > 0);
        gates.push((workspace.into(), Arc::downgrade(&gate)));
        gate
    }
    impl Gate {
        pub(crate) async fn entered(&self) {
            self.entered
                .subscribe()
                .wait_for(|entered| *entered)
                .await
                .unwrap();
        }
        pub(crate) fn release(&self) {
            self.release.send_replace(true);
        }
    }
    pub(crate) async fn before_publication(workspace: &Path) {
        let gate = GATES
            .lock()
            .unwrap()
            .iter()
            .find_map(|(path, weak)| (path == workspace).then(|| weak.upgrade()).flatten());
        if let Some(gate) = gate {
            let mut release = gate.release.subscribe();
            gate.entered.send_replace(true);
            release.wait_for(|released| *released).await.unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(root: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join(format!("{name}.toml")), text).unwrap();
    }
    #[test]
    fn canonical_agents_are_sorted_and_project_replaces_user_whole() {
        for reverse in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let workspace = dir.path().canonicalize().unwrap();
            let user = workspace.join("user");
            let project = workspace.join(".agents/agents");
            let names = if reverse {
                ["zeta", "alpha"]
            } else {
                ["alpha", "zeta"]
            };
            for name in names {
                write(
                    &user,
                    name,
                    "description = 'user'\ninstructions = 'user instructions'\nskills = ['user-only']",
                );
            }
            write(
                &project,
                "alpha",
                "description = 'project'\ninstructions = 'project instructions'",
            );
            let (catalog, sources) = load_authorized(&workspace, &user).unwrap();
            assert_eq!(
                catalog
                    .definitions()
                    .map(|agent| agent.name().as_str())
                    .collect::<Vec<_>>(),
                ["alpha", "zeta"]
            );
            let alpha = SubagentName::parse("alpha").unwrap();
            assert!(catalog.get(&alpha).unwrap().skills().is_empty());
            assert_eq!(
                catalog.get(&alpha).unwrap().instructions(),
                "project instructions"
            );
            assert_eq!(sources[&alpha].overridden, Some(user.join("alpha.toml")));
            std::fs::remove_file(project.join("alpha.toml")).unwrap();
            assert_eq!(
                catalog.get(&alpha).unwrap().instructions(),
                "project instructions"
            );
            assert_eq!(
                load_authorized(&workspace, &user)
                    .unwrap()
                    .0
                    .get(&alpha)
                    .unwrap()
                    .instructions(),
                "user instructions"
            );
        }
    }
    #[test]
    fn strict_toml_and_filename_identity_are_the_only_agent_reader() {
        for text in [
            "---\ndescription: old\n---\nbody",
            "description = 'x'\ninstructions = 'x'\nname = 'other'",
            "description = 'x'\ninstructions = 'x'\nunknown = true",
        ] {
            assert!(parse(text).is_err());
        }
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let root = workspace.join(".agents/agents");
        write(&root, "zeta", "broken");
        write(&root, "alpha", "broken");
        let first = load_authorized(&workspace, &workspace.join("user"))
            .unwrap()
            .0
            .invalid()
            .clone();
        assert_eq!(
            first.keys().map(SubagentName::as_str).collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
        assert_eq!(
            first.values().next().unwrap().source_file,
            Some(root.join("alpha.toml"))
        );
        assert_eq!(
            first,
            *load_authorized(&workspace, &workspace.join("user"))
                .unwrap()
                .0
                .invalid()
        );
    }
    #[test]
    fn agent_profile_file_diagnostics_use_current_toml_field_names() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let root = workspace.join(".agents/agents");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("alpha.toml"),
            "description = 'Alpha'\ninstructions = 'Inspect'\n[agents_md]\nfiles = ['../outside.md']\n",
        ).unwrap();
        let catalog = load_authorized(&workspace, &workspace.join("user"))
            .unwrap()
            .0;
        let error = catalog.invalid().values().next().unwrap();
        assert_eq!(
            error.field_path.as_deref(),
            Some("agents.alpha.agents_md.files")
        );
    }

    #[cfg(unix)]
    #[test]
    fn agent_discovery_rejects_redirected_files_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        let root = workspace.join(".agents/agents");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("alpha.toml")).unwrap();
        let catalog = load_authorized(&workspace, &workspace.join("user"))
            .unwrap()
            .0;
        assert!(
            catalog
                .get(&SubagentName::parse("alpha").unwrap())
                .is_none()
        );
        assert_eq!(catalog.invalid().len(), 1);
    }
}
