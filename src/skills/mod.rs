//! Skill discovery, package metadata, dependency declarations, version
//! identity, catalog rendering, and shared Python/Node environments (M6).
//!
//! # Ownership
//!
//! The skills plane owns:
//!
//! - Skill discovery from current user/global, project, configured, and CLI
//!   roots (`package::SkillDiscovery`);
//! - Agent Skills `SKILL.md` frontmatter parsing and validation
//!   (`package::SkillPackage`);
//! - deterministic content-derived package/version hashing (`identity`);
//! - rustX dependency declaration parsing, normalization, and
//!   merge/conflict detection (`dependencies`);
//! - Python/Node environment identity and materialization
//!   (`environments`);
//! - compact model-visible Skill catalog rendering (`catalog`).
//!
//! Candidate preparation callers wait on an `EnvironmentStore`-owned
//! `(ecosystem, digest)` build task. Caller cancellation stops only that
//! caller's wait; the physical materialization remains owned until its
//! supervised subprocess hierarchy settles and publication returns.
//!
//! Skills remain workflow/instruction packages: they are not tools and not
//! a parallel execution protocol. Skill bodies and supporting files are
//! current filesystem resources reached through ordinary native tool
//! semantics at their host paths; they do not create a second execution
//! protocol or a second path namespace. The capability coordination layer
//! (`crate::capabilities`) owns the immutable capability snapshot, attempt
//! leases, and quiescent commit; it consumes this plane but never vice
//! versa.

mod catalog;
mod dependencies;
pub mod environments;
mod identity;
mod package;

pub use catalog::{SkillCatalogEntry, SkillSnapshot, render_skill_catalog};
pub use dependencies::{
    DependencyConflict, DependencyError, DependencyManifest, Ecosystem, merge_dependency_manifests,
    parse_node_dependencies, parse_python_dependencies,
};
pub use environments::{
    ENVIRONMENT_MANIFEST_FILE, EnvironmentPreparationError, EnvironmentStore, NodeEnvironment,
    PythonEnvironment, RuntimeVersions, SkillEnvironmentBackend, node_environment_digest,
    python_environment_digest,
};
pub use package::{
    RUSTX_SKILLS_DIRECTORY, SkillDiscovery, SkillDiscoveryConfig, SkillPackage, SkillPackageError,
};
