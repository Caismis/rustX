//! Boundary conformance for the durable authority: real process death.
//!
//! The harness spawns this same test binary as a real child process, arms it
//! to freeze at deterministic gates, ends it with SIGKILL, and then proves
//! what the durable store recovered. No scripted seam can prove OS process
//! lifecycle, so this suite is boundary conformance even though it compiles
//! in-crate for the child entry point seam.

pub(crate) mod process_death;
