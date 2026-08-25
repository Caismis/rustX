//! The test-only real-process-death seam (Issue #111, FND-06).
//!
//! FND-06 proves the durable invariants of the earlier FND issues against an
//! actual `SIGKILL` of an actual process running the actual runtime stack.
//! Doing that deterministically needs exactly one capability that ordinary
//! runtime code cannot provide: the ability to **freeze a live process at a
//! named durable linearization point** and tell the owning test that the
//! process is frozen there.
//!
//! ```text
//! parent test process
//!   -> spawns a child running the real runtime stack
//!   -> child reaches one named boundary, announces it, and parks
//!   -> parent SIGKILLs the child
//!   -> parent reopens the durable authority and recovers
//! ```
//!
//! # What a boundary is
//!
//! A boundary is a *durable* linearization point, never a wall-clock moment:
//!
//! ```text
//! before:<transition>   the durable transaction is open and has committed
//!                       nothing; the store connection lock is held
//! after:<transition>    the durable transaction has committed; the store
//!                       connection lock is still held
//! ```
//!
//! Both variants park while the `SqliteConversationStore` connection mutex is
//! held, so a parked child cannot commit *anything* durable from any other
//! thread while the parent decides to kill it. "Killed before P" therefore
//! means the durable authority provably contains no P, not that the test hoped
//! the kill won a race.
//!
//! Two durable planes outside the conversation store use the same seam with
//! their own exclusion, because the fact they linearize is not a SQLite
//! transaction:
//!
//! ```text
//! reload:prepared / reload:published
//!     the resource reload's build/publish boundary, under the runtime's own
//!     one-reload-at-a-time gate
//!
//! before/after:publish_session, before/after:publish_node
//!     the native Session catalog's visibility commit — the atomic rename that
//!     makes a lineage the active one — under the supervisor state mutex the
//!     whole publish operation holds
//! ```
//!
//! In each case the parked thread owns the only path that can advance that
//! plane, so the durable world is frozen for the same reason.
//!
//! # Why this is a `cfg(test)` seam
//!
//! Parking a production process at a durable boundary is not a product
//! capability, and a runtime that can be frozen by an environment variable is
//! not a runtime anyone should ship. The whole mechanism therefore exists only
//! in this crate's own test build: in every other build the two entry points
//! are empty `const fn`s that the compiler removes entirely, and the
//! environment variables below are read by nothing.
//!
//! This is also why the FND-06 child process is the crate's **own test
//! binary** re-executed in child mode rather than the `rustx` binary: the
//! child needs both the real runtime stack and the `cfg(test)`-only seams
//! (this one and the scripted provider adapter), exactly like the in-crate
//! scripted suites described in `tests/scripted/mod.rs`. The same
//! re-execute-the-test-binary pattern already backs the M7 MCP stdio fixture in
//! `crate::tools::mcp::fixture`.
//!
//! # Control channel
//!
//! The child inherits `RUSTX_FND06_CONTROL`, the path of a `UnixListener` the
//! parent owns. The child connects once and both the gate and the child-side
//! scenario driver speak newline-delimited JSON over that one connection, so
//! boundary announcements and scenario steps are totally ordered on a single
//! stream and never interleave with the test harness's own stdout.

/// The boundary key one child parks at, for example
/// `after:event:model_request_completed`. Unset means "never park".
#[cfg(test)]
pub(crate) const GATE_ENV: &str = "RUSTX_FND06_GATE";

/// The 1-based occurrence of [`GATE_ENV`] the child parks at. Unset means the
/// first occurrence.
#[cfg(test)]
pub(crate) const GATE_NTH_ENV: &str = "RUSTX_FND06_GATE_NTH";

/// The path of the parent-owned `UnixListener` control socket.
#[cfg(test)]
pub(crate) const CONTROL_ENV: &str = "RUSTX_FND06_CONTROL";

#[cfg(test)]
pub(crate) use imp::{orphan_watchdog, reach, reach_event, recv_line, send_line};

/// Parks the calling thread at `boundary` when the process was started to die
/// there. Compiled away outside this crate's test build.
#[cfg(not(test))]
pub(crate) const fn reach(_boundary: &str) {}

/// Parks the calling thread at `"{prefix}:{event type}"` when the process was
/// started to die there. Compiled away outside this crate's test build.
#[cfg(not(test))]
pub(crate) const fn reach_event(_prefix: &str, _event: &crate::events::types::RuntimeEvent) {}

#[cfg(test)]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use crate::events::types::RuntimeEvent;

    use super::{CONTROL_ENV, GATE_ENV, GATE_NTH_ENV};

    /// The one control connection back to the owning parent test.
    struct Control {
        writer: Mutex<UnixStream>,
        reader: Mutex<BufReader<UnixStream>>,
    }

    /// The boundary this process was started to die at.
    struct Gate {
        boundary: String,
        nth: usize,
        seen: AtomicUsize,
    }

    fn control() -> Option<&'static Control> {
        static CONTROL: OnceLock<Option<Control>> = OnceLock::new();
        CONTROL
            .get_or_init(|| {
                let path = std::env::var_os(CONTROL_ENV)?;
                let stream = UnixStream::connect(path).expect("FND-06 control socket connects");
                let reader = stream
                    .try_clone()
                    .expect("FND-06 control socket clones for reading");
                Some(Control {
                    writer: Mutex::new(stream),
                    reader: Mutex::new(BufReader::new(reader)),
                })
            })
            .as_ref()
    }

    fn gate() -> Option<&'static Gate> {
        static GATE: OnceLock<Option<Gate>> = OnceLock::new();
        GATE.get_or_init(|| {
            let boundary = std::env::var(GATE_ENV).ok()?;
            let nth = std::env::var(GATE_NTH_ENV)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1);
            Some(Gate {
                boundary,
                nth,
                seen: AtomicUsize::new(0),
            })
        })
        .as_ref()
    }

    /// Writes one newline-delimited control line to the owning parent.
    ///
    /// # Panics
    ///
    /// Panics when the child was started without a control socket, or when the
    /// parent closed it early: both mean the harness itself is broken.
    pub(crate) fn send_line(line: &str) {
        let control = control().expect("a FND-06 child owns a control socket");
        let mut writer = control.writer.lock().expect("FND-06 control writer lock");
        writeln!(writer, "{line}").expect("FND-06 control line writes");
        writer.flush().expect("FND-06 control line flushes");
    }

    /// Blocks until the owning parent sends one control line, or returns
    /// `None` once the parent closed the socket.
    ///
    /// # Panics
    ///
    /// Panics when the child was started without a control socket.
    pub(crate) fn recv_line() -> Option<String> {
        let control = control().expect("a FND-06 child owns a control socket");
        let mut reader = control.reader.lock().expect("FND-06 control reader lock");
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end().to_owned()),
        }
    }

    /// Announces `boundary` to the owning parent and parks forever.
    ///
    /// The caller is holding the durable store's connection mutex, so the
    /// whole durable plane of this process is frozen from here until the
    /// parent's `SIGKILL` arrives.
    ///
    /// A parked child must never outlive its owner. The parking thread cannot
    /// observe that, so a watchdog thread holds the control reader and ends
    /// this process the moment the socket reports end of file — which happens
    /// only when the owning parent is gone.
    fn park_at(boundary: &str) -> ! {
        send_line(&serde_json::json!({"kind": "reached", "boundary": boundary}).to_string());
        std::thread::spawn(orphan_watchdog);
        loop {
            std::thread::park();
        }
    }

    /// Ends this process when its owning parent closes the control channel.
    pub(crate) fn orphan_watchdog() -> ! {
        while recv_line().is_some() {}
        std::process::exit(0)
    }

    pub(crate) fn reach(boundary: &str) {
        let Some(gate) = gate() else { return };
        if gate.boundary != boundary {
            return;
        }
        if gate.seen.fetch_add(1, Ordering::SeqCst) + 1 != gate.nth {
            return;
        }
        park_at(boundary);
    }

    pub(crate) fn reach_event(prefix: &str, event: &RuntimeEvent) {
        let Some(gate) = gate() else { return };
        // The gate is armed for at most one boundary, so the event type is
        // only ever rendered when this process might actually park.
        if !gate.boundary.starts_with(prefix) {
            return;
        }
        reach(&format!("{prefix}:{}", event_type(event)));
    }

    /// The wire discriminant of one runtime event, which is also its boundary
    /// name: boundary keys are derived from the durable event contract instead
    /// of a second hand-maintained vocabulary.
    fn event_type(event: &RuntimeEvent) -> String {
        serde_json::to_value(event)
            .ok()
            .and_then(|value| {
                value
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| "unknown".to_owned())
    }
}
