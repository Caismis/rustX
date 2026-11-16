//! The source Tools a Workflow of one scope can choose, as Dify lists the
//! installed tools of each provider. A Global Workflow sees the User MCP
//! servers and Managed Python packages; a Workspace Workflow also sees the
//! Workspace's own. Each page connects, lists and closes its source, with
//! a digest fence on continuation and the same owners and bounds as the settings probe. Choices, never
//! admission: the native check decides.
use super::configuration::{
    UserConfigManager,
    settings::{SettingsError, SourceTarget},
};
use super::probes::{PROBE_TIMEOUT, ProbeState, await_owned};
use super::session_runtime_manager::SessionRuntimeManager;
use crate::runtime::CancellationSignal;
use crate::runtime::identity::McpServerId;
use crate::runtime::resources::SourceScope;
use crate::runtime::workflow::{WorkflowToolChoice, WorkflowToolResult};
use crate::tools::mcp::{McpInvalidationState, McpServerBinding, McpServerRuntime, OwnedConnect};
use crate::tools::workspace::Workspace;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Sources connected at once, across requests.
static LISTINGS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[cfg(test)]
pub(crate) fn listings_for_test() -> &'static tokio::sync::Semaphore {
    &LISTINGS
}
/// Preparing a Python environment may install its requirements first.
const PREPARE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowToolSourceKind {
    Mcp,
    Python,
}

/// Why a source lists no Tools, or that it listed them.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowToolSourceState {
    /// Not listed yet: list it by its `source_id`.
    Pending,
    Listed,
    Invalid,
    PreparationFailed,
    ConnectionFailed,
    ListFailed,
    TimedOut,
    Cancelled,
}

/// One MCP server or Managed Python package and the Tools it lists.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowToolSource {
    /// The selector's `source_id`: an MCP server ID or `python:<package>`.
    pub source_id: String,
    pub kind: WorkflowToolSourceKind,
    pub scope: SourceScope,
    pub state: WorkflowToolSourceState,
    pub tools: Vec<WorkflowToolChoice>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowToolCatalog {
    pub sources: Vec<WorkflowToolSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<WorkflowToolCatalogCursor>,
}

/// A continuation is accepted only against the same complete catalog.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowToolCatalogCursor {
    pub offset: u32,
    pub digest: String,
}

const MAX_CATALOG_PAGE_BYTES: usize = 256 * 1024;

fn page(
    mut catalog: WorkflowToolCatalog,
    cursor: Option<&WorkflowToolCatalogCursor>,
    source: bool,
) -> Result<WorkflowToolCatalog, SettingsError> {
    use sha2::{Digest, Sha256};
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&catalog).map_err(|_| SettingsError::Io)?)
    );
    let offset = cursor.map_or(0, |cursor| cursor.offset as usize);
    if cursor.is_some_and(|cursor| cursor.digest != digest) {
        return Err(SettingsError::Invalid);
    }
    let sources = if source {
        Vec::new()
    } else {
        std::mem::take(&mut catalog.sources)
    };
    let tools = if source {
        std::mem::take(&mut catalog.sources[0].tools)
    } else {
        Vec::new()
    };
    let total = if source { tools.len() } else { sources.len() };
    if offset > total {
        return Err(SettingsError::Invalid);
    }
    // Include the complete envelope and reserve the bounded continuation.
    let mut bytes = serde_json::to_vec(&catalog)
        .map_err(|_| SettingsError::Io)?
        .len()
        + 256;
    let mut consumed = offset;
    for index in offset..total {
        let size = if source {
            serde_json::to_vec(&tools[index])
        } else {
            serde_json::to_vec(&sources[index])
        }
        .map_err(|_| SettingsError::Io)?
        .len()
            + 1;
        if bytes + size > MAX_CATALOG_PAGE_BYTES {
            break;
        }
        bytes += size;
        consumed += 1;
    }
    if consumed == offset && offset < total {
        return Err(SettingsError::Invalid);
    }
    if source {
        catalog.sources[0].tools = tools
            .into_iter()
            .skip(offset)
            .take(consumed - offset)
            .collect();
    } else {
        catalog.sources = sources
            .into_iter()
            .skip(offset)
            .take(consumed - offset)
            .collect();
    }
    if consumed < total {
        catalog.next = Some(WorkflowToolCatalogCursor {
            offset: u32::try_from(consumed).map_err(|_| SettingsError::Invalid)?,
            digest,
        });
    }
    Ok(catalog)
}

enum Connect {
    Mcp(Box<McpServerBinding>),
    Python(Box<crate::tools::python::PythonToolPackage>),
    Invalid,
}

struct Plan {
    source_id: String,
    kind: WorkflowToolSourceKind,
    scope: SourceScope,
    connect: Connect,
}

impl SessionRuntimeManager {
    /// The sources a Workflow of `target` can choose Tools from, each still
    /// `pending` and connecting to nothing; or, for one `source`, that source
    /// listed. A slow source (a package preparing its environment) then
    /// delays only itself.
    /// # Errors
    /// Rejects a redirected target or a source outside it. If the caller
    /// observed a specific source owner, its scope must still match the
    /// captured binding before anything connects.
    pub(crate) async fn workflow_tool_catalog(
        &self,
        target: SourceTarget,
        source: Option<String>,
        cursor: Option<WorkflowToolCatalogCursor>,
        expected_scope: Option<SourceScope>,
    ) -> Result<WorkflowToolCatalog, SettingsError> {
        if expected_scope.is_some() && source.is_none() {
            return Err(SettingsError::Invalid);
        }
        let owner = self.configuration.clone();
        let (plans, workspace, runtime_root) =
            tokio::task::spawn_blocking(move || capture(&owner, &target))
                .await
                .map_err(|_| SettingsError::Io)??;
        let Some(source) = source else {
            return page(
                WorkflowToolCatalog {
                    next: None,
                    sources: plans
                        .into_iter()
                        .map(|plan| WorkflowToolSource {
                            state: if matches!(plan.connect, Connect::Invalid) {
                                WorkflowToolSourceState::Invalid
                            } else {
                                WorkflowToolSourceState::Pending
                            },
                            source_id: plan.source_id,
                            kind: plan.kind,
                            scope: plan.scope,
                            tools: Vec::new(),
                        })
                        .collect(),
                },
                cursor.as_ref(),
                false,
            );
        };
        let plan = plans
            .into_iter()
            .find(|plan| plan.source_id == source)
            .filter(|plan| expected_scope.is_none_or(|scope| plan.scope == scope))
            .ok_or(SettingsError::Invalid)?;
        let _permit = LISTINGS.acquire().await.map_err(|_| SettingsError::Io)?;
        let mut listed = list(
            plan,
            &workspace,
            &runtime_root,
            &self.configuration.resource_io,
        )
        .await;
        listed
            .tools
            .sort_by(|left, right| left.name.cmp(&right.name));
        page(
            WorkflowToolCatalog {
                sources: vec![listed],
                next: None,
            },
            cursor.as_ref(),
            true,
        )
    }
}

/// The scope's winning definitions, in identity order: MCP servers, then
/// Managed Python packages.
fn capture(
    owner: &UserConfigManager,
    target: &SourceTarget,
) -> Result<(Vec<Plan>, Workspace, PathBuf), SettingsError> {
    target.validate()?;
    let user_root = owner.resource_root(&SourceTarget::User);
    let directory = target.workspace();
    let mcp = super::mcp_resources::load(&user_root, directory);
    let credentials = crate::credentials::CredentialSnapshot::capture();
    let scope_of = |location: Option<&crate::runtime::resources::ResourceLocation>| {
        location.map_or(SourceScope::User, |location| location.scope)
    };
    let mut plans = Vec::new();
    for (id, definition) in &mcp.definitions {
        let binding = definition.as_ref().ok().and_then(|definition| {
            super::config::resolve_mcp_bindings(&BTreeMap::from([(
                id.clone(),
                definition.clone().resolve(),
            )]))
            .ok()?
            .remove(id)
        });
        plans.push(Plan {
            source_id: id.to_string(),
            kind: WorkflowToolSourceKind::Mcp,
            scope: scope_of(mcp.locations.get(id)),
            connect: binding.map_or(Connect::Invalid, |mut binding| {
                binding.credentials.capture(credentials.clone());
                Connect::Mcp(Box::new(binding))
            }),
        });
    }
    if let Ok(python) = super::managed_python_resources::discover_controlled(
        directory,
        &user_root,
        &owner.resource_io,
    ) {
        for (source, package) in python.packages() {
            plans.push(Plan {
                source_id: source.to_string(),
                kind: WorkflowToolSourceKind::Python,
                scope: scope_of(python.locations.get(source)),
                connect: package.as_ref().map_or(Connect::Invalid, |package| {
                    Connect::Python(Box::new(package.clone()))
                }),
            });
        }
    }
    let workspace = Workspace::new(directory.unwrap_or(&owner.sources.home_directory))
        .map_err(|_| SettingsError::Invalid)?;
    Ok((plans, workspace, owner.sources.runtime_root.clone()))
}

async fn list(
    plan: Plan,
    workspace: &Workspace,
    runtime_root: &std::path::Path,
    cancellation: &CancellationSignal,
) -> WorkflowToolSource {
    let state = |state| WorkflowToolSource {
        source_id: plan.source_id.clone(),
        kind: plan.kind,
        scope: plan.scope,
        state,
        tools: Vec::new(),
    };
    let binding = match plan.connect {
        Connect::Invalid => return state(WorkflowToolSourceState::Invalid),
        Connect::Mcp(binding) => *binding,
        Connect::Python(ref package) => {
            let Ok(store) = python_store(runtime_root) else {
                return state(WorkflowToolSourceState::PreparationFailed);
            };
            let owned = cancellation.child();
            let deadline = tokio::time::sleep(PREPARE_TIMEOUT);
            match await_owned(store.ensure_prepared(package, &owned), &owned, deadline).await {
                (ProbeState::Unresolved, Ok(prepared)) => prepared.server_binding(),
                (ProbeState::TimedOut, _) => return state(WorkflowToolSourceState::TimedOut),
                (ProbeState::Cancelled, _) => return state(WorkflowToolSourceState::Cancelled),
                _ => return state(WorkflowToolSourceState::PreparationFailed),
            }
        }
    };
    // A package's synthesized server identity is its `python:<package>` ID.
    let id = McpServerId::new(&plan.source_id);
    let owned = cancellation.child();
    let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
    let connect = McpServerRuntime::connect_owned(OwnedConnect::new(
        &id,
        &binding,
        workspace,
        Arc::new(McpInvalidationState::new()),
        owned.clone(),
    ));
    let (connected_state, connected) = Box::pin(await_owned(
        connect,
        &owned,
        tokio::time::sleep_until(deadline),
    ))
    .await;
    let Ok(runtime) = connected else {
        return state(match connected_state {
            ProbeState::TimedOut => WorkflowToolSourceState::TimedOut,
            ProbeState::Cancelled => WorkflowToolSourceState::Cancelled,
            _ => WorkflowToolSourceState::ConnectionFailed,
        });
    };
    let listed = tokio::select! {
        biased;
        () = owned.cancelled() => Err(WorkflowToolSourceState::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(WorkflowToolSourceState::TimedOut),
        tools = runtime.list_tools() => tools.map_err(|_| WorkflowToolSourceState::ListFailed),
    };
    // A source the runtime cannot close is not reported as listed.
    let closed = runtime.close().await.is_ok();
    match listed {
        Ok(tools) if closed => WorkflowToolSource {
            tools: tools
                .into_iter()
                .map(|tool| WorkflowToolChoice {
                    result: WorkflowToolResult::declared(tool.output_schema.as_ref()),
                    name: tool.name,
                    description: tool.description,
                    input_schema: tool.input_schema,
                })
                .collect(),
            ..state(WorkflowToolSourceState::Listed)
        },
        Ok(_) => state(WorkflowToolSourceState::ConnectionFailed),
        Err(failed) => state(failed),
    }
}

/// The same environment store live Sessions prepare packages in.
fn python_store(
    runtime_root: &std::path::Path,
) -> Result<crate::tools::python::PythonToolStore, String> {
    let product = crate::runtime::local_storage::ProductRoot::create(runtime_root)
        .map_err(|error| error.to_string())?;
    let path = product
        .confined(&product.root().join("environments/python-tools"))
        .map_err(|error| error.to_string())?;
    crate::tools::python::PythonToolStore::new(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn large() -> WorkflowToolCatalog {
        let properties = (0..64)
            .map(|index| {
                (
                    format!("field_{index}"),
                    json!({"type":"string","description":"A field description. ".repeat(6)}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        WorkflowToolCatalog {
            next: None,
            sources: vec![WorkflowToolSource {
                source_id: "large".into(),
                kind: WorkflowToolSourceKind::Mcp,
                scope: SourceScope::User,
                state: WorkflowToolSourceState::Listed,
                tools: (0..128)
                    .map(|index| WorkflowToolChoice {
                        name: format!("tool_{index:03}"),
                        description: "Quoted \"description\" and Unicode 界\n".repeat(8),
                        input_schema: json!({"type":"object","properties":properties}),
                        result: WorkflowToolResult::Text {},
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn legal_large_catalog_is_complete_in_encoded_byte_bounded_pages() {
        let catalog = large();
        assert!(serde_json::to_vec(&catalog).unwrap().len() > 1024 * 1024);
        let mut cursor = None;
        let mut all = Vec::new();
        let mut pages = 0;
        loop {
            let listed = page(catalog.clone(), cursor.as_ref(), true).unwrap();
            assert!(serde_json::to_vec(&listed).unwrap().len() <= MAX_CATALOG_PAGE_BYTES);
            all.extend(listed.sources[0].tools.clone());
            pages += 1;
            cursor = listed.next;
            if cursor.is_none() {
                break;
            }
        }
        assert!(pages > 1);
        assert_eq!(all, catalog.sources[0].tools);
    }

    #[test]
    fn changed_catalog_and_out_of_range_continuations_are_refused() {
        let catalog = large();
        let cursor = page(catalog.clone(), None, true).unwrap().next.unwrap();
        let mut changed = catalog.clone();
        changed.sources[0].tools[0].description.push('!');
        assert!(page(changed, Some(&cursor), true).is_err());
        assert!(
            page(
                catalog,
                Some(&WorkflowToolCatalogCursor {
                    offset: u32::MAX,
                    ..cursor
                }),
                true
            )
            .is_err()
        );
    }
}
