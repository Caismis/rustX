//! Seekable immutable observation bytes derived from existing native authority.
mod byte_carrier;
#[cfg(test)]
mod byte_reader_tests;
pub(crate) mod canonical;
mod key_order;
#[cfg(test)]
mod resident_byte_tests;
mod small;
use rusqlite::OptionalExtension as _;
use serde_json::Value;
use std::sync::Arc;
const ITEM_BYTES: usize = 16_384;
/// At most 16 KiB of original JSON can contribute compact nodes/keys to this index.
const MEMORY_INDEX_BYTES: usize = 16_384;
const SAFE: u64 = 9_007_199_254_740_991;
#[cfg(test)]
thread_local! { pub(crate) static READ_WORK: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) }; }
#[cfg(test)]
thread_local! {pub(crate) static INDEX_OPENS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[cfg(test)]
thread_local! {static NODE_LOOKUPS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
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
    Skip,
}

/// A preview is computed only when the current bounded consumer requests it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FieldPreview<'a> {
    Inline(&'a Value),
    Skipped,
}

/// Facts from the exact-byte indexing pass, borrowed during ordered traversal.
/// These describe the original representation, not the normalized preview.
pub(crate) struct FieldMetadata<'a> {
    pub path: &'a [String],
    pub encoded_path: &'a str,
    pub parent: Option<(&'a [String], &'a str, u64)>,
    pub start: u64,
    pub end: u64,
    pub children: u64,
    pub recursive_nodes: u64,
    pub object: bool,
    pub container: bool,
    pub key_span: Option<(u64, u64)>,
    pub key: Option<&'a str>,
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
}
#[cfg(test)]
pub(crate) fn take_file_work() -> (usize, usize) {
    FILE_WORK.with(|work| work.replace((0, 0)))
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
    directory: Option<tempfile::TempDir>,
    bytes: byte_carrier::Bytes,
    connection: std::sync::Mutex<Option<rusqlite::Connection>>,
    written: u64,
    small_index: Option<small::SmallIndex>,
    next_node: u64,
    deadline: Option<std::time::Instant>,
}

enum LayoutConnection<'a> {
    Borrowed(std::sync::MutexGuard<'a, Option<rusqlite::Connection>>),
    Reading(rusqlite::Connection),
}
impl std::ops::Deref for LayoutConnection<'_> {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(guard) => guard.as_ref().expect("native source owns index"),
            Self::Reading(connection) => connection,
        }
    }
}

enum IndexReader<'a> {
    Small(&'a small::SmallIndex),
    Sqlite(LayoutConnection<'a>),
}
impl IndexReader<'_> {
    fn node(&self, id: u64) -> Result<Node, &'static str> {
        match self {
            Self::Small(index) => {
                #[cfg(test)]
                NODE_LOOKUPS.with(|count| count.set(count.get() + 1));
                index.node(id)
            }
            Self::Sqlite(connection) => Layout::node_at(connection, id),
        }
    }
    fn select(
        &self,
        parent: u64,
        ordinal: Option<u64>,
        key: &str,
    ) -> Result<u64, ObservationReadError> {
        match self {
            Self::Small(index) => index.select(parent, ordinal, key),
            Self::Sqlite(connection) => Layout::indexed_select(connection, parent, ordinal, key),
        }
    }
    fn children(
        &self,
        parent: u64,
        offset: u64,
        limit: usize,
    ) -> Result<Vec<(u64, u64)>, &'static str> {
        match self {
            Self::Small(index) => Ok(index.children(parent, offset, limit)),
            Self::Sqlite(connection) => Layout::children_at(connection, parent, offset, limit),
        }
    }
    fn visit(
        &self,
        first: u64,
        last: u64,
        include: impl FnMut(u64) -> bool,
        visit: impl FnMut(NodeRecord) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        match self {
            Self::Small(index) => index.visit(first, last, include, visit),
            Self::Sqlite(connection) => {
                Layout::visit_selected_records(connection, first, last, include, visit)
            }
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
        let count = self.bytes.write(bytes, &mut self.directory)?;
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
        if let Some(index) = &mut self.small_index {
            index.seal(self.next_node, self.written)?;
            self.bytes.seal();
            return Ok(());
        }
        self.connection
            .get_mut()
            .map_err(|_| "native source index poisoned")?
            .as_mut()
            .ok_or("native source already sealed")?
            .execute_batch("CREATE UNIQUE INDEX nodes_ordinal ON nodes(parent,ordinal); CREATE INDEX nodes_key ON nodes(parent,key) WHERE key_complete=1; COMMIT")
            .map_err(|_| "native source index commit unavailable")?;
        self.bytes.seal();
        *self
            .connection
            .get_mut()
            .map_err(|_| "native source index poisoned")? = None;
        Ok(())
    }
    fn index_reader(&self) -> Result<IndexReader<'_>, &'static str> {
        match &self.small_index {
            Some(index) => Ok(IndexReader::Small(index)),
            None => self.reader_connection().map(IndexReader::Sqlite),
        }
    }
    fn reader_connection(&self) -> Result<LayoutConnection<'_>, &'static str> {
        if self.small_index.is_some() {
            return Err("native small index has no SQLite connection");
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| "native source index poisoned")?;
        if guard.is_some() {
            return Ok(LayoutConnection::Borrowed(guard));
        }
        drop(guard);
        #[cfg(test)]
        INDEX_OPENS.with(|count| count.set(count.get() + 1));
        let connection = rusqlite::Connection::open_with_flags(
            self.directory
                .as_ref()
                .ok_or("native source directory unavailable")?
                .path()
                .join("index.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|_| "native source index unavailable")?;
        connection
            .execute_batch("PRAGMA cache_size=-1024")
            .map_err(|_| "native source cache unavailable")?;
        Ok(LayoutConnection::Reading(connection))
    }

    fn new() -> Result<Self, &'static str> {
        let mut layout = Self::unindexed();
        layout.initialize_index(false)?;
        Ok(layout)
    }
    fn unindexed() -> Self {
        Self {
            directory: None,
            bytes: byte_carrier::Bytes::default(),
            connection: std::sync::Mutex::new(None),
            written: 0,
            small_index: None,
            next_node: 0,
            deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(5)),
        }
    }
    fn initialize_index(&mut self, memory: bool) -> Result<(), &'static str> {
        if memory {
            // Admission uses the completed original encoding, never an estimate
            // or a second serialization. JSON size bounds node and key storage.
            if self.written > MEMORY_INDEX_BYTES as u64 {
                return Err("native memory index exceeds byte bound");
            }
            self.small_index = Some(small::SmallIndex::default());
            return Ok(());
        }
        let directory = byte_carrier::directory(&mut self.directory)
            .map_err(|_| "pending source directory unavailable")?;
        let connection = rusqlite::Connection::open(directory.path().join("index.sqlite"))
            .map_err(|_| "pending source index unavailable")?;
        // Derived observation only: this is not the durable execution journal.
        connection.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-1024; CREATE TABLE nodes(id INTEGER PRIMARY KEY,parent INTEGER,ordinal INTEGER NOT NULL,start INTEGER NOT NULL,end INTEGER NOT NULL,children INTEGER NOT NULL,recursive_nodes INTEGER NOT NULL,container INTEGER NOT NULL,object INTEGER NOT NULL,key TEXT,key_start INTEGER,key_end INTEGER,key_complete INTEGER NOT NULL); BEGIN;").map_err(|_|"pending source index unavailable")?;
        *self
            .connection
            .get_mut()
            .map_err(|_| "native source index poisoned")? = Some(connection);
        Ok(())
    }
    fn node(&self, id: u64) -> Result<Node, &'static str> {
        self.index_reader()?.node(id)
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
        self.index_reader()?.select(parent, ordinal, key)
    }
    fn indexed_select(
        connection: &rusqlite::Connection,
        parent: u64,
        ordinal: Option<u64>,
        key: &str,
    ) -> Result<u64, ObservationReadError> {
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
    fn children(
        &self,
        parent: u64,
        offset: u64,
        limit: usize,
    ) -> Result<Vec<(u64, u64)>, &'static str> {
        self.index_reader()?.children(parent, offset, limit)
    }
    fn children_at(
        connection: &rusqlite::Connection,
        parent: u64,
        offset: u64,
        limit: usize,
    ) -> Result<Vec<(u64, u64)>, &'static str> {
        let mut statement=connection.prepare_cached("SELECT id,ordinal FROM nodes WHERE parent=?1 AND ordinal>=?2 ORDER BY ordinal LIMIT ?3").map_err(|_|"pending source page unavailable")?;
        statement
            .query_map(rusqlite::params![parent, offset, limit], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|_| "pending source page unavailable")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "pending source page unavailable")
    }
    fn open_bytes(&self) -> Result<byte_carrier::Reader, &'static str> {
        self.bytes
            .reader(self.directory.as_ref())
            .map_err(|_| "native byte source unavailable")
    }
    fn bytes(&self, start: u64, count: usize) -> Result<Vec<u8>, &'static str> {
        Self::read_bytes(&self.open_bytes()?, start, count)
    }
    fn read_bytes(
        file: &byte_carrier::Reader,
        start: u64,
        count: usize,
    ) -> Result<Vec<u8>, &'static str> {
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
/// One bounded byte carrier before navigation is requested. A durable producer
/// may reuse an authenticated immutable index by exact encoded-byte identity;
/// a miss consumes this carrier directly, without serializing the value twice.
pub(crate) struct EncodedObservationSource {
    layout: Layout,
    digest: sha2::Sha256,
}
impl std::io::Write for EncodedObservationSource {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        use sha2::Digest as _;
        let count = self.layout.write(bytes)?;
        self.digest.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl EncodedObservationSource {
    pub(crate) fn new<T: serde::Serialize + schemars::JsonSchema>(
        value: &T,
    ) -> Result<Self, &'static str> {
        use sha2::Digest as _;
        use std::io::Write as _;
        let mut encoded = Self {
            layout: Layout::unindexed(),
            digest: sha2::Sha256::new(),
        };
        {
            let mut writer = std::io::BufWriter::with_capacity(65_536, &mut encoded);
            crate::runtime_client::structural_json::to_writer(&mut writer, value)
                .map_err(|_| "native observation source encoding unavailable")?;
            writer
                .flush()
                .map_err(|_| "native observation source flush unavailable")?;
        }
        encoded.layout.bytes.seal();
        Ok(encoded)
    }
    pub(crate) fn digest(&self) -> [u8; 32] {
        use sha2::Digest as _;
        self.digest.clone().finalize().into()
    }
    pub(crate) fn length(&self) -> u64 {
        self.layout.written
    }
    pub(crate) fn bounded_bytes(&self, start: u64, end: u64) -> Result<Vec<u8>, &'static str> {
        let count = end
            .checked_sub(start)
            .filter(|count| *count <= ITEM_BYTES as u64 && end <= self.layout.written)
            .ok_or("encoded observation range exceeds bound")?;
        Layout::read_bytes(
            &self.layout.open_bytes()?,
            start,
            usize::try_from(count)
                .map_err(|_| "encoded observation range exceeds platform capacity")?,
        )
    }
    pub(crate) fn index(self) -> Result<SeekableObservationSource, &'static str> {
        SeekableObservationSource::from_layout(self.layout)
    }
}
/// One synchronous byte reader borrowed from its immutable source. The borrow
/// pins the resident bytes or scratch directory; file readers close with the scope.
/// It owns neither a task nor a cached index/authentication result.
pub(crate) struct ObservationByteReader<'a> {
    source: &'a SeekableObservationSource,
    file: byte_carrier::Reader,
}
impl ObservationByteReader<'_> {
    pub(crate) fn indexed_span(
        &self,
        start: u64,
        end: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, ObservationReadError> {
        let count = self.source.indexed_span_count(start, end, max_bytes)?;
        Ok(Layout::read_bytes(&self.file, start, count)?)
    }
}
struct FieldReader<'a> {
    connection: IndexReader<'a>,
    bytes: byte_carrier::Reader,
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
    pub(crate) fn byte_reader(&self) -> Result<ObservationByteReader<'_>, ObservationReadError> {
        Ok(ObservationByteReader {
            source: self,
            file: self.layout.open_bytes()?,
        })
    }
    pub(crate) fn new<T: serde::Serialize + schemars::JsonSchema>(
        value: &T,
    ) -> Result<Self, &'static str> {
        let mut layout = Layout::unindexed();
        {
            use std::io::Write as _;
            let mut writer = std::io::BufWriter::with_capacity(65_536, &mut layout);
            crate::runtime_client::structural_json::to_writer(&mut writer, value)
                .map_err(|_| "native observation source encoding unavailable")?;
            writer
                .flush()
                .map_err(|_| "native observation source flush unavailable")?;
        }
        Self::from_layout(layout)
    }
    /// Re-index one bounded committed structural JSON value without re-encoding it.
    /// The caller retains its native identity and read-cut authority.
    pub(crate) fn from_bounded_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        use std::io::Write as _;
        if bytes.is_empty() || bytes.len() > MEMORY_INDEX_BYTES {
            return Err("inline native source exceeds bound");
        }
        // The ordinary constructor consumes serializer output. Persisted bytes
        // must also pass JSON syntax validation before geometry is trusted.
        serde_json::from_slice::<serde::de::IgnoredAny>(bytes)
            .map_err(|_| "inline native source JSON invalid")?;
        let mut layout = Layout::unindexed();
        layout
            .write_all(bytes)
            .map_err(|_| "inline native source write unavailable")?;
        Self::from_layout(layout)
    }
    fn from_layout(mut layout: Layout) -> Result<Self, &'static str> {
        layout.initialize_index(layout.written <= MEMORY_INDEX_BYTES as u64)?;
        layout.bytes.seal();
        let reader = layout.open_bytes()?;
        let mut reader = std::io::BufReader::with_capacity(
            usize::try_from(layout.written.clamp(1, 65_536))
                .map_err(|_| "native parser buffer exceeds platform capacity")?,
            reader,
        );
        std::io::Seek::rewind(&mut reader).map_err(|_| "native source seek unavailable")?;
        let mut parser = LayoutParser {
            reader,
            position: 0,
            layout: &mut layout,
        };
        parser.value(None, 0, None, 0)?;
        if parser.position != parser.layout.written {
            return Err("native source has trailing bytes");
        }
        layout.seal()?;
        Ok(Self { layout })
    }
    /// Copies this completed source's exact original extent into its native owner.
    /// The borrow pins the source storage until the synchronous copy finishes;
    /// each window is bounded and does not reopen or query the field index.
    pub(crate) fn copy_original_bytes_to(
        &self,
        destination: &mut impl std::io::Write,
    ) -> std::io::Result<u64> {
        let file = self.layout.bytes.reader(self.layout.directory.as_ref())?;
        let capacity = usize::try_from(self.layout.written.min(65_536))
            .map_err(|_| std::io::Error::other("native copy buffer exceeds platform capacity"))?;
        let mut buffer = vec![0; capacity];
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

    /// Reads a trusted span for the independent eager-preview test oracle.
    /// Production fragment reads use one pinned `ObservationByteReader` instead.
    #[cfg(test)]
    pub(crate) fn indexed_span(
        &self,
        start: u64,
        end: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, ObservationReadError> {
        let count = self.indexed_span_count(start, end, max_bytes)?;
        Ok(self.layout.bytes(start, count)?)
    }
    fn indexed_span_count(
        &self,
        start: u64,
        end: u64,
        max_bytes: u32,
    ) -> Result<usize, ObservationReadError> {
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
        usize::try_from(end - start)
            .map_err(|_| "native indexed span exceeds platform capacity".into())
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
        let mut preview_policy = preview_policy;
        let mut visit = visit;
        Self::visit_selected_fields_in(
            &reader,
            first,
            last,
            include,
            |field| preview_policy(field.path),
            |field, preview| {
                visit(
                    field.path,
                    field.encoded_path,
                    field.parent.map(|(_, encoded, ordinal)| (encoded, ordinal)),
                    field.start,
                    field.end,
                    field.children,
                    field.key_span,
                    preview,
                )
            },
        )
    }
    /// Visit immutable navigation geometry without constructing unused previews.
    pub(crate) fn visit_field_geometry(
        &self,
        mut visit: impl FnMut(FieldMetadata<'_>) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let reader = self.field_reader()?;
        Self::visit_selected_fields_in(
            &reader,
            0,
            SAFE,
            |_| true,
            |_| FieldPreviewPolicy::Skip,
            |field, _| visit(field),
        )
    }
    fn field_reader(&self) -> Result<FieldReader<'_>, &'static str> {
        Ok(FieldReader {
            bytes: self.layout.open_bytes()?,
            connection: self.layout.index_reader()?,
        })
    }
    fn visit_selected_fields_in(
        reader: &FieldReader<'_>,
        first: u64,
        last: u64,
        mut include: impl FnMut(u64) -> bool,
        mut preview_policy: impl FnMut(&FieldMetadata<'_>) -> FieldPreviewPolicy,
        mut visit: impl FnMut(FieldMetadata<'_>, FieldPreview<'_>) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let mut stack: Vec<(u64, usize, String, bool)> = Vec::new();
        let mut path = Vec::new();
        let (connection, bytes) = (&reader.connection, &reader.bytes);
        let mut process = |record: NodeRecord| -> Result<(), &'static str> {
            let NodeRecord {
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
            let field = FieldMetadata {
                path: &path,
                encoded_path: &path_json,
                parent: stack
                    .last()
                    .map(|(_, depth, encoded, _)| (&path[..*depth], encoded.as_str(), ordinal)),
                start: node.start,
                end: node.end,
                children: node.children,
                recursive_nodes: node.recursive_nodes,
                object: node.object,
                container: node.container,
                key_span: node.key_span,
                key: node.key.as_deref().filter(|_| node.key_complete),
            };
            let policy = preview_policy(&field);
            let requested = policy == FieldPreviewPolicy::Inline;
            #[cfg(test)]
            FIELD_PREVIEWS.with(|work| {
                let (computed, skipped) = work.get();
                work.set((
                    computed + usize::from(requested),
                    skipped + usize::from(!requested),
                ));
            });
            let value = requested
                .then(|| Self::preview_node(connection, bytes, id, &node, &mut 32))
                .transpose()?;
            let preview = if let Some(value) = &value {
                FieldPreview::Inline(value)
            } else {
                FieldPreview::Skipped
            };
            visit(field, preview)?;
            stack.push((id, path.len(), path_json, node.object));
            Ok(())
        };
        connection.visit(first, last, &mut include, &mut process)?;
        Ok(())
    }
    fn preview_node(
        connection: &IndexReader<'_>,
        bytes: &byte_carrier::Reader,
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
        let children = connection.children(id, 0, 32)?;
        if node.object {
            let mut values = serde_json::Map::new();
            for (child, _) in children {
                if *budget == 0 {
                    break;
                }
                let child_node = connection.node(child)?;
                if child_node.key_complete && child_node.key.is_some() {
                    let value = Self::preview_node(connection, bytes, child, &child_node, budget)?;
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
                let child_node = connection.node(child)?;
                values.push(Self::preview_node(
                    connection,
                    bytes,
                    child,
                    &child_node,
                    budget,
                )?);
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
        let bytes = self.layout.open_bytes()?;
        let connection = self.layout.index_reader()?;
        connection.visit(
            0,
            self.layout.next_node,
            |_| true,
            |record| {
                let NodeRecord {
                    id,
                    parent,
                    ordinal,
                    ..
                } = record;
                while stack.last().map(|(id, _, _)| *id) != parent {
                    stack.pop().ok_or("native layout parent ordering invalid")?;
                }
                let node = connection.node(id)?;
                let mut path = stack
                    .last()
                    .map_or_else(Vec::new, |(_, path, _)| path.clone());
                if let Some(parent) = parent {
                    let object = connection.node(parent)?.object;
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
                Ok(())
            },
        )
    }
    #[cfg(test)]
    fn preview_node_legacy(
        connection: &IndexReader<'_>,
        bytes: &byte_carrier::Reader,
        id: u64,
        budget: &mut usize,
    ) -> Result<Value, &'static str> {
        if *budget == 0 {
            return Ok(Value::Null);
        }
        *budget -= 1;
        let node = connection.node(id)?;
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
        let children = connection.children(id, 0, 32)?;
        if node.object {
            let mut values = serde_json::Map::new();
            for (child, _) in children {
                if *budget == 0 {
                    break;
                }
                let key = connection.node(child)?;
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
    reader: std::io::BufReader<byte_carrier::Reader>,
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
    fn whitespace(&mut self) -> Result<(), &'static str> {
        use std::io::BufRead as _;
        loop {
            if self.layout.expired() {
                return Err("native source indexing deadline reached");
            }
            let buffer = self
                .reader
                .fill_buf()
                .map_err(|_| "native source parser unavailable")?;
            let count = buffer
                .iter()
                .take_while(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
                .count();
            if count == 0 {
                return Ok(());
            }
            self.consume(count);
        }
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
        self.whitespace()?;
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
        let id = self.layout.next_node;
        self.layout.next_node = self
            .layout
            .next_node
            .checked_add(1)
            .filter(|next| *next <= SAFE)
            .ok_or("native source nodes exhausted")?;
        let root_start = self.position;
        self.whitespace()?;
        let start = if parent.is_none() {
            root_start
        } else {
            self.position
        };
        let byte = self.peek()?.ok_or("native source JSON truncated")?;
        let container = matches!(byte, b'{' | b'[');
        let object = byte == b'{';
        let (name, complete, span) = key.map_or((None, true, None), |(name, complete, span)| {
            (Some(name), complete, Some(span))
        });
        let (mut children, mut recursive) = (0_u64, 1_u64);
        if container {
            self.consume(1);
            self.whitespace()?;
            let close = if object { b'}' } else { b']' };
            while self.peek()? != Some(close) {
                if children > 0 {
                    self.expect(b',')?;
                    self.whitespace()?;
                }
                let child_key = if object {
                    Some(self.child_key()?)
                } else {
                    None
                };
                let (_, count) = self.value(Some(id), children, child_key, depth + 1)?;
                recursive = (recursive + count).min(33);
                children += 1;
                self.whitespace()?;
            }
            self.expect(close)?;
        } else if byte == b'"' {
            self.string()?;
        } else {
            while self.peek()?.is_some_and(|byte| {
                !matches!(byte, b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t')
            }) {
                self.consume(1);
            }
        }
        if parent.is_none() {
            self.whitespace()?;
        }
        let end = self.position;
        // IDs remain preorder; completed rows need no placeholder update.
        if let Some(index) = &mut self.layout.small_index {
            index.push(NodeRecord {
                id,
                parent,
                ordinal,
                node: Node {
                    start,
                    end,
                    children,
                    recursive_nodes: recursive,
                    container,
                    object,
                    key: name,
                    key_span: span,
                    key_complete: complete,
                },
            })?;
            #[cfg(test)]
            NODE_WRITES.with(|work| {
                let (inserts, updates) = work.get();
                work.set((inserts + 1, updates));
            });
            return Ok((id, recursive));
        }
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

#[derive(Debug, Clone)]
struct NodeRecord {
    id: u64,
    parent: Option<u64>,
    ordinal: u64,
    node: Node,
}
impl Layout {
    fn visit_selected_records(
        connection: &rusqlite::Connection,
        first: u64,
        last: u64,
        mut include: impl FnMut(u64) -> bool,
        mut visit: impl FnMut(NodeRecord) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let mut statement=connection.prepare_cached("SELECT id,parent,ordinal,start,end,children,recursive_nodes,container,object,key,key_start,key_end,key_complete FROM nodes WHERE id>=?1 AND id<=?2 ORDER BY id").map_err(|_|"native layout traversal unavailable")?;
        let mut rows = statement
            .query(rusqlite::params![first, last])
            .map_err(|_| "native layout traversal unavailable")?;
        while let Some(row) = rows
            .next()
            .map_err(|_| "native layout traversal unavailable")?
        {
            let id = row
                .get(0)
                .map_err(|_| "native layout identity unavailable")?;
            if include(id) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_index_matches_disk_fields_and_allows_reentrant_reads() {
        use std::io::{Seek as _, Write as _};
        let value = serde_json::json!({"000-padding":"p".repeat(8192),"@0":[1,2],"$key":true,"escaped/~":"水", "wide":(0..70).collect::<Vec<_>>()});
        let small = SeekableObservationSource::new(&value).unwrap();
        assert!(small.layout.small_index.is_some());
        let original = original_layout_bytes(&small);
        let mut layout = Layout::new().unwrap();
        layout.write_all(&original).unwrap();
        let mut reader = std::io::BufReader::new(layout.open_bytes().unwrap());
        reader.rewind().unwrap();
        LayoutParser {
            reader,
            position: 0,
            layout: &mut layout,
        }
        .value(None, 0, None, 0)
        .unwrap();
        layout.seal().unwrap();
        let disk = SeekableObservationSource { layout };
        let collect = |source: &SeekableObservationSource| {
            let mut fields = Vec::new();
            source
                .visit_fields(
                    |path, encoded, parent, start, end, children, key, preview| {
                        let bytes = source.read(path, 0, 65536).unwrap().bytes;
                        assert_eq!(
                            bytes,
                            original
                                [usize::try_from(start).unwrap()..usize::try_from(end).unwrap()]
                        );
                        fields.push((
                            path.to_vec(),
                            encoded.to_owned(),
                            parent.map(|(path, n)| (path.to_owned(), n)),
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
            fields
        };
        assert_eq!(collect(&small), collect(&disk));
        assert_eq!(original_layout_bytes(&disk), original);
    }

    #[test]
    fn small_index_admission_uses_exact_encoded_bytes_and_retains_navigation() {
        for (length, memory) in [(16_381, true), (16_382, true), (16_383, false)] {
            let value = "x".repeat(length);
            let source = SeekableObservationSource::new(&value).unwrap();
            assert_eq!(source.layout.written, (length + 2) as u64);
            assert_eq!(source.layout.small_index.is_some(), memory);
            assert_eq!(
                source
                    .layout
                    .directory
                    .as_ref()
                    .is_some_and(|directory| directory.path().join("index.sqlite").exists()),
                !memory
            );
            INDEX_OPENS.with(|n| n.set(0));
            assert_eq!(
                source.read(&[], 0, 65536).unwrap().bytes,
                serde_json::to_vec(&value).unwrap()
            );
            assert_eq!(source.page(&[], 0, 1).unwrap().total, 1);
            assert_eq!(INDEX_OPENS.with(std::cell::Cell::get) == 0, memory);
        }
    }

    #[test]
    fn small_index_dense_input_is_bounded_and_pagination_is_exact() {
        let values = vec![0; 8191]; // 16,383 bytes, 8,192 compact index nodes.
        let source = SeekableObservationSource::new(&values).unwrap();
        assert!(source.layout.small_index.is_some());
        let index = source.layout.small_index.as_ref().unwrap();
        let retained = index.nodes.capacity() * std::mem::size_of::<small::CompactRecord>()
            + index
                .nodes
                .iter()
                .filter_map(|record| record.node.key.as_ref())
                .map(String::capacity)
                .sum::<usize>();
        assert!(retained <= 1_048_576);
        assert!(source.layout.connection.lock().unwrap().is_none());
        let mut offset = 0;
        loop {
            let page = source.page(&[], offset, 32).unwrap();
            assert_eq!(page.total, 8191);
            for (index, field) in page.fields.iter().enumerate() {
                assert_eq!(field.ordinal, offset + index as u64);
                assert_eq!(field.value, Some(serde_json::json!(0)));
            }
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
    }

    #[test]
    fn small_index_preserves_lexical_bytes_keys_and_u64_evidence() {
        #[derive(serde::Serialize, schemars::JsonSchema)]
        struct Fixture<'a> {
            padding: &'a str,
            revision: u64,
            #[schemars(with = "serde_json::Value")]
            raw: &'a serde_json::value::RawValue,
        }
        let raw = serde_json::value::RawValue::from_string(
            "{\"z\":1.00,\"a\":-0.0,\"escaped\":\"\\u6c34\",\"u\":18446744073709551615}".into(),
        )
        .unwrap();
        let padding = "p".repeat(8192);
        let fixture = Fixture {
            padding: &padding,
            revision: u64::MAX,
            raw: &raw,
        };
        let source = SeekableObservationSource::new(&fixture).unwrap();
        assert!(source.layout.small_index.is_some());
        assert!(source.layout.written > 4096);
        let mut expected = Vec::new();
        crate::runtime_client::structural_json::to_writer(&mut expected, &fixture).unwrap();
        let mut copied = Vec::new();
        assert_eq!(
            source.copy_original_bytes_to(&mut copied).unwrap(),
            expected.len() as u64
        );
        assert_eq!(copied, expected);
        assert_eq!(
            source.read(&["raw".into()], 0, 4096).unwrap().bytes,
            raw.get().as_bytes()
        );
        let page = source.page(&["raw".into()], 0, 32).unwrap();
        assert_eq!(
            page.fields
                .iter()
                .map(|f| f.key.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["z", "a", "escaped", "u"]
        );
        assert_eq!(
            source.read(&["revision".into()], 0, 4096).unwrap().bytes,
            serde_json::to_vec(&u64::MAX.to_string()).unwrap()
        );
    }

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
        assert!(matches!(layout.bytes, byte_carrier::Bytes::Writing(_)));
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
        for padding in [0, MEMORY_INDEX_BYTES] {
            let opaque = serde_json::json!({"wide": u64::MAX,"padding":"x".repeat(padding)});
            let native = NativeSource {
                title: "original",
                revision: u64::MAX,
                opaque: &opaque,
            };
            let mut source = SeekableObservationSource::new(&native).unwrap();
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
            if let Some(index) = &mut source.layout.small_index {
                index.nodes.clear();
            } else {
                let connection = rusqlite::Connection::open(
                    source
                        .layout
                        .directory
                        .as_ref()
                        .unwrap()
                        .path()
                        .join("index.sqlite"),
                )
                .unwrap();
                connection.execute("DROP TABLE nodes", []).unwrap();
            }
            assert!(matches!(
                source.read(&["title".into()], 0, 1).unwrap_err(),
                ObservationReadError::Unavailable(_)
            ));
        }
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
        assert!(matches!(
            source.layout.bytes,
            byte_carrier::Bytes::Disk(None)
        ));
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
        assert!(matches!(
            source.layout.bytes,
            byte_carrier::Bytes::Disk(None)
        ));
        assert!(source.layout.connection.lock().unwrap().is_none());
    }
}
