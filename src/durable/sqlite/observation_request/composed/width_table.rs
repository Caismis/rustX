//! Exact occurrence lengths without repeating the decimal-width contribution.
//!
//! Every message contributes one J and one rank coordinate. For a subtree with
//! `count` messages that contribution is `count * weights::delta(class)`.
//! Only changes to the remaining body length need storage; admission boundaries
//! may change that body, so all eighty classes remain independently represented.
use super::{ConversationStoreError, SAFE, add, manifest, storage, weights};
use serde::{Deserialize, Serialize, de};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WidthTable {
    count: u64,
    #[serde(deserialize_with = "runs")]
    runs: Vec<(u8, u64)>,
}

fn runs<'de, D: de::Deserializer<'de>>(decoder: D) -> Result<Vec<(u8, u64)>, D::Error> {
    super::record::sequence::<_, _, { weights::CLASSES }>(decoder)
}

impl WidthTable {
    pub(super) fn leaf(
        bases: impl IntoIterator<Item = u64>,
    ) -> Result<Self, ConversationStoreError> {
        let mut table = Self {
            count: 1,
            runs: Vec::new(),
        };
        let mut classes = 0;
        for (class, base) in bases.into_iter().enumerate() {
            if class >= weights::CLASSES {
                return Err(storage("Source width classes exceed bound"));
            }
            table.push(
                u8::try_from(class).map_err(|_| storage("Source width class invalid"))?,
                base,
            );
            classes += 1;
        }
        if classes != weights::CLASSES {
            return Err(storage("Source width classes incomplete"));
        }
        table.validate(1)?;
        Ok(table)
    }

    fn push(&mut self, class: u8, base: u64) {
        if self
            .runs
            .last()
            .is_none_or(|(_, previous)| *previous != base)
        {
            self.runs.push((class, base));
        }
    }

    pub(super) fn join(left: &Self, right: &Self) -> Result<Self, ConversationStoreError> {
        let count = add(left.count, right.count)?;
        let mut joined = Self {
            count,
            runs: Vec::new(),
        };
        let (mut a, mut b) = (0, 0);
        loop {
            let (a_class, a_base) = left.runs[a];
            let (b_class, b_base) = right.runs[b];
            joined.push(a_class.max(b_class), add(add(a_base, b_base)?, 1)?);
            let next_a = left.runs.get(a + 1).map(|(class, _)| *class);
            let next_b = right.runs.get(b + 1).map(|(class, _)| *class);
            match (next_a, next_b) {
                (None, None) => break,
                (Some(_), None) => a += 1,
                (None, Some(_)) => b += 1,
                (Some(x), Some(y)) => {
                    a += usize::from(x <= y);
                    b += usize::from(y <= x);
                }
            }
        }
        joined.validate(count)?;
        Ok(joined)
    }

    pub(super) fn validate(&self, count: u64) -> Result<(), ConversationStoreError> {
        if self.count != count
            || count == 0
            || count > manifest::MAX_ITEMS
            || self.runs.first().is_none_or(|(class, _)| *class != 0)
            || self.runs.len() > weights::CLASSES
            || self
                .runs
                .iter()
                .any(|(class, base)| usize::from(*class) >= weights::CLASSES || *base > SAFE)
            || self
                .runs
                .windows(2)
                .any(|pair| pair[0].0 >= pair[1].0 || pair[0].1 == pair[1].1)
        {
            return Err(storage("Source width geometry differs"));
        }
        // Rank width resets at each J width. The maximum in a run is either
        // its final class or the preceding complete J group's last class.
        for (index, &(start, base)) in self.runs.iter().enumerate() {
            let end = self
                .runs
                .get(index + 1)
                .map_or(weights::CLASSES, |(class, _)| usize::from(*class))
                - 1;
            let mut maximum = weights::delta(end)?;
            if let Some(previous) =
                (end / weights::RANK_WIDTHS * weights::RANK_WIDTHS).checked_sub(1)
                && previous >= usize::from(start)
            {
                maximum = maximum.max(weights::delta(previous)?);
            }
            add(
                base,
                count
                    .checked_mul(maximum)
                    .ok_or_else(|| storage("Source width exceeds bound"))?,
            )?;
        }
        Ok(())
    }

    pub(super) fn length(&self, class: usize) -> Result<u64, ConversationStoreError> {
        let extra = self
            .count
            .checked_mul(weights::delta(class)?)
            .ok_or_else(|| storage("Source width exceeds bound"))?;
        let at = self
            .runs
            .partition_point(|(start, _)| usize::from(*start) <= class);
        let base = self
            .runs
            .get(
                at.checked_sub(1)
                    .ok_or_else(|| storage("Source width unavailable"))?,
            )
            .ok_or_else(|| storage("Source width unavailable"))?
            .1;
        add(base, extra)
    }
}

#[cfg(test)]
mod tests;
