//! Compile the immutable schema vocabulary once; values never enter this plan.
use super::{Value, resolve};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Unsigned {
    Plain,
    Decimal,
    Safe,
}

struct Property {
    name: String,
    node: usize,
    // The original tag selector inspects the property's own const, not a ref.
    constant: Option<Value>,
}
struct Node {
    unsigned: Unsigned,
    signed_safe: bool,
    properties: Vec<Property>,
    // Sorted indexes preserve the schema's serialization-order vector while
    // map/fallback/tag lookups avoid rescanning a wide DTO's property names.
    property_names: Vec<usize>,
    additional: Option<usize>,
    items: Option<usize>,
    items_object: bool,
    prefix: Vec<usize>,
    branches: [Vec<usize>; 3],
}
impl Default for Node {
    fn default() -> Self {
        Self {
            unsigned: Unsigned::Plain,
            signed_safe: false,
            properties: Vec::new(),
            property_names: Vec::new(),
            additional: None,
            items: None,
            items_object: false,
            prefix: Vec::new(),
            branches: std::array::from_fn(|_| Vec::new()),
        }
    }
}

impl Node {
    fn property(&self, key: &str) -> Option<&Property> {
        let index = self
            .property_names
            .binary_search_by(|index| self.properties[*index].name.as_str().cmp(key))
            .ok()?;
        self.properties.get(self.property_names[index])
    }
}

pub(super) struct Plan {
    nodes: Vec<Node>,
    addresses: HashMap<usize, usize>,
}
impl Plan {
    pub(super) fn new(schema: &Value, root: &Value) -> Self {
        let mut plan = Self {
            // Node zero is the opaque schema, including missing properties.
            nodes: vec![Node::default()],
            addresses: HashMap::new(),
        };
        plan.compile(schema, root);
        plan
    }
    pub(super) fn select(&self, schema: &Value) -> Option<View<'_>> {
        let index = if schema.is_null() {
            0
        } else {
            *self.addresses.get(&(std::ptr::from_ref(schema) as usize))?
        };
        Some(View::Compiled(self, index))
    }
    fn compile(&mut self, schema: &Value, root: &Value) -> usize {
        if schema.is_null() {
            return 0;
        }
        let address = std::ptr::from_ref(schema) as usize;
        if let Some(index) = self.addresses.get(&address) {
            return *index;
        }
        let schema = resolve(schema, root);
        if schema.is_null() {
            // A reference to null is an opaque, but syntactically present,
            // selection. branch_child stops at that reference, whereas a
            // literal null permits the following branch to supply a field.
            let index = self.nodes.len();
            self.nodes.push(Node::default());
            self.addresses.insert(address, index);
            return index;
        }
        let target = std::ptr::from_ref(schema) as usize;
        if let Some(index) = self.addresses.get(&target).copied() {
            self.addresses.insert(address, index);
            return index;
        }
        let index = self.nodes.len();
        self.addresses.insert(address, index);
        self.addresses.insert(target, index);
        // Reserve before following children: generated recursive DTO refs point
        // back to this node, not to a recursively cloned schema tree.
        self.nodes.push(Node::default());
        let unsigned = match schema["format"].as_str() {
            Some("uint64")
                if schema["maximum"]
                    .as_f64()
                    .is_none_or(|maximum| maximum > 9_007_199_254_740_991.0) =>
            {
                Unsigned::Decimal
            }
            Some("uint" | "uint64") => Unsigned::Safe,
            _ => Unsigned::Plain,
        };
        let properties: Vec<Property> =
            schema["properties"]
                .as_object()
                .map_or_else(Vec::new, |fields| {
                    fields
                        .iter()
                        .map(|(name, value)| Property {
                            name: name.clone(),
                            node: self.compile(value, root),
                            constant: value.get("const").cloned(),
                        })
                        .collect()
                });
        let mut property_names: Vec<_> = (0..properties.len()).collect();
        property_names
            .sort_unstable_by(|left, right| properties[*left].name.cmp(&properties[*right].name));
        let additional = schema
            .get("additionalProperties")
            .filter(|value| value.is_object())
            .map(|value| self.compile(value, root));
        let items = schema.get("items").map(|value| self.compile(value, root));
        let prefix = schema["prefixItems"]
            .as_array()
            .map_or_else(Vec::new, |values| {
                values
                    .iter()
                    .map(|value| self.compile(value, root))
                    .collect()
            });
        let branches = ["oneOf", "anyOf", "allOf"].map(|keyword| {
            schema[keyword].as_array().map_or_else(Vec::new, |values| {
                values
                    .iter()
                    .map(|value| self.compile(value, root))
                    .collect()
            })
        });
        self.nodes[index] = Node {
            unsigned,
            signed_safe: matches!(schema["format"].as_str(), Some("int" | "int64")),
            properties,
            property_names,
            additional,
            items,
            items_object: schema["items"].is_object(),
            prefix,
            branches,
        };
        index
    }
}

#[derive(Clone, Copy)]
pub(super) enum View<'a> {
    Compiled(&'a Plan, usize),
    // The pre-change schema interpreter is a differential oracle, never a
    // runtime fallback or a second encoding format.
    #[cfg(test)]
    Interpreted {
        schema: &'a Value,
        root: &'a Value,
    },
}
impl<'a> View<'a> {
    pub(super) fn is_null(self) -> bool {
        match self {
            Self::Compiled(_, index) => index == 0,
            #[cfg(test)]
            Self::Interpreted { schema, .. } => schema.is_null(),
        }
    }
    pub(super) fn unsigned(self) -> Unsigned {
        match self {
            Self::Compiled(plan, index) => plan.nodes[index].unsigned,
            #[cfg(test)]
            Self::Interpreted { schema, root } => {
                let schema = resolve(schema, root);
                match schema["format"].as_str() {
                    Some("uint64")
                        if schema["maximum"]
                            .as_f64()
                            .is_none_or(|v| v > 9_007_199_254_740_991.0) =>
                    {
                        Unsigned::Decimal
                    }
                    Some("uint" | "uint64") => Unsigned::Safe,
                    _ => Unsigned::Plain,
                }
            }
        }
    }
    pub(super) fn signed_safe(self) -> bool {
        match self {
            Self::Compiled(plan, index) => plan.nodes[index].signed_safe,
            #[cfg(test)]
            Self::Interpreted { schema, root } => matches!(
                resolve(schema, root)["format"].as_str(),
                Some("int" | "int64")
            ),
        }
    }
    pub(super) fn property(self, index: usize) -> Option<(&'a str, Self)> {
        match self {
            Self::Compiled(plan, node) => plan.nodes[node]
                .properties
                .get(index)
                .map(|property| (property.name.as_str(), Self::Compiled(plan, property.node))),
            #[cfg(test)]
            Self::Interpreted { schema, root } => resolve(schema, root)["properties"]
                .as_object()?
                .iter()
                .nth(index)
                .map(|(name, schema)| (name.as_str(), Self::Interpreted { schema, root })),
        }
    }
    pub(super) fn tag(self, key: &str) -> Option<&'a Value> {
        match self {
            Self::Compiled(plan, node) => plan.nodes[node].property(key)?.constant.as_ref(),
            #[cfg(test)]
            Self::Interpreted { schema, root } => {
                resolve(schema, root)["properties"][key].get("const")
            }
        }
    }
    pub(super) fn branch(self, kind: usize, index: usize) -> Option<Self> {
        match self {
            Self::Compiled(plan, node) => plan.nodes[node].branches[kind]
                .get(index)
                .map(|index| Self::Compiled(plan, *index)),
            #[cfg(test)]
            Self::Interpreted { schema, root } => resolve(schema, root)
                [["oneOf", "anyOf", "allOf"][kind]]
                .as_array()?
                .get(index)
                .map(|schema| Self::Interpreted { schema, root }),
        }
    }
    pub(super) fn child(self, key: Option<&str>) -> Self {
        match self {
            Self::Compiled(plan, index) => {
                let node = &plan.nodes[index];
                let direct = match key {
                    Some(key) => node
                        .property(key)
                        .map(|property| property.node)
                        .or(node.additional),
                    None => node.items,
                };
                if let Some(index) = direct {
                    return Self::Compiled(plan, index);
                }
                for branch in node.branches.iter().flatten() {
                    let found = Self::Compiled(plan, *branch).child(key);
                    if !found.is_null() {
                        return found;
                    }
                }
                Self::Compiled(plan, 0)
            }
            #[cfg(test)]
            Self::Interpreted { schema, root } => Self::Interpreted {
                schema: super::child(schema, root, key),
                root,
            },
        }
    }

    // App Server's Value codec visits untagged numeric alternatives even when
    // the union itself has no format (notably RequestId). Scalars are tiny;
    // preserving that order needs no intermediate record/object tree.
    pub(super) fn wire_number(self, mut value: Value) -> Result<Value, &'static str> {
        let (plan, index) = match self {
            Self::Compiled(plan, index) => (plan, index),
            #[cfg(test)]
            Self::Interpreted { .. } => unreachable!("wire encoding uses a compiled schema"),
        };
        let node = &plan.nodes[index];
        if node
            .properties
            .iter()
            .any(|property| property.constant.is_some())
        {
            return Ok(value);
        }
        if node.unsigned == Unsigned::Decimal {
            return value
                .as_u64()
                .map(|number| Value::from(number.to_string()))
                .ok_or("exact domain requires native u64");
        }
        if value.is_number() && (node.unsigned == Unsigned::Safe || node.signed_safe) {
            let number = value
                .as_i64()
                .ok_or("quantity must be a safe JSON integer")?;
            if number.unsigned_abs() > 9_007_199_254_740_991 {
                return Err("quantity exceeds safe JSON integer range");
            }
        }
        for branch in node.branches.iter().flatten() {
            value = Self::Compiled(plan, *branch).wire_number(value)?;
        }
        Ok(value)
    }
    pub(super) fn has_wire_tag(self, key: &str) -> bool {
        for kind in 0..3 {
            let mut index = 0;
            while let Some(branch) = self.branch(kind, index) {
                index += 1;
                if branch.tag(key).is_some() || branch.has_wire_tag(key) {
                    return true;
                }
            }
        }
        false
    }
    pub(super) fn wire_variant(self, key: &str, value: &Value) -> Option<Self> {
        for kind in 0..3 {
            let mut index = 0;
            while let Some(branch) = self.branch(kind, index) {
                index += 1;
                if let Some(expected) = branch.tag(key) {
                    if expected == value {
                        return Some(branch);
                    }
                } else if let Some(selected) = branch.wire_variant(key, value) {
                    return Some(selected);
                }
            }
        }
        None
    }
    pub(super) fn wire_element(self) -> Self {
        let (plan, index) = match self {
            Self::Compiled(plan, index) => (plan, index),
            #[cfg(test)]
            Self::Interpreted { .. } => unreachable!("wire encoding uses a compiled schema"),
        };
        let node = &plan.nodes[index];
        if node.items_object
            && let Some(index) = node.items
        {
            return Self::Compiled(plan, index);
        }
        for branch in node.branches.iter().flatten() {
            let found = Self::Compiled(plan, *branch).wire_element();
            if !found.is_null() {
                return found;
            }
        }
        Self::Compiled(plan, 0)
    }
    pub(super) fn element(self, index: usize) -> Self {
        match self {
            Self::Compiled(plan, node) => plan.nodes[node]
                .prefix
                .get(index)
                .map_or_else(|| self.child(None), |index| Self::Compiled(plan, *index)),
            #[cfg(test)]
            Self::Interpreted { schema, root } => Self::Interpreted {
                schema: super::element(schema, root, index),
                root,
            },
        }
    }
}
