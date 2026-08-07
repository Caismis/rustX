//! rustX runtime library.
//!
//! The crate is intentionally layered around runtime-owned contracts. External
//! SDK types must terminate at adapter boundaries and must not leak into the
//! agent kernel.

pub mod agent;
pub mod context;
pub mod events;
pub mod message;
pub mod model;
pub mod protocol;
pub mod runtime;
pub mod skills;
pub mod tools;
