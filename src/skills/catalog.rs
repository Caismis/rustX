//! The compact model-visible Skill catalog (M6).
//!
//! The catalog is rendered deterministically from the attempt's immutable
//! Skill snapshot. Each entry contains only the validated standard `name`,
//! `description`, and the host path of the package's `SKILL.md`. `SKILL.md`
//! bodies, supporting resources, and dependency metadata never appear in the
//! catalog.
//!
//! The published location is a real host path, not a runtime-owned virtual
//! spelling. A Skill package is an ordinary directory whose `SKILL.md`
//! references its own scripts, references, and assets relatively; the model
//! resolves those references against the package directory and reaches them
//! through the same native Read, Bash, Grep, and Glob semantics as any other
//! file. A virtual namespace would be understood by Read alone and would make
//! every Bash-executed Skill resource unreachable.
//!
//! The catalog is an immutable capability snapshot. Its rendered guidance
//! enters the request-time Effective System Prompt through Context Assembly;
//! it is not canonical conversation history and is not carried by a
//! provider-request special channel.

use std::sync::Arc;

use crate::protocol::manifest::SkillBinding;
use crate::skills::package::{SKILL_MARKDOWN_FILE, SkillPackage};

/// One model-visible Skill catalog entry: standard metadata plus the host
/// location of the primary instructions file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillCatalogEntry {
    /// The validated standard Skill name.
    pub name: String,
    /// The validated standard Skill description.
    pub description: String,
    /// The host path of the package's `SKILL.md`. The model passes it to
    /// Read, and resolves the Skill's own relative references against its
    /// parent directory.
    pub location: String,
}

/// The immutable Skill snapshot of one capability set.
///
/// The snapshot holds the accepted Skill packages, the deterministically
/// ordered catalog metadata entries after Skill-level invocation filtering,
/// and the deterministic `SkillId` + `SkillVersionId` bindings. The entries
/// are the one Skill-level model-visible set used by capability projections.
/// It is constructed once per candidate preparation and never mutated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSnapshot {
    packages: Vec<Arc<SkillPackage>>,
    catalog: Vec<SkillCatalogEntry>,
    bindings: Vec<SkillBinding>,
    visible_bindings: Vec<SkillBinding>,
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
            .map(|package| SkillCatalogEntry {
                name: package.name().to_owned(),
                description: package.description().to_owned(),
                location: skill_markdown_location(package),
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
        Self {
            packages,
            catalog,
            bindings,
            visible_bindings,
        }
    }

    /// The accepted packages, deterministically ordered by Skill name.
    #[must_use]
    pub fn packages(&self) -> &[Arc<SkillPackage>] {
        &self.packages
    }

    /// The catalog metadata entries that pass Skill-level
    /// `disable-model-invocation` filtering, deterministically ordered by
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

    /// The bindings corresponding exactly to the Skill-level visible catalog
    /// metadata.
    #[must_use]
    pub fn visible_bindings(&self) -> &[SkillBinding] {
        &self.visible_bindings
    }

    /// The host `SKILL.md` locations of every accepted package, ordered by
    /// Skill name. Unlike the catalog, this covers packages hidden by
    /// `disable-model-invocation: true`.
    #[must_use]
    pub fn locations(&self) -> Vec<String> {
        self.packages
            .iter()
            .map(|package| skill_markdown_location(package))
            .collect()
    }

    /// Whether two snapshots have the same execution-semantic Skill state.
    ///
    /// Skill identity/version bindings describe package provenance, while the
    /// published locations describe where the admitted packages currently
    /// live. Both facts are required for rediscovery to be a no-op: identical
    /// package content moved to another current root must replace the active
    /// snapshot rather than leave the catalog pointing at the old host path.
    #[must_use]
    pub fn semantically_equivalent(&self, other: &Self) -> bool {
        self.bindings == other.bindings
            && self.visible_bindings == other.visible_bindings
            && self.catalog == other.catalog
            && self.locations() == other.locations()
    }
}

/// The host path of one package's `SKILL.md`, in the host's own spelling.
///
/// The path is published verbatim: the model hands it straight back to Read
/// and Bash, so rewriting separators would produce a path the host never
/// named.
fn skill_markdown_location(package: &SkillPackage) -> String {
    package
        .root()
        .join(SKILL_MARKDOWN_FILE)
        .to_string_lossy()
        .into_owned()
}

/// Renders the compact `## Skills` catalog deterministically.
///
/// The rendered form gives each Skill its host `SKILL.md` location in
/// deterministic sorted order. No `SKILL.md` body, supporting resource, or
/// dependency metadata ever appears.
#[must_use]
pub fn render_skill_catalog(entries: &[SkillCatalogEntry]) -> String {
    let mut out = String::from(
        "## Skills\n\n\
         The following skills provide specialized instructions for specific tasks.\n\
         Use the Read tool to load a skill when the task matches its description.\n\
         When a skill file references a relative path, resolve it against the skill \
         directory (the parent of its SKILL.md) and use that absolute path in tool \
         commands.\n\n\
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
/// Skill names and locations are validated/host-derived, but descriptions
/// are accepted metadata and must not be able to change the catalog shape.
fn escape_catalog_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
