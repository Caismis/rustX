//! Byte/field layouts of existing Goal, canonical Todo and Compaction owners.
//! Goal layouts are current-only; immutable Todo/J layouts share their native retention.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::observation::{
    ObservationReadCut, ObservationValueChunk, RequestObservationField, RequestObservationPage,
};
const TABLE: &str = "state_observation_fields";
fn kind(domain: &str) -> Result<&'static str, ConversationStoreError> {
    match domain {
        "goal" => Ok("observation_goal"),
        "todos" => Ok("observation_todo"),
        "context" => Ok("observation_context"),
        _ => Err(ConversationStoreError::InvalidReference(
            "unknown native state domain".into(),
        )),
    }
}
pub(super) fn goal_key(reference: &crate::goal::GoalRef) -> String {
    format!("goal/{}/{}", reference.id, reference.revision)
}
pub(super) fn goal(
    connection: &Connection,
    value: &crate::goal::GoalSnapshot,
) -> Result<(), ConversationStoreError> {
    connection
        .execute(
            "DELETE FROM state_observation_fields WHERE source_id LIKE 'goal/%'",
            [],
        )
        .map_err(|error| storage(error.to_string()))?;
    connection
        .execute(
            "DELETE FROM content_segments WHERE source_kind='observation_goal'",
            [],
        )
        .map_err(|error| storage(error.to_string()))?;
    connection
        .execute(
            "DELETE FROM content_frontiers WHERE source_kind='observation_goal'",
            [],
        )
        .map_err(|error| storage(error.to_string()))?;
    let id = goal_key(&value.reference);
    super::observation_request::index_state(
        connection,
        "observation_goal",
        &id,
        &crate::goal::GoalView {
            current: Some(value.clone()),
        },
    )?;
    connection
        .execute(
            "UPDATE goal_state SET observation_source_id=?1 WHERE singleton=1",
            [&id],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
pub(super) fn message(
    connection: &Connection,
    message: &crate::message::types::MessageBlock,
) -> Result<(), ConversationStoreError> {
    let Some(value) = todo_value(message) else {
        return Ok(());
    };
    let ledger: u64 = connection
        .query_row(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [message.id().as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let id = format!("todos/{ledger}");
    let summary = todo_summary(value, ledger)?;
    let header = crate::runtime_client::checkpoint::bounded_bytes(&summary, 16_384)
        .ok_or_else(|| storage("native Todo header exceeds bound"))?;
    connection.execute("INSERT INTO state_observation_sources(domain,source_id,ledger_position,todo_header) VALUES('todos',?1,?2,?3)",params![id,ledger,header]).map_err(|error|storage(error.to_string()))?;
    super::observation_request::index_state(
        connection,
        "observation_todo",
        &id,
        &BorrowedTodo(value),
    )
}

struct BorrowedTodo<'a>(&'a serde_json::Value);
impl serde::Serialize for BorrowedTodo<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl schemars::JsonSchema for BorrowedTodo<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        <crate::tools::todo::TodoSnapshot as schemars::JsonSchema>::schema_name()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <crate::tools::todo::TodoSnapshot as schemars::JsonSchema>::json_schema(generator)
    }
}
pub(super) fn todo_value(
    message: &crate::message::types::MessageBlock,
) -> Option<&serde_json::Value> {
    use crate::message::types::MessageBlock;
    use crate::tools::types::ToolResultContent;
    let MessageBlock::Tool(tool) = message else {
        return None;
    };
    if tool.tool_id.as_str() != crate::tools::todo::TODO_TOOL_ID
        || tool.result.status != crate::tools::types::ToolExecutionStatus::Success
    {
        return None;
    }
    tool.result.content.iter().find_map(|item| match item {
        ToolResultContent::Json { value } => Some(value),
        _ => None,
    })
}
fn todo_summary(
    value: &serde_json::Value,
    ledger: u64,
) -> Result<crate::durable::NativeTodoObservation, ConversationStoreError> {
    let tasks = value
        .get("tasks")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| storage("native Todo tasks unavailable"))?;
    let next_id = value
        .get("next_id")
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| storage("native Todo allocator unavailable"))?;
    let mut snapshot = crate::tools::todo::TodoSnapshot {
        tasks: Vec::new(),
        next_id,
    };
    let mut remaining = 15_360;
    for task in tasks.iter().take(32) {
        let Some(bytes) = crate::runtime_client::checkpoint::bounded_bytes(task, remaining) else {
            break;
        };
        let parsed = serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
        remaining = remaining.saturating_sub(bytes.len() + 1);
        snapshot.tasks.push(parsed);
    }
    let total = tasks.len() as u64;
    let loaded = snapshot.tasks.len() as u64;
    Ok(crate::durable::NativeTodoObservation {
        source_ledger_position: ledger,
        snapshot,
        total,
        loaded,
        next_offset: (loaded < total).then_some(loaded),
    })
}
pub(super) fn todo_head(
    connection: &Connection,
    first: u64,
    last: u64,
) -> Result<Option<crate::durable::NativeTodoObservation>, ConversationStoreError> {
    let bytes: Option<Option<Vec<u8>>> = connection.query_row(
        "SELECT s.todo_header FROM message_ledger m INDEXED BY message_ledger_todo_idx LEFT JOIN state_observation_sources s ON s.domain='todos' AND s.ledger_position=m.position WHERE m.native_todo=1 AND m.position>=?1 AND m.position<=?2 ORDER BY m.position DESC LIMIT 1",
        params![first,last], |row| row.get(0),
    ).optional().map_err(|error| storage(error.to_string()))?;
    match bytes {
        None => Ok(None),
        Some(None) => Err(storage("native Todo observation header unavailable")),
        Some(Some(bytes)) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| storage(error.to_string())),
    }
}

pub(super) fn event(
    connection: &Connection,
    event: &crate::events::types::RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    use crate::events::types::RuntimeEvent;
    if matches!(
        event.event,
        RuntimeEvent::CompactionStarted { .. }
            | RuntimeEvent::CompactionFailed { .. }
            | RuntimeEvent::CompactionCompleted { .. }
    ) {
        connection.execute("INSERT INTO state_observation_sources(domain,source_id,journal_sequence) VALUES('context',?1,?2)",params![format!("context/{}",event.sequence),event.sequence]).map_err(|error|storage(error.to_string()))?;
    }
    match &event.event {
        RuntimeEvent::Goal {
            fact:
                crate::goal::GoalFact::Written { .. } | crate::goal::GoalFact::RoundAdmitted { .. },
        } => {
            connection
                .execute(
                    "UPDATE goal_state SET observation_sequence=?1 WHERE singleton=1",
                    [event.sequence],
                )
                .map_err(|error| storage(error.to_string()))?;
        }
        RuntimeEvent::CompactionStarted { .. } => {
            super::observation_request::index_state(
                connection,
                "observation_context",
                &format!("context/{}", event.sequence),
                &serde_json::json!({}),
            )?;
        }
        RuntimeEvent::CompactionFailed { error, .. } => {
            #[derive(serde::Serialize, schemars::JsonSchema)]
            struct Diagnostic<'a> {
                compaction_error: &'a str,
            }
            super::observation_request::index_state(
                connection,
                "observation_context",
                &format!("context/{}", event.sequence),
                &Diagnostic {
                    compaction_error: error,
                },
            )?;
        }
        RuntimeEvent::CompactionCompleted { occupancy, .. } => {
            #[derive(serde::Serialize, schemars::JsonSchema)]
            struct Diagnostic<'a> {
                latest_compaction: crate::runtime_client::snapshot::RuntimeClientCompactionView,
                occupancy: &'a Option<crate::context::occupancy::ContextOccupancy>,
            }
            let value = Diagnostic {
                latest_compaction: super::observation_occupancy::compaction(connection, event)?,
                occupancy,
            };
            super::observation_request::index_state(
                connection,
                "observation_context",
                &format!("context/{}", event.sequence),
                &value,
            )?;
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn source(
    connection: &Connection,
    domain: &str,
    cut: &ObservationReadCut,
) -> Result<Option<String>, ConversationStoreError> {
    kind(domain)?;
    match domain {
        "goal" => connection.query_row("SELECT observation_source_id FROM goal_state WHERE singleton=1 AND observation_sequence<=?1",[cut.read.journal],|row|row.get(0)).optional().map(Option::flatten),
        "todos" => connection.query_row("SELECT source_id FROM state_observation_sources INDEXED BY state_observation_ledger_idx WHERE domain='todos' AND ledger_position<=?1 ORDER BY ledger_position DESC LIMIT 1",[cut.ledger_through],|row|row.get(0)).optional(),
        "context" => connection.query_row("SELECT source_id FROM state_observation_sources INDEXED BY state_observation_journal_idx WHERE domain='context' AND journal_sequence<=?1 ORDER BY journal_sequence DESC LIMIT 1",[cut.read.journal],|row|row.get(0)).optional(),
        _=>unreachable!(),
    }.map_err(|error|storage(error.to_string()))
}
fn validate(
    connection: &Connection,
    domain: &str,
    id: &str,
    cut: &ObservationReadCut,
) -> Result<&'static str, ConversationStoreError> {
    let kind = kind(domain)?;
    if id.len() > 512 {
        return Err(ConversationStoreError::InvalidReference(
            "native state locator exceeds bound".into(),
        ));
    }
    let admitted = match domain {
        "goal" => connection.query_row("SELECT EXISTS(SELECT 1 FROM goal_state WHERE observation_source_id=?1 AND observation_sequence<=?2)",params![id,cut.read.journal],|row|row.get::<_,bool>(0)),
        "todos" => connection.query_row("SELECT EXISTS(SELECT 1 FROM state_observation_sources s JOIN message_ledger m ON m.position=s.ledger_position WHERE s.domain='todos' AND s.source_id=?1 AND m.position<=?2)",params![id,cut.ledger_through],|row|row.get::<_,bool>(0)),
        "context" => connection.query_row("SELECT EXISTS(SELECT 1 FROM state_observation_sources s JOIN events e ON e.sequence=s.journal_sequence WHERE s.domain='context' AND s.source_id=?1 AND e.sequence<=?2)",params![id,cut.read.journal],|row|row.get::<_,bool>(0)),
        _ => unreachable!(),
    }.map_err(|error|storage(error.to_string()))?;
    if !admitted {
        return Err(ConversationStoreError::InvalidReference(
            "native state source is outside read cut or has been replaced".into(),
        ));
    }
    Ok(kind)
}
pub(super) fn header(
    connection: &Connection,
    domain: &str,
    id: &str,
    cut: &ObservationReadCut,
    path: &[String],
) -> Result<RequestObservationField, ConversationStoreError> {
    validate(connection, domain, id, cut)?;
    let (start, end, children_total, bytes) =
        super::observation_request::field_from(connection, TABLE, id, path)?;
    Ok(RequestObservationField {
        path: path.to_vec(),
        key: None,
        value: serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?,
        content: Vec::new(),
        collections: Vec::new(),
        encoded_bytes: end - start,
        children_total,
        deferred: end - start > bytes.len() as u64,
    })
}
pub(super) fn page(
    connection: &Connection,
    domain: &str,
    id: &str,
    cut: &ObservationReadCut,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<RequestObservationPage, ConversationStoreError> {
    validate(connection, domain, id, cut)?;
    super::observation_request::page_from(connection, TABLE, id, path, offset, limit)
}
pub(super) fn read(
    connection: &Connection,
    domain: &str,
    id: &str,
    cut: &ObservationReadCut,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    let kind = validate(connection, domain, id, cut)?;
    super::observation_request::read_from(connection, kind, TABLE, id, path, offset, max_bytes)
}
