//! Inline single-use reference nodes without unfolding recursion or duplicating
//! shared schemas. Ordinary fields stay inline, as in the authored contract.

use std::collections::HashMap;

use referencing::Draft;
use serde_json::{Map, Value};

use super::{SchemaError, map_schema_children, schema_children};

pub(super) fn inline_single_use(
    root: &Value,
    definitions: &Map<String, Value>,
    draft: Draft,
) -> Result<(Value, Map<String, Value>), SchemaError> {
    let mut uses = HashMap::<String, usize>::new();
    let mut count = |reference: &str| {
        *uses.entry(reference.to_owned()).or_default() += 1;
    };
    visit_references(root, draft, &mut count);
    for node in definitions.values() {
        visit_references(node, draft, &mut count);
    }
    let root = expand(root, definitions, &uses, draft)?;
    let mut pending = Vec::new();
    let enqueue = |reference: &str, pending: &mut Vec<String>| {
        if let Some(name) = reference.strip_prefix("#/$defs/")
            && definitions.contains_key(name)
        {
            pending.push(name.to_owned());
        }
    };
    visit_references(&root, draft, &mut |reference| {
        enqueue(reference, &mut pending);
    });
    let mut retained = Map::new();
    while let Some(name) = pending.pop() {
        if retained.contains_key(&name) {
            continue;
        }
        let node = expand(&definitions[&name], definitions, &uses, draft)?;
        visit_references(&node, draft, &mut |reference| {
            enqueue(reference, &mut pending);
        });
        retained.insert(name, node);
    }
    Ok((root, retained))
}

fn visit_references(source: &Value, draft: Draft, visit: &mut impl FnMut(&str)) {
    if let Some(reference) = source.get("$ref").and_then(Value::as_str) {
        visit(reference);
    }
    // Schema-aware traversal must not interpret $ref inside const/enum/default
    // instance data as an edge in the resource graph.
    for child in schema_children(source, draft) {
        visit_references(child, draft, visit);
    }
}

fn expand(
    source: &Value,
    definitions: &Map<String, Value>,
    uses: &HashMap<String, usize>,
    draft: Draft,
) -> Result<Value, SchemaError> {
    if source.as_object().is_some_and(|object| object.len() == 1)
        && let Some(reference) = source.get("$ref").and_then(Value::as_str)
        && uses.get(reference) == Some(&1)
        && let Some(target) = reference
            .strip_prefix("#/$defs/")
            .and_then(|name| definitions.get(name))
    {
        // A reachable reference cycle has an extra incoming edge from its
        // entry (including root). Keeping multiply-used nodes breaks every
        // cycle and bounds expansion by the original graph's size.
        return expand(target, definitions, uses, draft);
    }
    let mut emitted = source.clone();
    map_schema_children(
        source,
        &mut emitted,
        &schema_children(source, draft),
        &mut |child| expand(child, definitions, uses, draft),
    )?;
    Ok(emitted)
}
