//! The sole low-level process owner of one subagent child (Issue #60).
//!
//! This module owns the **physical** boundary of a child rustX runtime:
//!
//! ```text
//! spawn            (own process group, inherited control channel on fd 0)
//! control channel  (bounded framed IPC; also the parent-liveness authority)
//! start gate       (the child performs no semantic work before Delegate)
//! escalation       (Cancel frame -> SIGTERM group -> SIGKILL group)
//! wait/reap        (the direct child is reaped exactly here)
//! ```
//!
//! The logical [`SubagentRegistry`](super::registry) never holds an OS
//! process handle: it stages a [`StagedChild`], and the one ownership
//! commit moves the handle into the registry-owned driver task. No second
//! kill/reap authority exists anywhere.
//!
//! # Platform semantics
//!
//! The child is a **direct child** of the runtime process on both Linux and
//! macOS, spawned into its own process group so escalation can signal the
//! whole group. Parent-lifetime containment does not depend on a platform
//! primitive: the control channel endpoint is inherited by the child as its
//! fd 0, and every other copy of the parent's endpoint is `CLOEXEC` by
//! construction (Rust socket creation), so a hard parent death — including
//! `SIGKILL` — closes the channel and the child observes EOF. A child that
//! loses its parent drains its own runtime and exits; the restarted parent
//! classifies the durable ownership as interrupted and never reattaches.

use std::process::Stdio;
use std::time::Duration;

use chrono_tz::Tz;

use crate::context::SessionContextPolicy;
use crate::model::session::SessionModelConfig;
use crate::runtime::identity::SubagentId;

use super::ipc::{
    ChildFrame, ParentFrame, ResultFrame, SubagentChildSpec, read_child_frame, write_parent_frame,
};

/// The liveness guard of the startup handshake. The child composes only
/// local state before `Ready` (catalog file, durable store, capability
/// plane), so a handshake that outlasts this bound is a hung child; the
/// stage is then torn down. This is a supervision policy bound, never a
/// test synchronization mechanism.
const STARTUP_LIVENESS: Duration = Duration::from_secs(60);

/// The grace a child gets to drain after a `Cancel` frame before the
/// supervisor escalates to `SIGTERM` on the child's process group.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

/// The grace after `SIGTERM` before `SIGKILL`.
const TERM_GRACE: Duration = Duration::from_secs(5);

/// The composition inputs one child process is spawned with.
///
/// Everything the child needs that is not process-inheritable travels in
/// the typed [`SubagentChildSpec`] over the control channel; this plan
/// carries the spawn-time locations only.
#[derive(Debug, Clone)]
pub struct SubagentSpawnPlan {
    /// The rustX program executed as the child (the current executable in
    /// production; the test binary's sibling `rustx` in tests).
    pub program: std::path::PathBuf,
    /// The model catalog path handed to the child.
    pub models: std::path::PathBuf,
    /// The shared read-only workspace root.
    pub workspace: std::path::PathBuf,
    /// The **parent** runtime-private root; each child gets a disjoint
    /// `subagents/<subagent_id>` subtree under it.
    pub runtime_root: std::path::PathBuf,
    /// The frozen session model configuration of every child.
    pub model: SessionModelConfig,
    /// The conversation timezone inherited by the child.
    pub timezone: Option<Tz>,
    /// The session context policy inherited by the child.
    pub context: SessionContextPolicy,
}

impl SubagentSpawnPlan {
    /// The child-private runtime root of one subagent.
    #[must_use]
    pub fn child_runtime_root(&self, subagent_id: &SubagentId) -> std::path::PathBuf {
        self.runtime_root
            .join("subagents")
            .join(subagent_id.as_str())
    }

    /// The one typed startup specification of a child.
    #[must_use]
    pub(crate) fn child_spec(
        &self,
        subagent_id: &SubagentId,
        child_conversation_id: &crate::runtime::identity::ConversationId,
        child_agent_id: &crate::runtime::identity::AgentId,
        parent_agent_id: &crate::runtime::identity::AgentId,
        profile: super::SubagentProfile,
    ) -> SubagentChildSpec {
        SubagentChildSpec {
            protocol_version: super::ipc::SUBAGENT_IPC_VERSION,
            subagent_id: subagent_id.clone(),
            child_conversation_id: child_conversation_id.clone(),
            child_agent_id: child_agent_id.clone(),
            parent_agent_id: parent_agent_id.clone(),
            profile: profile.name().to_owned(),
            persona: profile.persona(),
            models: self.models.clone(),
            model: self.model.clone(),
            timezone: self.timezone,
            context: self.context,
            workspace: self.workspace.clone(),
            runtime_root: self.child_runtime_root(subagent_id),
        }
    }
}

/// A failure to stage a child process.
///
/// Every failure happens before any ownership commit: no `SubagentId` is
/// published, no capacity is consumed, and no staged process survives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    /// The child-private runtime root could not be prepared.
    WorkspaceSetup {
        /// The failure detail.
        detail: String,
    },
    /// The OS spawn failed.
    Spawn {
        /// The failure detail.
        detail: String,
    },
    /// The startup handshake failed: the child reported a startup error,
    /// violated the protocol, or exited before `Ready`.
    Handshake {
        /// The failure detail.
        detail: String,
    },
}

impl core::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WorkspaceSetup { detail } => {
                write!(f, "cannot prepare the child runtime root: {detail}")
            }
            Self::Spawn { detail } => write!(f, "cannot spawn the child runtime: {detail}"),
            Self::Handshake { detail } => {
                write!(f, "the child startup handshake failed: {detail}")
            }
        }
    }
}

impl std::error::Error for SpawnError {}

/// Spawns and stages one child behind the start gate.
///
/// Staging performs, in order: child runtime-root preparation, control
/// channel creation, process spawn into its own process group, the
/// versioned `Hello` handoff, and the `Ready` handshake. Any failure tears
/// the stage down completely (the staged process is killed and reaped)
/// before the error is returned.
///
/// # Errors
///
/// Returns the typed [`SpawnError`] of the first failing stage.
pub(crate) async fn spawn_staged(
    plan: &SubagentSpawnPlan,
    spec: &SubagentChildSpec,
) -> Result<StagedChild, SpawnError> {
    let runtime_root = plan.child_runtime_root(&spec.subagent_id);
    std::fs::create_dir_all(&runtime_root).map_err(|error| SpawnError::WorkspaceSetup {
        detail: format!("{}: {error}", runtime_root.display()),
    })?;
    let mut staged = match spawn_process(plan, spec, &runtime_root).await {
        Ok(staged) => staged,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&runtime_root);
            return Err(error);
        }
    };
    if let Err(error) = staged.handshake(spec).await {
        staged.rollback().await;
        return Err(error);
    }
    Ok(staged)
}

/// Spawns the child process with the control channel inherited as fd 0 and
/// hands it the typed startup specification.
async fn spawn_process(
    plan: &SubagentSpawnPlan,
    spec: &SubagentChildSpec,
    runtime_root: &std::path::Path,
) -> Result<StagedChild, SpawnError> {
    // The control channel: one UnixStream pair. The child end becomes the
    // child's fd 0; both ends are CLOEXEC, so no other descendant of either
    // process can hold the liveness endpoint open.
    let (parent_end, child_end) =
        tokio::net::UnixStream::pair().map_err(|error| SpawnError::Spawn {
            detail: format!("control channel: {error}"),
        })?;
    let child_std = child_end.into_std().map_err(|error| SpawnError::Spawn {
        detail: format!("control channel: {error}"),
    })?;
    let child_stdio: Stdio = std::os::fd::OwnedFd::from(child_std).into();
    // The child's diagnostics never travel through a pipe to the parent: a
    // hard parent death must not turn the child's stderr writes into
    // SIGPIPE. They land in a child-private diagnostics log instead.
    let diagnostics = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(runtime_root.join("diagnostics.log"))
        .map_err(|error| SpawnError::WorkspaceSetup {
            detail: format!("diagnostics log: {error}"),
        })?;
    let mut command = tokio::process::Command::new(&plan.program);
    command
        .arg("--subagent-child")
        .stdin(child_stdio)
        .stdout(Stdio::null())
        .stderr(Stdio::from(diagnostics));
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn().map_err(|error| SpawnError::Spawn {
        detail: format!("{}: {error}", plan.program.display()),
    })?;
    let mut staged = StagedChild {
        child,
        control: parent_end,
        runtime_root: runtime_root.to_path_buf(),
    };
    // The typed startup specification travels over the control channel; no
    // temporary configuration file is ever written.
    write_parent_frame(
        &mut staged.control,
        &ParentFrame::Hello(Box::new(spec.clone())),
    )
    .await
    .map_err(|error| SpawnError::Handshake {
        detail: error.to_string(),
    })?;
    Ok(staged)
}

/// A spawned child parked behind the start gate, not yet owned by the
/// conversation.
///
/// The child has composed and activated its runtime (it answered `Ready`)
/// but has received no delegation and therefore performs no semantic work.
/// The handle is consumed by exactly one of [`StagedChild::into_driver`]
/// (ownership commit) or [`StagedChild::rollback`] (teardown).
#[derive(Debug)]
pub(crate) struct StagedChild {
    child: tokio::process::Child,
    control: tokio::net::UnixStream,
    runtime_root: std::path::PathBuf,
}

/// The physical terminal outcome the driver observed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PhysicalOutcome {
    /// The child emitted its terminal result candidate and then exited;
    /// the process is reaped.
    Completed(ResultFrame),
    /// The child exited (reaped) without a valid terminal result envelope.
    Lost {
        /// The bounded diagnostic.
        diagnostic: String,
    },
}

impl StagedChild {
    /// Moves the staged child into the driver task at the ownership commit.
    pub(crate) fn into_driver(self, delegate: super::ipc::DelegationFrame) -> ChildDriver {
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(4);
        let task = tokio::spawn(async move {
            drive_child(self.child, self.control, delegate, command_rx).await
        });
        ChildDriver {
            commands: command_tx,
            task,
        }
    }

    /// Tears the staged child down completely: kill the process group, reap
    /// the direct child, and remove the child runtime root.
    ///
    /// Called on every pre-commit failure and on every rolled-back commit
    /// attempt; the registry's no-rollback and no-stale-partial-record
    /// guarantees extend to the OS process.
    pub(crate) async fn rollback(mut self) {
        kill_group(&self.child, Signal::Kill);
        let _ = tokio::time::timeout(TERM_GRACE, self.child.wait()).await;
        let _ = std::fs::remove_dir_all(&self.runtime_root);
    }

    /// Completes the startup handshake: awaits `Ready` (or an honest
    /// `StartupError`), bounded by the startup liveness guard.
    async fn handshake(&mut self, spec: &SubagentChildSpec) -> Result<(), SpawnError> {
        let handshake = async {
            loop {
                match read_child_frame(&mut self.control).await {
                    Ok(Some(ChildFrame::Ready(ready))) if ready.subagent_id == spec.subagent_id => {
                        return Ok(());
                    }
                    Ok(Some(ChildFrame::Ready(_))) => {
                        return Err(SpawnError::Handshake {
                            detail: "the child reported the wrong identity".to_owned(),
                        });
                    }
                    Ok(Some(ChildFrame::StartupError(error))) => {
                        return Err(SpawnError::Handshake {
                            detail: error.message,
                        });
                    }
                    Ok(Some(ChildFrame::Diagnostic(_))) => continue,
                    Ok(Some(ChildFrame::Result(_))) => {
                        return Err(SpawnError::Handshake {
                            detail: "the child produced a result before delegation".to_owned(),
                        });
                    }
                    Ok(None) => {
                        let exit = try_wait(&mut self.child);
                        return Err(SpawnError::Handshake {
                            detail: format!("the child exited before Ready{exit}"),
                        });
                    }
                    Err(error) => {
                        return Err(SpawnError::Handshake {
                            detail: error.to_string(),
                        });
                    }
                }
            }
        };
        match tokio::time::timeout(STARTUP_LIVENESS, handshake).await {
            Ok(result) => result,
            Err(_) => Err(SpawnError::Handshake {
                detail: "the child did not answer Ready within the startup liveness bound"
                    .to_owned(),
            }),
        }
    }
}

/// The narrow control handle the registry holds for one running child.
///
/// It carries **no** OS process handle: it can only forward cancellation
/// into the driver task and observe task completion. Kill, reap, and the
/// control stream stay inside the driver task, the sole process owner.
#[derive(Debug)]
pub(crate) struct ChildDriver {
    commands: tokio::sync::mpsc::Sender<DriverCommand>,
    task: tokio::task::JoinHandle<PhysicalOutcome>,
}

/// The driver command channel payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriverCommand {
    /// Cancel the child: send the `Cancel` frame, then escalate.
    Cancel,
}

impl ChildDriver {
    /// Splits the handle into the narrow command channel and the driver
    /// task: the registry keeps the former, the settlement task awaits the
    /// latter.
    pub(crate) fn split(
        self,
    ) -> (
        tokio::sync::mpsc::Sender<DriverCommand>,
        tokio::task::JoinHandle<PhysicalOutcome>,
    ) {
        (self.commands, self.task)
    }
}

/// The sole driver of one committed child: sends the delegation, observes
/// frames, owns cancellation escalation, waits, and reaps.
///
/// Terminal order is structural: the driver only returns after the child
/// process has exited and been reaped, and the registry settles durable
/// authority only from the returned [`PhysicalOutcome`]. A `Cancel` frame
/// alone is never treated as proof of shutdown; the proof is always the
/// reaped process.
async fn drive_child(
    mut child: tokio::process::Child,
    mut control: tokio::net::UnixStream,
    delegate: super::ipc::DelegationFrame,
    mut commands: tokio::sync::mpsc::Receiver<DriverCommand>,
) -> PhysicalOutcome {
    if let Err(error) = write_parent_frame(&mut control, &ParentFrame::Delegate(delegate)).await {
        kill_group(&child, Signal::Kill);
        let _ = child.wait().await;
        return PhysicalOutcome::Lost {
            diagnostic: format!("could not deliver the delegation: {error}"),
        };
    }
    let mut result: Option<ResultFrame> = None;
    let mut violation: Option<String> = None;
    let mut cancel_deadline: Option<tokio::time::Instant> = None;
    let mut kill_deadline: Option<tokio::time::Instant> = None;
    let mut eof = false;
    loop {
        if result.is_some() || violation.is_some() || eof {
            break;
        }
        tokio::select! {
            command = commands.recv() => {
                // A closed channel (registry dropped) is not a command.
                if matches!(command, Some(DriverCommand::Cancel)) && cancel_deadline.is_none() {
                    // The Cancel frame is a request, not evidence. The
                    // driver keeps observing; the escalation deadline
                    // below is the bounded fallback.
                    if write_parent_frame(&mut control, &ParentFrame::Cancel).await.is_err() {
                        violation = Some("control channel lost while cancelling".to_owned());
                    } else {
                        cancel_deadline = Some(tokio::time::Instant::now() + CANCEL_GRACE);
                    }
                }
            }
            frame = read_child_frame(&mut control) => {
                match frame {
                    Ok(Some(ChildFrame::Result(frame))) => result = Some(frame),
                    Ok(Some(ChildFrame::Diagnostic(_))) => {}
                    Ok(Some(_)) => {
                        violation = Some(
                            "protocol violation: unexpected frame after Ready".to_owned(),
                        );
                    }
                    Ok(None) => eof = true,
                    Err(error) => violation = Some(error.to_string()),
                }
            }
            _ = async {
                match cancel_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            }, if cancel_deadline.is_some() && kill_deadline.is_none() => {
                kill_group(&child, Signal::Term);
                kill_deadline = Some(tokio::time::Instant::now() + TERM_GRACE);
            }
            _ = async {
                match kill_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            }, if kill_deadline.is_some() => {
                kill_group(&child, Signal::Kill);
                // The kill is the final escalation; reap below.
                eof = true;
            }
        }
    }
    // Terminal settlement requires the reaped process, never a frame alone
    // and never a kill signal alone.
    reap(&mut child).await;
    match (result, violation) {
        (Some(frame), _) => PhysicalOutcome::Completed(frame),
        (None, Some(diagnostic)) => PhysicalOutcome::Lost { diagnostic },
        (None, None) => PhysicalOutcome::Lost {
            diagnostic: "the child exited without a terminal result".to_owned(),
        },
    }
}

/// Reaps the direct child, escalating if it outlives its cancellation
/// grace after the control channel is gone.
async fn reap(child: &mut tokio::process::Child) {
    if tokio::time::timeout(CANCEL_GRACE, child.wait())
        .await
        .is_err()
    {
        kill_group(child, Signal::Term);
        if tokio::time::timeout(TERM_GRACE, child.wait())
            .await
            .is_err()
        {
            kill_group(child, Signal::Kill);
            let _ = child.wait().await;
        }
    }
}

fn try_wait(child: &mut tokio::process::Child) -> String {
    match child.try_wait() {
        Ok(Some(status)) => format!(" (exit status: {status})"),
        _ => String::new(),
    }
}

#[derive(Debug, Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

/// Signals the child's whole process group. A best-effort operation: a
/// group that no longer exists is exactly the outcome we wanted.
fn kill_group(child: &tokio::process::Child, signal: Signal) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // The child was spawned with `process_group(0)`, so its pgid
            // equals its pid and the group can never collide with the
            // parent's group. `killpg` keeps this unsafe-free.
            let nix_signal = match signal {
                Signal::Term => nix::sys::signal::Signal::SIGTERM,
                Signal::Kill => nix::sys::signal::Signal::SIGKILL,
            };
            let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid as i32), nix_signal);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signal;
        let _ = child;
    }
}
