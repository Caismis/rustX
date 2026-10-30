//! Fixed Tool input and result contracts; native invocation owns execution.
use super::{
    Deserialize, EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope, Serialize, Utc, Value,
    WorkflowNodeInstance, WorkflowRun, WorkflowRunError, WorkflowRuntime,
    bound_workflow_diagnostic, execution, expressions, workflow_event_id,
};
use crate::capabilities::selection::ExactToolSelector;
use crate::runtime::subagent::NativeExecutionContext;
use crate::tools::executor::{PreflightOutcome, ToolExecutionContext};
use crate::tools::invocation::{ForegroundInvocation, NativeInvocationFact, terminal};
use crate::tools::types::{
    ToolDefinition, ToolExecutionResult, ToolExecutionStatus, ToolInvocationId,
};

pub(super) fn eligible(definition: &ToolDefinition) -> bool {
    definition.execution_policy != crate::tools::types::ToolExecutionPolicy::BackgroundOnly
        && !definition
            .id
            .as_str()
            .starts_with(super::WORKFLOW_TOOL_ID_PREFIX)
        // `todo` is deliberately absent from this list. It is no longer an
        // ordinary Builtin capability at all, so it never appears in the
        // available catalog a Workflow selector resolves against and is
        // refused one step earlier, as an unknown ordinary identity
        // (Issue #259). Naming it here too would imply it could otherwise
        // have been selected.
        && !(definition.origin == crate::tools::types::ToolOrigin::Builtin
            && (crate::tools::native::is_agent_control(&definition.name)
                || crate::tools::native::is_domain_control(&definition.name)))
}

/// One builtin Tool an author can choose for a Tool node: its name and the
/// tool-owned description and argument schema an authoring form is built from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowToolChoice {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// The fixed result a node of this Tool commits.
    pub result: WorkflowToolResult,
}

/// The builtin Tools a Workflow Tool node can select at all, in the native
/// order. Authoring presents these as choices; admission against the
/// adopted generation remains the only authority.
#[must_use]
pub fn selectable_builtin_tools() -> Vec<WorkflowToolChoice> {
    crate::tools::native::definitions(
        crate::tools::native::NativeToolPolicies::default(),
        &crate::runtime::subagent::AgentCatalog::default(),
    )
    .into_iter()
    .filter(|(definition, policy)| {
        definition.origin == crate::tools::types::ToolOrigin::Builtin
            && *policy == crate::tools::deadline::ForegroundPolicy::Leaf
            // A Workflow asks a person through its Review node.
            && definition.name != "ask_user"
            && eligible(definition)
    })
    .map(|(definition, _)| WorkflowToolChoice {
        result: WorkflowToolResult::declared(
            crate::tools::native::output_schema(&definition.name).as_ref(),
        ),
        name: definition.name,
        description: definition.description,
        input_schema: definition.input_schema,
    })
    .collect()
}

impl WorkflowRuntime {
    /// One exact admitted registration through the standard native
    /// invocation path and the invoking Session's execution binding.
    /// External Tools keep their own admitted materialization contract.
    /// `retry` numbers this call of the visit; the flag says whether the
    /// call's physical settlement is proven, as only then may it be handled.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub(super) async fn invoke_tool(
        &self,
        run: &WorkflowRun,
        context: &NativeExecutionContext,
        node: &WorkflowNodeInstance,
        selector: &ExactToolSelector,
        arguments: Value,
        retry: u32,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<(ToolExecutionResult, bool), WorkflowRunError> {
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
            retry,
        };
        self.read_model.node(node, |view| {
            view.invocation = Some(id.clone());
            view.tool_id = Some(definition.id.clone());
        });
        let prepared = match registration
            .prepare_fixed(id.clone(), definition, &arguments)
            .map_err(WorkflowRunError::IdentityChanged)?
        {
            PreflightOutcome::Ready(prepared) => prepared,
            PreflightOutcome::Rejected { error, .. } => {
                return Ok((terminal(ToolExecutionStatus::Failed { error }), true));
            }
        };
        let emit = |fact| self.emit_native(run, &id, &definition.id, fact);
        emit(NativeInvocationFact::Prepared {
            tool_name: definition.name.clone(),
            arguments_digest: crate::events::interaction::interaction_arguments_digest(
                &prepared.invocation.arguments,
            ),
        });
        if let Some(effects) = &run.effects {
            effects.owner(
                node,
                super::scheduled::lifecycle::ScheduledEffectOwner::Tool {
                    invocation: id.clone(),
                },
            )?;
        }
        let view = crate::agent::PreToolView {
            conversation_id: &run.run_id.conversation_id,
            execution: services.execution.clone(),
            invocation_id: &id,
            tool_id: &definition.id,
            tool_name: &definition.name,
            origin: &prepared.origin,
            mode: prepared.invocation.mode,
            arguments: &prepared.invocation.arguments,
            audit_arguments: &prepared.invocation.arguments,
            approval_policy: prepared.approval,
        };
        self.read_model.node(node, |view| {
            view.state = super::read_model::WorkflowState::Waiting {
                reason: super::read_model::WorkflowWait::Approval,
            }
        });
        let mut settled = true;
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
            self.read_model.node(node, |view| {
                view.state = super::read_model::WorkflowState::Waiting {
                    reason: super::read_model::WorkflowWait::Capacity,
                }
            });
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
                self.read_model.node(node, |view| {
                    view.state = super::read_model::WorkflowState::Waiting {
                        reason: if definition.name == "ask_user" {
                            super::read_model::WorkflowWait::Questionnaire
                        } else {
                            super::read_model::WorkflowWait::Tool
                        },
                    }
                });
                let progress = NativeProgress(std::sync::Mutex::new(Vec::new()));
                let native_context = ToolExecutionContext::new(
                    &run.run_id.conversation_id,
                    None,
                    cancellation.clone(),
                    services.runtime.workspace(),
                    &progress,
                    services.runtime.artifacts(),
                    services.runtime.tool_output(),
                    context.resources().capability().effective_environment(),
                );
                let requester = services
                    .lifecycle
                    .native_interaction_coordinator()
                    .map(|owner| {
                        crate::runtime::interaction::QuestionnaireRequester::for_execution(
                            owner,
                            services.execution.clone(),
                            cancellation.clone(),
                        )
                    });
                let native_context = match requester {
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
                let outcome = driver
                    .execute(
                        &*executor,
                        prepared.invocation,
                        prepared.progress_capability,
                        native_context,
                    )
                    .await;
                if outcome.physical_settlement
                    == crate::tools::deadline::ToolSettlementCertainty::Unconfirmed
                {
                    settled = false;
                    if let Some(effects) = &run.effects {
                        effects.unproven(node,"native Tool physical settlement is unconfirmed; inspect its original owner, never replay the Workflow");
                    }
                }
                for progress in progress.0.into_inner().expect("progress buffer") {
                    emit(NativeInvocationFact::Progress { progress });
                }
                for fact in outcome.facts {
                    emit(NativeInvocationFact::Lifecycle { fact });
                }
                outcome.result
            }
        };
        emit(NativeInvocationFact::Completed {
            status: bounded_status(&result.status),
        });
        Ok((result, settled))
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
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id,
            sequence: 0,
            conversation_id: run.run_id.conversation_id.clone(),
            attempt_id: run.run_id.origin.attempt_id().cloned(),
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

pub(crate) const fn default_workflow_timeout_ms() -> u64 {
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

/// What a Tool node commits, fixed by the Tool: its declared structured
/// result, or its text. Never provider text heuristics or a part index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum WorkflowToolResult {
    /// The result's one native structured JSON part, satisfying this schema.
    Json { schema: Value },
    /// `{text}`: the result's native text parts, joined by newlines.
    Text {},
}

impl WorkflowToolResult {
    /// The result a Tool declares: structured when its output schema is a
    /// Workflow object contract, otherwise its text.
    #[must_use]
    pub fn declared(output_schema: Option<&Value>) -> Self {
        match output_schema {
            Some(schema)
                if super::schema_type(schema) == Some("object")
                    && super::validate_workflow_schema(schema, "Tool result").is_ok() =>
            {
                Self::Json {
                    schema: schema.clone(),
                }
            }
            _ => Self::Text {},
        }
    }

    pub(super) fn schema(&self) -> Value {
        match self {
            Self::Json { schema } => schema.clone(),
            Self::Text {} => serde_json::json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
                "additionalProperties": false,
            }),
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
            Self::Json { .. } => {
                let mut parts = result.content.iter().filter_map(|part| match part {
                    ToolResultContent::Json { value } => Some(value),
                    _ => None,
                });
                match (parts.next(), parts.next()) {
                    (Some(value), None) => {
                        expressions::bounded_value_bytes(value)?;
                        value.clone()
                    }
                    _ => {
                        return Err(WorkflowRunError::InvalidValue(
                            "Tool result has no single structured JSON part".into(),
                        ));
                    }
                }
            }
            Self::Text {} => {
                let mut text = String::new();
                for part in &result.content {
                    if let ToolResultContent::Text(block) = part {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&block.text);
                        if text.len() > super::MAX_VALUE_BYTES {
                            return Err(WorkflowRunError::InvalidValue(
                                "Tool text exceeds Workflow value bound".into(),
                            ));
                        }
                    }
                }
                serde_json::json!({ "text": text })
            }
        };
        expressions::bounded_value_bytes(&value)?;
        execution::validate_commit(&self.schema(), &value)?;
        Ok(value)
    }
}
