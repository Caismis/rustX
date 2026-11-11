//! Compile the immutable schema vocabulary once; values never enter this plan.
use super::{Value, resolve};
use std::collections::HashMap;

#[cfg(test)]
thread_local! {
    static TAG_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PROPERTY_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
pub(crate) fn take_tag_probes() -> usize {
    TAG_PROBES.with(|count| count.replace(0))
}

#[cfg(test)]
pub(crate) fn take_property_work() -> (usize, usize) {
    PROPERTY_WORK.with(|count| count.replace((0, 0)))
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
pub(crate) enum Unsigned {
    Plain,
    Decimal,
    Safe,
}

/// The numeric domain is derived once from the Rust-owned schema, including
/// the explicit bound that distinguishes exact identities from safe quantities.
pub(crate) fn unsigned(schema: &Value) -> Unsigned {
    match schema["format"].as_str() {
        Some("uint64")
            if schema["maximum"]
                .as_f64()
                .is_none_or(|maximum| maximum > 9_007_199_254_740_991.0) =>
        {
            Unsigned::Decimal
        }
        Some("uint" | "uint64") => Unsigned::Safe,
        _ => Unsigned::Plain,
    }
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
struct Span {
    start: u32,
    end: u32,
}
impl Span {
    fn new(start: usize, end: usize) -> Self {
        Self {
            start: u32::try_from(start).expect("finite codec vocabulary"),
            end: u32::try_from(end).expect("finite codec vocabulary"),
        }
    }
    fn slice<T>(self, pool: &[T]) -> &[T] {
        &pool[self.start as usize..self.end as usize]
    }
    fn text(self, pool: &str) -> &str {
        &pool[self.start as usize..self.end as usize]
    }
}

#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
struct Property {
    name: Span,
    node: u32,
    // The original tag selector inspects the property's own const, not a ref.
    constant: Option<Box<Value>>,
}
#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
struct Node {
    unsigned: Unsigned,
    signed_safe: bool,
    properties: Span,
    // Sorted indexes preserve the schema's serialization-order vector while
    // map/fallback/tag lookups avoid rescanning a wide DTO's property names.
    property_names: Span,
    additional: Option<u32>,
    items: Option<u32>,
    items_object: bool,
    prefix: Span,
    branches: [Span; 3],
    // Derived solely from immutable branch descriptors and never persisted.
    #[serde(skip)]
    wire_tags: std::sync::OnceLock<Box<[Span]>>,
}
impl Default for Node {
    fn default() -> Self {
        Self {
            unsigned: Unsigned::Plain,
            signed_safe: false,
            properties: Span::default(),
            property_names: Span::default(),
            additional: None,
            items: None,
            items_object: false,
            prefix: Span::default(),
            branches: [Span::default(); 3],
            wire_tags: std::sync::OnceLock::new(),
        }
    }
}

impl Node {
    fn wire_tags(&self, plan: &Plan) -> &[Span] {
        if self.branches.iter().all(|span| span.start == span.end) {
            return &[];
        }
        self.wire_tags.get_or_init(|| {
            let mut visited = vec![false; plan.nodes.len()];
            let mut pending: Vec<u32> = self
                .branches
                .iter()
                .flat_map(|span| span.slice(&plan.indices).iter().copied())
                .collect();
            let mut tags = Vec::new();
            while let Some(index) = pending.pop() {
                if std::mem::replace(&mut visited[index as usize], true) {
                    continue;
                }
                let node = &plan.nodes[index as usize];
                tags.extend(
                    node.properties
                        .slice(&plan.properties)
                        .iter()
                        .filter(|property| property.constant.is_some())
                        .map(|property| property.name),
                );
                pending.extend(
                    node.branches
                        .iter()
                        .flat_map(|span| span.slice(&plan.indices).iter().copied()),
                );
            }
            tags.sort_unstable_by(|left, right| {
                left.text(&plan.names).cmp(right.text(&plan.names))
            });
            tags.dedup_by(|left, right| left.text(&plan.names) == right.text(&plan.names));
            tags.into_boxed_slice()
        })
    }
    fn property<'a>(&self, key: &str, plan: &'a Plan) -> Option<&'a Property> {
        #[cfg(test)]
        PROPERTY_WORK.with(|count| {
            let (lookups, comparisons) = count.get();
            count.set((lookups + 1, comparisons));
        });
        let properties = self.properties.slice(&plan.properties);
        let names = self.property_names.slice(&plan.indices);
        let index = names
            .binary_search_by(|index| {
                #[cfg(test)]
                PROPERTY_WORK.with(|count| {
                    let (lookups, comparisons) = count.get();
                    count.set((lookups, comparisons + 1));
                });
                properties[*index as usize].name.text(&plan.names).cmp(key)
            })
            .ok()?;
        properties.get(names[index] as usize)
    }
}

#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
pub(crate) struct Plan {
    nodes: Box<[Node]>,
    properties: Box<[Property]>,
    indices: Box<[u32]>,
    names: Box<str>,
    root: u32,
}

struct Builder {
    nodes: Vec<Node>,
    properties: Vec<Property>,
    indices: Vec<u32>,
    names: String,
    addresses: HashMap<usize, u32>,
    // Only build-time vocabulary is interned; no schema JSON or addresses survive.
    shapes: HashMap<String, u32>,
}
impl Plan {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        rmp_serde::from_slice(bytes).expect("generated numeric codec descriptor")
    }
    #[cfg(test)]
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        rmp_serde::to_vec(self).expect("numeric codec descriptor")
    }

    pub(crate) fn new(schema: &Value, root: &Value) -> Self {
        let mut builder = Builder {
            nodes: vec![Node::default()],
            properties: Vec::new(),
            indices: Vec::new(),
            names: String::new(),
            addresses: HashMap::new(),
            shapes: HashMap::new(),
        };
        let root = builder.compile(schema, root);
        Self {
            nodes: builder.nodes.into_boxed_slice(),
            properties: builder.properties.into_boxed_slice(),
            indices: builder.indices.into_boxed_slice(),
            names: builder.names.into_boxed_str(),
            root,
        }
    }
    pub(crate) fn root(&self) -> View<'_> {
        View::Compiled(self, self.root)
    }
}
impl Builder {
    fn name(&mut self, name: &str) -> Span {
        let start = self.names.len();
        self.names.push_str(name);
        Span::new(start, self.names.len())
    }
    fn indices(&mut self, indices: Vec<u32>) -> Span {
        let start = self.indices.len();
        self.indices.extend(indices);
        Span::new(start, self.indices.len())
    }

    fn compile(&mut self, schema: &Value, root: &Value) -> u32 {
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
            let index = u32::try_from(self.nodes.len()).expect("finite codec vocabulary");
            self.nodes.push(Node::default());
            self.addresses.insert(address, index);
            return index;
        }
        let target = std::ptr::from_ref(schema) as usize;
        if let Some(index) = self.addresses.get(&target).copied() {
            self.addresses.insert(address, index);
            return index;
        }
        let shape = serde_json::to_string(schema).expect("schema vocabulary");
        if let Some(index) = self.shapes.get(&shape).copied() {
            self.addresses.insert(address, index);
            return index;
        }
        let index = u32::try_from(self.nodes.len()).expect("finite codec vocabulary");
        self.addresses.insert(address, index);
        self.addresses.insert(target, index);
        self.shapes.insert(shape, index);
        // Reserve before following children: generated recursive DTO refs point
        // back to this node, not to a recursively cloned schema tree.
        self.nodes.push(Node::default());
        let unsigned = unsigned(schema);
        let properties: Vec<Property> =
            schema["properties"]
                .as_object()
                .map_or_else(Vec::new, |fields| {
                    fields
                        .iter()
                        .map(|(name, value)| Property {
                            name: self.name(name),
                            node: self.compile(value, root),
                            constant: value.get("const").cloned().map(Box::new),
                        })
                        .collect()
                });
        let mut property_names: Vec<_> =
            (0..u32::try_from(properties.len()).expect("finite properties")).collect();
        property_names.sort_unstable_by(|left, right| {
            properties[*left as usize]
                .name
                .text(&self.names)
                .cmp(properties[*right as usize].name.text(&self.names))
        });
        let additional = schema
            .get("additionalProperties")
            .filter(|value| value.is_object())
            .map(|value| self.compile(value, root));
        let items = schema.get("items").map(|value| self.compile(value, root));
        let prefix: Vec<u32> = schema["prefixItems"]
            .as_array()
            .map_or_else(Vec::new, |values| {
                values
                    .iter()
                    .map(|value| self.compile(value, root))
                    .collect()
            });
        let branches: [Vec<u32>; 3] = ["oneOf", "anyOf", "allOf"].map(|keyword| {
            schema[keyword].as_array().map_or_else(Vec::new, |values| {
                values
                    .iter()
                    .map(|value| self.compile(value, root))
                    .collect()
            })
        });
        let property_span = Span::new(
            self.properties.len(),
            self.properties.len() + properties.len(),
        );
        self.properties.extend(properties);
        let property_names = self.indices(property_names);
        let prefix = self.indices(prefix);
        let branches = branches.map(|branches| self.indices(branches));
        self.nodes[index as usize] = Node {
            unsigned,
            signed_safe: matches!(schema["format"].as_str(), Some("int" | "int64")),
            properties: property_span,
            property_names,
            additional,
            items,
            items_object: schema["items"].is_object(),
            prefix,
            branches,
            wire_tags: std::sync::OnceLock::new(),
        };
        index
    }
}

#[derive(Clone, Copy)]
pub(crate) enum View<'a> {
    Compiled(&'a Plan, u32),
    // The pre-change schema interpreter is a differential oracle, never a
    // runtime fallback or a second encoding format.
    #[cfg(test)]
    Interpreted {
        schema: &'a Value,
        root: &'a Value,
    },
}
impl<'a> View<'a> {
    pub(crate) fn property_named(self, key: &str) -> Option<Self> {
        match self {
            Self::Compiled(plan, index) => plan.nodes[index as usize]
                .property(key, plan)
                .map(|property| Self::Compiled(plan, property.node)),
            #[cfg(test)]
            Self::Interpreted { schema, root } => resolve(schema, root)["properties"]
                .get(key)
                .map(|schema| Self::Interpreted { schema, root }),
        }
    }
    pub(crate) fn additional(self) -> Option<Self> {
        match self {
            Self::Compiled(plan, index) => plan.nodes[index as usize]
                .additional
                .map(|index| Self::Compiled(plan, index)),
            #[cfg(test)]
            Self::Interpreted { schema, root } => resolve(schema, root)
                .get("additionalProperties")
                .filter(|schema| schema.is_object())
                .map(|schema| Self::Interpreted { schema, root }),
        }
    }
    pub(crate) fn array_item(self, index: usize) -> Option<Self> {
        match self {
            Self::Compiled(plan, node) => {
                let node = &plan.nodes[node as usize];
                node.prefix
                    .slice(&plan.indices)
                    .get(index)
                    .copied()
                    .or(node.items.filter(|_| node.items_object))
                    .map(|index| Self::Compiled(plan, index))
            }
            #[cfg(test)]
            Self::Interpreted { schema, root } => {
                let schema = resolve(schema, root);
                schema["prefixItems"]
                    .get(index)
                    .or_else(|| schema.get("items").filter(|schema| schema.is_object()))
                    .map(|schema| Self::Interpreted { schema, root })
            }
        }
    }
    pub(crate) fn matches_tagged(self, value: &Value) -> bool {
        let mut tagged = false;
        let mut index = 0;
        while let Some((key, _)) = self.property(index) {
            index += 1;
            if let Some(expected) = self.tag(key) {
                tagged = true;
                if value.get(key) != Some(expected) {
                    return false;
                }
            }
        }
        tagged
    }
    pub(crate) fn convert_wire(self, value: &mut Value, encode: bool) -> Result<(), &'static str> {
        if value.is_null() {
            return Ok(());
        }
        let mut index = 0;
        while let Some((key, _)) = self.property(index) {
            index += 1;
            if self
                .tag(key)
                .is_some_and(|expected| value.get(key) != Some(expected))
            {
                return Ok(());
            }
        }
        if self.unsigned() == Unsigned::Decimal {
            if encode {
                *value = Value::from(
                    value
                        .as_u64()
                        .ok_or("exact domain requires native u64")?
                        .to_string(),
                );
            } else {
                let text = value.as_str().ok_or("exact domain requires decimal text")?;
                let number = text
                    .parse::<u64>()
                    .map_err(|_| "invalid unsigned decimal domain")?;
                if number.to_string() != text {
                    return Err("noncanonical unsigned decimal domain");
                }
                *value = Value::from(number);
            }
            return Ok(());
        }
        if value.is_number() && (self.unsigned() == Unsigned::Safe || self.signed_safe()) {
            let number = value
                .as_i64()
                .ok_or("quantity must be a safe JSON integer")?;
            if number.unsigned_abs() > 9_007_199_254_740_991 {
                return Err("quantity exceeds safe JSON integer range");
            }
        }
        if let Some(object) = value.as_object_mut() {
            let mut index = 0;
            while let Some((key, schema)) = self.property(index) {
                index += 1;
                if let Some(value) = object.get_mut(key) {
                    schema.convert_wire(value, encode)?;
                }
            }
            if let Some(schema) = self.additional() {
                for (key, value) in object {
                    if self.property_named(key).is_none() {
                        schema.convert_wire(value, encode)?;
                    }
                }
            }
        }
        // Wire conversion historically applies only homogeneous object items;
        // positional tuples keep their native representation.
        let items = match self {
            Self::Compiled(plan, index) => {
                let node = &plan.nodes[index as usize];
                node.items
                    .filter(|_| node.items_object)
                    .map(|index| Self::Compiled(plan, index))
            }
            #[cfg(test)]
            Self::Interpreted { schema, root } => resolve(schema, root)
                .get("items")
                .filter(|schema| schema.is_object())
                .map(|schema| Self::Interpreted { schema, root }),
        };
        if let (Some(values), Some(schema)) = (value.as_array_mut(), items) {
            for value in values {
                schema.convert_wire(value, encode)?;
            }
        }
        for kind in 0..3 {
            let mut index = 0;
            while let Some(branch) = self.branch(kind, index) {
                index += 1;
                branch.convert_wire(value, encode)?;
            }
        }
        Ok(())
    }
    pub(crate) fn is_null(self) -> bool {
        match self {
            Self::Compiled(_, index) => index == 0,
            #[cfg(test)]
            Self::Interpreted { schema, .. } => schema.is_null(),
        }
    }
    pub(crate) fn unsigned(self) -> Unsigned {
        match self {
            Self::Compiled(plan, index) => plan.nodes[index as usize].unsigned,
            #[cfg(test)]
            Self::Interpreted { schema, root } => unsigned(resolve(schema, root)),
        }
    }
    pub(crate) fn signed_safe(self) -> bool {
        match self {
            Self::Compiled(plan, index) => plan.nodes[index as usize].signed_safe,
            #[cfg(test)]
            Self::Interpreted { schema, root } => matches!(
                resolve(schema, root)["format"].as_str(),
                Some("int" | "int64")
            ),
        }
    }
    pub(crate) fn property(self, index: usize) -> Option<(&'a str, Self)> {
        match self {
            Self::Compiled(plan, node) => plan.nodes[node as usize]
                .properties
                .slice(&plan.properties)
                .get(index)
                .map(|property| {
                    (
                        property.name.text(&plan.names),
                        Self::Compiled(plan, property.node),
                    )
                }),
            #[cfg(test)]
            Self::Interpreted { schema, root } => resolve(schema, root)["properties"]
                .as_object()?
                .iter()
                .nth(index)
                .map(|(name, schema)| (name.as_str(), Self::Interpreted { schema, root })),
        }
    }
    pub(crate) fn tag(self, key: &str) -> Option<&'a Value> {
        match self {
            Self::Compiled(plan, node) => plan.nodes[node as usize]
                .property(key, plan)?
                .constant
                .as_deref(),
            #[cfg(test)]
            Self::Interpreted { schema, root } => {
                resolve(schema, root)["properties"][key].get("const")
            }
        }
    }
    pub(crate) fn branch(self, kind: usize, index: usize) -> Option<Self> {
        match self {
            Self::Compiled(plan, node) => plan.nodes[node as usize].branches[kind]
                .slice(&plan.indices)
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
    pub(crate) fn child(self, key: Option<&str>) -> Self {
        match self {
            Self::Compiled(plan, index) => {
                let node = &plan.nodes[index as usize];
                let direct = match key {
                    Some(key) => node
                        .property(key, plan)
                        .map(|property| property.node)
                        .or(node.additional),
                    None => node.items,
                };
                if let Some(index) = direct {
                    return Self::Compiled(plan, index);
                }
                for branch in node
                    .branches
                    .iter()
                    .flat_map(|span| span.slice(&plan.indices))
                {
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
    pub(crate) fn wire_number(self, mut value: Value) -> Result<Value, &'static str> {
        let (plan, index) = match self {
            Self::Compiled(plan, index) => (plan, index),
            #[cfg(test)]
            Self::Interpreted { .. } => unreachable!("wire encoding uses a compiled schema"),
        };
        let node = &plan.nodes[index as usize];
        if node
            .properties
            .slice(&plan.properties)
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
        for branch in node
            .branches
            .iter()
            .flat_map(|span| span.slice(&plan.indices))
        {
            value = Self::Compiled(plan, *branch).wire_number(value)?;
        }
        Ok(value)
    }
    pub(crate) fn has_wire_tag(self, key: &str) -> bool {
        match self {
            Self::Compiled(plan, index) => plan.nodes[index as usize]
                .wire_tags(plan)
                .binary_search_by(|name| {
                    #[cfg(test)]
                    TAG_PROBES.with(|count| count.set(count.get() + 1));
                    name.text(&plan.names).cmp(key)
                })
                .is_ok(),
            #[cfg(test)]
            Self::Interpreted { .. } => {
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
        }
    }
    pub(crate) fn wire_variant(self, key: &str, value: &Value) -> Option<Self> {
        self.wire_variant_matching(key, &|expected| expected == value)
    }
    pub(crate) fn wire_variant_str(self, key: &str, value: &str) -> Option<Self> {
        self.wire_variant_matching(key, &|expected| expected.as_str() == Some(value))
    }
    fn wire_variant_matching(self, key: &str, matches: &impl Fn(&Value) -> bool) -> Option<Self> {
        for kind in 0..3 {
            let mut index = 0;
            while let Some(branch) = self.branch(kind, index) {
                index += 1;
                if let Some(expected) = branch.tag(key) {
                    if matches(expected) {
                        return Some(branch);
                    }
                } else if let Some(selected) = branch.wire_variant_matching(key, matches) {
                    return Some(selected);
                }
            }
        }
        None
    }
    pub(crate) fn wire_element(self) -> Self {
        let (plan, index) = match self {
            Self::Compiled(plan, index) => (plan, index),
            #[cfg(test)]
            Self::Interpreted { .. } => unreachable!("wire encoding uses a compiled schema"),
        };
        let node = &plan.nodes[index as usize];
        if node.items_object
            && let Some(index) = node.items
        {
            return Self::Compiled(plan, index);
        }
        for branch in node
            .branches
            .iter()
            .flat_map(|span| span.slice(&plan.indices))
        {
            let found = Self::Compiled(plan, *branch).wire_element();
            if !found.is_null() {
                return found;
            }
        }
        Self::Compiled(plan, 0)
    }
    pub(crate) fn element(self, index: usize) -> Self {
        match self {
            Self::Compiled(plan, node) => plan.nodes[node as usize]
                .prefix
                .slice(&plan.indices)
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
