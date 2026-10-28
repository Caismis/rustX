//! Attempt-bound direct Calls through the existing Auxiliary physical owner.
use std::sync::{Arc, Mutex};

use crate::context::{ContextError, ContextErrorKind, TokenEstimator as _};
use crate::events::types::RuntimeEvent;
use crate::model::{
    AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallStart, AuxiliaryFingerprint,
    AuxiliaryInputEvidence, ModelErrorKind, ModelFinishReason, ModelRequest,
};
use crate::runtime::auxiliary::executor::{
    AuxiliaryConsumer, AuxiliaryExecutor, AuxiliaryScope, ConfirmedExecution, FrozenCall,
};
use crate::runtime::auxiliary::output::{AuxiliaryOutput, AuxiliaryOutputFormat};
use crate::runtime::cancellation::ExecutionCancellation;
use crate::runtime::identity::{AuxiliaryCallId, RequestId};
use crate::runtime::subagent::NativeExecutionContext;
use crate::tools::types::ToolExecutionStatus;

use super::{
    MAX_VALUE_BYTES, Value, WorkflowModelOutput, WorkflowModelProgram, WorkflowNodeInstance,
    WorkflowRun, WorkflowRunError, WorkflowRuntime,
};

/// Only Workflow knows how to turn trusted compiled intent into a model Call.
/// This capability retains the Conversation's existing owner, never a new one.
#[derive(Clone)]
pub(crate) struct WorkflowModelCapability {
    executor: AuxiliaryExecutor,
    #[cfg(test)]
    pub(super) hook: Arc<Mutex<Option<ModelFrontierHook>>>,
}

impl WorkflowModelCapability {
    pub(crate) fn new(executor: AuxiliaryExecutor) -> Self {
        Self {
            executor,
            #[cfg(test)]
            hook: Arc::default(),
        }
    }

    #[cfg(test)]
    pub(super) async fn frontier(&self, frontier: ModelFrontier) {
        let hook = {
            let mut slot = self.hook.lock().expect("model frontier");
            if slot.as_ref().is_some_and(|hook| hook.frontier == frontier) {
                slot.take()
            } else {
                None
            }
        };
        if let Some(hook) = hook {
            let _ = hook.entered.send(());
            let _ = hook.release.await;
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelFrontier {
    BeforeAdmission,
    AfterReservation,
    BeforeCommit,
}

#[cfg(test)]
pub(super) struct ModelFrontierHook {
    pub(super) frontier: ModelFrontier,
    pub(super) entered: tokio::sync::oneshot::Sender<()>,
    pub(super) release: tokio::sync::oneshot::Receiver<()>,
}

/// Retained by the actual Auxiliary record until its worker and writer join.
/// Abandoning a Workflow wait cancels that Call but cannot retire this lease.
struct ModelConsumer {
    lease: Mutex<Option<crate::tools::invocation::NativeChildLease>>,
}
impl AuxiliaryConsumer for ModelConsumer {
    fn registered(&self) {}
    fn output_limit(&self) -> usize {
        MAX_VALUE_BYTES
    }
    fn generation_limit(&self) -> u64 {
        // Permit finite private reasoning in addition to the separately
        // bounded visible value; it can never enter Workflow locals.
        (4 * MAX_VALUE_BYTES) as u64
    }
    fn stopped(&self) {}
    fn attempt(&self, _: &RequestId, _: u32) {}
    fn text(&self, _: &RequestId, _: &str) {}
    fn settled(&self, _: &Result<ConfirmedExecution, ContextError>) {
        self.lease.lock().expect("model descendant").take();
    }
}

struct CloseScope(Arc<AuxiliaryScope>);
impl Drop for CloseScope {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Resolve every static policy before graph progression, including unselected
/// branches. This generation never consults runtime-current configuration.
pub(super) fn freeze_models(
    block: &super::WorkflowBlockProgram,
    context: &NativeExecutionContext,
) -> Result<super::BTreeMap<Vec<String>, crate::model::ResolvedModelInvocation>, WorkflowRunError> {
    let mut frozen = super::BTreeMap::new();
    for (id, node) in &block.nodes {
        match node {
            super::WorkflowNodeProgram::Model(program) => {
                if context.workflow_model.is_none() {
                    return Err(WorkflowRunError::InvocationAuthority(
                        "attempt has no Workflow Model capability".into(),
                    ));
                }
                let mut key = block.path.clone();
                key.push(id.clone());
                let invocation = context
                    .resolve_workflow_model(&program.model)
                    .map_err(|_| {
                        WorkflowRunError::InvocationAuthority(format!(
                            "Model/Profile unavailable under resource generation {} at {}",
                            context.resources().revision().get(),
                            key.join("/")
                        ))
                    })?;
                frozen.insert(key, invocation);
            }
            super::WorkflowNodeProgram::Parallel { branches, .. } => {
                for branch in branches.values() {
                    frozen.extend(freeze_models(&branch.block, context)?);
                }
            }
            super::WorkflowNodeProgram::Loop { body, .. } => {
                frozen.extend(freeze_models(body, context)?);
            }
            _ => {}
        }
    }
    Ok(frozen)
}

impl WorkflowRuntime {
    /// Publish only through the existing durable Workflow progress frontier.
    /// Call identity is a reservation association, not an Auxiliary start fact.
    fn publish_model_wait(
        &self,
        run: &WorkflowRun,
        node: &WorkflowNodeInstance,
        reason: super::read_model::WorkflowWait,
        auxiliary_call_id: Option<AuxiliaryCallId>,
    ) {
        self.read_model.node(node, |view| {
            view.state = super::read_model::WorkflowState::Waiting { reason };
            view.auxiliary_call_id.clone_from(&auxiliary_call_id);
        });
        self.emit_observability(
            run,
            RuntimeEvent::WorkflowNodeWaiting {
                instance: node.clone(),
                reason,
                auxiliary_call_id,
            },
        );
    }

    #[allow(clippy::too_many_lines)] // One linear admission, physical join and acceptance protocol.
    pub(super) async fn invoke_model(
        &self,
        run: &WorkflowRun,
        context: &NativeExecutionContext,
        node: &WorkflowNodeInstance,
        program: &WorkflowModelProgram,
        input: Value,
        cancellation: &ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let capability = context.workflow_model.as_ref().ok_or_else(|| {
            WorkflowRunError::InvocationAuthority("attempt has no Workflow Model capability".into())
        })?;
        let mut key = node.block.definition.blocks.clone();
        key.push(node.node.clone());
        let invocation = run.model_invocations.get(&key).cloned().ok_or_else(|| {
            WorkflowRunError::InvocationAuthority(
                "Model invocation was not frozen at Workflow admission".into(),
            )
        })?;
        super::expressions::bounded_value_bytes(&input)?;
        let format = match &program.output {
            WorkflowModelOutput::Text {} => AuxiliaryOutputFormat::Text,
            WorkflowModelOutput::JsonSchema { schema } => AuxiliaryOutputFormat::JsonSchema(
                crate::model::structured::ResponseSchema::new(
                    "workflow_output".into(),
                    schema.clone(),
                    MAX_VALUE_BYTES,
                )
                .map_err(|_| failure(node, failed("invalid frozen Model output schema")))?,
            ),
        };
        let request = format
            .prepare(ModelRequest {
                response_schema: None,
                response_byte_limit: Some(
                    crate::model::adapter::response_budget::AUXILIARY_RESPONSE_BYTES,
                ),
                images: super::BTreeMap::default(),
                invocation: invocation.invocation_config(),
                messages: vec![crate::model::ModelInputMessage::RequestOnly(
                    crate::model::RequestOnlyModelContext::AuxiliaryQuestion {
                        text: serde_json::to_string(&input).expect("bounded JSON bindings"),
                    },
                )]
                .into(),
                tools: Vec::new(),
                effective_system_prompt: program.instruction.clone(),
                continuation: None,
                continuation_source: None,
            })
            .map_err(|_| failure(node, failed("cannot prepare frozen Model output contract")))?;
        // Bound the fully prepared request (including Prompt schema fallback)
        // before a permit, worker or durable start can exist.
        crate::model::adapter::validation::validate_bounded_prepared_request(&request, 256 * 1024)
            .map_err(|_| {
                failure(
                    node,
                    failed("prepared Model input exceeds its request bound"),
                )
            })?;
        let input_limit = crate::context::ContextConfig {
            context_window_tokens: invocation.context_window(),
            reserve_tokens: context.execution_policy().context.reserve_tokens,
            keep_recent_tokens: 0,
        }
        .soft_input_limit(invocation.max_output_tokens())
        .map_err(|_| failure(node, failed("Model configuration has no input budget")))?;
        if crate::context::DefaultTokenEstimator.estimate_frozen_request(&request) > input_limit {
            return Err(failure(
                node,
                failed("Model input exceeds the frozen model context window"),
            ));
        }
        #[cfg(test)]
        capability.frontier(ModelFrontier::BeforeAdmission).await;
        let scope = capability.executor.general_scope(
            crate::runtime::auxiliary::authority::CALL_START_LIMIT,
            &cancellation.child_signal(),
        );
        let _close = CloseScope(scope.clone());
        // Subscribe before checking capacity: a release between the check and
        // the await remains visible. Busy is unchanged in the shared Executor.
        let mut changed = capability.executor.capacity_changes();
        let mut admission_waiting = false;
        let reservation = loop {
            if cancellation.is_cancelled() {
                return Err(WorkflowRunError::from_cancellation(cancellation));
            }
            match capability.executor.reserve_general(&scope) {
                Ok(reservation) => break reservation,
                Err(error)
                    if matches!(
                        error.kind,
                        ContextErrorKind::AuxiliaryBusy | ContextErrorKind::AuxiliaryLifecycleBusy
                    ) =>
                {
                    if !admission_waiting {
                        self.publish_model_wait(
                            run,
                            node,
                            super::read_model::WorkflowWait::Capacity,
                            None,
                        );
                        admission_waiting = true;
                    }
                    // A refused lifecycle grant reserves no permits and emits
                    // no capacity notification. Its existing waiter registers
                    // before inspecting the lock, including a release between
                    // reserve_general's refusal and the wait's first poll.
                    let available = async {
                        if error.kind == ContextErrorKind::AuxiliaryLifecycleBusy {
                            capability.executor.admission_commit_available().await;
                            Ok(())
                        } else {
                            changed.changed().await
                        }
                    };
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => return Err(WorkflowRunError::from_cancellation(cancellation)),
                        () = capability.executor.admission_closed() => return Err(failure(node, failed("Auxiliary admission owner closed"))),
                        result = available => if result.is_err() { return Err(failure(node, failed("Auxiliary capacity owner closed"))); },
                    }
                }
                Err(error) => return Err(context_failure(node, &error, cancellation)),
            }
        };
        let call_id = reservation.call_id().clone();
        if let Some(effects) = &run.effects {
            effects.owner(
                node,
                super::scheduled::lifecycle::ScheduledEffectOwner::Model {
                    call_id: call_id.clone(),
                },
            )?;
        }
        #[cfg(test)]
        capability.frontier(ModelFrontier::AfterReservation).await;
        self.publish_model_wait(
            run,
            node,
            super::read_model::WorkflowWait::Model,
            Some(call_id.clone()),
        );
        // Opaque attribution hashes the complete native instance identity;
        // the read model supplies its exact CallId association without input.
        let reference = {
            use sha2::{Digest, Sha256};
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(node).expect("native node identity"))
            )
        };
        let start = AuxiliaryCallStart {
            call_id,
            purpose: AuxiliaryAttribution::new("workflow_model").expect("native attribution"),
            trigger: AuxiliaryAttribution::new("workflow_node").expect("native attribution"),
            business_source: AuxiliaryBusinessSource::Business {
                namespace: AuxiliaryAttribution::new("workflow_node_instance")
                    .expect("native namespace"),
                reference: AuxiliaryAttribution::new(reference).expect("bounded node identity"),
            },
            input: AuxiliaryInputEvidence::Temporary,
            semantic_input_fingerprint: AuxiliaryFingerprint::new(
                request.semantic_input_fingerprint(),
            )
            .expect("native fingerprint"),
            invocation_fingerprint: crate::runtime::auxiliary::executor::invocation_fingerprint(
                &request.invocation,
                context.resources().revision().get(),
            )
            .map_err(|error| context_failure(node, &error, cancellation))?,
            selected_message_ids: Vec::new(),
        };
        let consumer = Arc::new(ModelConsumer {
            lease: Mutex::new(
                context
                    .native
                    .as_ref()
                    .map(|services| services.descendants.enter()),
            ),
        });
        let completed = reservation
            .with_consumer(consumer)
            .submit(FrozenCall {
                start,
                request,
                invocation,
                timeout: context.execution_policy().model_timeout,
                resource_revision: context.resources().revision().get(),
                dispatch_check: None,
                title_source: None,
            })
            .map_err(|error| context_failure(node, &error, cancellation))?
            // Never select cancellation against this join. The scope's child
            // signal propagates immediately; the physical owner still drains.
            .await
            .map_err(|_| {
                if let Some(effects) = &run.effects {
                    effects.unproven(node, "Auxiliary physical completion unavailable");
                }
                failure(
                    node,
                    ToolExecutionStatus::OutcomeUnknown {
                        detail: "Auxiliary physical completion unavailable".into(),
                    },
                )
            })?
            .map_err(|error| {
                if matches!(
                    error.kind,
                    ContextErrorKind::AuxiliaryDurabilityFailed
                        | ContextErrorKind::AuxiliarySupervisionFailed
                ) && let Some(effects) = &run.effects
                {
                    effects.unproven(
                        node,
                        "Auxiliary execution or durable settlement could not be confirmed",
                    );
                }
                context_failure(node, &error, cancellation)
            })?;
        let output = completed.output.map_err(|error| {
            if cancellation.is_cancelled() {
                WorkflowRunError::from_cancellation(cancellation)
            } else {
                failure(
                    node,
                    match error.kind {
                        ModelErrorKind::Timeout => ToolExecutionStatus::TimedOut,
                        _ => failed("Model output failed or violated its frozen contract"),
                    },
                )
            }
        })?;
        if cancellation.is_cancelled() {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        }
        match (&program.output, output) {
            (WorkflowModelOutput::Text {}, AuxiliaryOutput::Text(output))
                if !output.refused
                    && output.finish_reason == ModelFinishReason::Stop
                    && !output.text.is_empty() =>
            {
                Ok(Value::String(output.text))
            }
            (WorkflowModelOutput::JsonSchema { .. }, AuxiliaryOutput::JsonSchema(value)) => {
                Ok(value)
            }
            _ => Err(failure(
                node,
                failed("Model produced no usable final output"),
            )),
        }
    }
}

fn failed(message: &str) -> ToolExecutionStatus {
    ToolExecutionStatus::Failed {
        error: message.into(),
    }
}
fn failure(node: &WorkflowNodeInstance, status: ToolExecutionStatus) -> WorkflowRunError {
    WorkflowRunError::ModelFailed {
        node: node.to_string(),
        status,
    }
}
fn context_failure(
    node: &WorkflowNodeInstance,
    error: &ContextError,
    cancellation: &ExecutionCancellation,
) -> WorkflowRunError {
    use ContextErrorKind as Kind;
    // Unknown settlement outranks cancellation: neither a cancelled wait nor a
    // business failure can prove the owner completed its durable obligations.
    match error.kind {
        Kind::AuxiliaryDurabilityFailed | Kind::AuxiliarySupervisionFailed => failure(
            node,
            ToolExecutionStatus::OutcomeUnknown {
                detail: "Auxiliary execution or durable settlement could not be confirmed".into(),
            },
        ),
        _ if cancellation.is_cancelled() => WorkflowRunError::from_cancellation(cancellation),
        Kind::AuxiliaryDeadlineExceeded => failure(node, ToolExecutionStatus::TimedOut),
        _ => failure(
            node,
            failed("Auxiliary Model Call could not be admitted or settled"),
        ),
    }
}
