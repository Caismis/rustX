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
                WorkflowNodeDefinition::Model {
                    on_error: WorkflowModelErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Tool {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                } => 1,
                _ => 0,
            })
            .sum::<usize>();
        let variables = BddVariableSet::new_anonymous(count as u16);
        let mut allocated = variables.variables().into_iter();
        let mut gates = BTreeMap::new();
        let mut succeeded = BTreeMap::new();
        for (id, node) in &block.nodes {
            let mut ports = BTreeMap::new();
            match node {
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
}
fn too_complex() -> WorkflowCompileError {
    WorkflowCompileError::InvalidField(
        "workflow activation proof exceeds its bounded complexity".into(),
    )
}
