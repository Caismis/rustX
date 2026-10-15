//! External lexical ordering keeps only 32 native node IDs and two byte readers.
use super::Layout;
use std::cmp::Ordering;
use std::io::{BufReader, Read as _, Seek as _};

pub(super) fn order(
    layout: &Layout,
    connection: &rusqlite::Connection,
    parent: u64,
) -> std::io::Result<()> {
    let mut count = 0_u64;
    let mut offset = 0;
    loop {
        let rows = Layout::children_at(connection, parent, offset, 32).map_err(error)?;
        #[cfg(test)]
        super::canonical::WORK.with(|work| {
            let (read, key, page, written) = work.get();
            work.set((read, key, page.max(rows.len()), written));
        });
        if rows.is_empty() {
            break;
        }
        let mut ids = rows.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        for index in 1..ids.len() {
            let mut selected = index;
            while selected > 0
                && compare(layout, connection, ids[selected - 1], ids[selected])?
                    == Ordering::Greater
            {
                ids.swap(selected - 1, selected);
                selected -= 1;
            }
        }
        for (ordinal, id) in ids.iter().enumerate() {
            insert(connection, parent, count, ordinal as u64, *id)?;
        }
        size(connection, parent, count, ids.len() as u64)?;
        offset = rows
            .last()
            .ok_or_else(|| error("canonical key page unavailable"))?
            .1
            + 1;
        count += 1;
    }
    let mut base = 0;
    let mut next = count;
    while count > 1 {
        let target_base = next;
        for pair in (0..count).step_by(2) {
            let left = base + pair;
            let target = next;
            next += 1;
            if pair + 1 == count {
                connection.execute("INSERT INTO canonical_runs SELECT parent,?3,ordinal,id FROM canonical_runs WHERE parent=?1 AND run=?2",rusqlite::params![parent,left,target]).map_err(std::io::Error::other)?;
                size(
                    connection,
                    parent,
                    target,
                    run_size(connection, parent, left)?,
                )?;
            } else {
                merge(layout, connection, parent, left, left + 1, target)?;
            }
        }
        connection
            .execute(
                "DELETE FROM canonical_runs WHERE parent=?1 AND run>=?2 AND run<?3",
                rusqlite::params![parent, base, base + count],
            )
            .map_err(std::io::Error::other)?;
        connection
            .execute(
                "DELETE FROM canonical_run_sizes WHERE parent=?1 AND run>=?2 AND run<?3",
                rusqlite::params![parent, base, base + count],
            )
            .map_err(std::io::Error::other)?;
        base = target_base;
        count = count.div_ceil(2);
    }
    connection.execute("INSERT INTO canonical_order SELECT parent,ordinal,id FROM canonical_runs WHERE parent=?1 AND run=?2",rusqlite::params![parent,base]).map_err(std::io::Error::other)?;
    connection
        .execute("DELETE FROM canonical_runs WHERE parent=?1", [parent])
        .map_err(std::io::Error::other)?;
    connection
        .execute("DELETE FROM canonical_run_sizes WHERE parent=?1", [parent])
        .map_err(std::io::Error::other)?;
    Ok(())
}
fn merge(
    layout: &Layout,
    connection: &rusqlite::Connection,
    parent: u64,
    left: u64,
    right: u64,
    target: u64,
) -> std::io::Result<()> {
    let (left_count, right_count) = (
        run_size(connection, parent, left)?,
        run_size(connection, parent, right)?,
    );
    let (mut l, mut r, mut ordinal) = (0, 0, 0);
    while l < left_count || r < right_count {
        let from_left = if l == left_count {
            false
        } else if r == right_count {
            true
        } else {
            compare(
                layout,
                connection,
                at(connection, parent, left, l)?,
                at(connection, parent, right, r)?,
            )? != Ordering::Greater
        };
        let id = if from_left {
            let id = at(connection, parent, left, l)?;
            l += 1;
            id
        } else {
            let id = at(connection, parent, right, r)?;
            r += 1;
            id
        };
        insert(connection, parent, target, ordinal, id)?;
        ordinal += 1;
    }
    size(connection, parent, target, ordinal)
}
fn insert(
    connection: &rusqlite::Connection,
    parent: u64,
    run: u64,
    ordinal: u64,
    id: u64,
) -> std::io::Result<()> {
    connection
        .execute(
            "INSERT INTO canonical_runs VALUES(?1,?2,?3,?4)",
            rusqlite::params![parent, run, ordinal, id],
        )
        .map_err(std::io::Error::other)?;
    Ok(())
}
fn size(
    connection: &rusqlite::Connection,
    parent: u64,
    run: u64,
    size: u64,
) -> std::io::Result<()> {
    connection
        .execute(
            "INSERT INTO canonical_run_sizes VALUES(?1,?2,?3)",
            rusqlite::params![parent, run, size],
        )
        .map_err(std::io::Error::other)?;
    Ok(())
}
fn at(
    connection: &rusqlite::Connection,
    parent: u64,
    run: u64,
    ordinal: u64,
) -> std::io::Result<u64> {
    connection
        .query_row(
            "SELECT id FROM canonical_runs WHERE parent=?1 AND run=?2 AND ordinal=?3",
            rusqlite::params![parent, run, ordinal],
            |row| row.get(0),
        )
        .map_err(std::io::Error::other)
}
fn run_size(connection: &rusqlite::Connection, parent: u64, run: u64) -> std::io::Result<u64> {
    connection
        .query_row(
            "SELECT count FROM canonical_run_sizes WHERE parent=?1 AND run=?2",
            rusqlite::params![parent, run],
            |row| row.get(0),
        )
        .map_err(std::io::Error::other)
}
fn error(message: &'static str) -> std::io::Error {
    std::io::Error::other(message)
}
fn compare(
    layout: &Layout,
    connection: &rusqlite::Connection,
    left: u64,
    right: u64,
) -> std::io::Result<Ordering> {
    let left = Layout::node_at(connection, left).map_err(error)?;
    let right = Layout::node_at(connection, right).map_err(error)?;
    if left.key_complete && right.key_complete {
        return Ok(left.key.cmp(&right.key));
    }
    let mut left = KeyChars::new(
        layout,
        left.key_span
            .ok_or_else(|| error("canonical key missing"))?,
    )?;
    let mut right = KeyChars::new(
        layout,
        right
            .key_span
            .ok_or_else(|| error("canonical key missing"))?,
    )?;
    loop {
        let order = left.next()?.cmp(&right.next()?);
        if order != Ordering::Equal {
            return Ok(order);
        }
        if left.remaining == 0 && right.remaining == 0 {
            return Ok(Ordering::Equal);
        }
    }
}
struct KeyChars {
    reader: BufReader<std::fs::File>,
    remaining: u64,
}
impl KeyChars {
    fn new(layout: &Layout, (start, end): (u64, u64)) -> std::io::Result<Self> {
        let mut file = std::fs::File::open(layout.directory.path().join("bytes"))?;
        file.seek(std::io::SeekFrom::Start(start + 1))?;
        #[cfg(test)]
        super::canonical::WORK.with(|work| {
            let (read, key, page, written) = work.get();
            work.set((read, key.max(65_536), page, written));
        });
        Ok(Self {
            reader: BufReader::with_capacity(65_536, file),
            remaining: end
                .checked_sub(start + 2)
                .ok_or_else(|| error("canonical key span invalid"))?,
        })
    }
    fn byte(&mut self) -> std::io::Result<u8> {
        if self.remaining == 0 {
            return Err(error("canonical key escape truncated"));
        }
        let mut byte = [0];
        self.reader.read_exact(&mut byte)?;
        self.remaining -= 1;
        Ok(byte[0])
    }
    fn unit(&mut self) -> std::io::Result<u16> {
        let mut value = 0;
        for _ in 0..4 {
            let byte = self.byte()?;
            value = (value << 4)
                | u16::from(match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => return Err(error("canonical key escape invalid")),
                });
        }
        Ok(value)
    }
    fn next(&mut self) -> std::io::Result<Option<char>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let byte = self.byte()?;
        if byte == b'\\' {
            return self.escape().map(Some);
        }
        let count = match byte {
            0..=127 => 1,
            192..=223 => 2,
            224..=239 => 3,
            240..=244 => 4,
            _ => return Err(error("canonical key UTF8 invalid")),
        };
        let mut bytes = [byte, 0, 0, 0];
        for next in bytes.iter_mut().take(count).skip(1) {
            *next = self.byte()?;
        }
        std::str::from_utf8(&bytes[..count])
            .map_err(std::io::Error::other)?
            .chars()
            .next()
            .ok_or_else(|| error("canonical key character missing"))
            .map(Some)
    }
    fn escape(&mut self) -> std::io::Result<char> {
        Ok(match self.byte()? {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{0008}',
            b'f' => '\u{000c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let first = self.unit()?;
                let scalar = if (0xd800..=0xdbff).contains(&first) {
                    if self.byte()? != b'\\' || self.byte()? != b'u' {
                        return Err(error("canonical key surrogate invalid"));
                    }
                    let second = self.unit()?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(error("canonical key surrogate invalid"));
                    }
                    0x1_0000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
                } else {
                    u32::from(first)
                };
                char::from_u32(scalar).ok_or_else(|| error("canonical key scalar invalid"))?
            }
            _ => return Err(error("canonical key escape invalid")),
        })
    }
}
