//! Canonical JSON ordering over the existing seekable native byte layout.
//! Original values remain borrowed; wide object key ordering uses a disk index.
use super::{Layout, LayoutParser};
use std::io::{Seek as _, Write};
#[cfg(test)]
thread_local! {pub(crate) static WORK:std::cell::Cell<(usize,usize,usize,u64)>=const {std::cell::Cell::new((0,0,0,0))};}

pub(crate) fn to_writer<T: serde::Serialize + ?Sized>(
    writer: &mut impl Write,
    value: &T,
) -> Result<(), std::io::Error> {
    #[cfg(test)]
    WORK.with(|work| work.set((0, 0, 0, 0)));
    if let Some(bytes) = crate::runtime_client::checkpoint::bounded_bytes(value, 4096) {
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
        return small(writer, &value);
    }
    let mut layout = Layout::new().map_err(error)?;
    // Construction belongs to the optional native commit layout, rather than
    // a client request deadline. Its scratch space remains fixed independently
    // of source size; a busy host must not permanently lose a valid source.
    layout.deadline = None;
    {
        let mut buffered = std::io::BufWriter::with_capacity(65_536, &mut layout);
        serde_json::to_writer(&mut buffered, value).map_err(std::io::Error::other)?;
        buffered.flush()?;
    }
    let mut reader = std::io::BufReader::with_capacity(
        65_536,
        layout
            .file
            .as_ref()
            .ok_or_else(|| error("source sealed"))?
            .try_clone()?,
    );
    #[cfg(test)]
    WORK.with(|work| {
        let (read, key, page, _) = work.get();
        work.set((read, key, page, layout.written));
    });
    reader.rewind()?;
    LayoutParser {
        reader,
        position: 0,
        layout: &mut layout,
    }
    .value(None, 0, None, 0)
    .map_err(error)?;
    layout.connection.get_mut().map_err(|_| error("source poisoned"))?
        .as_mut().ok_or_else(||error("source sealed"))?
        .execute_batch("CREATE TABLE canonical_order(parent INTEGER NOT NULL,ordinal INTEGER NOT NULL,id INTEGER NOT NULL,PRIMARY KEY(parent,ordinal)) WITHOUT ROWID; CREATE TABLE canonical_runs(parent INTEGER NOT NULL,run INTEGER NOT NULL,ordinal INTEGER NOT NULL,id INTEGER NOT NULL,PRIMARY KEY(parent,run,ordinal)) WITHOUT ROWID; CREATE TABLE canonical_run_sizes(parent INTEGER NOT NULL,run INTEGER NOT NULL,count INTEGER NOT NULL,PRIMARY KEY(parent,run)) WITHOUT ROWID")
        .map_err(std::io::Error::other)?;
    let connection = layout.reader_connection().map_err(error)?;
    emit(&layout, &connection, writer, 0, 0)
}
fn small(writer: &mut impl Write, value: &serde_json::Value) -> Result<(), std::io::Error> {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            writer.write_all(b"{")?;
            for (ordinal, key) in keys.into_iter().enumerate() {
                if ordinal > 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, key).map_err(std::io::Error::other)?;
                writer.write_all(b":")?;
                small(writer, &map[key])?;
            }
            writer.write_all(b"}")
        }
        serde_json::Value::Array(items) => {
            writer.write_all(b"[")?;
            for (ordinal, value) in items.iter().enumerate() {
                if ordinal > 0 {
                    writer.write_all(b",")?;
                }
                small(writer, value)?;
            }
            writer.write_all(b"]")
        }
        scalar => serde_json::to_writer(writer, scalar).map_err(std::io::Error::other),
    }
}

fn error(message: &'static str) -> std::io::Error {
    std::io::Error::other(message)
}
fn emit(
    layout: &Layout,
    connection: &rusqlite::Connection,
    writer: &mut impl Write,
    id: u64,
    depth: usize,
) -> Result<(), std::io::Error> {
    if depth > 256 {
        return Err(error("canonical source depth exhausted"));
    }
    let node = Layout::node_at(connection, id).map_err(error)?;
    if !node.container {
        return copy(layout, writer, node.start, node.end);
    }
    if node.object && node.children > 1 {
        super::key_order::order(layout, connection, id)?;
    }
    writer.write_all(if node.object { b"{" } else { b"[" })?;
    for ordinal in 0..node.children {
        if ordinal > 0 {
            writer.write_all(b",")?;
        }
        let child: u64 = if node.object && node.children > 1 {
            connection
                .query_row(
                    "SELECT id FROM canonical_order WHERE parent=?1 AND ordinal=?2",
                    rusqlite::params![id, ordinal],
                    |row| row.get(0),
                )
                .map_err(std::io::Error::other)?
        } else {
            connection
                .query_row(
                    "SELECT id FROM nodes WHERE parent=?1 AND ordinal=?2",
                    rusqlite::params![id, ordinal],
                    |row| row.get(0),
                )
                .map_err(std::io::Error::other)?
        };
        if node.object {
            let child_node = Layout::node_at(connection, child).map_err(error)?;
            let (start, end) = child_node
                .key_span
                .ok_or_else(|| error("canonical key unavailable"))?;
            copy(layout, writer, start, end)?;
            writer.write_all(b":")?;
        }
        emit(layout, connection, writer, child, depth + 1)?;
    }
    writer.write_all(if node.object { b"}" } else { b"]" })
}
fn copy(
    layout: &Layout,
    writer: &mut impl Write,
    mut start: u64,
    end: u64,
) -> Result<(), std::io::Error> {
    while start < end {
        let count = usize::try_from((end - start).min(65_536))
            .map_err(|_| error("canonical source length exhausted"))?;
        #[cfg(test)]
        WORK.with(|work| {
            let (read, key, page, written) = work.get();
            work.set((read.max(count), key, page, written));
        });
        writer.write_all(&layout.bytes(start, count).map_err(error)?)?;
        start += count as u64;
    }
    Ok(())
}
