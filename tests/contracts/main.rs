//! Pure serialization and configuration contract tests.
//!
//! Wire-contract fixtures round-trip through the typed domain values, and
//! the committed configuration examples stay loadable and documented. No
//! runtime, no provider, no process: these suites pin the deterministic
//! serialization contract only.

mod protocol_fixtures;
mod runtime_examples;
