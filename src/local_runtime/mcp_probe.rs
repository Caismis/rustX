//! Finite, source-scoped diagnostics. No retained settings connection registry.
use super::probes::{PROBE_TIMEOUT, ProbeState, await_owned};
use super::{
    configuration::{
        UserConfigManager,
        settings::{SettingsError, SourceTarget},
    },
    session_runtime_manager::SessionRuntimeManager,
};
use crate::{
    runtime::{CancellationSignal, identity::McpServerId},
    tools::{
        mcp::{McpInvalidationState, McpServerRuntime, OwnedConnect},
        workspace::Workspace,
    },
};
use std::{collections::BTreeMap, sync::Arc};

// Reject excess work rather than retaining an unbounded native queue.
static PROBES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum McpProbeOutcome {
    Reachable,
    ConnectionFailed,
    ListFailed,
    TimedOut,
    Cancelled,
    SettlementFailed,
}

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct McpProbeResult {
    pub id: McpServerId,
    pub revision: String,
    pub outcome: McpProbeOutcome,
}

struct CancelOnDrop(CancellationSignal);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl SessionRuntimeManager {
    /// Probe exactly the authored source, without changing Session resources.
    /// The native task owns cleanup even if its RPC waiter is dropped.
    pub(crate) async fn probe_mcp(
        &self,
        target: SourceTarget,
        id: McpServerId,
        revision: String,
    ) -> Result<McpProbeResult, SettingsError> {
        probe(self.configuration.clone(), target, id, revision).await
    }
}

async fn probe(
    owner: UserConfigManager,
    target: SourceTarget,
    id: McpServerId,
    revision: String,
) -> Result<McpProbeResult, SettingsError> {
    let permit = PROBES.try_acquire().map_err(|_| SettingsError::Invalid)?;
    let cancellation = CancellationSignal::new();
    let _cancel = CancelOnDrop(cancellation.clone());
    tokio::spawn(async move {
        let _permit = permit;
        let capture_owner = owner.clone();
        let capture_target = target.clone();
        let capture_id = id.clone();
        let capture_revision = revision.clone();
        let (binding, workspace) = tokio::task::spawn_blocking(move || {
            capture(
                &capture_owner,
                &capture_target,
                &capture_id,
                &capture_revision,
            )
        })
        .await
        .map_err(|_| SettingsError::Io)??;
        let outcome = run(&id, &binding, &workspace, cancellation).await;
        // A changed definition cannot receive a successful result for old bytes.
        let check_id = id.clone();
        let check_revision = revision.clone();
        tokio::task::spawn_blocking(move || capture(&owner, &target, &check_id, &check_revision))
            .await
            .map_err(|_| SettingsError::Io)??;
        Ok(McpProbeResult {
            id,
            revision,
            outcome,
        })
    })
    .await
    .map_err(|_| SettingsError::Io)?
}

fn capture(
    owner: &UserConfigManager,
    target: &SourceTarget,
    id: &McpServerId,
    expected: &str,
) -> Result<(crate::tools::mcp::McpServerBinding, Workspace), SettingsError> {
    target.validate()?;
    // Load only this scope. Inherited/same-name winning definitions are not the
    // configuration row the user requested to check.
    let root = owner.resource_root(target);
    let catalog = super::mcp_resources::load(&root, None);
    let actual = catalog
        .revisions
        .get(&root.join("mcp.toml"))
        .ok_or(SettingsError::Invalid)?;
    if actual != expected {
        return Err(SettingsError::Conflict {
            scope: target.scope(),
            expected: expected.into(),
            actual: actual.clone(),
        });
    }
    let definition = catalog
        .definitions
        .get(id)
        .and_then(|value| value.as_ref().ok())
        .ok_or(SettingsError::Invalid)?;
    let mut bindings = super::config::resolve_mcp_bindings(
        &BTreeMap::from([(id.clone(), definition.clone().resolve())]),
        &BTreeMap::new(),
    )
    .map_err(|_| SettingsError::Invalid)?;
    let mut binding = bindings.remove(id).ok_or(SettingsError::Invalid)?;
    binding
        .credentials
        .capture(crate::credentials::CredentialSnapshot::capture());
    let workspace = Workspace::new(target.workspace().unwrap_or(&owner.sources.home_directory))
        .map_err(|_| SettingsError::Invalid)?;
    Ok((binding, workspace))
}

async fn run(
    id: &McpServerId,
    binding: &crate::tools::mcp::McpServerBinding,
    workspace: &Workspace,
    cancellation: CancellationSignal,
) -> McpProbeOutcome {
    use McpProbeOutcome as O;
    let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
    let connect = McpServerRuntime::connect_owned(OwnedConnect::new(
        id,
        binding,
        workspace,
        Arc::new(McpInvalidationState::new()),
        cancellation.clone(),
    ));
    let (state, connected) = Box::pin(await_owned(
        connect,
        &cancellation,
        tokio::time::sleep_until(deadline),
    ))
    .await;
    let mut outcome = match state {
        ProbeState::TimedOut => O::TimedOut,
        ProbeState::Cancelled => O::Cancelled,
        _ => O::ConnectionFailed,
    };
    match connected {
        Ok(runtime) => {
            outcome = inspect_and_close(&runtime, &cancellation, deadline, outcome).await;
        }
        Err(crate::tools::mcp::McpError::PhysicalSettlement(_)) => outcome = O::SettlementFailed,
        Err(_) => {}
    }
    outcome
}

async fn inspect_and_close(
    runtime: &McpServerRuntime,
    cancellation: &CancellationSignal,
    deadline: tokio::time::Instant,
    mut outcome: McpProbeOutcome,
) -> McpProbeOutcome {
    use McpProbeOutcome as O;
    if !cancellation.is_cancelled() {
        outcome = tokio::select! {
            biased;
            () = cancellation.cancelled() => O::Cancelled,
            () = tokio::time::sleep_until(deadline) => O::TimedOut,
            result = runtime.list_tools() => if result.is_ok() { O::Reachable } else { O::ListFailed },
        };
    }
    // Never report success before physical cleanup has settled.
    if runtime.close().await.is_err() {
        outcome = O::SettlementFailed;
    }
    outcome
}

#[cfg(all(test, feature = "mcp-fixture"))]
mod tests {
    use super::*;
    use crate::tools::mcp::fixture::streamable_http::{HttpFixture, HttpFixtureControl};
    use std::time::Duration;

    fn owner(path: &std::path::Path) -> UserConfigManager {
        UserConfigManager::new(super::super::configuration::UserConfigSources {
            home_directory: path.to_path_buf(),
            config_path: path.join("config.toml"),
            runtime_root: path.join("runtime"),
        })
        .unwrap()
    }
    fn write(owner: &UserConfigManager, target: &SourceTarget, text: &str) -> String {
        let root = owner.resource_root(target);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("mcp.toml"), text).unwrap();
        super::super::settings::revision(Some(text.as_bytes()))
    }

    #[tokio::test]
    async fn probe_is_scope_local_revision_bound_and_never_calls_business_tools() {
        let fixture = HttpFixture::start(HttpFixtureControl::default()).await;
        let directory = tempfile::tempdir().unwrap();
        let owner = owner(directory.path());
        let id = McpServerId::new("same");
        let workspace = SourceTarget::Workspace {
            directory: directory.path().canonicalize().unwrap(),
        };
        let user_revision = write(
            &owner,
            &SourceTarget::User,
            &format!("[mcp_servers.same]\nurl = {:?}\n", fixture.endpoint),
        );
        let workspace_revision = write(
            &owner,
            &workspace,
            "[mcp_servers.same]\ncommand = '/nonexistent/rustx-mcp-probe'\n",
        );
        let user = probe(
            owner.clone(),
            SourceTarget::User,
            id.clone(),
            user_revision.clone(),
        )
        .await
        .unwrap();
        assert_eq!(user.outcome, McpProbeOutcome::Reachable);
        let failed = probe(
            owner.clone(),
            workspace.clone(),
            id.clone(),
            workspace_revision.clone(),
        )
        .await
        .unwrap();
        assert_eq!(failed.outcome, McpProbeOutcome::ConnectionFailed);
        // Scope-local diagnostics do not resolve the user's same-name definition
        // when the Workspace entry disappears.
        let empty_revision = write(&owner, &workspace, "");
        assert!(matches!(
            probe(
                owner.clone(),
                workspace.clone(),
                id.clone(),
                workspace_revision
            )
            .await,
            Err(SettingsError::Conflict { .. })
        ));
        assert!(matches!(
            probe(owner.clone(), workspace, id.clone(), empty_revision).await,
            Err(SettingsError::Invalid)
        ));
        assert!(matches!(
            probe(owner, SourceTarget::User, id, "stale".into()).await,
            Err(SettingsError::Conflict { .. })
        ));
        assert_eq!(fixture.control.accepted_calls(), 0);
        fixture.shutdown().await;
    }

    #[tokio::test]
    async fn cancelled_waiter_terminates_the_owned_http_handshake() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let owner = owner(directory.path());
        let revision = write(
            &owner,
            &SourceTarget::User,
            &format!("[mcp_servers.held]\nurl = 'http://{address}/mcp'\n"),
        );
        let task = tokio::spawn(probe(
            owner,
            SourceTarget::User,
            McpServerId::new("held"),
            revision,
        ));
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        assert!(stream.read(&mut bytes).await.unwrap() > 0);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // EOF is transport evidence; dropping the RPC waiter cannot leave the
        // native diagnostic's HTTP handshake alive.
        tokio::time::timeout(Duration::from_secs(5), async {
            while stream.read(&mut bytes).await.unwrap() != 0 {}
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn success_waits_for_close_and_cleanup_failure_is_not_success() {
        use crate::tools::mcp::test_sync::CloseProbe;
        let fixture = HttpFixture::start(HttpFixtureControl::default()).await;
        let directory = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(directory.path()).unwrap();
        for fail in [false, true] {
            let runtime = McpServerRuntime::connect_owned(OwnedConnect::new(
                &McpServerId::new("close"),
                &fixture.binding(),
                &workspace,
                Arc::new(McpInvalidationState::new()),
                CancellationSignal::new(),
            ))
            .await
            .unwrap();
            let close = Arc::new(if fail {
                CloseProbe::failing("injected physical settlement failure")
            } else {
                CloseProbe::parking()
            });
            runtime.install_close_probe(close.clone());
            let task = tokio::spawn(async move {
                inspect_and_close(
                    &runtime,
                    &CancellationSignal::new(),
                    tokio::time::Instant::now() + PROBE_TIMEOUT,
                    McpProbeOutcome::ConnectionFailed,
                )
                .await
            });
            close.wait_entered().await;
            if !fail {
                assert!(!task.is_finished());
                close.release();
            }
            assert_eq!(
                task.await.unwrap(),
                if fail {
                    McpProbeOutcome::SettlementFailed
                } else {
                    McpProbeOutcome::Reachable
                }
            );
        }
        fixture.shutdown().await;
    }

    #[tokio::test]
    async fn native_deadline_cancels_a_held_handshake() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let owner = owner(directory.path());
        let id = McpServerId::new("held");
        let revision = write(
            &owner,
            &SourceTarget::User,
            &format!(
                "[mcp_servers.held]\nurl = 'http://{}/mcp'\n",
                listener.local_addr().unwrap()
            ),
        );
        let (binding, workspace) = capture(&owner, &SourceTarget::User, &id, &revision).unwrap();
        let task =
            tokio::spawn(
                async move { run(&id, &binding, &workspace, CancellationSignal::new()).await },
            );
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        assert!(stream.read(&mut bytes).await.unwrap() > 0);
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(16)).await;
        tokio::time::resume();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap(),
            McpProbeOutcome::TimedOut
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), stream.read(&mut bytes))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
}
