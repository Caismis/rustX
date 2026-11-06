//! Activation proofs for a finite DAG. Joins wait for all incoming paths to
//! settle; only selected paths run. A producer is in scope exactly when it is
//! an ancestor and every activation of the consumer also activates it.
use super::{
    BTreeMap, BTreeSet, SchemaMap, WorkflowBlock, WorkflowCompileError, WorkflowEdgeProgram,
    WorkflowModelErrorStrategy, WorkflowNodeDefinition, WorkflowPort, WorkflowValueErrorStrategy,
};
use biodivine_lib_bdd::{Bdd, BddVariableSet, op_function};

const MAX_ACTIVATION_NODES: usize = 65_536;

pub(super) struct ActivationScopes {
    active: BTreeMap<String, Bdd>,
    ancestors: BTreeMap<String, BTreeSet<String>>,
    /// For a Model, Tool or Agent with a fail branch: true exactly when it succeeded.
    succeeded: BTreeMap<String, Bdd>,
}
impl ActivationScopes {
    /// Two writers need an authored order unless their activations are disjoint.
    pub(super) fn unordered_together(&self, left: &str, right: &str) -> bool {
        !self.ancestors[left].contains(right)
            && !self.ancestors[right].contains(left)
            && Bdd::binary_op_with_limit(
                2,
                &self.active[left],
                &self.active[right],
                op_function::and,
            )
            .is_none_or(|overlap| !overlap.is_false())
    }
    #[allow(clippy::too_many_lines)] // One ordered pass builds every node's port gates.
    pub(super) fn new(
        block: &WorkflowBlock,
        entry: &str,
        outgoing: &BTreeMap<String, Vec<WorkflowEdgeProgram>>,
        order: &[String],
    ) -> Result<Self, WorkflowCompileError> {
        let count = block
            .nodes
            .values()
            .map(|node| match node {
                WorkflowNodeDefinition::Branch { cases } => cases.len(),
                // Each action, then the timeout that takes what remains.
                WorkflowNodeDefinition::Review { actions, .. } => actions.len(),
                // Routes are mutually exclusive: the last takes what remains.
                WorkflowNodeDefinition::Agent {
                    routes, on_error, ..
                } => {
                    routes.len().saturating_sub(1)
                        + usize::from(*on_error == WorkflowValueErrorStrategy::FailBranch)
                }
                WorkflowNodeDefinition::Classifier { classes, .. } => {
                    classes.len().saturating_sub(1)
                }
                WorkflowNodeDefinition::Model {
                    on_error: WorkflowModelErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Tool {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Code {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                } => 1,
                _ => 0,
            })
            .sum::<usize>();
        let variables =
            BddVariableSet::new_anonymous(u16::try_from(count).map_err(|_| too_complex())?);
        let mut allocated = variables.variables().into_iter();
        let mut gates = BTreeMap::new();
        let mut succeeded = BTreeMap::new();
        for (id, node) in &block.nodes {
            let mut ports = BTreeMap::new();
            match node {
                WorkflowNodeDefinition::Classifier { classes, .. } => {
                    let mut remaining = variables.mk_true();
                    for (index, class) in classes.iter().enumerate() {
                        let guard = if index + 1 == classes.len() {
                            remaining.clone()
                        } else {
                            let selected =
                                variables.mk_var(allocated.next().expect("allocated class"));
                            let guard = Bdd::binary_op_with_limit(
                                MAX_ACTIVATION_NODES,
                                &remaining,
                                &selected,
                                op_function::and,
                            )
                            .ok_or_else(too_complex)?;
                            remaining = Bdd::binary_op_with_limit(
                                MAX_ACTIVATION_NODES,
                                &remaining,
                                &selected.not(),
                                op_function::and,
                            )
                            .ok_or_else(too_complex)?;
                            guard
                        };
                        ports.insert(WorkflowPort::case(&class.id), guard);
                    }
                }
                WorkflowNodeDefinition::Branch { cases } => {
                    let mut remaining = variables.mk_true();
                    for case in cases {
                        let selected = variables.mk_var(allocated.next().expect("allocated case"));
                        let guard = Bdd::binary_op_with_limit(
                            MAX_ACTIVATION_NODES,
                            &remaining,
                            &selected,
                            op_function::and,
                        )
                        .ok_or_else(too_complex)?;
                        ports.insert(WorkflowPort::case(&case.id), guard);
                        remaining = Bdd::binary_op_with_limit(
                            MAX_ACTIVATION_NODES,
                            &remaining,
                            &selected.not(),
                            op_function::and,
                        )
                        .ok_or_else(too_complex)?;
                    }
                    ports.insert(WorkflowPort::False, remaining);
                }
                WorkflowNodeDefinition::Review { actions, .. } => {
                    let mut remaining = variables.mk_true();
                    for action in actions {
                        let selected =
                            variables.mk_var(allocated.next().expect("allocated action"));
                        let guard = Bdd::binary_op_with_limit(
                            MAX_ACTIVATION_NODES,
                            &remaining,
                            &selected,
                            op_function::and,
                        )
                        .ok_or_else(too_complex)?;
                        ports.insert(WorkflowPort::case(&action.id), guard);
                        remaining = Bdd::binary_op_with_limit(
                            MAX_ACTIVATION_NODES,
                            &remaining,
                            &selected.not(),
                            op_function::and,
                        )
                        .ok_or_else(too_complex)?;
                    }
                    ports.insert(WorkflowPort::case(super::REVIEW_TIMEOUT_PORT), remaining);
                }
                WorkflowNodeDefinition::Model {
                    on_error: WorkflowModelErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Tool {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Code {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                } => {
                    let ok = variables.mk_var(allocated.next().expect("allocated call outcome"));
                    ports.insert(WorkflowPort::Next, ok.clone());
                    ports.insert(WorkflowPort::Fail, ok.not());
                    succeeded.insert(id.clone(), ok);
                }
                WorkflowNodeDefinition::Agent {
                    routes, on_error, ..
                } => {
                    let ok = (*on_error == WorkflowValueErrorStrategy::FailBranch).then(|| {
                        variables.mk_var(allocated.next().expect("allocated Agent outcome"))
                    });
                    let mut remaining = ok.clone().unwrap_or_else(|| variables.mk_true());
                    for (index, route) in routes.iter().enumerate() {
                        let guard = if index + 1 == routes.len() {
                            remaining.clone()
                        } else {
                            let selected =
                                variables.mk_var(allocated.next().expect("allocated route"));
                            let guard = Bdd::binary_op_with_limit(
                                MAX_ACTIVATION_NODES,
                                &remaining,
                                &selected,
                                op_function::and,
                            )
                            .ok_or_else(too_complex)?;
                            remaining = Bdd::binary_op_with_limit(
                                MAX_ACTIVATION_NODES,
                                &remaining,
                                &selected.not(),
                                op_function::and,
                            )
                            .ok_or_else(too_complex)?;
                            guard
                        };
                        ports.insert(WorkflowPort::case(&route.id), guard);
                    }
                    if let Some(ok) = ok {
                        ports.insert(WorkflowPort::Next, ok.clone());
                        ports.insert(WorkflowPort::Fail, ok.not());
                        succeeded.insert(id.clone(), ok);
                    }
                }
                _ => {}
            }
            gates.insert(id.clone(), ports);
        }
        let mut active = BTreeMap::from([(entry.to_owned(), variables.mk_true())]);
        let mut ancestors: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for id in order {
            ancestors.entry(id.clone()).or_default();
            let current = active
                .get(id)
                .cloned()
                .unwrap_or_else(|| variables.mk_false());
            for edge in &outgoing[id] {
                let path = if let Some(gate) = gates[id].get(&edge.port) {
                    Bdd::binary_op_with_limit(
                        MAX_ACTIVATION_NODES,
                        &current,
                        gate,
                        op_function::and,
                    )
                } else {
                    Some(current.clone())
                }
                .ok_or_else(too_complex)?;
                let previous = active
                    .entry(edge.to.clone())
                    .or_insert_with(|| variables.mk_false());
                *previous = Bdd::binary_op_with_limit(
                    MAX_ACTIVATION_NODES,
                    previous,
                    &path,
                    op_function::or,
                )
                .ok_or_else(too_complex)?;
                let mut preceding = ancestors[id].clone();
                preceding.insert(id.clone());
                ancestors
                    .entry(edge.to.clone())
                    .or_default()
                    .extend(preceding);
            }
        }
        Ok(Self {
            active,
            ancestors,
            succeeded,
        })
    }
    /// A round completes after all selected paths settle, including an
    /// unconnected branch port. Its exports must exist on every activation.
    fn available_at_completion(&self, producer: &str) -> bool {
        self.active.get(producer).is_some_and(Bdd::is_true)
            && self.succeeded.get(producer).is_none_or(Bdd::is_true)
    }

    /// The virtual completion point is shared by compilation and authoring.
    pub(super) fn completion_scope(&self, declarations: &SchemaMap, base: &SchemaMap) -> SchemaMap {
        let mut scope = base.clone();
        for producer in self.active.keys() {
            if self.available_at_completion(producer)
                && let Some(schema) = declarations.0.get(producer)
            {
                scope.0.insert(producer.clone(), schema.clone());
            }
        }
        scope
    }

    pub(super) fn scope(
        &self,
        node: &str,
        after: &BTreeMap<String, SchemaMap>,
        base: &SchemaMap,
    ) -> SchemaMap {
        let mut scope = base.clone();
        for producer in &self.ancestors[node] {
            if Bdd::binary_op_with_limit(
                2,
                &self.active[node],
                &self.active[producer],
                op_function::imp,
            )
            .is_some_and(|proof| proof.is_true())
                && let Some(schema) = after.get(producer).and_then(|map| map.0.get(producer))
            {
                let implies = |outcome: &Bdd| {
                    Bdd::binary_op_with_limit(2, &self.active[node], outcome, op_function::imp)
                        .is_some_and(|proof| proof.is_true())
                };
                // A fail-branch node's value depends on which port was
                // taken: success fields only where success is proven, its
                // error only where failure is.
                match self.succeeded.get(producer) {
                    None => {
                        scope.0.insert(producer.clone(), schema.clone());
                    }
                    Some(ok) if implies(ok) => {
                        scope.0.insert(producer.clone(), schema.clone());
                    }
                    Some(ok) if implies(&ok.not()) => {
                        scope
                            .0
                            .insert(producer.clone(), WorkflowModelErrorStrategy::error_schema());
                    }
                    Some(_) => {}
                }
            }
        }
        scope
    }

    /// Aggregator selectors may name ancestors on exclusive paths. Only this
    /// node receives their possible declarations; ordinary reads remain strict.
    pub(super) fn potential_scope(
        &self,
        node: &str,
        after: &BTreeMap<String, SchemaMap>,
        base: &SchemaMap,
    ) -> SchemaMap {
        let mut scope = base.clone();
        for producer in &self.ancestors[node] {
            if let Some(schema) = after.get(producer).and_then(|s| s.0.get(producer)) {
                scope.0.insert(producer.clone(), schema.clone());
            }
        }
        scope
    }

    pub(super) fn validate_aggregate(
        &self,
        node: &str,
        paths: &[Vec<String>],
        base: &SchemaMap,
    ) -> Result<(), WorkflowCompileError> {
        let active = &self.active[node];
        let mut union = active.and(&active.not());
        for path in paths {
            let producer = path.first().ok_or_else(|| {
                WorkflowCompileError::InvalidReference("empty aggregate selector".into())
            })?;
            let present = if base.0.contains_key(producer) {
                active.clone()
            } else {
                if !self.ancestors[node].contains(producer) {
                    return Err(WorkflowCompileError::InvalidReference(
                        "aggregate source must precede the node".into(),
                    ));
                }
                let mut present = self.active[producer].clone();
                if let Some(ok) = self.succeeded.get(producer) {
                    present = Bdd::binary_op_with_limit(
                        MAX_ACTIVATION_NODES,
                        &present,
                        ok,
                        op_function::and,
                    )
                    .ok_or_else(too_complex)?;
                }
                Bdd::binary_op_with_limit(MAX_ACTIVATION_NODES, active, &present, op_function::and)
                    .ok_or_else(too_complex)?
            };
            union =
                Bdd::binary_op_with_limit(MAX_ACTIVATION_NODES, &union, &present, op_function::or)
                    .ok_or_else(too_complex)?;
        }
        if !Bdd::binary_op_with_limit(2, active, &union, op_function::imp)
            .is_some_and(|proof| proof.is_true())
        {
            return Err(WorkflowCompileError::InvalidReference(
                "aggregate sources must cover every activation".into(),
            ));
        }
        Ok(())
    }
}
fn too_complex() -> WorkflowCompileError {
    WorkflowCompileError::InvalidField(
        "workflow activation proof exceeds its bounded complexity".into(),
    )
}
