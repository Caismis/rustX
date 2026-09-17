//! Inert MCP definitions. Identity shadowing precedes per-definition validation.
use super::authoring::McpAuthoring;
use super::configuration::Origin;
use crate::runtime::identity::McpServerId;
use crate::runtime::resources::RuntimeResourceLoadError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    if raw.mcp_servers.len() > 128 {
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

pub(crate) fn load(user_root: &Path, workspace: &Path) -> McpCatalog {
    let mut catalog = McpCatalog::default();
    for (path, user) in [
        (user_root.join("mcp.toml"), true),
        (workspace.join(".agents/mcp.toml"), false),
    ] {
        let captured = super::settings::read_document(&path).map_err(|_| {
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
                        location.scope = super::configuration::settings::SourceScope::Workspace;
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
                        super::configuration::settings::SourceScope::User
                    } else {
                        super::configuration::settings::SourceScope::Workspace
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
                    *cwd = super::configuration::absolute(&base, cwd);
                }
                if let Some(command) = &mut definition.command
                    && command.contains('/')
                {
                    *command = super::configuration::absolute(&base, &PathBuf::from(&*command))
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
