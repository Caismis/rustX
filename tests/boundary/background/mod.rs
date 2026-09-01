//! Boundary conformance for background execution: real bash processes run
//! through the actual `bash-supervisor` binary.
//!
//! The deterministic registry/routing contracts stay in
//! `scripted_suites::background`; only suites whose invariant is real shell
//! execution live here.

mod text_spill;
