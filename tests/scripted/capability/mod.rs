//! Capability plane: snapshots, quiescent commits, and materialization.
//!
//! Environment materialization uses the deterministic fake backend
//! (`common::FakeSkillEnvironmentBackend`); race semantics use exact
//! synchronization points, never sleeps.

mod python_tools;
mod snapshots;
