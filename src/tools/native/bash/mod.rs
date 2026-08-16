//! Native Bash tool (M5).
//!
//! Executes one non-interactive `/bin/bash -c <command>` invocation per
//! call. No persistent shell exists and no shell state survives between
//! calls. The current working directory is always the explicit workspace
//! root, and the child environment is explicit: `env_clear()` followed by
//! the runtime-approved basics plus explicitly authorized entries, so
//! parent-process secrets are absent unless explicitly authorized.
//!
//! # Complete lifecycle ownership
//!
//! A Bash invocation is one complete lifecycle: spawn one per-invocation
//! supervisor (see [`supervisor`]), read stdout/stderr, wait for the
//! shell, let the supervisor own the invocation's process group to its
//! kernel-mediated terminal state, handle cancellation/timeout with a
//! `TERM` -> grace -> `KILL` sequence inside the supervisor, complete the
//! output draining, finalize the artifacts, and produce a single canonical
//! result.
//!
//! The process-ownership half of this lifecycle is the shared internal
//! supervised command runner (`crate::runtime::process_runner`): the same
//! supervisor unit, control protocol, process-group ownership, cancellation
//! settlement, and catastrophic containment that native Bash uses is reused
//! by Skill environment materialization, so package-manager work never
//! becomes a second independent subprocess hierarchy. This tool owns the
//! capture half: artifact spooling, bounded previews, and the final
//! canonical result formatting.
//!
//! Shell-parent exit is **not** by itself the Bash settlement boundary:
//! the shell may exit while a descendant still belongs to the
//! invocation-owned process group, with the output pipes either still held
//! or already redirected away. The invocation therefore settles naturally
//! only when all three of the following are true:
//!
//! - the shell's terminal status is known (the supervisor reported it);
//! - the invocation-owned process group is terminal (the supervisor's
//!   group-scoped wait reached `ECHILD` and the outer supervisor reported
//!   the authoritative `AllChildrenReaped`);
//! - the runtime-owned output capture is settled.
//!
//! Cancellation and the invocation deadline remain authoritative until the
//! complete lifecycle settles: they trigger the supervisor's
//! `TERM` -> grace -> `KILL` sequence, so a shell-parent exit can never let
//! owned group work escape the timeout/cancellation contract, even when the
//! descendant no longer holds the rustX pipes.
//!
//! # Fixed invocation process group
//!
//! A Bash invocation executes inside one dedicated rustX-owned process
//! group. On Linux, process-group/session mutation from Bash descendants is
//! rejected by an inherited seccomp policy. On macOS, the same session/group
//! lifecycle and cancellation path are available, but macOS has no seccomp
//! or child-subreaper equivalent; the supervisor wraps Bash with an EXIT
//! `wait` so ordinary background jobs remain attached to the shell lifecycle.
//! macOS therefore does not claim Linux's immutable-membership or
//! supervisor-loss orphan-adoption proof.
//!
//! On Linux this restriction makes the supervisor's kernel child-wait
//! terminal proof complete. macOS retains the real process-group wait and
//! normal cancellation behavior, with the weaker platform guarantees stated
//! above.
//!
//! # Ownership boundary
//!
//! The Bash invocation's ownership boundary is its dedicated process
//! group. The invocation owns, guarantees termination of, and bases its
//! settlement on exactly the processes that remain in that group. On Linux,
//! group membership is immutable for Bash descendants. On macOS, descendants
//! normally remain in the dedicated group, but a command that successfully
//! calls `setsid(2)` can leave it; macOS does not provide the Linux seccomp
//! mechanism used to reject that syscall.
//!
//! # Runtime child-subreaper capability
//!
//! Linux's process-wide `PR_SET_CHILD_SUBREAPER` activation is a runtime
//! coordination-layer capability, not a Bash-local setting or generic
//! reaper. It is owned by [`crate::runtime::process_supervision`] and
//! established before `START` authorizes the Bash spawn. macOS has no
//! equivalent; its normal path relies on Bash waiting for ordinary
//! background jobs, and a lost supervisor cannot claim the Linux
//! adopted-anchor proof.
//!
//! # Terminal results
//!
//! Every Bash `ToolExecutionResult` — `Success`, `Failed`, `Cancelled`,
//! and `TimedOut` alike — is terminal with respect to the invocation-owned
//! process group. Linux also proves that no descendant can escape that
//! group. macOS has the same normal group cancellation path, but a command
//! that deliberately creates a new session is outside the group guarantee;
//! process-control failures remain explicit and are never treated as proof
//! of physical settlement.
//!
//! # Output capture
//!
//! stdout, stderr, and the runtime-observed combined multiplex are captured
//! separately. Full output is spooled to the conversation artifact store
//! while bounded previews (head/tail with an explicit truncation marker) are
//! retained for the model; the stored artifact bytes are never corrupted.
//! Non-zero exits are failed tool results with the exit code preserved —
//! never attempt-level runtime failures.
//!
//! Artifact capture failures (pipe reads, artifact allocation, artifact
//! open, writes, or the combined multiplex) are never silently discarded:
//! when no cancellation/timeout owns the outcome, a capture failure is an
//! explicit failed tool result, so the runtime never reports ordinary
//! success while silently losing the promised retained output. During a
//! cancellation/timeout settlement the terminated-process capture is
//! inherently partial, so the cancellation/timeout status wins.
//!

mod capture;
mod executor;
mod input;
// The supervisor is not a separate tool: it is an implementation detail of
// Bash execution ownership, owned by this module.
#[cfg(unix)]
#[doc(hidden)]
pub mod supervisor;
#[cfg(not(unix))]
#[doc(hidden)]
pub mod supervisor {
    /// The supervisor role names remain available to the dedicated binary,
    /// which reports the unsupported platform explicitly at runtime.
    pub const ROLE_OUTER: &str = "outer";
    pub const ROLE_INNER: &str = "inner";

    /// Unix is required for the process-group ownership proof used by the
    /// Bash supervisor.
    pub fn run_outer_supervisor() -> ! {
        eprintln!("Bash supervisor requires Unix process supervision");
        std::process::exit(1);
    }

    /// Unix is required for the process-group ownership proof used by the
    /// Bash supervisor.
    pub fn run_inner_supervisor() -> ! {
        eprintln!("Bash supervisor requires Unix process supervision");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests;

use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::types::ToolInvocationPolicy;

use input::BashInput;

#[cfg(test)]
pub(crate) use executor::BashTestControl;
pub(crate) use executor::BashTool;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "bash";

/// The tool-owned registration of the native Bash tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<BashInput>(
            "tool-bash",
            NAME,
            "Run one non-interactive /bin/bash command inside the workspace. No shell state \
             survives between calls. The optional timeout is in seconds.",
            policy,
        ),
        std::sync::Arc::new(BashTool::new()),
    )
}
