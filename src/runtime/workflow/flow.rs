//! Activation proofs for a finite DAG. Joins wait for all incoming paths to
//! settle; only selected paths run. A producer is in scope exactly when it is
//! an ancestor and every activation of the consumer also activates it.
use super::{
    BTreeMap, BTreeSet, SchemaMap, Value, WorkflowBlock, WorkflowCompileError, WorkflowEdgeProgram,
    WorkflowNodeDefinition, WorkflowPort,
};
use biodivine_lib_bdd::{Bdd, BddVariableSet, op_function};

const MAX_ACTIVATION_NODES: usize = 65_536;

pub(super) struct ActivationScopes {
    active: BTreeMap<String, Bdd>,
    ancestors: BTreeMap<String, BTreeSet<String>>,
}
impl ActivationScopes {
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
                WorkflowNodeDefinition::Loop { .. } => 1,
                _ => 0,
            })
            .sum::<usize>();
        let variables = BddVariableSet::new_anonymous(count as u16);
        let mut allocated = variables.variables().into_iter();
        let mut gates = BTreeMap::new();
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
                WorkflowNodeDefinition::Loop { .. } => {
                    let selected = variables.mk_var(allocated.next().expect("allocated Loop"));
                    ports.insert(WorkflowPort::Satisfied, selected.clone());
                    ports.insert(WorkflowPort::Exhausted, selected.not());
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
        Ok(Self { active, ancestors })
    }
    pub(super) fn scope(
        &self,
        node: &str,
        after: &BTreeMap<String, SchemaMap>,
        input: &Value,
    ) -> SchemaMap {
        let mut scope = SchemaMap::from_schema(input);
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
                scope.0.insert(producer.clone(), schema.clone());
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
