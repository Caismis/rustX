//! Bounded navigation metadata for at most 4 KiB of original JSON.
//! Original bytes stay in their existing byte file; this is never an authority.
use super::{MEMORY_INDEX_BYTES, Node, NodeRecord, ObservationReadError};

const MAX_NODES: usize = MEMORY_INDEX_BYTES;
// Node storage plus seal-time child counts and bounded decoded keys stays below
// the former small SQLite page budget. Admission is based on actual encoded bytes.
const _: () = assert!(
    MAX_NODES * (std::mem::size_of::<NodeRecord>() + std::mem::size_of::<u64>())
        + 2 * MEMORY_INDEX_BYTES
        <= 1_048_576
);

#[derive(Debug, Default)]
pub(super) struct SmallIndex {
    pub(super) nodes: Vec<NodeRecord>,
}
impl SmallIndex {
    pub(super) fn push(&mut self, record: NodeRecord) -> Result<(), &'static str> {
        if self.nodes.len() == MAX_NODES {
            return Err("native small index exceeds node bound");
        }
        self.nodes.push(record);
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
            if record.id != id as u64
                || record.node.start > record.node.end
                || record.node.end > bytes
            {
                return Err("native small index geometry invalid");
            }
            if let Some(parent) = record.parent {
                let parent = usize::try_from(parent)
                    .ok()
                    .filter(|parent| *parent < id)
                    .ok_or("native small index parent invalid")?;
                let owner = &self.nodes[parent].node;
                if !owner.container
                    || record.ordinal != children[parent]
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
            .any(|(record, count)| record.node.children != count)
        {
            return Err("native small index child count invalid");
        }
        Ok(())
    }

    pub(super) fn node(&self, id: u64) -> Result<Node, &'static str> {
        usize::try_from(id)
            .ok()
            .and_then(|id| self.nodes.get(id))
            .filter(|record| record.id == id)
            .map(|record| record.node.clone())
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
                record.parent == Some(parent)
                    && ordinal.map_or_else(
                        || record.node.key_complete && record.node.key.as_deref() == Some(key),
                        |ordinal| record.ordinal == ordinal,
                    )
            })
            .map(|record| record.id)
            .ok_or(ObservationReadError::InvalidRequest(
                "invalid pending source path",
            ))
    }

    pub(super) fn children(&self, parent: u64, offset: u64, limit: usize) -> Vec<(u64, u64)> {
        self.nodes
            .iter()
            .filter(|record| record.parent == Some(parent) && record.ordinal >= offset)
            .take(limit)
            .map(|record| (record.id, record.ordinal))
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
            .skip_while(|record| record.id < first)
            .take_while(|record| record.id <= last)
        {
            if include(record.id) {
                visit(record.clone())?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(id: u64, parent: Option<u64>, ordinal: u64, children: u64) -> NodeRecord {
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
