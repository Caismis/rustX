//! The source Tools a Workflow of one scope can choose, as Dify lists the
//! installed tools of each provider. A Global Workflow sees the User MCP
//! servers and Managed Python packages; a Workspace Workflow also sees the
//! Workspace's own. Each source is connected once, listed and closed, with
//! the same owners and bounds as the settings probe. Choices, never
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
    /// Rejects a redirected target or a source outside it.
    pub(crate) async fn workflow_tool_catalog(
        &self,
        target: SourceTarget,
        source: Option<String>,
    ) -> Result<WorkflowToolCatalog, SettingsError> {
        let owner = self.configuration.clone();
        let (plans, workspace, runtime_root) =
            tokio::task::spawn_blocking(move || capture(&owner, &target))
                .await
                .map_err(|_| SettingsError::Io)??;
        let Some(source) = source else {
            return Ok(WorkflowToolCatalog {
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
            });
        };
        let plan = plans
            .into_iter()
            .find(|plan| plan.source_id == source)
            .ok_or(SettingsError::Invalid)?;
        let _permit = LISTINGS.acquire().await.map_err(|_| SettingsError::Io)?;
        let listed = list(plan, &workspace, &runtime_root, &CancellationSignal::new()).await;
        Ok(WorkflowToolCatalog {
            sources: vec![listed],
        })
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
            super::config::resolve_mcp_bindings(
                &BTreeMap::from([(id.clone(), definition.clone().resolve())]),
                &BTreeMap::new(),
            )
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
    if let Ok(python) = super::managed_python_resources::discover(directory, &user_root) {
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
