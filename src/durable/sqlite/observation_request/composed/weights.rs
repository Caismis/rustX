//! Exact independent decimal widths for native J and rank coordinates.
use super::{ConversationStoreError, storage};
pub(super) const SEQUENCE_WIDTHS: usize = 20;
pub(super) const RANK_WIDTHS: usize = 4;
pub(super) const CLASSES: usize = SEQUENCE_WIDTHS * RANK_WIDTHS;
pub(super) fn digits(value: u64) -> u32 {
    value.checked_ilog10().map_or(1, |value| value + 1)
}
pub(super) fn class(sequence: u64, rank: u64) -> Result<usize, ConversationStoreError> {
    if sequence == 0 || rank >= super::manifest::MAX_ITEMS {
        return Err(storage("request occurrence exceeds native bound"));
    }
    let sequence =
        usize::try_from(digits(sequence) - 1).map_err(|_| storage("native J width invalid"))?;
    let rank =
        usize::try_from(digits(rank) - 1).map_err(|_| storage("native rank width invalid"))?;
    if sequence >= SEQUENCE_WIDTHS || rank >= RANK_WIDTHS {
        return Err(storage("native occurrence width invalid"));
    }
    Ok(sequence * RANK_WIDTHS + rank)
}
pub(super) fn delta(class: usize) -> Result<u64, ConversationStoreError> {
    if class >= CLASSES {
        return Err(storage("native occurrence class invalid"));
    }
    u64::try_from(class / RANK_WIDTHS + class % RANK_WIDTHS)
        .map_err(|_| storage("native occurrence width invalid"))
}
pub(super) fn representative(class: usize) -> Result<(u64, u64), ConversationStoreError> {
    if class >= CLASSES {
        return Err(storage("native occurrence class invalid"));
    }
    let sequence = 10_u64
        .checked_pow(
            u32::try_from(class / RANK_WIDTHS).map_err(|_| storage("native J width invalid"))?,
        )
        .ok_or_else(|| storage("native J width invalid"))?;
    let rank = if class.is_multiple_of(RANK_WIDTHS) {
        0
    } else {
        10_u64
            .checked_pow(
                u32::try_from(class % RANK_WIDTHS)
                    .map_err(|_| storage("native rank width invalid"))?,
            )
            .ok_or_else(|| storage("native rank width invalid"))?
    };
    Ok((sequence, rank))
}
