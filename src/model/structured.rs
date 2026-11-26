//! Frozen prepared-output payloads shared with the types contract crate.

pub use rustx_types::model::structured::*;

#[cfg(test)]
#[path = "structured_tests.rs"]
mod tests;
