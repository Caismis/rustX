//! Skill discovery, package metadata, version identity, and catalog rendering.
//!
//! # Ownership
//!
//! Skills are discovered instruction resources. They are not dependency
//! managers, execution environments, or permission grants. The skills plane
//! owns:
//!
//! - the session-level Skill **source policy**, source identity, and
//!   automatic root resolution (`source`);
//! - Skill discovery, per-candidate validation, same-scope conflict
//!   elimination, and `Workspace > User` merge
//!   (`package::SkillDiscovery`);
//! - typed generation-scoped Skill diagnostics and effective provenance
//!   (`diagnostics`);
//! - Agent Skills `SKILL.md` frontmatter parsing and validation
//!   (`package::SkillPackage`);
//! - deterministic normalized frontmatter definition hashing (`identity`);
//! - compact model-visible Skill catalog rendering (`catalog`).
//!
//! Generic `metadata`, including any `rustx.python-dependencies` or
//! `rustx.node-dependencies` strings, is uninterpreted descriptive text.
//! Discovery does not parse it as a dependency syntax, probe interpreters,
//! or write an environment. A malformed YAML document is still a malformed
//! document and excludes only that candidate.
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
mod diagnostics;
pub(crate) mod discovery;
mod frontmatter;
pub(crate) mod identity;
pub(crate) mod package;
pub mod source;

pub use catalog::{SkillCatalogEntry, SkillReadTool, SkillSnapshot, render_skill_catalog};
pub use diagnostics::{ShadowedSkill, SkillDiagnostic, SkillDiagnosticSeverity, SkillProvenance};
pub use package::{
    SkillDiscovery, SkillDiscoveryConfig, SkillDiscoveryOutcome, SkillPackage, SkillPackageError,
};
pub use source::{
    AutomaticSkillRoot, SKILLS_DIRECTORY, SKILLS_ROOT, SkillSource, automatic_skill_roots,
};

#[cfg(test)]
mod tests;
