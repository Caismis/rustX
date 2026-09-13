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
//! The catalog never derives that path: [`crate::skills::SkillPackage`]
//! establishes one canonical absolute UTF-8 location at discovery time (see
//! its package root invariant), and the catalog is only a projection of it.
//! Deriving it here instead would let a non-canonical package root reach the
//! model as a path that resolves differently per tool.
//!
//! The catalog is an immutable capability snapshot. Its rendered guidance
//! enters the request-time Effective System Prompt through Context Assembly;
//! it is not canonical conversation history and is not carried by a
//! provider-request special channel.

use std::sync::Arc;

use crate::protocol::manifest::SkillBinding;
use crate::skills::diagnostics::{SkillDiagnostic, SkillProvenance};
use crate::skills::package::{SkillDiscoveryOutcome, SkillPackage};

/// One model-visible Skill catalog entry: standard metadata plus the host
/// location of the primary instructions file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillCatalogEntry {
    /// The validated standard Skill name.
    pub name: String,
    /// The validated standard Skill description.
    pub description: String,
    /// The canonical absolute host path of the package's `SKILL.md`. The
    /// model passes it to Read, and resolves the Skill's own relative
    /// references against its parent directory.
    pub location: String,
}

/// The immutable Skill snapshot of one capability set.
///
/// The snapshot holds the accepted Skill packages, the deterministically
/// ordered catalog metadata entries after Skill-level invocation filtering,
/// the deterministic `SkillId` + `SkillVersionId` bindings, the effective
/// source provenance of each identity, and the generation-scoped discovery
/// diagnostics. The entries are the one Skill-level model-visible set used
/// by capability projections. It is constructed once per candidate
/// preparation and never mutated.
///
/// Provenance and diagnostics are deliberately *beside* the catalog, not in
/// it: they explain the generation to inspection, and they never reach the
/// model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSnapshot {
    packages: Vec<Arc<SkillPackage>>,
    catalog: Vec<SkillCatalogEntry>,
    bindings: Vec<SkillBinding>,
    visible_bindings: Vec<SkillBinding>,
    provenance: Vec<SkillProvenance>,
    diagnostics: Vec<SkillDiagnostic>,
}

impl SkillSnapshot {
    /// Freezes one complete discovery outcome, including its provenance and
    /// its typed generation-scoped diagnostics.
    #[must_use]
    pub fn from_discovery(outcome: SkillDiscoveryOutcome) -> Self {
        let SkillDiscoveryOutcome {
            packages,
            provenance,
            diagnostics,
        } = outcome;
        Self {
            provenance,
            diagnostics,
            ..Self::new(packages.into_iter().map(Arc::new).collect())
        }
    }

    /// The effective source provenance of every admitted identity, ordered
    /// by Skill name. Generation/inspection metadata, never model input.
    #[must_use]
    pub fn provenance(&self) -> &[SkillProvenance] {
        &self.provenance
    }

    /// The canonically ordered Skill diagnostics of this generation.
    ///
    /// They are computed once, with the generation, and are never re-emitted
    /// per model turn.
    #[must_use]
    pub fn diagnostics(&self) -> &[SkillDiagnostic] {
        &self.diagnostics
    }

    /// Builds the immutable snapshot from the accepted packages, ordering
    /// everything deterministically by validated Skill name.
    ///
    /// Provenance is derived from the packages themselves and records no
    /// shadowing; [`Self::from_discovery`] is the complete boundary.
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
                location: package.location().to_owned(),
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
        let provenance = packages
            .iter()
            .map(|package| SkillProvenance {
                name: package.name().to_owned(),
                source: package.source(),
                location: package.location().to_owned(),
                shadowed: Vec::new(),
            })
            .collect();
        Self {
            packages,
            catalog,
            bindings,
            visible_bindings,
            provenance,
            diagnostics: Vec::new(),
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
    pub fn locations(&self) -> Vec<&str> {
        self.packages
            .iter()
            .map(|package| package.location())
            .collect()
    }

    /// Whether two snapshots have the same **execution-semantic** Skill
    /// state.
    ///
    /// Skill identity/version bindings describe package provenance, while the
    /// published locations describe where the admitted packages currently
    /// live. Both facts are required for rediscovery to be a no-op: identical
    /// package content moved to another current root must replace the active
    /// snapshot rather than leave the catalog pointing at the old host path.
    ///
    /// This deliberately covers *only* what an execution can observe. It is
    /// therefore **not** sufficient to decide a publication no-op: a
    /// generation also publishes provenance and diagnostics, which explain
    /// facts no executable binding can express. Use
    /// [`Self::publication_equivalent`] for that decision.
    #[must_use]
    pub fn semantically_equivalent(&self, other: &Self) -> bool {
        self.bindings == other.bindings
            && self.visible_bindings == other.visible_bindings
            && self.catalog == other.catalog
            && self.locations() == other.locations()
    }

    /// Whether two snapshots publish the **complete** same generation.
    ///
    /// A candidate is a true publication no-op only when both the executable
    /// Skill semantics and every generation-scoped Skill fact are unchanged:
    ///
    /// ```text
    /// publication_equivalent = semantically_equivalent
    ///                        + effective provenance
    ///                        + typed diagnostics
    /// ```
    ///
    /// The two extra dimensions are the reason this concept exists separately.
    /// Both of these rediscoveries leave the executable catalog untouched and
    /// must still publish a new generation:
    ///
    /// - a *diagnostics-only* change — a newly added malformed package
    ///   excludes itself, so the effective catalog is byte-identical while
    ///   the generation now owns a `package_invalid` fact;
    /// - a *provenance-only* change — a lower-precedence source starts
    ///   offering an identity the winner already owned, so the winner, its
    ///   bindings, and its location are unchanged while the generation now
    ///   owns shadowing provenance.
    ///
    /// Collapsing either into a no-op would leave inspection describing a
    /// filesystem state that no longer exists. Neither half is model-visible:
    /// they travel beside the catalog, never inside it.
    #[must_use]
    pub fn publication_equivalent(&self, other: &Self) -> bool {
        self.semantically_equivalent(other)
            && self.provenance == other.provenance
            && self.diagnostics == other.diagnostics
    }
}

/// Renders the compact `## Skills` catalog deterministically.
///
/// Callers projecting to a model must first apply `admitted_skill_entries`.
///
/// The rendered form gives each Skill its canonical host `SKILL.md` location
/// in deterministic sorted order. No `SKILL.md` body, supporting resource, or
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

/// The single lazy-Skill dependency projection for every execution domain.
/// Package discovery and child Skill admission stay intact; only guidance
/// requiring a model-callable native Read is hidden when Read is not admitted.
pub(crate) fn admitted_skill_entries<'a>(
    entries: &'a [SkillCatalogEntry],
    tools: &crate::tools::executor::ToolRegistry,
) -> &'a [SkillCatalogEntry] {
    if tools.definitions().iter().any(|definition| {
        definition.id.as_str() == crate::tools::native::READ_TOOL_ID
            && definition.name == "read"
            && definition.origin == crate::tools::types::ToolOrigin::Builtin
    }) {
        entries
    } else {
        &[]
    }
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

#[cfg(test)]
mod tests {
    use super::SkillSnapshot;
    use crate::skills::diagnostics::{ShadowedSkill, SkillDiagnostic, SkillProvenance};
    use crate::skills::package::{SkillDiscoveryOutcome, SkillPackageError};
    use crate::skills::source::SkillSource;

    fn snapshot(
        provenance: Vec<SkillProvenance>,
        diagnostics: Vec<SkillDiagnostic>,
    ) -> SkillSnapshot {
        SkillSnapshot::from_discovery(SkillDiscoveryOutcome {
            packages: Vec::new(),
            provenance,
            diagnostics,
        })
    }

    fn provenance(shadowed: Vec<ShadowedSkill>) -> Vec<SkillProvenance> {
        vec![SkillProvenance {
            name: "foo".to_owned(),
            source: SkillSource::Workspace,
            location: "/w/.agents/skills/foo/SKILL.md".to_owned(),
            shadowed,
        }]
    }

    /// #280: publication equivalence is strictly stronger than execution
    /// equivalence. Provenance and diagnostics each independently make a
    /// rediscovery a real publication, even when nothing an execution can
    /// observe has changed.
    #[test]
    fn cfg280_provenance_and_diagnostics_each_defeat_a_publication_noop() {
        let base = snapshot(provenance(Vec::new()), Vec::new());
        // Provenance only: the same winner now shadows a lower-precedence
        // package. No binding, catalog entry, or location changes.
        let shadowing = snapshot(
            provenance(vec![ShadowedSkill {
                source: SkillSource::Global,
                location: "/h/.agents/skills/foo/SKILL.md".to_owned(),
            }]),
            Vec::new(),
        );
        // Diagnostics only: a newly added malformed package excludes itself,
        // so the effective catalog is byte-identical.
        let diagnosed = snapshot(
            provenance(Vec::new()),
            vec![SkillDiagnostic::PackageInvalid {
                source: SkillSource::Workspace,
                package: "/w/.agents/skills/broken".to_owned(),
                cause: SkillPackageError::MissingSkillMarkdown {
                    directory: "broken".to_owned(),
                },
            }],
        );
        for changed in [&shadowing, &diagnosed] {
            assert!(
                base.semantically_equivalent(changed),
                "executable Skill semantics must be unchanged for this to prove anything"
            );
            assert!(
                !base.publication_equivalent(changed),
                "a generation-scoped Skill fact changed, so this is a real publication"
            );
            assert!(!changed.publication_equivalent(&base), "symmetric");
        }
        // A byte-for-byte identical generation stays a true no-op.
        assert!(base.publication_equivalent(&snapshot(provenance(Vec::new()), Vec::new())));
    }
}
