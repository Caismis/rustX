//! Generic containment of **nested** supervised process units (Issue #145).
//!
//! # The problem
//!
//! Once a subagent child may run Bash, MCP stdio, Python/uv, or Skill
//! environment work, the process tree gains a level rustX never had before:
//!
//! ```text
//! top-level rustX
//!   └── subagent child rustX            (its own process group)
//!         └── outer supervisor
//!               └── inner supervisor    (its own setsid() session/group)
//!                     └── owned work
//! ```
//!
//! Killing the subagent child's process group reaches the child and the
//! outer supervisor. It does **not** reach the inner group: `setsid()` put
//! it in a different session by construction — that is precisely what makes
//! the unit's own `TERM`/`KILL` discipline correct in the first place. A
//! hard-killed child therefore leaves live owned work with no rustX owner.
//!
//! # The mechanism
//!
//! Every supervised unit already announces its containment anchor to its
//! local owner before the owner opens the unit's `START` gate:
//!
//! ```text
//! inner  --AnchorReady(pgid)-->  local owner  --START-->  inner
//! ```
//!
//! Inside a subagent child this module inserts the top-level parent into
//! that gate, once, generically, for **every** unit that goes through the
//! shared supervised-unit path:
//!
//! ```text
//! inner --AnchorReady(pgid)--> local owner
//!                                  |
//!                    ProcessUnitAnchorOffered { unit_id, pgid }
//!                                  v
//!                            top-level parent
//!                      retains the exact anchor under the
//!                      staged/live subagent owner
//!                                  |
//!                    ProcessUnitAnchorAccepted { unit_id }
//!                                  v
//!                             local owner --START--> inner
//! ```
//!
//! ## Core invariant
//!
//! > A nested supervised unit may not cross its local start/ownership gate
//! > until the top-level parent has acknowledged retention of that exact
//! > containment anchor.
//!
//! The acknowledgement is part of the ownership protocol, not telemetry.
//! It splits child death into two provably different worlds:
//!
//! ```text
//! child dies BEFORE the ACK   the inner never received START, so the
//!                             semantic command was never spawned
//! child dies AFTER the ACK    the parent holds the exact pgid and owns
//!                             catastrophic containment for it
//! ```
//!
//! ## Why the parent must already be a subreaper
//!
//! Parent-side containment reuses the *same* proven primitive the unit's own
//! owner uses — [`emergency_contain_group`] — which retains the adopted
//! anchor with `WNOWAIT`, issues one anchored `SIGKILL`, and proves the
//! group terminal by a group-scoped `ECHILD`. Every step requires the anchor
//! to have been **adopted** by the containing process when the intermediate
//! child died. On Linux that adoption is exactly `PR_SET_CHILD_SUBREAPER`,
//! and it must be installed in the top-level parent *before* the child is
//! spawned — a subreaper installed afterwards does not retroactively adopt.
//! [`crate::runtime::process_supervision::ensure_child_subreaper`] is
//! therefore consulted before staging, and preparation fails rather than
//! claiming containment it cannot perform.
//!
//! macOS has no equivalent primitive: an orphaned anchor reparents to
//! `init`, the anchor wait answers `ECHILD`, and containment reports
//! [`EmergencyContainment::AnchorUnavailable`] — the existing explicit
//! "terminality unproven" semantics. That weaker contract is reported
//! honestly and never fabricated into parity.
//!
//! # Ownership
//!
//! ```text
//! NestedAnchorAuthority   installed once per process; present only in a
//!                         subagent child. Its implementation is the child
//!                         control dispatcher — the sole owner of the IPC
//!                         transport. Unit owners never touch that stream.
//!
//! ProcessUnitAnchor       the RAII lease one supervised unit holds from
//!                         ACK until it has proven its unit physically
//!                         terminal. Releasing it emits exactly one
//!                         ProcessUnitAnchorReleased for that unit.
//!
//! RetainedProcessUnits    the parent-side set. It belongs to whichever
//!                         owner currently owns the child: StagedChild
//!                         before the ownership commit, the live record
//!                         after it. Ownership transfers exactly once.
//! ```
//!
//! In the **top-level** process no authority is installed, so
//! [`retain`] resolves immediately into an unanchored lease and the
//! existing single-level behaviour is bit-for-bit unchanged.

use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::future::BoxFuture;

use crate::runtime::identity::ProcessUnitId;

/// A failure of the nested containment handshake.
///
/// Every variant happens **before** the unit's local `START` gate opens, so
/// no semantic command has been spawned when one is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AnchorError {
    /// The parent control channel closed while the offer was outstanding.
    /// The parent is gone; nothing may start.
    ParentLost,
    /// The parent refused to retain the anchor (it is settling this child).
    Refused(String),
}

impl core::fmt::Display for AnchorError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ParentLost => formatter.write_str(
                "the parent runtime disappeared before it acknowledged retention of this \
                 nested process unit's containment anchor",
            ),
            Self::Refused(detail) => write!(
                formatter,
                "the parent runtime refused to retain this nested process unit's containment \
                 anchor: {detail}"
            ),
        }
    }
}

impl std::error::Error for AnchorError {}

/// The process-local authority that anchors nested supervised units in the
/// top-level parent.
///
/// Exactly one implementation exists — the subagent child's control
/// dispatcher — and it is installed once, before the child answers `Ready`.
/// A unit owner never learns anything about the transport behind it.
pub(crate) trait NestedAnchorAuthority: Send + Sync + std::fmt::Debug {
    /// Offers one anchor and resolves when the parent has acknowledged
    /// retention of exactly this unit.
    fn offer(&self, unit: ProcessUnitId, pgid: i32) -> BoxFuture<'static, Result<(), AnchorError>>;

    /// Reports that this unit is physically terminal, so the parent may drop
    /// exactly this retained anchor. Best-effort by construction: a parent
    /// that is already gone has nothing left to release.
    fn release(&self, unit: ProcessUnitId, pgid: i32);
}

/// The process's one nested anchor authority.
///
/// The slot is a lock rather than a `OnceLock` only so the test suite can
/// install a recording authority around one test and remove it again;
/// [`INSTALLED`] preserves the production one-time-and-sticky contract
/// regardless.
static AUTHORITY: std::sync::RwLock<Option<std::sync::Arc<dyn NestedAnchorAuthority>>> =
    std::sync::RwLock::new(None);
static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static NEXT_UNIT: AtomicU64 = AtomicU64::new(0);

/// Installs the process's nested anchor authority.
///
/// Installation is one-time and sticky: a second installation would let two
/// owners believe they anchor the same units.
///
/// # Errors
///
/// Returns an error when an authority is already installed.
pub(crate) fn install_authority(
    authority: std::sync::Arc<dyn NestedAnchorAuthority>,
) -> Result<(), String> {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return Err("a nested anchor authority is already installed".to_owned());
    }
    *AUTHORITY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(authority);
    Ok(())
}

/// The installed authority, if this process is a subagent child.
#[must_use]
pub(crate) fn authority() -> Option<std::sync::Arc<dyn NestedAnchorAuthority>> {
    AUTHORITY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Serializes every test that installs a process-global authority.
///
/// The authority is process-global by design (a subagent child has exactly
/// one parent), so tests that install one must not run concurrently with
/// each other or with a test that relies on there being none.
#[cfg(test)]
pub(crate) fn test_authority_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Installs a nested anchor authority for the duration of one test.
///
/// The returned guard removes it again, so the process returns to its
/// top-level (unanchored) shape. Hold [`test_authority_lock`] across the
/// guard's lifetime.
#[cfg(test)]
pub(crate) fn install_authority_for_test(
    authority: std::sync::Arc<dyn NestedAnchorAuthority>,
) -> TestAuthorityGuard {
    *AUTHORITY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(authority);
    TestAuthorityGuard
}

/// Removes the test-installed authority on drop.
#[cfg(test)]
pub(crate) struct TestAuthorityGuard;

#[cfg(test)]
impl Drop for TestAuthorityGuard {
    fn drop(&mut self) {
        *AUTHORITY
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// Whether this process anchors its supervised units in a parent.
#[cfg(test)]
#[must_use]
pub(crate) fn is_nested() -> bool {
    authority().is_some()
}

/// Allocates the typed identity of one supervised process unit.
#[must_use]
pub(crate) fn allocate_unit_id() -> ProcessUnitId {
    let ordinal = NEXT_UNIT.fetch_add(1, Ordering::Relaxed);
    ProcessUnitId::new(format!("unit:{}:{ordinal}", std::process::id()))
}

/// Retains one supervised unit's containment anchor in the top-level parent
/// and resolves only when the parent has acknowledged it.
///
/// In the top-level process there is no authority, so this resolves
/// immediately with an unanchored lease: there is no outer owner to anchor
/// into, and the unit's own supervisor already owns its containment.
///
/// `override_authority` is `None` in production, which reads the
/// process-global authority. An explicit authority exists so one invocation
/// can be anchored into a recording owner without perturbing every other
/// supervised unit in the same process.
pub(crate) fn retain_with(
    unit: ProcessUnitId,
    pgid: i32,
    override_authority: Option<std::sync::Arc<dyn NestedAnchorAuthority>>,
) -> BoxFuture<'static, Result<ProcessUnitAnchor, AnchorError>> {
    let Some(authority) = override_authority.or_else(authority) else {
        return Box::pin(std::future::ready(Ok(ProcessUnitAnchor {
            unit,
            pgid,
            authority: None,
        })));
    };
    Box::pin(async move {
        authority.offer(unit.clone(), pgid).await?;
        Ok(ProcessUnitAnchor {
            unit,
            pgid,
            authority: Some(authority),
        })
    })
}

/// The lease one supervised unit holds from the parent's acknowledgement
/// until that unit is proven physically terminal.
///
/// Release is idempotent and also happens on drop, so no code path can lose
/// a retained anchor in the parent by forgetting to release it. Releasing
/// early would be the real hazard, which is why the only release sites are
/// the unit's proven-terminal settlements.
#[derive(Debug)]
pub(crate) struct ProcessUnitAnchor {
    unit: ProcessUnitId,
    pgid: i32,
    authority: Option<std::sync::Arc<dyn NestedAnchorAuthority>>,
}

impl ProcessUnitAnchor {
    /// Whether this lease is actually anchored in a parent.
    #[cfg(test)]
    pub(crate) const fn is_anchored(&self) -> bool {
        self.authority.is_some()
    }

    /// Reports the unit physically terminal and drops the parent's retained
    /// anchor. Called exactly at a proven-terminal settlement.
    pub(crate) fn release(mut self) {
        self.release_in_place();
    }

    /// Discards the lease without emitting a release, keeping the parent's
    /// retained anchor alive.
    fn forget(&mut self) {
        self.authority = None;
    }

    fn release_in_place(&mut self) {
        if let Some(authority) = self.authority.take() {
            authority.release(self.unit.clone(), self.pgid);
        }
    }
}

impl Drop for ProcessUnitAnchor {
    fn drop(&mut self) {
        self.release_in_place();
    }
}

/// The gate state one supervised-unit owner keeps while its anchor offer is
/// outstanding.
///
/// The owner's settlement loop must stay responsive to cancellation, the
/// invocation deadline, and supervisor events while the parent decides, so
/// the offer is polled as one arm of that loop rather than awaited inline.
#[derive(Default)]
pub(crate) enum AnchorGate {
    /// No anchor has been offered yet.
    #[default]
    Idle,
    /// The offer is outstanding; the unit may not `START`.
    Pending(BoxFuture<'static, Result<ProcessUnitAnchor, AnchorError>>),
    /// The parent acknowledged; the unit may `START` and the lease is held
    /// until proven-terminal settlement.
    Held(ProcessUnitAnchor),
    /// The gate has been consumed by settlement.
    Released,
}

impl core::fmt::Debug for AnchorGate {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Idle => formatter.write_str("Idle"),
            Self::Pending(_) => formatter.write_str("Pending"),
            Self::Held(anchor) => formatter.debug_tuple("Held").field(anchor).finish(),
            Self::Released => formatter.write_str("Released"),
        }
    }
}

impl AnchorGate {
    /// Begins the handshake for one unit anchor, with an optional explicit
    /// authority override.
    pub(crate) fn offer_with(
        &mut self,
        pgid: i32,
        override_authority: Option<std::sync::Arc<dyn NestedAnchorAuthority>>,
    ) {
        *self = Self::Pending(retain_with(allocate_unit_id(), pgid, override_authority));
    }

    /// Whether the gate is waiting for the parent.
    pub(crate) const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }

    /// Awaits the outstanding offer. Only valid while [`AnchorGate::is_pending`].
    pub(crate) async fn settle_offer(&mut self) -> Result<(), AnchorError> {
        let Self::Pending(pending) = self else {
            // Not pending: park forever so this select arm never fires.
            std::future::pending::<()>().await;
            unreachable!("a non-pending anchor gate never resolves");
        };
        match pending.await {
            Ok(anchor) => {
                *self = Self::Held(anchor);
                Ok(())
            }
            Err(error) => {
                *self = Self::Released;
                Err(error)
            }
        }
    }

    /// Releases the retained anchor at a proven-terminal settlement.
    pub(crate) fn release(&mut self) {
        if let Self::Held(anchor) = std::mem::replace(self, Self::Released) {
            anchor.release();
        }
    }

    /// Keeps the parent's retained anchor **alive** because this process
    /// could not prove the unit physically terminal.
    ///
    /// This is the honest counterpart of [`AnchorGate::release`]: an
    /// unproven settlement must not drop the parent's containment authority
    /// for that exact group, so the lease is discarded without emitting a
    /// release.
    pub(crate) fn retain_unproven(&mut self) {
        if let Self::Held(mut anchor) = std::mem::replace(self, Self::Released) {
            anchor.forget();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AnchorError, AnchorGate, allocate_unit_id, is_nested, retain_with};

    /// In the top-level process there is no authority, so retention is an
    /// immediate unanchored lease and existing single-level behaviour is
    /// unchanged.
    #[tokio::test]
    async fn the_top_level_process_retains_nothing() {
        let _serial = super::test_authority_lock().lock().await;
        assert!(!is_nested(), "the test process installs no authority");
        let anchor = retain_with(allocate_unit_id(), 4242, None)
            .await
            .expect("an unanchored lease never fails");
        assert!(!anchor.is_anchored());
    }

    /// Unit identities are unique within a process.
    #[test]
    fn unit_identities_are_unique() {
        let first = allocate_unit_id();
        let second = allocate_unit_id();
        assert_ne!(first, second);
        assert!(
            first
                .as_str()
                .starts_with(&format!("unit:{}:", std::process::id()))
        );
    }

    /// A gate that never received an offer parks forever rather than
    /// resolving, so its select arm can be written unconditionally.
    #[tokio::test]
    async fn an_idle_gate_never_resolves() {
        let mut gate = AnchorGate::Idle;
        assert!(!gate.is_pending());
        let parked =
            tokio::time::timeout(std::time::Duration::from_millis(20), gate.settle_offer()).await;
        assert!(parked.is_err(), "an idle gate must not resolve");
    }

    /// A failed offer leaves the gate released: the unit may never start
    /// and there is nothing to release later.
    #[tokio::test]
    async fn a_failed_offer_never_holds_an_anchor() {
        #[derive(Debug)]
        struct Refusing;
        impl super::NestedAnchorAuthority for Refusing {
            fn offer(
                &self,
                _unit: crate::runtime::identity::ProcessUnitId,
                _pgid: i32,
            ) -> futures_util::future::BoxFuture<'static, Result<(), AnchorError>> {
                Box::pin(std::future::ready(Err(AnchorError::ParentLost)))
            }
            fn release(&self, _unit: crate::runtime::identity::ProcessUnitId, _pgid: i32) {}
        }
        let authority = std::sync::Arc::new(Refusing);
        let mut gate = AnchorGate::Pending(Box::pin(async move {
            super::NestedAnchorAuthority::offer(authority.as_ref(), allocate_unit_id(), 7)
                .await
                .map(|()| unreachable!("the refusing authority never accepts"))
        }));
        assert!(gate.is_pending());
        assert_eq!(gate.settle_offer().await, Err(AnchorError::ParentLost));
        assert!(matches!(gate, AnchorGate::Released));
    }
}

#[cfg(all(test, unix))]
mod global_authority_tests {
    use crate::runtime::identity::ProcessUnitId;
    use crate::runtime::nested_containment::{AnchorError, NestedAnchorAuthority};
    use std::sync::{Arc, Mutex};

    /// An authority that accepts every offer immediately and records it.
    ///
    /// Accepting immediately matters: this test installs a **process-global**
    /// authority, exactly as the child dispatcher does, so any other
    /// supervised unit running concurrently in this test binary also passes
    /// through it and must not be blocked by it.
    #[derive(Debug, Default)]
    struct RecordingAuthority {
        offers: Mutex<Vec<(ProcessUnitId, i32)>>,
        releases: Mutex<Vec<(ProcessUnitId, i32)>>,
    }

    impl NestedAnchorAuthority for RecordingAuthority {
        fn offer(
            &self,
            unit: ProcessUnitId,
            pgid: i32,
        ) -> futures_util::future::BoxFuture<'static, Result<(), AnchorError>> {
            self.offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((unit, pgid));
            Box::pin(std::future::ready(Ok(())))
        }

        fn release(&self, unit: ProcessUnitId, pgid: i32) {
            self.releases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((unit, pgid));
        }
    }

    /// The **production wiring**: a supervised unit created with no
    /// per-invocation override reads the process-global authority, which is
    /// what the subagent child's control dispatcher installs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_globally_installed_authority_anchors_ordinary_supervised_units() {
        let _serial = super::test_authority_lock().lock().await;
        let authority = Arc::new(RecordingAuthority::default());
        let _guard = super::install_authority_for_test(authority.clone());

        let dir = tempfile::tempdir().expect("lab");
        let marker = dir.path().join("started");
        let result = crate::runtime::process_runner::SupervisedProcessRunner::run(
            &crate::runtime::process_runner::RunnerBackedProcessRunner::default(),
            crate::runtime::process_runner::SupervisedCommandSpec {
                command: format!("touch {}", marker.display()),
                cwd: std::env::temp_dir(),
                environment: vec![("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned())],
                timeout: Some(std::time::Duration::from_secs(30)),
                cancellation: crate::runtime::cancellation::CancellationSignal::new(),
            },
            None,
        )
        .await
        .expect("a terminal result");
        assert_eq!(
            result.intent,
            crate::runtime::process_runner::ProcessOutcomeIntent::Completed
        );
        assert!(marker.exists());

        let offers = authority
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let releases = authority
            .releases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        // The authority is process-global, so concurrently running tests in
        // this binary also route their units through it. The assertions are
        // therefore written over facts that hold regardless of which other
        // units happened to be in flight.
        assert!(
            !offers.is_empty(),
            "an ordinary supervised unit offers its anchor to the installed authority"
        );
        assert!(
            offers.iter().all(|(_, pgid)| *pgid > 0),
            "every offered anchor is a real process group"
        );
        assert!(
            !releases.is_empty(),
            "this invocation returned only after its own unit was proven terminal, so its \
             anchor release is already recorded"
        );
        assert!(
            releases
                .iter()
                .all(|(released, _)| offers.iter().any(|(offered, _)| offered == released)),
            "a release always names a unit that was offered first"
        );
    }
}

#[cfg(all(test, unix))]
mod containment_tests {
    use crate::runtime::supervised_unit::{EmergencyContainment, contain_adopted_group};

    /// A direct child of this process in its own process group is a valid
    /// adopted anchor: containment retains its identity, kills the group,
    /// and proves the group terminal group-scoped.
    ///
    /// This is the exact primitive the parent uses for a nested unit whose
    /// owning subagent child has died: it must not wait for the inner to
    /// exit on its own, because nothing remains to drive it there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_adopted_live_group_is_contained_and_proven_terminal() {
        let mut command = tokio::process::Command::new("sleep");
        command.arg("300");
        command.process_group(0);
        let child = command.spawn().expect("spawn a group leader");
        let pgid = i32::try_from(child.id().expect("a live child has a pid")).expect("pid fits");

        let contained = tokio::task::spawn_blocking(move || contain_adopted_group(pgid))
            .await
            .expect("containment task")
            .expect("containment must not fail for an adopted anchor");
        assert_eq!(contained, EmergencyContainment::TerminalProven);
        assert!(
            matches!(
                nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), None),
                Err(nix::errno::Errno::ESRCH)
            ),
            "the contained group no longer exists"
        );
        drop(child);
    }

    /// An anchor this process cannot adopt is never signalled and never
    /// reported terminal: the weaker platform contract is explicit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unadoptable_anchor_is_reported_unproven() {
        // pid 1 is never a child of this process, so the anchor wait answers
        // ECHILD exactly as an orphan reparented to init would on a platform
        // without an orphan-adoption primitive.
        let contained = tokio::task::spawn_blocking(|| contain_adopted_group(1))
            .await
            .expect("containment task")
            .expect("an unadoptable anchor is a result, not an error");
        assert_eq!(
            contained,
            EmergencyContainment::AnchorUnavailable,
            "no terminality may be claimed for an anchor this process does not own"
        );
    }
}
