//! Durable store suites whose assertions exercise the runtime-client evidence
//! layer (`runtime_client`), provider adapters, or monolith context resources.
//! They live in the lib test binary with the rest of `scripted_suites` because
//! they need `pub(crate)` seams on both crates.
pub use crate::durable::sqlite::test_support::*;
pub use crate::durable::sqlite::*;

pub mod auxiliary_supervision;
pub mod compaction_resources_tests;
pub mod model_snapshot_tests;
pub mod observation;
pub mod projection_store_tests;
pub mod projection_tests;
pub mod request_tests;
pub mod workflow_trace_tests;
