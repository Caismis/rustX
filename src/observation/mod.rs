//! Native bounded observation contracts.
//!
//! The descriptor record vocabulary is owned by the durable journal plane
//! (`rustx_durable::observation`); this module re-exports it so existing
//! `crate::observation::*` paths keep resolving. The read sources that prepare
//! one bounded display head over a live native generation remain here.

pub mod capability_source;
#[cfg(test)]
mod content_tests;
pub mod resource_source;
pub(crate) mod workflow_source;

pub use rustx_durable::observation::{
    bounded, capture, content, cut, delivery, failure, goal, head, interaction, metadata,
    occupancy, preflight, statistics, status, structural_json, tool, trace, transcript, turn,
};
