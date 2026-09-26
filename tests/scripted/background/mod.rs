//! Finite background Job ownership and domain-specific controls.
//!
//! Registry tests prove admission, cancellation/settlement, output and exactly-once
//! notification. Job control tests prove snapshot, exact watch waits and isolation.
//! Real process output/settlement belongs to `boundary_suites::background`; durable
//! Agent identity and message arbitration belong to the Subagent owner tests.

mod jobs;
mod registry;
