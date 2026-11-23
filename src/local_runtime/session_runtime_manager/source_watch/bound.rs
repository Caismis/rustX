//! The authenticated Client owns native Workspace watches. Hints trigger a
//! fresh capture on the exact binding that delivered them.
use super::super::{LocalExecutionEndpoint, SessionRuntimeManager};
use crate::runtime::identity::SessionId;
use rustx_execution_protocol::watch::{MAX_CONFIGURATION_WATCHES, WatchId};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::JoinSet;
use tokio::time::{Duration, Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

const POLL: Duration = Duration::from_millis(250);
const COVERAGE_RECHECK: Duration = Duration::from_secs(5);

type WatchPlan = BTreeMap<PathBuf, Option<BTreeSet<OsString>>>;

struct Observer {
    endpoint: Arc<LocalExecutionEndpoint>,
    stop: CancellationToken,
}

impl SessionRuntimeManager {
    pub(super) fn bound_source_bindings(&self) -> Vec<(SessionId, Arc<LocalExecutionEndpoint>)> {
        let sessions = self
            .sessions
            .configuration_bindings
            .lock()
            .expect("Session configuration bindings")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        sessions
            .into_iter()
            .filter_map(|session| {
                let endpoint = self
                    .execution
                    .configuration_endpoint(&session)
                    .ok()
                    .flatten()?;
                Some((session, endpoint))
            })
            .collect()
    }

    pub(super) async fn run_bound_source_watch(&self, stop: CancellationToken) {
        let mut owners = BTreeMap::<SessionId, Observer>::new();
        let mut workers = JoinSet::new();
        let mut ticks = tokio::time::interval(POLL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => break,
                done = workers.join_next(), if !workers.is_empty() => {
                    if let Some(Ok((session, endpoint))) = done
                        && owners.get(&session).is_some_and(|owner| Arc::ptr_eq(&owner.endpoint, &endpoint))
                    {
                        owners.remove(&session);
                    }
                },
                _ = ticks.tick() => {
                    let bindings: BTreeMap<_, _> = self.bound_source_bindings()
                        .into_iter().filter(|(_, endpoint)| endpoint.control().is_some()).collect();
                    owners.retain(|session, observer| {
                        let retained = bindings.get(session)
                            .is_some_and(|endpoint| Arc::ptr_eq(endpoint, &observer.endpoint));
                        if !retained {
                            observer.stop.cancel();
                        }
                        retained
                    });
                    for (session, endpoint) in bindings {
                        owners.entry(session.clone()).or_insert_with(|| {
                            let token = stop.child_token();
                            let manager = self.clone();
                            let worker_endpoint = endpoint.clone();
                            let worker_stop = token.clone();
                            workers.spawn(async move {
                                watch_session(manager, session.clone(), worker_endpoint.clone(), worker_stop).await;
                                (session, worker_endpoint)
                            });
                            Observer { endpoint, stop: token }
                        });
                    }
                }
            }
        }
        for observer in owners.into_values() {
            observer.stop.cancel();
        }
        while workers.join_next().await.is_some() {}
    }

    fn workspace_watch_plan(&self, session: &SessionId, workspace: &Path) -> WatchPlan {
        let bindings = self
            .sessions
            .configuration_bindings
            .lock()
            .expect("Session configuration bindings");
        let mut plan = bindings.get(session).map_or_else(WatchPlan::new, |config| {
            config.skill_discovery.observed_workspace.clone()
        });
        named(
            &mut plan,
            workspace.to_path_buf(),
            ["rustx.toml", "AGENTS.md", ".agents"].map(Into::into),
        );
        let agents = workspace.join(".agents");
        named(
            &mut plan,
            agents.clone(),
            ["mcp.toml", "agents", "workflows", "skills", "tools"].map(Into::into),
        );
        for name in ["agents", "workflows", "tools", "skills"] {
            plan.entry(agents.join(name)).or_insert(None);
        }
        plan
    }
}

fn named(plan: &mut WatchPlan, parent: PathBuf, names: impl IntoIterator<Item = OsString>) {
    if let Some(filter) = plan.entry(parent).or_insert_with(|| Some(BTreeSet::new())) {
        filter.extend(names);
    }
}

#[derive(Default)]
struct Watches {
    registered: BTreeMap<PathBuf, (PathBuf, WatchId)>,
    plan: WatchPlan,
    incomplete: bool,
}

impl Watches {
    fn register(&mut self, endpoint: &LocalExecutionEndpoint, plan: WatchPlan) -> io::Result<bool> {
        let changed = plan != self.plan;
        self.plan = plan;
        self.incomplete = self.plan.len() > MAX_CONFIGURATION_WATCHES;
        let expired = self
            .registered
            .keys()
            .filter(|path| !self.plan.contains_key(*path))
            .cloned()
            .collect::<Vec<_>>();
        for path in expired {
            if let Some((_, id)) = self.registered.remove(&path) {
                endpoint.stop_watch(id)?;
            }
        }
        let mut new_watch = false;
        for path in self.plan.keys().take(MAX_CONFIGURATION_WATCHES) {
            if self.registered.contains_key(path) {
                continue;
            }
            let Some(id) = WatchId::configuration() else {
                self.incomplete = true;
                break;
            };
            match endpoint.register_watch(id, path, || Ok(())) {
                Ok(relative) => {
                    self.registered.insert(path.clone(), (relative, id));
                    new_watch = true;
                }
                // A watched ancestor observes a later creation.
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) if endpoint.control().is_some() => {
                    tracing::debug!(kind = ?error.kind(), "configuration watch coverage incomplete");
                    self.incomplete = true;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(changed || new_watch)
    }

    fn poll(&mut self, endpoint: &LocalExecutionEndpoint) -> io::Result<bool> {
        let mut changed = false;
        let mut retired = Vec::new();
        for (path, (relative, id)) in &self.registered {
            match endpoint.poll_watch(*id, relative) {
                Ok(notice) => {
                    if notice.retired {
                        retired.push(path.clone());
                    }
                    changed |= notice.resync_required
                        || notice.changed_paths.iter().any(|entry| {
                            let Some(filter) = &self.plan[path] else {
                                return true;
                            };
                            // On folding volumes the name reported by the watcher
                            // can differ from the exact name read by the loader.
                            entry.file_name().is_some_and(|actual| {
                                filter.iter().any(|name| {
                                    actual
                                        .as_encoded_bytes()
                                        .eq_ignore_ascii_case(name.as_encoded_bytes())
                                })
                            })
                        });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
                Err(error) => return Err(error),
            }
        }
        for path in retired {
            self.registered.remove(&path);
        }
        Ok(changed)
    }

    fn stop_all(self, endpoint: &LocalExecutionEndpoint) {
        for (_, id) in self.registered.into_values() {
            if endpoint.control().is_none() || endpoint.stop_watch(id).is_err() {
                // The binding owns unresolved physical settlement.
                return;
            }
        }
    }
}

async fn watch_session(
    manager: SessionRuntimeManager,
    session: SessionId,
    endpoint: Arc<LocalExecutionEndpoint>,
    stop: CancellationToken,
) {
    let mut watches = Watches::default();
    let mut check = Instant::now();
    let mut ticks = tokio::time::interval(POLL);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            () = stop.cancelled() => break,
            _ = ticks.tick() => (),
        }
        let worker = manager.clone();
        let binding = endpoint.clone();
        let id = session.clone();
        let now = Instant::now();
        let result = tokio::task::spawn_blocking(move || {
            let success = (|| {
                let mut changed = watches.poll(&binding)?;
                if changed || now >= check {
                    changed |= watches.register(
                        &binding,
                        worker.workspace_watch_plan(&id, binding.workspace()),
                    )?;
                    changed |= watches.incomplete;
                    check = now + COVERAGE_RECHECK;
                }
                if changed {
                    worker.observe_bound_source(&id, &binding);
                    #[cfg(test)]
                    worker
                        .configuration
                        .test_hooks
                        .reach("bound_configuration_watch_resynced");
                }
                Ok::<_, io::Error>(())
            })();
            (watches, check, success)
        })
        .await;
        let Ok((next, due, outcome)) = result else {
            return;
        };
        watches = next;
        check = due;
        if outcome.is_err() {
            break;
        }
    }
    let _ = tokio::task::spawn_blocking(move || watches.stop_all(&endpoint)).await;
}
