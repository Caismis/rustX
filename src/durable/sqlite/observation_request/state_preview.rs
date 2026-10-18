//! Bounded on-demand previews over retained native state coordinates and bytes.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use serde_json::Value;

pub(super) const COLUMNS: &str = "start,end,children,preview_nodes,preview_kind,CASE WHEN preview_key IS NULL OR (typeof(preview_key)='text' AND length(CAST(preview_key AS BLOB))<=256) THEN preview_key ELSE 0 END";

pub(super) struct Field {
    pub start: u64,
    pub end: u64,
    pub children: u64,
    nodes: u8,
    kind: u8,
    key: Option<String>,
}
impl Field {
    pub(super) fn row(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            start: row.get(at)?,
            end: row.get(at + 1)?,
            children: row.get(at + 2)?,
            nodes: row.get(at + 3)?,
            kind: row.get(at + 4)?,
            key: row.get(at + 5)?,
        })
    }
    pub(super) fn materialize(
        self,
        connection: &Connection,
        id: &str,
        path: &[String],
        source: &Source,
    ) -> Result<(u64, u64, u64, Vec<u8>), ConversationStoreError> {
        let preview = self.preview(connection, id, path, source, &mut 32)?;
        // The historical message override applies only to the requested field,
        // never recursively to a message reached inside another preview.
        let value = if path.len() == 2 && path[0] == "messages" && self.end - self.start <= 16_384 {
            source.value(connection, id, self.start, self.end, self.kind)?
        } else {
            preview
        };
        let bytes = crate::runtime_client::checkpoint::bounded_bytes(&value, 16_384)
            .unwrap_or_else(|| b"null".to_vec());
        Ok((self.start, self.end, self.children, bytes))
    }
    fn preview(
        &self,
        connection: &Connection,
        id: &str,
        path: &[String],
        source: &Source,
        budget: &mut usize,
    ) -> Result<Value, ConversationStoreError> {
        if self.start > self.end
            || self.end > source.length
            || !(1..=33).contains(&self.nodes)
            || self.kind > 3
            || matches!(self.kind, 0 | 3) && self.children != 0
            || (self.kind == 3) != path.last().is_some_and(|part| part == "$key")
        {
            return Err(storage("native state preview geometry invalid"));
        }
        if *budget == 0 {
            return Ok(Value::Null);
        }
        *budget -= 1;
        if self.kind == 3 {
            return Ok(Value::String(String::new()));
        }
        let length = self.end - self.start;
        if length <= 16_384 && self.nodes <= 32 {
            *budget = budget.saturating_sub(usize::from(self.nodes.saturating_sub(1)));
            return source.value(connection, id, self.start, self.end, self.kind);
        }
        if self.kind == 0 {
            return if length <= 1024 {
                source.value(connection, id, self.start, self.end, self.kind)
            } else {
                Ok(Value::Null)
            };
        }
        let parent = super::encoded_path(path)?;
        let mut object = serde_json::Map::new();
        let mut array = Vec::new();
        for ordinal in 0..self.children.min(32) {
            if *budget == 0 {
                break;
            }
            let sql = format!(
                "SELECT CASE WHEN typeof(path_json)='text' AND length(CAST(path_json AS BLOB))<=4096 THEN path_json END,{COLUMNS} FROM state_observation_fields WHERE source_id=?1 AND parent_path=?2 AND ordinal=?3"
            );
            let (encoded_child, child): (String, Self) = connection
                .prepare_cached(&sql)
                .map_err(|e| storage(e.to_string()))?
                .query_row(params![id, parent, ordinal], |row| {
                    Ok((row.get(0)?, Self::row(row, 1)?))
                })
                .optional()
                .map_err(|e| storage(e.to_string()))?
                .ok_or_else(|| storage("native state preview child unavailable"))?;
            let child_path: Vec<String> =
                serde_json::from_str(&encoded_child).map_err(|e| storage(e.to_string()))?;
            if child.start < self.start
                || child.end > self.end
                || child_path.len() != path.len() + 1
                || !child_path.starts_with(path)
            {
                return Err(storage("native state preview child geometry invalid"));
            }
            if self.kind == 2 {
                if let Some(key) = &child.key {
                    object.insert(
                        key.clone(),
                        child.preview(connection, id, &child_path, source, budget)?,
                    );
                }
            } else {
                array.push(child.preview(connection, id, &child_path, source, budget)?);
            }
        }
        Ok(if self.kind == 2 {
            Value::Object(object)
        } else {
            Value::Array(array)
        })
    }
}

pub(super) struct Source {
    kind: String,
    length: u64,
}
impl Source {
    pub(super) fn load(connection: &Connection, id: &str) -> Result<Self, ConversationStoreError> {
        let (kind,length):(String,u64)=connection.query_row("SELECT CASE WHEN typeof(source_kind)='text' AND length(CAST(source_kind AS BLOB))<=128 THEN source_kind END,end FROM state_observation_fields WHERE source_id=?1 AND path_json='[]' AND start=0",[id],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|e|storage(e.to_string()))?;
        if length > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX {
            return Err(storage("native state preview extent invalid"));
        }
        Ok(Self { kind, length })
    }
    fn value(
        &self,
        connection: &Connection,
        id: &str,
        start: u64,
        end: u64,
        kind: u8,
    ) -> Result<Value, ConversationStoreError> {
        if start > end || end > self.length || end - start > 16_384 {
            return Err(storage("native state preview span invalid"));
        }
        let bytes = super::super::content::range_bytes(
            connection,
            (
                &self.kind,
                id,
                0,
                crate::runtime_client::content::ContentRepresentation::CanonicalJsonV1,
            ),
            start,
            end,
        )?;
        if u64::try_from(bytes.len()).map_err(|e| storage(e.to_string()))? != end - start {
            return Err(storage("native state preview bytes truncated"));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|e| storage(e.to_string()))?;
        let actual = u8::from(value.is_array() || value.is_object()) + u8::from(value.is_object());
        if actual != kind {
            return Err(storage("native state preview kind differs from bytes"));
        }
        Ok(value)
    }
}

const FIELDS_SQL: &str = "CREATE TABLE state_observation_fields (
    source_id TEXT NOT NULL,
    source_kind TEXT CHECK(source_kind IS NULL OR (typeof(source_kind)='text' AND octet_length(source_kind)<=128)),
    path_json TEXT NOT NULL,parent_path TEXT,ordinal INTEGER,
    key_path TEXT,key_header BLOB CHECK(key_header IS NULL OR length(key_header)<=4096),
    start INTEGER NOT NULL CHECK(start>=0),end INTEGER NOT NULL CHECK(end>=start),
    children INTEGER NOT NULL CHECK(children>=0),
    preview_nodes INTEGER NOT NULL CHECK(typeof(preview_nodes)='integer' AND preview_nodes BETWEEN 1 AND 33),
    preview_kind INTEGER NOT NULL CHECK(typeof(preview_kind)='integer' AND preview_kind BETWEEN 0 AND 3),
    preview_key TEXT CHECK(preview_key IS NULL OR (typeof(preview_key)='text' AND octet_length(preview_key)<=256)),
    CHECK((path_json='[]' AND source_kind IS NOT NULL) OR (path_json!='[]' AND source_kind IS NULL)),
    PRIMARY KEY(source_id,path_json)
) WITHOUT ROWID";

pub(in crate::durable::sqlite) fn create_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for sql in [
        FIELDS_SQL,
        "CREATE UNIQUE INDEX state_observation_children_idx ON state_observation_fields(source_id,parent_path,ordinal)",
    ] {
        connection
            .execute_batch(sql)
            .map_err(|e| storage(e.to_string()))?;
    }
    Ok(())
}
pub(in crate::durable::sqlite) fn verify_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    let actual: Option<String> = connection.query_row(
        "SELECT CASE WHEN typeof(sql)='text' AND octet_length(sql)<=8192 THEN sql END FROM sqlite_schema WHERE type='table' AND name='state_observation_fields'",
        [], |row| row.get(0)).optional().map_err(|error| storage(error.to_string()))?.flatten();
    if actual.as_deref() != Some(FIELDS_SQL) {
        return Err(ConversationStoreError::IncompatibleSchema(
            "missing or invalid state preview table".into(),
        ));
    }
    Ok(())
}

pub(super) fn index_fields(
    writer: &super::SourceWriter<'_>,
    source: &crate::runtime::read_layout::SeekableObservationSource,
) -> Result<(), ConversationStoreError> {
    let connection = writer.connection;
    let mut fields=connection.prepare_cached("INSERT INTO state_observation_fields(source_id,path_json,parent_path,ordinal,start,end,children,preview_nodes,preview_kind,preview_key,source_kind) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)").map_err(|e|storage(e.to_string()))?;
    let mut failure = None;
    source
        .visit_field_geometry(|field| {
            let result = (|| {
                let kind = u8::from(field.container) + u8::from(field.object);
                fields
                    .execute(params![
                        writer.id,
                        field.encoded_path,
                        field.parent.map(|(_, path, _)| path),
                        field.parent.map(|(_, _, ordinal)| ordinal),
                        field.start,
                        field.end,
                        field.children,
                        field.recursive_nodes.min(33),
                        kind,
                        field.key,
                        field.path.is_empty().then_some(writer.kind)
                    ])
                    .map_err(|e| storage(e.to_string()))?;
                if let Some((start, end)) = field.key_span
                    && field.path.last().is_some_and(|part| part.starts_with('@'))
                {
                    super::index_key(writer, field.path, "", start, end)?;
                }
                Ok(())
            })();
            result.map_err(|error| {
                failure = Some(error);
                "native state geometry unavailable"
            })
        })
        .map_err(|error| failure.unwrap_or_else(|| storage(error)))?;
    Ok(())
}
