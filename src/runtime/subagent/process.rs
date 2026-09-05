//! The sole low-level process owner of one subagent child (Issue #60).
//!
//! This module owns the **physical** boundary of a child rustX runtime:
//!
//! ```text
//! spawn            (own process group, inherited control channel on fd 0,
//!                   inherited disposable observation channel on fd 1)
//! control channel  (bounded framed IPC; also the parent-liveness authority)
//! observation      (Activity frames only, child->parent; its stall or loss
//! channel          is diagnostics-only and never evidences lifecycle)
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

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::context::{AgentStatusConfig, SessionContextPolicy};
use crate::runtime::identity::{ConversationId, SubagentId};
use crate::runtime::interaction::{InteractionRef, InteractionResponse, RoutedInteractionError};
use crate::runtime::types::CancellationReason;

use super::anchors::{NestedUnitSettlement, RetainedProcessUnits, contain_retained};
use super::ipc::{
    ChildFrame, ChildTerminalMode, ParentFrame, ProcessUnitAckFrame, ProcessUnitRefusalFrame,
    ResultFrame, SubagentChildSpec, read_child_frame, write_parent_frame,
};
use super::registry::{SubagentInteractionSink, SubagentTerminalMode};
use super::resolver::ResolvedSubagentSpec;
use super::workspace::{WorkspaceLease, WorkspaceSnapshot};

/// The liveness guard of the startup handshake. The child composes only
/// local state before `Ready` (catalog file, durable store, capability
/// plane), so a handshake that outlasts this bound is a hung child; the
/// stage is then torn down. This is a supervision policy bound, never a
/// test synchronization mechanism.
#[cfg(not(test))]
const STARTUP_LIVENESS: Duration = Duration::from_mins(1);
#[cfg(test)]
const STARTUP_LIVENESS: Duration = Duration::from_secs(5);

/// The grace a child gets to drain after a `Cancel` frame before the
/// supervisor escalates to `SIGTERM` on the child's process group.
#[cfg(not(test))]
const CANCEL_GRACE: Duration = Duration::from_secs(10);
#[cfg(test)]
const CANCEL_GRACE: Duration = Duration::from_millis(200);

/// The grace after `SIGTERM` before `SIGKILL`.
#[cfg(not(test))]
const TERM_GRACE: Duration = Duration::from_secs(5);
#[cfg(test)]
const TERM_GRACE: Duration = Duration::from_millis(200);

/// The number of OS-random bytes in one physical spawn-incarnation token.
/// The token is never semantic state; it only prevents a later process
/// generation from receiving an earlier child's mutable pathname.
const INCARNATION_TOKEN_BYTES: usize = 16;

/// A bounded retry budget for the exclusive incarnation-directory create.
/// A collision is harmless — the existing directory is never opened or
/// removed — but a broken entropy source must not turn staging into an
/// unbounded loop.
const INCARNATION_ALLOCATION_ATTEMPTS: usize = 8;

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
    /// The **parent** runtime-private root. Each child gets a fresh physical
    /// incarnation directory below
    /// `subagents/<semantic_subagent_id>/`, and only that directory is ever
    /// given to the child as mutable authority.
    pub runtime_root: std::path::PathBuf,
    /// The parent runtime's frozen model timeout policy, inherited by every
    /// child unchanged (Issue #138).
    pub model_timeout_policy: crate::model::ModelTimeoutPolicy,
    /// The launch-scoped Agent Status configuration inherited by the child.
    pub agent_status: AgentStatusConfig,
    /// The session context policy inherited by the child.
    pub context: SessionContextPolicy,
}

impl SubagentSpawnPlan {
    /// Reserves one fresh physical runtime namespace for a spawn
    /// incarnation.
    ///
    /// The semantic `SubagentId` is only a grouping component. The mutable
    /// authority is the exclusively-created `incarnation-...` child below
    /// that grouping directory, so a stale child can keep writing in its own
    /// old directory without ever naming a later child's directory.
    pub(crate) fn allocate_child_runtime_root(
        &self,
        subagent_id: &SubagentId,
    ) -> Result<PhysicalChildRuntimeRoot, SpawnError> {
        PhysicalChildRuntimeRoot::allocate(
            &self.runtime_root,
            &ConversationId::new(subagent_id.as_str()),
        )
    }

    /// The one typed startup specification of a child.
    ///
    /// The spawn plan contributes only launch-scoped physical locations and
    /// inherited launch policy. Every semantic decision — agent identity,
    /// instructions, the resolved model invocation, capabilities, Skills,
    /// project instructions — comes from the already-frozen
    /// [`ResolvedSubagentSpec`] the invoking attempt's generation produced.
    /// The plan deliberately carries no model catalog path: a child that
    /// could open one could observe a catalog edit the parent never
    /// authorized.
    #[must_use]
    #[allow(clippy::too_many_arguments)] // mirrors the typed child identity/resource boundary
    pub(crate) fn child_spec(
        &self,
        subagent_id: &SubagentId,
        child_conversation_id: &crate::runtime::identity::ConversationId,
        child_agent_id: &crate::runtime::identity::AgentId,
        parent_agent_id: &crate::runtime::identity::AgentId,
        resolved: &ResolvedSubagentSpec,
        approval_mode: crate::runtime::types::ApprovalMode,
        runtime_root: &PhysicalChildRuntimeRoot,
        workspace: &WorkspaceLease,
        terminal: &SubagentTerminalMode,
    ) -> SubagentChildSpec {
        SubagentChildSpec {
            protocol_version: super::ipc::SUBAGENT_IPC_VERSION,
            subagent_id: subagent_id.clone(),
            child_conversation_id: child_conversation_id.clone(),
            child_agent_id: child_agent_id.clone(),
            parent_agent_id: parent_agent_id.clone(),
            resolved: resolved.clone(),
            approval_mode,
            model_timeout_policy: self.model_timeout_policy,
            agent_status: self.agent_status.clone(),
            context: self.context,
            workspace_snapshot: workspace.snapshot().clone(),
            runtime_root: runtime_root.path().to_path_buf(),
            terminal: match terminal {
                SubagentTerminalMode::Normal => ChildTerminalMode::Normal,
                SubagentTerminalMode::WorkflowOutput { output_schema, .. } => {
                    ChildTerminalMode::WorkflowOutput {
                        output_schema: output_schema.clone(),
                    }
                }
            },
        }
    }
}

/// The unique physical mutable namespace of one staged spawn incarnation.
///
/// This value is deliberately not `Clone`: the path is one lifecycle-owned
/// capability. Its pathname contains an OS-random token and was created with
/// an exclusive directory operation, so it cannot alias a still-existing
/// namespace from an earlier rustX process generation.
#[derive(Debug)]
pub(crate) struct PhysicalChildRuntimeRoot {
    path: PathBuf,
    /// The stable child Message Ledger/Event Journal database, when this is a
    /// production-allocated root. Test-only roots do not own a durable store.
    durable_store: Option<PathBuf>,
}

impl PhysicalChildRuntimeRoot {
    /// Creates the semantic grouping directory and exclusively creates one
    /// fresh incarnation directory beneath it.
    fn allocate(parent: &Path, conversation_id: &ConversationId) -> Result<Self, SpawnError> {
        if !super::is_safe_child_conversation_component(conversation_id) {
            return Err(SpawnError::WorkspaceSetup {
                detail: format!(
                    "child conversation identity {:?} is not a safe filesystem component",
                    conversation_id.as_str()
                ),
            });
        }
        let semantic_root = super::child_conversation_store_path(parent, conversation_id)
            .parent()
            .expect("a child conversation database has a semantic parent")
            .to_path_buf();
        let durable_store = super::child_conversation_store_path(parent, conversation_id);
        for path in [
            durable_store.clone(),
            PathBuf::from(format!("{}-wal", durable_store.display())),
            PathBuf::from(format!("{}-shm", durable_store.display())),
        ] {
            if path.exists() {
                return Err(SpawnError::ConversationIdentityInUse {
                    conversation_id: conversation_id.clone(),
                    path,
                });
            }
        }
        std::fs::create_dir_all(&semantic_root).map_err(|error| SpawnError::WorkspaceSetup {
            detail: format!(
                "create semantic child runtime grouping {}: {error}",
                semantic_root.display()
            ),
        })?;

        for _ in 0..INCARNATION_ALLOCATION_ATTEMPTS {
            let mut token = [0u8; INCARNATION_TOKEN_BYTES];
            getrandom::fill(&mut token).map_err(|error| SpawnError::WorkspaceSetup {
                detail: format!(
                    "generate a physical child spawn-incarnation token for {}: {error}",
                    semantic_root.display()
                ),
            })?;
            let name = format!("incarnation-{}", hex_token(&token));
            let path = semantic_root.join(name);
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        durable_store: Some(durable_store),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(SpawnError::WorkspaceSetup {
                        detail: format!(
                            "create physical child runtime root {}: {error}",
                            path.display()
                        ),
                    });
                }
            }
        }

        Err(SpawnError::WorkspaceSetup {
            detail: format!(
                "allocate a fresh physical child runtime root below {} after {} attempts",
                semantic_root.display(),
                INCARNATION_ALLOCATION_ATTEMPTS
            ),
        })
    }

    /// The exact path handed to the child and used by its private stores.
    #[must_use]
    fn path(&self) -> &Path {
        &self.path
    }

    /// Removes exactly this incarnation directory. The semantic grouping
    /// directory is intentionally left alone because another incarnation
    /// may be using it.
    fn remove(self) -> std::io::Result<()> {
        std::fs::remove_dir_all(self.path)
    }

    /// Removes the exact stable store owned by an uncommitted spawn. A
    /// committed child never calls this method: its durable conversation is
    /// retained after the physical execution root is settled.
    fn remove_durable_store(&self) -> std::io::Result<()> {
        let Some(database) = &self.durable_store else {
            return Ok(());
        };
        let mut failures = Vec::new();
        for path in [
            database.clone(),
            PathBuf::from(format!("{}-wal", database.display())),
            PathBuf::from(format!("{}-shm", database.display())),
        ] {
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                failures.push(format!("{}: {error}", path.display()));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(std::io::Error::other(failures.join("; ")))
        }
    }

    /// Wraps a test-created directory in the same lifecycle owner used by a
    /// production staged child.
    #[cfg(test)]
    fn from_existing(path: PathBuf) -> Self {
        Self {
            path,
            durable_store: None,
        }
    }
}

/// Encodes a physical token without introducing another identifier type or
/// dependency. The result is safe as one directory component.
fn hex_token(token: &[u8; INCARNATION_TOKEN_BYTES]) -> String {
    let mut encoded = String::with_capacity(INCARNATION_TOKEN_BYTES * 2);
    for byte in token {
        write!(&mut encoded, "{byte:02x}").expect("writing into a String cannot fail");
    }
    encoded
}

/// A failure to stage a child process.
///
/// Every failure happens before any ownership commit: no `SubagentId` is
/// published, no capacity is consumed, and no staged process survives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    /// The platform process-supervision prerequisite that makes nested
    /// containment provable could not be established (Issue #145).
    ///
    /// On Linux this is `PR_SET_CHILD_SUBREAPER`, and it must exist
    /// **before** the child is spawned: a subreaper installed afterwards
    /// does not retroactively adopt the child's orphaned supervised units,
    /// so the parent could not contain them. Failing here is deliberate —
    /// the alternative is claiming containment authority that does not
    /// exist.
    ContainmentPrerequisite {
        /// The activation failure detail.
        detail: String,
    },
    /// The child-private runtime root could not be prepared.
    WorkspaceSetup {
        /// The failure detail.
        detail: String,
    },
    /// The semantic child identity already has a durable conversation store.
    /// A new physical incarnation must never reuse that store: the caller
    /// must advance to a fresh child identity instead.
    ConversationIdentityInUse {
        /// The identity whose durable store is already present.
        conversation_id: ConversationId,
        /// The existing durable store or sidecar that caused the refusal.
        path: PathBuf,
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
    /// The invoking attempt's cancellation became observable while the
    /// child was still staging (Issue #145). The staged child was settled
    /// completely — the `Cancel` frame delivered best-effort, the process
    /// reaped, every retained nested anchor contained, the runtime root
    /// removed — before this was returned.
    Cancelled,
    /// The staged child could not be conclusively killed and reaped after a
    /// pre-commit failure.
    Rollback {
        /// The failure detail.
        detail: String,
    },
}

impl core::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ContainmentPrerequisite { detail } => write!(
                f,
                "cannot establish the process-supervision prerequisite required before a \
                 subagent child may create nested supervised process units: {detail}"
            ),
            Self::WorkspaceSetup { detail } => {
                write!(f, "cannot prepare the child runtime root: {detail}")
            }
            Self::ConversationIdentityInUse {
                conversation_id,
                path,
            } => write!(
                f,
                "child conversation identity {} already has a durable store at {}; refusing to reuse it",
                conversation_id.as_str(),
                path.display()
            ),
            Self::Spawn { detail } => write!(f, "cannot spawn the child runtime: {detail}"),
            Self::Handshake { detail } => {
                write!(f, "the child startup handshake failed: {detail}")
            }
            Self::Cancelled => write!(
                f,
                "the invoking attempt was cancelled while the child was still staging"
            ),
            Self::Rollback { detail } => {
                write!(
                    f,
                    "the staged child rollback was not proven complete: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for SpawnError {}

/// A pre-commit teardown failure. Returning this error is deliberately
/// stronger than pretending rollback completed: the caller must not report
/// a rolled-back ownership decision while the direct child may be
/// unreaped, nor remove a private runtime root before that reap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RollbackError {
    /// The direct child could not be waited/reaped conclusively.
    Reap {
        /// The failure detail.
        detail: String,
    },
    /// The child was reaped but its private runtime root could not be
    /// removed.
    Cleanup {
        /// The failure detail.
        detail: String,
    },
    /// The child was reaped but one or more of its retained nested
    /// supervised process units could not be proven physically terminal
    /// (Issue #145). Rollback is not complete while owned work may survive.
    NestedContainment {
        /// The bounded per-unit detail.
        detail: String,
    },
    /// The staged workspace could not be safely settled before ownership was
    /// committed. The lease remains conservative: dirty or otherwise
    /// unproven work is preserved rather than force-destroyed.
    Workspace {
        /// The bounded workspace settlement detail.
        detail: String,
    },
}

impl core::fmt::Display for RollbackError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Reap { detail } => write!(f, "child reap failed: {detail}"),
            Self::Cleanup { detail } => write!(f, "child cleanup failed: {detail}"),
            Self::NestedContainment { detail } => write!(
                f,
                "child nested process-unit containment is unproven: {detail}"
            ),
            Self::Workspace { detail } => {
                write!(
                    f,
                    "child workspace settlement was not proven safe: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for RollbackError {}

/// Spawns and stages one child behind the start gate.
///
/// The caller reserves the exclusive physical runtime-root token before
/// invoking this function. Staging then performs, in order: control channel
/// creation, process spawn into its own process group, the versioned `Hello`
/// handoff, and the `Ready` handshake. Any failure tears the stage down
/// completely (the staged process is killed and reaped) before the error is
/// returned. The supplied root token is consumed by this function and becomes
/// owned by `StagedChild` before the Hello/Ready handshake begins. The
/// workspace lease is transferred at the same boundary, so every asynchronous
/// preparation failure has one staged owner for both the process and the
/// workspace.
///
/// `preparation_cancellation` is the invoking attempt's cancellation
/// authority (Issue #145): it participates in staging **from the start**,
/// not only at the ownership commit. If it becomes observable while the
/// child is still preparing, the child is told (`Cancel`), never treated
/// as startable, and every staged physical resource — the direct child and
/// every retained nested anchor — is settled before this returns
/// [`SpawnError::Cancelled`].
///
/// # Errors
///
/// Returns the typed [`SpawnError`] of the first failing stage, or
/// [`SpawnError::Cancelled`] when the preparation cancellation won.
pub(crate) async fn spawn_staged(
    plan: &SubagentSpawnPlan,
    spec: &SubagentChildSpec,
    runtime_root: PhysicalChildRuntimeRoot,
    workspace: WorkspaceLease,
    preparation_cancellation: &crate::runtime::cancellation::CancellationSignal,
) -> Result<StagedChild, SpawnError> {
    if preparation_cancellation.is_cancelled() {
        return Err(
            discard_unstaged_resources(runtime_root, workspace, SpawnError::Cancelled).await,
        );
    }
    if spec.runtime_root.as_path() != runtime_root.path() {
        let owned_path = runtime_root.path().display().to_string();
        let specified_path = spec.runtime_root.display().to_string();
        return Err(
            discard_unstaged_resources(
                runtime_root,
                workspace,
                SpawnError::WorkspaceSetup {
                    detail: format!(
                        "child spec runtime root {specified_path} does not match the reserved physical incarnation {owned_path}"
                    ),
                },
            )
            .await,
        );
    }
    if spec.workspace_snapshot != *workspace.snapshot() {
        return Err(discard_unstaged_resources(
            runtime_root,
            workspace,
            SpawnError::WorkspaceSetup {
                detail: "child spec workspace snapshot does not match the staged workspace lease"
                    .to_owned(),
            },
        )
        .await);
    }
    if let Err(detail) = spec.workspace_snapshot.validate() {
        return Err(discard_unstaged_resources(
            runtime_root,
            workspace,
            SpawnError::WorkspaceSetup {
                detail: format!("child spec carries an invalid workspace snapshot: {detail}"),
            },
        )
        .await);
    }
    // The containment prerequisite is established BEFORE the child exists.
    // A subagent child may create nested supervised process units during
    // its own preparation, and the parent can only contain an orphaned unit
    // anchor it has adopted — which on Linux requires child-subreaper mode
    // to have been active when the intermediate process died. Installing it
    // lazily inside the child (as the child's own local runner does) would
    // make the child a subreaper, not this process, and the anchors this
    // process retains would be unreachable.
    if let Err(detail) = crate::runtime::process_supervision::ensure_child_subreaper() {
        return Err(discard_unstaged_resources(
            runtime_root,
            workspace,
            SpawnError::ContainmentPrerequisite { detail },
        )
        .await);
    }
    let spawned = match spawn_process(plan, runtime_root.path(), workspace.logical_workspace()) {
        Ok(spawned) => spawned,
        Err(error) => {
            return Err(discard_unstaged_resources(runtime_root, workspace, error).await);
        }
    };
    let mut staged = StagedChild {
        child: spawned.child,
        control: spawned.control,
        observation: spawned.observation,
        runtime_root,
        workspace: Some(workspace),
        retained: RetainedProcessUnits::default(),
    };
    // The typed startup specification travels over the control channel; no
    // temporary configuration file is ever written.
    if let Err(error) = write_parent_frame(
        &mut staged.control,
        &ParentFrame::Hello(Box::new(spec.clone())),
    )
    .await
    {
        return match staged.rollback().await {
            Ok(()) => Err(SpawnError::Handshake {
                detail: error.to_string(),
            }),
            Err(rollback) => Err(SpawnError::Rollback {
                detail: format!("{error}; {rollback}"),
            }),
        };
    }
    if let Err(error) = staged.handshake(spec, preparation_cancellation).await {
        if matches!(error, SpawnError::Cancelled) {
            return match staged.rollback_cancelled().await {
                Ok(()) => Err(SpawnError::Cancelled),
                Err(rollback) => Err(SpawnError::Rollback {
                    detail: format!("cancelled staging rollback: {rollback}"),
                }),
            };
        }
        return match staged.rollback().await {
            Ok(()) => Err(error),
            Err(rollback) => Err(SpawnError::Rollback {
                detail: format!("{error}; {rollback}"),
            }),
        };
    }
    Ok(staged)
}

/// Settles a pre-spawn failure after removing only the physical child root
/// reserved for that failed spawn. The semantic grouping directory is never
/// removed. The acquired workspace is settled through its staged-owner path
/// before this function returns, so it cannot be forgotten between workspace
/// acquisition and process staging.
async fn discard_unstaged_resources(
    runtime_root: PhysicalChildRuntimeRoot,
    workspace: WorkspaceLease,
    error: SpawnError,
) -> SpawnError {
    let path = runtime_root.path().display().to_string();
    let durable_error = runtime_root.remove_durable_store().err().map(|cleanup| {
        format!("could not remove unowned durable child conversation store for {path}: {cleanup}")
    });
    let root_error = runtime_root.remove().err().map(|cleanup| {
        format!("could not remove unowned physical child runtime root {path}: {cleanup}")
    });
    let workspace_error = workspace
        .settle_staged()
        .await
        .err()
        .map(|error| error.detail);
    match (durable_error, root_error, workspace_error) {
        (None, None, None) => error,
        (durable_error, root_error, workspace_error) => SpawnError::Rollback {
            detail: format!(
                "{error}; {}",
                [durable_error, root_error, workspace_error]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        },
    }
}

/// Spawns the child process with the control channel inherited as fd 0 and
/// the disposable observation channel inherited as fd 1 (Issue #178).
fn spawn_process(
    plan: &SubagentSpawnPlan,
    runtime_root: &Path,
    project_workspace: &Path,
) -> Result<SpawnedProcess, SpawnError> {
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
    // The observation channel (Issue #178): a second UnixStream pair whose
    // child end becomes the child's fd 1. It carries Activity frames only,
    // child-to-parent, with a backpressure domain fully independent of the
    // control channel: a stalled or lost observation transport delays no
    // control frame and evidences no lifecycle fact.
    let (observation_parent_end, observation_child_end) =
        tokio::net::UnixStream::pair().map_err(|error| SpawnError::Spawn {
            detail: format!("observation channel: {error}"),
        })?;
    let observation_child_std =
        observation_child_end
            .into_std()
            .map_err(|error| SpawnError::Spawn {
                detail: format!("observation channel: {error}"),
            })?;
    let observation_stdio: Stdio = std::os::fd::OwnedFd::from(observation_child_std).into();
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
        // The typed child spec is the project-workspace authority. This
        // matters for subprocesses that inherit cwd in addition to the
        // native tools that receive ConversationToolRuntime's Workspace.
        .current_dir(project_workspace)
        .arg("--subagent-child")
        .stdin(child_stdio)
        .stdout(observation_stdio)
        .stderr(Stdio::from(diagnostics));
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn().map_err(|error| SpawnError::Spawn {
        detail: format!("{}: {error}", plan.program.display()),
    })?;
    Ok(SpawnedProcess {
        child,
        control: parent_end,
        observation: observation_parent_end,
    })
}

/// The process and channel endpoints that become one `StagedChild` only
/// after the physical-root token is moved into that owner.
#[derive(Debug)]
struct SpawnedProcess {
    child: tokio::process::Child,
    control: tokio::net::UnixStream,
    observation: tokio::net::UnixStream,
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
    /// The parent end of the disposable observation channel (Issue #178).
    /// Held unread while staged — the child never publishes activity before
    /// its delegation — and moved into the driver at the ownership commit,
    /// where exactly one observation receiver task drains it into the
    /// registry read model.
    observation: tokio::net::UnixStream,
    runtime_root: PhysicalChildRuntimeRoot,
    /// The staged project-workspace owner. It moves into the driver at the
    /// durable ownership boundary, or is settled by rollback before then.
    workspace: Option<WorkspaceLease>,
    /// The nested supervised process units this child has anchored in this
    /// process (Issue #145).
    ///
    /// External capability preparation — MCP stdio startup, a uv
    /// environment build, a Skill environment subprocess — happens while
    /// the child is still *staged*, so anchors can exist long before any
    /// durable ownership commit. The staged owner therefore owns them, and
    /// the one ownership commit moves the whole set into the driver task.
    retained: RetainedProcessUnits,
}

/// The complete physical settlement of one committed child (Issue #145).
///
/// > A direct child reap is not proof of physical settlement while retained
/// > nested anchors are unresolved.
///
/// The driver therefore publishes both facts together, and only after both
/// are decided: the direct child's terminal outcome and the settlement of
/// every nested supervised process unit the child anchored here.
/// The registry consumes the complete value as a physical proof boundary: an
/// unproven nested, workspace, or private-runtime settlement is an existing
/// runtime `Failed` terminal input, never a warning that can leave a semantic
/// success published as ordinary `Succeeded`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PhysicalSettlement {
    /// The direct child's terminal outcome.
    pub outcome: PhysicalOutcome,
    /// The settlement of the child's retained nested process units.
    pub nested: NestedUnitSettlement,
    /// A failure to remove the exact physical root after the child and all
    /// proven nested units settled. An unproven nested unit deliberately
    /// leaves its root in place instead.
    pub runtime_root_cleanup_error: Option<String>,
    /// The final workspace inspection and cleanup/handoff facts.
    pub workspace: super::workspace::WorkspaceSettlement,
}

impl PhysicalSettlement {
    /// A settlement with no nested units, for the paths that never had any.
    pub(crate) fn of(outcome: PhysicalOutcome) -> Self {
        Self {
            outcome,
            nested: NestedUnitSettlement {
                contained: Vec::new(),
                unproven: Vec::new(),
            },
            runtime_root_cleanup_error: None,
            workspace: super::workspace::WorkspaceSettlement::shared(WorkspaceSnapshot::shared(
                PathBuf::from("<shared-workspace>"),
            )),
        }
    }
}

/// The physical terminal outcome the driver observed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PhysicalOutcome {
    /// The child emitted its terminal result candidate and then exited;
    /// the process is reaped.
    Completed(ResultFrame),
    /// The child exited (reaped) without a valid terminal result envelope.
    /// Its semantic outcome is unknown, but the direct process and control
    /// settlement were proven. `cancellation_delivered` says that the
    /// registry's already-committed cancellation was successfully written to
    /// the child; it is physical evidence, not a cancellation reason.
    Lost {
        /// The bounded diagnostic.
        diagnostic: String,
        /// Whether the typed cancellation frame was successfully delivered.
        cancellation_delivered: bool,
    },
    /// The low-level driver could not prove a required process/control
    /// operation. This is an explicit infrastructure/containment failure,
    /// never an `Interrupted` semantic outcome.
    ControlFailure {
        /// The bounded diagnostic.
        diagnostic: String,
    },
}

impl StagedChild {
    /// Moves the staged child into the driver task at the ownership commit.
    ///
    /// This is the **exactly-once** ownership transfer of both physical
    /// resources: the OS child handle and the retained nested process-unit
    /// anchors move together into the driver task. They are moved, never
    /// copied, so there is one owner at every instant and no second
    /// containment authority can exist.
    pub(crate) fn into_driver(
        self,
        delegate: super::ipc::DelegationFrame,
        activity: Option<super::registry::SubagentActivitySink>,
        interactions: Option<SubagentInteractionSink>,
        provider_available: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> ChildDriver {
        let Self {
            child,
            control,
            observation,
            runtime_root,
            workspace,
            retained,
        } = self;
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);
        // The driver owns the OS handle immediately after this call, but it
        // cannot send Delegate until the registry has installed its command
        // handle and resolved any cancellation intent that committed during
        // the ownership-to-driver handoff.
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let Ok(cancelled_before_start) = start_rx.await else {
                // The observation channel is disposable: dropping the
                // endpoint is its whole teardown here.
                drop(observation);
                return settle_after_driver_loss(
                    child,
                    control,
                    retained,
                    runtime_root,
                    workspace,
                    "the registry dropped the child start decision".to_owned(),
                    false,
                )
                .await;
            };
            drive_child(
                child,
                control,
                observation,
                retained,
                runtime_root,
                workspace,
                delegate,
                command_rx,
                cancelled_before_start,
                activity,
                interactions,
                provider_available,
            )
            .await
        });
        ChildDriver {
            commands: command_tx,
            start: start_tx,
            task,
        }
    }

    /// Tears the staged child down completely: kill the process group, reap
    /// the direct child, settle retained anchors, and remove exactly the
    /// physical incarnation root it owns.
    ///
    /// Called on every pre-commit failure and on every rolled-back commit
    /// attempt; the registry's no-rollback and no-stale-partial-record
    /// guarantees extend to the OS process.
    pub(crate) async fn rollback(self) -> Result<(), RollbackError> {
        self.settle(false).await
    }

    /// Tears down a staged child whose preparation the invoking attempt
    /// cancelled (Issue #145).
    ///
    /// The handshake already delivered the `Cancel` frame, so the child's
    /// own preparation cancellation authority has fired: it cancels and
    /// physically settles its preparatory supervised units itself. This
    /// path first gives the child the cancellation grace to do exactly
    /// that and exit, then escalates (group `SIGTERM`, then `SIGKILL`),
    /// reaps, contains every retained nested anchor, and removes the child
    /// spawn-incarnation root — the same complete settlement as
    /// [`StagedChild::rollback`].
    pub(crate) async fn rollback_cancelled(self) -> Result<(), RollbackError> {
        self.settle(true).await
    }

    /// The one staged-teardown settlement. `cancellation_grace` gives a
    /// cancelled child the grace window to settle its own preparation
    /// before signal escalation.
    async fn settle(mut self, cancellation_grace: bool) -> Result<(), RollbackError> {
        if cancellation_grace {
            // The Cancel frame is already on the wire; the child settles
            // its preparation and exits on its own within the grace.
            let _ = tokio::time::timeout(CANCEL_GRACE, self.child.wait()).await;
        }
        kill_group(&self.child, Signal::Term);
        let term_wait = tokio::time::timeout(TERM_GRACE, self.child.wait()).await;
        match term_wait {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                return Err(RollbackError::Reap {
                    detail: format!("wait after SIGTERM: {error}"),
                });
            }
            Err(_) => {
                kill_group(&self.child, Signal::Kill);
                self.child
                    .wait()
                    .await
                    .map_err(|error| RollbackError::Reap {
                        detail: format!("wait after SIGKILL: {error}"),
                    })?;
            }
        }
        // The direct child is reaped, so every nested unit it anchored here
        // is now orphaned (and, on Linux, adopted by this process).
        // Rollback is not physically complete until each retained anchor is
        // settled: a staged child that created supervised work must not
        // leave that work running behind a "rolled back" ownership answer.
        let settlement = contain_retained(self.retained.take()).await;
        let workspace_result = if let Some(workspace) = self.workspace.take() {
            if settlement.unproven.is_empty() {
                workspace.settle_after_child().await
            } else {
                workspace.preserve_after_unresolved_nested(
                    "a nested supervised process anchor remains physically unresolved",
                )
            }
        } else {
            super::workspace::WorkspaceSettlement::shared(WorkspaceSnapshot::shared(PathBuf::from(
                "<test-shared-workspace>",
            )))
        };
        // The child-private runtime root is independent of the project
        // workspace. Once the direct child and every nested anchor are
        // settled, remove that disposable root even when the project
        // workspace must be retained for handoff. Returning early on a dirty
        // workspace would otherwise leak runtime-private materialization and
        // diagnostics merely because the user work correctly survived.
        let runtime_root = self.runtime_root;
        remove_inspection_liveness_marker(&runtime_root);
        let runtime_root_cleanup_error = if settlement.unproven.is_empty() {
            let path = runtime_root.path().display().to_string();
            let durable_error = runtime_root.remove_durable_store().err().map(|error| {
                format!("remove uncommitted durable child conversation store for {path}: {error}")
            });
            let root_error = runtime_root
                .remove()
                .err()
                .map(|error| format!("remove child runtime root {path}: {error}"));
            [durable_error, root_error]
                .into_iter()
                .flatten()
                .reduce(|left, right| format!("{left}; {right}"))
        } else {
            None
        };
        if let Some(detail) = settlement.unproven_diagnostic() {
            return Err(RollbackError::NestedContainment { detail });
        }
        let workspace_issue = workspace_result.error().map(str::to_owned).or_else(|| {
            workspace_result
                .handoff()
                .map(|_| "staged workspace was retained because it contains work".to_owned())
        });
        if let Some(detail) = workspace_issue {
            return Err(RollbackError::Workspace {
                detail: match runtime_root_cleanup_error {
                    Some(root_error) => format!("{detail}; {root_error}"),
                    None => detail,
                },
            });
        }
        if let Some(detail) = runtime_root_cleanup_error {
            return Err(RollbackError::Cleanup { detail });
        }
        Ok(())
    }

    /// Constructs a staged child from raw parts (tests only).
    ///
    /// The test plays the child role over `control`'s and `observation`'s
    /// peers while `child` supplies the real OS-process kill/reap semantics
    /// the driver owns.
    #[cfg(test)]
    pub(crate) fn for_test(
        child: tokio::process::Child,
        control: tokio::net::UnixStream,
        observation: tokio::net::UnixStream,
        runtime_root: std::path::PathBuf,
    ) -> Self {
        Self {
            child,
            control,
            observation,
            runtime_root: PhysicalChildRuntimeRoot::from_existing(runtime_root),
            workspace: None,
            retained: RetainedProcessUnits::default(),
        }
    }

    /// Attaches the prepared workspace lease to this staged process. The
    /// process cannot enter the driver until this transfer is complete.
    #[cfg(test)]
    pub(crate) fn with_workspace(mut self, workspace: WorkspaceLease) -> Self {
        debug_assert!(self.workspace.is_none());
        self.workspace = Some(workspace);
        self
    }

    /// The immutable workspace binding held by the staged owner. The
    /// registry uses this at its durable ownership boundary instead of
    /// retaining a parallel prepared-state copy.
    pub(crate) fn workspace_snapshot(&self) -> &WorkspaceSnapshot {
        self.workspace
            .as_ref()
            .expect("a registry-staged child owns a workspace lease")
            .snapshot()
    }

    /// The number of nested process-unit anchors this staged child
    /// currently has retained (tests only).
    #[cfg(test)]
    pub(crate) fn retained_anchor_count(&self) -> usize {
        self.retained.len()
    }

    /// Retains one anchor directly (tests only), standing in for an offer
    /// that arrived over the control channel.
    #[cfg(test)]
    pub(crate) fn retain_for_test(
        &mut self,
        unit_id: crate::runtime::identity::ProcessUnitId,
        pgid: i32,
    ) {
        self.retained.retain(unit_id, pgid).expect("test retention");
    }

    /// Runs the startup handshake against a bare subagent identity (tests
    /// only), so the anchor-offer arm can be exercised without composing a
    /// whole child specification.
    #[cfg(test)]
    pub(crate) async fn handshake_for_test(
        &mut self,
        subagent_id: &str,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<(), SpawnError> {
        let expected = crate::runtime::identity::SubagentId::new(subagent_id);
        handshake_core(
            &mut self.control,
            &mut self.child,
            &mut self.retained,
            &expected,
            cancellation,
        )
        .await
    }

    /// Completes the startup handshake: awaits `Ready` (or an honest
    /// `StartupError`), bounded by the startup liveness guard.
    async fn handshake(
        &mut self,
        spec: &SubagentChildSpec,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<(), SpawnError> {
        let handshake = handshake_core(
            &mut self.control,
            &mut self.child,
            &mut self.retained,
            &spec.subagent_id,
            cancellation,
        );
        match tokio::time::timeout(STARTUP_LIVENESS, handshake).await {
            Ok(result) => result,
            Err(_) => Err(SpawnError::Handshake {
                detail: "the child did not answer Ready within the startup liveness bound"
                    .to_owned(),
            }),
        }
    }
}

/// The one startup-handshake loop: consumes child frames until `Ready`
/// while the invoking attempt's preparation cancellation participates from
/// the start (Issue #145).
///
/// The cancellation arm is **biased** ahead of the frame arm: once the
/// attempt cancellation is observable, a `Ready` that is already queued
/// can never turn the child into a startable staged child. The child is
/// told via the `Cancel` frame — its own preparation cancellation
/// authority is driven by it — and the staged teardown then settles every
/// physical resource before the caller's start decision returns.
async fn handshake_core(
    control: &mut tokio::net::UnixStream,
    child: &mut tokio::process::Child,
    retained: &mut RetainedProcessUnits,
    expected: &crate::runtime::identity::SubagentId,
    cancellation: &crate::runtime::cancellation::CancellationSignal,
) -> Result<(), SpawnError> {
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                // Preparation cancellation is not a committed child
                // attempt cancellation. There is no semantic reason to
                // invent here; the child only needs its preparation signal
                // woken so staged physical teardown can complete.
                let _ = write_parent_frame(
                    control,
                    &ParentFrame::Cancel { reason: None },
                )
                .await;
                return Err(SpawnError::Cancelled);
            }
            frame = read_child_frame(control) => match frame {
                Ok(Some(ChildFrame::Ready(ready))) if ready.subagent_id == *expected => {
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
                Ok(Some(ChildFrame::Diagnostic(_))) => {}
                // External capability preparation runs before `Ready`,
                // so a nested supervised unit can legitimately offer its
                // anchor here. The staged owner retains it; the child's
                // local `START` gate opens only after the ACK below.
                Ok(Some(ChildFrame::AnchorOffered(offer))) => {
                    if let Err(error) = answer_anchor_offer(control, retained, &offer).await {
                        return Err(SpawnError::Handshake {
                            detail: error.to_string(),
                        });
                    }
                }
                Ok(Some(ChildFrame::AnchorReleased(release))) => {
                    retained.release(&release.unit_id, release.pgid);
                }
                Ok(Some(ChildFrame::Result(_))) => {
                    return Err(SpawnError::Handshake {
                        detail: "the child produced a result before delegation".to_owned(),
                    });
                }
                Ok(Some(
                    ChildFrame::InteractionRequested(_)
                    | ChildFrame::InteractionSettled { .. }
                    | ChildFrame::InteractionResponseResult(_)
                    | ChildFrame::InteractionPublicationAdmissionRequested(_),
                )) => {
                    return Err(SpawnError::Handshake {
                        detail: "the child produced an interaction frame before Ready".to_owned(),
                    });
                }
                Ok(None) => {
                    let exit = try_wait(child);
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
    }
}

/// Retains one offered nested anchor and answers the child.
///
/// The retention happens **strictly before** the acknowledgement is written,
/// so an acknowledged unit is always already retained: a child that dies
/// immediately after receiving the ACK is contained, and a child that dies
/// before it never started the unit's semantic command.
async fn answer_anchor_offer(
    control: &mut tokio::net::UnixStream,
    retained: &mut RetainedProcessUnits,
    offer: &super::ipc::ProcessUnitAnchorFrame,
) -> Result<(), super::ipc::ProtocolError> {
    let frame = match retained.retain(offer.unit_id.clone(), offer.pgid) {
        Ok(()) => ParentFrame::AnchorAccepted(ProcessUnitAckFrame {
            unit_id: offer.unit_id.clone(),
        }),
        Err(refusal) => ParentFrame::AnchorRefused(ProcessUnitRefusalFrame {
            unit_id: offer.unit_id.clone(),
            reason: refusal.reason().to_owned(),
        }),
    };
    write_parent_frame(control, &frame).await
}

/// The narrow control handle the registry holds for one running child.
///
/// It carries **no** OS process handle: it can only forward cancellation
/// into the driver task and observe task completion. Kill, reap, and the
/// control stream stay inside the driver task, the sole process owner.
#[derive(Debug)]
pub(crate) struct ChildDriver {
    commands: tokio::sync::mpsc::Sender<DriverCommand>,
    start: tokio::sync::oneshot::Sender<Option<CancellationReason>>,
    task: tokio::task::JoinHandle<PhysicalSettlement>,
}

/// The driver command channel payload.
#[derive(Debug)]
pub(crate) enum DriverCommand {
    /// Cancel the child with the reason committed by the registry: send the
    /// typed `Cancel` frame, then escalate. The driver never chooses the
    /// semantic reason.
    Cancel {
        /// The registry's first-winner cancellation cause.
        reason: CancellationReason,
    },
    /// Forward one root response to the child coordinator and resolve the
    /// sender when the child has applied the originating transition.
    InteractionRespond {
        /// Transport-only response correlation identity.
        response_id: u64,
        /// The full routed interaction identity.
        interaction: InteractionRef,
        /// The typed response; the child coordinator validates it.
        response: InteractionResponse,
        /// The child coordinator's accepted or fail-closed result.
        result: tokio::sync::oneshot::Sender<Result<(), RoutedInteractionError>>,
    },
}

impl ChildDriver {
    /// Splits the handle into the narrow command channel and the driver
    /// task: the registry keeps the former, the settlement task awaits the
    /// latter.
    pub(crate) fn split(
        self,
    ) -> (
        tokio::sync::mpsc::Sender<DriverCommand>,
        tokio::sync::oneshot::Sender<Option<CancellationReason>>,
        tokio::task::JoinHandle<PhysicalSettlement>,
    ) {
        (self.commands, self.start, self.task)
    }
}

/// Settles a driver-owned child after a low-level control operation fails.
/// A proven direct reap makes this an ordinary unknown process/IPC outcome;
/// only an unproven reap remains an explicit infrastructure failure. The
/// driver never turns a failed control write into a semantic cancellation.
async fn settle_after_driver_loss(
    mut child: tokio::process::Child,
    mut control: tokio::net::UnixStream,
    mut retained: RetainedProcessUnits,
    runtime_root: PhysicalChildRuntimeRoot,
    workspace: Option<WorkspaceLease>,
    diagnostic: String,
    cancellation_delivered: bool,
) -> PhysicalSettlement {
    let _ = control.shutdown().await;
    let outcome = match reap(&mut child).await {
        Ok(()) => PhysicalOutcome::Lost {
            diagnostic,
            cancellation_delivered,
        },
        Err(error) => PhysicalOutcome::ControlFailure {
            diagnostic: format!("{diagnostic}; {error}"),
        },
    };
    settle_nested(outcome, &mut retained, runtime_root, workspace).await
}

/// The sole driver of one committed child: owns the control channel, the
/// cancellation escalation, the wait, and the reap — plus the one
/// observation receiver of the disposable activity channel (Issue #178).
///
/// The observation receiver is a fully independent task on its own
/// transport: it can only decode Activity frames into the registry read
/// model. It holds no lifecycle authority — its stall, EOF, or error never
/// delays, cancels, settles, or evidences anything — and it is torn down
/// with the drive.
#[allow(clippy::too_many_arguments)] // one driver owns every physical child resource
async fn drive_child(
    child: tokio::process::Child,
    control: tokio::net::UnixStream,
    observation: tokio::net::UnixStream,
    retained: RetainedProcessUnits,
    runtime_root: PhysicalChildRuntimeRoot,
    workspace: Option<WorkspaceLease>,
    delegate: super::ipc::DelegationFrame,
    commands: tokio::sync::mpsc::Receiver<DriverCommand>,
    cancelled_before_start: Option<CancellationReason>,
    // The registry's live-activity sink (Issue #178). `None` only in tests
    // that drive the physical pipeline without a registry.
    activity: Option<super::registry::SubagentActivitySink>,
    interactions: Option<SubagentInteractionSink>,
    provider_available: Option<tokio::sync::watch::Receiver<bool>>,
) -> PhysicalSettlement {
    let observation_task = if let Some(sink) = activity {
        Some(tokio::spawn(run_observation_receiver(observation, sink)))
    } else {
        // No read model to feed: release the endpoint. The child's
        // observation writer ends itself on the first failed write —
        // observation is disposable.
        drop(observation);
        None
    };
    let settlement = drive_child_control(
        child,
        control,
        retained,
        runtime_root,
        workspace,
        delegate,
        commands,
        cancelled_before_start,
        interactions,
        provider_available,
    )
    .await;
    if let Some(task) = observation_task {
        task.abort();
        let _ = task.await;
    }
    settlement
}

/// The one observer-side owner of the disposable observation transport
/// (Issue #178): decode `Activity` and apply it into the registry read
/// model — nothing else.
///
/// Observation EOF or failure ends only this task. It is never child
/// process/lifecycle evidence: it cannot settle, cancel, or outlive the
/// child, it retains no process anchors, and it touches no journal.
async fn run_observation_receiver(
    mut stream: tokio::net::UnixStream,
    sink: super::registry::SubagentActivitySink,
) {
    loop {
        match super::ipc::read_activity_frame(&mut stream).await {
            Ok(Some(frame)) => sink.apply(frame.observation),
            Ok(None) | Err(_) => return,
        }
    }
}

/// The control-plane drive of one committed child: sends the delegation,
/// observes control frames, owns cancellation escalation, waits, and reaps.
///
/// Terminal order is structural: the driver only returns after the child
/// process has exited and been reaped, and the registry settles durable
/// authority only from the returned [`PhysicalOutcome`]. A `Cancel` frame
/// alone is never treated as proof of shutdown; the proof is always the
/// reaped process.
#[allow(clippy::too_many_lines)] // one coherent delegate/observe/settle pipeline
#[allow(clippy::too_many_arguments)] // one driver owns every physical child resource
async fn drive_child_control(
    mut child: tokio::process::Child,
    mut control: tokio::net::UnixStream,
    mut retained: RetainedProcessUnits,
    runtime_root: PhysicalChildRuntimeRoot,
    workspace: Option<WorkspaceLease>,
    mut delegate: super::ipc::DelegationFrame,
    mut commands: tokio::sync::mpsc::Receiver<DriverCommand>,
    cancelled_before_start: Option<CancellationReason>,
    interactions: Option<SubagentInteractionSink>,
    mut provider_available: Option<tokio::sync::watch::Receiver<bool>>,
) -> PhysicalSettlement {
    let mut cancel_deadline = None;
    let mut cancellation_delivered = false;
    let mut response_waiters: HashMap<
        u64,
        (
            InteractionRef,
            tokio::sync::oneshot::Sender<Result<(), RoutedInteractionError>>,
        ),
    > = HashMap::new();
    let mut commands_open = true;
    if let Some(reason) = cancelled_before_start {
        if let Err(error) = write_parent_frame(
            &mut control,
            &ParentFrame::Cancel {
                reason: Some(reason),
            },
        )
        .await
        {
            return settle_after_driver_loss(
                child,
                control,
                retained,
                runtime_root,
                workspace,
                format!("could not deliver the cancellation: {error}"),
                false,
            )
            .await;
        }
        cancellation_delivered = true;
        cancel_deadline = Some(tokio::time::Instant::now() + CANCEL_GRACE);
    } else {
        if let Some(provider_available) = provider_available.as_ref() {
            delegate.interaction_provider_available = *provider_available.borrow();
        }
        if let Err(error) = write_parent_frame(&mut control, &ParentFrame::Delegate(delegate)).await
        {
            return settle_after_driver_loss(
                child,
                control,
                retained,
                runtime_root,
                workspace,
                format!("could not deliver the delegation: {error}"),
                false,
            )
            .await;
        }
    }
    let mut result: Option<ResultFrame> = None;
    let mut violation: Option<String> = None;
    let mut kill_deadline: Option<tokio::time::Instant> = None;
    let mut eof = false;
    loop {
        if result.is_some() || violation.is_some() || eof {
            break;
        }
        tokio::select! {
            command = commands.recv(), if commands_open => {
                match command {
                    Some(DriverCommand::Cancel { reason })
                        if cancel_deadline.is_none() => {
                        // The Cancel frame is a request, not evidence. The
                        // driver keeps observing; the escalation deadline
                        // below is the bounded fallback.
                        match write_parent_frame(
                            &mut control,
                            &ParentFrame::Cancel {
                                reason: Some(reason),
                            },
                        )
                        .await
                        {
                            Ok(()) => {
                                cancellation_delivered = true;
                                cancel_deadline =
                                    Some(tokio::time::Instant::now() + CANCEL_GRACE);
                            }
                            Err(error) => {
                                violation = Some(format!(
                                    "control channel lost while delivering cancellation: {error}"
                                ));
                            }
                        }
                    }
                    Some(DriverCommand::InteractionRespond {
                        response_id,
                        interaction,
                        response,
                        result,
                    }) => {
                        let frame = ParentFrame::InteractionRespond {
                            response_id,
                            interaction: interaction.clone(),
                            response,
                        };
                        match write_parent_frame(&mut control, &frame).await {
                            Ok(()) => {
                                let previous =
                                    response_waiters.insert(response_id, (interaction, result));
                                if let Some((expected, sender)) = previous {
                                    let _ = sender.send(Err(
                                        RoutedInteractionError::NotPending {
                                            interaction: expected,
                                        },
                                    ));
                                    violation = Some(
                                        "duplicate child interaction response correlation id"
                                            .to_owned(),
                                    );
                                }
                            }
                            Err(error) => {
                                let _ = result.send(Err(RoutedInteractionError::NotPending {
                                    interaction,
                                }));
                                violation = Some(format!(
                                    "control channel lost while delivering interaction response: {error}"
                                ));
                            }
                        }
                    }
                    Some(DriverCommand::Cancel { .. }) => {}
                    None => commands_open = false,
                }
            }
            frame = read_child_frame(&mut control) => {
                match frame {
                    Ok(Some(ChildFrame::Result(frame))) => result = Some(frame),
                    Ok(Some(ChildFrame::Diagnostic(_))) => {}
                    // The nested anchor protocol stays live for the whole
                    // committed lifetime: a unit may be created at any point
                    // during the child's semantic work.
                    Ok(Some(ChildFrame::AnchorOffered(offer))) => {
                        if let Err(error) =
                            answer_anchor_offer(&mut control, &mut retained, &offer).await
                        {
                            violation = Some(format!(
                                "control channel lost while acknowledging a nested process \
                                 unit anchor: {error}"
                            ));
                        }
                    }
                    Ok(Some(ChildFrame::AnchorReleased(release))) => {
                        retained.release(&release.unit_id, release.pgid);
                    }
                    Ok(Some(ChildFrame::InteractionRequested(request))) => {
                        if let Some(sink) = &interactions {
                            sink.apply_requested(request);
                        }
                    }
                    Ok(Some(ChildFrame::InteractionSettled {
                        interaction,
                        outcome,
                    })) => {
                        if let Some(sink) = &interactions {
                            sink.apply_settled(&interaction, &outcome);
                        }
                    }
                    Ok(Some(ChildFrame::InteractionResponseResult(frame))) => {
                        let Some((expected, sender)) = response_waiters.remove(&frame.response_id)
                        else {
                            violation = Some(
                                "child returned an unknown interaction response correlation id"
                                    .to_owned(),
                            );
                            continue;
                        };
                        if expected == frame.interaction {
                            let _ = sender.send(frame.result);
                        } else {
                            let _ = sender.send(Err(RoutedInteractionError::NotPending {
                                interaction: expected,
                            }));
                            violation = Some(
                                "child returned a mismatched interaction response identity"
                                    .to_owned(),
                            );
                        }
                    }
                    Ok(Some(
                        ChildFrame::InteractionPublicationAdmissionRequested(request),
                    )) => {
                        let admitted = interactions
                            .as_ref()
                            .is_some_and(|sink| sink.admit_publication(&request.interaction));
                        // The request frame uses `admitted = false` as its
                        // wire shape; the root authority's answer must echo
                        // the exact identity and set the decision explicitly.
                        let mut result = request;
                        result.admitted = admitted;
                        if let Err(error) = write_parent_frame(
                            &mut control,
                            &ParentFrame::InteractionPublicationAdmissionResult(result),
                        )
                        .await
                        {
                            violation = Some(format!(
                                "control channel lost while delivering interaction publication admission: {error}"
                            ));
                        }
                    }
                    Ok(Some(_)) => {
                        violation = Some(
                            "protocol violation: unexpected frame after Ready".to_owned(),
                        );
                    }
                    Ok(None) => eof = true,
                    Err(error) => violation = Some(error.to_string()),
                }
            }
            provider = async {
                match provider_available.as_mut() {
                    Some(receiver) => match receiver.changed().await {
                        Ok(()) => Some(*receiver.borrow()),
                        Err(_) => None,
                    },
                    None => std::future::pending::<Option<bool>>().await,
                }
            }, if provider_available.is_some() => {
                let Some(available) = provider else {
                    provider_available = None;
                    continue;
                };
                if let Err(error) = write_parent_frame(
                    &mut control,
                    &ParentFrame::InteractionProviderAvailable { available },
                )
                .await
                {
                    violation = Some(format!(
                        "control channel lost while delivering interaction provider state: {error}"
                    ));
                }
            }
            () = async {
                match cancel_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            }, if cancel_deadline.is_some() && kill_deadline.is_none() => {
                kill_group(&child, Signal::Term);
                kill_deadline = Some(tokio::time::Instant::now() + TERM_GRACE);
            }
            () = async {
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
    for (_, (interaction, sender)) in response_waiters {
        let _ = sender.send(Err(RoutedInteractionError::NotPending { interaction }));
    }
    // Terminal settlement requires the reaped process, never a frame alone
    // and never a kill signal alone. Closing the write half is the
    // well-behaved child's drain signal after its terminal frame.
    let _ = control.shutdown().await;
    if let Err(error) = reap(&mut child).await {
        return settle_after_driver_loss(
            child,
            control,
            retained,
            runtime_root,
            workspace,
            format!("the child could not be reaped: {error}"),
            cancellation_delivered,
        )
        .await;
    }
    let outcome = match (result, violation) {
        (Some(frame), _) => PhysicalOutcome::Completed(frame),
        (None, Some(diagnostic)) => PhysicalOutcome::Lost {
            diagnostic,
            cancellation_delivered,
        },
        (None, None) => PhysicalOutcome::Lost {
            diagnostic: "the child exited without a terminal result".to_owned(),
            cancellation_delivered,
        },
    };
    settle_nested(outcome, &mut retained, runtime_root, workspace).await
}

/// Settles every anchor the child still had retained when it exited.
///
/// This is the one place the "reap is not settlement" rule is enforced: the
/// driver task does not return — and therefore the registry's counted
/// lifecycle admission is not released and runtime drain cannot declare
/// quiescence — until every retained nested unit is contained or explicitly
/// reported unprovable.
async fn settle_nested(
    outcome: PhysicalOutcome,
    retained: &mut RetainedProcessUnits,
    runtime_root: PhysicalChildRuntimeRoot,
    workspace: Option<WorkspaceLease>,
) -> PhysicalSettlement {
    let nested = contain_retained(retained.take()).await;
    remove_inspection_liveness_marker(&runtime_root);
    // Workspace inspection is deliberately after nested containment. A
    // nested process may still hold or mutate the worktree after the direct
    // child exits; only the complete physical settlement permits cleanup.
    let workspace = match workspace {
        Some(lease) if nested.unproven.is_empty() => lease.settle_after_child().await,
        Some(lease) => lease.preserve_after_unresolved_nested(
            "a nested supervised process anchor remains physically unresolved",
        ),
        None => super::workspace::WorkspaceSettlement::shared(WorkspaceSnapshot::shared(
            PathBuf::from("<shared-workspace>"),
        )),
    };
    let runtime_root_cleanup_error = if nested.unproven.is_empty() {
        let path = runtime_root.path().display().to_string();
        runtime_root
            .remove()
            .err()
            .map(|error| format!("remove child runtime root {path}: {error}"))
    } else {
        // An unproven nested unit may still be alive, so keep its mutable
        // namespace rather than deleting it before physical settlement is
        // established. The old incarnation remains isolated from all later
        // incarnation roots.
        None
    };
    PhysicalSettlement {
        outcome,
        nested,
        runtime_root_cleanup_error,
        workspace,
    }
}

/// Removes the exact disposable live-inspection lease belonging to one
/// physical child incarnation. The driver calls this only after the direct
/// child has been reaped; the child itself removes the lease on its normal
/// path, and this owner closes the abnormal-death gap.
fn remove_inspection_liveness_marker(runtime_root: &PhysicalChildRuntimeRoot) {
    let Some(semantic_root) = runtime_root.path().parent() else {
        return;
    };
    let _ = std::fs::remove_file(semantic_root.join(".inspection-live"));
}

/// Reaps the direct child, escalating if it outlives its cancellation
/// grace after the control channel is gone.
async fn reap(child: &mut tokio::process::Child) -> Result<(), String> {
    match tokio::time::timeout(CANCEL_GRACE, child.wait()).await {
        Ok(Ok(_)) => return Ok(()),
        Ok(Err(error)) => return Err(format!("wait during reap: {error}")),
        Err(_) => {}
    }
    kill_group(child, Signal::Term);
    match tokio::time::timeout(TERM_GRACE, child.wait()).await {
        Ok(Ok(_)) => return Ok(()),
        Ok(Err(error)) => return Err(format!("wait after SIGTERM: {error}")),
        Err(_) => {}
    }
    kill_group(child, Signal::Kill);
    child
        .wait()
        .await
        .map(|_| ())
        .map_err(|error| format!("wait after SIGKILL: {error}"))
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
            let Ok(pid) = i32::try_from(pid) else {
                return;
            };
            let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), nix_signal);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signal;
        let _ = child;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::path::{Path, PathBuf};

    use super::{
        PhysicalChildRuntimeRoot, PhysicalOutcome, StagedChild, SubagentSpawnPlan, settle_nested,
        spawn_staged,
    };
    use crate::context::{AgentStatusConfig, SessionContextPolicy};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::{ConversationId, ProcessUnitId, SubagentId};
    use crate::runtime::subagent::anchors::RetainedProcessUnits;
    use crate::runtime::subagent::ipc::{
        ChildFrame, ParentFrame, ProcessUnitAnchorFrame, ReadyFrame, read_parent_frame,
        write_child_frame,
    };
    use crate::runtime::subagent::{
        SubagentWorkspaceManager, SubagentWorkspacePolicy, WorkspaceCleanup,
        WorkspaceSettlementDisposition, WorkspaceUnresolvedReason,
    };

    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

    /// A staged child plus the socket the test plays the child role over.
    struct StagedHarness {
        staged: StagedChild,
        child: tokio::net::UnixStream,
        /// The child end of the observation channel: the test decides
        /// whether the staged driver's observation receiver sees a live,
        /// a stalled, or a dead transport.
        observation_child: tokio::net::UnixStream,
        runtime_root: PathBuf,
        _dir: tempfile::TempDir,
    }

    fn stage() -> StagedHarness {
        let dir = tempfile::tempdir().expect("lab");
        let root = dir.path().join("child");
        std::fs::create_dir_all(&root).expect("child root");
        let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
        let (observation_parent, observation_child) =
            tokio::net::UnixStream::pair().expect("observation pair");
        // The stand-in child leads its own process group, exactly like a
        // real staged child: rollback signals that group.
        let process = tokio::process::Command::new("sleep")
            .arg("300")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .expect("a stand-in direct child");
        StagedHarness {
            staged: StagedChild::for_test(process, parent, observation_parent, root),
            child,
            observation_child,
            runtime_root: dir.path().join("child"),
            _dir: dir,
        }
    }

    fn allocation_plan(runtime_root: PathBuf) -> SubagentSpawnPlan {
        SubagentSpawnPlan {
            program: PathBuf::from("/nonexistent/rustx"),
            runtime_root,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            agent_status: AgentStatusConfig::default(),
            context: SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
        }
    }

    fn assert_no_named_entry(root: &Path, name: &str) {
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory).expect("walk the physical root") {
                let entry = entry.expect("read physical-root entry");
                assert_ne!(
                    entry.file_name().to_string_lossy(),
                    name,
                    "the old marker is absent from the new physical root"
                );
                if entry
                    .file_type()
                    .expect("inspect physical-root entry")
                    .is_dir()
                {
                    pending.push(entry.path());
                }
            }
        }
    }

    fn git(cwd: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env("GIT_AUTHOR_NAME", "rustX tests")
            .env("GIT_AUTHOR_EMAIL", "rustx-tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "rustX tests")
            .env("GIT_COMMITTER_EMAIL", "rustx-tests@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temporary repository");
        git(dir.path(), &["init"]);
        std::fs::write(dir.path().join("tracked.txt"), "committed\n").expect("tracked file");
        git(dir.path(), &["add", "tracked.txt"]);
        git(dir.path(), &["commit", "-m", "initial"]);
        dir
    }

    /// The parent retains an offered anchor **before** it acknowledges it,
    /// so an acknowledged unit is always already retained. The child's local
    /// `START` gate can therefore never open against an anchor the parent is
    /// not holding.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one coherent offer/ack/refuse/release sequence
    async fn an_offered_anchor_is_retained_before_it_is_acknowledged() {
        let mut harness = stage();
        let unit = ProcessUnitId::new("unit-a");
        write_child_frame(
            &mut harness.child,
            &ChildFrame::AnchorOffered(ProcessUnitAnchorFrame {
                unit_id: unit.clone(),
                pgid: 4242,
            }),
        )
        .await
        .expect("offer");
        write_child_frame(
            &mut harness.child,
            &ChildFrame::Ready(ReadyFrame {
                subagent_id: crate::runtime::identity::SubagentId::new("conv-1-subagent-1"),
            }),
        )
        .await
        .expect("ready");

        // The handshake loop answers the offer and then consumes the Ready
        // frame the child already queued.
        tokio::time::timeout(
            DEADLINE,
            harness.staged.handshake_for_test(
                "conv-1-subagent-1",
                &crate::runtime::cancellation::CancellationSignal::new(),
            ),
        )
        .await
        .expect("handshake liveness")
        .expect("the child answered Ready");
        assert_eq!(
            harness.staged.retained_anchor_count(),
            1,
            "the acknowledged anchor is retained by the staged owner"
        );
        let frame = tokio::time::timeout(DEADLINE, read_parent_frame(&mut harness.child))
            .await
            .expect("ack liveness")
            .expect("ack frame");
        assert_eq!(
            frame,
            Some(ParentFrame::AnchorAccepted(
                crate::runtime::subagent::ipc::ProcessUnitAckFrame {
                    unit_id: unit.clone()
                }
            )),
            "the parent acknowledges exactly the offered unit"
        );

        // A duplicate identity is refused, never silently replacing a
        // retained anchor.
        write_child_frame(
            &mut harness.child,
            &ChildFrame::AnchorOffered(ProcessUnitAnchorFrame {
                unit_id: unit.clone(),
                pgid: 4243,
            }),
        )
        .await
        .expect("duplicate offer");
        write_child_frame(
            &mut harness.child,
            &ChildFrame::Ready(ReadyFrame {
                subagent_id: crate::runtime::identity::SubagentId::new("conv-1-subagent-1"),
            }),
        )
        .await
        .expect("ready");
        tokio::time::timeout(
            DEADLINE,
            harness.staged.handshake_for_test(
                "conv-1-subagent-1",
                &crate::runtime::cancellation::CancellationSignal::new(),
            ),
        )
        .await
        .expect("handshake liveness")
        .expect("the child answered Ready");
        let frame = tokio::time::timeout(DEADLINE, read_parent_frame(&mut harness.child))
            .await
            .expect("refusal liveness")
            .expect("refusal frame");
        assert!(
            matches!(frame, Some(ParentFrame::AnchorRefused(refusal)) if refusal.unit_id == unit),
            "a duplicate unit identity is refused"
        );
        assert_eq!(harness.staged.retained_anchor_count(), 1);

        // Releasing that exact unit removes exactly that anchor.
        write_child_frame(
            &mut harness.child,
            &ChildFrame::AnchorReleased(ProcessUnitAnchorFrame {
                unit_id: unit,
                pgid: 4242,
            }),
        )
        .await
        .expect("release");
        write_child_frame(
            &mut harness.child,
            &ChildFrame::Ready(ReadyFrame {
                subagent_id: crate::runtime::identity::SubagentId::new("conv-1-subagent-1"),
            }),
        )
        .await
        .expect("ready");
        tokio::time::timeout(
            DEADLINE,
            harness.staged.handshake_for_test(
                "conv-1-subagent-1",
                &crate::runtime::cancellation::CancellationSignal::new(),
            ),
        )
        .await
        .expect("handshake liveness")
        .expect("the child answered Ready");
        assert_eq!(
            harness.staged.retained_anchor_count(),
            0,
            "the proven-terminal unit's anchor is dropped"
        );
        harness.staged.rollback().await.expect("rollback");
    }

    /// Rollback is not physically complete until every retained nested
    /// anchor is settled: a staged child that created supervised work must
    /// not leave that work running behind a rolled-back ownership answer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn staged_rollback_contains_every_retained_nested_anchor() {
        let harness = stage();
        let mut staged = harness.staged;
        // A real adopted group: a direct child of this process that leads
        // its own process group, exactly like an orphaned nested unit
        // anchor after the owning child dies.
        let mut nested = tokio::process::Command::new("sleep");
        nested.arg("300");
        nested.process_group(0);
        let nested = nested.spawn().expect("nested group leader");
        let pgid = i32::try_from(nested.id().expect("a live child has a pid")).expect("pid fits");
        staged.retain_for_test(ProcessUnitId::new("unit-a"), pgid);
        assert_eq!(staged.retained_anchor_count(), 1);

        tokio::time::timeout(DEADLINE, staged.rollback())
            .await
            .expect("rollback liveness")
            .expect("rollback must prove containment");
        assert!(
            matches!(
                nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), None),
                Err(nix::errno::Errno::ESRCH)
            ),
            "the retained nested unit group is contained by the rollback"
        );
        drop(nested);
    }

    /// A staged child may already have produced project work before the
    /// durable ownership commit. Rollback preserves that worktree, but its
    /// independent child-private runtime root is still disposable once the
    /// direct child and nested anchors have settled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn staged_workspace_handoff_does_not_leak_the_private_runtime_root() {
        let repository = git_repository();
        let artifacts = tempfile::tempdir().expect("artifact root");
        let manager = SubagentWorkspaceManager::new(repository.path(), artifacts.path());
        let lease = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: true,
                },
                &SubagentId::new("conversation-staged-worktree-subagent-1"),
                &CancellationSignal::new(),
            )
            .await
            .expect("staged worktree");
        let workspace = lease
            .physical_worktree_root()
            .expect("physical worktree")
            .to_path_buf();
        let branch = lease
            .snapshot()
            .git_worktree()
            .expect("Git worktree facts")
            .branch
            .clone();
        std::fs::write(workspace.join("staged-work.txt"), "retain me\n")
            .expect("staged project work");

        let harness = stage();
        let runtime_root = harness.runtime_root.clone();
        let error = tokio::time::timeout(DEADLINE, harness.staged.with_workspace(lease).rollback())
            .await
            .expect("rollback liveness")
            .expect_err("staged project work must prevent a clean rollback");
        assert!(matches!(error, super::RollbackError::Workspace { .. }));
        assert!(workspace.exists(), "the changed worktree is preserved");
        assert!(
            !runtime_root.exists(),
            "the disposable child-private root is removed independently"
        );

        // The test owns the retained worktree and can make it clean before
        // releasing the temporary repository. Production rollback never
        // force-removes this path.
        std::fs::remove_file(workspace.join("staged-work.txt")).expect("test work");
        let workspace_arg = workspace.to_str().expect("workspace path");
        git(
            repository.path(),
            &["worktree", "remove", "--", workspace_arg],
        );
        let reference = format!("refs/heads/{branch}");
        git(repository.path(), &["update-ref", "-d", &reference]);
    }

    /// **Reap is not settlement.** The committed child driver publishes its
    /// physical settlement only after every retained nested anchor is
    /// resolved — so the direct child's exit and reap alone can never make
    /// the registry (and therefore runtime drain) believe the child is
    /// physically settled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_committed_child_settles_its_nested_anchors_before_publishing() {
        let harness = stage();
        let mut staged = harness.staged;
        let mut nested = tokio::process::Command::new("sleep");
        nested.arg("300");
        nested
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        let nested = nested.spawn().expect("nested group leader");
        let pgid = i32::try_from(nested.id().expect("a live child has a pid")).expect("pid fits");
        let unit = ProcessUnitId::new("unit-a");
        staged.retain_for_test(unit.clone(), pgid);

        // The ownership commit: the direct child handle AND the retained
        // anchor set move into the driver task, exactly once.
        let driver = staged.into_driver(
            crate::runtime::subagent::ipc::DelegationFrame {
                task: "inspect".to_owned(),
                context: None,
                interaction_provider_available: false,
            },
            None,
            None,
            None,
        );
        let (_commands, start_gate, task) = driver.split();
        let _ = start_gate.send(None);

        // The child dies without releasing its anchor: close the control
        // channel and let the driver reap it.
        drop(harness.child);
        let settlement = tokio::time::timeout(DEADLINE, task)
            .await
            .expect("the driver must settle")
            .expect("the driver task must not panic");
        assert_eq!(
            settlement.nested.contained,
            vec![unit],
            "the driver contained the retained nested unit before publishing"
        );
        assert!(
            settlement.nested.unproven.is_empty(),
            "an adopted anchor is provably contained on this platform"
        );
        assert!(
            matches!(
                nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), None),
                Err(nix::errno::Errno::ESRCH)
            ),
            "the nested unit group is gone once the settlement is published"
        );
        assert!(
            !harness.runtime_root.exists(),
            "the committed driver, not a separate cleanup owner, removed its exact physical root"
        );
        drop(nested);
    }

    /// Control-channel loss remains the sole liveness authority (Issue
    /// #178): with the observation channel still open and healthy, closing
    /// the control channel settles the drive as a loss — a live observation
    /// transport can never substitute for the reliable control channel.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn control_loss_settles_even_with_a_live_observation_channel() {
        let harness = stage();
        let driver = harness.staged.into_driver(
            crate::runtime::subagent::ipc::DelegationFrame {
                task: "inspect".to_owned(),
                context: None,
                interaction_provider_available: false,
            },
            None,
            None,
            None,
        );
        let (_commands, start_gate, task) = driver.split();
        let _ = start_gate.send(None);

        // The observation channel stays open and healthy...
        let observation_child = harness.observation_child;
        // ...while the control channel closes: the drive settles as a loss.
        drop(harness.child);
        let settlement = tokio::time::timeout(DEADLINE, task)
            .await
            .expect("the driver must settle")
            .expect("the driver task must not panic");
        assert!(
            matches!(settlement.outcome, PhysicalOutcome::Lost { .. }),
            "control EOF with a live observation channel is still a loss: {:?}",
            settlement.outcome
        );
        drop(observation_child);
    }

    /// An unresolved nested anchor is a hard barrier for worktree cleanup:
    /// the lease is preserved without inspecting/removing the workspace, so
    /// a process that may still own it cannot race the handoff decision.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unresolved_nested_anchor_preserves_the_worktree() {
        let repository = git_repository();
        let runtime_root = repository.path().join("runtime");
        let manager = SubagentWorkspaceManager::new(repository.path(), &runtime_root);
        let lease = manager
            .acquire(
                SubagentWorkspacePolicy::GitWorktree {
                    require_clean_parent: true,
                },
                &SubagentId::new("conv-workspace-unresolved-anchor"),
                &CancellationSignal::new(),
            )
            .await
            .expect("worktree lease");
        let workspace_path = lease
            .physical_worktree_root()
            .expect("physical worktree")
            .to_path_buf();
        let child_runtime = repository.path().join("child-runtime");
        std::fs::create_dir_all(&child_runtime).expect("child runtime");
        let mut retained = RetainedProcessUnits::default();
        // No process owns this impossible group id. The containment primitive
        // therefore returns an explicit unproven result without signalling a
        // wildcard or relying on a sleep-based race.
        retained
            .retain(ProcessUnitId::new("unresolved"), i32::MAX)
            .expect("valid anchor shape");

        let settlement = settle_nested(
            PhysicalOutcome::Lost {
                diagnostic: "test outcome".to_owned(),
                cancellation_delivered: false,
            },
            &mut retained,
            PhysicalChildRuntimeRoot::from_existing(child_runtime.clone()),
            Some(lease),
        )
        .await;
        assert_eq!(settlement.nested.unproven.len(), 1);
        assert_eq!(settlement.workspace.cleanup(), WorkspaceCleanup::Preserved);
        assert!(settlement.workspace.handoff().is_none());
        assert!(matches!(
            settlement.workspace.disposition,
            WorkspaceSettlementDisposition::PreservedUnresolved {
                reason: WorkspaceUnresolvedReason::NestedContainment,
                ..
            }
        ));
        assert!(workspace_path.exists(), "unresolved ownership is preserved");
        assert!(
            child_runtime.exists(),
            "the mutable runtime root is preserved too"
        );
    }

    /// The Linux containment prerequisite is established **before** any
    /// child is spawned, so an orphaned nested anchor is adoptable by this
    /// process. A spawn that could not establish it must fail rather than
    /// claim containment authority it does not have.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_containment_prerequisite_precedes_child_staging() {
        // The prerequisite is one-time and sticky per process; consulting it
        // here is exactly what `spawn_staged` does before it launches the
        // child whose root was already reserved by the caller.
        assert_eq!(
            crate::runtime::process_supervision::ensure_child_subreaper(),
            Ok(()),
            "the supported platforms must be able to establish the prerequisite"
        );
        // A spawn whose program does not exist still fails *after* the
        // prerequisite, never before it.
        let dir = tempfile::tempdir().expect("lab");
        let plan = super::SubagentSpawnPlan {
            program: dir.path().join("no-such-rustx"),
            runtime_root: dir.path().join("runtime"),
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            agent_status: crate::context::AgentStatusConfig::default(),
            context: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
        };
        let mut spec = crate::runtime::subagent::ipc::SubagentChildSpec {
            protocol_version: crate::runtime::subagent::ipc::SUBAGENT_IPC_VERSION,
            subagent_id: crate::runtime::identity::SubagentId::new("conv-1-subagent-1"),
            child_conversation_id: crate::runtime::identity::ConversationId::new(
                "conv-1-subagent-1",
            ),
            child_agent_id: crate::runtime::identity::AgentId::new("agent-child"),
            parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
            resolved: crate::runtime::subagent::ResolvedSubagentSpec {
                agent: crate::runtime::subagent::SubagentName::parse("explore").expect("name"),
                definition_digest: serde_json::from_value(serde_json::json!("sha256:frozen"))
                    .expect("digest"),
                execution_deadline: None,
                workspace_policy:
                    crate::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
                instructions: String::new(),
                model: crate::model::frozen::test_frozen_model_spec(
                    serde_json::from_value(serde_json::json!("local/model-a")).expect("model"),
                ),
                tools: Vec::new(),
                skills: Vec::new(),
                project_instructions: Vec::new(),
                materialization:
                    crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
            },
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            agent_status: crate::context::AgentStatusConfig::default(),
            context: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            workspace_snapshot: crate::runtime::subagent::WorkspaceSnapshot::shared(
                dir.path().join("workspace"),
            ),
            runtime_root: dir.path().join("runtime"),
            terminal: crate::runtime::subagent::ipc::ChildTerminalMode::Normal,
        };
        let runtime_root = plan
            .allocate_child_runtime_root(&spec.subagent_id)
            .expect("a physical incarnation root");
        spec.runtime_root = runtime_root.path().to_path_buf();
        assert!(
            matches!(
                spawn_staged(
                    &plan,
                    &spec,
                    runtime_root,
                    crate::runtime::subagent::SubagentWorkspaceManager::new(
                        &spec.workspace_snapshot.logical_workspace,
                        dir.path().join("workspace-artifacts"),
                    )
                    .acquire(
                        crate::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
                        &spec.subagent_id,
                        &crate::runtime::cancellation::CancellationSignal::new(),
                    )
                    .await
                    .expect("shared workspace lease"),
                    &crate::runtime::cancellation::CancellationSignal::new()
                )
                .await,
                Err(super::SpawnError::Spawn { .. })
            ),
            "the prerequisite is consulted first; the spawn itself is what fails here"
        );
    }

    /// A stale physical incarnation is retained as its own namespace; a
    /// later spawn of the same semantic child receives a fresh sibling
    /// namespace rather than deleting and recreating the stale pathname.
    #[test]
    fn a_stale_child_incarnation_never_becomes_the_next_childs_authority() {
        let dir = tempfile::tempdir().expect("lab");
        let runtime_root = dir.path().join("runtime");
        let plan = allocation_plan(runtime_root);
        let semantic_id = SubagentId::new("conv-1-subagent-1");
        let semantic_root = plan
            .runtime_root
            .join("subagents")
            .join(semantic_id.as_str());
        let stale_root = semantic_root.join("incarnation-crashed-earlier");
        let stale_environment = stale_root.join("environments").join("stale");
        std::fs::create_dir_all(&stale_environment).expect("the stale tree");
        std::fs::write(stale_environment.join("pyvenv.cfg"), "stale").expect("the stale artifact");

        let fresh_root = plan
            .allocate_child_runtime_root(&semantic_id)
            .expect("the fresh physical incarnation root");

        assert!(
            stale_root.exists(),
            "the stale incarnation remains available to its original owner"
        );
        assert_eq!(
            std::fs::read_dir(fresh_root.path())
                .expect("the fresh root listing")
                .count(),
            0,
            "the fresh root has no stale mutable state"
        );
        assert_ne!(
            stale_root,
            fresh_root.path(),
            "the semantic grouping path is not itself a mutable child authority"
        );
        fresh_root
            .remove()
            .expect("remove only the fresh incarnation");
    }

    /// A durable store reserves its semantic conversation identity even when
    /// the physical incarnation is gone. Reissuing that identity would make
    /// a later child append to an earlier child's transcript, so allocation
    /// reports a typed collision for the registry to skip.
    #[test]
    fn an_existing_durable_store_blocks_semantic_identity_reuse() {
        let dir = tempfile::tempdir().expect("lab");
        let plan = allocation_plan(dir.path().join("runtime"));
        let subagent_id = SubagentId::new("conv-durable-subagent-1");
        let first_root = plan
            .allocate_child_runtime_root(&subagent_id)
            .expect("first physical incarnation");
        let durable_path = crate::runtime::subagent::child_conversation_store_path(
            &plan.runtime_root,
            &ConversationId::new(subagent_id.as_str()),
        );
        std::fs::write(&durable_path, b"durable child state").expect("durable marker");

        let error = plan
            .allocate_child_runtime_root(&subagent_id)
            .expect_err("the semantic identity is already durable");
        assert!(matches!(
            error,
            super::SpawnError::ConversationIdentityInUse { .. }
        ));
        first_root.remove().expect("remove the first physical root");
    }

    /// A committed child keeps its stable conversation database when the
    /// physical execution incarnation is settled and removed. A later
    /// inspector can therefore reopen the child's durable authorities after
    /// the child process and its runtime-private execution tree are gone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_settled_child_keeps_durable_store_after_physical_cleanup() {
        let dir = tempfile::tempdir().expect("lab");
        let plan = allocation_plan(dir.path().join("runtime"));
        let subagent_id = SubagentId::new("conv-durable-subagent-1");
        let conversation_id = ConversationId::new(subagent_id.as_str());
        let runtime_root = plan
            .allocate_child_runtime_root(&subagent_id)
            .expect("physical child incarnation");
        let physical_path = runtime_root.path().to_path_buf();
        let durable_path = crate::runtime::subagent::child_conversation_store_path(
            &plan.runtime_root,
            &conversation_id,
        );
        assert_eq!(
            durable_path,
            physical_path
                .parent()
                .expect("semantic child grouping")
                .join("conversation.sqlite")
        );
        std::fs::write(&durable_path, b"durable child state").expect("durable marker");

        let mut retained = RetainedProcessUnits::default();
        let settlement = settle_nested(
            PhysicalOutcome::Lost {
                diagnostic: "child settled for inspection test".to_owned(),
                cancellation_delivered: false,
            },
            &mut retained,
            runtime_root,
            None,
        )
        .await;

        assert!(settlement.runtime_root_cleanup_error.is_none());
        assert!(
            !physical_path.exists(),
            "only the physical incarnation is removed"
        );
        assert_eq!(
            std::fs::read(&durable_path).expect("durable store survives"),
            b"durable child state"
        );
    }

    /// A real old process is blocked immediately before its delayed
    /// filesystem create. A later process generation stages the same
    /// semantic identity through the production allocator while the old
    /// process is still alive. The old write then succeeds, but its exact
    /// pathname is a sibling of — never an alias for — the new root.
    #[test]
    fn a_surviving_old_incarnation_can_write_only_to_its_own_root() {
        let dir = tempfile::tempdir().expect("lab");
        let plan = allocation_plan(dir.path().join("runtime"));
        let semantic_id = SubagentId::new("conv-race-subagent-1");
        let old_root = plan
            .allocate_child_runtime_root(&semantic_id)
            .expect("the old physical incarnation root");
        let old_path = old_root.path().to_path_buf();

        let entered_fifo = dir.path().join("old-writer-entered");
        let release_fifo = dir.path().join("old-writer-release");
        let fifo_mode = nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR;
        nix::unistd::mkfifo(&entered_fifo, fifo_mode).expect("the entered rendezvous");
        nix::unistd::mkfifo(&release_fifo, fifo_mode).expect("the release rendezvous");

        let mut old_writer = std::process::Command::new("sh")
            .arg("-c")
            .arg("printf 'entered\\n' > \"$ENTERED\"; IFS= read -r _ < \"$RELEASE\"; printf 'old' > \"$ROOT/old-marker\"")
            .env("ENTERED", &entered_fifo)
            .env("RELEASE", &release_fifo)
            .env("ROOT", &old_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the real old writer process");

        let mut entered = std::fs::File::open(&entered_fifo).expect("open entered rendezvous");
        let mut announcement = String::new();
        entered
            .read_to_string(&mut announcement)
            .expect("read entered rendezvous");
        assert_eq!(announcement, "entered\n");
        assert!(
            old_writer
                .try_wait()
                .expect("probe the old writer")
                .is_none(),
            "the old writer is alive and blocked before its filesystem write"
        );

        // A separate plan value models a later rustX process generation using
        // the same stable runtime root and the same semantic child identity.
        let restarted_plan = plan.clone();
        let new_root = restarted_plan
            .allocate_child_runtime_root(&semantic_id)
            .expect("the new physical incarnation root");
        let new_path = new_root.path().to_path_buf();
        assert_ne!(old_path, new_path);
        std::fs::write(new_path.join("new-marker"), "new").expect("new child mutable state");

        // Opening and writing the release FIFO is the exact synchronization
        // point. No sleep is involved: the old process cannot reach its
        // marker create until this write completes.
        let mut release = std::fs::OpenOptions::new()
            .write(true)
            .open(&release_fifo)
            .expect("open release rendezvous");
        release
            .write_all(b"release\n")
            .expect("release the old writer");
        drop(release);
        let status = old_writer.wait().expect("wait for the old writer");
        assert!(status.success(), "the delayed old write succeeded");

        assert_eq!(
            std::fs::read(old_path.join("old-marker")).expect("the old marker"),
            b"old"
        );
        assert_eq!(
            std::fs::read(new_path.join("new-marker")).expect("the new marker"),
            b"new"
        );
        assert_no_named_entry(&new_path, "old-marker");

        old_root.remove().expect("old owner removes only old root");
        new_root.remove().expect("new owner removes only new root");
    }
}
