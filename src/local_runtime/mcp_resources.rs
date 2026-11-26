//! Inert MCP definitions. Identity shadowing precedes per-definition validation.
use super::authoring::McpAuthoring;
use super::configuration::Origin;
use crate::runtime::identity::McpServerId;
use crate::runtime::resources::RuntimeResourceLoadError;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub(super) const MAX_MCP_DEFINITIONS: usize = 128;
pub(crate) const WORKSPACE_DOCUMENT: &str = ".agents/mcp.toml";

#[derive(Debug, Clone, Default)]
pub(crate) struct McpCatalog {
    pub locations: BTreeMap<McpServerId, crate::runtime::resources::ResourceLocation>,
    pub invalid_scopes: Vec<RuntimeResourceLoadError>,
    pub revisions: BTreeMap<PathBuf, String>,
    pub definitions: BTreeMap<McpServerId, Result<McpAuthoring, RuntimeResourceLoadError>>,
    pub origins: BTreeMap<McpServerId, Origin>,
}

/// The strict editor schema; discovery validates each named unit independently.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpDocument {
    #[serde(default)]
    pub mcp_servers: BTreeMap<McpServerId, McpAuthoring>,
}

fn parse_entries(
    path: &Path,
    bytes: &[u8],
) -> Result<
    BTreeMap<McpServerId, Result<McpAuthoring, RuntimeResourceLoadError>>,
    RuntimeResourceLoadError,
> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Raw {
        #[serde(default)]
        mcp_servers: BTreeMap<McpServerId, toml::Value>,
    }
    let raw: Raw = crate::toml_authoring::parse(bytes)
        .map_err(|e| RuntimeResourceLoadError::new(e).at(path, "mcp_servers"))?;
    if raw.mcp_servers.len() > MAX_MCP_DEFINITIONS {
        return Err(
            RuntimeResourceLoadError::new("MCP catalog exceeds 128 definitions")
                .at(path, "mcp_servers"),
        );
    }
    Ok(raw
        .mcp_servers
        .into_iter()
        .map(|(id, value)| {
            let parsed: Result<McpAuthoring, _> = value.try_into();
            let result = parsed
                .map_err(|_| {
                    RuntimeResourceLoadError::new("invalid MCP definition")
                        .at(path, format!("mcp_servers.{id}"))
                })
                .and_then(|definition| {
                    super::config::resolve_mcp_entry(&id, &definition.clone().resolve()).map_err(
                        |e| {
                            RuntimeResourceLoadError::new(e.to_string())
                                .at(path, format!("mcp_servers.{id}"))
                        },
                    )?;
                    Ok(definition)
                });
            (id, result)
        })
        .collect())
}

/// A path written relative to a Workspace or Session document can refer to
/// a sibling under the admitted Workspace. It cannot become a host path.
fn resolve_stdio_paths(
    definition: &mut McpAuthoring,
    base: &Path,
    workspace: Option<&Path>,
) -> Result<(), &'static str> {
    let resolve = |path: &Path| -> Result<PathBuf, &'static str> {
        if path
            .to_str()
            .is_some_and(rustx_execution_protocol::mcp::foreign_stdio_path)
        {
            return Err("stdio path is not a Client path");
        }
        if path.is_absolute() {
            // Absolute names are interpreted only by the Client. Reject
            // lexical escapes and platform-specific prefixes rather than
            // silently treating them as usable Client paths.
            if path
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            {
                return Err("stdio path is not a normalized Client path");
            }
            return Ok(path.to_path_buf());
        }
        let Some(workspace) = workspace else {
            return Err("relative stdio path has no authorized Client Workspace");
        };
        let mut resolved = base
            .strip_prefix(workspace)
            .map_err(|_| "stdio document is outside the authorized Client Workspace")?
            .to_path_buf();
        for component in path.components() {
            match component {
                Component::CurDir => (),
                Component::Normal(name) => resolved.push(name),
                Component::ParentDir if resolved.pop() => (),
                _ => return Err("relative stdio path leaves the authorized Client Workspace"),
            }
        }
        Ok(workspace.join(resolved))
    };
    if let Some(cwd) = &mut definition.cwd {
        *cwd = resolve(cwd)?;
    }
    if let Some(command) = &mut definition.command {
        if rustx_execution_protocol::mcp::foreign_stdio_path(command) {
            return Err("stdio command is not a Client path");
        }
        if command.contains('/') {
            *command = resolve(Path::new(command))?
                .into_os_string()
                .into_string()
                .map_err(|_| "stdio command requires a UTF-8 Client path")?;
        }
    }
    let program = definition.command.as_deref().unwrap_or_default();
    let interpreter =
        rustx_execution_protocol::mcp::stdio_interpreter(Path::new(program), &definition.args)
            .to_path_buf();
    let script_index =
        rustx_execution_protocol::mcp::stdio_script_index(Path::new(program), &definition.args);
    let mut script_next = false;
    let mut input_next = false;
    for (index, arg) in definition.args.iter_mut().enumerate() {
        if script_next {
            script_next = false;
            continue;
        }
        if rustx_execution_protocol::mcp::stdio_inline_script(&interpreter, arg) {
            script_next = true;
            continue;
        }
        if rustx_execution_protocol::mcp::stdio_input_option(arg) {
            input_next = true;
            continue;
        }
        let required = std::mem::take(&mut input_next) || script_index == Some(index);
        let Some((prefix, value)) = rustx_execution_protocol::mcp::stdio_path_operand(arg)
            .or_else(|| required.then_some((None, arg.as_str())))
        else {
            continue;
        };
        let prefix = prefix.map(str::to_owned);
        let resolved = resolve(Path::new(value))?
            .into_os_string()
            .into_string()
            .map_err(|_| "stdio argument requires a UTF-8 Client path")?;
        *arg = prefix.map_or(resolved.clone(), |name| format!("{name}={resolved}"));
    }
    Ok(())
}

/// Read only the supplied owners. User-only discovery never inspects a Workspace.
pub(crate) fn load(user_root: &Path, workspace: Option<&Path>) -> McpCatalog {
    load_with_workspace(Some(user_root), workspace, super::settings::read_document)
}

pub(super) fn load_with_workspace(
    user_root: Option<&Path>,
    workspace: Option<&Path>,
    read_workspace: impl FnOnce(
        &Path,
    ) -> Result<
        Option<Vec<u8>>,
        crate::runtime_client::types::RuntimeClientError,
    >,
) -> McpCatalog {
    let mut catalog = McpCatalog::default();
    let mut read_workspace = Some(read_workspace);
    for (path, user) in user_root
        .into_iter()
        .map(|root| (root.join("mcp.toml"), true))
        .chain(workspace.map(|workspace| (workspace.join(WORKSPACE_DOCUMENT), false)))
    {
        let captured = if user {
            super::settings::read_document(&path)
        } else {
            read_workspace.take().expect("one Workspace document")(&path)
        }
        .map_err(|_| {
            RuntimeResourceLoadError::new("cannot read MCP definitions").at(&path, "mcp_servers")
        });
        catalog.revisions.insert(
            path.clone(),
            captured.as_ref().map_or_else(
                |_| "unreadable".into(),
                |bytes| super::settings::revision(bytes.as_deref()),
            ),
        );
        let parsed = captured.and_then(|bytes| match bytes {
            Some(bytes) => parse_entries(&path, &bytes),
            None => Ok(BTreeMap::new()),
        });
        let entries = match parsed {
            Ok(entries) => entries,
            Err(error) => {
                // An unreadable higher collection cannot expose lower credentials
                // through fallback. Its unknown identities are unavailable as a scope.
                if !user {
                    for entry in catalog.definitions.values_mut() {
                        *entry = Err(error.clone());
                    }
                    for origin in catalog.origins.values_mut() {
                        *origin = Origin::Workspace {
                            document: path.clone(),
                            base: path.parent().unwrap().to_path_buf(),
                        };
                    }
                    for location in catalog.locations.values_mut() {
                        location.shadowed = Some(location.path.clone());
                        location.path.clone_from(&path);
                        location.scope = crate::runtime::resources::SourceScope::Workspace;
                    }
                }
                catalog.invalid_scopes.push(error);
                continue;
            }
        };
        for (id, definition) in entries {
            let shadowed = catalog.locations.remove(&id).map(|lower| lower.path);
            if shadowed.is_some() && !user {
                tracing::warn!(server = %id, "Workspace MCP definition overrides User definition");
            }
            catalog.locations.insert(
                id.clone(),
                crate::runtime::resources::ResourceLocation {
                    scope: if user {
                        crate::runtime::resources::SourceScope::User
                    } else {
                        crate::runtime::resources::SourceScope::Workspace
                    },
                    path: path.clone(),
                    shadowed,
                },
            );
            let base = path.parent().expect("resource parent").to_path_buf();
            let origin = if user {
                Origin::User {
                    document: path.clone(),
                    base: base.clone(),
                }
            } else {
                Origin::Workspace {
                    document: path.clone(),
                    base: base.clone(),
                }
            };
            let definition = definition.and_then(|mut definition| {
                let client_workspace = if user { None } else { workspace };
                resolve_stdio_paths(&mut definition, &base, client_workspace).map_err(
                    |detail| {
                        RuntimeResourceLoadError::new(detail).at(&path, format!("mcp_servers.{id}"))
                    },
                )?;
                Ok(definition)
            });
            catalog.origins.insert(id.clone(), origin);
            catalog.definitions.insert(id, definition);
        }
    }
    catalog
}

impl McpCatalog {
    pub(super) fn apply_session(
        &mut self,
        authored: &super::session_inputs::SessionResources,
        workspace: &Path,
    ) -> Result<(), RuntimeResourceLoadError> {
        use crate::runtime::resources::{ResourceLocation, SourceScope};
        let document = PathBuf::from(super::session_inputs::RESOURCES_DOCUMENT);
        for (id, contents) in &authored.mcp {
            let field = format!("mcp_servers.{id}");
            let fail = |detail: String| {
                RuntimeResourceLoadError::new(detail)
                    .at(&document, &field)
                    .because("invalid Session MCP definition")
            };
            let mut definition: McpAuthoring =
                crate::toml_authoring::parse(contents.as_bytes()).map_err(fail)?;
            super::config::resolve_mcp_entry(id, &definition.clone().resolve())
                .map_err(|error| fail(error.to_string()))?;
            resolve_stdio_paths(&mut definition, workspace, Some(workspace))
                .map_err(|detail| fail(detail.to_owned()))?;
            let shadowed = self.locations.remove(id).map(|lower| lower.path);
            self.locations.insert(
                id.clone(),
                ResourceLocation {
                    scope: SourceScope::Session,
                    path: document.clone(),
                    shadowed,
                },
            );
            self.origins.insert(
                id.clone(),
                Origin::Session {
                    document: document.clone(),
                    base: workspace.to_path_buf(),
                },
            );
            self.definitions.insert(id.clone(), Ok(definition));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_stdio_paths_stay_with_their_client_workspace() {
        let workspace = Path::new("/logical/client");
        let user = Path::new("/unrelated/user");
        let catalog = load_with_workspace(None, Some(workspace), |document| {
            assert_eq!(document, workspace.join(".agents/mcp.toml"));
            Ok(Some(
                br#"
[mcp_servers.allowed]
command = "../fixture-server"
cwd = ".."
args = ["../fixture.py", "server.py", "dir/my tool.py", "--config=../settings.toml", "--flag", "https://example.com/a"]
[mcp_servers.unavailable]
command = "../../unexpected-server"
[mcp_servers.system]
command = "/bin/sh"
[mcp_servers.from_path]
command = "fixture-server"
"#
                .to_vec(),
            ))
        });
        let definition = |id: &str| catalog.definitions.get(&McpServerId::new(id)).unwrap();
        let allowed = definition("allowed").as_ref().unwrap();
        assert_eq!(
            allowed.command.as_deref(),
            Some("/logical/client/fixture-server")
        );
        assert_eq!(allowed.cwd.as_deref(), Some(workspace));
        assert_eq!(
            allowed.args,
            [
                "/logical/client/fixture.py",
                "/logical/client/.agents/server.py",
                "/logical/client/.agents/dir/my tool.py",
                "--config=/logical/client/settings.toml",
                "--flag",
                "https://example.com/a"
            ]
        );
        assert!(definition("unavailable").is_err());
        assert_eq!(
            definition("system").as_ref().unwrap().command.as_deref(),
            Some("/bin/sh")
        );
        assert_eq!(
            definition("from_path").as_ref().unwrap().command.as_deref(),
            Some("fixture-server")
        );
        let mut local = allowed.clone();
        local.command = Some("./tool".into());
        assert!(resolve_stdio_paths(&mut local, user, Some(workspace)).is_err());
        assert!(resolve_stdio_paths(&mut local, user, None).is_err());

        let mut script = allowed.clone();
        script.command = Some("/bin/sh".into());
        script.args = vec!["-c".into(), "printf 'x/y'".into()];
        resolve_stdio_paths(&mut script, user, None).unwrap();
        assert_eq!(script.args, ["-c", "printf 'x/y'"]);
        for command in ["/bin/sh", "/usr/bin/env"] {
            for flag in ["-lc", "-ic"] {
                let mut script = allowed.clone();
                script.command = Some(command.into());
                script.cwd = None;
                script.args = if command.ends_with("env") {
                    vec!["bash".into(), flag.into(), "exec /some/server.py".into()]
                } else {
                    vec![flag.into(), "exec /some/server.py".into()]
                };
                let expected = script.args.clone();
                resolve_stdio_paths(&mut script, user, None).unwrap();
                assert_eq!(script.args, expected);
            }
        }

        let mut escaped = allowed.clone();
        escaped.args = vec!["--config=../../outside.toml".into()];
        assert!(
            resolve_stdio_paths(
                &mut escaped,
                workspace.join(".agents").as_path(),
                Some(workspace)
            )
            .is_err()
        );
        let mut host_relative = allowed.clone();
        host_relative.command = Some("python3".into());
        host_relative.cwd = None;
        host_relative.args = vec!["server.py".into()];
        assert!(resolve_stdio_paths(&mut host_relative, user, None).is_err());
        host_relative.command = Some(r"C:\host\server.exe".into());
        host_relative.args.clear();
        assert!(resolve_stdio_paths(&mut host_relative, user, Some(workspace)).is_err());
    }

    #[test]
    fn user_discovery_does_not_read_workspace_overrides() {
        let dir = tempfile::tempdir().unwrap();
        // The workspace argument is canonical authority; model the production
        // binding point instead of passing an aliased temporary root.
        let root = dir.path().canonicalize().unwrap();
        let user = root.join("user");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(root.join(".agents")).unwrap();
        std::fs::write(
            user.join("mcp.toml"),
            "[mcp_servers.docs]\nurl = 'https://user.example/mcp'",
        )
        .unwrap();
        std::fs::write(root.join(".agents/mcp.toml"), "invalid = [").unwrap();
        let (catalog, effects) = super::super::static_effects::measure(|| load(&user, None));
        let id = McpServerId::new("docs");
        assert!(catalog.definitions[&id].is_ok());
        assert_eq!(
            catalog.invalid_scopes,
            [] as [crate::runtime::resources::RuntimeResourceLoadError; 0]
        );
        assert_eq!(catalog.revisions.len(), 1);
        assert!(matches!(catalog.origins[&id], Origin::User { .. }));
        assert_eq!(effects, [0; 11]);
        assert!(load(&user, Some(&root)).definitions[&id].is_err());
    }
}
