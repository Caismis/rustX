//! Inert MCP definitions. Identity shadowing precedes per-definition validation.
use super::authoring::McpAuthoring;
use super::configuration::Origin;
use crate::runtime::identity::McpServerId;
use crate::runtime::resources::RuntimeResourceLoadError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) const MAX_MCP_DEFINITIONS: usize = 128;
pub(super) const WORKSPACE_DOCUMENT: &str = ".agents/mcp.toml";

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

/// Read only the supplied owners. User-only discovery never inspects a Workspace.
pub(crate) fn load(user_root: &Path, workspace: Option<&Path>) -> McpCatalog {
    load_with_workspace(user_root, workspace, super::settings::read_document)
}

pub(super) fn load_with_workspace(
    user_root: &Path,
    workspace: Option<&Path>,
    read_workspace: impl Fn(
        &Path,
    )
        -> Result<Option<Vec<u8>>, crate::runtime_client::types::RuntimeClientError>,
) -> McpCatalog {
    let mut catalog = McpCatalog::default();
    for (path, user) in std::iter::once((user_root.join("mcp.toml"), true))
        .chain(workspace.map(|workspace| (workspace.join(WORKSPACE_DOCUMENT), false)))
    {
        let captured = if user {
            super::settings::read_document(&path)
        } else {
            read_workspace(&path)
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
            let definition = definition.map(|mut definition| {
                if let Some(cwd) = &mut definition.cwd {
                    *cwd = super::configuration::capture::absolute(&base, cwd);
                }
                if let Some(command) = &mut definition.command
                    && command.contains('/')
                {
                    *command =
                        super::configuration::capture::absolute(&base, &PathBuf::from(&*command))
                            .to_string_lossy()
                            .into_owned();
                }
                definition
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
            if let Some(cwd) = &mut definition.cwd {
                *cwd = super::configuration::capture::absolute(workspace, cwd);
            }
            if let Some(command) = &mut definition.command
                && command.contains('/')
            {
                *command = super::configuration::capture::absolute(workspace, Path::new(command))
                    .to_string_lossy()
                    .into_owned();
            }
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
