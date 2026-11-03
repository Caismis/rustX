//! A checkpoint plus at most fifteen independent contributions, using the two
//! existing aggregate-head masks. No new table, index or mutable current row.
use super::contribution::Contribution;
use super::{
    Connection, ConversationStoreError, OptionalExtension, StatisticsLayout, codec, decode, encode,
    fields, normalized_fold, params, shared, storage,
};
use sha2::{Digest as _, Sha256};

pub(super) const STRIDE: u8 = 16;
type StoredRecord = (u64, u64, u8, super::contribution::Stored, Option<Vec<u8>>);
pub(super) struct Record {
    pub(super) previous: u64,
    pub(super) anchor: u64,
    pub(super) distance: u8,
    pub(super) contribution: Contribution,
    digest: Option<Vec<u8>>,
}
pub(super) struct Frame {
    pub(super) sequence: u64,
    pub(super) anchor: u64,
    pub(super) distance: u8,
    pub(super) state: StatisticsLayout,
}
impl Record {
    pub(super) fn capture(
        previous: &Frame,
        sequence: u64,
        contribution: Contribution,
        state: &StatisticsLayout,
    ) -> Result<(Self, Option<Vec<u8>>), ConversationStoreError> {
        let checkpoint = if contribution.terminal() || previous.distance >= STRIDE - 1 {
            Some(encode(state)?)
        } else {
            codec::admit(
                &fields(state, shared(state)),
                crate::durable::observation::HEADER_ENCODED_MAX,
            )?;
            None
        };
        let mut record = Self {
            previous: previous.sequence,
            anchor: if checkpoint.is_some() {
                sequence
            } else {
                previous.anchor
            },
            distance: if checkpoint.is_some() {
                0
            } else {
                previous.distance + 1
            },
            contribution,
            digest: None,
        };
        record.digest = checkpoint
            .as_ref()
            .map(|bytes| record.checkpoint_digest(bytes))
            .transpose()?
            .map(|digest| digest.to_vec());
        Ok((record, checkpoint))
    }
    pub(super) fn encode(&self) -> Result<Vec<u8>, ConversationStoreError> {
        #[cfg(test)]
        super::WRITE_WORK.with(|work| {
            let (full, facts) = work.get();
            work.set((full, facts + 1));
        });
        let fields = (
            self.previous,
            self.anchor,
            self.distance,
            self.contribution.fields(),
            &self.digest,
        );
        codec::encode(
            &fields,
            &fields,
            crate::durable::observation::HEADER_ENCODED_MAX,
        )
    }
    fn decode(sequence: u64, bytes: &[u8]) -> Result<Self, ConversationStoreError> {
        let (previous, anchor, distance, contribution, digest): StoredRecord =
            codec::decode_json_tuple(bytes, crate::durable::observation::HEADER_ENCODED_MAX)?;
        if previous >= sequence
            || anchor > sequence
            || distance >= STRIDE
            || (distance == 0) != (anchor == sequence)
            || (distance == 0) != digest.is_some()
            || digest.as_ref().is_some_and(|digest| digest.len() != 32)
        {
            return Err(storage("statistics record position invalid"));
        }
        Ok(Self {
            previous,
            anchor,
            distance,
            contribution: Contribution::restore(contribution, sequence)?,
            digest,
        })
    }
    fn checkpoint_digest(&self, bytes: &[u8]) -> Result<[u8; 32], ConversationStoreError> {
        // Bind the materialization to its own independent fact as well. A valid
        // rewrite of either cell cannot silently seat a different fold.
        let binding = codec::encode_admitted(&(
            self.previous,
            self.anchor,
            self.distance,
            self.contribution.fields(),
        ))?;
        let mut digest = Sha256::new();
        digest.update(bytes);
        digest.update(&binding);
        Ok(digest.finalize().into())
    }
    fn checkpoint(&self, bytes: Option<&[u8]>) -> Result<StatisticsLayout, ConversationStoreError> {
        let bytes = bytes.ok_or_else(|| storage("statistics checkpoint unavailable"))?;
        if self.distance != 0
            || self.digest.as_deref() != Some(self.checkpoint_digest(bytes)?.as_slice())
        {
            return Err(storage("statistics checkpoint differs from its fact"));
        }
        decode(bytes)
    }
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
        .map_err(|error| storage(error.to_string()))?
}
fn hole(connection: &Connection, anchor: u64, through: u64) -> Result<(), ConversationStoreError> {
    let missing:bool=connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_aggregate_holes_idx WHERE observation_aggregate_holes>0 AND observation_aggregate_holes IN (4,5,6,7) AND sequence>?1 AND sequence<=?2)")
        .and_then(|mut statement|statement.query_row(params![anchor,through],|row|row.get(0))).map_err(|error|storage(error.to_string()))?;
    if missing {
        return Err(storage("native statistics contribution unavailable"));
    }
    Ok(())
}
fn read_record(
    connection: &Connection,
    sequence: u64,
) -> Result<(Record, Option<Vec<u8>>), ConversationStoreError> {
    connection.prepare_cached("SELECT CASE WHEN typeof(observation_statistics)='blob' AND length(observation_statistics)<=49152 THEN observation_statistics ELSE 0 END,CASE WHEN observation_statistics_checkpoint IS NULL THEN NULL WHEN typeof(observation_statistics_checkpoint)='blob' AND length(observation_statistics_checkpoint)<=49152 THEN observation_statistics_checkpoint ELSE 0 END FROM events WHERE sequence=?1")
        .and_then(|mut statement|statement.query_row([sequence],|row|{
            let rusqlite::types::ValueRef::Blob(bytes)=row.get_ref(0)? else{return Ok(Err(storage("statistics fact type invalid")))};
            let checkpoint:Option<Vec<u8>>=row.get(1)?;
            #[cfg(test)] super::record_read(1,bytes.len()+checkpoint.as_ref().map_or(0,Vec::len));
            Ok(Record::decode(sequence,bytes).map(|record|(record,checkpoint)))
        })).map_err(|error|storage(error.to_string()))?
}
type StoredHead = (u64, Record, Option<Vec<u8>>);
fn latest(
    connection: &Connection,
    through: u64,
) -> Result<Option<StoredHead>, ConversationStoreError> {
    connection
        .prepare_cached(super::super::event_heads::STATISTICS)
        .and_then(|mut statement| {
            statement.query_row([through], |row| {
                let sequence: u64 = row.get(0)?;
                let rusqlite::types::ValueRef::Blob(bytes) = row.get_ref(1)? else {
                    return Ok(Err(storage("statistics fact type invalid")));
                };
                let checkpoint: Option<Vec<u8>> = row.get(2)?;
                #[cfg(test)]
                super::record_read(1, bytes.len() + checkpoint.as_ref().map_or(0, Vec::len));
                Ok(Record::decode(sequence, bytes).map(|record| (sequence, record, checkpoint)))
            })
        })
        .optional()
        .map_err(|error| storage(error.to_string()))?
        .transpose()
}
pub(super) fn layout(
    connection: &Connection,
    through: u64,
) -> Result<Frame, ConversationStoreError> {
    let latest = latest(connection, through)?;
    let Some((sequence, last, checkpoint)) = latest else {
        hole(connection, 0, through)?;
        return Ok(Frame {
            sequence: 0,
            anchor: 0,
            distance: 0,
            state: bootstrap(connection)?,
        });
    };
    let mut frame = if last.distance == 0 {
        let state = last.checkpoint(checkpoint.as_deref())?;
        Frame {
            sequence,
            anchor: sequence,
            distance: 0,
            state,
        }
    } else {
        if checkpoint.is_some() {
            return Err(storage("statistics delta contains a checkpoint"));
        }
        let mut frame = if last.anchor == 0 {
            Frame {
                sequence: 0,
                anchor: 0,
                distance: 0,
                state: bootstrap(connection)?,
            }
        } else {
            let (record, checkpoint) = read_record(connection, last.anchor)?;
            let state = record.checkpoint(checkpoint.as_deref())?;
            Frame {
                sequence: last.anchor,
                anchor: last.anchor,
                distance: 0,
                state,
            }
        };
        // Each mask independently seeks and stops after STRIDE rows, before the
        // bounded merge. Even a forged ancient anchor cannot sort all history.
        let mut suffix = Vec::new();
        for mask in [2, 3] {
            let mut statement=connection.prepare_cached("SELECT sequence,CASE WHEN typeof(observation_statistics)='blob' AND length(observation_statistics)<=49152 THEN observation_statistics ELSE 0 END,observation_statistics_checkpoint IS NOT NULL FROM events INDEXED BY events_aggregate_heads_idx WHERE observation_aggregate_heads>0 AND observation_aggregate_heads=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT 16")
                .map_err(|error|storage(error.to_string()))?;
            let mut rows = statement
                .query(params![mask, last.anchor, sequence])
                .map_err(|error| storage(error.to_string()))?;
            while let Some(row) = rows.next().map_err(|error| storage(error.to_string()))? {
                let position: u64 = row.get(0).map_err(|error| storage(error.to_string()))?;
                let rusqlite::types::ValueRef::Blob(bytes) =
                    row.get_ref(1).map_err(|error| storage(error.to_string()))?
                else {
                    return Err(storage("statistics suffix fact type invalid"));
                };
                let record = Record::decode(position, bytes)?;
                let checkpoint: bool = row.get(2).map_err(|error| storage(error.to_string()))?;
                if checkpoint {
                    return Err(storage("statistics suffix contains a checkpoint"));
                }
                #[cfg(test)]
                super::record_read(1, bytes.len());
                suffix.push((position, record));
            }
        }
        suffix.sort_unstable_by_key(|(sequence, _)| *sequence);
        if suffix.len() != usize::from(last.distance) {
            return Err(storage("statistics contribution range differs"));
        }
        for (position, record) in suffix {
            if record.previous != frame.sequence
                || record.anchor != frame.anchor
                || record.distance != frame.distance + 1
            {
                return Err(storage("statistics contribution chain differs"));
            }
            record.contribution.apply(position, &mut frame.state)?;
            frame.sequence = position;
            frame.distance = record.distance;
        }
        if frame.sequence != sequence {
            return Err(storage("statistics contribution head differs"));
        }
        frame
    };
    hole(connection, frame.anchor, through)?;
    frame.state = normalized_fold(frame.state);
    codec::admit(
        &fields(&frame.state, shared(&frame.state)),
        crate::durable::observation::HEADER_ENCODED_MAX,
    )?;
    Ok(frame)
}
