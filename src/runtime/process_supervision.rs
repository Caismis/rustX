//! The runtime-level Linux process-supervision capability.
//!
//! This module owns the activation of rustX's process-wide child-subreaper
//! capability (`PR_SET_CHILD_SUBREAPER`). It is the runtime coordination
//! layer's one kernel-coordination primitive for catastrophic Bash
//! supervisor-loss recovery: when a Bash supervisor unit is lost, kernel
//! reparenting routes its orphaned invocation descendants to the runtime
//! process, where Bash catastrophic containment can still retain the inner
//! anchor and prove the invocation group terminal.
//!
//! # Contract — capability activation, not generic reaping
//!
//! The module activates a **kernel routing primitive**; it implements no
//! generic child-reaping semantics. Specifically:
//!
//! - enabling child-subreaper mode changes Linux process-wide orphan
//!   reparenting: orphans anywhere below the runtime process may reparent
//!   to the runtime process instead of init;
//! - M5 uses that mechanism only as the catastrophic fallback authority for
//!   **registered Bash supervisor units** — in M5, Bash supervisor units
//!   are the only production subprocess hierarchy relying on orphan
//!   adoption;
//! - kernel adoption does **not** by itself assign arbitrary adopted
//!   children to Bash lifecycle ownership, and M5 intentionally does **not**
//!   implement a generic unknown-child reaper: there is no process-wide
//!   `waitpid(-1)` / `waitid(P_ALL)` loop, no child ownership registry, and
//!   no background reaper task;
//! - kernel parenthood is not rustX semantic execution ownership: for
//!   Bash, semantic ownership is the invocation process group plus the
//!   retained inner anchor plus the explicit `ProcessLifecycle` state —
//!   never "anything whose `PPid` happens to become rustX";
//! - introducing another production subprocess hierarchy is an architecture
//!   change: its direct-child waiting, orphan adoption, and reaping
//!   ownership must be reconciled with runtime process supervision before
//!   it is merged.
//!
//! # Initialization contract
//!
//! Activation is **lazy, one-time, idempotent, and sticky**:
//!
//! - activation happens on the first Bash-capable invocation's consultation,
//!   not at runtime startup: the capability must exist before `START`
//!   authorizes Bash ownership, and it is not needed when no Bash execution
//!   is requested;
//! - the first consultation performs the `prctl` exactly once per runtime
//!   process; every later consultation observes the same result;
//! - a failed activation is remembered and fails every later consultation:
//!   Bash fallback containment must never be assumed after the runtime has
//!   once failed to become a subreaper;
//! - activation failure remains a pre-ownership setup failure: no
//!   supervisor unit, no `START`, no Bash, an explicit `Failed` result;
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

/// The one-time, sticky outcome of the child-subreaper activation.
///
/// A failed activation is recorded and every later consultation fails with
/// the same error: catastrophic Bash containment must never be assumed
/// after the runtime has once failed to become a subreaper.
#[cfg(target_os = "linux")]
static CHILD_SUBREAPER: OnceLock<Result<(), String>> = OnceLock::new();

/// Ensures the runtime process has the Linux child-subreaper capability:
/// the kernel routing primitive that makes catastrophic Bash supervisor-
/// loss adoption possible.
///
/// See the module documentation for the capability-activation contract,
/// the Bash-only M5 production scope, and the lazy one-time initialization
/// contract. This is a pre-ownership consultation: every Bash invocation
/// consults it before its supervisor unit spawns, so `START` (which
/// authorizes the Bash spawn) is never sent before catastrophic fallback
/// authority exists.
///
/// # Errors
///
/// Returns the `prctl` failure of the first (and only) activation attempt,
/// sticky for the process lifetime; on non-Linux platforms it always fails
/// because the subreaper mechanism does not exist there.
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

    /// The activation is idempotent: repeated consultations observe the
    /// same one-time result (on Linux this is the real `prctl`).
    #[test]
    fn child_subreaper_activation_is_idempotent() {
        let first = ensure_child_subreaper();
        let second = ensure_child_subreaper();
        assert_eq!(first, second, "one activation result per process lifetime");
    }
}
