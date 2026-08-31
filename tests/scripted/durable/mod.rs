//! Real process-death conformance for the durable runtime invariants.
//!
//! The store-contract suites treat "the process died" as `drop(store)`;
//! [`process_death`] kills a real process running the real runtime stack at
//! a named durable boundary and asserts the exact allowed/forbidden durable
//! state after real recovery. No sleep, no poll, no timing race: the child
//! is either parked inside a `cfg(test)` process-death boundary or blocked
//! on a control rendezvous.

mod process_death;
