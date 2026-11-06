//! Native-owned bounded auxiliary execution.
pub(crate) mod authority;
pub mod capacity;
mod execution;
pub(crate) mod executor;
pub(crate) mod output;
pub mod stream;
mod summary;
pub mod writer;

pub(crate) mod classification;
pub mod delivery;
pub mod submission;

pub mod observer;
