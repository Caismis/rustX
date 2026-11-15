//! Capability plane: snapshots and quiescent commits.
//!
//! Race semantics use exact synchronization points, never sleeps.

mod python_tools;
mod snapshots;

#[cfg(target_os = "linux")]
pub(crate) use snapshots::exercise_skill_catalog_without_environments;
