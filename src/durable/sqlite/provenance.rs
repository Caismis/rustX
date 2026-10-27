//! Immutable bootstrap records and exact canonical Assistant membership.
use super::{
    Connection, ConversationStoreError, Digest, LineageSeed, MessageBlock, MessageId,
    OptionalExtension, Sha256, decode, encode, params, read_inherited_occupancy, seq_to_i64,
    storage,
};
use crate::durable::reading::TurnReadingProvenance;
use crate::durable::response::CompletedResponseProvenance;
use crate::durable::sqlite::statements::FixedStatements;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
thread_local! {
    static TURN_HEADER_READ_WORK: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) };
    static TURN_HEADER_VALIDATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TURN_VALIDATION_COUNTS: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) };
}

/// Decoded Turn payloads and complete Turn membership validations on this thread.
#[cfg(test)]
pub(super) fn take_turn_validation_counts() -> (usize, usize) {
    TURN_VALIDATION_COUNTS.with(|counts| counts.replace((0, 0)))
}

/// Successful finite inherited header validations; no original payload is decoded.
#[cfg(test)]
pub(super) fn take_turn_header_validations() -> usize {
    TURN_HEADER_VALIDATIONS.with(|count| count.replace(0))
}

/// Returned finite metadata rows and bytes, measured immediately after SQL guards.
#[cfg(test)]
pub(super) fn take_turn_header_read_work() -> (usize, usize) {
    TURN_HEADER_READ_WORK.with(|work| work.replace((0, 0)))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CapturedTurnHeader<T, D = String> {
    ordinal: i64,
    payload_digest: D,
    value: T,
}

#[derive(Clone, Copy)]
pub(super) enum TurnHeaderKind {
    Native,
    Observed,
}

fn turn_header_witness(kind: TurnHeaderKind, ordinal: i64, native: &str, bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"rustx-inherited-turn-header\0");
    digest.update(match kind {
        TurnHeaderKind::Native => b"native".as_slice(),
        TurnHeaderKind::Observed => b"observed".as_slice(),
    });
    digest.update([0]);
    digest.update(ordinal.to_le_bytes());
    digest.update(native.as_bytes());
    digest.update(bytes);
    digest.finalize().into()
}

/// Called only in the existing optional native header savepoint. The original
/// validated digest/ordinal and exact encoded layout are bound on that same row.
pub(super) fn write_turn_header<T: serde::Serialize>(
    connection: &Connection,
    ordinal: i64,
    kind: TurnHeaderKind,
    value: &T,
) -> Result<(), ConversationStoreError> {
    let native: Option<String> = connection.query_row(
        "SELECT CASE WHEN typeof(payload_digest)='text' AND length(CAST(payload_digest AS BLOB))=64 THEN payload_digest END FROM inherited_turn_provenance WHERE ordinal=?1",
        [ordinal], |row| row.get(0),
    ).map_err(|error| storage(error.to_string()))?;
    let native = native.ok_or_else(|| storage("native turn digest unavailable"))?;
    let bytes = crate::runtime_client::checkpoint::bounded_bytes(
        &CapturedTurnHeader {
            ordinal,
            payload_digest: &native,
            value,
        },
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
    .ok_or_else(|| storage("inherited turn header exceeds bound"))?;
    let witness = turn_header_witness(kind, ordinal, &native, &bytes);
    let sql = match kind {
        TurnHeaderKind::Native => {
            "UPDATE inherited_turn_provenance SET observation_json=?2,observation_json_witness=?3 WHERE ordinal=?1"
        }
        TurnHeaderKind::Observed => {
            "UPDATE inherited_turn_provenance SET observation_turn_display=?2,observation_turn_display_witness=?3 WHERE ordinal=?1"
        }
    };
    connection
        .execute(sql, params![ordinal, bytes, witness.as_slice()])
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

type TurnHeaderEvidence = (Option<Vec<u8>>, Option<String>, Option<Vec<u8>>);

pub(super) fn read_turn_header<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    ordinal: i64,
    kind: TurnHeaderKind,
) -> Result<T, ConversationStoreError> {
    // SQL checks precede Rust allocation, even when fault injection bypasses
    // the schema CHECKs. No original payload or member list is selected.
    let sql = match kind {
        TurnHeaderKind::Native => {
            "SELECT CASE WHEN typeof(observation_json)='blob' AND length(observation_json)<=16384 THEN observation_json END,CASE WHEN typeof(payload_digest)='text' AND length(CAST(payload_digest AS BLOB))=64 THEN payload_digest END,CASE WHEN typeof(observation_json_witness)='blob' AND length(observation_json_witness)=32 THEN observation_json_witness END FROM inherited_turn_provenance WHERE ordinal=?1"
        }
        TurnHeaderKind::Observed => {
            "SELECT CASE WHEN typeof(observation_turn_display)='blob' AND length(observation_turn_display)<=16384 THEN observation_turn_display END,CASE WHEN typeof(payload_digest)='text' AND length(CAST(payload_digest AS BLOB))=64 THEN payload_digest END,CASE WHEN typeof(observation_turn_display_witness)='blob' AND length(observation_turn_display_witness)=32 THEN observation_turn_display_witness END FROM inherited_turn_provenance WHERE ordinal=?1"
        }
    };
    let (bytes, native, witness): TurnHeaderEvidence = connection
        .query_row(sql, [ordinal], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|error| storage(error.to_string()))?;
    #[cfg(test)]
    TURN_HEADER_READ_WORK.with(|work| {
        let (rows, materialized) = work.get();
        let bytes = bytes.as_ref().map_or(0, Vec::len)
            + native.as_ref().map_or(0, String::len)
            + witness.as_ref().map_or(0, Vec::len);
        work.set((rows + 1, materialized + bytes));
    });
    let bytes = bytes.ok_or_else(|| storage("inherited turn header unavailable"))?;
    let native = native.ok_or_else(|| storage("native turn digest unavailable"))?;
    let witness = witness.ok_or_else(|| storage("inherited turn header witness unavailable"))?;
    if !native.bytes().all(|byte| byte.is_ascii_hexdigit())
        || witness != turn_header_witness(kind, ordinal, &native, &bytes)
    {
        return Err(storage("inherited turn header read witness corrupt"));
    }
    let header: CapturedTurnHeader<T> =
        serde_json::from_slice(&bytes).map_err(|_| storage("inherited turn header corrupt"))?;
    if header.ordinal != ordinal || header.payload_digest != native {
        return Err(storage(
            "inherited turn header differs from native provenance",
        ));
    }
    #[cfg(test)]
    TURN_HEADER_VALIDATIONS.with(|count| count.set(count.get() + 1));
    Ok(header.value)
}

pub(super) fn verify_turn_header_shape(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    let sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='inherited_turn_provenance'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?
        .ok_or_else(|| {
            ConversationStoreError::IncompatibleSchema("inherited turn header row absent".into())
        })?;
    for constraint in [
        "CHECK(observation_json_witness IS NULL OR length(observation_json_witness)=32)",
        "CHECK(observation_turn_display_witness IS NULL OR length(observation_turn_display_witness)=32)",
        "CHECK(observation_json IS NULL OR length(observation_json)<=16384)",
        "CHECK(observation_turn_display IS NULL OR length(observation_turn_display)<=16384)",
    ] {
        if !sql.contains(constraint) {
            return Err(ConversationStoreError::IncompatibleSchema(
                "inherited turn header witness bounds absent".into(),
            ));
        }
    }
    let columns: Vec<String> = connection
        .prepare("PRAGMA table_info(inherited_turn_provenance)")
        .map_err(|error| storage(error.to_string()))?
        .query_map([], |row| row.get(1))
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<_, _>>()
        .map_err(|error| storage(error.to_string()))?;
    let payload = columns
        .iter()
        .position(|column| column == "payload")
        .ok_or_else(|| {
            ConversationStoreError::IncompatibleSchema("native inherited payload absent".into())
        })?;
    for column in [
        "observation_json",
        "observation_turn_display",
        "observation_json_witness",
        "observation_turn_display_witness",
        "payload_digest",
    ] {
        if columns
            .iter()
            .position(|name| name == column)
            .is_none_or(|index| index >= payload)
        {
            return Err(ConversationStoreError::IncompatibleSchema(
                "inherited turn read evidence must precede payload".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_turn_cursor(
    connection: &Connection,
    cursor: Option<crate::durable::TranscriptCursor>,
) -> Result<(), ConversationStoreError> {
    if let Some(cursor) = cursor {
        let present: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM transcript_order WHERE position=?1)",
                [cursor.get()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        if !present {
            return Err(storage("inherited turn transcript location unavailable"));
        }
    }
    Ok(())
}

fn decode_turn(json: &str) -> Result<TurnReadingProvenance, ConversationStoreError> {
    #[cfg(test)]
    TURN_VALIDATION_COUNTS.with(|counts| {
        let (decoded, members) = counts.get();
        counts.set((decoded + 1, members));
    });
    decode(json, "inherited Turn")
}

pub(super) const TURN_MEMBERS: &str = "SELECT assistant_message_id FROM inherited_message_membership INDEXED BY inherited_membership_turn_idx WHERE turn_ordinal=?1 ORDER BY assistant_message_id";

pub(super) const TURN_LOOKUP: &str = "SELECT m.assistant_message_id,m.turn_ordinal,m.response_ordinal,m.membership_digest,p.ordinal,p.payload,p.payload_digest FROM message_ledger l LEFT JOIN inherited_message_membership m ON m.assistant_message_id=l.message_id AND m.assistant_ledger_position=l.position LEFT JOIN inherited_turn_provenance p ON p.ordinal=m.turn_ordinal WHERE l.message_id=?1 AND l.position<=(SELECT message_count FROM bootstrap_identity WHERE id=1) AND json_extract(l.message_json,'$.role')='assistant'";
pub(super) const RESPONSE_LOOKUP: &str = "SELECT m.assistant_message_id,m.turn_ordinal,m.response_ordinal,m.membership_digest,p.ordinal,p.payload,p.payload_digest FROM message_ledger l LEFT JOIN inherited_message_membership m ON m.assistant_message_id=l.message_id AND m.assistant_ledger_position=l.position LEFT JOIN inherited_response_provenance p ON p.ordinal=m.response_ordinal WHERE l.message_id=?1 AND l.position<=(SELECT message_count FROM bootstrap_identity WHERE id=1) AND json_extract(l.message_json,'$.role')='assistant'";

fn digest(value: &str) -> String {
    use std::fmt::Write as _;
    let mut digest = String::new();
    for byte in Sha256::digest(value.as_bytes()) {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}
fn membership_digest(id: &str, turn: Option<i64>, response: Option<i64>) -> String {
    use std::fmt::Write as _;
    let mut hash = Sha256::new();
    hash.update(b"rustx-inherited-member\n");
    hash.update(id.as_bytes());
    hash.update(format!("\n{turn:?}\n{response:?}").as_bytes());
    let mut result = String::with_capacity(64);
    for byte in hash.finalize() {
        let _ = write!(result, "{byte:02x}");
    }
    result
}
fn payload_digest(kind: &str, ordinal: i64, payload: &str) -> String {
    digest(&format!("rustx-inherited-{kind}\n{ordinal}\n{payload}"))
}

pub(super) fn counts_digest(history: &str, turns: i64, responses: i64) -> String {
    digest(&format!(
        "rustx-inherited-counts\n{history}\n{turns}\n{responses}"
    ))
}

fn count(connection: &Connection, kind: &str) -> Result<i64, ConversationStoreError> {
    let metadata:Option<(String,i64,i64,String)>=connection.query_row("SELECT history_digest,turn_count,response_count,provenance_digest FROM bootstrap_identity WHERE id=1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(|error|storage(error.to_string()))?;
    let Some((history, turns, responses, hash)) = metadata else {
        return Ok(0);
    };
    if turns < 0 || responses < 0 || hash != counts_digest(&history, turns, responses) {
        return Err(storage("inconsistent immutable provenance metadata"));
    }
    Ok(if kind == "turn" { turns } else { responses })
}

/// Native immutable count/digest and the two indexed endpoint witnesses only.
/// Interior rows and members are checked when selected, never enumerated here.
pub(super) fn turn_header_count(connection: &Connection) -> Result<i64, ConversationStoreError> {
    let total = count(connection, "turn")?;
    let first: Option<i64> = connection
        .query_row(
            "SELECT ordinal FROM inherited_turn_provenance ORDER BY ordinal LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let last: Option<i64> = connection
        .query_row(
            "SELECT ordinal FROM inherited_turn_provenance ORDER BY ordinal DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    if (total == 0 && (first.is_some() || last.is_some()))
        || (total > 0 && (first != Some(0) || last != Some(total - 1)))
    {
        return Err(storage("inherited turn endpoints differ from native count"));
    }
    Ok(total)
}

pub(super) fn turn_count(connection: &Connection) -> Result<i64, ConversationStoreError> {
    read_inherited_occupancy(connection)?;
    count(connection, "turn")
}

pub(super) fn persist(
    connection: &Connection,
    seed: &LineageSeed,
) -> Result<(), ConversationStoreError> {
    let mut membership = BTreeMap::new();
    for message in seed.canonical() {
        if let MessageBlock::Assistant(message) = message {
            membership.insert(message.id.clone(), (None, None));
        }
    }
    for (ordinal, turn) in seed.turns().iter().enumerate() {
        let ordinal = seq_to_i64(ordinal as u64)?;
        let payload = encode(turn, "inherited turn")?;
        let display = TurnReadingProvenance {
            id: turn.id.clone(),
            process_message_ids: Vec::new(),
            preceding_message_id: turn.preceding_message_id.clone(),
            prompt_message_id: turn.prompt_message_id.clone(),
            outcome: turn.outcome.clone(),
            started_at: turn.started_at,
            ended_at: turn.ended_at,
            execution: None,
        };
        connection.execute("INSERT INTO inherited_turn_provenance(ordinal,payload,payload_digest) VALUES(?1,?2,?3)", params![ordinal,payload,payload_digest("turn",ordinal,&payload)]).map_err(|error| storage(error.to_string()))?;
        let _ = super::optional_display_layout(connection, || {
            super::observation_identity::capture(
                connection,
                crate::runtime_client::response::InheritedAttemptIdentityKind::Turn,
                u64::try_from(ordinal).map_err(|_| storage("negative inherited ordinal"))?,
                &turn.id.attempt_id,
            )
        });
        let _ = super::optional_display_layout(connection, || {
            connection
                .execute(
                    "UPDATE inherited_turn_provenance SET display_provenance=?2 WHERE ordinal=?1",
                    params![ordinal, display_bytes(&display)?],
                )
                .map_err(|error| storage(error.to_string()))?;
            Ok(())
        });
        for id in &turn.process_message_ids {
            let owner = membership
                .get_mut(id)
                .ok_or_else(|| storage("unknown inherited Turn member"))?;
            if owner.0.replace(ordinal).is_some() {
                return Err(storage("duplicate inherited Turn membership"));
            }
        }
    }
    for (ordinal, response) in seed.completed_responses().iter().enumerate() {
        let ordinal = seq_to_i64(ordinal as u64)?;
        let payload = encode(response, "inherited response")?;
        let closing_position: u64 = connection
            .query_row_cached(
                "SELECT position FROM message_ledger WHERE message_id=?1",
                [response.closing_message_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        connection.execute("INSERT INTO inherited_response_provenance(ordinal,payload,payload_digest,closing_ledger_position) VALUES(?1,?2,?3,?4)",params![ordinal,payload,payload_digest("response",ordinal,&payload),closing_position]).map_err(|error|storage(error.to_string()))?;
        let _ = super::optional_display_layout(connection, || {
            super::observation_identity::capture(
                connection,
                crate::runtime_client::response::InheritedAttemptIdentityKind::Response,
                u64::try_from(ordinal).map_err(|_| storage("negative inherited ordinal"))?,
                &response.origin.attempt_id,
            )?;
            response_identity_fields(connection, ordinal, response)?;
            let display = response_header(connection, ordinal, response)?;
            connection.execute("UPDATE inherited_response_provenance SET display_provenance=?2 WHERE ordinal=?1",params![ordinal,display_bytes(&display)?]).map_err(|error|storage(error.to_string()))?;
            Ok(())
        });
        for id in &response.process_message_ids {
            let owner = membership
                .get_mut(id)
                .ok_or_else(|| storage("unknown inherited Response member"))?;
            if owner.1.replace(ordinal).is_some() {
                return Err(storage("duplicate inherited Response membership"));
            }
        }
    }
    for (id, (turn, response)) in membership {
        let position: i64 = connection
            .query_row_cached(
                "SELECT position FROM message_ledger WHERE message_id=?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        let original_digest = membership_digest(id.as_str(), turn, response);
        connection.execute("INSERT INTO inherited_message_membership(assistant_message_id,assistant_ledger_position,turn_ordinal,response_ordinal,membership_digest) VALUES(?1,?2,?3,?4,?5)", params![id.as_str(),position,turn,response,original_digest]).map_err(|error| storage(error.to_string()))?;
        let _ = super::optional_display_layout(connection, || {
            super::observation_membership::member(
                connection,
                u64::try_from(position).map_err(|_| storage("negative native member position"))?,
                &id,
                turn,
                response,
                &original_digest,
            )
        });
    }
    Ok(())
}

fn response_identity_fields(
    connection: &Connection,
    ordinal: i64,
    response: &CompletedResponseProvenance,
) -> Result<(), ConversationStoreError> {
    let key = format!("inherited:{ordinal}");
    for (field, value) in [
        (0, Some(&response.closing_message_id)),
        (1, response.retry_message_id.as_ref()),
        (2, Some(&response.origin.closing_message_id)),
    ] {
        if let Some(value) = value {
            super::content::index_utf8_field(
                connection,
                "response_identity",
                &key,
                field,
                value.as_str(),
            )?;
        }
    }
    Ok(())
}

fn verify_members(
    connection: &Connection,
    kind: &str,
    ordinal: i64,
    members: &[MessageId],
) -> Result<(), ConversationStoreError> {
    #[cfg(test)]
    if kind == "turn" {
        TURN_VALIDATION_COUNTS.with(|counts| {
            let (decoded, members) = counts.get();
            counts.set((decoded, members + 1));
        });
    }
    let mut seen = BTreeSet::new();
    let mut statement = connection.prepare_cached("SELECT m.turn_ordinal,m.response_ordinal,m.membership_digest FROM inherited_message_membership m JOIN message_ledger l ON l.message_id=m.assistant_message_id AND l.position=m.assistant_ledger_position WHERE m.assistant_message_id=?1 AND json_extract(l.message_json,'$.role')='assistant' AND l.position<=(SELECT message_count FROM bootstrap_identity WHERE id=1)").map_err(|error| storage(error.to_string()))?;
    for id in members {
        let (turn, response, hash): (Option<i64>, Option<i64>, String) = statement
            .query_row([id.as_str()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|error| storage(format!("inherited membership: {error}")))?;
        if !seen.insert(id)
            || hash != membership_digest(id.as_str(), turn, response)
            || (if kind == "turn" { turn } else { response }) != Some(ordinal)
        {
            return Err(storage("inconsistent inherited membership"));
        }
    }
    if kind == "turn" {
        // Forward checks establish canonical identity and digest validity for
        // every declared member. The ordinal index proves that no persisted
        // member was omitted, including for a terminal-only Turn.
        let assigned = connection
            .prepare_cached(TURN_MEMBERS)
            .map_err(|error| storage(error.to_string()))?
            .query_map([ordinal], |row| row.get::<_, String>(0))
            .map_err(|error| storage(error.to_string()))?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|error| storage(error.to_string()))?;
        let declared = seen.iter().map(|id| id.as_str().to_owned()).collect();
        if assigned != declared {
            return Err(storage("incomplete inherited Turn membership"));
        }
    }
    Ok(())
}

fn validated_turn(
    connection: &Connection,
    ordinal: i64,
    json: &str,
) -> Result<TurnReadingProvenance, ConversationStoreError> {
    let turn = decode_turn(json)?;
    verify_members(connection, "turn", ordinal, &turn.process_message_ids)?;
    for (id, prompt) in [
        (&turn.preceding_message_id, false),
        (&turn.prompt_message_id, true),
    ] {
        if let Some(id) = id {
            let role:Option<String>=connection.query_row("SELECT json_extract(message_json,'$.role') FROM message_ledger WHERE message_id=?1 AND position<=(SELECT message_count FROM bootstrap_identity WHERE id=1)",[id.as_str()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
            if role.is_none() || (prompt && role.as_deref() != Some("user")) {
                return Err(storage("invalid inherited Turn canonical anchor"));
            }
        }
    }
    if turn.process_message_ids.is_empty() {
        use crate::durable::reading::InheritedTurnOutcome;
        let anchor:Option<i64>=connection.query_row("SELECT position FROM transcript_order WHERE reference_kind='inherited_turn' AND reference_id=?1",[ordinal.to_string()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        if matches!(
            turn.outcome,
            InheritedTurnOutcome::Completed | InheritedTurnOutcome::IncompleteAtCut
        ) || anchor.is_none()
        {
            return Err(storage("invalid terminal-only inherited Turn"));
        }
    }
    if let Some(reading) = turn
        .execution
        .as_ref()
        .and_then(|execution| execution.occupancy.as_ref())
    {
        reading.validate()?;
    }
    Ok(turn)
}

fn selected(
    connection: &Connection,
    messages: &[MessageId],
    kind: &str,
) -> Result<BTreeMap<i64, (String, BTreeSet<MessageId>)>, ConversationStoreError> {
    read_inherited_occupancy(connection)?;
    let expected = count(connection, kind)?;
    let mut statement = connection
        .prepare_cached(if kind == "turn" {
            TURN_LOOKUP
        } else {
            RESPONSE_LOOKUP
        })
        .map_err(|error| storage(error.to_string()))?;
    let mut records = BTreeMap::new();
    for id in messages.iter().collect::<BTreeSet<_>>() {
        let row = statement
            .query_row([id.as_str()], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        let Some((member, turn, response, hash, ordinal, payload, payload_hash)) = row else {
            continue;
        };
        if member.as_deref() != Some(id.as_str())
            || hash.as_deref() != Some(membership_digest(id.as_str(), turn, response).as_str())
        {
            return Err(storage("missing or inconsistent inherited membership"));
        }
        let owner = if kind == "turn" { turn } else { response };
        match (owner, ordinal, payload, payload_hash) {
            (None, None, None, None) => {}
            (Some(owner), Some(ordinal), Some(payload), Some(hash))
                if owner == ordinal
                    && ordinal >= 0
                    && ordinal < expected
                    && hash == payload_digest(kind, ordinal, &payload) =>
            {
                records
                    .entry(ordinal)
                    .or_insert_with(|| (payload, BTreeSet::new()))
                    .1
                    .insert(id.clone());
            }
            _ => return Err(storage("missing or corrupt inherited provenance")),
        }
    }
    Ok(records)
}

pub(super) fn selected_turns(
    connection: &Connection,
    messages: &[MessageId],
) -> Result<Vec<TurnReadingProvenance>, ConversationStoreError> {
    selected(connection, messages, "turn")?
        .into_iter()
        .map(|(ordinal, (json, selected_members))| {
            let turn = validated_turn(connection, ordinal, &json)?;
            if !selected_members
                .iter()
                .all(|id| turn.process_message_ids.contains(id))
            {
                return Err(storage("selected inherited Turn does not own the message"));
            }
            Ok(turn)
        })
        .collect()
}
pub(super) fn selected_responses(
    connection: &Connection,
    messages: &[MessageId],
) -> Result<Vec<CompletedResponseProvenance>, ConversationStoreError> {
    selected(connection, messages, "response")?
        .into_iter()
        .map(|(ordinal, (json, selected_members))| {
            let response: CompletedResponseProvenance = decode(&json, "inherited response")?;
            verify_members(
                connection,
                "response",
                ordinal,
                &response.process_message_ids,
            )?;
            if !response
                .process_message_ids
                .contains(&response.closing_message_id)
                || !selected_members
                    .iter()
                    .all(|id| response.process_message_ids.contains(id))
            {
                return Err(storage("invalid selected inherited Response membership"));
            }
            Ok(response)
        })
        .collect()
}

fn all(connection: &Connection, kind: &str) -> Result<Vec<(i64, String)>, ConversationStoreError> {
    read_inherited_occupancy(connection)?;
    let expected = count(connection, kind)?;
    let sql = if kind == "turn" {
        "SELECT ordinal,payload,payload_digest FROM inherited_turn_provenance ORDER BY ordinal"
    } else {
        "SELECT ordinal,payload,payload_digest FROM inherited_response_provenance ORDER BY ordinal"
    };
    let rows = connection
        .prepare(sql)
        .map_err(|error| storage(error.to_string()))?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    if expected != seq_to_i64(rows.len() as u64)? {
        return Err(storage("missing inherited provenance records"));
    }
    rows.into_iter()
        .enumerate()
        .map(|(index, (ordinal, payload, hash))| {
            if ordinal != seq_to_i64(index as u64)?
                || hash != payload_digest(kind, ordinal, &payload)
            {
                return Err(storage("corrupt inherited provenance ordering or payload"));
            }
            Ok((ordinal, payload))
        })
        .collect()
}
pub(super) fn turns(
    connection: &Connection,
) -> Result<Vec<TurnReadingProvenance>, ConversationStoreError> {
    all(connection, "turn")?
        .into_iter()
        .map(|(ordinal, json)| validated_turn(connection, ordinal, &json))
        .collect()
}
pub(super) fn responses(
    connection: &Connection,
) -> Result<Vec<CompletedResponseProvenance>, ConversationStoreError> {
    all(connection, "response")?
        .into_iter()
        .map(|(ordinal, json)| {
            let response: CompletedResponseProvenance = decode(&json, "inherited response")?;
            if !response
                .process_message_ids
                .contains(&response.closing_message_id)
            {
                return Err(storage("invalid inherited Response anchor"));
            }
            verify_members(
                connection,
                "response",
                ordinal,
                &response.process_message_ids,
            )?;
            Ok(response)
        })
        .collect()
}

pub(super) fn turn_at(
    connection: &Connection,
    ordinal: i64,
) -> Result<TurnReadingProvenance, ConversationStoreError> {
    let expected = turn_count(connection)?;
    if ordinal < 0 || ordinal >= expected {
        return Err(storage("inherited Turn ordinal outside bootstrap"));
    }
    let (json, hash): (String, String) = connection
        .query_row(
            "SELECT payload,payload_digest FROM inherited_turn_provenance WHERE ordinal=?1",
            [ordinal],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| storage(error.to_string()))?;
    if hash != payload_digest("turn", ordinal, &json) {
        return Err(storage("corrupt inherited Turn payload"));
    }
    validated_turn(connection, ordinal, &json)
}

pub(super) fn verify_seed_membership(
    connection: &Connection,
    seed: &LineageSeed,
) -> Result<(), ConversationStoreError> {
    let inconsistent: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM inherited_message_membership m LEFT JOIN message_ledger l ON l.position=m.assistant_ledger_position AND l.message_id=m.assistant_message_id WHERE l.position IS NULL)",
        [], |row| row.get(0),
    ).map_err(|error| storage(error.to_string()))?;
    if inconsistent {
        return Err(storage("inherited membership canonical locator mismatch"));
    }
    let mut expected = BTreeMap::new();
    for message in seed.canonical() {
        if let MessageBlock::Assistant(message) = message {
            expected.insert(message.id.to_string(), (None, None));
        }
    }
    for (ordinal, turn) in seed.turns().iter().enumerate() {
        for id in &turn.process_message_ids {
            expected
                .get_mut(id.as_str())
                .ok_or_else(|| storage("unknown inherited Turn member"))?
                .0 = Some(seq_to_i64(ordinal as u64)?);
        }
    }
    for (ordinal, response) in seed.completed_responses().iter().enumerate() {
        for id in &response.process_message_ids {
            expected
                .get_mut(id.as_str())
                .ok_or_else(|| storage("unknown inherited Response member"))?
                .1 = Some(seq_to_i64(ordinal as u64)?);
        }
    }
    let actual=connection.prepare("SELECT assistant_message_id,turn_ordinal,response_ordinal,membership_digest FROM inherited_message_membership").map_err(|error|storage(error.to_string()))?.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<i64>>(1)?,row.get::<_,Option<i64>>(2)?,row.get::<_,String>(3)?))).map_err(|error|storage(error.to_string()))?.collect::<Result<Vec<_>,_>>().map_err(|error|storage(error.to_string()))?;
    if actual.len() != expected.len() {
        return Err(storage("missing inherited membership"));
    }
    for (id, turn, response, hash) in actual {
        if expected.get(&id) != Some(&(turn, response))
            || hash != membership_digest(&id, turn, response)
        {
            return Err(storage("mismatched inherited membership"));
        }
    }
    Ok(())
}

fn display_bytes(value: &impl serde::Serialize) -> Result<Vec<u8>, ConversationStoreError> {
    crate::runtime_client::checkpoint::bounded_bytes(
        value,
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
    .ok_or_else(|| storage("inherited native display header exceeds bound"))
}
fn response_header(
    connection: &Connection,
    ordinal: i64,
    response: &CompletedResponseProvenance,
) -> Result<crate::durable::observation::InheritedResponseHeader, ConversationStoreError> {
    let mut models = Vec::new();
    for (index, model) in response.models.iter().enumerate() {
        let encoded_bytes = super::content::index_structural_value(
            connection,
            "inherited_model",
            &ordinal.to_string(),
            u32::try_from(index).map_err(|_| storage("inherited model ordinal exhausted"))?,
            model,
        )?;
        if index == models.len()
            && index < crate::durable::observation::HEADER_PAGE_MAX
            && encoded_bytes <= 1024
            && models.iter().map(String::len).sum::<usize>()
                + usize::try_from(encoded_bytes).unwrap_or(usize::MAX)
                <= 12288
        {
            models.push(model.clone());
        }
    }
    let models_total = response.models.len() as u64;
    let models_next_offset = ((models.len() as u64) < models_total).then_some(models.len() as u64);
    Ok(crate::durable::observation::InheritedResponseHeader {
        ordinal: u64::try_from(ordinal).map_err(|_| storage("negative inherited ordinal"))?,
        response: crate::durable::observation::InheritedResponseSummary {
            process_message_ids: Vec::new(),
            closing_message_id: (response.closing_message_id.as_str().len() <= 256)
                .then(|| response.closing_message_id.clone()),
            closing_ledger_position: connection
                .query_row_cached(
                    "SELECT position FROM message_ledger WHERE message_id=?1",
                    [response.closing_message_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?,
            retry_message_id: response
                .retry_message_id
                .as_ref()
                .filter(|id| id.as_str().len() <= 256)
                .cloned(),
            retry_ledger_position: response
                .retry_message_id
                .as_ref()
                .map(|id| {
                    connection
                        .query_row_cached(
                            "SELECT position FROM message_ledger WHERE message_id=?1",
                            [id.as_str()],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(|error| storage(error.to_string()))
                })
                .transpose()?
                .flatten(),
            origin: crate::durable::observation::InheritedResponseOrigin {
                conversation_id: response.origin.conversation_id.clone(),
                attempt_id: (response.origin.attempt_id.as_str().len() <= 512)
                    .then(|| response.origin.attempt_id.clone()),
                attempt_id_source: if response.origin.attempt_id.as_str().len() > 512 {
                    Some(super::observation_identity::response_owner(
                        connection,
                        &response.origin.conversation_id,
                        &response.origin.attempt_id,
                    )?)
                } else {
                    None
                },
                closing_message_id: (response.origin.closing_message_id.as_str().len() <= 256)
                    .then(|| response.origin.closing_message_id.clone()),
            },
            completed_at: response.completed_at,
            usage: response.usage.clone(),
            timing: response.timing.clone(),
            models,
        },
        models_total,
        models_next_offset,
    })
}

#[cfg(test)]
type DisplayMembershipRow = (
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);
/// Exact immutable membership selects finite display layouts; it never parses
/// either the full canonical Message or the full lineage payload during reads.
#[cfg(test)]
pub(super) fn observation_native_oracle(
    connection: &Connection,
    messages: &[MessageId],
) -> Result<crate::durable::observation::InheritedObservation, ConversationStoreError> {
    if messages.len() > crate::durable::observation::HEADER_PAGE_MAX {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let mut turns: BTreeMap<i64, TurnReadingProvenance> = BTreeMap::new();
    let mut responses: BTreeMap<i64, crate::durable::observation::InheritedResponseHeader> =
        BTreeMap::new();
    let mut statement=connection.prepare_cached("SELECT m.turn_ordinal,m.response_ordinal,m.membership_digest,t.display_provenance,r.display_provenance FROM message_ledger l JOIN observation_headers h ON h.reference_kind='message' AND h.reference_id=l.message_id LEFT JOIN inherited_message_membership m ON m.assistant_message_id=l.message_id AND m.assistant_ledger_position=l.position LEFT JOIN inherited_turn_provenance t ON t.ordinal=m.turn_ordinal LEFT JOIN inherited_response_provenance r ON r.ordinal=m.response_ordinal WHERE l.message_id=?1 AND json_extract(h.header_json,'$.role')='assistant' AND l.position<=(SELECT message_count FROM bootstrap_identity WHERE id=1)").map_err(|error|storage(error.to_string()))?;
    for id in messages.iter().collect::<BTreeSet<_>>() {
        let row: Option<DisplayMembershipRow> = statement
            .query_row([id.as_str()], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        let Some((turn, response, hash, turn_bytes, response_bytes)) = row else {
            continue;
        };
        if hash.as_deref() != Some(membership_digest(id.as_str(), turn, response).as_str()) {
            return Err(storage("inconsistent inherited display membership"));
        }
        if let Some(ordinal) = turn {
            if let std::collections::btree_map::Entry::Vacant(entry) = turns.entry(ordinal) {
                let bytes =
                    turn_bytes.ok_or_else(|| storage("missing inherited Turn display header"))?;
                entry.insert(
                    serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?,
                );
            }
            turns
                .get_mut(&ordinal)
                .expect("inserted native Turn")
                .process_message_ids
                .push(id.clone());
        }
        if let Some(ordinal) = response {
            if let std::collections::btree_map::Entry::Vacant(entry) = responses.entry(ordinal) {
                let bytes = response_bytes
                    .ok_or_else(|| storage("missing inherited response display header"))?;
                entry.insert(
                    serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?,
                );
            }
            responses
                .get_mut(&ordinal)
                .expect("inserted native response")
                .response
                .process_message_ids
                .push(id.clone());
        }
    }
    Ok(crate::durable::observation::InheritedObservation {
        turns: turns.into_values().collect(),
        responses: responses.into_values().collect(),
    })
}
fn observation_response(
    connection: &Connection,
    ordinal: u64,
) -> Result<crate::durable::observation::InheritedResponseHeader, ConversationStoreError> {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT display_provenance FROM inherited_response_provenance WHERE ordinal=?1",
            [ordinal],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?
        .ok_or_else(|| storage("unknown inherited response"))?;
    serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))
}
pub(super) fn observation_models(
    connection: &Connection,
    ordinal: u64,
    offset: u64,
    limit: usize,
) -> Result<crate::durable::observation::InheritedModelPage, ConversationStoreError> {
    use crate::durable::observation::{HEADER_PAGE_MAX, InheritedModelName, InheritedModelPage};
    if limit == 0 || limit > HEADER_PAGE_MAX {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let header = observation_response(connection, ordinal)?;
    if offset > header.models_total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let rows=connection.prepare_cached("SELECT block_index,readable_end FROM content_frontiers WHERE source_kind='inherited_model' AND source_id=?1 AND representation='canonical_json_v1' AND revision=1 AND block_index>=?2 AND block_index<?3 ORDER BY block_index LIMIT ?4").map_err(|error|storage(error.to_string()))?.query_map(params![ordinal.to_string(),offset,header.models_total,limit],|row|Ok((row.get::<_,u64>(0)?,row.get::<_,u64>(1)?))).map_err(|error|storage(error.to_string()))?.collect::<Result<Vec<_>,_>>().map_err(|error|storage(error.to_string()))?;
    let mut names = Vec::with_capacity(rows.len());
    for (index, encoded_bytes) in rows {
        let value = if encoded_bytes <= 1024 {
            Some(
                serde_json::from_slice(
                    &observation_model_read(connection, ordinal, index, 0, 1024)?.bytes,
                )
                .map_err(|error| storage(error.to_string()))?,
            )
        } else {
            None
        };
        names.push(InheritedModelName {
            ordinal: index,
            encoded_bytes,
            value,
        });
    }
    let next = offset
        .checked_add(names.len() as u64)
        .ok_or(ConversationStoreError::InvalidContentRange)?;
    if next == offset && next < header.models_total {
        return Err(storage("inherited model page cannot advance"));
    }
    Ok(InheritedModelPage {
        total: header.models_total,
        offset,
        names,
        next_offset: (next < header.models_total).then_some(next),
    })
}
pub(super) fn observation_model_read(
    connection: &Connection,
    ordinal: u64,
    model: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    let header = observation_response(connection, ordinal)?;
    if model >= header.models_total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    super::content::structural_field_read(
        connection,
        "inherited_model",
        &ordinal.to_string(),
        u32::try_from(model).map_err(|_| ConversationStoreError::InvalidContentRange)?,
        offset,
        max_bytes,
    )
}
