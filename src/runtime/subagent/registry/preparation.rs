//! The private, fallible first stage of a start: input validation, identity
//! allocation, process staging and runtime activation, with no published
//! conversation state. [`SubagentRegistry::commit`] is the only linearization
//! point that can make a prepared child visible.

use super::super::MAX_CONTEXT_PACKAGE_BYTES;
use super::super::MAX_TASK_BYTES;
use super::super::activity::SubagentExecutionProfile;
use super::super::catalog::NamedAgentDefinitionDigest;
use super::super::catalog::SubagentExecutionDeadline;
use super::super::catalog::SubagentName;
use super::super::process::StagedChild;
use super::super::resolver::SubagentExecutionProfileDigest;
use super::SubagentRegistry;
use super::SubagentTerminalMode;
use super::agents;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::AgentId;
use crate::runtime::identity::ConversationId;
use crate::runtime::identity::SubagentId;
use crate::runtime::identity::ToolCallId;
use std::sync::Arc;
use std::sync::PoisonError;

// Keep the frozen spelling as authority; canonicalization only proves that
// the binding satisfies the native Tool workspace contract right now.
pub(super) fn validate_execution_workspace(
    workspace: &std::path::Path,
) -> Result<(), SubagentStartError> {
    crate::tools::Workspace::new(workspace)
        .map(|_| ())
        .map_err(|error| SubagentStartError::Workspace {
            detail: error.to_string(),
        })
}

/// The inputs of one subagent start.
///
/// `resolved` is already the complete frozen outcome of resolving one named
/// definition against the invoking attempt's runtime resource generation.
/// The registry never resolves configuration itself: it owns live child
/// lifecycle only.
#[derive(Debug, Clone)]
pub struct SubagentStartSpec {
    pub authority: agents::DurableAgentAuthority,
    pub admission: ActivationAdmission,
}

/// Input and provenance of exactly one finite activation, never Agent authority.
#[derive(Debug, Clone)]
pub struct ActivationAdmission {
    pub attachments: Vec<crate::message::content::UploadedFileRef>,
    pub task: String,
    pub context: Option<String>,
    pub origin: AgentActivationOrigin,
    pub terminal: SubagentTerminalMode,
}

/// Truthful admission source. Client controls have no model `ToolCall` identity.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentActivationOrigin {
    CreationTool {
        tool_call_id: ToolCallId,
    },
    MessageTool {
        tool_call_id: ToolCallId,
    },
    ClientControl,
    Workflow {
        node_id: Box<crate::runtime::workflow::WorkflowNodeInstance>,
    },
}

impl AgentActivationOrigin {
    #[must_use]
    pub fn tool_call_id(&self) -> Option<&ToolCallId> {
        match self {
            Self::CreationTool { tool_call_id } | Self::MessageTool { tool_call_id } => {
                Some(tool_call_id)
            }
            Self::ClientControl | Self::Workflow { .. } => None,
        }
    }
}

/// A privately prepared subagent start: everything fallible already
/// succeeded, but nothing is published or owned yet.
#[derive(Debug)]
pub struct PreparedSubagent {
    pub(super) authority: agents::DurableAgentAuthority,
    /// The Session execution binding this child runs in, frozen before the
    /// ownership commit and retained by a durable Agent for every activation.
    pub(super) workspace: std::path::PathBuf,
    pub(super) subagent_id: SubagentId,
    pub(super) child_agent_id: AgentId,
    pub(super) child_conversation_id: ConversationId,
    pub(super) origin: AgentActivationOrigin,
    pub(super) agent: SubagentName,
    pub(super) definition_digest: NamedAgentDefinitionDigest,
    pub(super) profile_digest: SubagentExecutionProfileDigest,
    pub(super) terminal: SubagentTerminalMode,
    pub(super) task: String,
    pub(super) context: Option<String>,
    pub(super) attachments: Vec<crate::message::content::UploadedFileRef>,
    /// The definition-level deadline frozen before preparation. It is only
    /// scheduled after durable ownership commits.
    pub(super) execution_deadline: Option<SubagentExecutionDeadline>,
    /// The redacted execution profile derived from the frozen model
    /// authority at preparation time (Issue #178).
    pub(super) profile: SubagentExecutionProfile,
    pub(super) staged: StagedChild,
}

/// The outcome of a successful ownership commit.
///
/// This is a **runtime acceptance value**: it carries the runtime facts a
/// caller may legitimately need (the owned identity and its committed
/// provenance), never a model-facing tool result. The model-facing
/// `subagent` creation projection is built by the `subagent` intrinsic alone
/// (Issue #192).
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentAccepted {
    /// The conversation-owned subagent identity.
    pub subagent_id: SubagentId,
    /// The child agent identity.
    pub child_agent_id: AgentId,
    /// The child conversation identity.
    pub child_conversation_id: ConversationId,
    /// The canonical named-agent identity.
    pub agent: String,
    /// The deterministic definition digest frozen at start.
    pub definition_digest: String,
}

/// The outcome of one [`SubagentRegistry::commit`].
#[derive(Debug)]
pub enum SubagentStartOutcome {
    /// Ownership committed; the child is running behind the start gate
    /// release.
    Accepted(SubagentAccepted),
    /// The attempt cancellation won the race against the commit; the
    /// staged child was torn down and nothing was published.
    RolledBack,
}

/// A typed start failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentStartError {
    /// The owning conversation is draining or draining-complete.
    ConversationInactive,
    /// The task input is empty or its text exceeds [`MAX_TASK_BYTES`].
    InvalidTask {
        /// The offending byte length.
        bytes: usize,
    },
    /// The explicit context package exceeds [`MAX_CONTEXT_PACKAGE_BYTES`].
    ContextOversized {
        /// The offending byte length.
        bytes: usize,
    },
    /// The per-conversation concurrency bound is reached.
    CapacityExceeded {
        /// The configured bound.
        max: usize,
    },
    /// Staging the child process failed.
    Spawn {
        /// The failure detail.
        detail: String,
    },
    /// The frozen Session execution binding is missing or not a usable
    /// directory. There is no fallback directory and none is created. This
    /// is a failure of this child start, not a global runtime-health fault.
    Workspace {
        /// The bounded workspace diagnostic.
        detail: String,
    },
    /// The durable ownership commit failed.
    Durability {
        /// The failure detail.
        detail: String,
    },
    /// The owning conversation runtime's durable authority is in the
    /// explicit `DurabilityFailed` state (Issue #63): no new
    /// conversation-owned durable semantic ownership commit may begin until
    /// the runtime is reconstructed. The staged child is torn down
    /// conclusively and no ownership fact, record, or Delegate exists.
    DurabilityFailed {
        /// The owning runtime's bounded failure diagnostic.
        detail: String,
    },
    /// The ownership decision could not return while rollback was proven
    /// complete.
    Rollback {
        /// The failure detail.
        detail: String,
    },
    /// The invoking attempt's cancellation became observable before the
    /// durable ownership commit (Issue #145): the child never reached a
    /// startable `Ready` *as a committed start*, no ownership record,
    /// event, or capacity consumption survives, and every staged physical
    /// resource settled before this was returned.
    Cancelled,
}

impl core::fmt::Display for SubagentStartError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ConversationInactive => {
                write!(f, "the owning conversation is no longer active")
            }
            Self::InvalidTask { bytes } => write!(
                f,
                "the task input must contain text or attachments and at most {MAX_TASK_BYTES} text bytes ({bytes} bytes given)"
            ),
            Self::ContextOversized { bytes } => write!(
                f,
                "the context package exceeds the {MAX_CONTEXT_PACKAGE_BYTES}-byte bound \
                 ({bytes} bytes)"
            ),
            Self::CapacityExceeded { max } => {
                write!(f, "the per-conversation subagent bound ({max}) is reached")
            }
            Self::Spawn { detail } => write!(f, "could not start the child runtime: {detail}"),
            Self::Workspace { detail } => {
                write!(f, "the child execution workspace is unavailable: {detail}")
            }
            Self::Durability { detail } => {
                write!(f, "the durable ownership commit failed: {detail}")
            }
            Self::DurabilityFailed { detail } => write!(
                f,
                "the conversation runtime's durable authority has failed; no new subagent ownership may begin: {detail}"
            ),
            Self::Rollback { detail } => {
                write!(f, "the child rollback was not proven complete: {detail}")
            }
            Self::Cancelled => write!(
                f,
                "the invoking attempt was cancelled before the child was owned"
            ),
        }
    }
}

impl std::error::Error for SubagentStartError {}

impl SubagentStartError {
    /// Exhaustive typed accounting: every error except explicit rollback
    /// uncertainty is returned only after its resource owner proved teardown,
    /// or before that owner could acquire any physical resource.
    pub(crate) fn rollback_is_proven(&self) -> bool {
        match self {
            Self::Rollback { .. } => false,
            Self::ConversationInactive
            | Self::InvalidTask { .. }
            | Self::ContextOversized { .. }
            | Self::CapacityExceeded { .. }
            | Self::Spawn { .. }
            | Self::Workspace { .. }
            | Self::Durability { .. }
            | Self::DurabilityFailed { .. }
            | Self::Cancelled => true,
        }
    }
}

impl From<super::super::process::SpawnError> for SubagentStartError {
    fn from(error: super::super::process::SpawnError) -> Self {
        use super::super::process::SpawnError;
        match error {
            SpawnError::Cancelled => Self::Cancelled,
            SpawnError::Rollback { detail } => Self::Rollback { detail },
            error @ (SpawnError::ContainmentPrerequisite { .. }
            | SpawnError::WorkspaceSetup { .. }
            | SpawnError::ConversationIdentityInUse { .. }
            | SpawnError::Spawn { .. }
            | SpawnError::Handshake { .. }) => Self::Spawn {
                detail: error.to_string(),
            },
        }
    }
}

impl SubagentRegistry {
    /// **Prepare.** Runs every fallible stage privately: input validation,
    /// identity allocation, process spawn, and the activation handshake.
    /// No semantic ownership is published and no capacity is committed.
    /// Positive physical allocations retain their recovery evidence and consume
    /// their identities even when preparation fails.
    ///
    /// `preparation_cancellation` is the invoking attempt's cancellation
    /// authority (Issue #145): it owns the *whole* pre-commit lifecycle,
    /// not merely the commit decision. If it becomes observable while the
    /// child is still staging, the child never reaches a startable `Ready`,
    /// every staged physical resource settles, and this returns
    /// [`SubagentStartError::Cancelled`].
    ///
    /// # Errors
    ///
    /// Returns the typed [`SubagentStartError`] of the first failing stage,
    /// or [`SubagentStartError::Cancelled`] when the attempt cancellation
    /// won before the ownership commit.
    #[allow(clippy::too_many_lines)] // one ordered staged-start pipeline
    pub async fn prepare(
        &self,
        spec: &SubagentStartSpec,
        preparation_cancellation: &CancellationSignal,
    ) -> Result<PreparedSubagent, SubagentStartError> {
        self.prepare_inner(spec, preparation_cancellation, None)
            .await
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn prepare_inner(
        &self,
        spec: &SubagentStartSpec,
        preparation_cancellation: &CancellationSignal,
        resume: Option<&agents::ResumeIdentity>,
    ) -> Result<PreparedSubagent, SubagentStartError> {
        if preparation_cancellation.is_cancelled() {
            return Err(SubagentStartError::Cancelled);
        }
        if !Self::valid_input(&spec.admission.task, spec.admission.attachments.len()) {
            return Err(SubagentStartError::InvalidTask {
                bytes: spec.admission.task.len(),
            });
        }
        if let Some(context) = &spec.admission.context {
            let bytes = context.len();
            if bytes > MAX_CONTEXT_PACKAGE_BYTES {
                return Err(SubagentStartError::ContextOversized { bytes });
            }
        }
        #[cfg(test)]
        self.state
            .lock()
            .unwrap()
            .prepared_policies
            .push(spec.authority.execution_policy);
        if self.config.mailbox.begin_running_admission().is_err() {
            return Err(SubagentStartError::ConversationInactive);
        }
        // The redacted observation-plane profile derives from the frozen
        // model authority exactly once, at preparation time.
        let profile = SubagentExecutionProfile::from_frozen(&spec.authority.resolved.model);
        // A new child receives the Session binding; a resumed Agent keeps the
        // binding it froze at creation. Neither is ever replaced, re-resolved
        // or substituted with a fallback CWD, and nothing is created here.
        let workspace = resume.map_or_else(
            || self.config.workspace.clone(),
            |resume| resume.workspace.clone(),
        );
        validate_execution_workspace(&workspace)?;
        // Physical-root allocation is staged child ownership. A pre-commit
        // crash can leave a durable store for an identity that was never
        // published; skip that identity rather than ever allowing a new child
        // to append to its history.
        let mut identity_attempts = 0;
        let (subagent_id, child_conversation_id, child_agent_id, runtime_root) = loop {
            if preparation_cancellation.is_cancelled() {
                return Err(SubagentStartError::Cancelled);
            }
            if identity_attempts == 16 {
                return Err(SubagentStartError::Spawn {
                    detail: "Conversation identity reservation exhausted".into(),
                });
            }
            identity_attempts += 1;
            let subagent_id = if let Some(resume) = resume {
                resume.activation_id.clone()
            } else {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                let ordinal = state.next_ordinal;
                state.next_ordinal += 1;
                SubagentId::for_conversation(&self.config.conversation_id, ordinal)
            };
            let child_conversation_id = if let Some(resume) = resume {
                resume.conversation_id.clone()
            } else {
                ConversationId::from_uuid(self.identities.next_uuid())
                    .map_err(|detail| SubagentStartError::Spawn { detail })?
            };
            if resume.is_none()
                && (child_conversation_id == self.config.conversation_id
                    || self
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .records
                        .iter()
                        .any(|record| record.child_conversation_id == child_conversation_id))
            {
                continue;
            }
            let child_agent_id = resume.map_or_else(
                || AgentId::new(format!("agent-{subagent_id}")),
                |resume| resume.agent_id.clone(),
            );
            #[cfg(test)]
            let override_child = self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .staged_overrides
                .pop_front();
            #[cfg(test)]
            let overridden = override_child.is_some();
            #[cfg(not(test))]
            let overridden = false;
            #[cfg(test)]
            let allocation_stage = self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .allocation_test_hook
                .take()
                .map(|hook| {
                    hook.entered.send(()).unwrap();
                    hook.stage
                });
            // Allocate recoverable physical authority before any child exists.
            let runtime_root = if overridden {
                None
            } else {
                #[cfg(test)]
                let injected_failure = {
                    let mut state = self.state.lock().unwrap();
                    state.allocation_attempts += 1;
                    state.allocation_failure.take()
                };
                let allocate = async {
                    if resume.is_some() {
                        self.config
                            .spawn
                            .allocate_activation_runtime_root(
                                &child_conversation_id,
                                preparation_cancellation,
                            )
                            .await
                    } else {
                        self.config
                            .spawn
                            .allocate_child_runtime_root(
                                &child_conversation_id,
                                preparation_cancellation,
                            )
                            .await
                    }
                };
                #[cfg(test)]
                let allocation = match injected_failure {
                    Some(error) => {
                        drop(allocate);
                        Err(error)
                    }
                    None => allocate.await,
                };
                #[cfg(not(test))]
                let allocation = allocate.await;
                let mut runtime_root = match allocation {
                    Ok(root) => root,
                    Err(super::super::process::SpawnError::ConversationIdentityInUse {
                        ..
                    }) if resume.is_none() => {
                        continue;
                    }
                    Err(super::super::process::SpawnError::ConversationIdentityInUse {
                        ..
                    }) if resume.is_some() => {
                        return Err(SubagentStartError::Spawn {
                            detail:
                                "reserved activation identity conflicts with existing allocation"
                                    .into(),
                        });
                    }
                    Err(error) => return Err(SubagentStartError::from(error)),
                };
                let physical_owner = if let Some(resume) = resume {
                    resume
                        .physical_owner
                        .as_ref()
                        .expect("Reserved physical authority")
                        .clone()
                } else {
                    Arc::new(self.finish_physical_reservation(
                        &child_conversation_id,
                        &subagent_id,
                        super::super::physical_recovery::ParentPhysicalLease::reserve(
                            &self.config.spawn.product_root,
                            &self.config.spawn.session_id,
                            &child_conversation_id,
                            &subagent_id,
                        ),
                    )?)
                };
                runtime_root.install_physical_owner(physical_owner);
                Some(runtime_root)
            };
            #[cfg(test)]
            {
                if let Some(staged) = override_child {
                    return Ok(PreparedSubagent {
                        authority: spec.authority.clone(),
                        workspace,
                        subagent_id,
                        child_agent_id,
                        child_conversation_id,
                        origin: spec.admission.origin.clone(),
                        agent: spec.authority.resolved.agent.clone(),
                        definition_digest: spec.authority.resolved.definition_digest.clone(),
                        profile_digest: spec.authority.resolved.profile_digest(),
                        terminal: spec.admission.terminal.clone(),
                        attachments: spec.admission.attachments.clone(),
                        task: spec.admission.task.clone(),
                        context: spec.admission.context.clone(),
                        execution_deadline: spec.authority.resolved.execution_deadline,
                        profile,
                        staged,
                    });
                }
            }
            let runtime_root = runtime_root.expect("production preparation allocated its root");
            #[cfg(test)]
            if let Some(stage) = allocation_stage {
                return Ok(PreparedSubagent {
                    authority: spec.authority.clone(),
                    workspace,
                    subagent_id,
                    child_agent_id,
                    child_conversation_id,
                    origin: spec.admission.origin.clone(),
                    agent: spec.authority.resolved.agent.clone(),
                    definition_digest: spec.authority.resolved.definition_digest.clone(),
                    profile_digest: spec.authority.resolved.profile_digest(),
                    terminal: spec.admission.terminal.clone(),
                    attachments: spec.admission.attachments.clone(),
                    task: spec.admission.task.clone(),
                    context: spec.admission.context.clone(),
                    execution_deadline: spec.authority.resolved.execution_deadline,
                    profile,
                    staged: stage(runtime_root),
                });
            }
            break (
                subagent_id,
                child_conversation_id,
                child_agent_id,
                runtime_root,
            );
        };
        let child_spec = self.config.spawn.child_spec(
            &subagent_id,
            &child_conversation_id,
            &child_agent_id,
            &self.config.agent_id,
            &spec.authority.resolved,
            spec.authority.approval_mode,
            spec.authority.execution_policy,
            &runtime_root,
            &workspace,
            &spec.admission.terminal,
        );
        let staged = match super::super::process::spawn_staged(
            &self.config.spawn,
            &child_spec,
            runtime_root,
            preparation_cancellation,
        )
        .await
        {
            Ok(staged) => staged,
            Err(error) => {
                let start_error = SubagentStartError::from(error);
                return Err(start_error);
            }
        };
        crate::runtime::process_death::reach("after:subagent_staged");
        Ok(PreparedSubagent {
            authority: spec.authority.clone(),
            workspace,
            subagent_id,
            child_agent_id,
            child_conversation_id,
            origin: spec.admission.origin.clone(),
            agent: spec.authority.resolved.agent.clone(),
            definition_digest: spec.authority.resolved.definition_digest.clone(),
            profile_digest: spec.authority.resolved.profile_digest(),
            terminal: spec.admission.terminal.clone(),
            attachments: spec.admission.attachments.clone(),
            task: spec.admission.task.clone(),
            context: spec.admission.context.clone(),
            execution_deadline: spec.authority.resolved.execution_deadline,
            profile,
            staged,
        })
    }

    /// Installs a pre-staged child `prepare` consumes instead of spawning
    /// (tests only).
    #[cfg(test)]
    pub(crate) fn prepared_execution_policies(
        &self,
    ) -> Vec<super::super::InheritedExecutionPolicy> {
        self.state.lock().unwrap().prepared_policies.clone()
    }

    #[cfg(test)]
    pub(crate) fn push_staged_override(&self, staged: StagedChild) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.staged_overrides.push_back(staged);
    }
}
