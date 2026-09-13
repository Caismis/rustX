//! Skill discovery, package metadata, dependency declarations, version
//! identity, catalog rendering, and shared Python/Node environments (M6).
//!
//! # Ownership
//!
//! The skills plane owns:
//!
//! - the session-level Skill **source policy**, source identity, and
//!   automatic root resolution (`source`);
//! - Skill discovery, per-candidate validation, same-scope conflict
//!   elimination, and `explicit > workspace > global` merge
//!   (`package::SkillDiscovery`);
//! - typed generation-scoped Skill diagnostics and effective provenance
//!   (`diagnostics`);
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
mod diagnostics;
pub mod environments;
pub(crate) mod identity;
pub mod materialization;
pub(crate) mod package;
pub mod source;

pub(crate) use catalog::admitted_skill_entries;
pub use catalog::{SkillCatalogEntry, SkillSnapshot, render_skill_catalog};
pub use dependencies::{
    DependencyConflict, DependencyError, DependencyManifest, Ecosystem, merge_dependency_manifests,
    parse_node_dependencies, parse_python_dependencies,
};
pub use diagnostics::{ShadowedSkill, SkillDiagnostic, SkillDiagnosticSeverity, SkillProvenance};
pub use environments::{
    ENVIRONMENT_MANIFEST_FILE, EnvironmentPreparationError, EnvironmentStore, NodeEnvironment,
    PythonEnvironment, RuntimeVersions, SkillEnvironmentBackend, node_environment_digest,
    python_environment_digest,
};
pub use package::{
    SkillDiscovery, SkillDiscoveryConfig, SkillDiscoveryError, SkillDiscoveryOutcome, SkillPackage,
    SkillPackageError,
};
pub use source::{
    AutomaticSkillRoot, AutomaticSkillSource, SKILLS_DIRECTORY, SKILLS_ROOT, SkillSource,
    automatic_skill_roots, default_automatic_sources,
};
