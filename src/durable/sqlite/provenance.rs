//! Immutable bootstrap records and exact canonical Assistant membership.
use super::{
    Connection, ConversationStoreError, Digest, LineageSeed, MessageBlock, MessageId,
    OptionalExtension, Sha256, decode, encode, params, read_inherited_occupancy, seq_to_i64,
    storage,
};
use crate::durable::reading::TurnReadingProvenance;
use crate::durable::response::CompletedResponseProvenance;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const TURN_LOOKUP: &str = "SELECT m.assistant_message_id,m.turn_ordinal,m.response_ordinal,m.membership_digest,p.ordinal,p.payload,p.payload_digest FROM message_ledger l LEFT JOIN inherited_message_membership m ON m.assistant_message_id=l.message_id LEFT JOIN inherited_turn_provenance p ON p.ordinal=m.turn_ordinal WHERE l.message_id=?1 AND l.position<=(SELECT message_count FROM bootstrap_identity WHERE id=1) AND json_extract(l.message_json,'$.role')='assistant'";
pub(super) const RESPONSE_LOOKUP: &str = "SELECT m.assistant_message_id,m.turn_ordinal,m.response_ordinal,m.membership_digest,p.ordinal,p.payload,p.payload_digest FROM message_ledger l LEFT JOIN inherited_message_membership m ON m.assistant_message_id=l.message_id LEFT JOIN inherited_response_provenance p ON p.ordinal=m.response_ordinal WHERE l.message_id=?1 AND l.position<=(SELECT message_count FROM bootstrap_identity WHERE id=1) AND json_extract(l.message_json,'$.role')='assistant'";

fn digest(value: &str) -> String {
    use std::fmt::Write as _;
    let mut digest = String::new();
    for byte in Sha256::digest(value.as_bytes()) {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}
fn membership_digest(id: &str, turn: Option<i64>, response: Option<i64>) -> String {
    digest(&format!(
        "rustx-inherited-member\n{id}\n{turn:?}\n{response:?}"
    ))
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
        connection.execute("INSERT INTO inherited_turn_provenance(ordinal,payload,payload_digest) VALUES(?1,?2,?3)", params![ordinal,payload,payload_digest("turn",ordinal,&payload)]).map_err(|error| storage(error.to_string()))?;
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
        connection.execute("INSERT INTO inherited_response_provenance(ordinal,payload,payload_digest) VALUES(?1,?2,?3)", params![ordinal,payload,payload_digest("response",ordinal,&payload)]).map_err(|error| storage(error.to_string()))?;
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
        connection.execute("INSERT INTO inherited_message_membership(assistant_message_id,turn_ordinal,response_ordinal,membership_digest) VALUES(?1,?2,?3,?4)", params![id.as_str(),turn,response,membership_digest(id.as_str(),turn,response)]).map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}

fn verify_members(
    connection: &Connection,
    kind: &str,
    ordinal: i64,
    members: &[MessageId],
) -> Result<(), ConversationStoreError> {
    let mut seen = BTreeSet::new();
    let mut statement = connection.prepare_cached("SELECT m.turn_ordinal,m.response_ordinal,m.membership_digest FROM inherited_message_membership m JOIN message_ledger l ON l.message_id=m.assistant_message_id WHERE m.assistant_message_id=?1 AND json_extract(l.message_json,'$.role')='assistant' AND l.position<=(SELECT message_count FROM bootstrap_identity WHERE id=1)").map_err(|error| storage(error.to_string()))?;
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
    Ok(())
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
            let turn: TurnReadingProvenance = decode(&json, "inherited turn")?;
            verify_members(connection, "turn", ordinal, &turn.process_message_ids)?;
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
        .map(|(ordinal, json)| {
            let turn: TurnReadingProvenance = decode(&json, "inherited turn")?;
            verify_members(connection, "turn", ordinal, &turn.process_message_ids)?;
            Ok(turn)
        })
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
    read_inherited_occupancy(connection)?;
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
    let turn: TurnReadingProvenance = decode(&json, "inherited Turn")?;
    verify_members(connection, "turn", ordinal, &turn.process_message_ids)?;
    Ok(turn)
}

pub(super) fn verify_seed_membership(
    connection: &Connection,
    seed: &LineageSeed,
) -> Result<(), ConversationStoreError> {
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
