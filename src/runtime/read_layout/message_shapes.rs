//! Producer-certified request-message field geometry. The certificate borrows
//! its exact immutable source; neither shared SQL rows nor a digest certify it.
use super::{FieldPreview, FieldPreviewPolicy, FieldReader, Node, SAFE, SeekableObservationSource};
use sha2::{Digest as _, Sha256};

const MAX_SHAPES: usize = 8192;
const MAX_FACT_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FieldGeometry {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) children: u64,
    /// The existing preview count saturates at 33.
    pub(crate) recursive_nodes: u64,
}
impl From<&Node> for FieldGeometry {
    fn from(node: &Node) -> Self {
        Self {
            start: node.start,
            end: node.end,
            children: node.children,
            recursive_nodes: node.recursive_nodes,
        }
    }
}

/// One bounded typed-producer item with exactly one direct dynamic value.
/// Source additionally binds this evidence to its original canonical producer.
#[derive(Debug)]
pub(crate) struct CertifiedMessageShape<'a> {
    source: &'a SeekableObservationSource,
    first: u64,
    last: u64,
    hole_first: u64,
    hole_last: u64,
    pub(crate) item: u64,
    pub(crate) root: FieldGeometry,
    pub(crate) hole: FieldGeometry,
    pub(crate) hole_ordinal: u64,
    pub(crate) static_digest: [u8; 32],
}
impl CertifiedMessageShape<'_> {
    fn owns(&self, source: &SeekableObservationSource) -> bool {
        std::ptr::eq(self.source, source)
    }
    fn static_field(&self, id: u64) -> bool {
        id > self.first && id <= self.last && !(self.hole_first..=self.hole_last).contains(&id)
    }
}

/// A single request-local native connection/file. Certificates borrow only the
/// immutable source, so collecting them does not retain another history owner.
pub(crate) struct MessageShapeReader<'a> {
    source: &'a SeekableObservationSource,
    reader: FieldReader<'a>,
    messages: u64,
}
impl SeekableObservationSource {
    pub(crate) fn message_shape_reader(&self) -> Result<MessageShapeReader<'_>, &'static str> {
        let reader = self.field_reader()?;
        let messages = self
            .layout
            .indexed_select(&reader.connection, 0, None, "messages")
            .map_err(|_| "native message array unavailable")?;
        let node = self.layout.indexed_node(&reader.connection, messages)?;
        if !node.container || node.object {
            return Err("native message array unavailable");
        }
        Ok(MessageShapeReader {
            source: self,
            reader,
            messages,
        })
    }
}
impl<'a> MessageShapeReader<'a> {
    /// A different header, unsupported shape or oversized item is an ordinary
    /// producer miss. Only actual native index/read failures return an error.
    pub(crate) fn certify_message_shape(
        &self,
        item: u64,
        expected_header: &[u8],
    ) -> Result<Option<CertifiedMessageShape<'a>>, &'static str> {
        if item > SAFE || expected_header.is_empty() || expected_header.len() > 16_384 {
            return Ok(None);
        }
        let connection = &self.reader.connection;
        let first = self
            .source
            .layout
            .indexed_select(connection, self.messages, Some(item), "")
            .map_err(|_| "native message shape path unavailable")?;
        let root = self.source.layout.indexed_node(connection, first)?;
        if !root.object
            || root.children > 32
            || root.end - root.start != expected_header.len() as u64
        {
            return Ok(None);
        }
        let last = self.message_last_node(connection, first, &root)?;
        let node_count = usize::try_from(last - first + 1)
            .map_err(|_| "native message shape node count unavailable")?;
        if node_count > 512 {
            return Ok(None);
        }
        let Some((hole_first, hole, hole_ordinal)) =
            self.source
                .layout
                .indexed_unique_key(connection, first, "provenance_source")?
        else {
            return Ok(None);
        };
        let raw = super::Layout::read_bytes(&self.reader.bytes, root.start, expected_header.len())?;
        if raw != expected_header {
            return Ok(None);
        }
        let hole_last = self.message_last_node(connection, hole_first, &hole)?;
        let hole_start = usize::try_from(hole.start - root.start)
            .map_err(|_| "native message shape hole offset unavailable")?;
        let hole_end = usize::try_from(hole.end - root.start)
            .map_err(|_| "native message shape hole offset unavailable")?;
        let prefix = raw
            .get(..hole_start)
            .ok_or("native message shape hole span unavailable")?;
        let suffix = raw
            .get(hole_end..)
            .ok_or("native message shape hole span unavailable")?;
        let mut digest = Sha256::new();
        digest.update(b"native-message-shape-v1");
        digest.update((prefix.len() as u64).to_le_bytes());
        digest.update(prefix);
        digest.update((suffix.len() as u64).to_le_bytes());
        digest.update(suffix);
        Ok(Some(CertifiedMessageShape {
            source: self.source,
            first,
            last,
            hole_first,
            hole_last,
            item,
            root: (&root).into(),
            hole: (&hole).into(),
            hole_ordinal,
            static_digest: digest.finalize().into(),
        }))
    }

    // Native preorder IDs are contiguous, including reserved imported nodes.
    // Exact unsaturated counts need no descent; saturated counts retain the
    // bounded last-child walk rather than enumerating a member collection.
    fn message_last_node(
        &self,
        connection: &rusqlite::Connection,
        mut id: u64,
        node: &Node,
    ) -> Result<u64, &'static str> {
        // The native parser saturates only at33. An unsaturated count is exact,
        // and both parsed/imported descendants reserve contiguous preorder IDs.
        if (1..=32).contains(&node.recursive_nodes) {
            return id
                .checked_add(node.recursive_nodes - 1)
                .filter(|last| *last < self.source.layout.next_node)
                .ok_or("native message shape preorder unavailable");
        }
        for _ in 0..=256 {
            let node = self.source.layout.indexed_node(connection, id)?;
            if node.children == 0 {
                return Ok(id);
            }
            let children =
                self.source
                    .layout
                    .indexed_children(connection, id, node.children - 1, 1)?;
            let Some((last, ordinal)) = children.first() else {
                return Err("native message shape last child unavailable");
            };
            if *ordinal != node.children - 1 || *last <= id {
                return Err("native message shape preorder unavailable");
            }
            id = *last;
        }
        Err("native message shape depth exceeds bound")
    }

    /// Static descendants only: paths, parent paths, value spans and key spans
    /// are relative to the message root. The original property ordinal remains.
    /// No fact vector is retained; the finite evidence stream is capped at64KiB.
    pub(crate) fn visit_message_shape_fields(
        &self,
        shape: &CertifiedMessageShape<'_>,
        mut preview_policy: impl FnMut(&[String]) -> FieldPreviewPolicy,
        mut visit: impl FnMut(
            &[String],
            &str,
            Option<(&str, u64)>,
            u64,
            u64,
            u64,
            Option<(u64, u64)>,
            FieldPreview<'_>,
        ) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        if !shape.owns(self.source) {
            return Err("native message shape belongs to another source");
        }
        let mut charged = 0_usize;
        let prefix = serde_json::to_string(&["messages", &shape.item.to_string()])
            .map_err(|_| "native message shape prefix unavailable")?;
        self.source.visit_selected_fields_in(
            &self.reader,
            shape.first,
            shape.last,
            |id| id == shape.first || shape.static_field(id),
            |path| {
                if path.is_empty() {
                    FieldPreviewPolicy::Skip
                } else {
                    preview_policy(path)
                }
            },
            |path, encoded, parent, start, end, children, key, preview| {
                if path.is_empty() {
                    return Ok(());
                }
                if prefix.len() + encoded.len() - 1 > 3600 {
                    return Err("native layout path exceeds bound");
                }
                let header = match preview {
                    FieldPreview::Inline(value) => {
                        crate::runtime_client::checkpoint::bounded_bytes(value, 16_384)
                            .map_or(4, |bytes| bytes.len())
                    }
                    _ => 0,
                };
                charged = charged
                    .checked_add(
                        encoded.len() + parent.map_or(0, |(path, _)| path.len()) + 64 + header,
                    )
                    .filter(|bytes| *bytes <= MAX_FACT_BYTES)
                    .ok_or("native message shape facts exceed bound")?;
                visit(
                    path,
                    encoded,
                    parent,
                    start - shape.root.start,
                    end - shape.root.start,
                    children,
                    key.map(|(start, end)| (start - shape.root.start, end - shape.root.start)),
                    preview,
                )
            },
        )
    }

    /// Exact own-hole facts from this same immutable source. Geometry is
    /// relative to the hole; the certificate retains its actual root ordinal.
    pub(crate) fn visit_message_hole_fields(
        &self,
        shape: &CertifiedMessageShape<'_>,
        visit: impl FnMut(
            &[String],
            &str,
            Option<(&str, u64)>,
            u64,
            u64,
            u64,
            Option<(u64, u64)>,
            FieldPreview<'_>,
        ) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        if !shape.owns(self.source)
            || !(1..=32).contains(&shape.hole.recursive_nodes)
            || shape.hole.end - shape.hole.start > 16_384
        {
            return Err("native message hole exceeds bound");
        }
        let prefix = serde_json::to_string(&["messages", "", "provenance_source"])
            .map_err(|_| "native own hole prefix unavailable")?;
        let prefix_bytes = prefix.len() + shape.item.to_string().len();
        let mut visit = visit;
        self.source.visit_selected_fields_in(
            &self.reader,
            shape.hole_first,
            shape.hole_last,
            |_| true,
            |_| FieldPreviewPolicy::OriginalSpanIfSmall,
            |path, encoded, parent, start, end, children, key, preview| {
                if prefix_bytes + encoded.len() - 1 > 3600 {
                    return Err("native layout path exceeds bound");
                }
                visit(
                    path,
                    encoded,
                    parent,
                    start - shape.hole.start,
                    end - shape.hole.start,
                    children,
                    key.filter(|(a, _)| *a >= shape.hole.start)
                        .map(|(a, b)| (a - shape.hole.start, b - shape.hole.start)),
                    preview,
                )
            },
        )
    }
    /// Packed callers retain both static and own-hole metadata before omitting
    /// any descendant. Default/static-only traversal remains independent.
    pub(crate) fn visit_fields_with_packed_message_shapes(
        &self,
        shapes: &[CertifiedMessageShape<'_>],
        preview_policy: impl FnMut(&[String]) -> FieldPreviewPolicy,
        visit: impl FnMut(
            &[String],
            &str,
            Option<(&str, u64)>,
            u64,
            u64,
            u64,
            Option<(u64, u64)>,
            FieldPreview<'_>,
        ) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        if shapes.len() > MAX_SHAPES
            || shapes.iter().any(|shape| !shape.owns(self.source))
            || shapes.windows(2).any(|pair| pair[0].last >= pair[1].first)
        {
            return Err("native packed message selection unavailable");
        }
        self.source.visit_selected_fields_in(
            &self.reader,
            0,
            SAFE,
            |id| {
                let index = shapes.partition_point(|shape| shape.first <= id);
                index == 0 || id == shapes[index - 1].first || id > shapes[index - 1].last
            },
            preview_policy,
            visit,
        )
    }

    /// Fresh root/hole facts remain in the ordinary traversal. Only descendants
    /// certified against this exact source are omitted before node/path cloning.
    #[cfg(test)]
    pub(crate) fn visit_fields_with_message_shapes(
        &self,
        shapes: &[CertifiedMessageShape<'_>],
        preview_policy: impl FnMut(&[String]) -> FieldPreviewPolicy,
        visit: impl FnMut(
            &[String],
            &str,
            Option<(&str, u64)>,
            u64,
            u64,
            u64,
            Option<(u64, u64)>,
            FieldPreview<'_>,
        ) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        if shapes.len() > MAX_SHAPES
            || shapes.iter().any(|shape| !shape.owns(self.source))
            || shapes.windows(2).any(|pair| pair[0].last >= pair[1].first)
        {
            return Err("native message shape selection unavailable");
        }
        self.source.visit_selected_fields_in(
            &self.reader,
            0,
            SAFE,
            |id| {
                let index = shapes.partition_point(|shape| shape.first <= id);
                index == 0 || !shapes[index - 1].static_field(id)
            },
            preview_policy,
            visit,
        )
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod query_tests;
