//! Compact bounded navigation metadata for at most 16 KiB of original JSON.
//! Original bytes stay in their Source-owned carrier; this is never an authority.
use super::{MEMORY_INDEX_BYTES, Node, NodeRecord, ObservationReadError};

// A one-byte leaf plus separators is the densest valid JSON tree. A Source
// with B bytes has at most (B+1)/2 nodes, including the root. This is a bound
// on the completed exact encoding, not a serializer's size estimate.
const MAX_NODES: usize = MEMORY_INDEX_BYTES.div_ceil(2);
const _: () = assert!(MEMORY_INDEX_BYTES <= u16::MAX as usize);
const _: () = assert!(MAX_NODES.is_power_of_two());
// Vec capacity tops out at the power-of-two node bound. Include seal-time
// child counts and decoded key capacities in the existing 1 MiB allowance.
const _: () = assert!(
    MAX_NODES * (std::mem::size_of::<CompactRecord>() + std::mem::size_of::<u64>())
        + 2 * MEMORY_INDEX_BYTES
        <= 1_048_576
);

#[derive(Clone, Debug)]
pub(super) struct CompactRecord {
    id: u16,
    parent: Option<u16>,
    ordinal: u16,
    pub(super) node: CompactNode,
}
#[derive(Clone, Debug)]
pub(super) struct CompactNode {
    start: u16,
    end: u16,
    children: u16,
    recursive_nodes: u8,
    container: bool,
    object: bool,
    pub(super) key: Option<String>,
    key_span: Option<(u16, u16)>,
    key_complete: bool,
}
impl TryFrom<NodeRecord> for CompactRecord {
    type Error = &'static str;
    fn try_from(record: NodeRecord) -> Result<Self, Self::Error> {
        fn narrow(value: u64) -> Result<u16, &'static str> {
            u16::try_from(value).map_err(|_| "native small index coordinate exceeds bound")
        }
        let node = record.node;
        Ok(Self {
            id: narrow(record.id)?,
            parent: record.parent.map(narrow).transpose()?,
            ordinal: narrow(record.ordinal)?,
            node: CompactNode {
                start: narrow(node.start)?,
                end: narrow(node.end)?,
                children: narrow(node.children)?,
                recursive_nodes: u8::try_from(node.recursive_nodes)
                    .map_err(|_| "native small index preview exceeds bound")?,
                container: node.container,
                object: node.object,
                key: node.key,
                key_span: node
                    .key_span
                    .map(|(start, end)| Ok::<_, &'static str>((narrow(start)?, narrow(end)?)))
                    .transpose()?,
                key_complete: node.key_complete,
            },
        })
    }
}
impl CompactNode {
    fn expand(&self) -> Node {
        Node {
            start: u64::from(self.start),
            end: u64::from(self.end),
            children: u64::from(self.children),
            recursive_nodes: u64::from(self.recursive_nodes),
            container: self.container,
            object: self.object,
            key: self.key.clone(),
            key_span: self
                .key_span
                .map(|(start, end)| (u64::from(start), u64::from(end))),
            key_complete: self.key_complete,
        }
    }
}
impl CompactRecord {
    fn expand(&self) -> NodeRecord {
        NodeRecord {
            id: u64::from(self.id),
            parent: self.parent.map(u64::from),
            ordinal: u64::from(self.ordinal),
            node: self.node.expand(),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct SmallIndex {
    pub(super) nodes: Vec<CompactRecord>,
}
impl SmallIndex {
    pub(super) fn push(&mut self, record: NodeRecord) -> Result<(), &'static str> {
        if self.nodes.len() == MAX_NODES {
            return Err("native small index exceeds node bound");
        }
        self.nodes.push(record.try_into()?);
        Ok(())
    }

    pub(super) fn seal(&mut self, count: u64, bytes: u64) -> Result<(), &'static str> {
        self.nodes.sort_unstable_by_key(|record| record.id);
        if bytes > MEMORY_INDEX_BYTES as u64
            || self.nodes.len() as u64 != count
            || self.nodes.is_empty()
        {
            return Err("native small index incomplete");
        }
        let mut children = vec![0_u64; self.nodes.len()];
        for (id, record) in self.nodes.iter().enumerate() {
            if usize::from(record.id) != id
                || record.node.start > record.node.end
                || u64::from(record.node.end) > bytes
            {
                return Err("native small index geometry invalid");
            }
            if let Some(parent) = record.parent {
                let parent = usize::from(parent);
                if parent >= id {
                    return Err("native small index parent invalid");
                }
                let owner = &self.nodes[parent].node;
                if !owner.container
                    || u64::from(record.ordinal) != children[parent]
                    || record.node.start < owner.start
                    || record.node.end > owner.end
                {
                    return Err("native small index child invalid");
                }
                children[parent] += 1;
            } else if id != 0 || record.ordinal != 0 {
                return Err("native small index root invalid");
            }
        }
        if self
            .nodes
            .iter()
            .zip(children)
            .any(|(record, count)| u64::from(record.node.children) != count)
        {
            return Err("native small index child count invalid");
        }
        Ok(())
    }

    pub(super) fn node(&self, id: u64) -> Result<Node, &'static str> {
        usize::try_from(id)
            .ok()
            .and_then(|id| self.nodes.get(id))
            .filter(|record| u64::from(record.id) == id)
            .map(|record| record.node.expand())
            .ok_or("native small index node unavailable")
    }

    pub(super) fn select(
        &self,
        parent: u64,
        ordinal: Option<u64>,
        key: &str,
    ) -> Result<u64, ObservationReadError> {
        self.nodes
            .iter()
            .find(|record| {
                record.parent.map(u64::from) == Some(parent)
                    && ordinal.map_or_else(
                        || record.node.key_complete && record.node.key.as_deref() == Some(key),
                        |ordinal| u64::from(record.ordinal) == ordinal,
                    )
            })
            .map(|record| u64::from(record.id))
            .ok_or(ObservationReadError::InvalidRequest(
                "invalid pending source path",
            ))
    }

    pub(super) fn children(&self, parent: u64, offset: u64, limit: usize) -> Vec<(u64, u64)> {
        self.nodes
            .iter()
            .filter(|record| {
                record.parent.map(u64::from) == Some(parent) && u64::from(record.ordinal) >= offset
            })
            .take(limit)
            .map(|record| (u64::from(record.id), u64::from(record.ordinal)))
            .collect()
    }

    pub(super) fn visit(
        &self,
        first: u64,
        last: u64,
        mut include: impl FnMut(u64) -> bool,
        mut visit: impl FnMut(NodeRecord) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        for record in self
            .nodes
            .iter()
            .skip_while(|record| u64::from(record.id) < first)
            .take_while(|record| u64::from(record.id) <= last)
        {
            if include(u64::from(record.id)) {
                visit(record.expand())?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(id: u64, parent: Option<u64>, ordinal: u64, children: u64) -> CompactRecord {
        NodeRecord {
            id,
            parent,
            ordinal,
            node: Node {
                start: 0,
                end: 2,
                children,
                recursive_nodes: (1 + children).min(33),
                container: children > 0,
                object: false,
                key: None,
                key_span: None,
                key_complete: true,
            },
        }
        .try_into()
        .unwrap()
    }
    #[test]
    fn seal_rejects_incomplete_duplicate_or_unbounded_geometry() {
        let valid = vec![
            node(1, Some(0), 0, 0),
            node(2, Some(0), 1, 0),
            node(0, None, 0, 2),
        ];
        let mut index = SmallIndex {
            nodes: valid.clone(),
        };
        index.seal(3, 2).unwrap();
        assert_eq!(index.children(0, 0, 32), vec![(1, 0), (2, 1)]);
        for mutation in 0..7 {
            let mut index = SmallIndex {
                nodes: valid.clone(),
            };
            match mutation {
                0 => index.nodes[1].ordinal = 0,
                1 => index.nodes[1].id = 1,
                2 => index.nodes[1].parent = Some(2),
                3 => index.nodes[2].node.children = 1,
                4 => index.nodes[1].node.end = 3,
                5 => {
                    index.nodes.pop();
                }
                _ => index.nodes[1].parent = None,
            }
            assert!(index.seal(3, 2).is_err(), "mutation {mutation}");
        }
        assert!(
            SmallIndex { nodes: valid }
                .seal(3, MEMORY_INDEX_BYTES as u64 + 1)
                .is_err()
        );
    }
}
