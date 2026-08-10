//! The compact model-visible Skill catalog (M6).
//!
//! The catalog is rendered deterministically from the attempt's immutable
//! Skill snapshot. Each entry contains only the validated standard `name`
//! and `description`; the common `.agents/skills/` root is declared once;
//! host absolute paths, `SKILL.md` bodies, supporting resources, and
//! dependency metadata never appear.
//!
//! The catalog is capability projection, never canonical conversation
//! history: it is carried by a provider-independent
//! `SkillCatalogAttachment` through projection/fingerprint/token
//! accounting and placed by provider adapters in trusted system context.

use std::sync::Arc;

use crate::protocol::manifest::SkillBinding;
use crate::skills::package::SkillPackage;

/// One model-visible Skill catalog entry: standard `name` + `description`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillCatalogEntry {
    /// The validated standard Skill name.
    pub name: String,
    /// The validated standard Skill description.
    pub description: String,
}

/// The immutable Skill snapshot of one capability set.
///
/// The snapshot holds the accepted Skill packages, the deterministically
/// ordered model-visible catalog entries, and the deterministic
/// `SkillId` + `SkillVersionId` bindings. It is constructed once per
/// candidate preparation and never mutated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSnapshot {
    packages: Vec<Arc<SkillPackage>>,
    catalog: Vec<SkillCatalogEntry>,
    bindings: Vec<SkillBinding>,
}

impl SkillSnapshot {
    /// Builds the immutable snapshot from the accepted packages, ordering
    /// everything deterministically by validated Skill name.
    #[must_use]
    pub fn new(packages: Vec<Arc<SkillPackage>>) -> Self {
        let mut packages = packages;
        packages.sort_by(|left, right| left.id().cmp(right.id()));
        let catalog = packages
            .iter()
            .map(|package| SkillCatalogEntry {
                name: package.name().to_owned(),
                description: package.description().to_owned(),
            })
            .collect();
        let bindings = packages
            .iter()
            .map(|package| SkillBinding {
                skill_id: package.id().clone(),
                version_id: package.version_id().clone(),
            })
            .collect();
        Self {
            packages,
            catalog,
            bindings,
        }
    }

    /// The accepted packages, deterministically ordered by Skill name.
    #[must_use]
    pub fn packages(&self) -> &[Arc<SkillPackage>] {
        &self.packages
    }

    /// The model-visible catalog entries, deterministically ordered by
    /// Skill name.
    #[must_use]
    pub fn catalog_entries(&self) -> &[SkillCatalogEntry] {
        &self.catalog
    }

    /// The deterministic `SkillId` + `SkillVersionId` bindings, ordered by
    /// Skill name.
    #[must_use]
    pub fn bindings(&self) -> &[SkillBinding] {
        &self.bindings
    }

    /// The rendered catalog text, or `None` when no Skill is active (the
    /// caller omits the attachment entirely).
    #[must_use]
    pub fn render_catalog(&self) -> Option<String> {
        if self.catalog.is_empty() {
            return None;
        }
        Some(render_skill_catalog(&self.catalog))
    }
}

/// Renders the compact `## Skills` catalog deterministically.
///
/// The rendered form declares the common `.agents/skills/` root once and
/// lists each Skill as `- <name>: <description>` in deterministic sorted
/// order. No `SKILL.md` body, supporting resource, dependency metadata, or
/// host absolute path ever appears.
#[must_use]
pub fn render_skill_catalog(entries: &[SkillCatalogEntry]) -> String {
    let mut out = String::from(
        "## Skills\n\n\
         Skills are stored under `.agents/skills/`.\n\
         Before using a skill, read `.agents/skills/<skill-name>/SKILL.md`\n\
         with the Read tool.\n\n\
         Available skills:\n",
    );
    for entry in entries {
        out.push_str(&format!("\n- {}: {}", entry.name, entry.description));
    }
    out
}
