//! Fixed read witnesses over native paired identity and membership, never authority.
use super::{Connection, ConversationStoreError, MessageId, params, storage};
use sha2::{Digest, Sha256};
#[cfg(test)]
thread_local! {
    static READ_WORK: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) };
}
#[cfg(test)]
pub(super) fn read_work() -> (usize, usize) {
    READ_WORK.with(|work| work.replace((0, 0)))
}
pub(super) const SELECTED_EVIDENCE: &str = "SELECT CASE WHEN typeof(l.observation_identity_digest)='blob' AND length(l.observation_identity_digest)=32 THEN l.observation_identity_digest END,CASE WHEN typeof(m.observation_identity_digest)='blob' AND length(m.observation_identity_digest)=32 THEN m.observation_identity_digest END,CASE WHEN typeof(m.observation_membership_witness)='blob' AND length(m.observation_membership_witness)=32 THEN m.observation_membership_witness END,m.turn_ordinal,m.response_ordinal,CASE WHEN typeof(m.membership_digest)='text' AND length(CAST(m.membership_digest AS BLOB))=64 THEN m.membership_digest END FROM inherited_message_membership m JOIN message_ledger l ON l.position=m.assistant_ledger_position WHERE m.assistant_ledger_position=?1";
fn identity(id: &MessageId) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"rustx-canonical-identity-read-v1\0");
    hash.update((id.as_str().len() as u64).to_le_bytes());
    hash.update(id.as_str().as_bytes());
    hash.finalize().into()
}
fn witness(
    position: u64,
    id: &[u8],
    turn: Option<i64>,
    response: Option<i64>,
    original: &str,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"rustx-inherited-membership-read-v1\0");
    hash.update(position.to_le_bytes());
    hash.update(id);
    for ordinal in [turn, response] {
        hash.update([u8::from(ordinal.is_some())]);
        hash.update(ordinal.unwrap_or(0).to_le_bytes());
    }
    hash.update((original.len() as u64).to_le_bytes());
    hash.update(original.as_bytes());
    hash.finalize().into()
}
pub(super) fn canonical(
    connection: &Connection,
    position: u64,
    id: &MessageId,
) -> Result<(), ConversationStoreError> {
    connection
        .execute(
            "UPDATE message_ledger SET observation_identity_digest=?2 WHERE position=?1",
            params![position, identity(id).as_slice()],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
pub(super) fn member(
    connection: &Connection,
    position: u64,
    id: &MessageId,
    turn: Option<i64>,
    response: Option<i64>,
    original: &str,
) -> Result<(), ConversationStoreError> {
    let digest = identity(id);
    let value = witness(position, &digest, turn, response, original);
    connection.execute("UPDATE inherited_message_membership SET observation_identity_digest=?2,observation_membership_witness=?3 WHERE assistant_ledger_position=?1",params![position,digest.as_slice(),value.as_slice()]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}
pub(super) fn validate(
    connection: &Connection,
    position: u64,
) -> Result<(), ConversationStoreError> {
    type MembershipRow = (
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<i64>,
        Option<i64>,
        Option<String>,
    );
    #[cfg(test)]
    READ_WORK.with(|work| work.set((0, 0)));
    let (native, member, value, turn, response, original): MembershipRow = connection
        .query_row(SELECTED_EVIDENCE, [position], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .map_err(|_| storage("native inherited membership evidence unavailable"))?;
    #[cfg(test)]
    READ_WORK.with(|work| {
        work.set((
            1,
            native.as_ref().map_or(0, Vec::len)
                + member.as_ref().map_or(0, Vec::len)
                + value.as_ref().map_or(0, Vec::len)
                + original.as_ref().map_or(0, String::len),
        ));
    });
    let native = native.ok_or_else(|| storage("native canonical identity witness unavailable"))?;
    let member = member.ok_or_else(|| storage("native inherited identity witness unavailable"))?;
    let value = value.ok_or_else(|| storage("native inherited membership witness unavailable"))?;
    let original =
        original.ok_or_else(|| storage("native inherited membership digest unavailable"))?;
    if native.len() != 32
        || original.len() != 64
        || !original.bytes().all(|byte| byte.is_ascii_hexdigit())
        || native != member
        || value != witness(position, &member, turn, response, &original)
    {
        return Err(storage("native inherited membership read witness corrupt"));
    }
    Ok(())
}
