//! Process-owned MCP settings connections, independent of conversation execution.
use super::{
    UserConfigManager,
    settings::{SettingsError, SourceTarget},
};
use crate::{
    runtime::{CancellationSignal, identity::McpServerId},
    tools::{
        mcp::{McpInvalidationState, McpServerRuntime, OwnedConnect},
        workspace::Workspace,
    },
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum McpConnectionStatus {
    Connecting,
    Connected { tool_count: usize },
    Failed,
    TimedOut,
    Disconnected,
}
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct McpConnectionSnapshot {
    pub id: McpServerId,
    pub revision: String,
    pub state: McpConnectionStatus,
}
#[derive(Debug)]
struct Entry {
    revision: String,
    reusable: std::sync::atomic::AtomicBool,
    cancellation: CancellationSignal,
    state: watch::Sender<McpConnectionStatus>,
    settled: watch::Sender<bool>,
    touched: Mutex<Instant>,
}
#[derive(Debug, Default)]
struct Registry {
    closed: bool,
    entries: BTreeMap<(String, String), Arc<Entry>>,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct McpConnections(Arc<Mutex<Registry>>);
impl McpConnections {
    pub(crate) fn status(&self, target: &SourceTarget) -> Vec<McpConnectionSnapshot> {
        let scope = target.application_scope();
        self.0
            .lock()
            .expect("MCP registry")
            .entries
            .iter()
            .filter(|((owner, _), _)| owner == &scope)
            .map(|((_, id), entry)| {
                *entry.touched.lock().expect("MCP lease") = Instant::now();
                McpConnectionSnapshot {
                    id: McpServerId::new(id),
                    revision: entry.revision.clone(),
                    state: entry.state.borrow().clone(),
                }
            })
            .collect()
    }
    pub(crate) async fn connect(
        &self,
        owner: UserConfigManager,
        target: SourceTarget,
        id: McpServerId,
        revision: String,
        refresh: bool,
    ) -> Result<McpConnectionSnapshot, SettingsError> {
        let capture_owner = owner.clone();
        let capture_target = target.clone();
        let capture_id = id.clone();
        let capture_revision = revision.clone();
        let (binding, workspace) = tokio::task::spawn_blocking(move || {
            capture_owner.capture_mcp_connection(&capture_target, &capture_id, &capture_revision)
        })
        .await
        .map_err(|_| SettingsError::Io)??;
        let key = (target.application_scope(), id.to_string());
        loop {
            let old = {
                let mut registry = self.0.lock().expect("MCP registry");
                if registry.closed {
                    return Err(SettingsError::Invalid);
                }
                if let Some(entry) = registry.entries.get(&key) {
                    if entry.revision == revision
                        && !entry.cancellation.is_cancelled()
                        && (*entry.state.borrow() == McpConnectionStatus::Connecting
                            || (!refresh
                                && matches!(
                                    *entry.state.borrow(),
                                    McpConnectionStatus::Connected { .. }
                                )))
                    {
                        *entry.touched.lock().expect("MCP lease") = Instant::now();
                        return Ok(McpConnectionSnapshot {
                            id,
                            revision,
                            state: entry.state.borrow().clone(),
                        });
                    }
                    if !entry.reusable.load(std::sync::atomic::Ordering::SeqCst) {
                        return Err(SettingsError::Io);
                    }
                    Some(entry.clone())
                } else {
                    if registry.entries.len() >= 128 {
                        let expired = registry
                            .entries
                            .iter()
                            .find(|(_, entry)| {
                                *entry.settled.borrow()
                                    && entry.reusable.load(std::sync::atomic::Ordering::SeqCst)
                            })
                            .map(|(key, _)| key.clone());
                        if let Some(expired) = expired {
                            registry.entries.remove(&expired);
                        }
                    }
                    if registry.entries.len() >= 128 {
                        return Err(SettingsError::Invalid);
                    }
                    let entry = Arc::new(Entry {
                        revision: revision.clone(),
                        reusable: std::sync::atomic::AtomicBool::new(true),
                        cancellation: CancellationSignal::new(),
                        state: watch::channel(McpConnectionStatus::Connecting).0,
                        settled: watch::channel(false).0,
                        touched: Mutex::new(Instant::now()),
                    });
                    registry.entries.insert(key.clone(), entry.clone());
                    let server = id.clone();
                    tokio::spawn(async move {
                        run(entry, server, binding, workspace, owner, target).await;
                    });
                    return Ok(McpConnectionSnapshot {
                        id,
                        revision,
                        state: McpConnectionStatus::Connecting,
                    });
                }
            };
            if let Some(old) = old {
                old.cancellation.cancel();
                let _ = old.settled.subscribe().wait_for(|done| *done).await;
                if !old.reusable.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(SettingsError::Io);
                }
                let mut registry = self.0.lock().expect("MCP registry");
                if registry
                    .entries
                    .get(&key)
                    .is_some_and(|entry| Arc::ptr_eq(entry, &old))
                {
                    registry.entries.remove(&key);
                }
            }
        }
    }
    pub(crate) async fn disconnect(&self, target: &SourceTarget, id: &McpServerId) {
        let key = (target.application_scope(), id.to_string());
        let entry = self
            .0
            .lock()
            .expect("MCP registry")
            .entries
            .get(&key)
            .cloned();
        if let Some(entry) = entry {
            entry.cancellation.cancel();
            let _ = entry.settled.subscribe().wait_for(|done| *done).await;
        }
    }
    pub(crate) fn invalidate(&self, target: &SourceTarget) {
        let scope = target.application_scope();
        for ((owner, _), entry) in &self.0.lock().expect("MCP registry").entries {
            if matches!(target, SourceTarget::User) || owner == &scope {
                entry.cancellation.cancel();
            }
        }
    }
    pub(crate) async fn shutdown(&self) -> Vec<String> {
        let entries = {
            let mut registry = self.0.lock().expect("MCP registry");
            registry.closed = true;
            registry.entries.values().cloned().collect::<Vec<_>>()
        };
        for entry in &entries {
            entry.cancellation.cancel();
        }
        let mut failures = Vec::new();
        for entry in entries {
            let _ = entry.settled.subscribe().wait_for(|done| *done).await;
            if !entry.reusable.load(std::sync::atomic::Ordering::SeqCst) {
                failures.push("MCP settings connection could not prove physical settlement".into());
            }
        }
        failures
    }
}

async fn run(
    entry: Arc<Entry>,
    id: McpServerId,
    binding: crate::tools::mcp::McpServerBinding,
    workspace: Workspace,
    owner: UserConfigManager,
    target: SourceTarget,
) {
    struct Completion(Arc<Entry>);
    impl Drop for Completion {
        fn drop(&mut self) {
            if !*self.0.settled.borrow() {
                self.0
                    .reusable
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                self.0.state.send_replace(McpConnectionStatus::Failed);
                self.0.settled.send_replace(true);
            }
        }
    }
    let _completion = Completion(entry.clone());
    let cancellation = &entry.cancellation;
    if !current(&owner, &target, &id, &entry.revision).await {
        cancellation.cancel();
    }
    let connect = McpServerRuntime::connect_owned(OwnedConnect::new(
        &id,
        &binding,
        &workspace,
        Arc::new(McpInvalidationState::new()),
        cancellation.clone(),
    ));
    tokio::pin!(connect);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let (runtime, timed_out) = tokio::select! {
        result=&mut connect=>(result,false),
        ()=tokio::time::sleep_until(deadline)=>{cancellation.cancel();(connect.await,true)},
    };
    let mut terminal = if timed_out {
        McpConnectionStatus::TimedOut
    } else if cancellation.is_cancelled() {
        McpConnectionStatus::Disconnected
    } else {
        McpConnectionStatus::Failed
    };
    if matches!(
        &runtime,
        Err(crate::tools::mcp::McpError::PhysicalSettlement(_))
    ) {
        entry
            .reusable
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
    if let Ok(runtime) = runtime {
        if !cancellation.is_cancelled() {
            let listed = tokio::select! {
                biased;
                ()=cancellation.cancelled()=>None,
                ()=tokio::time::sleep_until(deadline)=>{terminal=McpConnectionStatus::TimedOut;None},
                result=runtime.list_tools()=>result.ok(),
            };
            if let Some(tools) = listed {
                entry.state.send_replace(McpConnectionStatus::Connected {
                    tool_count: tools.len(),
                });
                loop {
                    tokio::select! {()=cancellation.cancelled()=>break, ()=tokio::time::sleep(Duration::from_secs(1))=>{}}
                    if runtime.unusable_reason().is_some() {
                        terminal = McpConnectionStatus::Failed;
                        break;
                    }
                    if !current(&owner, &target, &id, &entry.revision).await {
                        terminal = McpConnectionStatus::Disconnected;
                        break;
                    }
                    // Native lease ownership: closing a client cannot orphan a server.
                    if entry.touched.lock().expect("MCP lease").elapsed() > Duration::from_secs(60)
                    {
                        terminal = McpConnectionStatus::Disconnected;
                        break;
                    }
                }
            }
        }
        if runtime.close().await.is_err() {
            entry
                .reusable
                .store(false, std::sync::atomic::Ordering::SeqCst);
            terminal = McpConnectionStatus::Failed;
        }
    }
    if cancellation.is_cancelled() && !timed_out {
        terminal = McpConnectionStatus::Disconnected;
    }
    if !entry.reusable.load(std::sync::atomic::Ordering::SeqCst) {
        terminal = McpConnectionStatus::Failed;
    }
    entry.state.send_replace(terminal);
    entry.settled.send_replace(true);
}

async fn current(
    owner: &UserConfigManager,
    target: &SourceTarget,
    id: &McpServerId,
    revision: &str,
) -> bool {
    let owner = owner.clone();
    let target = target.clone();
    let id = id.clone();
    let revision = revision.to_owned();
    tokio::task::spawn_blocking(move || {
        if target.validate().is_err() {
            return false;
        }
        let catalog = super::super::mcp_resources::load(
            &owner.resource_root(&SourceTarget::User),
            target.workspace(),
        );
        catalog
            .locations
            .get(&id)
            .and_then(|location| catalog.revisions.get(&location.path))
            .is_some_and(|actual| actual == &revision)
    })
    .await
    .unwrap_or(false)
}

impl UserConfigManager {
    /// Capture one exact definition; an unrelated model/provider error cannot
    /// prevent checking MCP. A stale document never starts external work.
    pub(crate) fn capture_mcp_connection(
        &self,
        target: &SourceTarget,
        id: &McpServerId,
        expected: &str,
    ) -> Result<(crate::tools::mcp::McpServerBinding, Workspace), SettingsError> {
        target.validate()?;
        let catalog = super::super::mcp_resources::load(
            &self.resource_root(&SourceTarget::User),
            target.workspace(),
        );
        let location = catalog.locations.get(id).ok_or(SettingsError::Invalid)?;
        let actual = catalog
            .revisions
            .get(&location.path)
            .ok_or(SettingsError::Invalid)?;
        if actual != expected {
            return Err(SettingsError::Conflict {
                scope: location.scope,
                expected: expected.into(),
                actual: actual.clone(),
            });
        }
        let definition = catalog
            .definitions
            .get(id)
            .and_then(|definition| definition.as_ref().ok())
            .ok_or(SettingsError::Invalid)?;
        let mut bindings = super::super::config::resolve_mcp_bindings(
            &BTreeMap::from([(id.clone(), definition.clone().resolve())]),
            &BTreeMap::new(),
        )
        .map_err(|_| SettingsError::Invalid)?;
        let mut binding = bindings.remove(id).ok_or(SettingsError::Invalid)?;
        binding
            .credentials
            .capture(crate::credentials::CredentialSnapshot::capture());
        let workspace = Workspace::new(target.workspace().unwrap_or(&self.sources.home_directory))
            .map_err(|_| SettingsError::Invalid)?;
        Ok((binding, workspace))
    }
}

#[cfg(all(test, feature = "mcp-fixture"))]
mod tests {
    use super::*;
    use crate::tools::mcp::fixture::streamable_http::{HttpFixture, HttpFixtureControl};
    fn owner(path: &std::path::Path) -> UserConfigManager {
        UserConfigManager::new(super::super::UserConfigSources {
            home_directory: path.to_path_buf(),
            config_path: path.join("config.toml"),
            runtime_root: path.join("runtime"),
        })
        .unwrap()
    }
    fn write_server(owner: &UserConfigManager, url: &str) -> String {
        let root = owner.resource_root(&SourceTarget::User);
        std::fs::create_dir_all(&root).unwrap();
        let bytes = format!("[mcp_servers.test]\nurl = {url:?}\n");
        std::fs::write(root.join("mcp.toml"), &bytes).unwrap();
        crate::local_runtime::settings::revision(Some(bytes.as_bytes()))
    }
    #[tokio::test]
    async fn native_mcp_connection_lifecycle_is_shared_and_disconnect_settles() {
        let fixture = HttpFixture::start(HttpFixtureControl::default()).await;
        let crate::tools::mcp::McpTransportConfig::StreamableHttp { endpoint, .. } =
            fixture.binding().transport
        else {
            panic!("http");
        };
        let dir = tempfile::tempdir().unwrap();
        let owner = owner(dir.path());
        let revision = write_server(&owner, &endpoint);
        let registry = McpConnections::default();
        let id = McpServerId::new("test");
        let started = registry
            .connect(
                owner.clone(),
                SourceTarget::User,
                id.clone(),
                revision.clone(),
                false,
            )
            .await
            .unwrap();
        assert_eq!(started.state, McpConnectionStatus::Connecting);
        let entry = registry
            .0
            .lock()
            .unwrap()
            .entries
            .values()
            .next()
            .unwrap()
            .clone();
        let mut receiver = entry.state.subscribe();
        let connected = tokio::time::timeout(
            Duration::from_secs(5),
            receiver.wait_for(|state| !matches!(state, McpConnectionStatus::Connecting)),
        )
        .await
        .unwrap()
        .unwrap()
        .clone();
        assert!(matches!(connected,McpConnectionStatus::Connected{tool_count} if tool_count>0));
        assert_eq!(
            registry.clone().status(&SourceTarget::User)[0].state,
            connected
        );
        assert_eq!(
            registry
                .connect(owner, SourceTarget::User, id.clone(), revision, false)
                .await
                .unwrap()
                .state,
            connected
        );
        assert_eq!(registry.0.lock().unwrap().entries.len(), 1);
        assert!(
            registry
                .status(&SourceTarget::Workspace {
                    directory: dir.path().to_path_buf()
                })
                .is_empty()
        );
        registry.disconnect(&SourceTarget::User, &id).await;
        assert!(*entry.settled.borrow());
        assert_eq!(
            registry.status(&SourceTarget::User)[0].state,
            McpConnectionStatus::Disconnected
        );
        registry.shutdown().await;
        fixture.shutdown().await;
    }
    #[tokio::test]
    async fn stale_definition_never_starts_a_connection() {
        let dir = tempfile::tempdir().unwrap();
        let owner = owner(dir.path());
        write_server(&owner, "http://127.0.0.1:1/mcp");
        let registry = McpConnections::default();
        assert!(matches!(
            registry
                .connect(
                    owner,
                    SourceTarget::User,
                    McpServerId::new("test"),
                    "stale".into(),
                    false
                )
                .await,
            Err(SettingsError::Conflict { .. })
        ));
        assert!(registry.status(&SourceTarget::User).is_empty());
    }
    #[tokio::test]
    async fn stalled_handshake_can_be_cancelled_or_times_out_in_native_owner() {
        for timeout in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
            let dir = tempfile::tempdir().unwrap();
            let owner = owner(dir.path());
            let revision = write_server(&owner, &endpoint);
            let registry = McpConnections::default();
            let id = McpServerId::new("test");
            registry
                .connect(owner, SourceTarget::User, id.clone(), revision, false)
                .await
                .unwrap();
            let (_socket, _) = listener.accept().await.unwrap();
            let entry = registry
                .0
                .lock()
                .unwrap()
                .entries
                .values()
                .next()
                .unwrap()
                .clone();
            assert_eq!(*entry.state.borrow(), McpConnectionStatus::Connecting);
            if timeout {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(16)).await;
                let _ = entry.settled.subscribe().wait_for(|done| *done).await;
                assert_eq!(*entry.state.borrow(), McpConnectionStatus::TimedOut);
                tokio::time::resume();
            } else {
                registry.disconnect(&SourceTarget::User, &id).await;
                assert_eq!(*entry.state.borrow(), McpConnectionStatus::Disconnected);
            }
            registry.shutdown().await;
        }
    }
}
