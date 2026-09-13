//! Skill package discovery, parsing, and validation (M6, Issue #280).
//!
//! # Ownership
//!
//! This module owns exactly one question per candidate: **is this directory
//! a valid Agent Skills package?** It does not decide which Agent may see a
//! Skill, and it does not decide where sources live — [`super::source`] owns
//! source identity and root resolution, and
//! [`crate::runtime::agent_profile`] owns capability selection.
//!
//! # Discovery pipeline
//!
//! ```text
//! configured sources (automatic roots + explicit launch paths)
//!         |
//!         v
//! enumerate candidate package directories per source
//!         |
//!         v
//! canonicalize + source containment
//!         |
//!         v
//! validate each candidate independently
//!         |
//!         +-- valid   -> source-local candidate
//!         |
//!         +-- invalid -> excluded + typed generation diagnostic
//!         |
//!         v
//! same-scope logical-identity conflict elimination
//!         |
//!         v
//! cross-source merge (explicit > workspace > global)
//!         |
//!         v
//! SkillDiscoveryOutcome: packages + provenance + diagnostics
//! ```
//!
//! **One malformed Skill package never suppresses unrelated valid Skills.**
//! A candidate that fails validation is excluded and represented by a typed
//! [`SkillDiagnostic`](crate::skills::SkillDiagnostic); the rest of its
//! source still publishes. Only a failure of the *explicit launch authority*
//! itself — a `--skill` path that does not exist, more explicit paths than
//! the bound allows, or more explicit candidate packages than one source's
//! cumulative budget allows — is an error, because that is authored launch
//! intent rather than discovered content.
//!
//! # Resource bounding
//!
//! Enumeration is separated from validation so that a source's complete
//! candidate count is known before a single `SKILL.md` is parsed.
//! [`MAX_SOURCE_SKILL_PACKAGES`] then bounds one logical **source**,
//! cumulatively across every root that source aggregates — see its
//! documentation for why that is not a per-root bound. An automatic source
//! that overruns its budget is excluded whole with a typed diagnostic; the
//! explicit authority's overrun is a launch error.
//!
//! # Package root invariant
//!
//! Discovery accepts non-canonical inputs — a relative `--skill` path, an
//! ancestor symlink, an embedded `..` — but an *accepted* package always
//! carries one canonical absolute host root, and a `location` that is that
//! root's `SKILL.md` losslessly representable as UTF-8. Everything
//! downstream (the catalog, snapshot equality, and every native tool the
//! model hands the published path to) consumes that single fact, so no
//! consumer can re-resolve a published path against a different base and
//! reach a different file. A candidate whose root cannot be canonicalized,
//! or whose canonical path is not valid UTF-8, is excluded rather than
//! published in a lossy spelling.
//!
//! A source is a bounded authority: an accepted candidate's canonical root
//! must remain inside its own source's canonical root. Global and workspace
//! are *different* authorities, so containment is always package-in-source,
//! never package-in-workspace.
//!
//! Discovery is one level only: direct child directories of the Skill
//! root, each containing a `SKILL.md`. Nested Skill packages are never
//! discovered recursively.
//!
//! # Discovery semantics
//!
//! - a missing automatic Skill root is an empty set and a benign fact;
//! - an automatic root that exists but cannot be scanned excludes only that
//!   source;
//! - hidden direct entries (names beginning with `.`) are ignored;
//! - ordinary unrelated files directly under an automatic Skill root are
//!   ignored;
//! - each non-hidden candidate directory must contain `SKILL.md`;
//! - symlinked Skill package roots and symlink entries inside a Skill
//!   package are rejected (this is Skill-package validation only; the
//!   general Workspace symlink contract for ordinary tools is unchanged);
//! - results are deterministically ordered by validated Skill name,
//!   independent of filesystem enumeration order and of configured root
//!   order.
//!
//! # Frontmatter contract
//!
//! `SKILL.md` is YAML frontmatter followed by Markdown instructions (the
//! Agent Skills standard format; no replacement format is invented). The
//! standard requirements validated here:
//!
//! - `name`: 1-64 characters, lowercase letters, numbers, and hyphens
//!   only; must not start or end with a hyphen and must not contain
//!   consecutive hyphens; must match the parent directory name;
//! - `description`: non-empty, at most 1024 characters;
//! - `metadata`: a string-to-string map when present;
//! - `license`, `compatibility` (1-500 characters), and `allowed-tools`
//!   are parsed and preserved but no runtime policy is invented for them;
//! - `disable-model-invocation` keeps the package out of the model-facing
//!   catalog while leaving it owned by the generation;
//! - the rustX dependency declaration keys are parsed from `metadata` (see
//!   [`crate::skills::dependencies`]).
//!
//! The model-visible catalog contains only the standard `name` and
//! `description` plus the host location of `SKILL.md`.
//!
//! # Resource boundary
//!
//! Skill packages remain current filesystem resources. Discovery freezes
//! identities, versions, catalog metadata, and dependency declarations at
//! candidate-generation time. Package *bodies* are read at use time through
//! ordinary tool semantics — discovery never loads a `SKILL.md` body into
//! model context — and an external rewrite is observed only at the next
//! quiescent re-discovery, which publishes a later generation without
//! mutating an already admitted one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::runtime::identity::{SkillId, SkillVersionId};
use crate::skills::dependencies::{DependencyManifest, parse_dependency_map};
use crate::skills::diagnostics::{ShadowedSkill, SkillDiagnostic, SkillProvenance};
use crate::skills::identity::package_version_id;
use crate::skills::source::{AutomaticSkillRoot, SkillSource};
use crate::tools::workspace::Workspace;

/// The canonical primary instructions file name of a Skill package.
pub const SKILL_MARKDOWN_FILE: &str = "SKILL.md";

/// The maximum allowed length of a validated standard Skill name.
pub const MAX_SKILL_NAME_CHARS: usize = 64;
/// The maximum allowed length of a validated standard Skill description.
pub const MAX_SKILL_DESCRIPTION_CHARS: usize = 1024;
/// The maximum allowed length of the standard `compatibility` field.
pub const MAX_SKILL_COMPATIBILITY_CHARS: usize = 500;

/// The maximum number of explicit `--skill` launch paths.
pub const MAX_EXPLICIT_SKILL_PATHS: usize = 128;
/// The maximum number of direct entries one Skill collection *directory* may
/// hold.
///
/// This bounds one `read_dir`, so it is deliberately per directory: it is the
/// cost of enumerating that directory, not the cost of the source.
pub const MAX_SKILL_ROOT_ENTRIES: usize = 1024;
/// The maximum number of candidate packages **one logical source** may offer,
/// cumulatively across every root that source aggregates.
///
/// This is a per-source bound, not a per-root one. A source may aggregate
/// several roots — every `--skill` collection path and every explicitly named
/// package path feeds the one [`SkillSource::Explicit`] domain — and all of
/// them draw from the same budget. Charging the bound per root instead would
/// silently multiply the ceiling by the number of configured roots, so a
/// launch with sixteen `--skill` collections could admit sixteen times the
/// intended startup and reload work.
///
/// The budget is charged against *candidates*, before validation, because the
/// work being bounded is the per-candidate validation itself: an excluded
/// malformed package still costs a `SKILL.md` parse and a package walk.
pub const MAX_SOURCE_SKILL_PACKAGES: usize = 128;

/// A parsing/validation failure of **one** Skill package.
///
/// Every variant identifies the responsible candidate. A failure here
/// excludes exactly that candidate and is preserved verbatim inside
/// [`SkillDiagnostic::PackageInvalid`]; it never fails discovery, and it
/// never suppresses an unrelated valid package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum SkillPackageError {
    /// The package name violates the Agent Skills naming rules.
    InvalidName {
        directory: String,
        name: String,
        detail: String,
    },
    /// The frontmatter `name` does not match the parent directory name.
    NameDirectoryMismatch { directory: String, name: String },
    /// The candidate directory contains no `SKILL.md`.
    MissingSkillMarkdown { directory: String },
    /// The `SKILL.md` entry is not an ordinary regular file (or is a
    /// symlink).
    SkillMarkdownNotRegularFile { directory: String },
    /// The `SKILL.md` frontmatter is malformed YAML or violates the
    /// standard shape.
    MalformedFrontmatter { directory: String, detail: String },
    /// The standard description is empty or exceeds the length bound.
    InvalidDescription { directory: String, detail: String },
    /// The standard `compatibility` field exceeds its length bound.
    InvalidCompatibility { directory: String, detail: String },
    /// The `metadata` field is not a string-to-string map.
    MalformedMetadata { directory: String, detail: String },
    /// A rustX dependency declaration is malformed or unsupported.
    InvalidDependencyDeclaration { directory: String, detail: String },
    /// A symlinked package root or a symlink entry inside the package was
    /// found. Package-internal symlinks are rejected; this is Skill-package
    /// validation, not a change to normal Workspace semantics.
    UnsupportedSymlink { path: String },
    /// The canonical package root is not losslessly representable as UTF-8,
    /// so it cannot be published as a model-visible location.
    UnrepresentableRoot { path: String },
    /// A filesystem failure while reading the package.
    Io { path: String, detail: String },
}

impl core::fmt::Display for SkillPackageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidName {
                directory,
                name,
                detail,
            } => write!(
                f,
                "skill {directory:?}: name {name:?} violates the Agent Skills naming rules: \
                 {detail}"
            ),
            Self::NameDirectoryMismatch { directory, name } => write!(
                f,
                "skill {directory:?}: frontmatter name {name:?} does not match the parent \
                 directory"
            ),
            Self::MissingSkillMarkdown { directory } => {
                write!(f, "skill {directory:?}: no {SKILL_MARKDOWN_FILE} present")
            }
            Self::SkillMarkdownNotRegularFile { directory } => write!(
                f,
                "skill {directory:?}: {SKILL_MARKDOWN_FILE} is not an ordinary regular file"
            ),
            Self::MalformedFrontmatter { directory, detail } => {
                write!(f, "skill {directory:?}: malformed frontmatter: {detail}")
            }
            Self::InvalidDescription { directory, detail } => {
                write!(f, "skill {directory:?}: invalid description: {detail}")
            }
            Self::InvalidCompatibility { directory, detail } => {
                write!(f, "skill {directory:?}: invalid compatibility: {detail}")
            }
            Self::MalformedMetadata { directory, detail } => {
                write!(f, "skill {directory:?}: malformed metadata: {detail}")
            }
            Self::InvalidDependencyDeclaration { directory, detail } => {
                write!(f, "skill {directory:?}: {detail}")
            }
            Self::UnrepresentableRoot { path } => write!(
                f,
                "the skill package root {path} is not valid UTF-8 and cannot be published as a \
                 model-visible location"
            ),
            Self::UnsupportedSymlink { path } => {
                write!(f, "skill package symlinks are rejected: {path:?}")
            }
            Self::Io { path, detail } => write!(f, "cannot read {path:?}: {detail}"),
        }
    }
}

impl std::error::Error for SkillPackageError {}

/// A failure of the **explicit Skill launch authority** itself.
///
/// This is authored launch intent, not discovered content: a `--skill` path
/// that does not exist is a launch error in exactly the same way a missing
/// `--config` is, and it is deliberately not downgraded to a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillDiscoveryError {
    /// An explicit Skill path could not be used.
    ExplicitPath { path: String, detail: String },
    /// More explicit Skill paths were supplied than the bound allows.
    TooManyExplicitPaths { count: usize },
    /// The explicit paths together offered more candidate packages than one
    /// source's cumulative budget allows.
    ///
    /// This is a launch error rather than a diagnostic for the same reason a
    /// missing `--skill` path is: the explicit authority is authored intent,
    /// and silently dropping the packages an author named would be worse than
    /// refusing the launch.
    TooManyExplicitPackages { count: usize },
}

impl core::fmt::Display for SkillDiscoveryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ExplicitPath { path, detail } => {
                write!(f, "explicit Skill path {path:?}: {detail}")
            }
            Self::TooManyExplicitPaths { count } => write!(
                f,
                "explicit Skill paths exceed the {MAX_EXPLICIT_SKILL_PATHS} bound, found {count}"
            ),
            Self::TooManyExplicitPackages { count } => write!(
                f,
                "the explicit Skill paths together offer {count} candidate packages, exceeding \
                 the cumulative {MAX_SOURCE_SKILL_PACKAGES}-package bound of one Skill source"
            ),
        }
    }
}

impl std::error::Error for SkillDiscoveryError {}

/// One discovered and validated Skill package.
///
/// The package is immutable after discovery: its `SkillVersionId` is
/// derived from the complete accepted package content, and its dependency
/// declarations are already parsed and normalized. `root` is canonical and
/// absolute, and `location` is the model-visible host path of its
/// `SKILL.md` — see the package root invariant in the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPackage {
    id: SkillId,
    version_id: SkillVersionId,
    name: String,
    description: String,
    metadata: BTreeMap<String, String>,
    license: Option<String>,
    compatibility: Option<String>,
    allowed_tools: Option<String>,
    dependencies: DependencyManifest,
    files: Vec<PathBuf>,
    root: PathBuf,
    location: String,
    disable_model_invocation: bool,
    source: SkillSource,
}

impl SkillPackage {
    /// The source authority that published this package.
    ///
    /// Provenance, never identity: the content-derived `SkillVersionId` is
    /// deliberately independent of it, so the *same* package moved between
    /// the global and workspace roots stays the same package.
    #[must_use]
    pub const fn source(&self) -> SkillSource {
        self.source
    }

    /// The validated standard Skill name, used as the logical `SkillId`.
    #[must_use]
    pub fn id(&self) -> &SkillId {
        &self.id
    }

    /// The content-derived immutable package version identity.
    #[must_use]
    pub fn version_id(&self) -> &SkillVersionId {
        &self.version_id
    }

    /// The standard validated Skill name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The standard validated Skill description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The preserved standard `metadata` map (including the rustX
    /// dependency declaration keys).
    #[must_use]
    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    /// The preserved standard optional `license` field.
    #[must_use]
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    /// The preserved standard optional `compatibility` field.
    #[must_use]
    pub fn compatibility(&self) -> Option<&str> {
        self.compatibility.as_deref()
    }

    /// The preserved standard optional `allowed-tools` field. M6 parses and
    /// preserves it but invents no runtime policy for it.
    #[must_use]
    pub fn allowed_tools(&self) -> Option<&str> {
        self.allowed_tools.as_deref()
    }

    /// The parsed and normalized rustX dependency declarations.
    #[must_use]
    pub fn dependencies(&self) -> &DependencyManifest {
        &self.dependencies
    }

    /// The sorted workspace-relative file paths of the accepted package.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// The canonical absolute host root of this package, as a
    /// **materialization source** for a separate-process runtime (Issue
    /// #145).
    ///
    /// A consumer of this path
    /// is copying bytes out of it and must prove the copy still hashes to
    /// the package's `SkillVersionId`. The path is never an identity.
    #[must_use]
    pub fn materialization_root(&self) -> &Path {
        &self.root
    }

    /// The model-visible host path of this package's `SKILL.md`.
    ///
    /// This is the one published address: the catalog projects it verbatim,
    /// and the model hands it straight back to Read and Bash.
    #[must_use]
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Whether this validated Skill is omitted from the model catalog.
    #[must_use]
    pub fn disable_model_invocation(&self) -> bool {
        self.disable_model_invocation
    }
}

/// The configured Skill discovery authorities of one candidate generation.
///
/// This is *where packages may be found*, never *which Skills an Agent may
/// see*. The two automatic roots are resolved by the launch/environment
/// owner from the session `[skills].sources` policy; explicit paths are the
/// separate `--skill` launch authority.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillDiscoveryConfig {
    /// Resolved automatic source roots. A missing root is an empty set.
    pub automatic: Vec<AutomaticSkillRoot>,
    /// Explicit collection roots, package directories, or `SKILL.md` paths
    /// supplied by the launch authority. A missing explicit path is an
    /// error, not a diagnostic.
    pub explicit_paths: Vec<PathBuf>,
}

impl SkillDiscoveryConfig {
    /// A configuration scanning exactly one workspace collection root.
    #[must_use]
    pub fn workspace_root(root: impl Into<PathBuf>) -> Self {
        Self {
            automatic: vec![AutomaticSkillRoot {
                source: SkillSource::Workspace,
                root: root.into(),
            }],
            explicit_paths: Vec::new(),
        }
    }

    /// A configuration using only the explicit launch authority.
    #[must_use]
    pub fn explicit(paths: Vec<PathBuf>) -> Self {
        Self {
            automatic: Vec::new(),
            explicit_paths: paths,
        }
    }
}

/// The deterministic outcome of one discovery pass.
///
/// The three parts are computed together and frozen together: the effective
/// packages, their provenance, and every typed fact about what was excluded
/// or shadowed on the way there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillDiscoveryOutcome {
    /// The effective packages, ordered by validated Skill name.
    pub packages: Vec<SkillPackage>,
    /// The provenance of every effective identity, ordered by name.
    pub provenance: Vec<SkillProvenance>,
    /// Canonically ordered generation-scoped diagnostics.
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// Discovers Skill packages across the configured bounded authorities.
#[derive(Debug, Clone)]
pub struct SkillDiscovery {
    config: SkillDiscoveryConfig,
    /// The cumulative candidate budget of **one logical source**. Always
    /// [`MAX_SOURCE_SKILL_PACKAGES`] in production; a test may lower it to
    /// prove the boundary without materializing hundreds of package trees.
    source_budget: usize,
}

/// One validated candidate before cross-source merge.
#[derive(Debug)]
struct Candidate {
    source: SkillSource,
    package: SkillPackage,
}

/// One source's resolved candidate entries, bounded by the root that offered
/// them.
///
/// Enumeration is separated from validation so that a source's *complete*
/// candidate count is known before a single `SKILL.md` is parsed. That is what
/// makes the cumulative budget decision — and the number it reports — a
/// function of the source alone, independent of the order its roots were
/// configured in.
struct ResolvedRoot {
    boundary: PathBuf,
    entries: Vec<(String, PathBuf)>,
}

impl SkillDiscovery {
    /// Creates discovery with explicit current runtime authorities.
    ///
    /// There is deliberately no process-environment constructor: the
    /// launch/environment owner resolves the automatic roots (including the
    /// global root, from its captured home directory) and hands them here, so
    /// a test injects an isolated HOME without touching process-global state.
    #[must_use]
    pub fn with_config(_workspace: &Workspace, config: SkillDiscoveryConfig) -> Self {
        Self {
            config,
            source_budget: MAX_SOURCE_SKILL_PACKAGES,
        }
    }

    /// The same discovery with a lowered per-source candidate budget.
    ///
    /// A deterministic boundary seam, not a configuration knob: production has
    /// exactly one budget ([`MAX_SOURCE_SKILL_PACKAGES`]). The arithmetic and
    /// every reporting path are shared, so a test proves the real contract —
    /// cumulative per source, exact at the limit, enumeration-order
    /// independent — without materializing hundreds of real package trees per
    /// root.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_source_budget(config: SkillDiscoveryConfig, source_budget: usize) -> Self {
        Self {
            config,
            source_budget,
        }
    }

    /// Discovers the effective Skill package set.
    ///
    /// Results are deterministically ordered by validated Skill name, and
    /// are independent of filesystem enumeration order and of the order the
    /// configured roots appear in. A malformed candidate is excluded with a
    /// typed diagnostic; a missing automatic root is a benign empty set.
    ///
    /// # Errors
    ///
    /// Returns [`SkillDiscoveryError`] only when the *explicit* launch
    /// authority is itself unusable.
    pub fn discover(&self) -> Result<SkillDiscoveryOutcome, SkillDiscoveryError> {
        if self.config.explicit_paths.len() > MAX_EXPLICIT_SKILL_PATHS {
            return Err(SkillDiscoveryError::TooManyExplicitPaths {
                count: self.config.explicit_paths.len(),
            });
        }
        let mut diagnostics = Vec::new();
        let mut candidates = Vec::<Candidate>::new();
        // Sort the configured roots by source identity so a permuted
        // configuration array cannot become a semantic mode. Precedence is
        // decided below by `SkillSource` ordering regardless.
        let mut automatic = self.config.automatic.clone();
        automatic.sort();
        automatic.dedup();
        // Group the roots under the source that owns them: the candidate budget
        // is cumulative per *source*, so a source is admitted or excluded as a
        // whole rather than root by root.
        let mut by_source: BTreeMap<SkillSource, Vec<&AutomaticSkillRoot>> = BTreeMap::new();
        for root in &automatic {
            by_source.entry(root.source).or_default().push(root);
        }
        for (source, roots) in by_source {
            candidates.extend(collect_automatic_source(
                source,
                &roots,
                self.source_budget,
                &mut diagnostics,
            ));
        }
        candidates.extend(collect_explicit_source(
            &self.config.explicit_paths,
            self.source_budget,
            &mut diagnostics,
        )?);
        Ok(merge_candidates(candidates, diagnostics))
    }
}

/// Eliminates same-scope conflicts, then applies cross-source precedence.
///
/// Both steps are total orders over typed values: the scope conflict is
/// decided before any winner selection, so an excluded candidate can never
/// win merely by living in the higher-precedence source.
fn merge_candidates(
    candidates: Vec<Candidate>,
    mut diagnostics: Vec<SkillDiagnostic>,
) -> SkillDiscoveryOutcome {
    // ---- same-scope logical identity conflicts ----
    let mut scoped: BTreeMap<(SkillSource, String), Vec<SkillPackage>> = BTreeMap::new();
    for candidate in candidates {
        scoped
            .entry((candidate.source, candidate.package.name().to_owned()))
            .or_default()
            .push(candidate.package);
    }
    let mut surviving: BTreeMap<String, BTreeMap<SkillSource, SkillPackage>> = BTreeMap::new();
    for ((source, name), mut packages) in scoped {
        if packages.len() > 1 {
            packages.sort_by(|left, right| left.location().cmp(right.location()));
            diagnostics.push(SkillDiagnostic::DuplicateIdentity {
                source,
                name,
                packages: packages
                    .iter()
                    .map(|package| package.location().to_owned())
                    .collect(),
            });
            continue;
        }
        let package = packages.pop().expect("one surviving scoped candidate");
        surviving.entry(name).or_default().insert(source, package);
    }
    // ---- cross-source precedence: explicit > workspace > global ----
    let mut packages = Vec::with_capacity(surviving.len());
    let mut provenance = Vec::with_capacity(surviving.len());
    for (name, by_source) in surviving {
        let mut by_source: Vec<_> = by_source.into_iter().collect();
        let (winner_source, winner) = by_source.pop().expect("one surviving source candidate");
        let mut shadowed = Vec::new();
        for (source, package) in by_source {
            diagnostics.push(SkillDiagnostic::Shadowed {
                name: name.clone(),
                effective_source: winner_source,
                effective_location: winner.location().to_owned(),
                shadowed_source: source,
                shadowed_location: package.location().to_owned(),
            });
            shadowed.push(ShadowedSkill {
                source,
                location: package.location().to_owned(),
            });
        }
        shadowed.sort();
        provenance.push(SkillProvenance {
            name,
            source: winner_source,
            location: winner.location().to_owned(),
            shadowed,
        });
        packages.push(winner);
    }
    packages.sort_by(|left, right| left.name().cmp(right.name()));
    provenance.sort();
    diagnostics.sort();
    diagnostics.dedup();
    SkillDiscoveryOutcome {
        packages,
        provenance,
        diagnostics,
    }
}

/// Enumerates and validates one automatic **source**, across every root
/// configured for it.
///
/// Every failure below a root is a per-candidate exclusion; only a failure of a
/// root itself, or an overrun of the source's cumulative candidate budget,
/// suppresses this source — and never another one.
fn collect_automatic_source(
    source: SkillSource,
    roots: &[&AutomaticSkillRoot],
    budget: usize,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Vec<Candidate> {
    // Phase 1: enumerate. No `SKILL.md` is parsed yet, so the source's total
    // candidate count is known before any validation work is spent.
    let mut resolved = Vec::with_capacity(roots.len());
    for root in roots {
        if let Some(root) = resolve_automatic_root(root, diagnostics) {
            resolved.push(root);
        }
    }
    let candidates = resolved
        .iter()
        .map(|root| root.entries.len())
        .sum::<usize>();
    if candidates > budget {
        // The budget belongs to the source, so the source contributes nothing.
        // Every other source stays unaffected, exactly as for an unusable root.
        diagnostics.push(SkillDiagnostic::SourceBudgetExceeded {
            source,
            roots: roots
                .iter()
                .map(|root| root.root.display().to_string())
                .collect(),
            candidates,
            limit: budget,
        });
        return Vec::new();
    }
    // Phase 2: validate each candidate against the root that offered it.
    let mut collected = Vec::with_capacity(candidates);
    for ResolvedRoot { boundary, entries } in resolved {
        collected.extend(validate_candidates(source, &boundary, entries, diagnostics));
    }
    collected
}

/// Enumerates one automatic source root, or reports why it offers nothing.
fn resolve_automatic_root(
    root: &AutomaticSkillRoot,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Option<ResolvedRoot> {
    let source = root.source;
    let display = root.root.display().to_string();
    let Some(metadata) = existing_symlink_metadata(&root.root) else {
        diagnostics.push(SkillDiagnostic::SourceRootMissing {
            source,
            root: display,
        });
        return None;
    };
    let mut invalid = |detail: String| {
        diagnostics.push(SkillDiagnostic::SourceRootInvalid {
            source,
            root: display.clone(),
            detail,
        });
        None
    };
    let metadata = match metadata {
        Ok(metadata) => metadata,
        Err(detail) => return invalid(detail),
    };
    if metadata.file_type().is_symlink() {
        return invalid("a Skill collection root must not be a symlink".to_owned());
    }
    if !metadata.is_dir() {
        return invalid("the Skill source root is not a directory".to_owned());
    }
    let boundary = match std::fs::canonicalize(&root.root) {
        Ok(boundary) => boundary,
        Err(error) => {
            return invalid(format!(
                "cannot canonicalize the Skill source root: {error}"
            ));
        }
    };
    match collect_collection_entries(&root.root) {
        Ok(entries) => Some(ResolvedRoot { boundary, entries }),
        Err(detail) => invalid(detail),
    }
}

/// Enumerates and validates the explicit `--skill` launch authority.
///
/// Each explicit path is its own containment boundary: an explicit package
/// path bounds itself, an explicit collection root bounds its children. All of
/// them belong to the one [`SkillSource::Explicit`] domain, so they share one
/// cumulative candidate budget rather than each admitting a full one.
fn collect_explicit_source(
    paths: &[PathBuf],
    budget: usize,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Result<Vec<Candidate>, SkillDiscoveryError> {
    let source = SkillSource::Explicit;
    // Phase 1: resolve every explicit path into candidate entries, without
    // validating any package. The cumulative count is therefore a function of
    // the authored path set alone, never of the order it was supplied in.
    let mut resolved = Vec::with_capacity(paths.len());
    let mut excluded = Vec::new();
    let mut candidates = 0usize;
    for path in paths {
        let display = path.display().to_string();
        let explicit_error = |detail: String| SkillDiscoveryError::ExplicitPath {
            path: display.clone(),
            detail,
        };
        let metadata = existing_symlink_metadata(path)
            .ok_or_else(|| explicit_error("explicit Skill path does not exist".to_owned()))?
            .map_err(explicit_error)?;
        if metadata.file_type().is_symlink() {
            // A rejected candidate still occupied a candidate slot.
            candidates += 1;
            excluded.push(SkillDiagnostic::PackageInvalid {
                source,
                package: display,
                cause: SkillPackageError::UnsupportedSymlink {
                    path: path.display().to_string(),
                },
            });
            continue;
        }
        let package_root = if metadata.is_file() {
            if path.file_name().and_then(|name| name.to_str()) != Some(SKILL_MARKDOWN_FILE) {
                return Err(explicit_error(format!(
                    "an explicit Skill file must be named {SKILL_MARKDOWN_FILE}"
                )));
            }
            Some(
                path.parent()
                    .ok_or_else(|| explicit_error("no package directory".to_owned()))?,
            )
        } else if !metadata.is_dir() {
            return Err(explicit_error(
                "an explicit Skill path must be a directory or SKILL.md".to_owned(),
            ));
        } else if path.join(SKILL_MARKDOWN_FILE).is_file() {
            Some(path.as_path())
        } else {
            None
        };
        if let Some(root) = package_root {
            candidates += 1;
            match resolve_explicit_package(root) {
                Ok(resolved_root) => resolved.push(resolved_root),
                Err(diagnostic) => excluded.push(diagnostic),
            }
            continue;
        }
        let boundary = std::fs::canonicalize(path).map_err(|error| {
            explicit_error(format!(
                "cannot canonicalize the Skill collection root: {error}"
            ))
        })?;
        let entries = collect_collection_entries(path).map_err(explicit_error)?;
        candidates += entries.len();
        resolved.push(ResolvedRoot { boundary, entries });
    }
    // An overrun of the *explicit* authority is a launch error rather than a
    // diagnostic, for the same reason a missing `--skill` path is: the packages
    // were authored by name, so dropping them silently would be worse than
    // refusing the launch.
    if candidates > budget {
        return Err(SkillDiscoveryError::TooManyExplicitPackages { count: candidates });
    }
    // Phase 2: validate.
    diagnostics.append(&mut excluded);
    let mut collected = Vec::with_capacity(candidates);
    for ResolvedRoot { boundary, entries } in resolved {
        collected.extend(validate_candidates(source, &boundary, entries, diagnostics));
    }
    Ok(collected)
}

/// Enumerates one explicitly named package directory, bounded by itself.
fn resolve_explicit_package(root: &Path) -> Result<ResolvedRoot, SkillDiagnostic> {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    let boundary =
        std::fs::canonicalize(root).map_err(|error| SkillDiagnostic::PackageInvalid {
            source: SkillSource::Explicit,
            package: root.display().to_string(),
            cause: SkillPackageError::Io {
                path: root.display().to_string(),
                detail: format!("cannot canonicalize the Skill package root: {error}"),
            },
        })?;
    Ok(ResolvedRoot {
        boundary,
        entries: vec![(name, root.to_path_buf())],
    })
}

/// Validates each candidate independently against its own source boundary.
///
/// A candidate that fails is excluded with a typed diagnostic; the loop
/// always continues, which is the whole point of the contract.
fn validate_candidates(
    source: SkillSource,
    boundary: &Path,
    entries: Vec<(String, PathBuf)>,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Vec<Candidate> {
    let mut candidates = Vec::with_capacity(entries.len());
    for (name, path) in entries {
        let package_display = path.display().to_string();
        if let Err(detail) = validate_skill_name(&name) {
            diagnostics.push(SkillDiagnostic::PackageInvalid {
                source,
                package: package_display.clone(),
                cause: SkillPackageError::InvalidName {
                    directory: package_display,
                    name,
                    detail,
                },
            });
            continue;
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                diagnostics.push(SkillDiagnostic::PackageInvalid {
                    source,
                    package: package_display.clone(),
                    cause: SkillPackageError::UnsupportedSymlink {
                        path: package_display,
                    },
                });
                continue;
            }
            Ok(_) => {}
            Err(error) => {
                diagnostics.push(SkillDiagnostic::PackageInvalid {
                    source,
                    package: package_display.clone(),
                    cause: SkillPackageError::Io {
                        path: package_display,
                        detail: error.to_string(),
                    },
                });
                continue;
            }
        }
        let root = match std::fs::canonicalize(&path) {
            Ok(root) => root,
            Err(error) => {
                diagnostics.push(SkillDiagnostic::PackageInvalid {
                    source,
                    package: package_display.clone(),
                    cause: SkillPackageError::Io {
                        path: package_display,
                        detail: format!("cannot canonicalize the Skill package root: {error}"),
                    },
                });
                continue;
            }
        };
        // A source is a bounded authority. Containment is package-in-source,
        // never package-in-workspace: global and workspace are different
        // authorities and neither may be measured against the other.
        if !root.starts_with(boundary) {
            diagnostics.push(SkillDiagnostic::PackageEscapesSource {
                source,
                package: package_display,
                root: boundary.display().to_string(),
            });
            continue;
        }
        match discover_package(&root, &name, source) {
            Ok(package) => candidates.push(Candidate { source, package }),
            Err(cause) => diagnostics.push(SkillDiagnostic::PackageInvalid {
                source,
                package: package_display,
                cause,
            }),
        }
    }
    candidates
}

/// The `symlink_metadata` of a path that exists, distinguishing absence
/// (`None`) from an unreadable entry (`Some(Err)`).
fn existing_symlink_metadata(path: &Path) -> Option<Result<std::fs::Metadata, String>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Some(Ok(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(Err(error.to_string())),
    }
}

/// The direct child package candidates of one Skill collection directory.
///
/// Hidden entries and ordinary files are ignored. A symlinked direct entry
/// is *returned* as a candidate so package validation rejects it as the
/// package-level fact it is, rather than suppressing the whole root.
fn collect_collection_entries(path: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let entries = std::fs::read_dir(path).map_err(|error| error.to_string())?;
    let entries = entries
        .take(MAX_SKILL_ROOT_ENTRIES + 1)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if entries.len() > MAX_SKILL_ROOT_ENTRIES {
        return Err(format!(
            "the Skill collection root exceeds {MAX_SKILL_ROOT_ENTRIES} entries"
        ));
    }
    let mut children = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() || file_type.is_symlink() {
            children.push((name, entry.path()));
        }
    }
    // The candidate *count* is deliberately not bounded here: it is charged
    // against the owning source's cumulative budget by the caller, because one
    // source may aggregate several roots. See `MAX_SOURCE_SKILL_PACKAGES`.
    //
    // Canonical candidate order. Discovery must not be able to observe the
    // filesystem's enumeration order, even before validation.
    children.sort();
    Ok(children)
}

/// Parses, validates, and hashes one Skill package directory.
#[allow(clippy::too_many_lines)] // Bounded package validation stays in its owner.
fn discover_package(
    root: &Path,
    directory_name: &str,
    source: SkillSource,
) -> Result<SkillPackage, SkillPackageError> {
    validate_skill_name(directory_name).map_err(|detail| SkillPackageError::InvalidName {
        directory: directory_name.to_owned(),
        name: directory_name.to_owned(),
        detail,
    })?;
    let skill_markdown = root.join(SKILL_MARKDOWN_FILE);
    let location = published_location(root, &skill_markdown)?;
    let markdown_meta = std::fs::symlink_metadata(&skill_markdown).map_err(|_| {
        SkillPackageError::MissingSkillMarkdown {
            directory: directory_name.to_owned(),
        }
    })?;
    if !markdown_meta.is_file() || markdown_meta.file_type().is_symlink() {
        return Err(SkillPackageError::SkillMarkdownNotRegularFile {
            directory: directory_name.to_owned(),
        });
    }
    let markdown_bytes = crate::bounded_file::read_bounded(&skill_markdown).map_err(|error| {
        SkillPackageError::Io {
            path: skill_markdown.display().to_string(),
            detail: error,
        }
    })?;
    let markdown_text = String::from_utf8(markdown_bytes.clone()).map_err(|error| {
        SkillPackageError::MalformedFrontmatter {
            directory: directory_name.to_owned(),
            detail: format!("SKILL.md is not valid UTF-8: {error}"),
        }
    })?;
    let frontmatter = parse_frontmatter(&markdown_text).map_err(|failure| match failure {
        FrontmatterFailure::Malformed(detail) => SkillPackageError::MalformedFrontmatter {
            directory: directory_name.to_owned(),
            detail,
        },
        FrontmatterFailure::InvalidMetadata(detail) => SkillPackageError::MalformedMetadata {
            directory: directory_name.to_owned(),
            detail,
        },
    })?;
    if frontmatter.name != directory_name {
        return Err(SkillPackageError::NameDirectoryMismatch {
            directory: directory_name.to_owned(),
            name: frontmatter.name.clone(),
        });
    }
    let description = frontmatter.description.trim();
    if description.is_empty() {
        return Err(SkillPackageError::InvalidDescription {
            directory: directory_name.to_owned(),
            detail: "description must be non-empty".to_owned(),
        });
    }
    if description.chars().count() > MAX_SKILL_DESCRIPTION_CHARS {
        return Err(SkillPackageError::InvalidDescription {
            directory: directory_name.to_owned(),
            detail: format!(
                "description exceeds the {MAX_SKILL_DESCRIPTION_CHARS}-character standard bound"
            ),
        });
    }
    if let Some(compatibility) = &frontmatter.compatibility {
        let compatibility = compatibility.trim();
        if compatibility.is_empty() || compatibility.chars().count() > MAX_SKILL_COMPATIBILITY_CHARS
        {
            return Err(SkillPackageError::InvalidCompatibility {
                directory: directory_name.to_owned(),
                detail: format!(
                    "compatibility must be 1-{MAX_SKILL_COMPATIBILITY_CHARS} characters"
                ),
            });
        }
    }
    let dependencies = parse_dependency_map(&frontmatter.metadata).map_err(|detail| {
        SkillPackageError::InvalidDependencyDeclaration {
            directory: directory_name.to_owned(),
            detail: detail.to_string(),
        }
    })?;

    // Collect the accepted package file set: every regular file below the
    // package root, deterministically sorted by workspace-relative path,
    // with package-internal symlinks rejected.
    let mut files = Vec::new();
    walk_package_files(root, root, &mut files, &mut 4096)?;
    let version_id = package_version_id(root, &files, &markdown_bytes).map_err(|detail| {
        SkillPackageError::Io {
            path: root.display().to_string(),
            detail,
        }
    })?;
    Ok(SkillPackage {
        id: SkillId::new(directory_name.to_owned()),
        version_id,
        name: directory_name.to_owned(),
        description: description.to_owned(),
        metadata: frontmatter.metadata,
        license: frontmatter.license,
        compatibility: frontmatter.compatibility,
        allowed_tools: frontmatter.allowed_tools,
        dependencies,
        files,
        root: root.to_path_buf(),
        location,
        disable_model_invocation: frontmatter.disable_model_invocation,
        source,
    })
}

/// The model-visible location of one canonical package root's `SKILL.md`.
///
/// A non-UTF-8 ancestor is rejected rather than published lossily: the model
/// hands this string straight back to Read and Bash, so a replacement
/// character would name a path that does not exist, and snapshot equality
/// would stop comparing real locations.
fn published_location(root: &Path, skill_markdown: &Path) -> Result<String, SkillPackageError> {
    skill_markdown.to_str().map(str::to_owned).ok_or_else(|| {
        SkillPackageError::UnrepresentableRoot {
            path: root.to_string_lossy().into_owned(),
        }
    })
}

/// Recursively collects every regular file of the package with symlinks
/// rejected at every level.
fn walk_package_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
    remaining: &mut usize,
) -> Result<(), SkillPackageError> {
    if directory
        .strip_prefix(root)
        .map_or(usize::MAX, |path| path.components().count())
        > 64
    {
        return Err(SkillPackageError::Io {
            path: directory.display().to_string(),
            detail: "Skill directory depth exceeds 64".into(),
        });
    }
    let entries = std::fs::read_dir(directory).map_err(|error| SkillPackageError::Io {
        path: directory.display().to_string(),
        detail: error.to_string(),
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        *remaining = remaining
            .checked_sub(1)
            .ok_or_else(|| SkillPackageError::Io {
                path: directory.display().to_string(),
                detail: "Skill package exceeds 4096 entries".into(),
            })?;
        let entry = entry.map_err(|error| SkillPackageError::Io {
            path: directory.display().to_string(),
            detail: error.to_string(),
        })?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        let file_type =
            std::fs::symlink_metadata(&path).map_err(|error| SkillPackageError::Io {
                path: path.display().to_string(),
                detail: error.to_string(),
            })?;
        if file_type.file_type().is_symlink() {
            return Err(SkillPackageError::UnsupportedSymlink {
                path: path.display().to_string(),
            });
        }
        if file_type.is_dir() {
            walk_package_files(root, &path, files, remaining)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(root).map_err(|_| SkillPackageError::Io {
                path: path.display().to_string(),
                detail: "cannot relativize the package file".to_owned(),
            })?;
            files.push(relative.to_path_buf());
        }
    }
    Ok(())
}

/// Validates one Skill name against the Agent Skills naming rules:
/// 1-64 characters, lowercase letters, numbers, and hyphens only, no
/// leading/trailing or consecutive hyphens.
pub(crate) fn validate_skill_name(name: &str) -> Result<(), String> {
    let count = name.chars().count();
    if !(1..=MAX_SKILL_NAME_CHARS).contains(&count) {
        return Err(format!(
            "name length must be 1-{MAX_SKILL_NAME_CHARS} characters, got {count}"
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("name must not start or end with a hyphen".to_owned());
    }
    if name.contains("--") {
        return Err("name must not contain consecutive hyphens".to_owned());
    }
    if !name.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return Err("name may contain only lowercase letters, numbers, and hyphens".to_owned());
    }
    Ok(())
}

/// The parsed and shape-validated `SKILL.md` frontmatter.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frontmatter {
    name: String,
    description: String,
    metadata: BTreeMap<String, String>,
    license: Option<String>,
    compatibility: Option<String>,
    allowed_tools: Option<String>,
    disable_model_invocation: bool,
}

/// The serde target of the standard frontmatter fields.
///
/// `metadata` is parsed as a map of YAML values so the standard
/// string-to-string constraint is enforced explicitly: `serde_yaml` would
/// otherwise coerce scalar numbers into strings.
#[derive(serde::Deserialize)]
struct FrontmatterSerde {
    name: String,
    description: String,
    #[serde(default)]
    metadata: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: bool,
}

/// The frontmatter parse outcome distinguishes a malformed YAML block from
/// a metadata map that violates the standard string-to-string constraint.
#[derive(Debug)]
enum FrontmatterFailure {
    /// The YAML block is malformed or the standard fields have the wrong
    /// shape.
    Malformed(String),
    /// The `metadata` map contains a non-string value.
    InvalidMetadata(String),
}

impl From<FrontmatterFailure> for String {
    fn from(failure: FrontmatterFailure) -> Self {
        match failure {
            FrontmatterFailure::Malformed(detail) | FrontmatterFailure::InvalidMetadata(detail) => {
                detail
            }
        }
    }
}

/// Splits `SKILL.md` into its YAML frontmatter block and the Markdown body.
///
/// The frontmatter is the first `---`-delimited block at the start of the
/// file. A missing opening or closing delimiter is malformed.
fn parse_frontmatter(markdown: &str) -> Result<Frontmatter, FrontmatterFailure> {
    let Some(opening_end) = frontmatter_line_end(markdown, 0) else {
        return Err(FrontmatterFailure::Malformed(
            "SKILL.md must start with a YAML frontmatter block".to_owned(),
        ));
    };
    if line_content(&markdown[..opening_end]) != "---" {
        return Err(FrontmatterFailure::Malformed(
            "SKILL.md must start with a YAML frontmatter block".to_owned(),
        ));
    }
    let remainder = &markdown[opening_end..];
    let mut cursor = 0;
    let mut closing = None;
    while cursor < remainder.len() {
        let Some(end) = frontmatter_line_end(remainder, cursor) else {
            break;
        };
        if line_content(&remainder[cursor..end]) == "---" {
            closing = Some((cursor, end));
            break;
        }
        cursor = end;
    }
    let Some((closing_start, _closing_end)) = closing else {
        return Err(FrontmatterFailure::Malformed(
            "SKILL.md frontmatter is missing its closing delimiter".to_owned(),
        ));
    };
    let yaml_block = &remainder[..closing_start];
    let frontmatter: FrontmatterSerde = serde_yaml::from_str(yaml_block).map_err(|error| {
        FrontmatterFailure::Malformed(format!("frontmatter is not valid YAML: {error}"))
    })?;
    // The standard requires metadata to be a string-to-string map; a
    // non-string value is malformed (never coerced).
    let mut metadata = BTreeMap::new();
    for (key, value) in frontmatter.metadata {
        let serde_yaml::Value::String(string) = value else {
            return Err(FrontmatterFailure::InvalidMetadata(format!(
                "metadata entry {key:?} must be a string, got {value:?}"
            )));
        };
        metadata.insert(key, string);
    }
    Ok(Frontmatter {
        name: frontmatter.name,
        description: frontmatter.description,
        metadata,
        license: frontmatter.license,
        compatibility: frontmatter.compatibility,
        allowed_tools: frontmatter.allowed_tools,
        disable_model_invocation: frontmatter.disable_model_invocation,
    })
}

/// Returns the end offset (exclusive) of the line beginning at `start`.
/// An EOF-terminated final line is still a line; delimiter recognition remains
/// exact because only the complete line content `---` is accepted.
fn frontmatter_line_end(text: &str, start: usize) -> Option<usize> {
    if start >= text.len() {
        return None;
    }
    Some(
        text[start..]
            .find('\n')
            .map_or(text.len(), |relative| start + relative + 1),
    )
}

/// Removes only the line ending from one frontmatter line.
fn line_content(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

#[cfg(test)]
mod frontmatter_tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::{
        AutomaticSkillRoot, SkillDiscovery, SkillDiscoveryConfig, SkillPackage, SkillSource,
        parse_frontmatter,
    };
    use crate::skills::diagnostics::SkillDiagnostic;
    use crate::skills::source::{AutomaticSkillSource, automatic_skill_roots};
    use crate::skills::{SkillDiagnosticSeverity, SkillSnapshot};
    use crate::tools::Workspace;

    fn write_skill(root: &Path, name: &str, description: &str, extra: &str) {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).expect("skill directory");
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}{extra}\n---\nbody\n"),
        )
        .expect("skill file");
        std::fs::write(directory.join("references.md"), "reference\n")
            .expect("skill supporting resource");
    }

    /// One isolated filesystem fixture.
    ///
    /// The returned base is the **canonical** temporary root, and every root a
    /// test derives from it is therefore spelled the way discovery publishes
    /// it. That matters on hosts whose temporary directory is itself an alias
    /// (macOS hands out `/var/folders/...` for `/private/var/folders/...`):
    /// discovery canonicalizes every admitted package root exactly once, at
    /// the filesystem authority boundary, so a test comparing against the
    /// pre-canonical spelling would be asserting a host alias rather than the
    /// contract. `TempDir` still owns cleanup through the returned handle.
    fn workspace_fixture() -> (tempfile::TempDir, PathBuf, Workspace) {
        let directory = tempfile::tempdir().expect("temporary root");
        let base = directory
            .path()
            .canonicalize()
            .expect("canonical temporary root");
        let workspace_root = base.join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        (directory, base, workspace)
    }

    fn automatic(roots: &[(SkillSource, PathBuf)]) -> SkillDiscoveryConfig {
        SkillDiscoveryConfig {
            automatic: roots
                .iter()
                .map(|(source, root)| AutomaticSkillRoot {
                    source: *source,
                    root: root.clone(),
                })
                .collect(),
            explicit_paths: Vec::new(),
        }
    }

    fn names(packages: &[SkillPackage]) -> Vec<&str> {
        packages.iter().map(SkillPackage::name).collect()
    }

    #[test]
    fn accepts_lf_and_crlf_delimiter_lines() {
        let lf = parse_frontmatter("---\nname: pdf\ndescription: test\n---\nbody\n")
            .expect("LF frontmatter");
        assert_eq!(lf.name, "pdf");
        let crlf = parse_frontmatter("---\r\nname: pdf\r\ndescription: test\r\n---\r\nbody\r\n")
            .expect("CRLF frontmatter");
        assert_eq!(crlf.description, "test");
    }

    #[test]
    fn requires_exact_opening_and_closing_delimiter_lines() {
        assert!(parse_frontmatter("name: pdf\n---\nbody\n").is_err());
        assert!(parse_frontmatter("---\nname: pdf\ndescription: test\n---oops\nbody\n").is_err());
        assert!(parse_frontmatter("---\nname: pdf\ndescription: test\nbody\n").is_err());
    }

    #[test]
    fn delimiter_text_inside_a_quoted_scalar_is_not_a_boundary() {
        let parsed = parse_frontmatter(
            "---\nname: pdf\ndescription: 'text --- remains scalar'\n---\nbody\n",
        )
        .expect("quoted scalar");
        assert_eq!(parsed.description, "text --- remains scalar");
    }

    /// #280 (1): the default policy resolves exactly the two canonical
    /// automatic roots, against an isolated HOME rather than the developer's.
    #[test]
    fn cfg280_default_source_policy_scans_exactly_global_and_workspace() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        write_skill(&home.join(".agents/skills"), "alpha", "Alpha", "");
        write_skill(&workspace.root().join(".agents/skills"), "zeta", "Zeta", "");
        // A legacy rustX-config-relative root must contribute nothing.
        write_skill(&base.join("config/rustx/skills"), "legacy", "Legacy", "");
        let roots = automatic_skill_roots(
            Some(&home),
            workspace.root(),
            &crate::skills::default_automatic_sources(),
        );
        assert_eq!(
            roots
                .iter()
                .map(|root| (root.source, root.root.clone()))
                .collect::<Vec<_>>(),
            vec![
                (SkillSource::Global, home.join(".agents/skills")),
                (
                    SkillSource::Workspace,
                    workspace.root().join(".agents/skills")
                ),
            ]
        );
        let outcome = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic: roots,
                explicit_paths: Vec::new(),
            },
        )
        .discover()
        .expect("automatic discovery never fails");
        assert_eq!(names(&outcome.packages), ["alpha", "zeta"]);
    }

    /// #280 (2)(3): each single-source policy excludes the other root.
    #[test]
    fn cfg280_single_source_policies_exclude_the_other_root() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        write_skill(&home.join(".agents/skills"), "alpha", "Alpha", "");
        write_skill(&workspace.root().join(".agents/skills"), "zeta", "Zeta", "");
        for (selected, expected) in [
            (AutomaticSkillSource::Global, "alpha"),
            (AutomaticSkillSource::Workspace, "zeta"),
        ] {
            let outcome = SkillDiscovery::with_config(
                &workspace,
                SkillDiscoveryConfig {
                    automatic: automatic_skill_roots(
                        Some(&home),
                        workspace.root(),
                        &[selected].into_iter().collect(),
                    ),
                    explicit_paths: Vec::new(),
                },
            )
            .discover()
            .expect("single-source discovery");
            assert_eq!(names(&outcome.packages), [expected]);
        }
    }

    /// #280 (5): a missing automatic root is a benign empty set.
    #[test]
    fn cfg280_missing_automatic_roots_are_benign_empty_sets() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        let outcome = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic: automatic_skill_roots(
                    Some(&home),
                    workspace.root(),
                    &crate::skills::default_automatic_sources(),
                ),
                explicit_paths: Vec::new(),
            },
        )
        .discover()
        .expect("missing roots are not a failure");
        assert!(outcome.packages.is_empty());
        assert_eq!(outcome.diagnostics.len(), 2);
        assert!(
            outcome
                .diagnostics
                .iter()
                .all(|fact| fact.severity() == SkillDiagnosticSeverity::Fact
                    && matches!(fact, SkillDiagnostic::SourceRootMissing { .. }))
        );
    }

    /// #280 (11): one malformed package is excluded; unrelated valid
    /// packages in the same source still publish.
    #[test]
    fn cfg280_one_malformed_package_never_suppresses_valid_ones() {
        let (_temp, _base, workspace) = workspace_fixture();
        let root = workspace.root().join(".agents/skills");
        write_skill(&root, "rust-review", "Review Rust", "");
        write_skill(&root, "debugging", "Debug", "");
        std::fs::create_dir_all(root.join("broken")).expect("broken package");
        std::fs::write(root.join("broken/SKILL.md"), "not frontmatter at all\n")
            .expect("broken SKILL.md");
        let outcome =
            SkillDiscovery::with_config(&workspace, SkillDiscoveryConfig::workspace_root(root))
                .discover()
                .expect("a malformed package is not a discovery failure");
        assert_eq!(names(&outcome.packages), ["debugging", "rust-review"]);
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [SkillDiagnostic::PackageInvalid { package, .. }] if package.ends_with("broken")
        ));
    }

    /// #280 (13)(14)(16): workspace shadows global regardless of the order
    /// the roots are configured in, and the shadow is retained as provenance.
    #[test]
    fn cfg280_workspace_shadows_global_independently_of_configured_order() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        write_skill(&home.join(".agents/skills"), "foo", "Global foo", "");
        write_skill(
            &workspace.root().join(".agents/skills"),
            "foo",
            "Workspace foo",
            "",
        );
        let global = (SkillSource::Global, home.join(".agents/skills"));
        let project = (
            SkillSource::Workspace,
            workspace.root().join(".agents/skills"),
        );
        let forward =
            SkillDiscovery::with_config(&workspace, automatic(&[global.clone(), project.clone()]))
                .discover()
                .expect("forward order");
        let reversed = SkillDiscovery::with_config(&workspace, automatic(&[project, global]))
            .discover()
            .expect("reversed order");
        assert_eq!(forward, reversed);
        assert_eq!(names(&forward.packages), ["foo"]);
        assert_eq!(forward.packages[0].description(), "Workspace foo");
        assert_eq!(forward.packages[0].source(), SkillSource::Workspace);
        let provenance = &forward.provenance[0];
        assert_eq!(provenance.source, SkillSource::Workspace);
        assert_eq!(provenance.shadowed.len(), 1);
        assert_eq!(provenance.shadowed[0].source, SkillSource::Global);
        assert!(
            provenance.shadowed[0]
                .location
                .starts_with(home.join(".agents/skills").to_str().expect("utf-8 home"))
        );
        assert!(matches!(
            forward.diagnostics.as_slice(),
            [SkillDiagnostic::Shadowed {
                effective_source: SkillSource::Workspace,
                shadowed_source: SkillSource::Global,
                ..
            }]
        ));
    }

    /// #280 (13): an invalid higher-precedence candidate never wins merely
    /// by being in the higher-precedence source.
    #[test]
    fn cfg280_an_invalid_workspace_candidate_does_not_shadow_a_valid_global_one() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        write_skill(&home.join(".agents/skills"), "foo", "Global foo", "");
        let project = workspace.root().join(".agents/skills/foo");
        std::fs::create_dir_all(&project).expect("workspace package");
        std::fs::write(
            project.join("SKILL.md"),
            "---\nname: bar\ndescription: x\n---\n",
        )
        .expect("mismatched name");
        let outcome = SkillDiscovery::with_config(
            &workspace,
            automatic(&[
                (SkillSource::Global, home.join(".agents/skills")),
                (
                    SkillSource::Workspace,
                    workspace.root().join(".agents/skills"),
                ),
            ]),
        )
        .discover()
        .expect("discovery");
        assert_eq!(names(&outcome.packages), ["foo"]);
        assert_eq!(outcome.packages[0].source(), SkillSource::Global);
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [SkillDiagnostic::PackageInvalid { .. }]
        ));
    }

    /// #280 (15): a same-scope logical conflict excludes every conflicting
    /// definition rather than picking an arbitrary winner.
    #[test]
    fn cfg280_same_scope_duplicates_exclude_every_definition() {
        let (_temp, base, workspace) = workspace_fixture();
        let first = base.join("first");
        let second = base.join("second");
        write_skill(&first, "same", "First", "");
        write_skill(&second, "same", "Second", "");
        write_skill(&first, "other", "Other", "");
        let forward = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig::explicit(vec![
                first.join("same"),
                second.join("same"),
                first.join("other"),
            ]),
        )
        .discover()
        .expect("explicit paths exist");
        let reversed = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig::explicit(vec![
                first.join("other"),
                second.join("same"),
                first.join("same"),
            ]),
        )
        .discover()
        .expect("explicit paths exist");
        assert_eq!(forward, reversed);
        assert_eq!(names(&forward.packages), ["other"]);
        let [
            SkillDiagnostic::DuplicateIdentity {
                source,
                name,
                packages,
            },
        ] = forward.diagnostics.as_slice()
        else {
            panic!(
                "expected one duplicate-identity fact, got {:?}",
                forward.diagnostics
            );
        };
        assert_eq!(*source, SkillSource::Explicit);
        assert_eq!(name, "same");
        assert_eq!(packages.len(), 2);
        assert!(packages[0] < packages[1]);
    }

    /// #280 (23): the explicit launch authority passes through the same
    /// normative validation, and a package it names that is malformed is
    /// excluded rather than silently admitted.
    #[test]
    fn cfg280_explicit_paths_use_the_same_validation_and_win_the_merge() {
        let (_temp, base, workspace) = workspace_fixture();
        write_skill(
            &workspace.root().join(".agents/skills"),
            "guide",
            "Workspace guide",
            "",
        );
        let explicit = base.join("explicit");
        write_skill(&explicit, "guide", "Explicit guide", "");
        let outcome = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic: vec![AutomaticSkillRoot {
                    source: SkillSource::Workspace,
                    root: workspace.root().join(".agents/skills"),
                }],
                explicit_paths: vec![explicit.join("guide")],
            },
        )
        .discover()
        .expect("explicit path exists");
        assert_eq!(outcome.packages[0].source(), SkillSource::Explicit);
        assert_eq!(outcome.packages[0].description(), "Explicit guide");
        assert_eq!(
            outcome.provenance[0].shadowed[0].source,
            SkillSource::Workspace
        );
    }

    /// A missing explicit path remains a launch-authority error: it is
    /// authored intent, not discovered content.
    #[test]
    fn cfg280_a_missing_explicit_path_is_a_launch_error() {
        let (_temp, base, workspace) = workspace_fixture();
        let error = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig::explicit(vec![base.join("absent")]),
        )
        .discover()
        .expect_err("missing explicit path");
        assert!(error.to_string().contains("does not exist"));
    }

    /// #280 (12): a symlinked package root is excluded as the package-level
    /// fact it is, without suppressing its source.
    #[test]
    fn cfg280_a_symlinked_package_root_is_excluded_not_fatal() {
        let (_temp, base, workspace) = workspace_fixture();
        let root = workspace.root().join(".agents/skills");
        write_skill(&root, "valid", "Valid", "");
        write_skill(&base, "linked", "Linked", "");
        std::os::unix::fs::symlink(base.join("linked"), root.join("linked")).expect("symlink");
        let outcome =
            SkillDiscovery::with_config(&workspace, SkillDiscoveryConfig::workspace_root(root))
                .discover()
                .expect("discovery");
        assert_eq!(names(&outcome.packages), ["valid"]);
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [SkillDiagnostic::PackageInvalid {
                cause: super::SkillPackageError::UnsupportedSymlink { .. },
                ..
            }]
        ));
    }

    /// A source root that exists but cannot be scanned excludes only that
    /// source; every other source still publishes.
    #[test]
    fn cfg280_an_unusable_source_root_excludes_only_that_source() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        std::fs::create_dir_all(home.join(".agents")).expect("home agents");
        std::fs::write(home.join(".agents/skills"), "not a directory").expect("file root");
        write_skill(&workspace.root().join(".agents/skills"), "zeta", "Zeta", "");
        let outcome = SkillDiscovery::with_config(
            &workspace,
            automatic(&[
                (SkillSource::Global, home.join(".agents/skills")),
                (
                    SkillSource::Workspace,
                    workspace.root().join(".agents/skills"),
                ),
            ]),
        )
        .discover()
        .expect("an unusable root is not a failure");
        assert_eq!(names(&outcome.packages), ["zeta"]);
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [SkillDiagnostic::SourceRootInvalid {
                source: SkillSource::Global,
                ..
            }]
        ));
    }

    #[test]
    fn discovery_merges_bounded_roots_in_deterministic_identity_order() {
        let (_temp, base, workspace) = workspace_fixture();
        let user_agents = base.join("user/.agents/skills");
        let project_agents = workspace.root().join(".agents/skills");
        let explicit = base.join("explicit/skills");
        write_skill(&project_agents, "zeta", "Zeta", "");
        write_skill(&user_agents, "alpha", "Alpha", "");
        write_skill(&explicit, "middle", "Middle", "");

        let outcome = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic: vec![
                    AutomaticSkillRoot {
                        source: SkillSource::Workspace,
                        root: project_agents,
                    },
                    AutomaticSkillRoot {
                        source: SkillSource::Global,
                        root: user_agents,
                    },
                ],
                explicit_paths: vec![explicit],
            },
        )
        .discover()
        .expect("roots discover");
        assert_eq!(names(&outcome.packages), vec!["alpha", "middle", "zeta"]);
        assert_eq!(
            outcome.packages[1].files(),
            &[
                std::path::PathBuf::from("SKILL.md"),
                std::path::PathBuf::from("references.md")
            ]
        );
    }

    /// #280 (22): the obsolete rustX-config-relative Skill root is not a
    /// discovery location, a fallback, or a migration path.
    #[test]
    fn cfg280_the_legacy_config_relative_skill_root_is_never_read() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        let legacy = home.join(".config/rustx/skills/ignored/SKILL.md");
        std::fs::create_dir_all(legacy.parent().expect("parent")).expect("legacy root");
        std::fs::write(
            &legacy,
            "unrelated legacy user bytes; not valid frontmatter",
        )
        .expect("legacy bytes");
        let outcome = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic: automatic_skill_roots(
                    Some(&home),
                    workspace.root(),
                    &crate::skills::default_automatic_sources(),
                ),
                explicit_paths: Vec::new(),
            },
        )
        .discover()
        .expect("discovery");
        assert!(outcome.packages.is_empty());
        assert_eq!(
            std::fs::read_to_string(legacy).expect("legacy bytes preserved"),
            "unrelated legacy user bytes; not valid frontmatter"
        );
    }

    /// Path identity: every **admitted** location is the canonical host path,
    /// whatever spelling the caller configured.
    ///
    /// This is the platform-independent statement of the contract a host alias
    /// exposes. macOS hands out `/var/folders/...` for `/private/var/...`, so
    /// on that host a tempdir-derived root reaches discovery pre-aliased; here
    /// the alias is constructed explicitly with a symlinked *ancestor*, so the
    /// same semantics are proven identically on every platform. Discovery
    /// canonicalizes once, at the filesystem authority boundary, and every
    /// published location — the effective package, its provenance, and the
    /// provenance of the package it shadowed — is that canonical path. Nothing
    /// downstream may re-resolve a published path and reach a different file.
    #[cfg(unix)]
    #[test]
    fn cfg280_admitted_locations_are_canonical_whatever_spelling_is_configured() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        write_skill(&home.join(".agents/skills"), "foo", "Global foo", "");
        write_skill(
            &workspace.root().join(".agents/skills"),
            "foo",
            "Workspace foo",
            "",
        );
        // An aliased spelling of the very same global root: `alias` is a
        // symlink to `home`, so `alias/.agents/skills` is a real directory
        // reached through a link, exactly like macOS's `/var` alias.
        let alias = base.join("alias");
        std::os::unix::fs::symlink(&home, &alias).expect("home alias");
        let canonical = SkillDiscovery::with_config(
            &workspace,
            automatic(&[
                (SkillSource::Global, home.join(".agents/skills")),
                (
                    SkillSource::Workspace,
                    workspace.root().join(".agents/skills"),
                ),
            ]),
        )
        .discover()
        .expect("canonical spelling");
        let aliased = SkillDiscovery::with_config(
            &workspace,
            automatic(&[
                (SkillSource::Global, alias.join(".agents/skills")),
                (
                    SkillSource::Workspace,
                    workspace.root().join(".agents/skills"),
                ),
            ]),
        )
        .discover()
        .expect("aliased spelling");
        // The alias is not a second Skill plane: it publishes the identical
        // generation, down to the shadowed provenance location.
        assert_eq!(canonical, aliased);
        let expected_shadow = home
            .join(".agents/skills/foo/SKILL.md")
            .to_str()
            .expect("utf-8 home")
            .to_owned();
        for outcome in [&canonical, &aliased] {
            let provenance = &outcome.provenance[0];
            assert_eq!(provenance.source, SkillSource::Workspace);
            assert_eq!(
                provenance.location,
                workspace
                    .root()
                    .join(".agents/skills/foo/SKILL.md")
                    .to_str()
                    .expect("utf-8 workspace")
            );
            assert_eq!(provenance.shadowed.len(), 1);
            // The admitted location is the canonical host path, never the
            // aliased spelling discovery was handed.
            assert_eq!(provenance.shadowed[0].location, expected_shadow);
            assert_eq!(
                outcome.packages[0].location(),
                provenance.location,
                "the catalog publishes exactly the provenance location"
            );
        }
        assert!(
            !aliased.provenance[0].shadowed[0]
                .location
                .starts_with(alias.to_str().expect("utf-8 alias")),
            "an aliased ancestor must never reach a published location"
        );
    }

    /// The cumulative per-source candidate budget: exact at the limit, one
    /// over is an exclusion, and every root of one source draws from the same
    /// counter.
    ///
    /// The budget is lowered through a deterministic seam rather than by
    /// materializing `MAX_SOURCE_SKILL_PACKAGES` + 1 real package trees: the
    /// arithmetic and every reporting path are the production ones.
    #[test]
    fn cfg280_one_cumulative_candidate_budget_per_source() {
        let (_temp, base, workspace) = workspace_fixture();
        let global = base.join("home/.agents/skills");
        let project = workspace.root().join(".agents/skills");
        for name in ["g-one", "g-two", "g-three"] {
            write_skill(&global, name, "Global", "");
        }
        write_skill(&project, "p-one", "Workspace", "");
        let config = || {
            automatic(&[
                (SkillSource::Global, global.clone()),
                (SkillSource::Workspace, project.clone()),
            ])
        };

        // Exactly at the limit: the source publishes completely.
        let exact = SkillDiscovery::with_source_budget(config(), 3)
            .discover()
            .expect("automatic discovery never fails on a budget");
        assert_eq!(
            names(&exact.packages),
            ["g-one", "g-three", "g-two", "p-one"]
        );
        assert!(
            !exact
                .diagnostics
                .iter()
                .any(|fact| matches!(fact, SkillDiagnostic::SourceBudgetExceeded { .. })),
            "{:?}",
            exact.diagnostics
        );

        // One over: that source is excluded, and only that source. The
        // budget belongs to the source, so the workspace source — well inside
        // its own budget — still publishes.
        let over = SkillDiscovery::with_source_budget(config(), 2)
            .discover()
            .expect("a budget overrun is not a discovery failure");
        assert_eq!(names(&over.packages), ["p-one"]);
        let [
            SkillDiagnostic::SourceBudgetExceeded {
                source,
                roots,
                candidates,
                limit,
            },
        ] = over.diagnostics.as_slice()
        else {
            panic!("expected one budget fact, got {:?}", over.diagnostics);
        };
        assert_eq!(*source, SkillSource::Global);
        assert_eq!(roots, &[global.display().to_string()]);
        assert_eq!((*candidates, *limit), (3, 2));
        assert_eq!(
            over.diagnostics[0].severity(),
            SkillDiagnosticSeverity::Warning
        );

        // Several roots configured for the *same* source share that source's
        // one budget, in either configured order: four candidates spread over
        // two global roots fit a budget of four and overrun a budget of three,
        // even though either root alone fits inside three.
        let split = base.join("home-two/.agents/skills");
        write_skill(&split, "g-four", "Global", "");
        let two_roots = |reverse: bool| {
            let mut roots = vec![
                (SkillSource::Global, global.clone()),
                (SkillSource::Global, split.clone()),
            ];
            if reverse {
                roots.reverse();
            }
            automatic(&roots)
        };
        for reverse in [false, true] {
            let outcome = SkillDiscovery::with_source_budget(two_roots(reverse), 4)
                .discover()
                .expect("exactly at the shared budget");
            assert_eq!(
                names(&outcome.packages),
                ["g-four", "g-one", "g-three", "g-two"]
            );
            let over = SkillDiscovery::with_source_budget(two_roots(reverse), 3)
                .discover()
                .expect("a budget overrun is not a discovery failure");
            assert!(over.packages.is_empty(), "the source is excluded whole");
            let [
                SkillDiagnostic::SourceBudgetExceeded {
                    roots, candidates, ..
                },
            ] = over.diagnostics.as_slice()
            else {
                panic!("expected one budget fact, got {:?}", over.diagnostics);
            };
            // Canonically ordered and order-independent: both the decision and
            // the number it reports are functions of the source alone.
            assert_eq!(
                roots,
                &[global.display().to_string(), split.display().to_string()]
            );
            assert_eq!(*candidates, 4);
        }
    }

    /// The complete `Explicit` source shares **one** cumulative budget across
    /// every explicit collection path and named package, and the result cannot
    /// depend on the order those paths are enumerated in.
    #[test]
    fn cfg280_explicit_collection_roots_share_one_cumulative_budget() {
        let (_temp, base, _workspace) = workspace_fixture();
        let first = base.join("first");
        let second = base.join("second");
        write_skill(&first, "alpha", "Alpha", "");
        write_skill(&first, "beta", "Beta", "");
        write_skill(&second, "gamma", "Gamma", "");
        write_skill(&second, "delta", "Delta", "");

        // Four candidates spread over two collection roots fit a budget of
        // four: the roots share it rather than each claiming a full one.
        let outcome = SkillDiscovery::with_source_budget(
            SkillDiscoveryConfig::explicit(vec![first.clone(), second.clone()]),
            4,
        )
        .discover()
        .expect("exactly at the cumulative budget");
        assert_eq!(
            names(&outcome.packages),
            ["alpha", "beta", "delta", "gamma"]
        );

        // A budget of three would be enough for either root alone, and is a
        // hard launch error for the authored explicit authority as a whole.
        for paths in [
            vec![first.clone(), second.clone()],
            vec![second.clone(), first.clone()],
            // The same total spread over a collection root plus two named
            // package paths charges the identical cumulative amount.
            vec![first.clone(), second.join("gamma"), second.join("delta")],
        ] {
            let error = SkillDiscovery::with_source_budget(
                SkillDiscoveryConfig::explicit(paths.clone()),
                3,
            )
            .discover()
            .expect_err("one over the cumulative budget");
            assert_eq!(
                error,
                super::SkillDiscoveryError::TooManyExplicitPackages { count: 4 },
                "{paths:?}"
            );
        }
    }

    #[test]
    fn no_automatic_roots_still_loads_explicit_skill_and_maps_resources() {
        let (_temp, base, workspace) = workspace_fixture();
        let explicit = base.join("user/skills");
        write_skill(
            &explicit,
            "private-guide",
            "Private guide",
            "\ndisable-model-invocation: true",
        );
        let outcome =
            SkillDiscovery::with_config(&workspace, SkillDiscoveryConfig::explicit(vec![explicit]))
                .discover()
                .expect("explicit Skill path");
        assert_eq!(outcome.packages.len(), 1);
        assert!(outcome.packages[0].disable_model_invocation());
        let snapshot = SkillSnapshot::new(outcome.packages.into_iter().map(Arc::new).collect());
        assert_eq!(snapshot.packages().len(), 1);
        assert!(snapshot.catalog_entries().is_empty());
        assert!(snapshot.visible_bindings().is_empty());
        assert_eq!(snapshot.bindings().len(), 1);
        // The package is hidden from the catalog but still tracked, so its
        // host location participates in snapshot equality.
        assert_eq!(snapshot.locations().len(), 1);
        assert!(snapshot.locations()[0].ends_with("private-guide/SKILL.md"));
    }
}
