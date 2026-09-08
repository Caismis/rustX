//! Fixed Tool input and result contracts; native invocation owns execution.
use super::{
    BTreeMap, Deserialize, EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope, Serialize,
    Utc, Value, WorkflowCatalog, WorkflowNodeInstance, WorkflowProgram, WorkflowRun,
    WorkflowRunError, WorkflowRuntime, bound_workflow_diagnostic, execution, expressions,
    workflow_event_id,
};
use crate::capabilities::selection::ToolSelector;
use crate::runtime::subagent::AttemptSubagentContext;
use crate::tools::executor::{PreflightOutcome, ToolExecutionContext};
use crate::tools::invocation::{ForegroundInvocation, NativeInvocationFact, terminal};
use crate::tools::types::{
    ToolDefinition, ToolExecutionResult, ToolExecutionStatus, ToolInvocationId,
};

pub(super) fn freeze(
    program: &WorkflowProgram,
    context: &AttemptSubagentContext,
) -> Result<BTreeMap<ToolSelector, ToolDefinition>, WorkflowRunError> {
    let resources = context.resources();
    let catalog = resources.capability().available_tools();
    program
        .tools
        .iter()
        .map(|selector| {
            let selected = crate::capabilities::selection::resolve_selector(
                selector,
                catalog,
                resources.capability_availability(),
            )
            .map_err(|error| match error {
                crate::capabilities::selection::ToolSelectionError::SourceUnavailable {
                    ..
                } => WorkflowRunError::SourceUnavailable(error.to_string()),
                crate::capabilities::selection::ToolSelectionError::UnknownCapability {
                    ..
                } => WorkflowRunError::InvalidSelector(error.to_string()),
            })?;
            let definition = selected;
            if definition.execution_policy
                == crate::tools::types::ToolExecutionPolicy::BackgroundOnly
                || catalog
                    .registration(definition)
                    .map_err(WorkflowRunError::IdentityChanged)?
                    .foreground()
                    != crate::tools::deadline::ForegroundPolicy::Leaf
                || !eligible(definition)
            {
                return Err(WorkflowRunError::IneligibleCapability(selector.to_string()));
            }
            Ok((selector.clone(), definition.clone()))
        })
        .collect()
}

fn eligible(definition: &ToolDefinition) -> bool {
    definition.execution_policy != crate::tools::types::ToolExecutionPolicy::BackgroundOnly
        && !definition
            .id
            .as_str()
            .starts_with(super::WORKFLOW_TOOL_ID_PREFIX)
        && !(definition.origin == crate::tools::types::ToolOrigin::Builtin
            && matches!(definition.name.as_str(), "subagent" | "execution" | "todo"))
}

impl WorkflowCatalog {
    /// Validate every selection against the same candidate generation. An
    /// unavailable source cannot hide a later invalid selector.
    pub(crate) fn validate_capabilities(
        &self,
        available: &crate::capabilities::AvailableToolCatalog,
        availability: &crate::capabilities::CapabilityAvailability,
    ) -> Result<(), String> {
        for program in self.definitions().values() {
            for selector in &program.tools {
                match crate::capabilities::selection::resolve_selector(
                    selector,
                    available,
                    availability,
                ) {
                    Ok(selected) => {
                        let definition = selected;
                        if !eligible(definition)
                            || available.registration(definition)?.foreground()
                                != crate::tools::deadline::ForegroundPolicy::Leaf
                        {
                            return Err(format!(
                                "Workflow {} selects ineligible leaf {selector}",
                                program.id()
                            ));
                        }
                    }
                    Err(
                        crate::capabilities::selection::ToolSelectionError::SourceUnavailable {
                            ..
                        },
                    ) => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
        }
        Ok(())
    }
}

impl WorkflowRuntime {
    pub(super) fn tool_workspace_use(
        run: &WorkflowRun,
        context: &AttemptSubagentContext,
        selector: &ToolSelector,
    ) -> Result<crate::tools::executor::WorkspaceUse, WorkflowRunError> {
        let definition = run.tools.get(selector).ok_or_else(|| {
            WorkflowRunError::CapabilityNotAdmitted("capability was not admitted".into())
        })?;
        let registration = context
            .resources()
            .capability()
            .available_tools()
            .registration(definition)
            .map_err(WorkflowRunError::IdentityChanged)?;
        Ok(registration.executor.workspace_use())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // One node admission followed by native invocation and fact projection.
    pub(super) async fn invoke_tool(
        &self,
        run: &WorkflowRun,
        context: &AttemptSubagentContext,
        node: &WorkflowNodeInstance,
        selector: &ToolSelector,
        arguments: expressions::CommittedValue,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
        admitted_access: &mut Option<crate::runtime::workspace::WorkspaceAccess>,
    ) -> Result<
        (
            ToolExecutionResult,
            Option<crate::runtime::workspace::CandidateReference>,
        ),
        WorkflowRunError,
    > {
        let policy = Self::tool_workspace_use(run, context, selector)?;
        let signal = cancellation.child_signal();
        if policy == crate::tools::executor::WorkspaceUse::Independent {
            debug_assert!(admitted_access.is_none());
        } else if run.candidate.is_some()
            && policy == crate::tools::executor::WorkspaceUse::Incompatible
        {
            return Err(WorkflowRunError::IneligibleCapability(
                "executor cannot consume a candidate workspace".into(),
            ));
        } else if admitted_access.is_none()
            && let Some(candidate) = &run.candidate
        {
            *admitted_access = Some(
                candidate
                    .borrow(node.clone(), arguments.candidate.as_ref(), &signal)
                    .await
                    .map_err(|error| {
                        if cancellation.is_cancelled() {
                            WorkflowRunError::from_cancellation(cancellation)
                        } else {
                            WorkflowRunError::InvocationAuthority(error)
                        }
                    })?,
            );
        }
        // Setup errors leave the exact access in the caller's cleanup scope.
        let access = admitted_access;
        let workspace = access
            .as_ref()
            .map(|access| {
                crate::tools::workspace::Workspace::new(&access.snapshot().logical_workspace)
            })
            .transpose()
            .map_err(|error| WorkflowRunError::InvocationAuthority(error.to_string()))?;
        let mut result = self
            .invoke_tool_bound(
                run,
                context,
                node,
                selector,
                arguments.value,
                cancellation,
                workspace.as_ref(),
            )
            .await;
        let mut applicability = None;
        if let Some(access) = access.take() {
            let input = access.input().clone();
            let status = match &result {
                Ok(result) => result.status.clone(),
                Err(error) => error.execution_status(),
            };
            let unchanged = if result.as_ref().is_ok_and(|result| {
                matches!(result.status, ToolExecutionStatus::OutcomeUnknown { .. })
            }) {
                access.unresolved("Tool physical settlement is unknown".into());
                false
            } else {
                match access.finish(true).await {
                    Ok(_) => true,
                    Err(error) => {
                        if result
                            .as_ref()
                            .is_ok_and(|result| result.status == ToolExecutionStatus::Success)
                        {
                            result = Err(WorkflowRunError::InvalidValue(error));
                        }
                        false
                    }
                }
            };
            self.commit_resource(RuntimeEvent::WorkflowCandidateInvocation {
                node: node.clone(),
                input: input.clone(),
                result: status,
                candidate_unchanged: unchanged,
            })?;
            if unchanged {
                applicability = Some(input);
            }
        }
        result.map(|result| (result, applicability))
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn invoke_tool_bound(
        &self,
        run: &WorkflowRun,
        context: &AttemptSubagentContext,
        node: &WorkflowNodeInstance,
        selector: &ToolSelector,
        arguments: Value,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
        workspace: Option<&crate::tools::workspace::Workspace>,
    ) -> Result<ToolExecutionResult, WorkflowRunError> {
        let services = context.native.as_ref().ok_or_else(|| {
            WorkflowRunError::InvocationAuthority(
                "attempt has no native invocation services".into(),
            )
        })?;
        let definition = run.tools.get(selector).ok_or_else(|| {
            WorkflowRunError::CapabilityNotAdmitted(
                "capability was not admitted to this Workflow".into(),
            )
        })?;
        let registration = context
            .resources()
            .capability()
            .available_tools()
            .registration(definition)
            .map_err(WorkflowRunError::IdentityChanged)?;
        let id = ToolInvocationId::Workflow {
            node: Box::new(node.clone()),
        };
        let prepared = match registration
            .prepare_fixed(id.clone(), definition, &arguments)
            .map_err(WorkflowRunError::IdentityChanged)?
        {
            PreflightOutcome::Ready(prepared) => prepared,
            PreflightOutcome::Rejected { error, .. } => {
                return Ok(terminal(ToolExecutionStatus::Failed { error }));
            }
        };
        let emit = |fact| self.emit_native(run, &id, &definition.id, fact);
        emit(NativeInvocationFact::Prepared {
            tool_name: definition.name.clone(),
            arguments_digest: crate::events::interaction::interaction_arguments_digest(
                &prepared.invocation.arguments,
            ),
        });
        let view = crate::agent::PreToolView {
            conversation_id: &run.run_id.conversation_id,
            attempt_id: context.attempt_id(),
            turn: services.turn,
            invocation_id: &id,
            tool_id: &definition.id,
            tool_name: &definition.name,
            origin: &prepared.origin,
            mode: prepared.invocation.mode,
            arguments: &prepared.invocation.arguments,
            audit_arguments: &prepared.invocation.arguments,
            approval_policy: prepared.approval,
        };
        let result = if let Some(result) =
            crate::tools::invocation::authorize(&services.lifecycle, &view, cancellation)
                .await
                .map_err(|e| WorkflowRunError::InvocationAuthority(e.to_string()))?
        {
            result
        } else {
            enum Permit<'a> {
                Read {
                    _guard: tokio::sync::RwLockReadGuard<'a, ()>,
                },
                Write {
                    _guard: tokio::sync::RwLockWriteGuard<'a, ()>,
                },
            }
            let acquire = async {
                match prepared.concurrency {
                    crate::tools::types::ToolConcurrencyPolicy::Parallel => Permit::Read {
                        _guard: services.scheduling.read().await,
                    },
                    crate::tools::types::ToolConcurrencyPolicy::Sequential => Permit::Write {
                        _guard: services.scheduling.write().await,
                    },
                }
            };
            let permit = tokio::select! {
                biased;
                () = cancellation.cancelled() => None,
                permit = acquire => Some(permit),
            };
            if permit.is_none() || cancellation.is_cancelled() {
                terminal(
                    cancellation
                        .native_status(crate::tools::types::ToolCancellationPhase::BeforeStart),
                )
            } else {
                // Execution-start frontier, before construction or first poll.
                emit(NativeInvocationFact::Started);
                let progress = NativeProgress(std::sync::Mutex::new(Vec::new()));
                let native_context = ToolExecutionContext::new(
                    &run.run_id.conversation_id,
                    None,
                    cancellation.clone(),
                    workspace.unwrap_or_else(|| services.runtime.workspace()),
                    &progress,
                    services.runtime.artifacts(),
                    services.runtime.tool_output(),
                    context.resources().capability().effective_environment(),
                );
                let native_context = match services.lifecycle.native_questionnaire_requester(
                    context.attempt_id().clone(),
                    cancellation.clone(),
                    services.turn,
                ) {
                    Some(requester) => native_context.with_questionnaire_requester(requester),
                    None => native_context,
                };
                let driver = ForegroundInvocation {
                    clock: &*services.clock,
                    policy: services.leaf_policy,
                    registration: registration.foreground(),
                    #[cfg(test)]
                    deadline_armed: None,
                    #[cfg(test)]
                    cancellation_won: None,
                    #[cfg(test)]
                    completion_won: None,
                };
                let executor = registration.executor.clone();
                let (result, facts) = driver
                    .execute(
                        &*executor,
                        prepared.invocation,
                        prepared.progress_capability,
                        native_context,
                    )
                    .await;
                for progress in progress.0.into_inner().expect("progress buffer") {
                    emit(NativeInvocationFact::Progress { progress });
                }
                for fact in facts {
                    emit(NativeInvocationFact::Lifecycle { fact });
                }
                result
            }
        };
        emit(NativeInvocationFact::Completed {
            status: bounded_status(&result.status),
        });
        Ok(result)
    }

    fn emit_native(
        &self,
        run: &WorkflowRun,
        id: &ToolInvocationId,
        tool_id: &crate::runtime::identity::ToolId,
        fact: NativeInvocationFact,
    ) {
        let event = RuntimeEvent::NativeToolInvocation {
            invocation_id: id.clone(),
            tool_id: tool_id.clone(),
            fact,
        };
        let event_id = workflow_event_id(&event);
        #[cfg(test)]
        self.observations
            .send_modify(|events| events.push(event.clone()));
        let _ = self.event_store.append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id,
            sequence: 0,
            conversation_id: run.run_id.conversation_id.clone(),
            attempt_id: Some(run.run_id.attempt_id.clone()),
            turn_id: None,
            timestamp: Utc::now(),
            event,
        });
    }
}

struct NativeProgress(std::sync::Mutex<Vec<crate::tools::types::ToolProgress>>);
impl crate::tools::executor::ProgressReporter for NativeProgress {
    fn report(&self, progress: crate::tools::types::ToolProgress) {
        let mut values = self.0.lock().expect("progress buffer");
        if values.len() < crate::tools::limits::MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL {
            values.push(progress);
        } else {
            *values.last_mut().expect("positive bound") = progress;
        }
    }
}

pub(super) const fn default_workflow_timeout_ms() -> u64 {
    600_000
}

fn bounded_status(status: &ToolExecutionStatus) -> ToolExecutionStatus {
    match status {
        ToolExecutionStatus::Failed { error } => ToolExecutionStatus::Failed {
            error: bound_workflow_diagnostic(error.clone()),
        },
        ToolExecutionStatus::Denied { reason } => ToolExecutionStatus::Denied {
            reason: bound_workflow_diagnostic(reason.clone()),
        },
        ToolExecutionStatus::OutcomeUnknown { detail } => ToolExecutionStatus::OutcomeUnknown {
            detail: bound_workflow_diagnostic(detail.clone()),
        },
        status => status.clone(),
    }
}

/// Select exactly one native content part, never provider text or log heuristics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowToolResult {
    /// The indexed part must be native structured JSON satisfying this schema.
    Json { part: usize, schema: Value },
    /// The indexed part must be native text; retain it as a typed JSON string.
    Text { part: usize },
}

impl WorkflowToolResult {
    pub(super) fn schema(&self) -> Value {
        match self {
            Self::Json { schema, .. } => schema.clone(),
            Self::Text { .. } => serde_json::json!({"type":"string"}),
        }
    }

    pub(super) fn project(
        &self,
        result: &crate::tools::types::ToolExecutionResult,
        node: &WorkflowNodeInstance,
    ) -> Result<Value, WorkflowRunError> {
        use crate::tools::types::{ToolExecutionStatus, ToolResultContent};
        if result.status != ToolExecutionStatus::Success {
            return Err(WorkflowRunError::ToolFailed {
                node: node.to_string(),
                status: bounded_status(&result.status),
            });
        }
        let value = match self {
            Self::Json { part, .. } => match result.content.get(*part) {
                Some(ToolResultContent::Json { value }) => {
                    expressions::bounded_value_bytes(value)?;
                    value.clone()
                }
                _ => {
                    return Err(WorkflowRunError::InvalidValue(
                        "selected result part is not structured JSON".into(),
                    ));
                }
            },
            Self::Text { part } => match result.content.get(*part) {
                Some(ToolResultContent::Text(text)) => {
                    if text.text.len() > super::MAX_VALUE_BYTES {
                        return Err(WorkflowRunError::InvalidValue(
                            "selected text exceeds Workflow value bound".into(),
                        ));
                    }
                    Value::String(text.text.clone())
                }
                _ => {
                    return Err(WorkflowRunError::InvalidValue(
                        "selected result part is not text".into(),
                    ));
                }
            },
        };
        expressions::bounded_value_bytes(&value)?;
        execution::validate_commit(&self.schema(), &value)?;
        Ok(value)
    }
}
