//! Explicit finite probes over the existing source and connection owners.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use super::launch::ProspectiveLaunch;
use crate::capabilities::activation::{SourceActivation, SourceEnablement};
use crate::runtime::CancellationSignal;
use crate::runtime::identity::McpServerId;
use crate::tools::mcp::{McpInvalidationState, McpServerRuntime, OwnedConnect};
use crate::tools::workspace::Workspace;

const PROBE_TIMEOUT_MS: u64 = 15_000;
const PROBE_TIMEOUT: Duration = Duration::from_millis(PROBE_TIMEOUT_MS);

#[derive(Debug, Serialize)]
pub(super) struct ProbePlan {
    pub version: u32,
    pub phase: &'static str,
    pub targets: Vec<ProbeTarget>,
    #[cfg(test)]
    #[serde(skip)]
    pub hooks: ProbeHooks,
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(super) struct ProbeHooks {
    pub credentials: Option<crate::credentials::CredentialSnapshot>,
    pub ownership_pause: Option<Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>>,
    pub close: Option<Arc<crate::tools::mcp::test_sync::CloseProbe>>,
    pub expire: Option<Arc<tokio::sync::Notify>>,
    pub python_store: Option<crate::tools::python::PythonToolStore>,
}

#[derive(Debug, Serialize)]
#[allow(clippy::struct_excessive_bools)] // independent disclosed effect dimensions
pub(super) struct ProbeTarget {
    pub target: String,
    pub kind: &'static str,
    pub activation: SourceActivation,
    pub spawn_process: bool,
    pub network: bool,
    pub prepare_environment: bool,
    pub resolve_credentials: bool,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProbeState {
    Skipped,
    Unavailable,
    Failed,
    TimedOut,
    Cancelled,
    Verified,
    Unresolved,
}

#[derive(Debug, Serialize)]
pub(super) struct ProbeResult {
    pub target: String,
    pub state: ProbeState,
    pub verified: Option<&'static str>,
    pub reason: &'static str,
}

impl ProbePlan {
    pub(super) fn render(&self, json: bool) -> String {
        if json {
            return serde_json::to_string(self).expect("plan serializes");
        }
        let mut output = String::from("Probe plan (before effects):\n");
        for target in &self.targets {
            use std::fmt::Write;
            let _ = writeln!(
                output,
                "{} [{}; {:?}]: process={}, network={}, prepare={}, credentials={}, deadline={}ms",
                target.target,
                target.kind,
                target.activation,
                target.spawn_process,
                target.network,
                target.prepare_environment,
                target.resolve_credentials,
                target.deadline_ms
            );
        }
        output
    }
}

pub(super) fn render_results(results: &[ProbeResult], json: bool) -> String {
    if json {
        return serde_json::json!({"version":1,"phase":"probe_results","results":results})
            .to_string();
    }
    let mut output = String::from("Probe results:\n");
    for result in results {
        use std::fmt::Write;
        let _ = writeln!(
            output,
            "{}: {:?}; {}; verified: {}",
            result.target,
            result.state,
            result.reason,
            result.verified.unwrap_or("nothing")
        );
    }
    output
}

pub(super) fn plan(launch: &ProspectiveLaunch, prepare: bool) -> ProbePlan {
    let mut targets = vec![ProbeTarget {
        target: launch.config.model.model.to_string(),
        kind: "provider",
        activation: SourceActivation::Unconfigured,
        spawn_process: false,
        network: false,
        prepare_environment: false,
        resolve_credentials: false,
        deadline_ms: 0,
    }];
    for (id, source) in &launch.config.mcp_servers {
        let activation = SourceActivation::evaluate(
            source.enabled.map(|enabled| {
                if enabled {
                    SourceEnablement::Enabled
                } else {
                    SourceEnablement::Disabled
                }
            }),
            launch.trusted,
        );
        let admitted = activation.admit().is_ok();
        targets.push(ProbeTarget {
            target: id.to_string(),
            kind: "mcp",
            activation,
            spawn_process: admitted && source.command.is_some(),
            network: admitted,
            prepare_environment: false,
            resolve_credentials: admitted
                && (!source.sensitive_env.is_empty() || !source.sensitive_headers.is_empty()),
            deadline_ms: PROBE_TIMEOUT_MS,
        });
    }
    for (id, intent) in &launch.config.python_sources {
        let activation = SourceActivation::evaluate(Some(*intent), launch.trusted);
        let admitted = activation.admit().is_ok()
            && prepare
            && launch.python_local_status.get(id) == Some(&super::launch::PythonLocalStatus::Valid);
        targets.push(ProbeTarget {
            target: id.to_string(),
            kind: "python",
            activation,
            spawn_process: admitted,
            network: admitted,
            prepare_environment: admitted,
            resolve_credentials: false,
            deadline_ms: PROBE_TIMEOUT_MS,
        });
    }
    ProbePlan {
        version: 1,
        phase: "probe_plan",
        targets,
        #[cfg(test)]
        hooks: ProbeHooks::default(),
    }
}

#[allow(clippy::too_many_lines)] // one finite source plan, with explicit settlement on every branch
pub(super) async fn execute(
    launch: &ProspectiveLaunch,
    plan: &ProbePlan,
    cancellation: CancellationSignal,
) -> Vec<ProbeResult> {
    if launch.trusted && launch.validate_resource_authority().is_err() {
        return unavailable_targets(plan, "workspace resource authority changed before effects");
    }
    let Ok(workspace) = Workspace::new(&launch.workspace) else {
        return unavailable_targets(plan, "workspace is no longer available");
    };
    let credentials = if !cancellation.is_cancelled()
        && plan.targets.iter().any(|target| target.resolve_credentials)
    {
        capture_credentials(plan)
    } else {
        crate::credentials::CredentialSnapshot::default()
    };
    let mut results = Vec::new();
    for target in &plan.targets {
        let mut result = ProbeResult {
            target: target.target.clone(),
            state: ProbeState::Unresolved,
            verified: None,
            reason: "no safe metadata probe exists in this provider adapter; no request was sent",
        };
        if target.kind == "provider" {
            results.push(result);
            continue;
        }
        if target.activation.admit().is_err() {
            result.state = if target.activation == SourceActivation::Untrusted {
                ProbeState::Unavailable
            } else {
                ProbeState::Skipped
            };
            result.reason = "source authority did not admit effects";
            results.push(result);
            continue;
        }
        if cancellation.is_cancelled() {
            result.state = ProbeState::Cancelled;
            result.reason = "probe cancelled before effects";
            results.push(result);
            continue;
        }
        let id = McpServerId::new(&target.target);
        let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
        let binding = if target.kind == "python" {
            if matches!(
                launch.python_local_status.get(&id),
                Some(
                    super::launch::PythonLocalStatus::Missing
                        | super::launch::PythonLocalStatus::Invalid
                )
            ) {
                result.state = ProbeState::Unavailable;
                result.reason = "managed package is missing or locally invalid; repair its local package contract before preparation";
                results.push(result);
                continue;
            }
            if !target.prepare_environment {
                result.state = ProbeState::Unavailable;
                result.reason = "managed environment preparation requires --prepare";
                results.push(result);
                continue;
            }
            // The managed Python owner performs discovery and preparation;
            // doctor neither invokes uv itself nor builds another environment.
            let packages =
                crate::tools::python::discover_admitted_python_packages(&workspace, |candidate| {
                    candidate == &id
                });
            let package = packages
                .ok()
                .and_then(|packages| packages.into_iter().find(|package| package.server_id == id))
                .and_then(|package| package.outcome.ok());
            let Some(package) = package else {
                result.state = ProbeState::Unavailable;
                result.reason = "managed package is missing or invalid";
                results.push(result);
                continue;
            };
            let prepare_store = || {
                let product =
                    crate::runtime::local_storage::ProductRoot::create(&launch.runtime_root)
                        .map_err(|error| error.to_string())?;
                let path = product
                    .confined(&product.root().join("environments/python-tools"))
                    .map_err(|error| error.to_string())?;
                crate::tools::python::PythonToolStore::new(path).map_err(|error| error.to_string())
            };
            #[cfg(test)]
            let store = plan
                .hooks
                .python_store
                .clone()
                .map_or_else(prepare_store, Ok);
            #[cfg(not(test))]
            let store = prepare_store();
            let Ok(store) = store else {
                result.state = ProbeState::Failed;
                result.reason = "managed environment store could not be opened";
                results.push(result);
                continue;
            };
            let owned = cancellation.child();
            let step = store.ensure_prepared(&package, &owned);
            let (state, prepared) =
                await_owned(step, &owned, tokio::time::sleep_until(deadline)).await;
            result.state = state;
            let prepared = if state == ProbeState::Unresolved {
                prepared.ok()
            } else {
                None
            };
            let Some(prepared) = prepared else {
                if result.state == ProbeState::Unresolved {
                    result.state = ProbeState::Failed;
                }
                result.reason = "managed preparation did not complete; owner settled";
                results.push(result);
                continue;
            };
            prepared.server_binding()
        } else {
            let Ok(mut bindings) = super::composition::mcp_bindings_with_authority(
                &launch.config,
                &launch.workspace,
                &launch.provenance,
                &credentials,
            ) else {
                result.state = ProbeState::Failed;
                result.reason = "source binding is invalid";
                results.push(result);
                continue;
            };
            let Some(mut binding) = bindings.remove(&id) else {
                continue;
            };
            binding.activation = target.activation;
            binding
        };
        let owned = cancellation.child();
        let connect = OwnedConnect::new(
            &id,
            &binding,
            &workspace,
            Arc::new(McpInvalidationState::new()),
            owned.clone(),
        );
        #[cfg(test)]
        let connect = connect.with_ownership_pause(plan.hooks.ownership_pause.clone());
        let step = McpServerRuntime::connect_owned(connect);
        let deadline = async {
            #[cfg(test)]
            if let Some(expire) = &plan.hooks.expire {
                expire.notified().await;
                return;
            }
            tokio::time::sleep_until(deadline).await;
        };
        let (state, connected) = Box::pin(await_owned(step, &owned, deadline)).await;
        result.state = state;
        match connected {
            Ok(runtime) => {
                #[cfg(test)]
                if let Some(probe) = &plan.hooks.close {
                    runtime.install_close_probe(probe.clone());
                }
                if runtime.close().await.is_err() {
                    result.state = ProbeState::Failed;
                    result.reason = "connection owner could not prove settlement";
                } else if cancellation.is_cancelled() {
                    result.state = ProbeState::Cancelled;
                    result.reason = "probe cancelled; connection owner settled";
                } else if result.state == ProbeState::Unresolved {
                    result.state = ProbeState::Verified;
                    result.verified = Some("MCP handshake and capability discovery");
                    result.reason = "temporary connection closed; no business Tool was called";
                } else {
                    result.reason = "cancelled or timed out; connection owner settled";
                }
            }
            Err(crate::tools::mcp::McpError::PhysicalSettlement(_)) => {
                result.state = ProbeState::Failed;
                result.reason = "connection owner reported unproven physical settlement";
            }
            Err(_) => {
                if result.state == ProbeState::Unresolved {
                    result.state = ProbeState::Failed;
                }
                result.reason = "connection did not complete; connection owner settled";
            }
        }
        results.push(result);
    }
    results
}

fn unavailable_targets(plan: &ProbePlan, reason: &'static str) -> Vec<ProbeResult> {
    plan.targets
        .iter()
        .map(|target| ProbeResult {
            target: target.target.clone(),
            state: ProbeState::Unavailable,
            verified: None,
            reason,
        })
        .collect()
}

fn capture_credentials(plan: &ProbePlan) -> crate::credentials::CredentialSnapshot {
    let _ = plan;
    #[cfg(test)]
    if let Some(credentials) = &plan.hooks.credentials {
        return credentials.clone();
    }
    crate::credentials::CredentialSnapshot::capture()
}

/// A deadline cancels the existing physical owner and then awaits it. Never
/// detach work by timing out its waiter. Tests control expiration with a channel.
async fn await_owned<T, E>(
    step: impl std::future::Future<Output = Result<T, E>>,
    cancellation: &CancellationSignal,
    deadline: impl std::future::Future<Output = ()>,
) -> (ProbeState, Result<T, E>) {
    tokio::pin!(step);
    tokio::select! {
        biased;
        () = cancellation.cancelled() => (ProbeState::Cancelled, step.await),
        () = deadline => { cancellation.cancel(); (ProbeState::TimedOut, step.await) },
        value = &mut step => (ProbeState::Unresolved, value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cfg235_timeout_and_cancel_await_physical_settlement() {
        for timed_out in [false, true] {
            let cancellation = CancellationSignal::new();
            let (expire, expired) = tokio::sync::oneshot::channel();
            let (settle, settled) = tokio::sync::oneshot::channel();
            let step = async {
                cancellation.cancelled().await;
                settled.await.unwrap();
                Ok::<_, ()>(())
            };
            let execution = await_owned(step, &cancellation, async {
                expired.await.unwrap();
            });
            tokio::pin!(execution);
            if timed_out {
                expire.send(()).unwrap();
            } else {
                cancellation.cancel();
            }
            // Poll once to the settlement gate, proving timeout/cancellation
            // cannot return while the physical owner is still held.
            assert!(futures_util::poll!(&mut execution).is_pending());
            assert!(cancellation.is_cancelled());
            settle.send(()).unwrap();
            let (state, result) = execution.await;
            assert_eq!(
                state,
                if timed_out {
                    ProbeState::TimedOut
                } else {
                    ProbeState::Cancelled
                }
            );
            assert!(result.is_ok());
        }
    }
}
