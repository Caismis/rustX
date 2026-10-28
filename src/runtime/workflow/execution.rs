//! The single root/Parallel/Loop block executor. Native children settle inline.
use super::read_model::{WorkflowInstanceView, WorkflowState};
use super::{
    BTreeMap, MAX_VALUE_BYTES, MAX_WORKFLOW_AGENTS, MAX_WORKFLOW_MODEL_CALLS, MAX_WORKFLOW_STEPS,
    RuntimeEvent, Value, WorkflowBlockInstance, WorkflowBlockProgram, WorkflowDefinitionPath,
    WorkflowExecutionOutcome, WorkflowNodeInstance, WorkflowNodeProgram, WorkflowPort, WorkflowRun,
    WorkflowRunError, WorkflowRuntime, evaluate_predicate, evaluate_value, single_successor,
};

#[cfg(test)]
pub(super) struct NodeFrontierHook {
    pub node: String,
    pub entered: tokio::sync::oneshot::Sender<()>,
    pub release: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(test)]
pub(super) struct IterationFrontierHook {
    pub iteration: u32,
    pub entered: tokio::sync::oneshot::Sender<()>,
    pub release: tokio::sync::oneshot::Receiver<()>,
}

pub(super) fn loop_result_schema(body: &Value) -> Value {
    serde_json::json!({"type":"object", "properties": {
        "status":{"type":"string","enum":["satisfied","exhausted"]},
        "iterations":{"type":"integer"}, "result":body
    }, "required":["status","iterations","result"], "additionalProperties":false})
}

/// Deliberate worst-case expansion, including iteration admission steps. Each
/// recursion is checked before multiplication, so overflow is not a bound.
pub(super) fn static_execution_bound(
    block: &WorkflowBlockProgram,
) -> Result<(usize, usize, usize), super::WorkflowCompileError> {
    let mut steps = block.nodes.len();
    let mut agents = 0;
    let mut model_calls = 0;
    for node in block.nodes.values() {
        match node {
            WorkflowNodeProgram::Agent(_) => agents += 1,
            WorkflowNodeProgram::Model(_) => model_calls += 1,
            WorkflowNodeProgram::Parallel { branches, .. } => {
                for branch in branches.values() {
                    let (child_steps, child_agents, child_models) =
                        static_execution_bound(&branch.block)?;
                    steps += child_steps;
                    agents += child_agents;
                    model_calls += child_models;
                }
            }
            WorkflowNodeProgram::Loop {
                body,
                max_iterations,
                ..
            } => {
                let (child_steps, child_agents, child_models) = static_execution_bound(body)?;
                steps += (child_steps + 1) * *max_iterations as usize;
                agents += child_agents * *max_iterations as usize;
                model_calls += child_models * *max_iterations as usize;
            }
            _ => {}
        }
        if steps > MAX_WORKFLOW_STEPS
            || agents > MAX_WORKFLOW_AGENTS
            || model_calls > MAX_WORKFLOW_MODEL_CALLS
        {
            return Err(super::WorkflowCompileError::InvalidField(
                "aggregate expanded execution bound exceeded".into(),
            ));
        }
    }
    Ok((steps, agents, model_calls))
}

#[derive(Debug, Default)]
pub(super) struct RunBudgets {
    steps: usize,
    agents: usize,
    model_calls: usize,
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

    /// Called only under the run mutex. A successful final cancellation read
    /// commits the reservation; its counter installation cannot fail or await.
    fn admit(
        &mut self,
        bound: usize,
        agent: bool,
        model: bool,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<(), WorkflowRunError> {
        if cancellation.is_cancelled() {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        }
        if self.steps >= bound || self.steps >= MAX_WORKFLOW_STEPS {
            return Err(WorkflowRunError::LimitExceeded(super::WorkflowLimit::Steps));
        }
        let agent = usize::from(agent);
        let model = usize::from(model);
        if self.model_calls + model > MAX_WORKFLOW_MODEL_CALLS {
            return Err(WorkflowRunError::LimitExceeded(
                super::WorkflowLimit::ModelCalls,
            ));
        }
        if self.agents + agent > MAX_WORKFLOW_AGENTS {
            return Err(WorkflowRunError::LimitExceeded(
                super::WorkflowLimit::Agents,
            ));
        }
        if cancellation.is_cancelled() {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        }
        self.steps += 1;
        self.agents += agent;
        self.model_calls += model;
        Ok(())
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
            WorkflowNodeProgram::Model(model) => schema_value_bound(&model.output.schema()),
            WorkflowNodeProgram::Tool { result, .. } => schema_value_bound(&result.schema()),
            WorkflowNodeProgram::Branch { .. } => 0,
            WorkflowNodeProgram::Loop {
                body,
                output_schema,
                ..
            } => {
                // One live body, one explicit carry plus transfer overlap; no history.
                static_retained_bound(body)
                    + 2 * schema_value_bound(&body.input_schema)
                    + schema_value_bound(output_schema)
            }
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
            return Err(WorkflowRunError::LimitExceeded(
                super::WorkflowLimit::RetainedData,
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

pub(super) struct BlockOutput<'a> {
    pub(super) value: Value,
    _reservation: LocalReservation<'a>,
}

impl WorkflowRuntime {
    /// Every lexical scope, including root, enters and exits here. Boxed only
    /// because fixed nested blocks recursively invoke the same executor.
    pub(super) fn execute_block<'a>(
        &'a self,
        run: &'a WorkflowRun,
        block: &'a WorkflowBlockProgram,
        context: &'a crate::runtime::subagent::NativeExecutionContext,
        input: Value,
        invocations: Vec<u32>,
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
                invocations,
            };
            self.read_model.block(&run.run_id, &instance);
            self.emit_observability(
                run,
                RuntimeEvent::WorkflowBlockStarted {
                    instance: instance.clone(),
                },
            );
            let result = Box::pin(self.execute_block_body(
                run,
                block,
                &instance,
                context,
                input,
                cancellation,
            ))
            .await;
            self.read_model.update(&run.run_id, |view| {
                if let Some(view) = view
                    .instances
                    .iter_mut()
                    .find(|view| view.block == instance && view.node.is_none())
                {
                    view.state = WorkflowState::Settled {
                        outcome: outcome(&result),
                    };
                }
            });
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

    #[allow(clippy::too_many_lines)]
    async fn execute_block_body<'a>(
        &'a self,
        run: &'a WorkflowRun,
        block: &'a WorkflowBlockProgram,
        instance: &WorkflowBlockInstance,
        context: &'a crate::runtime::subagent::NativeExecutionContext,
        input: Value,
        cancellation: &'a crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<BlockOutput<'a>, WorkflowRunError> {
        validate_commit(&block.input_schema, &input)?;
        let mut reservation = LocalReservation::new(run);
        reservation.retain(&input)?;
        let mut values = BTreeMap::new();
        let mut node_id = block.entry.clone();
        loop {
            context.check_workflow_deadline(cancellation);
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
            let mut started = false;
            let mut effect_started = false;
            let result: Result<Option<BlockOutput<'_>>, WorkflowRunError> = Box::pin(async {
                // Node admission frontier: cancellation observation and aggregate
                // count reservation are synchronous, before any child preparation.
                {
                    let mut budgets = run.budgets.lock().expect("run budgets");
                    budgets.admit(run.program.execution_bound, matches!(node, WorkflowNodeProgram::Agent(_)), matches!(node, WorkflowNodeProgram::Model(_)), cancellation)?;
                    // Both locks belong to Workflow; no callback or await enters
                    // another owner. Admission and its bounded native cut commit
                    // together before the first poll of any descendant work.
                    self.read_model.update(&run.run_id, |view| {
                        view.steps_consumed = budgets.steps;
                        view.agents_consumed = budgets.agents;
                        view.model_calls_consumed = budgets.model_calls;
                        // Append only after admission, under the same lock that
                        // orders parallel starts. Later state updates keep this
                        // position, including across nested blocks and loops.
                        view.instances.push(WorkflowInstanceView::started(&node_instance, node));
                    });
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
                        | WorkflowNodeProgram::Model(_)
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
                if let Some(effects)=&run.effects && let Some(kind)=super::scheduled::effects::ScheduledEffects::kind(node) {
                    effects.begin(&node_instance,kind)?;
                    effect_started=true;
                }
                match node {
                    WorkflowNodeProgram::Loop { input: initial, body, until, carry, max_iterations, output_schema } => {
                        let mut carried = evaluate_value(initial, &input, &values)?;
                        let mut carried_reservation = LocalReservation::new(run);
                        carried_reservation.retain(&carried)?;
                        for iteration in 1..=*max_iterations {
                            validate_commit(&body.input_schema, &carried)?;
                            #[cfg(test)]
                            {
                                let hook = {
                                    let mut slot = self.iteration_frontier.lock().expect("iteration frontier");
                                    if slot.as_ref().is_some_and(|hook| hook.iteration == iteration) { slot.take() } else { None }
                                };
                                if let Some(hook) = hook {
                                    let _ = hook.entered.send(());
                                    let _ = hook.release.await;
                                }
                            }
                            // Reservation, cancellation observation, admission and
                            // consumption are one synchronous decision. No failed
                            // reservation or detached continuation can dispatch.
                            {
                                let mut budgets = run.budgets.lock().expect("run budgets");
                                budgets.admit(run.program.execution_bound, false, false, cancellation)?;
                                self.read_model.update(&run.run_id, |view| {
                                    view.steps_consumed = budgets.steps;
                                    view.agents_consumed = budgets.agents;
                                    view.model_calls_consumed = budgets.model_calls;
                                    if let Some(row) = view.instances.iter_mut().find(|row| row.block == node_instance.block && row.node.as_ref() == Some(&node_instance.node)) {
                                        row.iteration = Some(iteration);
                                    }
                                });
                            }
                            let mut invocations = instance.invocations.clone();
                            invocations.push(iteration);
                            let body_instance = WorkflowBlockInstance {
                                run: run.run_id.clone(),
                                definition: WorkflowDefinitionPath { workflow_id: run.program.id.clone(), blocks: body.path.clone() },
                                invocations: invocations.clone(),
                            };
                            self.emit_observability(run, RuntimeEvent::WorkflowLoopIterationAdmitted {
                                node: node_instance.clone(), body: body_instance.clone(), iteration,
                            });
                            let settled = self.execute_block(run, body, context, carried, invocations, cancellation).await;
                            self.emit_observability(run, RuntimeEvent::WorkflowLoopIterationSettled {
                                node: node_instance.clone(), body: body_instance, iteration, outcome: outcome(&settled),
                            });
                            // The same block engine has validated/exported the body
                            // output and finalized every native owner before returning.
                            drop(carried_reservation);
                            let BlockOutput { value, _reservation: _result_reservation } = settled?;
                            let scope = BTreeMap::from([("result".into(), value)]);
                            let satisfied = evaluate_predicate(until, &Value::Null, &scope)?;
                            if satisfied || iteration == *max_iterations {
                                let status = if satisfied { super::WorkflowLoopExit::Satisfied } else { super::WorkflowLoopExit::Exhausted };
                                let value = serde_json::json!({"status":status,"iterations":iteration,"result":scope["result"]});
                                validate_commit(output_schema, &value)?;
                                reservation.retain(&value)?;
                                // Exit commits only the structural result, never a bare body value.
                                if cancellation.is_cancelled() { return Err(WorkflowRunError::from_cancellation(cancellation)); }
                                values.insert(node_id.clone(), value);
                                self.emit_observability(run, RuntimeEvent::WorkflowLoopExited { node: node_instance.clone(), iterations: iteration, status });
                                self.read_model.node(&node_instance, |view| view.loop_exit = Some(status));
                                let port = match status {
                                    super::WorkflowLoopExit::Satisfied => WorkflowPort::Satisfied,
                                    super::WorkflowLoopExit::Exhausted => WorkflowPort::Exhausted,
                                };
                                let successor = block.outgoing[&node_id].iter()
                                    .find(|edge| edge.port == port)
                                    .ok_or_else(|| WorkflowRunError::InvalidProgram(node_id.clone()))?
                                    .to.clone();
                                node_id = successor;
                                return Ok(None);
                            }
                            let next = evaluate_value(carry, &Value::Null, &scope)?;
                            validate_commit(&body.input_schema, &next)?;
                            let mut next_reservation = LocalReservation::new(run);
                            next_reservation.retain(&next)?;
                            if cancellation.is_cancelled() { return Err(WorkflowRunError::from_cancellation(cancellation)); }
                            // Atomic carry commit after full validation and byte reservation.
                            carried = next;
                            carried_reservation = next_reservation;
                            // Private body locals are already retired; only this carry survives.
                        }
                        unreachable!("compiled positive finite Loop")
                    }
                    WorkflowNodeProgram::Review {
                        subject,
                        context: checks,
                    } => {
                        let bound = evaluate_value(subject.value(), &input, &values)?;
                        let checks = checks
                            .iter()
                            .map(|check| evaluate_value(check, &input, &values))
                            .collect::<Result<Vec<_>, _>>()?;
                        // The decision is ordinary structured data. Only an
                        // explicit downstream Branch on `accepted` gates work.
                        let value = self
                            .review(context, &node_instance, bound, checks, cancellation)
                            .await?;
                        reservation.retain(&value)?;
                        values.insert(node_id.clone(), value);
                        Ok(None)
                    }

                    WorkflowNodeProgram::Model(model) => {
                        let bindings = model.input.iter().map(|(key, expression)| {
                            evaluate_value(expression, &input, &values).map(|value| (key.clone(), value))
                        }).collect::<Result<serde_json::Map<String, Value>, _>>()?;
                        let value = self.invoke_model(run, context, &node_instance, model, Value::Object(bindings), cancellation).await?;
                        validate_commit(&model.output.schema(), &value)?;
                        reservation.retain(&value)?;
                        #[cfg(test)]
                        if let Some(capability) = &context.workflow_model {
                            capability.frontier(super::model::ModelFrontier::BeforeCommit).await;
                        }
                        // Share the ancestor cancellation gate with the sole
                        // local value insertion. A late settled answer cannot
                        // become a successful node after cancellation won.
                        let _previous = cancellation.child_signal().with_active(|| values.insert(node_id.clone(), value))
                            .ok_or_else(|| WorkflowRunError::from_cancellation(cancellation))?;
                        Ok(None)
                    }
                    WorkflowNodeProgram::Tool {
                        selector,
                        arguments,
                        result,
                    } => {
                        let arguments = evaluate_value(arguments, &input, &values)?;
                        let native = self
                            .invoke_tool(run, context, &node_instance, selector, arguments, cancellation)
                            .await?;
                        let value = result.project(&native, &node_instance)?;
                        self.read_model.node(&node_instance, |view| {
                            view.checks_passed = value.get("passed").and_then(Value::as_bool);
                        });
                        reservation.retain(&value)?;
                        // Physical settlement precedes this sole local commit.
                        // The value reports what the leaf observed, not a
                        // durable certification of current workspace bytes.
                        values.insert(node_id.clone(), value);
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
                            )
                            .await?;
                        let value = self
                            .settle_agent(run, child, &node_instance, &agent.output_schema, cancellation)
                            .await?;
                        reservation.retain(&value)?;
                        values.insert(node_id.clone(), value);
                        Ok(None)
                    }
                    WorkflowNodeProgram::Branch { condition } => {
                        let port = if evaluate_predicate(condition, &input, &values)? {
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
                            async move {
                                let bound = evaluate_value(&branch.input, input, values);
                                let result = match bound {
                                    Ok(input) => {
                                        self.execute_block(
                                            run,
                                            &branch.block,
                                            context,
                                            input,
                                            { let mut path = instance.invocations.clone(); path.push(0); path },
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
                        let mut failures = BTreeMap::new();
                        for (key, result) in &settled {
                            match result {
                                Ok(output) => {
                                    results.insert((*key).clone(), output.value.clone());
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
                        let value = Value::Object(results);
                        validate_commit(output_schema, &value)?;
                        // Transfer exported output accounting before releasing
                        // branch outputs. Private locals were already retired.
                        reservation.retain(&value)?;
                        drop(settled);
                        values.insert(node_id.clone(), value);
                        Ok(None)
                    }
                    WorkflowNodeProgram::Return { output } => {
                        let value = evaluate_value(output, &input, &values)?;
                        validate_commit(&block.output_schema, &value)?;
                        Ok(Some(value))
                    }
                }
            })
            .await
            .and_then(|returned| {
                returned
                    .map(|value| {
                        let mut exported = LocalReservation::new(run);
                        exported.retain(&value)?;
                        Ok(BlockOutput {
                            value,
                            _reservation: exported,
                        })
                    })
                    .transpose()
            });
            if effect_started && let Some(effects) = &run.effects {
                effects.finish(&node_instance)?;
            }
            if started {
                self.read_model.node(&node_instance, |view| {
                    view.state = WorkflowState::Settled {
                        outcome: outcome(&result),
                    }
                });
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
            if !matches!(
                node,
                WorkflowNodeProgram::Branch { .. } | WorkflowNodeProgram::Loop { .. }
            ) {
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

pub(super) fn outcome<T>(result: &Result<T, WorkflowRunError>) -> WorkflowExecutionOutcome {
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
    use std::sync::Arc;

    #[test]
    fn model_call_limit_is_independent_of_agent_and_step_allowances() {
        let signal = crate::runtime::cancellation::CancellationSignal::new();
        let cancellation = crate::runtime::cancellation::ExecutionCancellation::detached(
            signal,
            crate::runtime::types::CancellationReason::UserRequested,
        );
        let mut budgets = RunBudgets::default();
        for _ in 0..MAX_WORKFLOW_MODEL_CALLS {
            budgets
                .admit(MAX_WORKFLOW_STEPS, false, true, &cancellation)
                .unwrap();
        }
        assert_eq!(budgets.agents, 0);
        let before = (budgets.steps, budgets.agents, budgets.model_calls);
        assert_eq!(
            budgets.admit(MAX_WORKFLOW_STEPS, false, true, &cancellation),
            Err(WorkflowRunError::LimitExceeded(
                super::super::WorkflowLimit::ModelCalls
            ))
        );
        assert_eq!((budgets.steps, budgets.agents, budgets.model_calls), before);
        budgets
            .admit(MAX_WORKFLOW_STEPS, true, false, &cancellation)
            .unwrap();
        assert_eq!(
            (budgets.agents, budgets.model_calls),
            (1, MAX_WORKFLOW_MODEL_CALLS)
        );
    }

    #[test]
    fn shared_admission_counts_agents_separately_and_rejection_commits_nothing() {
        let signal = crate::runtime::cancellation::CancellationSignal::new();
        let cancellation = crate::runtime::cancellation::ExecutionCancellation::detached(
            signal.clone(),
            crate::runtime::types::CancellationReason::UserRequested,
        );
        let mut budgets = RunBudgets::default();
        for _ in 0..MAX_WORKFLOW_AGENTS {
            budgets
                .admit(MAX_WORKFLOW_STEPS, true, false, &cancellation)
                .unwrap();
        }
        budgets
            .admit(MAX_WORKFLOW_STEPS, false, false, &cancellation)
            .unwrap(); // an iteration is not an Agent
        let before = (budgets.steps, budgets.agents);
        assert_eq!(
            budgets.admit(MAX_WORKFLOW_STEPS, true, false, &cancellation),
            Err(WorkflowRunError::LimitExceeded(
                super::super::WorkflowLimit::Agents
            ))
        );
        assert_eq!((budgets.steps, budgets.agents), before);
        signal.cancel();
        assert!(
            budgets
                .admit(MAX_WORKFLOW_STEPS, false, false, &cancellation)
                .unwrap_err()
                .is_cancelled()
        );
        assert_eq!((budgets.steps, budgets.agents), before);
    }

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
            WorkflowProgram::compile(WorkflowId::parse("budget").unwrap(), definition).unwrap(),
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
        assert_eq!(
            nested.retain(&value),
            Err(WorkflowRunError::LimitExceeded(
                super::super::WorkflowLimit::RetainedData
            )),
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
