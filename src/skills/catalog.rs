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
/// A discovery snapshot holds accepted packages; a one-shot child installs
/// only its already-frozen metadata and proven bindings, with no discoverable
/// package authority. Both forms hold the deterministically
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
    roots: Vec<crate::skills::AutomaticSkillRoot>,
    packages: Vec<Arc<SkillPackage>>,
    catalog: Vec<SkillCatalogEntry>,
    bindings: Vec<SkillBinding>,
    visible_bindings: Vec<SkillBinding>,
    provenance: Vec<SkillProvenance>,
    diagnostics: Vec<SkillDiagnostic>,
}

impl SkillSnapshot {
    /// Install the selected metadata and proven bindings of a one-shot child.
    /// Packages were validated by the parent and physically verified by the
    /// materializer. No discovery, parsing, or body loading occurs here.
    pub(crate) fn from_frozen(
        mut entries: Vec<(SkillCatalogEntry, SkillBinding, SkillProvenance)>,
        roots: Vec<crate::skills::AutomaticSkillRoot>,
    ) -> Self {
        entries.sort_by(|left, right| left.0.name.cmp(&right.0.name));
        let bindings: Vec<_> = entries.iter().map(|entry| entry.1.clone()).collect();
        Self {
            roots,
            packages: Vec::new(), // children cannot delegate or rediscover packages
            catalog: entries.iter().map(|entry| entry.0.clone()).collect(),
            visible_bindings: bindings.clone(),
            bindings,
            provenance: entries.into_iter().map(|entry| entry.2).collect(),
            diagnostics: Vec::new(),
        }
    }

    /// Freezes one complete discovery outcome, including its provenance and
    /// its typed generation-scoped diagnostics.
    #[must_use]
    pub fn from_discovery(outcome: SkillDiscoveryOutcome) -> Self {
        let SkillDiscoveryOutcome {
            invalid: _,
            roots,
            packages,
            provenance,
            diagnostics,
        } = outcome;
        Self {
            roots,
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
        let catalog: Vec<SkillCatalogEntry> = packages
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
            roots: roots_from_entries(
                catalog
                    .iter()
                    .zip(packages.iter())
                    .map(|(entry, package)| (entry, package.source())),
            ),
            packages,
            catalog,
            bindings,
            visible_bindings,
            provenance,
            diagnostics: Vec::new(),
        }
    }

    /// Collection roots frozen with this generation.
    #[must_use]
    pub fn roots(&self) -> &[crate::skills::AutomaticSkillRoot] {
        &self.roots
    }

    /// The accepted discovery packages, deterministically ordered by Skill name.
    /// Empty for a one-shot child, whose metadata and bindings are frozen directly.
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
        self.provenance
            .iter()
            .map(|entry| entry.location.as_str())
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
        self.roots == other.roots
            && self.bindings == other.bindings
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

fn roots_from_entries<'a>(
    entries: impl Iterator<Item = (&'a SkillCatalogEntry, crate::skills::SkillSource)>,
) -> Vec<crate::skills::AutomaticSkillRoot> {
    let mut roots = std::collections::BTreeSet::new();
    for (entry, source) in entries {
        if let Some(root) = std::path::Path::new(&entry.location)
            .parent()
            .and_then(std::path::Path::parent)
        {
            roots.insert(crate::skills::AutomaticSkillRoot {
                source,
                root: root.to_owned(),
            });
        }
    }
    roots.into_iter().collect()
}

/// Compact metadata and collection roots enable progressive disclosure without
/// enumerating absolute package paths. Selection controls prompt visibility only.
#[must_use]
pub fn render_skill_catalog(
    entries: &[SkillCatalogEntry],
    roots: &[crate::skills::AutomaticSkillRoot],
) -> String {
    use std::fmt::Write as _;
    let mut out = String::from(
        "## Skills\n\nSkill selection controls prompt visibility, not filesystem access. Use an available file-reading Tool to load <root>/<name>/SKILL.md when a description matches the task. Resolve package-relative references against that package directory. Workspace identities completely shadow User identities.\n\n",
    );
    for (index, root) in roots.iter().enumerate() {
        let label = match root.source {
            crate::skills::SkillSource::User => "User Skill root",
            crate::skills::SkillSource::Workspace => "Workspace Skill root",
        };
        let _ = writeln!(
            out,
            "{label} (root{index}): {}",
            escape_catalog_text(&root.root.display().to_string())
        );
    }
    out.push_str("\n<available_skills>\n");
    for entry in entries {
        let name = escape_catalog_text(&entry.name);
        let description = escape_catalog_text(&entry.description);
        let root_index = roots.iter().position(|root| {
            root.root.join(&entry.name).join("SKILL.md") == std::path::Path::new(&entry.location)
        });
        if let Some(index) = root_index {
            let _ = writeln!(
                out,
                "  <skill><name>{name}</name><description>{description}</description><root>root{index}</root></skill>"
            );
        }
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
            invalid: Vec::new(),
            roots: Vec::new(),

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
                source: SkillSource::User,
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
