//! The shared root and body executor. Native children settle inline.
use super::read_model::{WorkflowInstanceView, WorkflowState};
use super::{
    BTreeMap, MAX_VALUE_BYTES, MAX_WORKFLOW_AGENTS, MAX_WORKFLOW_MODEL_CALLS, MAX_WORKFLOW_STEPS,
    RuntimeEvent, Value, WorkflowBlockInstance, WorkflowBlockProgram, WorkflowDefinitionPath,
    WorkflowExecutionOutcome, WorkflowNodeInstance, WorkflowNodeProgram, WorkflowPort, WorkflowRun,
    WorkflowRunError, WorkflowRuntime, evaluate_predicate, evaluate_value,
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

/// An Iteration's value, as Dify's `output`: the rounds' values in order. A
/// round left out by `continue_on_error` is `null`, so its items are open;
/// flattening joins rounds that produced arrays.
pub(super) fn iteration_result_schema(
    produced: Option<&Value>,
    mode: super::WorkflowIterationErrorMode,
    flatten: bool,
) -> Value {
    let item = produced.and_then(|produced| {
        if flatten && super::schema_type(produced) == Some("array") {
            produced.get("items").cloned()
        } else {
            Some(produced.clone())
        }
    });
    let array = match item {
        Some(item) if mode != super::WorkflowIterationErrorMode::ContinueOnError => {
            serde_json::json!({"type": "array", "items": item})
        }
        _ => serde_json::json!({"type": "array"}),
    };
    serde_json::json!({"type": "object", "properties": {"output": array}, "required": ["output"], "additionalProperties": false})
}

/// Deliberate worst-case expansion, including round admission steps. Each
/// recursion is checked before multiplication, so overflow is not a bound.
/// An Iteration's rounds follow its array, so only the run's own limits
/// bound a program that has one.
pub(super) fn static_execution_bound(
    block: &WorkflowBlockProgram,
) -> Result<(usize, usize, usize), super::WorkflowCompileError> {
    let mut steps = block.nodes.len();
    let mut agents = 0;
    let mut model_calls = 0;
    for node in block.nodes.values() {
        match node {
            WorkflowNodeProgram::Agent(_) => agents += 1,
            // Every retry is one more call.
            WorkflowNodeProgram::Model(model) => {
                model_calls += 1 + model.retry.map_or(0, |retry| retry.max_retries as usize);
            }
            WorkflowNodeProgram::Loop {
                body, loop_count, ..
            } => {
                let (child_steps, child_agents, child_models) = static_execution_bound(body)?;
                steps += (child_steps + 1) * *loop_count as usize;
                agents += child_agents * *loop_count as usize;
                model_calls += child_models * *loop_count as usize;
            }
            WorkflowNodeProgram::Iteration { body, .. } => {
                let (_, child_agents, child_models) = static_execution_bound(body)?;
                steps = MAX_WORKFLOW_STEPS;
                if child_agents > 0 {
                    agents = MAX_WORKFLOW_AGENTS;
                }
                if child_models > 0 {
                    model_calls = MAX_WORKFLOW_MODEL_CALLS;
                }
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
    /// One more call of a node already admitted, for its retry.
    fn admit_model_retry(
        &mut self,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<(), WorkflowRunError> {
        if cancellation.is_cancelled() {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        }
        if self.model_calls + 1 > MAX_WORKFLOW_MODEL_CALLS {
            return Err(WorkflowRunError::LimitExceeded(
                super::WorkflowLimit::ModelCalls,
            ));
        }
        self.model_calls += 1;
        Ok(())
    }

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
    let mut bytes =
        schema_value_bound(&block.input_schema) + schema_value_bound(&block.output_schema);
    for node in block.nodes.values() {
        bytes = bytes.saturating_add(match node {
            WorkflowNodeProgram::Agent(agent) => schema_value_bound(&agent.value_schema()),
            WorkflowNodeProgram::Model(model) => schema_value_bound(&model.output.schema()),
            WorkflowNodeProgram::Tool { result, .. } => schema_value_bound(&result.schema()),
            WorkflowNodeProgram::Start
            | WorkflowNodeProgram::Branch { .. }
            | WorkflowNodeProgram::LoopEnd
            | WorkflowNodeProgram::Assigner { .. } => 0,
            WorkflowNodeProgram::Loop {
                body,
                output_schema,
                ..
            } => {
                // One live round, the variables it started from and the Loop's value.
                static_retained_bound(body) + 2 * schema_value_bound(output_schema)
            }
            WorkflowNodeProgram::Iteration {
                body,
                parallel,
                output_schema,
                ..
            } => (static_retained_bound(body) + MAX_VALUE_BYTES)
                .saturating_mul(*parallel as usize)
                .saturating_add(schema_value_bound(output_schema)),
            WorkflowNodeProgram::Review { actions, .. } => {
                schema_value_bound(&super::review::result_schema(actions))
            }
            WorkflowNodeProgram::Return { .. } => schema_value_bound(&block.output_schema),
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
    fn absorb(&mut self, mut other: Self) {
        self.bytes += other.bytes;
        other.bytes = 0;
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

/// A settled block: its variables, its Returns' values and whether an Exit
/// Loop node ran. The reservation holds the block's own locals.
pub(super) struct GraphOutcome<'a> {
    values: imbl::OrdMap<String, Value>,
    returned: BTreeMap<String, Value>,
    ended: bool,
    _reservation: LocalReservation<'a>,
}

impl WorkflowRuntime {
    /// The root block: the workflow's input in, its Return's output out.
    pub(super) async fn execute_block<'a>(
        &'a self,
        run: &'a WorkflowRun,
        block: &'a WorkflowBlockProgram,
        context: &'a crate::runtime::subagent::NativeExecutionContext,
        mut input: Value,
        invocations: Vec<u32>,
        cancellation: &'a crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<BlockOutput<'a>, WorkflowRunError> {
        super::apply_schema_defaults(&block.input_schema, &mut input);
        validate_commit(&block.input_schema, &input)?;
        let mut reservation = LocalReservation::new(run);
        reservation.retain(&input)?;
        let graph = self
            .execute_graph(
                run,
                block,
                context,
                &input,
                imbl::OrdMap::new(),
                invocations,
                cancellation,
            )
            .await?;
        if graph.returned.is_empty() {
            return Err(WorkflowRunError::InvalidProgram(
                "no active Return node".into(),
            ));
        }
        let mut result = serde_json::Map::new();
        for value in graph.returned.values() {
            let object = value.as_object().ok_or_else(|| {
                WorkflowRunError::InvalidValue("Return output must be an object".into())
            })?;
            for (key, value) in object {
                if result.get(key).is_some_and(|previous| previous != value) {
                    return Err(WorkflowRunError::InvalidValue(format!(
                        "active Return nodes conflict on output {key:?}"
                    )));
                }
                result.insert(key.clone(), value.clone());
            }
        }
        let value = Value::Object(result);
        validate_commit(&block.output_schema, &value)?;
        let mut exported = LocalReservation::new(run);
        exported.retain(&value)?;
        Ok(BlockOutput {
            value,
            _reservation: exported,
        })
    }

    /// Every block instance, root or body round, enters and exits here.
    /// Boxed only because bodies recursively invoke the same executor. A body
    /// starts from the variables around it and ends with every variable it
    /// set; its own locals retire with the returned reservation.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_graph<'a>(
        &'a self,
        run: &'a WorkflowRun,
        block: &'a WorkflowBlockProgram,
        context: &'a crate::runtime::subagent::NativeExecutionContext,
        input: &'a Value,
        values: imbl::OrdMap<String, Value>,
        invocations: Vec<u32>,
        cancellation: &'a crate::runtime::cancellation::ExecutionCancellation,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<GraphOutcome<'a>, WorkflowRunError>>
                + Send
                + 'a,
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
                values,
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

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn execute_block_body<'a>(
        &'a self,
        run: &'a WorkflowRun,
        block: &'a WorkflowBlockProgram,
        instance: &WorkflowBlockInstance,
        context: &'a crate::runtime::subagent::NativeExecutionContext,
        input: &'a Value,
        mut values: imbl::OrdMap<String, Value>,
        cancellation: &'a crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<GraphOutcome<'a>, WorkflowRunError> {
        use futures_util::{StreamExt, stream::FuturesUnordered};
        let mut reservation = LocalReservation::new(run);
        let mut waiting = block
            .nodes
            .keys()
            .map(|id| (id.clone(), 0usize))
            .collect::<BTreeMap<_, _>>();
        for edge in block.outgoing.values().flatten() {
            *waiting.get_mut(&edge.to).expect("compiled target") += 1;
        }
        let mut active = std::collections::BTreeSet::from([block.entry.clone()]);
        let mut ready = std::collections::BTreeSet::from([block.entry.clone()]);
        let mut pending = FuturesUnordered::new();
        let mut failures = BTreeMap::new();
        let mut returned = BTreeMap::new();
        let mut ended = false;
        loop {
            while failures.is_empty()
                && let Some(id) = ready.pop_first()
            {
                let scope = values.clone();
                pending.push(async move {
                    let result = self
                        .execute_node(
                            run,
                            block,
                            instance,
                            context,
                            input,
                            scope,
                            &id,
                            cancellation,
                        )
                        .await;
                    (id, result)
                });
            }
            let Some((id, settled)) = pending.next().await else {
                break;
            };
            match settled {
                Err(error) => {
                    failures.insert(id, error);
                }
                Ok(output) => {
                    let NodeCompletion {
                        value,
                        returned: result,
                        port,
                        ended: exit,
                        writes,
                        _reservation,
                    } = output;
                    reservation.absorb(_reservation);
                    if let Some(value) = value {
                        values.insert(id.clone(), value);
                    }
                    if let Some(value) = result {
                        returned.insert(id.clone(), value);
                    }
                    ended |= exit;
                    // Each write lands on the variable as it is now, so
                    // parallel Assigners keep each other's other writes;
                    // the last write to one variable wins, as in Dify.
                    for (owner, label, value) in writes {
                        if let Some(Value::Object(variables)) = values.get_mut(&owner) {
                            variables.insert(label, value);
                        }
                    }
                    // Resolve every edge, including inactive outcomes. A join is
                    // admitted once all incoming paths are settled, and runs once
                    // if at least one incoming path was selected.
                    let mut resolved = std::collections::VecDeque::from([(id, Some(port))]);
                    while let Some((from, port)) = resolved.pop_front() {
                        for edge in &block.outgoing[&from] {
                            if port.as_ref() == Some(&edge.port) {
                                active.insert(edge.to.clone());
                            }
                            let remaining = waiting.get_mut(&edge.to).expect("compiled target");
                            *remaining -= 1;
                            if *remaining == 0 {
                                if active.contains(&edge.to) {
                                    ready.insert(edge.to.clone());
                                } else {
                                    resolved.push_back((edge.to.clone(), None));
                                }
                            }
                        }
                    }
                }
            }
        }
        // FuturesUnordered is drained before propagating failure/cancellation;
        // every admitted native child keeps its physical settlement owner.
        if failures.len() == 1 {
            return Err(failures.into_values().next().expect("one failure"));
        }
        if !failures.is_empty() {
            return Err(WorkflowRunError::NodesFailed { failures });
        }
        if cancellation.is_cancelled() {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        }
        Ok(GraphOutcome {
            values,
            returned,
            ended,
            _reservation: reservation,
        })
    }

    /// Test-only pause before round `round` is admitted.
    #[cfg(test)]
    async fn round_frontier(&self, round: u32) {
        let hook = {
            let mut slot = self.iteration_frontier.lock().expect("iteration frontier");
            if slot.as_ref().is_some_and(|hook| hook.iteration == round) {
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

    /// Admit one Loop or Iteration round: one step, under the same lock
    /// that orders every other admission, then its block instance.
    fn admit_round(
        &self,
        run: &WorkflowRun,
        instance: &WorkflowBlockInstance,
        node: &WorkflowNodeInstance,
        body: &WorkflowBlockProgram,
        round: u32,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<(WorkflowBlockInstance, Vec<u32>), WorkflowRunError> {
        {
            let mut budgets = run.budgets.lock().expect("run budgets");
            budgets.admit(run.program.execution_bound, false, false, cancellation)?;
            self.read_model.update(&run.run_id, |view| {
                view.steps_consumed = budgets.steps;
                view.agents_consumed = budgets.agents;
                view.model_calls_consumed = budgets.model_calls;
                if let Some(row) = view
                    .instances
                    .iter_mut()
                    .find(|row| row.block == node.block && row.node.as_ref() == Some(&node.node))
                {
                    row.iteration = Some(round);
                }
            });
        }
        let mut invocations = instance.invocations.clone();
        invocations.push(round);
        let body_instance = WorkflowBlockInstance {
            run: run.run_id.clone(),
            definition: WorkflowDefinitionPath {
                workflow_id: run.program.id.clone(),
                blocks: body.path.clone(),
            },
            invocations: invocations.clone(),
        };
        self.emit_observability(
            run,
            RuntimeEvent::WorkflowLoopIterationAdmitted {
                node: node.clone(),
                body: body_instance.clone(),
                iteration: round,
            },
        );
        Ok((body_instance, invocations))
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn execute_node<'a>(
        &'a self,
        run: &'a WorkflowRun,
        block: &'a WorkflowBlockProgram,
        instance: &WorkflowBlockInstance,
        context: &'a crate::runtime::subagent::NativeExecutionContext,
        input: &'a Value,
        mut values: imbl::OrdMap<String, Value>,
        node_id: &str,
        cancellation: &'a crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<NodeCompletion<'a>, WorkflowRunError> {
        let mut reservation = LocalReservation::new(run);
        let mut selected = WorkflowPort::Next;
        let mut ended = false;
        let mut writes = Vec::new();
        context.check_workflow_deadline(cancellation);
        let node = &block.nodes[node_id];
        let node_instance = WorkflowNodeInstance {
            block: instance.clone(),
            node: node_id.to_owned(),
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
        let result = Box::pin(async {
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
                WorkflowNodeProgram::Start => Ok(None),
                WorkflowNodeProgram::Loop { loop_variables, break_condition, loop_count, body, output_schema } => {
                    let mut variables = serde_json::Map::new();
                    for variable in loop_variables {
                        variables.insert(variable.label.clone(), evaluate_value(&variable.value, input, &values)?);
                    }
                    let mut current = Value::Object(variables);
                    validate_commit(output_schema, &current)?;
                    let mut carried = LocalReservation::new(run);
                    carried.retain(&current)?;
                    values.insert(node_id.to_owned(), current.clone());
                    // As Dify, a condition on what no round has set yet does
                    // not break before the first round.
                    let breaks = |values: &imbl::OrdMap<String, Value>| break_condition.as_ref().map_or(Ok(false), |condition| evaluate_predicate(condition, input, values));
                    let mut status = breaks(&values).unwrap_or(false).then_some(super::WorkflowLoopExit::LoopBreak);
                    let mut round = 0;
                    while status.is_none() {
                        round += 1;
                        #[cfg(test)]
                        self.round_frontier(round).await;
                        let (body_instance, invocations) = self.admit_round(run, instance, &node_instance, body, round, cancellation)?;
                        let settled = self.execute_graph(run, body, context, input, values.clone(), invocations, cancellation).await;
                        self.emit_observability(run, RuntimeEvent::WorkflowLoopIterationSettled {
                            node: node_instance.clone(), body: body_instance, iteration: round, outcome: outcome(&settled),
                        });
                        let graph = settled?;
                        current = graph.values.get(node_id).cloned().ok_or_else(|| WorkflowRunError::InvalidProgram("Loop variables left the round".into()))?;
                        validate_commit(output_schema, &current)?;
                        let mut next = LocalReservation::new(run);
                        next.retain(&current)?;
                        carried = next;
                        if graph.ended || breaks(&graph.values)? {
                            status = Some(super::WorkflowLoopExit::LoopBreak);
                        } else if round == *loop_count {
                            status = Some(super::WorkflowLoopExit::LoopCompleted);
                        }
                        // The round's own locals retire here; its variables carry on.
                        drop(graph);
                        values.insert(node_id.to_owned(), current.clone());
                    }
                    let status = status.expect("a Loop ends with a reason");
                    reservation.retain(&current)?;
                    drop(carried);
                    if cancellation.is_cancelled() { return Err(WorkflowRunError::from_cancellation(cancellation)); }
                    self.emit_observability(run, RuntimeEvent::WorkflowLoopExited { node: node_instance.clone(), iterations: round, status });
                    self.read_model.node(&node_instance, |view| view.loop_exit = Some(status));
                    Ok(None)
                }
                WorkflowNodeProgram::Iteration { iterator, output, parallel, error_handle_mode, flatten_output, body, output_schema } => {
                    use futures_util::{StreamExt, stream::FuturesUnordered};
                    let Value::Array(items) = evaluate_value(iterator, input, &values)? else {
                        return Err(WorkflowRunError::InvalidValue("Iteration input must be an array".into()));
                    };
                    let mut rounds = FuturesUnordered::new();
                    let mut produced = BTreeMap::new();
                    let mut failure = None;
                    let mut next = 0;
                    loop {
                        while failure.is_none() && rounds.len() < *parallel as usize && next < items.len() {
                            let index = next;
                            next += 1;
                            let round = u32::try_from(index + 1).map_err(|_| WorkflowRunError::LimitExceeded(super::WorkflowLimit::Steps))?;
                            #[cfg(test)]
                            self.round_frontier(round).await;
                            let (body_instance, invocations) = match self.admit_round(run, instance, &node_instance, body, round, cancellation) {
                                Ok(admitted) => admitted,
                                Err(error) => { failure = Some(error); break; }
                            };
                            let mut scope = values.clone();
                            scope.insert(node_id.to_owned(), serde_json::json!({"item": items[index], "index": index}));
                            rounds.push(async move {
                                let settled = self.execute_graph(run, body, context, input, scope, invocations, cancellation).await;
                                let outcome = outcome(&settled);
                                (index, body_instance, outcome, settled.and_then(|graph| evaluate_value(output, input, &graph.values)))
                            });
                        }
                        let Some((index, body_instance, settled, result)) = rounds.next().await else { break };
                        self.emit_observability(run, RuntimeEvent::WorkflowLoopIterationSettled {
                            node: node_instance.clone(), body: body_instance, iteration: u32::try_from(index + 1).unwrap_or(u32::MAX), outcome: settled,
                        });
                        match result {
                            Ok(value) => { produced.insert(index, value); }
                            Err(error) if round_failed(&error) && !cancellation.is_cancelled() => match error_handle_mode {
                                super::WorkflowIterationErrorMode::Terminated => { failure.get_or_insert(error); }
                                super::WorkflowIterationErrorMode::ContinueOnError => { produced.insert(index, Value::Null); }
                                super::WorkflowIterationErrorMode::RemoveAbnormalOutput => {}
                            },
                            Err(error) => { failure.get_or_insert(error); }
                        }
                    }
                    if let Some(error) = failure { return Err(error); }
                    let outputs = flatten(produced.into_values().collect(), *flatten_output);
                    let value = serde_json::json!({"output": outputs});
                    validate_commit(output_schema, &value)?;
                    reservation.retain(&value)?;
                    if cancellation.is_cancelled() { return Err(WorkflowRunError::from_cancellation(cancellation)); }
                    values.insert(node_id.to_owned(), value);
                    Ok(None)
                }
                WorkflowNodeProgram::LoopEnd => {
                    ended = true;
                    Ok(None)
                }
                WorkflowNodeProgram::Assigner { items, types } => {
                    for (item, var_type) in items.iter().zip(types) {
                        let [owner, label] = item.variable.as_slice() else {
                            return Err(WorkflowRunError::InvalidProgram("assigned variable".into()));
                        };
                        // Earlier writes of this node are already visible to later ones.
                        let current = values.get(owner).and_then(|variables| variables.get(label)).cloned()
                            .ok_or_else(|| WorkflowRunError::InvalidValue(format!("Loop variable {label:?} is missing")))?;
                        let operand = item.value.as_ref().map(|value| evaluate_value(value, input, &values)).transpose()?;
                        let value = assign(item.operation, *var_type, &current, operand)?;
                        validate_commit(&var_type.schema(), &value)?;
                        if let Some(Value::Object(variables)) = values.get_mut(owner) {
                            variables.insert(label.clone(), value.clone());
                        }
                        writes.push((owner.clone(), label.clone(), value));
                    }
                    Ok(None)
                }
                WorkflowNodeProgram::Review { content: parts, actions, timeout_ms } => {
                    let shown = super::model::render_parts(parts, |part| evaluate_value(part, &input, &values))?;
                    let (value, port) = self
                        .review(context, &node_instance, shown, actions, *timeout_ms, cancellation)
                        .await?;
                    selected = port;
                    for successor in block.outgoing[node_id].iter().filter(|edge| edge.port == selected) {
                        self.emit_observability(run, RuntimeEvent::WorkflowBranchSelected {
                            workflow_id: run.program.id.clone(), run_id: run.run_id.clone(),
                            node_id: node_instance.clone(), port: selected.clone(), successor: successor.to.clone(),
                        });
                    }
                    reservation.retain(&value)?;
                    values.insert(node_id.to_owned(), value);
                    Ok(None)
                }
                WorkflowNodeProgram::Model(model) => {
                    let prompt = super::model::render_prompt(&model.messages, |part| evaluate_value(part, &input, &values))?;
                    let mut retries = 0;
                    let outcome = loop {
                        match self.invoke_model(run, context, &node_instance, model, prompt.clone(), cancellation).await {
                            Ok(value) => break Ok(value),
                            // Only a settled failure or timeout is handled;
                            // cancellation and unknown outcomes stop the run.
                            Err(WorkflowRunError::ModelFailed { status: status @ (crate::tools::types::ToolExecutionStatus::Failed { .. } | crate::tools::types::ToolExecutionStatus::TimedOut), .. })
                                if !cancellation.is_cancelled() =>
                            {
                                if let Some(retry) = model.retry && retries < retry.max_retries {
                                    retries += 1;
                                    tokio::select! {
                                        biased;
                                        () = cancellation.cancelled() => return Err(WorkflowRunError::from_cancellation(cancellation)),
                                        () = tokio::time::sleep(std::time::Duration::from_millis(retry.interval_ms.into())) => {}
                                    }
                                    run.budgets.lock().expect("run budgets").admit_model_retry(cancellation)?;
                                    continue;
                                }
                                break Err(status);
                            }
                            Err(error) => return Err(error),
                        }
                    };
                    let (value, schema) = match outcome {
                        Ok(value) => (value, model.output.schema()),
                        Err(status) => match &model.on_error {
                            super::WorkflowModelErrorStrategy::Stop => {
                                return Err(WorkflowRunError::ModelFailed { node: node_instance.to_string(), status });
                            }
                            super::WorkflowModelErrorStrategy::DefaultValue { text, structured_output } => {
                                let mut value = serde_json::json!({"text": text, "reasoning_content": "", "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}});
                                if let Some(structured) = structured_output {
                                    value["structured_output"] = structured.clone();
                                }
                                (value, model.output.schema())
                            }
                            super::WorkflowModelErrorStrategy::FailBranch => {
                                let (kind, message) = match status {
                                    crate::tools::types::ToolExecutionStatus::TimedOut => ("timed_out", "Model call timed out".to_owned()),
                                    crate::tools::types::ToolExecutionStatus::Failed { error } => ("failed", error),
                                    _ => unreachable!("only handled statuses reach the strategy"),
                                };
                                selected = WorkflowPort::Fail;
                                (serde_json::json!({"error_type": kind, "error_message": message}), super::WorkflowModelErrorStrategy::error_schema())
                            }
                        },
                    };
                    validate_commit(&schema, &value)?;
                    reservation.retain(&value)?;
                    #[cfg(test)]
                    if let Some(capability) = &context.workflow_model {
                        capability.frontier(super::model::ModelFrontier::BeforeCommit).await;
                    }
                    // Share the ancestor cancellation gate with the sole
                    // local value insertion. A late settled answer cannot
                    // become a successful node after cancellation won.
                    let _previous = cancellation.child_signal().with_active(|| values.insert(node_id.to_owned(), value))
                        .ok_or_else(|| WorkflowRunError::from_cancellation(cancellation))?;
                    Ok(None)
                }
                WorkflowNodeProgram::Tool {
                    selector,
                    arguments,
                    result,
                    retry,
                    on_error,
                } => {
                    let arguments = evaluate_value(arguments, &input, &values)?;
                    let mut retries = 0;
                    // Only a proven-settled failure or timeout is retried or
                    // handled; anything else is projected and stops as before.
                    let outcome = loop {
                        let (native, settled) = self
                            .invoke_tool(run, context, &node_instance, selector, arguments.clone(), retries, cancellation)
                            .await?;
                        let handled = settled && !cancellation.is_cancelled()
                            && matches!(native.status, crate::tools::types::ToolExecutionStatus::Failed { .. } | crate::tools::types::ToolExecutionStatus::TimedOut);
                        if !handled {
                            break Ok(native);
                        }
                        if let Some(retry) = retry && retries < retry.max_retries {
                            retries += 1;
                            tokio::select! {
                                biased;
                                () = cancellation.cancelled() => return Err(WorkflowRunError::from_cancellation(cancellation)),
                                () = tokio::time::sleep(std::time::Duration::from_millis(retry.interval_ms.into())) => {}
                            }
                            continue;
                        }
                        break if on_error.is_stop() { Ok(native) } else { Err(native.status) };
                    };
                    let (value, schema) = match (outcome, on_error) {
                        (Ok(native), _) => (result.project(&native, &node_instance)?, result.schema()),
                        (Err(_), super::WorkflowValueErrorStrategy::DefaultValue { value }) => (value.clone(), result.schema()),
                        (Err(status), _) => {
                            let (kind, message) = match status {
                                crate::tools::types::ToolExecutionStatus::Failed { error } => ("failed", super::bound_workflow_diagnostic(error)),
                                _ => ("timed_out", "Tool call timed out".to_owned()),
                            };
                            selected = WorkflowPort::Fail;
                            (serde_json::json!({"error_type": kind, "error_message": message}), super::WorkflowModelErrorStrategy::error_schema())
                        }
                    };
                    validate_commit(&schema, &value)?;
                    self.read_model.node(&node_instance, |view| {
                        view.checks_passed = value.get("passed").and_then(Value::as_bool);
                    });
                    reservation.retain(&value)?;
                    // Physical settlement precedes this sole local commit.
                    // The value reports what the leaf observed, not a
                    // durable certification of current workspace bytes.
                    values.insert(node_id.to_owned(), value);
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
                    // Only a proven-settled child's own failure or deadline is
                    // handled; cancellation, unknown outcomes and unproven
                    // termination stop the run.
                    let failure = match self.settle_agent(run, child, &node_instance, agent, cancellation).await {
                        Ok(value) => Ok(value),
                        Err(WorkflowRunError::ChildFailed { detail, .. }) if !agent.on_error.is_stop() && !cancellation.is_cancelled() => Err(("failed", detail)),
                        Err(WorkflowRunError::ChildOutcome { status: crate::tools::types::ToolExecutionStatus::TimedOut, .. })
                            if !agent.on_error.is_stop() && !cancellation.is_cancelled() => Err(("timed_out", "Agent run timed out".to_owned())),
                        Err(error) => return Err(error),
                    };
                    let (value, schema) = match (failure, &agent.on_error) {
                        (Ok(value), _) => (value, agent.value_schema()),
                        (Err(_), super::WorkflowValueErrorStrategy::DefaultValue { value }) => (value.clone(), agent.value_schema()),
                        (Err((kind, message)), _) => {
                            selected = WorkflowPort::Fail;
                            (serde_json::json!({"error_type": kind, "error_message": message}), super::WorkflowModelErrorStrategy::error_schema())
                        }
                    };
                    validate_commit(&schema, &value)?;
                    // As a Branch, a routed Agent records which exit it chose.
                    if selected != WorkflowPort::Fail && !agent.routes.is_empty() {
                        selected = super::WorkflowAgentProgram::chosen_route(&value);
                        for successor in block.outgoing[node_id].iter().filter(|edge| edge.port == selected) {
                            self.emit_observability(run, RuntimeEvent::WorkflowBranchSelected {
                                workflow_id: run.program.id.clone(), run_id: run.run_id.clone(),
                                node_id: node_instance.clone(), port: selected.clone(), successor: successor.to.clone(),
                            });
                        }
                    }
                    reservation.retain(&value)?;
                    values.insert(node_id.to_owned(), value);
                    Ok(None)
                }
                WorkflowNodeProgram::Branch { cases } => {
                    selected = WorkflowPort::False;
                    for case in cases {
                        if evaluate_predicate(&case.condition, &input, &values)? {
                            selected = WorkflowPort::case(&case.id);
                            break;
                        }
                    }
                    for successor in block.outgoing[node_id].iter().filter(|edge| edge.port == selected) {
                        self.emit_observability(run, RuntimeEvent::WorkflowBranchSelected {
                            workflow_id: run.program.id.clone(), run_id: run.run_id.clone(),
                            node_id: node_instance.clone(), port: selected.clone(), successor: successor.to.clone(),
                        });
                    }
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
            if let Some(value) = &returned { reservation.retain(value)?; }
            Ok(NodeCompletion { value: values.remove(node_id), returned, port: selected, ended, writes, _reservation: reservation })
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
        result
    }
}

struct NodeCompletion<'a> {
    value: Option<Value>,
    returned: Option<Value>,
    port: WorkflowPort,
    /// An Exit Loop node ran.
    ended: bool,
    /// Loop variables an Assigner wrote: owner, label and value.
    writes: Vec<(String, String, Value)>,
    _reservation: LocalReservation<'a>,
}

/// Whether an Iteration's error mode may handle a round's failure: a node
/// failed or timed out, never the run's cancellation, deadline or limits.
fn round_failed(error: &WorkflowRunError) -> bool {
    !matches!(
        error,
        WorkflowRunError::LimitExceeded(_)
            | WorkflowRunError::Deadline(_)
            | WorkflowRunError::Cancelled(_)
    ) && matches!(
        error.execution_status(),
        crate::tools::types::ToolExecutionStatus::Failed { .. }
            | crate::tools::types::ToolExecutionStatus::TimedOut
    )
}

/// As Dify: when every round that produced a value produced an array,
/// their items form one array.
fn flatten(outputs: Vec<Value>, flatten: bool) -> Vec<Value> {
    let produced = || outputs.iter().filter(|output| !output.is_null());
    if !flatten || produced().next().is_none() || !produced().all(Value::is_array) {
        return outputs;
    }
    outputs
        .into_iter()
        .flat_map(|output| match output {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            other => vec![other],
        })
        .collect()
}

/// One Variable Assigner write, as Dify's operations.
fn assign(
    operation: super::WorkflowWriteMode,
    var_type: super::WorkflowVarType,
    current: &Value,
    operand: Option<Value>,
) -> Result<Value, WorkflowRunError> {
    use super::WorkflowWriteMode as Mode;
    let number = |value: &Value| {
        value
            .as_f64()
            .ok_or_else(|| WorkflowRunError::InvalidValue("arithmetic needs numbers".into()))
    };
    let arithmetic = |apply: fn(f64, f64) -> Option<f64>| -> Result<Value, WorkflowRunError> {
        let operand = operand
            .as_ref()
            .ok_or_else(|| WorkflowRunError::InvalidProgram("operand".into()))?;
        let result = apply(number(current)?, number(operand)?)
            .filter(|result| result.is_finite())
            .ok_or_else(|| {
                WorkflowRunError::InvalidValue("the arithmetic has no finite result".into())
            })?;
        // Keep integers integral, as JSON numbers.
        #[allow(clippy::cast_possible_truncation)]
        Ok(if result.fract() == 0.0 && result.abs() < 9.0e15 {
            serde_json::json!(result as i64)
        } else {
            serde_json::json!(result)
        })
    };
    let array = || match current.clone() {
        Value::Array(items) => Ok(items),
        _ => Err(WorkflowRunError::InvalidValue(
            "the variable is not an array".into(),
        )),
    };
    Ok(match operation {
        Mode::Overwrite | Mode::Set => {
            operand.ok_or_else(|| WorkflowRunError::InvalidProgram("operand".into()))?
        }
        Mode::Clear => var_type.empty(),
        Mode::Append => {
            let mut items = array()?;
            items.push(operand.ok_or_else(|| WorkflowRunError::InvalidProgram("operand".into()))?);
            Value::Array(items)
        }
        Mode::Extend => {
            let mut items = array()?;
            match operand {
                Some(Value::Array(more)) => items.extend(more),
                _ => {
                    return Err(WorkflowRunError::InvalidValue(
                        "extend needs an array".into(),
                    ));
                }
            }
            Value::Array(items)
        }
        Mode::RemoveFirst => {
            let mut items = array()?;
            if !items.is_empty() {
                items.remove(0);
            }
            Value::Array(items)
        }
        Mode::RemoveLast => {
            let mut items = array()?;
            items.pop();
            Value::Array(items)
        }
        Mode::Increment => arithmetic(|left, right| Some(left + right))?,
        Mode::Decrement => arithmetic(|left, right| Some(left - right))?,
        Mode::Multiply => arithmetic(|left, right| Some(left * right))?,
        Mode::Divide => arithmetic(|left, right| (right != 0.0).then(|| left / right))?,
    })
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
                "input":{"type":"object"},"output":{"type":"object"},"nodes": {"workflow_start":{"type":"start"},"done":{"type":"return","output":{"type":"literal","value":{}}}},"edges": [{"from":"workflow_start","to":"done"}],}
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
