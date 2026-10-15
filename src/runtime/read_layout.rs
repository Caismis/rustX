//! Seekable immutable observation bytes derived from existing native authority.
pub(crate) mod canonical;
mod fragments;
#[cfg(test)]
pub(crate) use fragments::take_fragment_work;
pub(crate) use fragments::{FragmentCapture, TrustedFragment};
mod key_order;
mod message_shapes;
pub(crate) use message_shapes::{CertifiedMessageShape, MessageShapeReader};
use rusqlite::OptionalExtension as _;
use serde_json::Value;
use std::sync::Arc;
const ITEM_BYTES: usize = 16_384;
const SAFE: u64 = 9_007_199_254_740_991;
#[cfg(test)]
thread_local! { pub(crate) static READ_WORK: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) }; }
#[cfg(test)]
thread_local! {pub(crate) static INDEX_OPENS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[cfg(test)]
thread_local! {static NODE_LOOKUPS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[cfg(test)]
thread_local! {static CHILD_LOOKUPS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[cfg(test)]
thread_local! {static UNIQUE_KEY_LOOKUPS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[cfg(test)]
thread_local! {static NODE_SELECTIONS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[cfg(test)]
thread_local! {static NODE_WRITES:std::cell::Cell<(usize,usize)>=const {std::cell::Cell::new((0,0))};}
#[cfg(test)]
thread_local! {static FIELD_PREVIEWS:std::cell::Cell<(usize,usize)>=const {std::cell::Cell::new((0,0))};}
#[cfg(test)]
thread_local! {static PATH_WORK:std::cell::Cell<(usize,usize)>=const {std::cell::Cell::new((0,0))};}
#[cfg(test)]
pub(crate) fn take_field_preview_work() -> (usize, usize) {
    FIELD_PREVIEWS.with(|work| work.replace((0, 0)))
}

/// The consumer chooses whether it needs a computed preview at this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldPreviewPolicy {
    Inline,
    OriginalSpanIfSmall,
    Skip,
}

/// An original span is admitted only from the parser's actual node/byte counts.
/// It carries no replacement bytes or independently captured source authority.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FieldPreview<'a> {
    Inline(&'a Value),
    OriginalSpan { recursive_nodes: u64 },
    Skipped,
}

/// Immutable source reads have no mutable lease or cursor-expiration authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservationReadError {
    InvalidRequest(&'static str),
    Unavailable(&'static str),
}
impl From<&'static str> for ObservationReadError {
    fn from(message: &'static str) -> Self {
        Self::Unavailable(message)
    }
}
impl std::fmt::Display for ObservationReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::InvalidRequest(message) | Self::Unavailable(message)) = self;
        formatter.write_str(message)
    }
}
impl std::error::Error for ObservationReadError {}

#[cfg(test)]
thread_local! {
    static FILE_WORK: std::cell::Cell<(usize,usize)> = const {std::cell::Cell::new((0,0))};
    static UNBUFFERED_CONSTRUCTION: std::cell::Cell<bool> = const {std::cell::Cell::new(false)};
}

#[derive(Debug, Clone)]
struct Node {
    start: u64,
    end: u64,
    children: u64,
    recursive_nodes: u64,
    container: bool,
    object: bool,
    key: Option<String>,
    key_span: Option<(u64, u64)>,
    key_complete: bool,
}
impl Node {
    fn from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        let key_start: Option<u64> = row.get(offset + 7)?;
        let key_end: Option<u64> = row.get(offset + 8)?;
        Ok(Self {
            start: row.get(offset)?,
            end: row.get(offset + 1)?,
            children: row.get(offset + 2)?,
            recursive_nodes: row.get(offset + 3)?,
            container: row.get(offset + 4)?,
            object: row.get(offset + 5)?,
            key: row.get(offset + 6)?,
            key_span: key_start.zip(key_end),
            key_complete: row.get(offset + 9)?,
        })
    }
}
#[derive(Debug)]
struct Layout {
    directory: tempfile::TempDir,
    file: Option<std::fs::File>,
    connection: std::sync::Mutex<Option<rusqlite::Connection>>,
    written: u64,
    next_node: u64,
    deadline: Option<std::time::Instant>,
    fragments: Vec<fragments::FragmentSeat>,
    pending_fragments: std::collections::VecDeque<fragments::Emission>,
}

enum LayoutConnection<'a> {
    Building(std::sync::MutexGuard<'a, Option<rusqlite::Connection>>),
    Reading(rusqlite::Connection),
}
impl std::ops::Deref for LayoutConnection<'_> {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Building(guard) => guard.as_ref().expect("native construction owns index"),
            Self::Reading(connection) => connection,
        }
    }
}

impl std::io::Write for Layout {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.expired() {
            return Err(std::io::Error::other(
                "pending source indexing deadline reached",
            ));
        }
        #[cfg(test)]
        FILE_WORK.with(|work| {
            let (writes, opens) = work.get();
            work.set((writes + 1, opens));
        });
        let count = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("source sealed"))?
            .write(&bytes[..bytes.len().min(65_536)])?;
        self.written = self
            .written
            .checked_add(count as u64)
            .filter(|end| *end <= SAFE)
            .ok_or_else(|| std::io::Error::other("pending source length exhausted"))?;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Layout {
    fn expired(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    }
    fn seal(&mut self) -> Result<(), &'static str> {
        self.connection
            .get_mut()
            .map_err(|_| "native source index poisoned")?
            .as_mut()
            .ok_or("native source already sealed")?
            .execute_batch("CREATE UNIQUE INDEX nodes_ordinal ON nodes(parent,ordinal); CREATE INDEX nodes_key ON nodes(parent,key) WHERE key_complete=1; COMMIT")
            .map_err(|_| "native source index commit unavailable")?;
        self.file = None;
        *self
            .connection
            .get_mut()
            .map_err(|_| "native source index poisoned")? = None;
        Ok(())
    }
    fn reader_connection(&self) -> Result<LayoutConnection<'_>, &'static str> {
        let guard = self
            .connection
            .lock()
            .map_err(|_| "native source index poisoned")?;
        if guard.is_some() {
            return Ok(LayoutConnection::Building(guard));
        }
        drop(guard);
        #[cfg(test)]
        INDEX_OPENS.with(|count| count.set(count.get() + 1));
        let connection = rusqlite::Connection::open_with_flags(
            self.directory.path().join("index.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|_| "native source index unavailable")?;
        connection
            .execute_batch("PRAGMA cache_size=-1024")
            .map_err(|_| "native source cache unavailable")?;
        Ok(LayoutConnection::Reading(connection))
    }

    fn new() -> Result<Self, &'static str> {
        let directory = tempfile::tempdir().map_err(|_| "pending source directory unavailable")?;
        let file = std::fs::File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path().join("bytes"))
            .map_err(|_| "pending source file unavailable")?;
        let connection = rusqlite::Connection::open(directory.path().join("index.sqlite"))
            .map_err(|_| "pending source index unavailable")?;
        // This ephemeral immutable layout is derived from the original Arc;
        // it is never a durable interaction journal or settlement authority.
        connection.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-1024; CREATE TABLE nodes(id INTEGER PRIMARY KEY,parent INTEGER,ordinal INTEGER NOT NULL,start INTEGER NOT NULL,end INTEGER NOT NULL,children INTEGER NOT NULL,recursive_nodes INTEGER NOT NULL,container INTEGER NOT NULL,object INTEGER NOT NULL,key TEXT,key_start INTEGER,key_end INTEGER,key_complete INTEGER NOT NULL); BEGIN;").map_err(|_|"pending source index unavailable")?;
        Ok(Self {
            directory,
            file: Some(file),
            connection: std::sync::Mutex::new(Some(connection)),
            written: 0,
            next_node: 0,
            deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(5)),
            fragments: Vec::new(),
            pending_fragments: std::collections::VecDeque::new(),
        })
    }
    fn node(&self, id: u64) -> Result<Node, &'static str> {
        if let Some(record) = self.fragment_record(id) {
            return Ok(record.node);
        }
        let connection = self.reader_connection()?;
        Self::node_at(&connection, id)
    }
    fn indexed_node(
        &self,
        connection: &rusqlite::Connection,
        id: u64,
    ) -> Result<Node, &'static str> {
        self.fragment_record(id)
            .map_or_else(|| Self::node_at(connection, id), |record| Ok(record.node))
    }
    fn node_at(connection: &rusqlite::Connection, id: u64) -> Result<Node, &'static str> {
        #[cfg(test)]
        NODE_LOOKUPS.with(|count| count.set(count.get() + 1));
        connection.prepare_cached("SELECT start,end,children,recursive_nodes,container,object,key,key_start,key_end,key_complete FROM nodes WHERE id=?1").map_err(|_|"pending source node unavailable")?.query_row([id],|row| Node::from_row(row,0)).map_err(|_|"pending source node unavailable")
    }
    fn select(
        &self,
        parent: u64,
        ordinal: Option<u64>,
        key: &str,
    ) -> Result<u64, ObservationReadError> {
        if let Some(result) = self.fragment_select(parent, ordinal, key) {
            return result;
        }
        let connection = self.reader_connection()?;
        self.indexed_select(&connection, parent, ordinal, key)
    }
    fn indexed_select(
        &self,
        connection: &rusqlite::Connection,
        parent: u64,
        ordinal: Option<u64>,
        key: &str,
    ) -> Result<u64, ObservationReadError> {
        if let Some(result) = self.fragment_select(parent, ordinal, key) {
            return result;
        }
        #[cfg(test)]
        NODE_SELECTIONS.with(|work| work.set(work.get() + 1));
        if let Some(ordinal) = ordinal {
            connection
                .prepare_cached("SELECT id FROM nodes WHERE parent=?1 AND ordinal=?2")
                .and_then(|mut statement| {
                    statement.query_row(rusqlite::params![parent, ordinal], |row| row.get(0))
                })
        } else {
            connection
                .prepare_cached(
                    "SELECT id FROM nodes WHERE parent=?1 AND key=?2 AND key_complete=1",
                )
                .and_then(|mut statement| {
                    statement.query_row(rusqlite::params![parent, key], |row| row.get(0))
                })
        }
        .optional()
        .map_err(|_| ObservationReadError::Unavailable("native source path index unavailable"))?
        .ok_or(ObservationReadError::InvalidRequest(
            "invalid pending source path",
        ))
    }
    fn indexed_unique_key(
        &self,
        connection: &rusqlite::Connection,
        parent: u64,
        key: &str,
    ) -> Result<Option<(u64, Node, u64)>, &'static str> {
        match self.fragment_unique_key(parent, key) {
            fragments::FragmentField::NotImported => {}
            fragments::FragmentField::Missing => return Ok(None),
            fragments::FragmentField::Unique(id, node, ordinal) => {
                return Ok(Some((id, node, ordinal)));
            }
        }
        #[cfg(test)]
        UNIQUE_KEY_LOOKUPS.with(|work| work.set(work.get() + 1));
        let mut statement = connection.prepare_cached(
            "SELECT id,start,end,children,recursive_nodes,container,object,key,key_start,key_end,key_complete,ordinal FROM nodes WHERE parent=?1 AND key=?2 AND key_complete=1 LIMIT 2"
        ).map_err(|_| "native unique field lookup unavailable")?;
        let mut rows = statement
            .query(rusqlite::params![parent, key])
            .map_err(|_| "native unique field lookup unavailable")?;
        let Some(row) = rows
            .next()
            .map_err(|_| "native unique field lookup unavailable")?
        else {
            return Ok(None);
        };
        let selected = (
            row.get(0)
                .map_err(|_| "native unique field identity unavailable")?,
            Node::from_row(row, 1).map_err(|_| "native unique field facts unavailable")?,
            row.get(11)
                .map_err(|_| "native unique field ordinal unavailable")?,
        );
        if rows
            .next()
            .map_err(|_| "native unique field lookup unavailable")?
            .is_some()
        {
            return Ok(None);
        }
        Ok(Some(selected))
    }
    fn children(
        &self,
        parent: u64,
        offset: u64,
        limit: usize,
    ) -> Result<Vec<(u64, u64)>, &'static str> {
        if let Some(children) = self.fragment_children(parent, offset, limit) {
            return Ok(children);
        }
        let connection = self.reader_connection()?;
        Self::children_at(&connection, parent, offset, limit)
    }
    fn indexed_children(
        &self,
        connection: &rusqlite::Connection,
        parent: u64,
        offset: u64,
        limit: usize,
    ) -> Result<Vec<(u64, u64)>, &'static str> {
        self.fragment_children(parent, offset, limit)
            .map_or_else(|| Self::children_at(connection, parent, offset, limit), Ok)
    }
    fn children_at(
        connection: &rusqlite::Connection,
        parent: u64,
        offset: u64,
        limit: usize,
    ) -> Result<Vec<(u64, u64)>, &'static str> {
        #[cfg(test)]
        CHILD_LOOKUPS.with(|work| work.set(work.get() + 1));
        let mut statement=connection.prepare_cached("SELECT id,ordinal FROM nodes WHERE parent=?1 AND ordinal>=?2 ORDER BY ordinal LIMIT ?3").map_err(|_|"pending source page unavailable")?;
        statement
            .query_map(rusqlite::params![parent, offset, limit], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|_| "pending source page unavailable")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "pending source page unavailable")
    }
    fn bytes(&self, start: u64, count: usize) -> Result<Vec<u8>, &'static str> {
        #[cfg(test)]
        FILE_WORK.with(|work| {
            let (writes, opens) = work.get();
            work.set((writes, opens + 1));
        });
        let file = self
            .file
            .as_ref()
            .map_or_else(
                || std::fs::File::open(self.directory.path().join("bytes")),
                std::fs::File::try_clone,
            )
            .map_err(|_| "native byte source unavailable")?;
        Self::read_bytes(&file, start, count)
    }
    fn read_bytes(file: &std::fs::File, start: u64, count: usize) -> Result<Vec<u8>, &'static str> {
        use std::os::unix::fs::FileExt as _;
        let mut bytes = vec![0; count];
        file.read_exact_at(&mut bytes, start)
            .map_err(|_| "pending byte source unavailable")?;
        #[cfg(test)]
        READ_WORK.with(|work| {
            let (rows, materialized) = work.get();
            work.set((rows + usize::from(count > 0), materialized + count));
        });
        Ok(bytes)
    }
}

/// Immutable native display bytes. This layout is source-owned, built once
/// outside semantic locks; reading a later chunk never re-serializes the value.
#[derive(Debug)]
pub(crate) struct SeekableObservationSource {
    layout: Layout,
}
struct FieldReader<'a> {
    connection: LayoutConnection<'a>,
    bytes: std::fs::File,
}
pub(crate) type ObservationLayoutCache =
    Arc<std::sync::OnceLock<Result<SeekableObservationSource, &'static str>>>;
fn append_field_path(parent: &str, component: &str) -> Result<String, &'static str> {
    let component =
        serde_json::to_string(component).map_err(|_| "native layout path unavailable")?;
    let length = parent
        .len()
        .checked_add(component.len())
        .and_then(|length| length.checked_add(usize::from(parent.len() > 2)))
        .filter(|length| *length <= 3600)
        .ok_or("native layout path exceeds bound")?;
    let mut encoded = String::with_capacity(length);
    encoded.push_str(
        parent
            .strip_suffix(']')
            .ok_or("native layout parent path invalid")?,
    );
    if parent.len() > 2 {
        encoded.push(',');
    }
    encoded.push_str(&component);
    encoded.push(']');
    Ok(encoded)
}
impl SeekableObservationSource {
    pub(crate) fn new<T: serde::Serialize + schemars::JsonSchema>(
        value: &T,
    ) -> Result<Self, &'static str> {
        Self::build(value, None)
    }
    pub(crate) fn new_with_fragments<T: serde::Serialize + schemars::JsonSchema>(
        value: &T,
        capture: &FragmentCapture,
    ) -> Result<Self, &'static str> {
        Self::build(value, Some(capture))
    }
    fn build<T: serde::Serialize + schemars::JsonSchema>(
        value: &T,
        capture: Option<&FragmentCapture>,
    ) -> Result<Self, &'static str> {
        let mut layout = Layout::new()?;
        #[cfg(test)]
        let buffered = !UNBUFFERED_CONSTRUCTION.with(std::cell::Cell::get);
        #[cfg(not(test))]
        let buffered = true;
        if buffered {
            use std::io::Write as _;
            let mut writer = std::io::BufWriter::with_capacity(65_536, &mut layout);
            fragments::encode(&mut writer, value, capture)?;
            writer
                .flush()
                .map_err(|_| "native observation source flush unavailable")?;
        } else {
            fragments::encode(&mut layout, value, capture)?;
        }
        let reader = layout
            .file
            .as_ref()
            .ok_or("source sealed")?
            .try_clone()
            .map_err(|_| "native source reader unavailable")?;
        let mut reader = std::io::BufReader::with_capacity(65536, reader);
        std::io::Seek::rewind(&mut reader).map_err(|_| "native source seek unavailable")?;
        {
            let mut parser = LayoutParser {
                reader,
                position: 0,
                layout: &mut layout,
            };
            fragments::parse(&mut parser, capture)?;
        }
        layout.seal()?;
        Ok(Self { layout })
    }
    #[cfg(test)]
    pub(crate) fn profile_unbuffered<R>(read: impl FnOnce() -> R) -> R {
        struct Reset(bool);
        impl Drop for Reset {
            fn drop(&mut self) {
                UNBUFFERED_CONSTRUCTION.with(|flag| flag.set(self.0));
            }
        }
        let _reset = Reset(UNBUFFERED_CONSTRUCTION.with(|flag| flag.replace(true)));
        read()
    }

    /// Copies this completed source's exact original extent into its native owner.
    /// The borrow pins the source directory until the synchronous copy finishes;
    /// each window is bounded and does not reopen or query the field index.
    pub(crate) fn copy_original_bytes_to(
        &self,
        destination: &mut impl std::io::Write,
    ) -> std::io::Result<u64> {
        use std::os::unix::fs::FileExt as _;
        let file = std::fs::File::open(self.layout.directory.path().join("bytes"))?;
        #[cfg(test)]
        FILE_WORK.with(|work| {
            let (writes, opens) = work.get();
            work.set((writes, opens + 1));
        });
        let mut buffer = vec![0; 65_536];
        let mut offset = 0;
        while offset < self.layout.written {
            let count = (self.layout.written - offset).min(65_536);
            let count_usize = usize::try_from(count).map_err(|_| {
                std::io::Error::other("native copy range exceeds platform capacity")
            })?;
            file.read_exact_at(&mut buffer[..count_usize], offset)?;
            #[cfg(test)]
            READ_WORK.with(|work| {
                let (rows, bytes) = work.get();
                work.set((rows + 1, bytes + count_usize));
            });
            destination.write_all(&buffer[..count_usize])?;
            offset += count;
        }
        Ok(offset)
    }

    /// Reads a trusted span already supplied by this immutable layout's visitor.
    /// The native producer avoids resolving the same path and reopening its index.
    pub(crate) fn indexed_span(
        &self,
        start: u64,
        end: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, ObservationReadError> {
        if max_bytes == 0
            || max_bytes > 65_536
            || start > end
            || end > self.layout.written
            || end - start > u64::from(max_bytes)
        {
            return Err(ObservationReadError::InvalidRequest(
                "invalid native indexed span",
            ));
        }
        let count = usize::try_from(end - start)
            .map_err(|_| "native indexed span exceeds platform capacity")?;
        Ok(self.layout.bytes(start, count)?)
    }

    /// The default visitor requests every original finite preview.
    #[cfg(test)]
    pub(crate) fn visit_fields(
        &self,
        mut visit: impl FnMut(
            &[String],
            &str,
            Option<(&str, u64)>,
            u64,
            u64,
            u64,
            Option<(u64, u64)>,
            &Value,
        ) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        self.visit_fields_selecting_previews(
            |_| FieldPreviewPolicy::Inline,
            |path, encoded, parent, start, end, children, key, preview| {
                let FieldPreview::Inline(preview) = preview else {
                    return Err("requested native preview unavailable");
                };
                visit(path, encoded, parent, start, end, children, key, preview)
            },
        )
    }

    /// Streams every original field coordinate. A consumer with an already
    /// captured header can decline only its unused preview, without skipping
    /// parsing, path validation, native bytes, children or descendant fields.
    pub(crate) fn visit_fields_selecting_previews(
        &self,
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
        self.visit_selected_fields(0, SAFE, |_| true, preview_policy, visit)
    }
    fn visit_selected_fields(
        &self,
        first: u64,
        last: u64,
        include: impl FnMut(u64) -> bool,
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
        let reader = self.field_reader()?;
        self.visit_selected_fields_in(&reader, first, last, include, preview_policy, visit)
    }
    fn field_reader(&self) -> Result<FieldReader<'_>, &'static str> {
        #[cfg(test)]
        FILE_WORK.with(|work| {
            let (writes, opens) = work.get();
            work.set((writes, opens + 1));
        });
        Ok(FieldReader {
            bytes: std::fs::File::open(self.layout.directory.path().join("bytes"))
                .map_err(|_| "native byte source unavailable")?,
            connection: self.layout.reader_connection()?,
        })
    }
    fn visit_selected_fields_in(
        &self,
        reader: &FieldReader<'_>,
        first: u64,
        last: u64,
        mut include: impl FnMut(u64) -> bool,
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
        let mut stack: Vec<(u64, usize, String, bool)> = Vec::new();
        let mut path = Vec::new();
        let (connection, bytes) = (&reader.connection, &reader.bytes);
        let mut process = |record: fragments::NodeRecord| -> Result<(), &'static str> {
            let fragments::NodeRecord {
                id,
                mut parent,
                ordinal,
                node,
            } = record;
            if id == first {
                parent = None;
            }
            while stack.last().map(|(id, _, _, _)| *id) != parent {
                stack.pop().ok_or("native layout parent ordering invalid")?;
            }
            path.truncate(stack.last().map_or(0, |(_, depth, _, _)| *depth));
            let path_json = if let Some((_, _, parent_path, object)) = stack.last() {
                let component = if *object {
                    node.key
                        .as_ref()
                        .filter(|_| node.key_complete)
                        .filter(|key| !key.starts_with('@') && key.as_str() != "$key")
                        .cloned()
                        .unwrap_or_else(|| format!("@{ordinal}"))
                } else {
                    ordinal.to_string()
                };
                let encoded = append_field_path(parent_path, &component)?;
                #[cfg(test)]
                PATH_WORK.with(|work| {
                    let (encoded, reused) = work.get();
                    work.set((encoded + 1, reused + path.len()));
                });
                path.push(component);
                encoded
            } else {
                "[]".to_owned()
            };
            let policy = preview_policy(&path);
            let original_span = policy == FieldPreviewPolicy::OriginalSpanIfSmall
                && (1..=32).contains(&node.recursive_nodes)
                && node
                    .end
                    .checked_sub(node.start)
                    .is_some_and(|bytes| bytes > 0 && bytes <= 16_384);
            let requested = policy == FieldPreviewPolicy::Inline
                || (policy == FieldPreviewPolicy::OriginalSpanIfSmall && !original_span);
            #[cfg(test)]
            FIELD_PREVIEWS.with(|work| {
                let (computed, skipped) = work.get();
                work.set((
                    computed + usize::from(requested),
                    skipped + usize::from(!requested),
                ));
            });
            let value = requested
                .then(|| {
                    self.layout.fragment_preview(id).map_or_else(
                        || {
                            self.preview_node(connection, bytes, id, &node, &mut 32)
                                .map(std::borrow::Cow::Owned)
                        },
                        |value| Ok(std::borrow::Cow::Borrowed(value)),
                    )
                })
                .transpose()?;
            let preview = if let Some(value) = &value {
                FieldPreview::Inline(value.as_ref())
            } else if original_span {
                FieldPreview::OriginalSpan {
                    recursive_nodes: node.recursive_nodes,
                }
            } else {
                FieldPreview::Skipped
            };
            visit(
                &path,
                &path_json,
                stack.last().map(|(_, _, path, _)| (path.as_str(), ordinal)),
                node.start,
                node.end,
                node.children,
                node.key_span,
                preview,
            )?;
            stack.push((id, path.len(), path_json, node.object));
            Ok(())
        };
        self.layout
            .visit_selected_records(connection, first, last, &mut include, &mut process)?;
        Ok(())
    }
    fn preview_node(
        &self,
        connection: &rusqlite::Connection,
        bytes: &std::fs::File,
        id: u64,
        node: &Node,
        budget: &mut usize,
    ) -> Result<Value, &'static str> {
        if *budget == 0 {
            return Ok(Value::Null);
        }
        *budget -= 1;
        if node.recursive_nodes <= 32 && node.end - node.start <= 16_384 {
            *budget = budget.saturating_sub(
                usize::try_from(node.recursive_nodes.saturating_sub(1))
                    .map_err(|_| "native preview count unavailable")?,
            );
            return serde_json::from_slice(&Layout::read_bytes(
                bytes,
                node.start,
                usize::try_from(node.end - node.start)
                    .map_err(|_| "native preview length unavailable")?,
            )?)
            .map_err(|_| "native preview unavailable");
        }
        if !node.container {
            return if node.end - node.start <= 1024 {
                serde_json::from_slice(&Layout::read_bytes(
                    bytes,
                    node.start,
                    usize::try_from(node.end - node.start)
                        .map_err(|_| "native preview length unavailable")?,
                )?)
                .map_err(|_| "native preview unavailable")
            } else {
                Ok(Value::Null)
            };
        }
        let children = self.layout.indexed_children(connection, id, 0, 32)?;
        if node.object {
            let mut values = serde_json::Map::new();
            for (child, _) in children {
                if *budget == 0 {
                    break;
                }
                let child_node = self.layout.indexed_node(connection, child)?;
                if child_node.key_complete && child_node.key.is_some() {
                    let value = self.preview_node(connection, bytes, child, &child_node, budget)?;
                    values.insert(
                        child_node.key.ok_or("native preview key unavailable")?,
                        value,
                    );
                }
            }
            Ok(Value::Object(values))
        } else {
            let mut values = Vec::new();
            for (child, _) in children {
                if *budget == 0 {
                    break;
                }
                let child_node = self.layout.indexed_node(connection, child)?;
                values.push(self.preview_node(connection, bytes, child, &child_node, budget)?);
            }
            Ok(Value::Array(values))
        }
    }
    #[cfg(test)]
    fn visit_fields_legacy(
        &self,
        mut visit: impl FnMut(
            &[String],
            &str,
            Option<(&str, u64)>,
            u64,
            u64,
            u64,
            Option<(u64, u64)>,
            &Value,
        ) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let mut stack: Vec<(u64, Vec<String>, String)> = Vec::new();
        #[cfg(test)]
        FILE_WORK.with(|work| {
            let (writes, opens) = work.get();
            work.set((writes, opens + 1));
        });
        let bytes = std::fs::File::open(self.layout.directory.path().join("bytes"))
            .map_err(|_| "native byte source unavailable")?;
        let connection = self.layout.reader_connection()?;
        let mut rows = connection
            .prepare("SELECT id,parent,ordinal FROM nodes ORDER BY id")
            .map_err(|_| "native layout traversal unavailable")?;
        let mut rows = rows
            .query([])
            .map_err(|_| "native layout traversal unavailable")?;
        while let Some(row) = rows
            .next()
            .map_err(|_| "native layout traversal unavailable")?
        {
            let id: u64 = row
                .get(0)
                .map_err(|_| "native layout identity unavailable")?;
            let parent: Option<u64> = row.get(1).map_err(|_| "native layout parent unavailable")?;
            let ordinal: u64 = row
                .get(2)
                .map_err(|_| "native layout ordinal unavailable")?;
            while stack.last().map(|(id, _, _)| *id) != parent {
                stack.pop().ok_or("native layout parent ordering invalid")?;
            }
            let node = Layout::node_at(&connection, id)?;
            let mut path = stack
                .last()
                .map_or_else(Vec::new, |(_, path, _)| path.clone());
            if let Some(parent) = parent {
                let object = Layout::node_at(&connection, parent)?.object;
                path.push(if object {
                    node.key
                        .as_ref()
                        .filter(|_| node.key_complete)
                        .filter(|key| !key.starts_with('@') && key.as_str() != "$key")
                        .cloned()
                        .unwrap_or_else(|| format!("@{ordinal}"))
                } else {
                    ordinal.to_string()
                });
            }
            let path_json =
                serde_json::to_string(&path).map_err(|_| "native layout path unavailable")?;
            if path_json.len() > 3600 {
                return Err("native layout path exceeds bound");
            }
            let value = Self::preview_node_legacy(&connection, &bytes, id, &mut 32)?;
            visit(
                &path,
                &path_json,
                stack.last().map(|(_, _, path)| (path.as_str(), ordinal)),
                node.start,
                node.end,
                node.children,
                node.key_span,
                &value,
            )?;
            stack.push((id, path, path_json));
        }
        Ok(())
    }
    #[cfg(test)]
    fn preview_node_legacy(
        connection: &rusqlite::Connection,
        bytes: &std::fs::File,
        id: u64,
        budget: &mut usize,
    ) -> Result<Value, &'static str> {
        if *budget == 0 {
            return Ok(Value::Null);
        }
        *budget -= 1;
        let node = Layout::node_at(connection, id)?;
        if node.recursive_nodes <= 32 && node.end - node.start <= 16_384 {
            *budget = budget.saturating_sub(
                usize::try_from(node.recursive_nodes.saturating_sub(1))
                    .map_err(|_| "native preview count unavailable")?,
            );
            return serde_json::from_slice(&Layout::read_bytes(
                bytes,
                node.start,
                usize::try_from(node.end - node.start)
                    .map_err(|_| "native preview length unavailable")?,
            )?)
            .map_err(|_| "native preview unavailable");
        }
        if !node.container {
            return if node.end - node.start <= 1024 {
                serde_json::from_slice(&Layout::read_bytes(
                    bytes,
                    node.start,
                    usize::try_from(node.end - node.start)
                        .map_err(|_| "native preview length unavailable")?,
                )?)
                .map_err(|_| "native preview unavailable")
            } else {
                Ok(Value::Null)
            };
        }
        let children = Layout::children_at(connection, id, 0, 32)?;
        if node.object {
            let mut values = serde_json::Map::new();
            for (child, _) in children {
                if *budget == 0 {
                    break;
                }
                let key = Layout::node_at(connection, child)?;
                if let Some(key) = key.key.filter(|_| key.key_complete) {
                    values.insert(
                        key,
                        Self::preview_node_legacy(connection, bytes, child, budget)?,
                    );
                }
            }
            Ok(Value::Object(values))
        } else {
            let mut values = Vec::new();
            for (child, _) in children {
                if *budget == 0 {
                    break;
                }
                values.push(Self::preview_node_legacy(connection, bytes, child, budget)?);
            }
            Ok(Value::Array(values))
        }
    }
    fn node(&self, path: &[String]) -> Result<u64, ObservationReadError> {
        if path.len() > 256 {
            return Err(ObservationReadError::InvalidRequest(
                "invalid pending source path",
            ));
        }

        let layout = &self.layout;
        let mut index = 0;
        for part in path {
            let current = layout.node(index)?;
            let ordinal = part
                .strip_prefix('@')
                .map(str::parse::<u64>)
                .transpose()
                .map_err(|_| {
                    ObservationReadError::InvalidRequest("invalid pending source ordinal")
                })?
                .or_else(|| {
                    (!current.object)
                        .then(|| part.parse::<u64>().ok())
                        .flatten()
                });
            if !current.container && ordinal == Some(0) {
                continue;
            }
            index = layout.select(index, ordinal, part)?;
        }
        Ok(index)
    }
    /// Return a complete value only when both its bytes and member count fit.
    pub(crate) fn value(&self, path: &[String]) -> Result<Option<Value>, ObservationReadError> {
        let node = self.layout.node(self.node(path)?)?;
        let size = node.end - node.start;
        if size > ITEM_BYTES as u64 || node.recursive_nodes > 32 {
            return Ok(None);
        }
        let bytes = self.layout.bytes(
            node.start,
            usize::try_from(size).map_err(|_| "invalid source length")?,
        )?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| ObservationReadError::Unavailable("native value unavailable"))?;
        Ok(crate::runtime_client::checkpoint::bounded_json_value(
            &value, ITEM_BYTES,
        ))
    }
    pub(crate) fn page(
        &self,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<ObservationPage, ObservationReadError> {
        if limit == 0 || limit > 32 {
            return Err(ObservationReadError::InvalidRequest(
                "invalid pending page bound",
            ));
        }
        let layout = &self.layout;
        let index = self.node(path)?;
        let node = layout.node(index)?;
        let total = if node.container { node.children } else { 1 };
        if offset > total {
            return Err(ObservationReadError::InvalidRequest(
                "pending page exceeds frontier",
            ));
        }
        let end = offset.saturating_add(limit as u64).min(total);
        let members = if node.container {
            layout.children(index, offset, limit)?
        } else if offset == 0 {
            vec![(index, 0)]
        } else {
            Vec::new()
        };
        let fields = members
            .into_iter()
            .map(|(index, ordinal)| {
                let child = layout.node(index)?;
                let encoded_bytes = child.end - child.start;
                let value = if child.recursive_nodes <= 32 && encoded_bytes <= ITEM_BYTES as u64 {
                    let value = serde_json::from_slice(
                        &layout.bytes(
                            child.start,
                            usize::try_from(encoded_bytes)
                                .map_err(|_| "invalid pending source length")?,
                        )?,
                    )
                    .map_err(|_| {
                        ObservationReadError::Unavailable("native page value unavailable")
                    })?;
                    crate::runtime_client::checkpoint::bounded_json_value(&value, ITEM_BYTES)
                } else {
                    None
                };
                Ok(ObservationField {
                    ordinal,
                    key: child.key,
                    key_bytes: child.key_span.map(|(start, end)| end - start),
                    key_complete: child.key_complete,
                    encoded_bytes,
                    children: child.children,
                    value,
                })
            })
            .collect::<Result<Vec<_>, ObservationReadError>>()?;
        Ok(ObservationPage {
            total,
            fields,
            next_offset: (end < total).then_some(end),
        })
    }
    pub(crate) fn read(
        &self,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<ObservationChunk, ObservationReadError> {
        #[cfg(test)]
        READ_WORK.with(|work| work.set((0, 0)));
        if max_bytes == 0 || max_bytes > 65_536 || offset > SAFE {
            return Err(ObservationReadError::InvalidRequest(
                "invalid pending byte range",
            ));
        }
        let key = path.last().is_some_and(|part| part == "$key");
        let selected = if key { &path[..path.len() - 1] } else { path };
        let layout = &self.layout;
        let node = layout.node(self.node(selected)?)?;
        let (start, end) = if key {
            node.key_span.ok_or(ObservationReadError::InvalidRequest(
                "selected source has no object key",
            ))?
        } else {
            (node.start, node.end)
        };
        let readable_end = end - start;
        if offset > readable_end {
            return Err(ObservationReadError::InvalidRequest(
                "pending byte range exceeds frontier",
            ));
        }
        let count = usize::try_from((readable_end - offset).min(u64::from(max_bytes)))
            .map_err(|_| "native source read range exceeds platform capacity")?;
        let bytes = layout.bytes(
            start
                .checked_add(offset)
                .ok_or("pending byte range overflow")?,
            count,
        )?;
        let end = offset + bytes.len() as u64;
        Ok(ObservationChunk {
            bytes,
            readable_end,
            next_offset: (end < readable_end).then_some(end),
        })
    }
}

struct LayoutParser<'a> {
    reader: std::io::BufReader<std::fs::File>,
    position: u64,
    layout: &'a mut Layout,
}
impl LayoutParser<'_> {
    fn peek(&mut self) -> Result<Option<u8>, &'static str> {
        use std::io::BufRead as _;
        Ok(self
            .reader
            .fill_buf()
            .map_err(|_| "native source parser unavailable")?
            .first()
            .copied())
    }
    fn consume(&mut self, count: usize) {
        use std::io::BufRead as _;
        self.reader.consume(count);
        self.position += count as u64;
    }
    fn expect(&mut self, byte: u8) -> Result<(), &'static str> {
        if self.peek()? != Some(byte) {
            return Err("native source JSON invalid");
        }
        self.consume(1);
        Ok(())
    }
    fn string(&mut self) -> Result<(), &'static str> {
        use std::io::BufRead as _;
        self.expect(b'"')?;
        loop {
            if self.layout.expired() {
                return Err("native source indexing deadline reached");
            }
            let buffer = self
                .reader
                .fill_buf()
                .map_err(|_| "native source parser unavailable")?;
            if buffer.is_empty() {
                return Err("native source JSON truncated");
            }
            let count = buffer
                .iter()
                .position(|byte| matches!(byte, b'"' | b'\\'))
                .unwrap_or(buffer.len());
            self.consume(count);
            match self.peek()? {
                Some(b'"') => {
                    self.consume(1);
                    return Ok(());
                }
                Some(b'\\') => {
                    self.consume(1);
                    if self.peek()?.is_none() {
                        return Err("native source JSON truncated");
                    }
                    self.consume(1);
                }
                Some(_) => {}
                None => return Err("native source JSON truncated"),
            }
        }
    }
    fn child_key(&mut self) -> Result<(String, bool, (u64, u64)), &'static str> {
        let begin = self.position;
        self.string()?;
        let end = self.position;
        let name = if end - begin <= 256 {
            serde_json::from_slice::<String>(&Layout::read_bytes(
                self.reader.get_ref(),
                begin,
                usize::try_from(end - begin).map_err(|_| "native key exceeds platform capacity")?,
            )?)
            .ok()
        } else {
            None
        };
        let complete = name.is_some();
        self.expect(b':')?;
        Ok((name.unwrap_or_default(), complete, (begin, end)))
    }
    fn index_write(
        &mut self,
        sql: &'static str,
        parameters: impl rusqlite::Params,
    ) -> Result<(), &'static str> {
        self.layout
            .connection
            .get_mut()
            .map_err(|_| "native index poisoned")?
            .as_mut()
            .ok_or("source sealed")?
            .prepare_cached(sql)
            .map_err(|_| "native source index unavailable")?
            .execute(parameters)
            .map_err(|_| "native source index unavailable")?;
        #[cfg(test)]
        NODE_WRITES.with(|work| {
            let (inserts, updates) = work.get();
            work.set((
                inserts + usize::from(sql.starts_with("INSERT ")),
                updates + usize::from(sql.starts_with("UPDATE ")),
            ));
        });
        Ok(())
    }

    fn value(
        &mut self,
        parent: Option<u64>,
        ordinal: u64,
        key: Option<(String, bool, (u64, u64))>,
        depth: usize,
    ) -> Result<(u64, u64), &'static str> {
        if depth > 256 || self.layout.expired() {
            return Err("native source index limit reached");
        }
        if let Some(imported) = self.try_fragment(parent, ordinal, key.as_ref(), depth)? {
            return Ok(imported);
        }
        let id = self.layout.next_node;
        self.layout.next_node = self
            .layout
            .next_node
            .checked_add(1)
            .filter(|next| *next <= SAFE)
            .ok_or("native source nodes exhausted")?;
        let start = self.position;
        let byte = self.peek()?.ok_or("native source JSON truncated")?;
        let container = matches!(byte, b'{' | b'[');
        let object = byte == b'{';
        let (name, complete, span) = key.map_or((None, true, None), |(name, complete, span)| {
            (Some(name), complete, Some(span))
        });
        let (mut children, mut recursive) = (0_u64, 1_u64);
        if container {
            self.consume(1);
            let close = if object { b'}' } else { b']' };
            while self.peek()? != Some(close) {
                if children > 0 {
                    self.expect(b',')?;
                }
                let child_key = if object {
                    Some(self.child_key()?)
                } else {
                    None
                };
                let (_, count) = self.value(Some(id), children, child_key, depth + 1)?;
                recursive = (recursive + count).min(33);
                children += 1;
            }
            self.expect(close)?;
        } else if byte == b'"' {
            self.string()?;
        } else {
            while self
                .peek()?
                .is_some_and(|byte| !matches!(byte, b',' | b'}' | b']'))
            {
                self.consume(1);
            }
        }
        let end = self.position;
        // IDs remain preorder; completed rows need no placeholder update.
        self.index_write(
            "INSERT INTO nodes VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            rusqlite::params![
                id,
                parent,
                ordinal,
                start,
                end,
                children,
                recursive,
                container,
                object,
                name,
                span.map(|span| span.0),
                span.map(|span| span.1),
                complete,
            ],
        )?;
        Ok((id, recursive))
    }
}

#[derive(Debug)]
pub(crate) struct ObservationField {
    pub ordinal: u64,
    pub key: Option<String>,
    pub key_bytes: Option<u64>,
    pub key_complete: bool,
    pub encoded_bytes: u64,
    pub children: u64,
    pub value: Option<Value>,
}
#[derive(Debug)]
pub(crate) struct ObservationPage {
    pub total: u64,
    pub fields: Vec<ObservationField>,
    pub next_offset: Option<u64>,
}
#[derive(Debug)]
pub(crate) struct ObservationChunk {
    pub bytes: Vec<u8>,
    pub readable_end: u64,
    pub next_offset: Option<u64>,
}

/// An original native UTF-8 scalar needs no structural encoding or read index.
pub(crate) fn text_chunk(
    text: &str,
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationChunk, ObservationReadError> {
    if max_bytes == 0 || max_bytes > 65536 || offset > SAFE {
        return Err(ObservationReadError::InvalidRequest(
            "invalid native text range",
        ));
    }
    let readable_end = u64::try_from(text.len()).map_err(|_| "native text length exhausted")?;
    if readable_end > SAFE || offset > readable_end {
        return Err(ObservationReadError::InvalidRequest(
            "native text range exceeds frontier",
        ));
    }
    let start = usize::try_from(offset).map_err(|_| "native text offset unavailable")?;
    let end = start.saturating_add(max_bytes as usize).min(text.len());
    let bytes = text.as_bytes()[start..end].to_vec();
    #[cfg(test)]
    READ_WORK.with(|work| work.set((usize::from(!bytes.is_empty()), bytes.len())));
    Ok(ObservationChunk {
        bytes,
        readable_end,
        next_offset: (end < text.len()).then_some(end as u64),
    })
}

#[cfg(test)]
mod path_tests;

#[cfg(test)]
mod source_copy_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(serde::Serialize, schemars::JsonSchema)]
    struct NativeSource<'a> {
        title: &'a str,
        revision: u64,
        opaque: &'a Value,
    }
    #[derive(serde::Serialize, schemars::JsonSchema)]
    struct RequestProducerFixture<'a> {
        messages: Vec<NativeSource<'a>>,
    }
    #[test]
    fn t08_t15_t20_layout_traversal_reuses_rows_without_changing_original_field_evidence() {
        #[derive(Debug, PartialEq)]
        struct Field {
            path: Vec<String>,
            encoded_path: String,
            parent: Option<(String, u64)>,
            start: u64,
            end: u64,
            children: u64,
            key_span: Option<(u64, u64)>,
            preview: Value,
        }
        let opaque = serde_json::json!({
            "content": {"untrusted": true}, "collections": ["original", u64::MAX],
            "@0": {"$key": "原\\\"\n🙂", "array": [[null, true, -0.0], {"slash/~": [1, 2]}]},
            "k".repeat(300): "key is deferred", "huge": "水🙂".repeat(4000),
            "wide": (0..80).map(|index| serde_json::json!({"index": index, "value": "x"})).collect::<Vec<_>>()
        });
        let value = RequestProducerFixture {
            messages: (0..40)
                .map(|_| NativeSource {
                    title: "\u{feff}水🙂\\\"\n",
                    revision: u64::MAX,
                    opaque: &opaque,
                })
                .collect(),
        };
        let source = SeekableObservationSource::new(&value).unwrap();
        let mut actual = Vec::new();
        NODE_LOOKUPS.with(|count| count.set(0));
        source
            .visit_fields(
                |path, encoded_path, parent, start, end, children, key_span, preview| {
                    actual.push(Field {
                        path: path.to_vec(),
                        encoded_path: encoded_path.into(),
                        parent: parent.map(|(path, ordinal)| (path.into(), ordinal)),
                        start,
                        end,
                        children,
                        key_span,
                        preview: preview.clone(),
                    });
                    Ok(())
                },
            )
            .unwrap();
        let actual_queries = NODE_LOOKUPS.with(std::cell::Cell::get);
        let mut expected = Vec::new();
        NODE_LOOKUPS.with(|count| count.set(0));
        source
            .visit_fields_legacy(
                |path, encoded_path, parent, start, end, children, key_span, preview| {
                    expected.push(Field {
                        path: path.to_vec(),
                        encoded_path: encoded_path.into(),
                        parent: parent.map(|(path, ordinal)| (path.into(), ordinal)),
                        start,
                        end,
                        children,
                        key_span,
                        preview: preview.clone(),
                    });
                    Ok(())
                },
            )
            .unwrap();
        let legacy_queries = NODE_LOOKUPS.with(std::cell::Cell::get);
        assert_eq!(actual, expected);
        assert!(actual.len() > 10_000);
        assert!(legacy_queries >= actual_queries + 3 * actual.len() - 1);
        assert!(
            actual
                .iter()
                .any(|field| field.path.last().is_some_and(|key| key.starts_with('@')))
        );
        assert!(actual.iter().any(|field| field.children > 32));
        assert!(
            actual
                .iter()
                .any(|field| field.preview == Value::Null && field.end - field.start > 16_384)
        );
    }

    fn original_layout_bytes(source: &SeekableObservationSource) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut offset = 0;
        loop {
            let chunk = source.read(&[], offset, 65_536).unwrap();
            bytes.extend_from_slice(&chunk.bytes);
            match chunk.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => return bytes,
            }
        }
    }

    #[test]
    fn t08_t15_t20_captured_field_preview_selection_preserves_original_traversal_and_bytes() {
        let opaque = serde_json::json!({
            "escaped/~": ["水🙂\\\"\n", {"@0": u64::MAX}],
            "large": "水🙂".repeat(4000),
            "wide": (0..80).map(|i| serde_json::json!({"original":i})).collect::<Vec<_>>()
        });
        let value = RequestProducerFixture {
            messages: (0..3)
                .map(|_| NativeSource {
                    title: "original message",
                    revision: u64::MAX,
                    opaque: &opaque,
                })
                .collect(),
        };
        let source = SeekableObservationSource::new(&value).unwrap();
        let mut expected_bytes = Vec::new();
        crate::runtime_client::structural_json::to_writer(&mut expected_bytes, &value).unwrap();
        assert_eq!(original_layout_bytes(&source), expected_bytes);
        let is_captured = |path: &[String]| {
            path.len() == 2 && path[0] == "messages" && path[1].parse::<u64>().is_ok()
        };
        let mut expected = Vec::new();
        source
            .visit_fields_legacy(
                |path, encoded, parent, start, end, children, key, preview| {
                    expected.push((
                        path.to_vec(),
                        encoded.to_owned(),
                        parent.map(|(path, ordinal)| (path.to_owned(), ordinal)),
                        start,
                        end,
                        children,
                        key,
                        (!is_captured(path)).then(|| preview.clone()),
                    ));
                    Ok(())
                },
            )
            .unwrap();
        let mut actual = Vec::new();
        take_field_preview_work();
        source
            .visit_fields_selecting_previews(
                |path| {
                    if is_captured(path) {
                        FieldPreviewPolicy::Skip
                    } else {
                        FieldPreviewPolicy::Inline
                    }
                },
                |path, encoded, parent, start, end, children, key, preview| {
                    let preview = match preview {
                        FieldPreview::Inline(value) => Some(value.clone()),
                        FieldPreview::Skipped => None,
                        FieldPreview::OriginalSpan { .. } => {
                            panic!("inline policy cannot select a span")
                        }
                    };
                    actual.push((
                        path.to_vec(),
                        encoded.to_owned(),
                        parent.map(|(path, ordinal)| (path.to_owned(), ordinal)),
                        start,
                        end,
                        children,
                        key,
                        preview,
                    ));
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            take_field_preview_work(),
            (actual.len() - value.messages.len(), value.messages.len())
        );
        assert!(actual.len() > 500);
        assert!(
            actual
                .iter()
                .filter(|field| is_captured(&field.0))
                .all(|field| field.7.is_none())
        );
        assert!(
            actual
                .iter()
                .filter(|field| !is_captured(&field.0))
                .all(|field| field.7.is_some())
        );
        assert_eq!(original_layout_bytes(&source), expected_bytes);
    }

    fn original_span_policy_fixture() -> (SeekableObservationSource, Vec<u8>) {
        let opaque = serde_json::json!({
            "escaped/~": ["水🙂\\\"\n", {"@0": u64::MAX}],
            "float": -0.0, "raw": " {\"n\":1e0,\"escaped\":\"\\u6c34\"} ",
            "exact": "x".repeat(ITEM_BYTES - 2),
            "over": "x".repeat(ITEM_BYTES - 1),
            "thirty_two_nodes": (0..31).collect::<Vec<u32>>(),
            "thirty_three_nodes": (0..32).collect::<Vec<u32>>(),
            "k".repeat(300): "original deferred key"
        });
        let value = RequestProducerFixture {
            messages: (0..3)
                .map(|_| NativeSource {
                    title: "original",
                    revision: u64::MAX,
                    opaque: &opaque,
                })
                .collect(),
        };
        let source = SeekableObservationSource::new(&value).unwrap();
        let mut original = Vec::new();
        crate::runtime_client::structural_json::to_writer(&mut original, &value).unwrap();
        assert_eq!(original_layout_bytes(&source), original);
        (source, original)
    }

    #[test]
    fn t08_t15_t19_original_span_policy_preserves_every_legacy_field_and_actual_bound() {
        fn recursive(value: &Value) -> u64 {
            1 + match value {
                Value::Array(values) => values.iter().map(recursive).sum(),
                Value::Object(values) => values.values().map(recursive).sum(),
                _ => 0,
            }
        }
        let (source, original) = original_span_policy_fixture();
        let mut expected = Vec::new();
        source
            .visit_fields_legacy(
                |path, encoded, parent, start, end, children, key, preview| {
                    expected.push((
                        path.to_vec(),
                        encoded.to_owned(),
                        parent.map(|(path, ordinal)| (path.to_owned(), ordinal)),
                        start,
                        end,
                        children,
                        key,
                        preview.clone(),
                    ));
                    Ok(())
                },
            )
            .unwrap();
        let mut actual = Vec::new();
        let mut spans = Vec::new();
        take_field_preview_work();
        source
            .visit_fields_selecting_previews(
                |_| FieldPreviewPolicy::OriginalSpanIfSmall,
                |path, encoded, parent, start, end, children, key, preview| {
                    let preview = match preview {
                        FieldPreview::OriginalSpan { recursive_nodes } => {
                            assert!((1..=32).contains(&recursive_nodes));
                            assert!((1..=16_384).contains(&(end - start)));
                            let bytes = source.indexed_span(start, end, 16_384).unwrap();
                            assert_eq!(
                                bytes,
                                original[usize::try_from(start).unwrap()
                                    ..usize::try_from(end).unwrap()]
                            );
                            let value: Value = serde_json::from_slice(&bytes).unwrap();
                            assert_eq!(recursive_nodes, recursive(&value));
                            spans.push(path.to_vec());
                            value
                        }
                        FieldPreview::Inline(value) => value.clone(),
                        FieldPreview::Skipped => panic!("no captured headers were skipped"),
                    };
                    actual.push((
                        path.to_vec(),
                        encoded.to_owned(),
                        parent.map(|(path, ordinal)| (path.to_owned(), ordinal)),
                        start,
                        end,
                        children,
                        key,
                        preview,
                    ));
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(actual, expected);
        assert!(spans.len() > actual.len() / 2);
        assert_eq!(
            take_field_preview_work(),
            (actual.len() - spans.len(), spans.len())
        );
        for key in ["exact", "thirty_two_nodes"] {
            assert!(
                spans
                    .iter()
                    .any(|path| path.last().is_some_and(|last| last == key))
            );
        }
        for key in ["over", "thirty_three_nodes"] {
            assert!(
                !spans
                    .iter()
                    .any(|path| path.last().is_some_and(|last| last == key))
            );
        }
        assert_eq!(original_layout_bytes(&source), original);
    }

    #[test]
    fn t08_t15_t20_layout_inserts_complete_nodes_once_before_sealing_required_indexes() {
        fn recursive(value: &Value) -> u64 {
            let children = match value {
                Value::Array(values) => values.iter().map(recursive).sum::<u64>(),
                Value::Object(values) => values.values().map(recursive).sum::<u64>(),
                _ => 0,
            };
            (1 + children).min(33)
        }
        let opaque = serde_json::json!({
            "escaped/~": ["水🙂\\\"\n", {"@0": u64::MAX}],
            "large": "水🙂".repeat(4000),
            "wide": (0..80).map(|i| serde_json::json!({"value":i})).collect::<Vec<_>>()
        });
        let value = RequestProducerFixture {
            messages: (0..3)
                .map(|_| NativeSource {
                    title: "original",
                    revision: u64::MAX,
                    opaque: &opaque,
                })
                .collect(),
        };
        NODE_WRITES.with(|work| work.set((0, 0)));
        let source = SeekableObservationSource::new(&value).unwrap();
        let writes = NODE_WRITES.with(std::cell::Cell::get);
        assert_eq!(
            writes,
            (usize::try_from(source.layout.next_node).unwrap(), 0)
        );
        assert!(writes.0 > 500);
        let mut original = Vec::new();
        crate::runtime_client::structural_json::to_writer(&mut original, &value).unwrap();
        assert_eq!(original_layout_bytes(&source), original);
        let connection = source.layout.reader_connection().unwrap();
        let mut statement = connection
            .prepare("SELECT id,start,end,children,recursive_nodes FROM nodes ORDER BY id")
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, usize>(1)?,
                    row.get::<_, usize>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                ))
            })
            .unwrap();
        for (expected_id, row) in rows.enumerate() {
            let (id, start, end, children, nodes) = row.unwrap();
            assert_eq!(id, u64::try_from(expected_id).unwrap());
            let original: Value = serde_json::from_slice(&original[start..end]).unwrap();
            let count = match &original {
                Value::Array(values) => values.len(),
                Value::Object(values) => values.len(),
                _ => 0,
            };
            assert_eq!(children, u64::try_from(count).unwrap());
            assert_eq!(nodes, recursive(&original));
        }
        let indexes: u64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name IN ('nodes_ordinal','nodes_key')",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(indexes, 2);
        let page = source.page(&["messages".into()], 1, 1).unwrap();
        assert_eq!(
            (page.total, page.fields[0].ordinal, page.next_offset),
            (3, 1, Some(2))
        );
        assert_eq!(
            source
                .read(&["messages".into(), "1".into(), "revision".into()], 0, 64)
                .unwrap()
                .bytes,
            serde_json::to_vec(&u64::MAX.to_string()).unwrap()
        );
    }

    #[test]
    fn t10_t19_layout_cannot_escape_before_required_ordinal_uniqueness_is_validated() {
        let mut layout = Layout::new().unwrap();
        layout
            .connection
            .get_mut()
            .unwrap()
            .as_ref()
            .unwrap()
            .execute_batch(
                "INSERT INTO nodes VALUES(0,NULL,0,0,2,2,3,1,0,NULL,NULL,NULL,1);
             INSERT INTO nodes VALUES(1,0,0,0,1,0,1,0,0,NULL,NULL,NULL,1);
             INSERT INTO nodes VALUES(2,0,0,1,2,0,1,0,0,NULL,NULL,NULL,1);",
            )
            .unwrap();
        assert!(layout.seal().is_err());
        assert!(layout.file.is_some());
        assert!(layout.connection.get_mut().unwrap().is_some());
    }

    #[test]
    fn t08_t15_t20_layout_construction_buffers_tokens_and_scopes_preview_reader() {
        let opaque =
            serde_json::json!({"wide":u64::MAX,"raw":" {\"n\":1.0} ","text":"\u{feff}🙂水"});
        let value = RequestProducerFixture {
            messages: (0..300)
                .map(|_| NativeSource {
                    title: "\u{feff}🙂水\"\\\n",
                    revision: u64::MAX,
                    opaque: &opaque,
                })
                .collect(),
        };
        FILE_WORK.with(|work| work.set((0, 0)));
        let source = SeekableObservationSource::new(&value).unwrap();
        let (writes, _) = FILE_WORK.with(std::cell::Cell::get);
        assert!(writes <= usize::try_from(source.layout.written.div_ceil(65_536)).unwrap());
        FILE_WORK.with(|work| work.set((0, 0)));
        let mut messages = 0;
        source
            .visit_fields(|path, _, _, start, end, _, _, preview| {
                if path.len() == 2 && path[0] == "messages" {
                    messages += 1;
                    assert_eq!(
                        *preview,
                        serde_json::from_slice::<Value>(
                            &source.indexed_span(start, end, 16_384).unwrap()
                        )
                        .unwrap()
                    );
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(messages, 300);
        assert_eq!(FILE_WORK.with(std::cell::Cell::get), (0, 301));
        eprintln!(
            "buffered source bytes={} file_writes={writes}; traversal_reader_opens=1 plus300 explicit span reads",
            source.layout.written
        );
    }

    #[test]
    fn t08_t15_t20_trusted_message_spans_avoid_index_reopens_and_preserve_exact_bytes() {
        let opaque =
            serde_json::json!({"wide":u64::MAX,"raw":" {\"n\":1.0,\"escaped\":\"\\u6c34\"} \n"});
        let value = RequestProducerFixture {
            messages: (0..300)
                .map(|_| NativeSource {
                    title: "\u{feff}🙂水\"\\\n",
                    revision: u64::MAX,
                    opaque: &opaque,
                })
                .collect(),
        };
        let source = SeekableObservationSource::new(&value).unwrap();
        let mut spans = Vec::new();
        source
            .visit_fields(|path, _, _, start, end, _, _, _| {
                if path.len() == 2 && path[0] == "messages" {
                    spans.push((path.to_vec(), start, end));
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(spans.len(), 300);
        INDEX_OPENS.with(|opens| opens.set(0));
        let before = std::time::Instant::now();
        let named = spans
            .iter()
            .map(|(path, _, _)| source.read(path, 0, 16_384).unwrap().bytes)
            .collect::<Vec<_>>();
        let named_time = before.elapsed();
        let named_opens = INDEX_OPENS.with(std::cell::Cell::get);
        assert_eq!(named_opens, 1500);
        INDEX_OPENS.with(|opens| opens.set(0));
        let after = std::time::Instant::now();
        for ((_, start, end), expected) in spans.iter().zip(&named) {
            READ_WORK.with(|work| work.set((0, 0)));
            let bytes = source.indexed_span(*start, *end, 16_384).unwrap();
            assert_eq!(&bytes, expected);
            assert_eq!(READ_WORK.with(std::cell::Cell::get), (1, bytes.len()));
            let decoded: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(decoded["revision"], u64::MAX.to_string());
            assert_eq!(decoded["opaque"], opaque);
            assert_eq!(decoded["title"], value.messages[0].title);
        }
        let span_time = after.elapsed();
        assert_eq!(INDEX_OPENS.with(std::cell::Cell::get), 0);
        assert!(matches!(
            source.indexed_span(0, source.layout.written, 16_384),
            Err(ObservationReadError::InvalidRequest(_))
        ));
        assert!(matches!(
            source.indexed_span(2, 1, 16_384),
            Err(ObservationReadError::InvalidRequest(_))
        ));
        eprintln!(
            "300 request message previews: named={named_time:?}, indexed_span={span_time:?}, index_opens={named_opens}->0"
        );
    }

    #[test]
    fn t08_t12_source_read_errors_distinguish_invalid_selection_from_layout_failure() {
        let opaque = serde_json::json!({"wide": u64::MAX});
        let native = NativeSource {
            title: "original",
            revision: u64::MAX,
            opaque: &opaque,
        };
        let source = SeekableObservationSource::new(&native).unwrap();
        for error in [
            source.page(&["absent".into()], 0, 32).unwrap_err(),
            source.read(&["title".into()], 100, 1).unwrap_err(),
            source.read(&["@bad".into()], 0, 1).unwrap_err(),
            source.page(&[], 0, 33).unwrap_err(),
        ] {
            assert!(
                matches!(error, ObservationReadError::InvalidRequest(_)),
                "{error:?}"
            );
        }
        assert!(
            source
                .value(&["opaque".into(), "wide".into()])
                .unwrap()
                .is_none()
        );
        let exact = source
            .read(&["opaque".into(), "wide".into()], 0, 65_536)
            .unwrap();
        assert_eq!(exact.bytes, u64::MAX.to_string().as_bytes());
        assert_eq!(
            source.value(&["revision".into()]).unwrap(),
            Some(Value::String(u64::MAX.to_string()))
        );
        let connection =
            rusqlite::Connection::open(source.layout.directory.path().join("index.sqlite"))
                .unwrap();
        connection.execute("DROP TABLE nodes", []).unwrap();
        assert!(matches!(
            source.read(&["title".into()], 0, 1).unwrap_err(),
            ObservationReadError::Unavailable(_)
        ));
    }

    #[test]
    fn t08_t19_t20_native_text_ranges_need_no_index_or_utf8_boundary_alignment() {
        let text = "α🙂".repeat(3_000_000);
        let end = text.len() as u64;
        let chunk = text_chunk(&text, 1, 65536).unwrap();
        assert_eq!(chunk.bytes, &text.as_bytes()[1..65537]);
        assert_eq!(chunk.readable_end, end);
        assert_eq!(chunk.next_offset, Some(65537));
        assert_eq!(READ_WORK.with(std::cell::Cell::get), (1, 65536));
        let tail = text_chunk(&text, end - 1, 65536).unwrap();
        assert_eq!(tail.bytes, &text.as_bytes()[text.len() - 1..]);
        assert!(tail.next_offset.is_none());
        assert!(text_chunk(&text, end + 1, 65536).is_err());
        assert!(text_chunk(&text, 0, 65537).is_err());
    }
    #[test]
    fn t08_t12_t15_t20_native_collection_source_indexes_once_and_seeks_each_chunk() {
        let title = "α🙂".repeat(3_000_000);
        let opaque = serde_json::json!({"many":(0..73).collect::<Vec<_>>(),"integer":9_007_199_254_740_992_u64});
        let native = NativeSource {
            title: &title,
            revision: 9_007_199_254_740_992,
            opaque: &opaque,
        };
        let cache: ObservationLayoutCache = Arc::default();
        let source = cache
            .get_or_init(|| SeekableObservationSource::new(&native))
            .as_ref()
            .unwrap();
        assert!(source.layout.file.is_none());
        assert!(source.layout.connection.lock().unwrap().is_none());
        let page = source.page(&[], 0, 32).unwrap();
        assert_eq!(page.total, 3);
        assert!(page.fields[0].value.is_none());
        assert_eq!(
            page.fields[1].value,
            Some(Value::String("9007199254740992".into()))
        );
        let last = source
            .page(&["opaque".into(), "many".into()], 64, 32)
            .unwrap();
        assert_eq!(last.total, 73);
        assert_eq!(last.fields.len(), 9);
        assert!(last.next_offset.is_none());
        let expected = serde_json::to_vec(&title).unwrap();
        let (mut offset, mut actual) = (0, Vec::new());
        loop {
            let selected = cache
                .get_or_init(|| panic!("native immutable source must index only once"))
                .as_ref()
                .unwrap();
            assert!(std::ptr::eq(source, selected));
            let chunk = selected.read(&["title".into()], offset, 65536).unwrap();
            assert_eq!(
                READ_WORK.with(std::cell::Cell::get),
                (usize::from(!chunk.bytes.is_empty()), chunk.bytes.len())
            );
            assert!(chunk.bytes.len() <= 65536);
            actual.extend(chunk.bytes);
            let Some(next) = chunk.next_offset else { break };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(actual, expected);
        let raw = source
            .read(&["opaque".into(), "integer".into()], 0, 65536)
            .unwrap();
        assert_eq!(raw.bytes, b"9007199254740992");
        assert!(
            source
                .read(&["title".into()], expected.len() as u64 + 1, 65536)
                .is_err()
        );
        assert!(source.layout.file.is_none());
        assert!(source.layout.connection.lock().unwrap().is_none());
    }
}
