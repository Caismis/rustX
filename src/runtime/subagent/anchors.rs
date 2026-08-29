//! Parent-side retention and settlement of a subagent child's **nested**
//! supervised process units (Issue #145).
//!
//! A subagent child that runs Bash, MCP stdio, Python/uv, or Skill
//! environment work creates supervised units whose inner `setsid()` group is
//! deliberately outside the child's own process group. Killing the child's
//! group therefore cannot reach them. This module is the parent half of the
//! generic anchor protocol described in
//! [`crate::runtime::nested_containment`]:
//!
//! ```text
//! child   ProcessUnitAnchorOffered { unit_id, pgid }
//!   ->    parent retains the exact pair
//!   ->    ProcessUnitAnchorAccepted { unit_id }
//!   ->    only now may the child's local supervisor send START
//!
//! child   ProcessUnitAnchorReleased { unit_id, pgid }
//!   ->    parent drops exactly that anchor (proven terminal by the child)
//! ```
//!
//! # Ownership
//!
//! The retained set is **owned**, never shared:
//!
//! ```text
//! StagedChild   owns the direct child process AND the retained anchors
//!     |          (units may be offered during child preparation, long
//!     |           before any durable ownership commit)
//!     |  exactly-once transfer at the ownership commit
//!     v
//! child driver task (registry-owned)
//!                owns both until it publishes the physical settlement
//! ```
//!
//! There is one ownership commit, not two, and the set is moved rather than
//! copied — so no code path can leave an anchor with an ambiguous owner.
//!
//! # Settlement
//!
//! > A direct child reap is not proof of physical settlement while retained
//! > nested anchors are unresolved.
//!
//! The owner therefore settles every still-retained anchor **after** the
//! direct child is reaped and before it publishes an outcome. Containment
//! reuses the one proven primitive
//! ([`emergency_contain_group`](crate::runtime::supervised_unit::emergency_contain_group)):
//! retain the adopted anchor with `WNOWAIT`, issue one anchored `SIGKILL`,
//! and prove the group terminal with a group-scoped `ECHILD`.
//!
//! On Linux the adoption that makes this possible is the top-level parent's
//! `PR_SET_CHILD_SUBREAPER`, which is why it is installed *before* the child
//! is spawned. On macOS an orphaned anchor reparents to `init`, the anchor
//! wait answers `ECHILD`, and the settlement is reported as explicitly
//! **unproven** rather than pretending Linux parity.

use std::collections::BTreeMap;

use crate::runtime::identity::ProcessUnitId;

/// The exact anchors one child currently has retained in this parent.
#[derive(Debug, Default)]
pub(crate) struct RetainedProcessUnits {
    units: BTreeMap<ProcessUnitId, i32>,
}

/// Why an offered anchor was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AnchorRefusal {
    /// The child offered the same unit identity twice.
    DuplicateUnit,
    /// The offered process-group id is not a legal group identity.
    InvalidPgid,
}

impl AnchorRefusal {
    /// The bounded reason sent back to the child.
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::DuplicateUnit => "a nested process unit of that identity is already retained",
            Self::InvalidPgid => "the offered containment anchor is not a valid process group",
        }
    }
}

impl RetainedProcessUnits {
    /// Retains one offered anchor.
    ///
    /// Retention is the parent's linearization point: it happens strictly
    /// before the acknowledgement is written, so an acknowledged unit is
    /// always already retained.
    ///
    /// # Errors
    ///
    /// Refuses a duplicate identity or an illegal process-group id. A
    /// refused offer leaves the set unchanged and the unit never starts.
    pub(crate) fn retain(
        &mut self,
        unit_id: ProcessUnitId,
        pgid: i32,
    ) -> Result<(), AnchorRefusal> {
        if pgid <= 0 {
            return Err(AnchorRefusal::InvalidPgid);
        }
        if self.units.contains_key(&unit_id) {
            return Err(AnchorRefusal::DuplicateUnit);
        }
        self.units.insert(unit_id, pgid);
        Ok(())
    }

    /// Drops exactly the named anchor after the child proved that unit
    /// physically terminal.
    ///
    /// Correlation is by typed identity, never by pgid similarity or by
    /// arrival order, and a release whose pgid disagrees with what was
    /// retained is ignored: it cannot name the unit the parent retained.
    /// Returns whether an anchor was actually removed.
    pub(crate) fn release(&mut self, unit_id: &ProcessUnitId, pgid: i32) -> bool {
        match self.units.get(unit_id) {
            Some(retained) if *retained == pgid => {
                self.units.remove(unit_id);
                true
            }
            _ => false,
        }
    }

    /// Whether every anchor this child offered has been released.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// The retained anchors in deterministic identity order.
    #[cfg(test)]
    pub(crate) fn retained(&self) -> Vec<(ProcessUnitId, i32)> {
        self.units
            .iter()
            .map(|(unit, pgid)| (unit.clone(), *pgid))
            .collect()
    }

    /// The number of currently retained anchors.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.units.len()
    }

    /// Takes the retained set, leaving it empty. Used at the one settlement
    /// boundary so containment consumes the anchors it settles.
    pub(crate) fn take(&mut self) -> Vec<(ProcessUnitId, i32)> {
        std::mem::take(&mut self.units)
            .into_iter()
            .collect::<Vec<_>>()
    }
}

/// The outcome of settling one child's still-retained nested anchors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NestedUnitSettlement {
    /// Units the parent proved terminal by containment, in identity order.
    pub contained: Vec<ProcessUnitId>,
    /// Units whose physical terminality could not be proven on this
    /// platform, with the bounded reason.
    pub unproven: Vec<(ProcessUnitId, String)>,
}

impl NestedUnitSettlement {
    /// Whether every settled unit reached a proven-terminal state.
    #[cfg(test)]
    pub(crate) fn is_proven(&self) -> bool {
        self.unproven.is_empty()
    }

    /// The bounded diagnostic of an unproven settlement, if any.
    pub(crate) fn unproven_diagnostic(&self) -> Option<String> {
        if self.unproven.is_empty() {
            return None;
        }
        let detail = self
            .unproven
            .iter()
            .map(|(unit, reason)| format!("{unit}: {reason}"))
            .collect::<Vec<_>>()
            .join("; ");
        Some(format!(
            "the physical settlement of {} nested supervised process unit(s) of this child \
             is unproven on this platform: {detail}",
            self.unproven.len()
        ))
    }
}

/// Contains every still-retained nested anchor of a child that did not
/// release them itself.
///
/// Called strictly **after** the direct child has been reaped, so the
/// anchors have already been orphaned and (on Linux) adopted by this
/// process. Containment of one anchor never depends on another, so a child
/// with several units settles each independently.
pub(crate) async fn contain_retained(retained: Vec<(ProcessUnitId, i32)>) -> NestedUnitSettlement {
    let mut settlement = NestedUnitSettlement::default();
    for (unit_id, pgid) in retained {
        match contain_one(pgid).await {
            Ok(()) => settlement.contained.push(unit_id),
            Err(reason) => settlement.unproven.push((unit_id, reason)),
        }
    }
    settlement
}

#[cfg(unix)]
async fn contain_one(pgid: i32) -> Result<(), String> {
    use crate::runtime::supervised_unit::{EmergencyContainment, contain_adopted_group};
    // The containment primitive blocks on kernel waits, so it runs on the
    // blocking pool.
    match tokio::task::spawn_blocking(move || contain_adopted_group(pgid)).await {
        Ok(Ok(EmergencyContainment::TerminalProven)) => Ok(()),
        Ok(Ok(EmergencyContainment::AnchorUnavailable)) => Err(
            "the orphaned unit anchor is not adoptable by this process, so its process group \
             cannot be proven terminal (this platform provides no orphan-adoption primitive)"
                .to_owned(),
        ),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(format!("the containment task failed: {error}")),
    }
}

#[cfg(not(unix))]
async fn contain_one(_pgid: i32) -> Result<(), String> {
    Err("nested process-unit containment requires Unix process supervision".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{AnchorRefusal, NestedUnitSettlement, RetainedProcessUnits};
    use crate::runtime::identity::ProcessUnitId;

    fn unit(name: &str) -> ProcessUnitId {
        ProcessUnitId::new(name)
    }

    /// Retention is by exact typed identity, and a duplicate identity is
    /// refused rather than silently replacing a retained anchor.
    #[test]
    fn a_duplicate_unit_identity_is_refused() {
        let mut retained = RetainedProcessUnits::default();
        retained.retain(unit("a"), 11).expect("first offer");
        assert_eq!(
            retained.retain(unit("a"), 22),
            Err(AnchorRefusal::DuplicateUnit)
        );
        assert_eq!(retained.retained(), vec![(unit("a"), 11)]);
    }

    /// An illegal group id is refused: the parent must never retain
    /// something it could later signal as a wildcard.
    #[test]
    fn an_illegal_group_is_refused() {
        let mut retained = RetainedProcessUnits::default();
        assert_eq!(
            retained.retain(unit("a"), 0),
            Err(AnchorRefusal::InvalidPgid)
        );
        assert_eq!(
            retained.retain(unit("a"), -1),
            Err(AnchorRefusal::InvalidPgid)
        );
        assert!(retained.is_empty());
    }

    /// Releasing one unit removes exactly that anchor and leaves every
    /// other retained anchor untouched.
    #[test]
    fn releasing_one_unit_removes_only_that_anchor() {
        let mut retained = RetainedProcessUnits::default();
        retained.retain(unit("a"), 11).expect("a");
        retained.retain(unit("b"), 22).expect("b");
        retained.retain(unit("c"), 33).expect("c");
        assert!(retained.release(&unit("b"), 22));
        assert_eq!(retained.retained(), vec![(unit("a"), 11), (unit("c"), 33)]);
        assert_eq!(retained.len(), 2);
    }

    /// A release that names a retained unit with a different pgid is not
    /// that unit's release and never drops the retained anchor.
    #[test]
    fn a_mismatched_release_is_ignored() {
        let mut retained = RetainedProcessUnits::default();
        retained.retain(unit("a"), 11).expect("a");
        assert!(!retained.release(&unit("a"), 12));
        assert!(!retained.release(&unit("z"), 11));
        assert_eq!(retained.retained(), vec![(unit("a"), 11)]);
    }

    /// Taking the set is the settlement boundary: it consumes exactly what
    /// was retained.
    #[test]
    fn taking_the_set_consumes_it_once() {
        let mut retained = RetainedProcessUnits::default();
        retained.retain(unit("a"), 11).expect("a");
        assert_eq!(retained.take(), vec![(unit("a"), 11)]);
        assert!(retained.is_empty());
        assert!(retained.take().is_empty());
    }

    /// An unproven settlement renders one bounded diagnostic naming every
    /// unit it could not prove.
    #[test]
    fn an_unproven_settlement_is_reported_explicitly() {
        let settlement = NestedUnitSettlement {
            contained: vec![unit("a")],
            unproven: vec![(unit("b"), "no adoption primitive".to_owned())],
        };
        assert!(!settlement.is_proven());
        let diagnostic = settlement.unproven_diagnostic().expect("a diagnostic");
        assert!(diagnostic.contains("b: no adoption primitive"));
        assert!(NestedUnitSettlement::default().is_proven());
        assert_eq!(NestedUnitSettlement::default().unproven_diagnostic(), None);
    }
}
