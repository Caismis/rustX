//! Executable reads, including reads inherited by nested bodies. Literal data
//! and schema metadata never create variable dependencies.
use super::{WorkflowNodeDefinition, WorkflowPredicate, WorkflowValue};

fn value<'a>(input: &'a WorkflowValue, reads: &mut Vec<&'a [String]>) {
    match input {
        WorkflowValue::Reference { path } => reads.push(path),
        WorkflowValue::Literal { .. } => {}
        WorkflowValue::Object { fields } => {
            for field in fields.values() {
                value(field, reads);
            }
        }
        WorkflowValue::Array { items } | WorkflowValue::Template { parts: items } => {
            for item in items {
                value(item, reads);
            }
        }
    }
}

fn predicate<'a>(input: &'a WorkflowPredicate, reads: &mut Vec<&'a [String]>) {
    match input {
        WorkflowPredicate::Boolean { value: input }
        | WorkflowPredicate::Empty { value: input }
        | WorkflowPredicate::NotEmpty { value: input } => value(input, reads),
        WorkflowPredicate::Equal { left, right }
        | WorkflowPredicate::NotEqual { left, right }
        | WorkflowPredicate::Greater { left, right }
        | WorkflowPredicate::GreaterEqual { left, right }
        | WorkflowPredicate::Less { left, right }
        | WorkflowPredicate::LessEqual { left, right }
        | WorkflowPredicate::Contains { left, right }
        | WorkflowPredicate::NotContains { left, right }
        | WorkflowPredicate::StartsWith { left, right }
        | WorkflowPredicate::EndsWith { left, right } => {
            value(left, reads);
            value(right, reads);
        }
        WorkflowPredicate::Not { predicate: input } => predicate(input, reads),
        WorkflowPredicate::And { predicates } | WorkflowPredicate::Or { predicates } => {
            for input in predicates {
                predicate(input, reads);
            }
        }
    }
}

fn list_reads<'a>(
    input: &'a WorkflowValue,
    filter: Option<&'a super::WorkflowListFilter>,
    extract: Option<&'a WorkflowValue>,
    reads: &mut Vec<&'a [String]>,
) {
    value(input, reads);
    if let Some(filter) = filter
        && !matches!(
            filter.operator,
            super::WorkflowListOperator::Empty | super::WorkflowListOperator::NotEmpty
        )
    {
        value(&filter.value, reads);
    }
    if let Some(extract) = extract {
        value(extract, reads);
    }
}

pub(super) fn node(input: &WorkflowNodeDefinition) -> Vec<&[String]> {
    let mut reads = Vec::new();
    match input {
        WorkflowNodeDefinition::Start {} | WorkflowNodeDefinition::LoopEnd {} => {}
        WorkflowNodeDefinition::Template { variables, .. }
        | WorkflowNodeDefinition::Code { variables, .. } => {
            for input in variables.values() {
                value(input, &mut reads);
            }
        }
        WorkflowNodeDefinition::List {
            input,
            filter,
            extract,
            ..
        } => list_reads(input, filter.as_ref(), extract.as_ref(), &mut reads),
        WorkflowNodeDefinition::Classifier {
            input,
            instruction,
            classes,
            ..
        } => {
            value(input, &mut reads);
            for part in instruction
                .iter()
                .chain(classes.iter().flat_map(|class| &class.description))
            {
                value(part, &mut reads);
            }
        }
        WorkflowNodeDefinition::Aggregate { groups, grouped } => {
            let active = super::transform::active_groups(groups, *grouped);
            for group in active {
                for path in &group.variables {
                    reads.push(path.as_slice());
                }
            }
        }
        WorkflowNodeDefinition::Model { messages, .. } => {
            for message in messages {
                for part in &message.content {
                    value(part, &mut reads);
                }
            }
        }
        WorkflowNodeDefinition::Agent { task, .. }
        | WorkflowNodeDefinition::Review { content: task, .. } => {
            for part in task {
                value(part, &mut reads);
            }
        }
        WorkflowNodeDefinition::Tool { arguments, .. } => value(arguments, &mut reads),
        WorkflowNodeDefinition::Branch { cases } => {
            for case in cases {
                predicate(&case.condition, &mut reads);
            }
        }
        WorkflowNodeDefinition::Assigner { items } => {
            for item in items {
                if let Some(input) = &item.value {
                    value(input, &mut reads);
                }
            }
        }
        WorkflowNodeDefinition::Return { outputs } => {
            for output in outputs {
                reads.push(output.value_selector.as_slice());
            }
        }
        WorkflowNodeDefinition::Loop {
            loop_variables,
            break_condition,
            body,
            ..
        } => {
            for variable in loop_variables {
                value(&variable.value, &mut reads);
            }
            if let Some(condition) = break_condition {
                predicate(condition, &mut reads);
            }
            for child in body.nodes.values() {
                reads.extend(node(child));
            }
        }
        WorkflowNodeDefinition::Iteration {
            iterator,
            output,
            body,
            ..
        } => {
            value(iterator, &mut reads);
            value(output, &mut reads);
            for child in body.nodes.values() {
                reads.extend(node(child));
            }
        }
    }
    reads
}
