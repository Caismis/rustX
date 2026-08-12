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
//! A Bash invocation executes inside one fixed rustX-owned process group.
//! Process-group/session mutation from Bash descendants is rejected so the
//! ownership boundary cannot be escaped or partially hidden: the inner
//! supervisor installs a narrow inherited seccomp policy between its own
//! `setsid()` setup and the `/bin/bash` spawn that rejects `setsid(2)` and
//! `setpgid(2)` with `EPERM` (see [`supervisor`]). `setsid`/`setpgid`
//! are the only syscalls that can change process-group/session membership
//! on Linux, and seccomp filters are inherited across `fork`/`exec` and can
//! only become more restrictive. A command such as `setsid sleep 30`
//! therefore fails deterministically (the utility exits non-zero) and
//! nothing leaves the invocation group.
//!
//! This restriction is what makes the supervisor's kernel child-wait
//! terminal proof complete: an in-domain descendant cannot remain hidden
//! behind an ancestor that left the domain. See the "Ownership boundary"
//! section below and [`supervisor`] for the full argument.
//!
//! # Ownership boundary
//!
//! The Bash invocation's ownership boundary is its dedicated process
//! group. The invocation owns, guarantees termination of, and bases its
//! settlement on exactly the processes that remain in that group. Because
//! group membership is immutable for bash descendants, every process ever
//! spawned by the shell — background children, subshells, replacement
//! processes — remains in the invocation group for its whole lifetime:
//! **there is no way to leave the owned execution domain from inside a
//! Bash command**.
//!
//! # Runtime child-subreaper capability
//!
//! rustX's process-wide `PR_SET_CHILD_SUBREAPER` activation is a runtime
//! coordination-layer capability, not a Bash-local setting and not a
//! generic reaper: it is owned by
//! [`crate::runtime::process_supervision`], activated lazily once,
//! idempotently and sticky, before any Bash ownership exists (before
//! `START` authorizes the Bash spawn), and never toggled per invocation.
//! It exists solely so that a lost Bash supervisor unit's orphaned
//! invocation descendants reparent to the runtime process, where the
//! invocation-scoped catastrophic containment can still retain the inner
//! anchor and prove the invocation group terminal. Kernel reparenting does
//! not expand Bash semantic ownership beyond the invocation process group,
//! and rustX implements no generic unknown-child reaper: catastrophic
//! containment remains invocation-scoped (anchor pid and invocation
//! process group only — never a broad wait).
//!
//! # Terminal results
//!
//! Every Bash `ToolExecutionResult` — `Success`, `Failed`, `Cancelled`,
//! and `TimedOut` alike — is terminal with respect to the invocation-owned
//! process group: no invocation-owned Bash process remains capable of
//! executing work before any result is returned. A detected
//! process-control/runtime failure determines the eventual result status
//! but does not itself settle the invocation lifecycle: owned work is
//! contained and the owned group reaped to either the normal outer terminal
//! event or the reuse-safe catastrophic terminal point (and the capture
//! settled) before the remembered `Failed` result is returned.
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
pub fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<BashInput>(
            "tool-bash",
            NAME,
            "Run one non-interactive /bin/bash command inside the workspace with an explicit \
             environment and supervised process ownership.",
            policy,
        ),
        std::sync::Arc::new(BashTool::new()),
    )
}
