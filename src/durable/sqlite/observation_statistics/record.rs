//! One self-contained full statistics record per relevant event. Facts and
//! state share one bounded cell; no history chain or separate checkpoint column.
use super::contribution::Contribution;
use super::{
    Connection, ConversationStoreError, OptionalExtension, StatisticsLayout, codec, decode, encode,
    fields, normalized_fold, params, shared, storage,
};
use sha2::{Digest as _, Sha256};

const PREFIX: usize = 36;
pub(in crate::durable::sqlite) const MAX_BYTES: usize = 2 * codec::MAX_BYTES + PREFIX;
pub(super) struct Frame {
    pub(super) sequence: u64,
    pub(super) state: StatisticsLayout,
}

/// The two component tuples keep their existing independent JSON admission.
/// The fixed prefix is a binding digest plus the fact length. Both are always
/// present: there is only one record shape, not checkpoint/delta variants.
pub(super) fn capture(
    sequence: u64,
    contribution: &Contribution,
    state: &StatisticsLayout,
) -> Result<Vec<u8>, ConversationStoreError> {
    let fact = codec::encode(
        &(sequence, contribution.fields()),
        &(sequence, contribution.fields()),
        crate::durable::observation::HEADER_ENCODED_MAX,
    )?;
    let state = encode(state)?;
    #[cfg(test)]
    super::WRITE_WORK.with(|work| {
        let (full, records) = work.get();
        work.set((full, records + 1));
    });
    Ok(bind(sequence, &fact, &state))
}
fn bind(sequence: u64, fact: &[u8], state: &[u8]) -> Vec<u8> {
    let mut record = Vec::with_capacity(PREFIX + fact.len() + state.len());
    record.resize(32, 0);
    record.extend_from_slice(
        &u32::try_from(fact.len())
            .expect("bounded statistics fact")
            .to_le_bytes(),
    );
    record.extend_from_slice(fact);
    record.extend_from_slice(state);
    let digest = binding(sequence, &record[32..]);
    record[..32].copy_from_slice(&digest);
    record
}

fn binding(sequence: u64, payload: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"rustx-full-statistics-record-v1\0");
    digest.update(sequence.to_be_bytes());
    digest.update(payload);
    digest.finalize().into()
}
pub(super) fn parts(sequence: u64, bytes: &[u8]) -> Result<(&[u8], &[u8]), ConversationStoreError> {
    if !(PREFIX + 2..=MAX_BYTES).contains(&bytes.len())
        || bytes[..32] != binding(sequence, &bytes[32..])
    {
        return Err(storage("statistics record differs from its fact binding"));
    }
    let fact_length =
        u32::from_le_bytes(bytes[32..PREFIX].try_into().expect("checked record prefix")) as usize;
    if fact_length == 0 || fact_length > codec::MAX_BYTES {
        return Err(storage("statistics fact length invalid"));
    }
    let end = PREFIX
        .checked_add(fact_length)
        .filter(|end| *end < bytes.len())
        .ok_or_else(|| storage("statistics record length invalid"))?;
    let state = &bytes[end..];
    if state.len() > codec::MAX_BYTES {
        return Err(storage("statistics state length invalid"));
    }
    Ok((&bytes[PREFIX..end], state))
}
fn restore(sequence: u64, bytes: &[u8]) -> Result<StatisticsLayout, ConversationStoreError> {
    let (fact, state) = parts(sequence, bytes)?;
    let (position, contribution): (u64, super::contribution::Stored) =
        codec::decode_json_tuple(fact, crate::durable::observation::HEADER_ENCODED_MAX)?;
    if position != sequence {
        return Err(storage("statistics fact position invalid"));
    }
    // Keep independent fact validation even though public history needs only
    // this record's full state. No native event body is a recovery fallback.
    Contribution::restore(contribution, sequence)?;
    let state = normalized_fold(decode(state)?);
    codec::admit(
        &fields(&state, shared(&state)),
        crate::durable::observation::HEADER_ENCODED_MAX,
    )?;
    Ok(state)
}
fn bootstrap(connection: &Connection) -> Result<StatisticsLayout, ConversationStoreError> {
    connection
        .prepare_cached(
            "SELECT CASE WHEN observation_statistics IS NULL THEN NULL WHEN typeof(observation_statistics)='blob' AND length(observation_statistics)<=49152 THEN observation_statistics ELSE 0 END,bootstrap_initialized FROM rustx_store WHERE id=1",
        )
        .and_then(|mut statement| {
            statement.query_row([], |row| {
                let initialized: bool = row.get(1)?;
                Ok(match row.get_ref(0)? {
                    rusqlite::types::ValueRef::Blob(bytes) => {
                        #[cfg(test)]
                        super::record_read(1, bytes.len());
                        decode(bytes)
                    }
                    rusqlite::types::ValueRef::Null if !initialized => {
                        Ok(StatisticsLayout::default())
                    }
                    _ => Err(storage("native statistics bootstrap header unavailable")),
                })
            })
        })
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?
}
pub(super) fn layout(
    connection: &Connection,
    through: u64,
) -> Result<Frame, ConversationStoreError> {
    let latest = connection
        .prepare_cached(super::super::event_heads::STATISTICS)
        .and_then(|mut statement| {
            statement.query_row([through], |row| {
                let sequence: u64 = row.get(0)?;
                let state = match row.get_ref(1)? {
                    rusqlite::types::ValueRef::Blob(bytes) => {
                        #[cfg(test)]
                        super::record_read(1, bytes.len());
                        restore(sequence, bytes)
                    }
                    _ => Err(storage("statistics record type invalid")),
                };
                Ok(state.map(|state| Frame { sequence, state }))
            })
        })
        .optional()
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?
        .transpose()?;
    let sequence = latest.as_ref().map_or(0, |frame| frame.sequence);
    // A newer independent full record survives damage before its own cut.
    // Missing relevant facts after the selected record still fail closed.
    let missing: bool = connection.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_aggregate_holes_idx WHERE observation_aggregate_holes>0 AND observation_aggregate_holes IN (4,5,6,7) AND sequence>?1 AND sequence<=?2)",
    ).and_then(|mut statement| statement.query_row(params![sequence, through], |row| row.get(0)))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    if missing {
        return Err(storage("native statistics record unavailable"));
    }
    match latest {
        Some(frame) => Ok(frame),
        None => Ok(Frame {
            sequence: 0,
            state: bootstrap(connection)?,
        }),
    }
}

#[cfg(test)]
pub(super) fn test_record(sequence: u64, state: &[u8]) -> Vec<u8> {
    let fact = codec::encode_admitted(&(
        sequence,
        (
            0_u8,
            (0_i64, 0_u32),
            None::<(Option<crate::runtime::identity::AttemptId>, Option<u64>)>,
            0_u8,
            false,
            None::<crate::model::ModelUsage>,
            None::<crate::model::generation_evidence::GenerationEvidence>,
            None::<u64>,
        ),
    ))
    .unwrap();
    bind(sequence, &fact, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_record_rejects_every_truncation_row_move_and_unbound_fact_or_state() {
        let state = encode(&StatisticsLayout::default()).unwrap();
        let bytes = test_record(7, &state);
        assert!(restore(7, &bytes).is_ok());
        assert!(restore(8, &bytes).is_err());
        for end in 0..bytes.len() {
            assert!(restore(7, &bytes[..end]).is_err(), "{end}");
        }
        for at in [0, 32, 36, bytes.len() - 1] {
            let mut changed = bytes.clone();
            changed[at] ^= 1;
            assert!(restore(7, &changed).is_err(), "{at}");
        }
    }
    #[test]
    fn full_record_enforces_component_eof_and_bounds_after_valid_binding() {
        let state = encode(&StatisticsLayout::default()).unwrap();
        let bytes = test_record(7, &state);
        let (fact, _) = parts(7, &bytes).unwrap();
        let mut trailing = state;
        trailing.push(0);
        assert!(restore(7, &bind(7, fact, &trailing)).is_err());
        let oversized = vec![0; codec::MAX_BYTES + 1];
        assert!(restore(7, &bind(7, fact, &oversized)).is_err());
        assert!(restore(7, &bind(7, &oversized, &[])).is_err());
        let mut bad_length = bytes.clone();
        bad_length[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        let digest = binding(7, &bad_length[32..]);
        bad_length[..32].copy_from_slice(&digest);
        assert!(restore(7, &bad_length).is_err());
    }
}
