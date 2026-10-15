//! Producer-certified immutable subtrees reused only while constructing a new
//! observation source. Every durable request still owns its own bytes/fields.
use super::{Layout, LayoutParser, Node, ObservationReadError, SAFE, SeekableObservationSource};
use serde::Serialize;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::io::{BufRead as _, Write};
use std::rc::Rc;
use std::sync::Arc;

// Separate from the Store's shared fragment-data budget: these are only
// request-local offsets/Arc pins and imported parent/key seats. The doubled
// charge covers simultaneous Vec capacities while parsing moves registrations.
const MAX_FRAGMENT_METADATA_BYTES: usize = 8 * 1024 * 1024;
const SEAT_CHARGE: usize =
    2 * (std::mem::size_of::<Emission>() + std::mem::size_of::<FragmentSeat>() + 256);

#[cfg(test)]
thread_local! { static FRAGMENT_WORK: Cell<(usize,usize)> = const {Cell::new((0,0))}; }
#[cfg(test)]
pub(crate) fn take_fragment_work() -> (usize, usize) {
    FRAGMENT_WORK.with(|work| work.replace((0, 0)))
}

#[derive(Debug)]
struct FragmentNode {
    parent: Option<usize>,
    ordinal: u64,
    node: Node,
    children: Vec<usize>,
    preview: Option<Value>,
}

/// Only `capture_fragment` can certify bytes and native relative node facts.
#[derive(Debug)]
pub(crate) struct TrustedFragment {
    raw: Box<serde_json::value::RawValue>,
    nodes: Vec<FragmentNode>,
    owned_bytes: usize,
    depth: usize,
}
impl TrustedFragment {
    pub(crate) fn raw_bytes(&self) -> &[u8] {
        self.raw.get().as_bytes()
    }
    pub(crate) fn owned_bytes(&self) -> usize {
        self.owned_bytes
    }
    pub(crate) fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub(crate) fn emitted<'a>(
        self: &'a Arc<Self>,
        capture: &'a FragmentCapture,
    ) -> FragmentValue<'a> {
        FragmentValue {
            fragment: self,
            capture,
        }
    }
}

/// Request-local, fixed-capacity registration. A full registration set simply
/// leaves further values on the ordinary parser path.
#[derive(Debug, Clone)]
pub(crate) struct FragmentCapture(Rc<CaptureState>);
#[derive(Debug)]
struct CaptureState {
    active: Cell<bool>,
    position: Cell<u64>,
    maximum: usize,
    emitted: RefCell<Vec<Emission>>,
    charged: Cell<usize>,
}
#[derive(Debug)]
pub(super) struct Emission {
    start: u64,
    fragment: Arc<TrustedFragment>,
}
impl FragmentCapture {
    pub(crate) fn new(maximum: usize) -> Self {
        Self(Rc::new(CaptureState {
            active: Cell::new(false),
            position: Cell::new(0),
            maximum: maximum.min(MAX_FRAGMENT_METADATA_BYTES / SEAT_CHARGE),
            emitted: RefCell::new(Vec::new()),
            charged: Cell::new(0),
        }))
    }
    #[cfg(test)]
    pub(crate) fn metadata_limit(&self) -> usize {
        self.0.maximum * SEAT_CHARGE
    }
    #[cfg(test)]
    pub(crate) fn metadata_bytes(&self) -> usize {
        self.0.charged.get()
    }
}
pub(crate) struct FragmentValue<'a> {
    fragment: &'a Arc<TrustedFragment>,
    capture: &'a FragmentCapture,
}
impl Serialize for FragmentValue<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let state = &self.capture.0;
        if state.active.get() && state.emitted.borrow().len() < state.maximum {
            state.emitted.borrow_mut().push(Emission {
                start: state.position.get(),
                fragment: Arc::clone(self.fragment),
            });
            state
                .charged
                .set(state.emitted.borrow().len() * SEAT_CHARGE);
        }
        self.fragment.raw.serialize(serializer)
    }
}
struct Counter<'a, W> {
    inner: &'a mut W,
    state: &'a CaptureState,
}
impl<W: Write> Write for Counter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.state.position.set(
            self.state
                .position
                .get()
                .checked_add(count as u64)
                .filter(|end| *end <= SAFE)
                .ok_or_else(|| std::io::Error::other("fragment offset exhausted"))?,
        );
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
pub(super) fn encode<T: Serialize + schemars::JsonSchema>(
    writer: &mut impl Write,
    value: &T,
    capture: Option<&FragmentCapture>,
) -> Result<(), &'static str> {
    struct Reset<'a>(&'a CaptureState);
    impl Drop for Reset<'_> {
        fn drop(&mut self) {
            self.0.active.set(false);
        }
    }
    if let Some(capture) = capture {
        let state = &capture.0;
        if state.active.replace(true) {
            return Err("fragment construction already active");
        }
        let _reset = Reset(state);
        state.position.set(0);
        state.emitted.borrow_mut().clear();
        state.charged.set(0);
        crate::runtime_client::structural_json::to_writer(
            &mut Counter {
                inner: writer,
                state,
            },
            value,
        )
        .map_err(|_| "native observation source encoding unavailable")
    } else {
        crate::runtime_client::structural_json::to_writer(writer, value)
            .map_err(|_| "native observation source encoding unavailable")
    }
}
pub(super) fn parse(
    parser: &mut LayoutParser<'_>,
    capture: Option<&FragmentCapture>,
) -> Result<(), &'static str> {
    if let Some(capture) = capture {
        parser.layout.pending_fragments =
            std::mem::take(&mut *capture.0.emitted.borrow_mut()).into();
    }
    parser.value(None, 0, None, 0)?;
    if !parser.layout.pending_fragments.is_empty() {
        return Err("fragment emission was not consumed");
    }
    Ok(())
}

pub(super) enum FragmentField {
    NotImported,
    Missing,
    Unique(u64, Node, u64),
}

pub(super) struct NodeRecord {
    pub id: u64,
    pub parent: Option<u64>,
    pub ordinal: u64,
    pub node: Node,
}
#[derive(Debug)]
pub(super) struct FragmentSeat {
    first: u64,
    start: u64,
    parent: Option<u64>,
    ordinal: u64,
    key: Option<(String, bool, (u64, u64))>,
    fragment: Arc<TrustedFragment>,
}
impl FragmentSeat {
    fn record(&self, index: usize) -> NodeRecord {
        let cached = &self.fragment.nodes[index];
        let mut node = cached.node.clone();
        node.start += self.start;
        node.end += self.start;
        node.key_span = node
            .key_span
            .map(|(start, end)| (start + self.start, end + self.start));
        let parent = if index == 0 {
            node.key = self.key.as_ref().map(|(key, _, _)| key.clone());
            node.key_complete = self.key.as_ref().is_none_or(|(_, complete, _)| *complete);
            node.key_span = self.key.as_ref().map(|(_, _, span)| *span);
            self.parent
        } else {
            cached.parent.map(|parent| self.first + parent as u64)
        };
        NodeRecord {
            id: self.first + index as u64,
            parent,
            ordinal: if index == 0 {
                self.ordinal
            } else {
                cached.ordinal
            },
            node,
        }
    }
}
impl Layout {
    fn fragment_seat(&self, id: u64) -> Option<(&FragmentSeat, usize)> {
        let index = self
            .fragments
            .partition_point(|seat| seat.first <= id)
            .checked_sub(1)?;
        let seat = &self.fragments[index];
        let relative = usize::try_from(id - seat.first).ok()?;
        (relative < seat.fragment.nodes.len()).then_some((seat, relative))
    }
    pub(super) fn fragment_record(&self, id: u64) -> Option<NodeRecord> {
        self.fragment_seat(id)
            .map(|(seat, index)| seat.record(index))
    }
    pub(super) fn fragment_preview(&self, id: u64) -> Option<&Value> {
        self.fragment_seat(id)
            .and_then(|(seat, index)| seat.fragment.nodes[index].preview.as_ref())
    }
    pub(super) fn fragment_children(
        &self,
        parent: u64,
        offset: u64,
        limit: usize,
    ) -> Option<Vec<(u64, u64)>> {
        let (seat, index) = self.fragment_seat(parent)?;
        Some(
            seat.fragment.nodes[index]
                .children
                .iter()
                .filter_map(|child| {
                    let ordinal = seat.fragment.nodes[*child].ordinal;
                    (ordinal >= offset).then_some((seat.first + *child as u64, ordinal))
                })
                .take(limit)
                .collect(),
        )
    }
    pub(super) fn fragment_unique_key(&self, parent: u64, key: &str) -> FragmentField {
        let Some((seat, index)) = self.fragment_seat(parent) else {
            return FragmentField::NotImported;
        };
        let mut matches = seat.fragment.nodes[index].children.iter().filter(|child| {
            let node = &seat.fragment.nodes[**child].node;
            node.key_complete && node.key.as_deref() == Some(key)
        });
        let Some(child) = matches.next() else {
            return FragmentField::Missing;
        };
        if matches.next().is_some() {
            return FragmentField::Missing;
        }
        let selected = seat.record(*child);
        FragmentField::Unique(selected.id, selected.node, selected.ordinal)
    }
    pub(super) fn fragment_select(
        &self,
        parent: u64,
        ordinal: Option<u64>,
        key: &str,
    ) -> Option<Result<u64, ObservationReadError>> {
        let (seat, index) = self.fragment_seat(parent)?;
        let selected = seat.fragment.nodes[index]
            .children
            .iter()
            .find(|child| {
                let node = &seat.fragment.nodes[**child];
                ordinal.map_or_else(
                    || node.node.key_complete && node.node.key.as_deref() == Some(key),
                    |ordinal| node.ordinal == ordinal,
                )
            })
            .map(|child| seat.first + *child as u64);
        Some(selected.ok_or(ObservationReadError::InvalidRequest(
            "invalid pending source path",
        )))
    }
    pub(super) fn visit_selected_records(
        &self,
        connection: &rusqlite::Connection,
        first: u64,
        last: u64,
        mut include: impl FnMut(u64) -> bool,
        mut visit: impl FnMut(NodeRecord) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        // A scoped root may itself be virtual inside an earlier imported seat;
        // its one SQL row precedes this range, so emit the overlap explicitly.
        if let Some((seat, index)) = self.fragment_seat(first)
            && index > 0
        {
            for index in index..seat.fragment.nodes.len() {
                let id = seat.first + index as u64;
                if id <= last && include(id) {
                    visit(seat.record(index))?;
                }
            }
        }
        let mut rows = connection
            .prepare("SELECT id,parent,ordinal,start,end,children,recursive_nodes,container,object,key,key_start,key_end,key_complete FROM nodes WHERE id>=?1 AND id<=?2 ORDER BY id")
            .map_err(|_| "native layout traversal unavailable")?;
        let mut rows = rows
            .query(rusqlite::params![first, last])
            .map_err(|_| "native layout traversal unavailable")?;
        while let Some(row) = rows
            .next()
            .map_err(|_| "native layout traversal unavailable")?
        {
            let id = row
                .get(0)
                .map_err(|_| "native layout identity unavailable")?;
            if let Some((seat, 0)) = self.fragment_seat(id) {
                for index in 0..seat.fragment.nodes.len() {
                    let id = seat.first + index as u64;
                    if id <= last && include(id) {
                        visit(seat.record(index))?;
                    }
                }
            } else if include(id) {
                visit(NodeRecord {
                    id,
                    parent: row.get(1).map_err(|_| "native layout parent unavailable")?,
                    ordinal: row
                        .get(2)
                        .map_err(|_| "native layout ordinal unavailable")?,
                    node: Node::from_row(row, 3).map_err(|_| "pending source node unavailable")?,
                })?;
            }
        }
        Ok(())
    }
}
impl LayoutParser<'_> {
    pub(super) fn try_fragment(
        &mut self,
        parent: Option<u64>,
        ordinal: u64,
        key: Option<&(String, bool, (u64, u64))>,
        depth: usize,
    ) -> Result<Option<(u64, u64)>, &'static str> {
        let Some(next) = self.layout.pending_fragments.front() else {
            return Ok(None);
        };
        if next.start > self.position {
            return Ok(None);
        }
        if next.start < self.position {
            return Err("fragment emission offset changed");
        }
        let emission = self
            .layout
            .pending_fragments
            .pop_front()
            .ok_or("fragment emission unavailable")?;
        if depth
            .checked_add(emission.fragment.depth)
            .is_none_or(|depth| depth > 256)
        {
            return Err("native source index limit reached");
        }
        let start = self.position;
        let mut at = 0;
        while at < emission.fragment.raw_bytes().len() {
            let available = self
                .reader
                .fill_buf()
                .map_err(|_| "native fragment bytes unavailable")?;
            let count = available
                .len()
                .min(emission.fragment.raw_bytes().len() - at);
            if count == 0 || available[..count] != emission.fragment.raw_bytes()[at..at + count] {
                return Err("native fragment bytes differ from certified source");
            }
            self.consume(count);
            at += count;
        }
        let first = self.layout.next_node;
        self.layout.next_node = first
            .checked_add(emission.fragment.nodes.len() as u64)
            .filter(|end| *end <= SAFE)
            .ok_or("native source nodes exhausted")?;
        let recursive = emission.fragment.nodes[0].node.recursive_nodes;
        let seat = FragmentSeat {
            first,
            start,
            parent,
            ordinal,
            key: key.cloned(),
            fragment: emission.fragment,
        };
        let root = seat.record(0);
        self.index_write(
            "INSERT INTO nodes VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            rusqlite::params![
                first,
                parent,
                ordinal,
                root.node.start,
                root.node.end,
                root.node.children,
                root.node.recursive_nodes,
                root.node.container,
                root.node.object,
                root.node.key,
                root.node.key_span.map(|span| span.0),
                root.node.key_span.map(|span| span.1),
                root.node.key_complete
            ],
        )?;
        #[cfg(test)]
        FRAGMENT_WORK.with(|work| {
            let (values, nodes) = work.get();
            work.set((values + 1, nodes + seat.fragment.nodes.len() - 1));
        });
        self.layout.fragments.push(seat);
        Ok(Some((first, recursive)))
    }
}

impl SeekableObservationSource {
    /// A miss never changes admission: the caller keeps the ordinary producer.
    pub(crate) fn capture_fragment(
        &self,
        path: &[String],
        byte_budget: usize,
        node_budget: usize,
    ) -> Result<Option<Arc<TrustedFragment>>, &'static str> {
        let byte_budget = byte_budget.min(65_536);
        let node_budget = node_budget.min(512);
        let root = self
            .node(path)
            .map_err(|_| "native fragment path unavailable")?;
        let connection = self.layout.reader_connection()?;
        let node = self.layout.indexed_node(&connection, root)?;
        let length = usize::try_from(node.end - node.start)
            .map_err(|_| "native fragment span unavailable")?;
        if length > byte_budget || node_budget == 0 {
            return Ok(None);
        }
        let raw = self.layout.bytes(node.start, length)?;
        let bytes = std::fs::File::open(self.layout.directory.path().join("bytes"))
            .map_err(|_| "native fragment bytes unavailable")?;
        let mut pending: Vec<(u64, Option<usize>, u64, usize)> = vec![(root, None, 0, 0)];
        let mut nodes: Vec<FragmentNode> = Vec::new();
        let mut owned =
            length + std::mem::size_of::<TrustedFragment>() + 2 * std::mem::size_of::<usize>();
        let mut maximum_depth = 0;
        while let Some((id, parent, ordinal, depth)) = pending.pop() {
            if nodes.len() == node_budget {
                return Ok(None);
            }
            let mut current = self.layout.indexed_node(&connection, id)?;
            if current.children > (node_budget - nodes.len() - pending.len() - 1) as u64 {
                return Ok(None);
            }
            // Source serves these nodes directly as OriginalSpan. Other
            // visitors can still obtain their exact preview lazily from the
            // certified bytes; retaining it here would duplicate every small
            // nested value across rounds without serving the Source writer.
            let preview = (!(current.recursive_nodes <= 32
                && current.end - current.start <= 16_384))
                .then(|| self.preview_node(&connection, &bytes, id, &current, &mut 32))
                .transpose()?;
            owned = owned
                .checked_add(std::mem::size_of::<FragmentNode>())
                .and_then(|size| size.checked_add(current.key.as_ref().map_or(0, String::capacity)))
                .and_then(|size| size.checked_add(preview.as_ref().map_or(0, value_owned)))
                .ok_or("native fragment allocation exhausted")?;
            let children = self
                .layout
                .indexed_children(&connection, id, 0, node_budget + 1)?;
            owned = owned
                .checked_add(children.len() * std::mem::size_of::<usize>())
                .ok_or("native fragment allocation exhausted")?;
            if owned > byte_budget {
                return Ok(None);
            }
            let index = nodes.len();
            if let Some(parent) = parent {
                nodes[parent].children.reserve_exact(1);
                nodes[parent].children.push(index);
            }
            for (child, ordinal) in children.into_iter().rev() {
                pending.push((child, Some(index), ordinal, depth + 1));
            }
            current.start -= node.start;
            current.end -= node.start;
            if index == 0 {
                current.key = None;
                current.key_span = None;
                current.key_complete = true;
            } else {
                current.key_span = current
                    .key_span
                    .map(|(start, end)| (start - node.start, end - node.start));
            }
            nodes.reserve_exact(1);
            nodes.push(FragmentNode {
                parent,
                ordinal,
                node: current,
                children: Vec::new(),
                preview,
            });
            maximum_depth = maximum_depth.max(depth);
        }
        let raw = String::from_utf8(raw).map_err(|_| "native fragment UTF-8 unavailable")?;
        // Account retained capacities, not only logical member lengths.
        owned += (nodes.capacity() - nodes.len()) * std::mem::size_of::<FragmentNode>();
        owned += nodes
            .iter()
            .map(|node| {
                (node.children.capacity() - node.children.len()) * std::mem::size_of::<usize>()
            })
            .sum::<usize>();
        if owned > byte_budget {
            return Ok(None);
        }
        let Ok(raw) = serde_json::value::RawValue::from_string(raw) else {
            return Ok(None);
        };
        Ok(Some(Arc::new(TrustedFragment {
            raw,
            nodes,
            owned_bytes: owned,
            depth: maximum_depth,
        })))
    }
}
fn value_owned(value: &Value) -> usize {
    match value {
        Value::String(value) => value.capacity(),
        Value::Array(values) => {
            values.capacity() * std::mem::size_of::<Value>()
                + values.iter().map(value_owned).sum::<usize>()
        }
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                8 * (std::mem::size_of::<String>()
                    + std::mem::size_of::<Value>()
                    + 4 * std::mem::size_of::<usize>())
                    + key.capacity()
                    + value_owned(value)
            })
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
pub(super) mod tests;
