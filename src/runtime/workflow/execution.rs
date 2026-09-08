//! The single root/branch block executor. Native children always settle inline.
use super::expressions::CommittedValue;
use super::{
    BTreeMap, MAX_VALUE_BYTES, MAX_WORKFLOW_NODES, RuntimeEvent, Value, WorkflowBlockInstance,
    WorkflowBlockProgram, WorkflowDefinitionPath, WorkflowExecutionOutcome, WorkflowNodeInstance,
    WorkflowNodeProgram, WorkflowPort, WorkflowRun, WorkflowRunError, WorkflowRuntime,
    evaluate_predicate, evaluate_value, single_successor,
};

#[cfg(test)]
pub(super) struct NodeFrontierHook {
    pub node: String,
    pub entered: tokio::sync::oneshot::Sender<()>,
    pub release: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(test)]
pub(super) struct PreStartHook {
    pub node: String,
    pub acquired: tokio::sync::oneshot::Sender<(
        crate::runtime::workspace::CandidateScope,
        crate::runtime::workspace::CandidateReference,
        WorkflowNodeInstance,
        [usize; 2],
    )>,
    pub proceed: tokio::sync::oneshot::Receiver<PreStartAction>,
    pub released: tokio::sync::oneshot::Sender<[usize; 2]>,
    pub finish: tokio::sync::oneshot::Receiver<()>,
}
#[cfg(test)]
pub(super) enum PreStartAction {
    Continue,
    ExhaustNodes,
    ExhaustAgents,
}

#[derive(Debug, Default)]
pub(super) struct RunBudgets {
    nodes: usize,
    agents: usize,
    retained_bytes: usize,
    reserved_bytes: usize,
}

impl RunBudgets {
    pub(super) fn reserved(bytes: usize) -> Self {
        Self {
            reserved_bytes: bytes,
            ..Self::default()
        }
    }
}

/// Reserve a completion-order-independent upper bound for every possible
/// private input, node local and export. Mutually exclusive paths may be
/// overcounted deliberately. Actual byte accounting still follows lifetimes.
pub(super) fn static_retained_bound(block: &WorkflowBlockProgram) -> usize {
    let mut bytes = schema_value_bound(&block.input_schema);
    for node in block.nodes.values() {
        bytes = bytes.saturating_add(match node {
            WorkflowNodeProgram::Agent(agent) => schema_value_bound(&agent.output_schema),
            WorkflowNodeProgram::Tool { result, .. } => schema_value_bound(&result.schema()),
            WorkflowNodeProgram::Branch { .. } => 0,
            WorkflowNodeProgram::Review { .. } => {
                schema_value_bound(&super::review::result_schema())
            }
            WorkflowNodeProgram::Return { .. } => schema_value_bound(&block.output_schema),
            WorkflowNodeProgram::Parallel {
                branches,
                output_schema,
            } => branches
                .values()
                .map(|branch| static_retained_bound(&branch.block))
                .fold(schema_value_bound(output_schema), usize::saturating_add),
        });
    }
    bytes
}

fn schema_value_bound(schema: &Value) -> usize {
    if let Some(values) = super::finite_schema_values(schema) {
        // JSON Schema considers integral float and integer encodings equal.
        // Their serialized sizes can differ, including inside containers.
        let mut pending = values.iter().collect::<Vec<_>>();
        while let Some(value) = pending.pop() {
            match value {
                Value::Number(_) => return MAX_VALUE_BYTES,
                Value::Array(items) => pending.extend(items),
                Value::Object(fields) => pending.extend(fields.values()),
                _ => (),
            }
        }
        return values
            .iter()
            .map(|value| serde_json::to_vec(value).expect("schema JSON").len())
            .max()
            .unwrap_or(MAX_VALUE_BYTES)
            .min(MAX_VALUE_BYTES);
    }
    match super::schema_type(schema) {
        Some("null") => 4,
        Some("boolean") => 5,
        Some("object") if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return 2;
            };
            let bytes = properties.iter().fold(2_usize, |bytes, (key, value)| {
                bytes.saturating_add(
                    serde_json::to_vec(key).expect("schema key").len()
                        + 2
                        + schema_value_bound(value),
                )
            });
            bytes
                .saturating_sub(usize::from(!properties.is_empty()))
                .min(MAX_VALUE_BYTES)
        }
        _ => MAX_VALUE_BYTES,
    }
}

/// Accounting follows the actual lifetime of private state and exported values.
struct LocalReservation<'a> {
    run: &'a WorkflowRun,
    bytes: usize,
}

impl<'a> LocalReservation<'a> {
    fn new(run: &'a WorkflowRun) -> Self {
        Self { run, bytes: 0 }
    }
    fn retain(&mut self, value: &Value) -> Result<(), WorkflowRunError> {
        let bytes = super::expressions::bounded_value_bytes(value)?;
        let mut budgets = self.run.budgets.lock().expect("run budgets");
        if bytes > MAX_VALUE_BYTES || budgets.retained_bytes + bytes > budgets.reserved_bytes {
            return Err(WorkflowRunError::InvalidValue(
                "aggregate retained value budget exceeded".into(),
            ));
        }
        budgets.retained_bytes += bytes;
        self.bytes += bytes;
        Ok(())
    }
}

impl Drop for LocalReservation<'_> {
    fn drop(&mut self) {
        self.run.budgets.lock().expect("run budgets").retained_bytes -= self.bytes;
    }
}

/// Optional exact human-acceptance constraint, not Tool/workspace permission.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum Acceptance {
    #[default]
    None,
    Accepted(crate::runtime::workspace::CandidateReference),
}
impl Acceptance {
    fn candidate(&self) -> Option<&crate::runtime::workspace::CandidateReference> {
        match self {
            Self::None => None,
            Self::Accepted(candidate) => Some(candidate),
        }
    }
    fn apply(&mut self, transition: &AcceptanceTransition) {
        match transition {
            AcceptanceTransition::Unchanged => {}
            AcceptanceTransition::Cleared => *self = Self::None,
            AcceptanceTransition::Replaced(candidate) => *self = Self::Accepted(candidate.clone()),
        }
    }
}

/// A block's explicit effect relative to its entry, never an inherited snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum AcceptanceTransition {
    #[default]
    Unchanged,
    Cleared,
    Replaced(crate::runtime::workspace::CandidateReference),
}
impl AcceptanceTransition {
    /// Sequential composition: a later explicit effect supersedes the earlier one.
    fn then(&mut self, later: &Self) {
        if !matches!(later, Self::Unchanged) {
            *self = later.clone();
        }
    }
    /// Parallel composition: inherited state contributes nothing. Clear/replace
    /// has no proven common timeline in these facts, so it fails closed too.
    fn merge(&mut self, other: &Self) -> Result<(), WorkflowRunError> {
        if matches!(other, Self::Unchanged) {
            return Ok(());
        }
        if matches!(self, Self::Unchanged) {
            *self = other.clone();
            return Ok(());
        }
        if self == other {
            return Ok(());
        }
        Err(WorkflowRunError::InvalidValue(
            "conflicting candidate acceptance transitions".into(),
        ))
    }
}

pub(super) struct BlockOutput<'a> {
    pub(super) value: CommittedValue,
    acceptance_transition: AcceptanceTransition,
    _reservation: LocalReservation<'a>,
}

impl WorkflowRuntime {
    /// Every lexical scope, including root, enters and exits here. Boxed only
    /// because fixed nested Parallel recursively invokes the same executor.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_block<'a>(
        &'a self,
        run: &'a WorkflowRun,
        block: &'a WorkflowBlockProgram,
        context: &'a crate::runtime::subagent::AttemptSubagentContext,
        input: CommittedValue,
        acceptance: Acceptance,
        cancellation: &'a crate::runtime::cancellation::ExecutionCancellation,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<BlockOutput<'a>, WorkflowRunError>> + Send + 'a,
        >,
    > {
        Box::pin(async move {
            let instance = WorkflowBlockInstance {
                run: run.run_id.clone(),
                definition: WorkflowDefinitionPath {
                    workflow_id: run.program.id.clone(),
                    blocks: block.path.clone(),
                },
                invocations: vec![0; block.path.len() / 2 + 1],
            };
            self.emit_observability(
                run,
                RuntimeEvent::WorkflowBlockStarted {
                    instance: instance.clone(),
                },
            );
            let result = self
                .execute_block_body(
                    run,
                    block,
                    &instance,
                    context,
                    input,
                    acceptance,
                    cancellation,
                )
                .await;
            self.emit_observability(
                run,
                RuntimeEvent::WorkflowBlockSettled {
                    instance,
                    outcome: outcome(&result),
                },
            );
            result
        })
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn execute_block_body<'a>(
        &'a self,
        run: &'a WorkflowRun,
        block: &'a WorkflowBlockProgram,
        instance: &WorkflowBlockInstance,
        context: &'a crate::runtime::subagent::AttemptSubagentContext,
        input: CommittedValue,
        acceptance: Acceptance,
        cancellation: &'a crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<BlockOutput<'a>, WorkflowRunError> {
        input.assert_current(run).await?;
        validate_commit(&block.input_schema, &input.value)?;
        let mut reservation = LocalReservation::new(run);
        reservation.retain(&input.value)?;
        let mut control = acceptance;
        let mut transition = AcceptanceTransition::Unchanged;
        let mut values = BTreeMap::new();
        let mut node_id = block.entry.clone();
        loop {
            let node = &block.nodes[&node_id];
            let node_instance = WorkflowNodeInstance {
                block: instance.clone(),
                node: node_id.clone(),
                visit: 0,
            };
            #[cfg(test)]
            {
                let hook = {
                    let mut hook = self.node_frontier.lock().expect("node frontier");
                    if hook.as_ref().is_some_and(|hook| hook.node == node_id) {
                        hook.take()
                    } else {
                        None
                    }
                };
                if let Some(hook) = hook {
                    let _ = hook.entered.send(());
                    let _ = hook.release.await;
                }
            }
            // Only candidate consumers apply the exact acceptance constraint. Business-only
            // tools and pure reads of Review decisions do not touch CandidateScope.
            let consumes = match node {
                WorkflowNodeProgram::Agent(_) => run.candidate.is_some(),
                WorkflowNodeProgram::Tool { selector, .. } => {
                    Self::tool_workspace_use(run, context, selector)?
                        == crate::tools::executor::WorkspaceUse::ConsumesProvided
                }
                _ => false,
            };
            let mut dependency = CommittedValue::from(Value::Null);
            match node {
                WorkflowNodeProgram::Tool { arguments, .. } => {
                    dependency.depend_on(&evaluate_value(arguments, &input, &values)?)?;
                }
                WorkflowNodeProgram::Agent(agent) => {
                    for binding in agent.input.values() {
                        dependency.depend_on(&evaluate_value(binding, &input, &values)?)?;
                    }
                }
                WorkflowNodeProgram::Branch { condition } => {
                    dependency.depend_on(&evaluate_predicate(condition, &input, &values)?)?;
                }
                WorkflowNodeProgram::Return { output } => {
                    dependency.depend_on(&evaluate_value(output, &input, &values)?)?;
                }
                _ => {}
            }
            // Explicit data dependencies remain checked even for independent tools,
            // but never grant them an exclusive workspace borrow during execution.
            dependency.assert_current(run).await?;
            if consumes {
                dependency.depend_on(&CommittedValue {
                    value: Value::Null,
                    candidate: control.candidate().cloned(),
                })?;
            }
            let mut admitted_access = if let Some(reference) = dependency
                .candidate
                .as_ref()
                .filter(|_| consumes || !matches!(node, WorkflowNodeProgram::Tool { .. }))
            {
                Some(
                    run.candidate
                        .as_ref()
                        .expect("candidate owner")
                        .borrow(
                            node_instance.clone(),
                            Some(reference),
                            &cancellation.child_signal(),
                        )
                        .await
                        .map_err(|error| {
                            if cancellation.is_cancelled() {
                                WorkflowRunError::from_cancellation(cancellation)
                            } else {
                                WorkflowRunError::InvalidValue(error)
                            }
                        })?,
                )
            } else {
                None
            };
            #[cfg(test)]
            let pre_start_hook = {
                let mut hook = self.pre_start.lock().expect("pre-start hook");
                if admitted_access.is_some()
                    && hook.as_ref().is_some_and(|hook| hook.node == node_id)
                {
                    hook.take()
                } else {
                    None
                }
            };
            #[cfg(test)]
            let pre_start_cleanup = if let Some(PreStartHook {
                acquired,
                proceed,
                released,
                finish,
                ..
            }) = pre_start_hook
            {
                let counts = {
                    let budgets = run.budgets.lock().unwrap();
                    [budgets.nodes, budgets.agents]
                };
                let _ = acquired.send((
                    run.candidate.as_ref().unwrap().clone(),
                    admitted_access.as_ref().unwrap().input().clone(),
                    node_instance.clone(),
                    counts,
                ));
                match proceed.await.expect("pre-start action") {
                    PreStartAction::Continue => {}
                    PreStartAction::ExhaustNodes => {
                        run.budgets.lock().unwrap().nodes = run.program.total_nodes;
                    }
                    PreStartAction::ExhaustAgents => {
                        run.budgets.lock().unwrap().agents = MAX_WORKFLOW_NODES;
                    }
                }
                Some((released, finish))
            } else {
                None
            };
            let mut started = false;
            // Every fallible step after borrow stays inside this result scope.
            // The one finalizer below returns any access not transferred to a
            // native owner, including zero-start cancellation/budget rejection.
            let result: Result<Option<BlockOutput<'_>>, WorkflowRunError> = async {
                // Node admission frontier: cancellation observation and aggregate
                // count reservation are synchronous, before any child preparation.
                {
                    let mut budgets = run.budgets.lock().expect("run budgets");
                    if cancellation.is_cancelled() {
                        return Err(WorkflowRunError::from_cancellation(cancellation));
                    }
                    let agent = usize::from(matches!(node, WorkflowNodeProgram::Agent(_)));
                    if budgets.nodes >= run.program.total_nodes
                        || budgets.agents + agent > MAX_WORKFLOW_NODES
                    {
                        return Err(WorkflowRunError::InvalidProgram(
                            "aggregate execution count exceeded".into(),
                        ));
                    }
                    budgets.nodes += 1;
                    budgets.agents += agent;
                }
                self.emit_observability(
                    run,
                    RuntimeEvent::WorkflowNodeStarted {
                        instance: node_instance.clone(),
                    },
                );
                started = true;
                // This lease owns the complete admission/settlement future,
                // including approval or staged Agent cleanup. It retires only
                // after the native owner returned; it is not outcome authority.
                let _native_owner = if matches!(
                    node,
                    WorkflowNodeProgram::Agent(_)
                        | WorkflowNodeProgram::Tool { .. }
                        | WorkflowNodeProgram::Review { .. }
                ) {
                    context
                        .native
                        .as_ref()
                        .map(|services| services.descendants.enter())
                } else {
                    None
                };
                match node {
                    WorkflowNodeProgram::Review {
                        subject,
                        context: checks,
                    } => {
                        if let Some(access) = admitted_access.take() {
                            access
                                .finish(true)
                                .await
                                .map_err(WorkflowRunError::InvalidValue)?;
                        }
                        let bound = evaluate_value(subject.value(), &input, &values)?;
                        bound.assert_current(run).await?;
                        let mut facts = Vec::new();
                        for check in checks {
                            let value = evaluate_value(check, &input, &values)?;
                            value.assert_current(run).await?;
                            facts.push(crate::events::review::ReviewFact {
                                value: value.value,
                                candidate: value.candidate,
                            });
                        }
                        let checks = facts;
                        let (value, accepted_candidate) = self
                            .review(
                                run,
                                context,
                                &node_instance,
                                subject,
                                bound,
                                checks,
                                cancellation,
                            )
                            .await?;
                        reservation.retain(&value.value)?;
                        if let Some(candidate) = accepted_candidate {
                            // A new explicit acceptance replaces the old subject.
                            // Business-only decisions cannot erase an existing dependency.
                            transition = AcceptanceTransition::Replaced(candidate);
                            control.apply(&transition);
                        }
                        values.insert(node_id.clone(), value);
                        Ok(None)
                    }

                    WorkflowNodeProgram::Tool {
                        selector,
                        arguments,
                        result,
                    } => {
                        let arguments = evaluate_value(arguments, &input, &values)?;
                        if admitted_access.is_none() {
                            arguments.assert_current(run).await?;
                        }
                        let (native, candidate) = self
                            .invoke_tool(
                                run,
                                context,
                                &node_instance,
                                selector,
                                arguments,
                                cancellation,
                                &mut admitted_access,
                            )
                            .await?;
                        let value = result.project(&native, &node_instance)?;
                        reservation.retain(&value)?;
                        // Physical settlement and source verification precede
                        // this sole candidate-bound local commit.
                        values.insert(node_id.clone(), CommittedValue { value, candidate });
                        Ok(None)
                    }
                    WorkflowNodeProgram::Agent(agent) => {
                        let child = self
                            .admit_agent(
                                run,
                                context,
                                &input,
                                &values,
                                &node_instance,
                                agent,
                                cancellation,
                                &mut admitted_access,
                            )
                            .await?;
                        let value = self
                            .settle_agent(child, &node_instance, &agent.output_schema, cancellation)
                            .await?;
                        reservation.retain(&value.value)?;
                        values.insert(node_id.clone(), value);
                        if consumes && matches!(control, Acceptance::Accepted(_)) {
                            transition = AcceptanceTransition::Cleared;
                            control.apply(&transition);
                        }
                        Ok(None)
                    }
                    WorkflowNodeProgram::Branch { condition } => {
                        let condition = evaluate_predicate(condition, &input, &values)?;
                        if admitted_access.is_none() {
                            condition.assert_current(run).await?;
                        }
                        let port = if condition.value.as_bool().expect("predicate boolean") {
                            WorkflowPort::True
                        } else {
                            WorkflowPort::False
                        };
                        let successor = block.outgoing[&node_id]
                            .iter()
                            .find(|edge| edge.port == port)
                            .ok_or_else(|| WorkflowRunError::InvalidProgram(node_id.clone()))?
                            .to
                            .clone();
                        self.emit_observability(
                            run,
                            RuntimeEvent::WorkflowBranchSelected {
                                workflow_id: run.program.id.clone(),
                                run_id: run.run_id.clone(),
                                node_id: node_instance.clone(),
                                port,
                                successor: successor.clone(),
                            },
                        );
                        node_id = successor;
                        Ok(None)
                    }
                    WorkflowNodeProgram::Parallel {
                        branches,
                        output_schema,
                    } => {
                        if let Some(access) = admitted_access.take() {
                            access
                                .finish(true)
                                .await
                                .map_err(WorkflowRunError::InvalidValue)?;
                        }
                        self.emit_observability(
                            run,
                            RuntimeEvent::WorkflowParallelAdmitted {
                                workflow_id: run.program.id.clone(),
                                run_id: run.run_id.clone(),
                                node_id: node_instance.clone(),
                                branches: branches.keys().cloned().collect(),
                            },
                        );
                        // join_all owns and drains every branch future, even on
                        // failure/cancellation. Collection order is definition
                        // order; readiness never determines result placement.
                        let pending = branches.iter().map(|(key, branch)| {
                            let input = &input;
                            let values = &values;
                            let control = &control;
                            async move {
                                let bound = evaluate_value(&branch.input, input, values);
                                let result = match bound {
                                    Ok(input) => {
                                        self.execute_block(
                                            run,
                                            &branch.block,
                                            context,
                                            input,
                                            control.clone(),
                                            cancellation,
                                        )
                                        .await
                                    }
                                    Err(error) => Err(error),
                                };
                                (key, result)
                            }
                        });
                        let settled = futures_util::future::join_all(pending).await;
                        let mut results = serde_json::Map::new();
                        let mut applicability = CommittedValue::from(Value::Null);
                        let mut failures = BTreeMap::new();
                        for (key, result) in &settled {
                            match result {
                                Ok(output) => {
                                    results.insert((*key).clone(), output.value.value.clone());
                                }
                                Err(error) => {
                                    failures.insert((*key).clone(), error.clone());
                                }
                            }
                        }
                        self.emit_observability(
                            run,
                            RuntimeEvent::WorkflowParallelSettled {
                                workflow_id: run.program.id.clone(),
                                run_id: run.run_id.clone(),
                                node_id: node_instance.clone(),
                                succeeded: results.keys().cloned().collect(),
                                failed: failures.keys().cloned().collect(),
                            },
                        );
                        if !failures.is_empty() {
                            return Err(WorkflowRunError::ParallelFailed {
                                node: node_instance.to_string(),
                                failures,
                            });
                        }
                        if cancellation.is_cancelled() {
                            return Err(WorkflowRunError::from_cancellation(cancellation));
                        }
                        let mut joined_transition = AcceptanceTransition::Unchanged;
                        // Native branch failures retain their status before
                        // validating applicability of an otherwise successful join.
                        for (_, result) in &settled {
                            let output = result.as_ref().expect("successful branches");
                            output.value.assert_current(run).await?;
                            applicability.depend_on(&output.value)?;
                            joined_transition.merge(&output.acceptance_transition)?;
                        }
                        control.apply(&joined_transition);
                        transition.then(&joined_transition);
                        let value = Value::Object(results);
                        validate_commit(output_schema, &value)?;
                        // Transfer exported output accounting before releasing
                        // branch outputs. Private locals were already retired.
                        reservation.retain(&value)?;
                        drop(settled);
                        applicability.value = value;
                        values.insert(node_id.clone(), applicability);
                        Ok(None)
                    }
                    WorkflowNodeProgram::Return { output } => {
                        let value = evaluate_value(output, &input, &values)?;
                        if admitted_access.is_none() {
                            value.assert_current(run).await?;
                        }
                        validate_commit(&block.output_schema, &value.value)?;
                        Ok(Some(value))
                    }
                }
            }
            .await
            .and_then(|candidate| {
                candidate
                    .map(|value| {
                        let mut exported = LocalReservation::new(run);
                        exported.retain(&value.value)?;
                        Ok(BlockOutput {
                            value,
                            acceptance_transition: transition.clone(),
                            _reservation: exported,
                        })
                    })
                    .transpose()
            });
            let result = if let Some(access) = admitted_access.take() {
                match access.finish(true).await {
                    Ok(_) => result,
                    Err(error) => Err(WorkflowRunError::InvalidValue(error)),
                }
            } else {
                result
            };
            #[cfg(test)]
            if let Some((released, finish)) = pre_start_cleanup {
                let counts = {
                    let budgets = run.budgets.lock().unwrap();
                    [budgets.nodes, budgets.agents]
                };
                let _ = released.send(counts);
                let _ = finish.await;
            }
            if started {
                self.emit_observability(
                    run,
                    RuntimeEvent::WorkflowNodeSettled {
                        instance: node_instance,
                        outcome: outcome(&result),
                    },
                );
            }
            if let Some(output) = result? {
                // Input and locals retire at owning block completion. Only the
                // declared export survives, with its own bounded reservation.
                return Ok(output);
            }
            if !matches!(node, WorkflowNodeProgram::Branch { .. }) {
                node_id = single_successor(block, &node_id)?;
            }
        }
    }
}

pub(super) fn validate_commit(schema: &Value, value: &Value) -> Result<(), WorkflowRunError> {
    let validator = jsonschema::Validator::new(schema)
        .map_err(|error| WorkflowRunError::InvalidValue(error.to_string()))?;
    if !validator.is_valid(value) {
        return Err(WorkflowRunError::InvalidValue(
            "block commit violates frozen schema".into(),
        ));
    }
    Ok(())
}

fn outcome<T>(result: &Result<T, WorkflowRunError>) -> WorkflowExecutionOutcome {
    match result {
        Ok(_) => WorkflowExecutionOutcome::Completed,
        Err(error) => match error.execution_status() {
            crate::tools::types::ToolExecutionStatus::Cancelled { .. } => {
                WorkflowExecutionOutcome::Cancelled
            }
            crate::tools::types::ToolExecutionStatus::Denied { .. } => {
                WorkflowExecutionOutcome::Denied
            }
            crate::tools::types::ToolExecutionStatus::TimedOut => {
                WorkflowExecutionOutcome::TimedOut
            }
            crate::tools::types::ToolExecutionStatus::OutcomeUnknown { .. } => {
                WorkflowExecutionOutcome::OutcomeUnknown
            }
            _ => WorkflowExecutionOutcome::Failed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::workflow::{MAX_LOCAL_BYTES, WorkflowId, WorkflowProgram, test_instance};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    #[test]
    fn retained_budget_is_shared_and_released_only_with_private_state() {
        assert_eq!(
            schema_value_bound(&serde_json::json!({"type":"object","const":{"value":1}})),
            MAX_VALUE_BYTES,
            "numerically equal JSON encodings can have different serialized lengths"
        );
        let definition = serde_json::from_value(serde_json::json!({
            "description":"budget fixture", "block": {
                "input":{"type":"object"},"output":{"type":"object"},"entry":"done",
                "nodes":{"done":{"type":"return","output":{"type":"literal","value":{}}}}, "edges":[]
            }
        })).unwrap();
        let program = Arc::new(
            WorkflowProgram::compile(
                WorkflowId::parse("budget").unwrap(),
                definition,
                &BTreeSet::new(),
            )
            .unwrap(),
        );
        let run = WorkflowRun::new(program, test_instance("budget", "done").block.run);
        // Exercise the counter at its global limit independently of the
        // compiler's tighter reservation for this tiny fixture.
        run.budgets.lock().unwrap().reserved_bytes = MAX_LOCAL_BYTES;
        let value = serde_json::json!("x".repeat(MAX_VALUE_BYTES - 2));
        let mut scopes = Vec::new();
        for _ in 0..MAX_LOCAL_BYTES / MAX_VALUE_BYTES {
            let mut scope = LocalReservation::new(&run);
            scope.retain(&value).unwrap();
            scopes.push(scope);
        }
        assert_eq!(run.budgets.lock().unwrap().retained_bytes, MAX_LOCAL_BYTES);
        let mut nested = LocalReservation::new(&run);
        assert!(
            nested.retain(&value).is_err(),
            "child scope cannot reset aggregate counter"
        );
        assert_eq!(nested.bytes, 0, "rejected reservation commits nothing");
        drop(scopes.pop());
        nested.retain(&value).unwrap();
        drop(nested);
        drop(scopes);
        assert_eq!(
            run.budgets.lock().unwrap().retained_bytes,
            0,
            "private state retirement releases every byte"
        );
    }
}

#[cfg(test)]
mod acceptance_tests {
    use super::*;
    fn candidate(content: char) -> crate::runtime::workspace::CandidateReference {
        crate::runtime::workspace::CandidateReference {
            run: super::super::test_instance("acceptance", "node").block.run,
            version: 1,
            content: content.to_string().repeat(64),
        }
    }
    #[test]
    fn parallel_transition_merge_is_commutative_and_conflicts_fail_closed() {
        use AcceptanceTransition::{Cleared, Replaced, Unchanged};
        let a = candidate('a');
        let b = candidate('b');
        let c = candidate('c');
        for (left, right, expected) in [
            (Unchanged, Unchanged, Some(Unchanged)),
            (Unchanged, Cleared, Some(Cleared)),
            (Unchanged, Replaced(b.clone()), Some(Replaced(b.clone()))),
            (Cleared, Cleared, Some(Cleared)),
            (
                Replaced(b.clone()),
                Replaced(b.clone()),
                Some(Replaced(b.clone())),
            ),
            (Replaced(b.clone()), Replaced(c), None),
            (Cleared, Replaced(b.clone()), None),
        ] {
            for (mut merged, other) in [(left.clone(), right.clone()), (right, left)] {
                let result = merged.merge(&other);
                if let Some(expected) = &expected {
                    result.unwrap();
                    assert_eq!(&merged, expected);
                    let mut snapshot = Acceptance::Accepted(a.clone());
                    snapshot.apply(&merged);
                    match expected {
                        Unchanged => assert_eq!(snapshot, Acceptance::Accepted(a.clone())),
                        Cleared => assert_eq!(snapshot, Acceptance::None),
                        Replaced(candidate) => {
                            assert_eq!(snapshot.candidate(), Some(candidate));
                        }
                    }
                } else {
                    assert!(result.is_err());
                }
            }
        }
    }
    #[test]
    fn sequential_effects_remain_explicit_even_when_replacement_equals_entry() {
        let a = candidate('a');
        let mut effect = AcceptanceTransition::Cleared;
        effect.then(&AcceptanceTransition::Replaced(a.clone()));
        effect.then(&AcceptanceTransition::Unchanged);
        assert_eq!(effect, AcceptanceTransition::Replaced(a));
    }
}
