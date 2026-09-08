//! Workflow's logical binding to native source ownership. No Git operations.
use super::{
    EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope, Utc, Value, WorkflowBlockProgram,
    WorkflowNodeProgram, WorkflowRun, WorkflowRunError, WorkflowRuntime, workflow_event_id,
};
use crate::runtime::workspace::{
    CandidateScope, WorkspaceOwner, WorkspacePolicy, WorkspaceSettlement,
};

pub(super) const fn strict_parent() -> bool {
    true
}

impl WorkflowRuntime {
    pub(super) async fn prepare_workspace(
        &self,
        run: &WorkflowRun,
        context: &crate::runtime::subagent::AttemptSubagentContext,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Option<CandidateScope>, WorkflowRunError> {
        let Some(binding) = run.program.workspace else {
            return Ok(None);
        };
        let policy = WorkspacePolicy::GitWorktree {
            require_clean_parent: binding.require_clean_parent,
        };
        validate_profiles(&run.program.block, context, policy)?;
        for definition in run.tools.values() {
            let registration = context
                .resources()
                .capability()
                .available_tools()
                .registration(definition)
                .map_err(WorkflowRunError::IdentityChanged)?;
            if registration.executor.workspace_use()
                == crate::tools::executor::WorkspaceUse::Incompatible
            {
                return Err(WorkflowRunError::IneligibleCapability(format!(
                    "{} cannot consume a candidate workspace",
                    definition.name
                )));
            }
        }
        if cancellation.is_cancelled() {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        }
        let signal = cancellation.child_signal();
        let lease = self
            .subagents
            .workspace_manager()
            .acquire(
                policy,
                &WorkspaceOwner::Workflow(run.run_id.clone()),
                &signal,
            )
            .await
            .map_err(|error| {
                if cancellation.is_cancelled() {
                    WorkflowRunError::from_cancellation(cancellation)
                } else {
                    WorkflowRunError::InvocationAuthority(error.to_string())
                }
            })?;
        let snapshot = lease.snapshot().clone();
        let candidate = match lease.retain_for_run(run.run_id.clone()).await {
            Ok(candidate) => candidate,
            Err((lease, detail)) => {
                let workspace = (*lease)
                    .settle_staged()
                    .await
                    .unwrap_or_else(|error| *error.settlement);
                return Err(WorkflowRunError::WorkspaceSettlement {
                    candidate: None,
                    error: Box::new(WorkflowRunError::InvocationAuthority(detail)),
                    workspace: Box::new(workspace),
                });
            }
        };
        let committed = if cancellation.is_cancelled() {
            Err(WorkflowRunError::from_cancellation(cancellation))
        } else {
            self.commit_resource(RuntimeEvent::WorkflowWorkspaceOwned {
                run_id: run.run_id.clone(),
                workspace: snapshot,
            })
        };
        if let Err(error) = committed {
            let workspace = candidate.settle().await;
            return Err(WorkflowRunError::WorkspaceSettlement {
                candidate: None,
                error: Box::new(error),
                workspace: Box::new(workspace),
            });
        }
        Ok(Some(candidate))
    }

    pub(super) fn commit_resource(&self, event: RuntimeEvent) -> Result<(), WorkflowRunError> {
        self.event_store
            .append_event(RuntimeEventEnvelope {
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: workflow_event_id(&event),
                sequence: 0,
                conversation_id: self.event_store.conversation_id().clone(),
                attempt_id: None,
                turn_id: None,
                timestamp: Utc::now(),
                event,
            })
            .map(|_| ())
            .map_err(|error| {
                WorkflowRunError::InvocationAuthority(format!(
                    "workspace durable publication failed: {error}"
                ))
            })
    }

    pub(super) fn commit_workspace_settlement(
        &self,
        run: &WorkflowRun,
        workspace: Option<&WorkspaceSettlement>,
        candidate: Option<&crate::runtime::workspace::CandidateReference>,
        recovery_guard: Option<&crate::runtime::workspace::CandidateRecoveryGuard>,
        execution: Result<Value, WorkflowRunError>,
    ) -> Result<Value, WorkflowRunError> {
        if let Some(workspace) = workspace {
            self.commit_resource(RuntimeEvent::WorkflowWorkspaceSettled {
                candidate: candidate.cloned(),
                recovery_guard: recovery_guard.cloned().map(Box::new),
                run_id: run.run_id.clone(),
                workspace: workspace.clone(),
            })?;
            if workspace.unresolved_reason().is_some() && execution.is_ok() {
                return Err(WorkflowRunError::InvocationAuthority(
                    "candidate physical settlement is unresolved".into(),
                ));
            }
        }
        execution
    }
}

fn validate_profiles(
    block: &WorkflowBlockProgram,
    context: &crate::runtime::subagent::AttemptSubagentContext,
    policy: WorkspacePolicy,
) -> Result<(), WorkflowRunError> {
    for node in block.nodes.values() {
        match node {
            WorkflowNodeProgram::Agent(agent) => {
                let resolved = context
                    .resolve_workflow(&agent.profile)
                    .map_err(|error| WorkflowRunError::InvocationAuthority(error.to_string()))?;
                if resolved.workspace_policy != policy {
                    return Err(WorkflowRunError::InvalidProgram(format!(
                        "profile {} workspace policy conflicts with the run candidate",
                        agent.profile
                    )));
                }
                for tool in &resolved.tools {
                    // Reconstructed native implementations consume child cwd.
                    // MCP materializations have server-owned cwd and cannot be
                    // silently rebound. Nested orchestration is not admitted.
                    if tool.definition().origin != crate::tools::types::ToolOrigin::Builtin
                        || matches!(tool.name(), "subagent" | "execution")
                    {
                        return Err(WorkflowRunError::IneligibleCapability(format!(
                            "{} cannot honor candidate child authority",
                            tool.name()
                        )));
                    }
                }
            }
            WorkflowNodeProgram::Parallel { branches, .. } => {
                for branch in branches.values() {
                    validate_profiles(&branch.block, context, policy)?;
                }
            }
            WorkflowNodeProgram::Loop { body, .. } => validate_profiles(body, context, policy)?,
            _ => {}
        }
    }
    Ok(())
}
