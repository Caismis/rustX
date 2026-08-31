//! Issue #111 (FND-06) — real process-death conformance for the durable
//! runtime invariants established by FND-01 … FND-05.
//!
//! # What makes this suite different
//!
//! The existing recovery suites treat "the process died" as `drop(store)`.
//! That proves the *store* contract, and nothing about a live runtime whose
//! threads, tool executions, publication streams, resource generations, and
//! background children all vanish at once. This suite kills a **real
//! process**:
//!
//! ```text
//! parent test
//!   -> spawns a child running the real runtime stack over a real durable file
//!   -> child reaches one named durable boundary and freezes there
//!   -> parent SIGKILLs the child's whole process group
//!   -> parent reopens the durable authority and runs real recovery
//!   -> parent asserts the exact allowed/forbidden durable state
//! ```
//!
//! # Determinism
//!
//! No assertion in this suite depends on a sleep, a poll, or a timing race.
//! Every kill happens at one of two provable states:
//!
//! - a **durable boundary**: the child is parked inside
//!   [`process_death::reach`](crate::runtime::process_death), holding the
//!   durable store's connection mutex, so the whole process is incapable of
//!   committing anything else;
//! - a **control rendezvous**: the child announced a fact over its control
//!   socket and is blocked reading the next line, so it is not executing
//!   anything at all.
//!
//! The only wall-clock values anywhere are outer liveness guards
//! ([`harness::LIVENESS`]) whose expiry is a harness failure, never a
//! conformance verdict.
//!
//! # Why the child is this test binary
//!
//! The child must run the real `ConversationRuntime`, the real durable store,
//! the real Tool Plane, the real publication plane, and the real filesystem
//! resource loader — while also using two `cfg(test)`-only seams: the
//! process-death boundaries and the scripted provider adapter. Neither seam
//! may exist in the published API (see `tests/scripted/mod.rs`), so the child
//! is this crate's own test binary re-executed in child mode, exactly like the
//! M7 MCP stdio fixture in [`crate::tools::mcp::fixture`]. Everything below
//! the seam — composition, admission, the Agent Loop, durability — is the same
//! code the `rustx` binary runs.
//!
//! # Layout
//!
//! - [`harness`] — the parent side: the lab directory, the child process, the
//!   control channel, the durable reopen/recovery view.
//! - [`child`] — the child side: the scenarios that drive the real stack to a
//!   boundary.
//! - [`conformance`] — the boundary matrix itself, one test per proven
//!   invariant, mirroring `docs/process-death-conformance.md`.

pub(crate) mod child;
pub(crate) mod conformance;
pub(crate) mod harness;

/// The environment variable that selects a child scenario. Its absence means
/// "this is an ordinary test run", so the child entry point below is inert.
pub(crate) const SCENARIO_ENV: &str = "RUSTX_FND06_SCENARIO";

/// The environment variable carrying the lab root a child runs against.
pub(crate) const ROOT_ENV: &str = "RUSTX_FND06_ROOT";

/// The libtest path of [`fnd06_child_runtime_process`], used to re-execute
/// this binary in child mode.
pub(crate) const CHILD_TEST: &str =
    "scripted_suites::durable::process_death::fnd06_child_runtime_process";

/// The one child entry point of this suite.
///
/// In an ordinary test run there is no scenario in the environment and this
/// test does nothing. When [`harness::Lab::spawn`] re-executes this binary it
/// selects exactly this test, the scenario is present, and the process becomes
/// a real runtime child that never returns: it is always ended by its parent's
/// `SIGKILL`.
#[test]
fn fnd06_child_runtime_process() {
    let Ok(scenario) = std::env::var(SCENARIO_ENV) else {
        return;
    };
    child::run(&scenario);
}
