//! The compact model-visible Skill catalog (M6).
//!
//! The catalog is rendered deterministically from the attempt's immutable
//! Skill snapshot. Each entry contains only the validated standard `name`,
//! `description`, and its exact runtime-owned virtual `SKILL.md` location;
//! host absolute paths, `SKILL.md` bodies, supporting resources, and dependency
//! metadata never appear in the catalog. The snapshot separately owns the
//! runtime-controlled resource map used by native Read.
//!
//! The catalog is an immutable capability snapshot. Its rendered guidance
//! enters the request-time Effective System Prompt through Context Assembly;
//! it is not canonical conversation history and is not carried by a
//! provider-request special channel.

use std::path::Path;
use std::sync::Arc;

use crate::protocol::manifest::SkillBinding;
use crate::skills::package::{
    SKILL_MARKDOWN_FILE, SkillPackage, SkillResourceMap, virtual_skill_resource_location,
    virtual_skill_resource_path,
};

/// One model-visible Skill catalog entry: standard metadata plus the exact
/// runtime-owned virtual location of the primary instructions file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillCatalogEntry {
    /// The validated standard Skill name.
    pub name: String,
    /// The validated standard Skill description.
    pub description: String,
    /// The exact virtual location the model passes to native Read.
    pub location: String,
}

/// The immutable Skill snapshot of one capability set.
///
/// The snapshot holds the accepted Skill packages, the deterministically
/// ordered catalog metadata entries after Skill-level invocation filtering,
/// and the deterministic `SkillId` + `SkillVersionId` bindings. The
/// capability layer adds active native Read eligibility before exposing these
/// entries as model-visible. It is constructed once per candidate preparation
/// and never mutated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSnapshot {
    packages: Vec<Arc<SkillPackage>>,
    catalog: Vec<SkillCatalogEntry>,
    bindings: Vec<SkillBinding>,
    visible_bindings: Vec<SkillBinding>,
    resources: SkillResourceMap,
}

impl SkillSnapshot {
    /// Builds the immutable snapshot from the accepted packages, ordering
    /// everything deterministically by validated Skill name.
    #[must_use]
    pub fn new(packages: Vec<Arc<SkillPackage>>) -> Self {
        let mut packages = packages;
        packages.sort_by(|left, right| left.name().cmp(right.name()));
        let catalog = packages
            .iter()
            .filter(|package| !package.disable_model_invocation())
            .map(|package| {
                let location =
                    virtual_skill_resource_location(package.name(), Path::new(SKILL_MARKDOWN_FILE));
                SkillCatalogEntry {
                    name: package.name().to_owned(),
                    description: package.description().to_owned(),
                    location,
                }
            })
            .collect();
        let bindings = packages
            .iter()
            .map(|package| SkillBinding {
                skill_id: package.id().clone(),
                version_id: package.version_id().clone(),
            })
            .collect();
        let visible_bindings = packages
            .iter()
            .filter(|package| !package.disable_model_invocation())
            .map(|package| SkillBinding {
                skill_id: package.id().clone(),
                version_id: package.version_id().clone(),
            })
            .collect();
        let mut resources = SkillResourceMap::default();
        for package in &packages {
            for file in package.files() {
                resources.entries.insert(
                    virtual_skill_resource_path(package.name(), file),
                    package.root().join(file),
                );
            }
        }
        Self {
            packages,
            catalog,
            bindings,
            visible_bindings,
            resources,
        }
    }

    /// The accepted packages, deterministically ordered by Skill name.
    #[must_use]
    pub fn packages(&self) -> &[Arc<SkillPackage>] {
        &self.packages
    }

    /// The catalog metadata entries that pass Skill-level
    /// `disable-model-invocation` filtering, deterministically ordered by
    /// Skill name. [`crate::capabilities::CapabilitySnapshot`] applies the
    /// active native Read predicate before treating them as model-visible.
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

    /// The bindings corresponding exactly to the Skill-level visible catalog
    /// metadata. The capability layer applies active native Read eligibility
    /// together with these bindings for model-facing projections.
    #[must_use]
    pub fn visible_bindings(&self) -> &[SkillBinding] {
        &self.visible_bindings
    }

    /// The runtime-owned virtual-to-host resource map used by Read.
    #[must_use]
    pub fn resources(&self) -> &SkillResourceMap {
        &self.resources
    }

    /// Whether two snapshots have the same execution-semantic Skill state.
    ///
    /// Skill identity/version bindings describe package provenance, while the
    /// resource map describes where the admitted virtual files resolve for
    /// the current runtime. Both facts are required for rediscovery to be a
    /// no-op: identical package content moved to another current root must
    /// replace the active snapshot rather than leave Read pointing at the old
    /// host path.
    #[must_use]
    pub fn semantically_equivalent(&self, other: &Self) -> bool {
        self.bindings == other.bindings
            && self.visible_bindings == other.visible_bindings
            && self.catalog == other.catalog
            && self.resources == other.resources
    }
}

/// Renders the compact `## Skills` catalog deterministically.
///
/// The rendered form gives each Skill its exact virtual location in
/// deterministic sorted order. No `SKILL.md` body, supporting resource,
/// dependency metadata, or host absolute path ever appears.
#[must_use]
pub fn render_skill_catalog(entries: &[SkillCatalogEntry]) -> String {
    let mut out = String::from(
        "## Skills\n\n\
         The following skills provide specialized instructions for specific tasks.\n\
         Use the Read tool to load a skill when the task matches its description.\n\
         Use the exact location shown below; do not construct or rewrite Skill paths.\n\n\
         <available_skills>\n",
    );
    for entry in entries {
        use std::fmt::Write as _;
        let name = escape_catalog_text(&entry.name);
        let description = escape_catalog_text(&entry.description);
        let location = escape_catalog_text(&entry.location);
        let _ = write!(
            out,
            "  <skill>\n    <name>{name}</name>\n    <description>{description}</description>\n    <location>{location}</location>\n  </skill>\n"
        );
    }
    out.push_str("</available_skills>");
    out
}

/// Escapes text placed inside the compact XML-shaped catalog representation.
/// Skill names and locations are validated/runtime-derived, but descriptions
/// are accepted metadata and must not be able to change the catalog shape.
fn escape_catalog_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
