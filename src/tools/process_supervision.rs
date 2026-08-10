//! The runtime-level Linux process-supervision primitive.
//!
//! This module is the one explicit owner of rustX's process-wide
//! child-subreaper enablement (`PR_SET_CHILD_SUBREAPER`). It is the
//! reaper-of-last-resort authority for owned subprocess hierarchies: when
//! the per-invocation Bash supervisor unit is lost, adopted invocation
//! descendants reparent into the runtime process and are contained and
//! reaped from here (see the Bash tool's catastrophic containment).
//!
//! # Process-wide scope — not a Bash-local setting
//!
//! `PR_SET_CHILD_SUBREAPER` is a property of the whole rustX process, not
//! of one invocation or one tool: once enabled, orphaned descendants of
//! **any** rustX-owned subprocess hierarchy (Bash today, other native
//! executors later) reparent to the runtime process instead of init. This
//! module therefore documents the adoption contract explicitly:
//!
//! - the runtime process owns every adopted child in its subreaper domain;
//! - adoption is a reaping responsibility, never a semantic-ownership
//!   claim: one invocation's containment may only ever signal and wait on
//!   its own explicit identities (anchor pid / invocation process group),
//!   never on a broad `waitpid(-1)` or `waitid(P_ALL)`;
//! - concurrent Bash invocations stay isolated: each retains and reaps
//!   only its own adopted anchor, and each signals only its own process
//!   group.
//!
//! # Initialization contract
//!
//! Initialization is **one-time, idempotent, and sticky**:
//!
//! - the first consultation performs the `prctl` exactly once per runtime
//!   process; every later consultation observes the same result;
//! - a failed initialization is remembered and fails every later
//!   consultation: Bash fallback containment must never be assumed after
//!   the runtime has once failed to become a subreaper;
//! - the consultation happens **before** any Bash ownership exists (before
//!   the supervisor spawn, hence before `START` authorizes the Bash
//!   spawn), so an initialization failure is a pre-ownership setup failure
//!   that settles as `Failed` with no Bash tree spawned;
//! - the mode is never toggled per invocation and never disabled.
//!
//! # Unsafe boundary
//!
//! The only OS mutation is one `prctl` scalar syscall with a literal
//! enable value, executed at most once per process lifetime.
//!
//! [`PR_SET_CHILD_SUBREAPER`]: libc::PR_SET_CHILD_SUBREAPER

#[cfg(target_os = "linux")]
use std::sync::OnceLock;

/// The one-time, sticky outcome of the child-subreaper initialization.
///
/// A failed initialization is recorded and every later consultation fails
/// with the same error: catastrophic Bash containment must never be
/// assumed after the runtime has once failed to become a subreaper.
#[cfg(target_os = "linux")]
static CHILD_SUBREAPER: OnceLock<Result<(), String>> = OnceLock::new();

/// Ensures the runtime process is a child subreaper: the reaper of last
/// resort for owned subprocess hierarchies.
///
/// See the module documentation for the process-wide scope, the adoption
/// contract, and the one-time initialization contract. This is a
/// pre-ownership consultation: every Bash invocation calls it before its
/// supervisor unit spawns, so `START` (which authorizes the Bash spawn)
/// is never sent before catastrophic fallback authority exists.
///
/// # Errors
///
/// Returns the `prctl` failure of the first (and only) initialization
/// attempt, sticky for the process lifetime; on non-Linux platforms it
/// always fails because the subreaper mechanism does not exist there.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // one scalar prctl syscall, no pointer arguments
pub(crate) fn ensure_child_subreaper() -> Result<(), String> {
    CHILD_SUBREAPER
        .get_or_init(|| {
            // SAFETY: prctl with PR_SET_CHILD_SUBREAPER and a literal 1 is
            // a single scalar syscall with no pointer arguments, executed
            // at most once per runtime process lifetime.
            let result = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
            if result == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error().to_string())
            }
        })
        .clone()
}

/// The non-Linux consultation: the lifecycle contract is claimed only
/// where the kernel provides the subreaper mechanism, so every Bash
/// invocation fails as a pre-ownership setup failure.
#[cfg(not(target_os = "linux"))]
pub(crate) fn ensure_child_subreaper() -> Result<(), String> {
    Err("Bash fallback containment requires Linux PR_SET_CHILD_SUBREAPER".to_owned())
}

#[cfg(test)]
mod tests {
    use super::ensure_child_subreaper;

    /// The initialization is idempotent: repeated consultations observe
    /// the same one-time result (on Linux this is the real `prctl`).
    #[test]
    fn child_subreaper_initialization_is_idempotent() {
        let first = ensure_child_subreaper();
        let second = ensure_child_subreaper();
        assert_eq!(
            first, second,
            "one initialization result per process lifetime"
        );
    }
}
