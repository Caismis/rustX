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
//! Discovery follows ordered User and Workspace collections, stopping
//! recursion at the first SKILL.md in a directory. A standalone Markdown
//! instruction file can also define a Skill. The first valid definition for a
//! name wins. Skill metadata is frozen for a generation, and the prompt
//! contains the real path so the model can load instructions on demand.
//!
//! # Resource boundary
//!
//! Skill packages remain current filesystem resources. Discovery freezes
//! identities, versions, catalog metadata, and dependency declarations at
//! candidate-generation time. Package *bodies* are read at use time through
//! ordinary tool semantics — discovery never loads a `SKILL.md` body into
//! model context. Only definition edits publish a later generation at the
//! existing safe boundary. Bodies and assets remain live Tool inputs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::runtime::identity::{SkillId, SkillVersionId};
use crate::skills::dependencies::{DependencyManifest, parse_dependency_map};
use crate::skills::diagnostics::{ShadowedSkill, SkillDiagnostic, SkillProvenance};
use crate::skills::identity::definition_version_id;
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

/// The maximum number of direct entries one Skill collection *directory* may
/// hold.
///
/// This bounds one `read_dir`, so it is deliberately per directory: it is the
/// cost of enumerating that directory, not the cost of the source.
pub const MAX_SKILL_ROOT_ENTRIES: usize = 1024;
/// Maximum candidate packages in one fixed User or Workspace collection.
/// Candidates consume this budget before parsing, including malformed packages.
pub const MAX_SOURCE_SKILL_PACKAGES: usize = 128;

/// A parsing/validation failure of **one** Skill package.
///
/// Every variant identifies the responsible candidate. A failure here
/// excludes exactly that candidate and is preserved verbatim inside
/// [`SkillDiagnostic::PackageInvalid`]; it never fails discovery, and it
/// never suppresses an unrelated valid package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum SkillPackageError {
    /// The package name violates the Agent Skills naming rules.
    InvalidName {
        directory: String,
        #[serde(skip)]
        name: String,
        #[serde(skip)]
        detail: String,
    },
    /// The frontmatter `name` does not match the parent directory name.
    NameDirectoryMismatch {
        directory: String,
        #[serde(skip)]
        name: String,
    },
    /// The candidate directory contains no `SKILL.md`.
    MissingSkillMarkdown { directory: String },
    /// The `SKILL.md` entry is not an ordinary regular file (or is a
    /// symlink).
    SkillMarkdownNotRegularFile { directory: String },
    /// The `SKILL.md` frontmatter is malformed YAML or violates the
    /// standard shape.
    MalformedFrontmatter {
        directory: String,
        #[serde(skip)]
        detail: String,
    },
    /// The standard description is empty or exceeds the length bound.
    InvalidDescription {
        directory: String,
        #[serde(skip)]
        detail: String,
    },
    /// The standard `compatibility` field exceeds its length bound.
    InvalidCompatibility {
        directory: String,
        #[serde(skip)]
        detail: String,
    },
    /// The `metadata` field is not a string-to-string map.
    MalformedMetadata {
        directory: String,
        #[serde(skip)]
        detail: String,
    },
    /// A rustX dependency declaration is malformed or unsupported.
    InvalidDependencyDeclaration {
        directory: String,
        #[serde(skip)]
        detail: String,
    },
    /// A symlinked package root was found. Internal resources are validated
    /// by the Tool that actually opens them, never by discovery.
    UnsupportedSymlink { path: String },
    /// The canonical package root is not losslessly representable as UTF-8,
    /// so it cannot be published as a model-visible location.
    UnrepresentableRoot { path: String },
    /// A filesystem failure while reading the package.
    Io {
        path: String,
        #[serde(skip)]
        detail: String,
    },
}

impl SkillPackageError {
    pub(crate) fn redacted(&self) -> Self {
        let mut error = self.clone();
        match &mut error {
            Self::InvalidName { name, detail, .. } => {
                name.clear();
                detail.clear();
            }
            Self::NameDirectoryMismatch { name, .. } => name.clear(),
            Self::MalformedFrontmatter { detail, .. }
            | Self::InvalidDescription { detail, .. }
            | Self::InvalidCompatibility { detail, .. }
            | Self::MalformedMetadata { detail, .. }
            | Self::InvalidDependencyDeclaration { detail, .. }
            | Self::Io { detail, .. } => detail.clear(),
            _ => {}
        }
        error
    }
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

/// One discovered and validated Skill package.
///
/// The package is immutable after discovery: its `SkillVersionId` is
/// derived only from its normalized frontmatter definition, and its dependency
/// declarations are already parsed and normalized. `location` is the canonical,
/// absolute model-visible host path of its
/// `SKILL.md` — see the package root invariant in the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPackage {
    id: SkillId,
    version_id: SkillVersionId,
    definition: Frontmatter,
    dependencies: DependencyManifest,
    location: String,
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

    /// The normalized frontmatter definition version, independent of body/assets.
    #[must_use]
    pub fn version_id(&self) -> &SkillVersionId {
        &self.version_id
    }

    /// The standard validated Skill name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.definition.name
    }

    /// The standard validated Skill description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.definition.description
    }

    /// The preserved standard `metadata` map (including the rustX
    /// dependency declaration keys).
    #[must_use]
    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.definition.metadata
    }

    /// The preserved standard optional `license` field.
    #[must_use]
    pub fn license(&self) -> Option<&str> {
        self.definition.license.as_deref()
    }

    /// The preserved standard optional `compatibility` field.
    #[must_use]
    pub fn compatibility(&self) -> Option<&str> {
        self.definition.compatibility.as_deref()
    }

    /// The preserved standard optional `allowed-tools` field. M6 parses and
    /// preserves it but invents no runtime policy for it.
    #[must_use]
    pub fn allowed_tools(&self) -> Option<&str> {
        self.definition.allowed_tools.as_deref()
    }

    /// The parsed and normalized rustX dependency declarations.
    #[must_use]
    pub fn dependencies(&self) -> &DependencyManifest {
        &self.dependencies
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
        self.definition.disable_model_invocation
    }
    /// Frozen normalized definition metadata for the configuration Journal.
    pub(crate) fn definition(&self) -> serde_json::Value {
        serde_json::to_value(&self.definition).expect("validated Skill definition serializes")
    }
}

/// The configured Skill discovery authorities of one candidate generation.
///
/// This is *where packages may be found*, never *which Skills an Agent may
/// see*. The two roots come from the process-bound User home and the Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillDiscoveryConfig {
    /// Resolved automatic source roots. A missing root is an empty set.
    pub automatic: Vec<AutomaticSkillRoot>,
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
    /// Winning malformed identities, retained for source inventory and shadowing.
    pub invalid: Vec<SkillProvenance>,
    /// Resolved collection roots, including roots with no packages.
    pub roots: Vec<AutomaticSkillRoot>,
    /// The effective packages, ordered by validated Skill name.
    pub packages: Vec<SkillPackage>,
    /// The provenance of every effective identity, ordered by name.
    pub provenance: Vec<SkillProvenance>,
    /// Canonically ordered generation-scoped diagnostics.
    pub diagnostics: Vec<SkillDiagnostic>,
}

impl SkillDiscoveryOutcome {
    /// Revision of the discoverable definitions, independent of body or asset bytes.
    pub(crate) fn revision(&self) -> String {
        use sha2::{Digest, Sha256};
        let facts = serde_json::to_vec(&(
            &self.roots,
            &self.provenance,
            &self.invalid,
            self.diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            self.packages
                .iter()
                .map(|package| (package.id(), package.version_id()))
                .collect::<Vec<_>>(),
        ))
        .expect("Skill discovery facts serialize");
        format!("sha256:{:x}", Sha256::digest(facts))
    }
}

/// Discovers Skill packages across the configured bounded authorities.
#[derive(Debug, Clone)]
pub struct SkillDiscovery {
    workspace: Option<PathBuf>,
    config: SkillDiscoveryConfig,
    /// The cumulative candidate budget of **one logical source**. Always
    /// [`MAX_SOURCE_SKILL_PACKAGES`] in production; a test may lower it to
    /// prove the boundary without materializing hundreds of package trees.
    source_budget: usize,
    cancellation: crate::runtime::CancellationSignal,
}

/// A valid definition or an invalid declared file for source inspection.
#[derive(Debug)]
struct Candidate {
    source: SkillSource,
    name: String,
    location: String,
    package: Option<SkillPackage>,
}

impl SkillDiscovery {
    /// Discover only the process-bound User collections.
    #[must_use]
    pub fn user(home: &Path) -> Self {
        Self {
            workspace: None,
            config: SkillDiscoveryConfig {
                automatic: super::source::user_skill_roots(home),
            },
            source_budget: MAX_SOURCE_SKILL_PACKAGES,
            cancellation: crate::runtime::CancellationSignal::new(),
        }
    }

    /// Discover one explicitly bound user collection.
    #[must_use]
    pub fn user_root(root: PathBuf) -> Self {
        Self {
            workspace: None,
            config: SkillDiscoveryConfig {
                automatic: vec![AutomaticSkillRoot {
                    source: SkillSource::User,
                    root,
                }],
            },
            source_budget: MAX_SOURCE_SKILL_PACKAGES,
            cancellation: crate::runtime::CancellationSignal::new(),
        }
    }

    /// Creates discovery with explicit current runtime authorities.
    ///
    /// There is deliberately no process-environment constructor: the
    /// launch/environment owner resolves the automatic roots (including the
    /// global root, from its captured home directory) and hands them here, so
    /// a test injects an isolated HOME without touching process-global state.
    #[must_use]
    pub fn with_config(workspace: &Workspace, config: SkillDiscoveryConfig) -> Self {
        Self {
            workspace: Some(workspace.root().to_path_buf()),
            config,
            source_budget: MAX_SOURCE_SKILL_PACKAGES,
            cancellation: crate::runtime::CancellationSignal::new(),
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
            workspace: None,
            config,
            source_budget,
            cancellation: crate::runtime::CancellationSignal::new(),
        }
    }

    /// Attach the native capture owner's cancellation authority to chunked I/O.
    #[must_use]
    pub fn with_cancellation(mut self, cancellation: crate::runtime::CancellationSignal) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Discovers the effective Skill set in source and collection precedence.
    #[must_use]
    pub fn discover(&self) -> SkillDiscoveryOutcome {
        let mut diagnostics = Vec::new();
        let mut candidates = Vec::new();
        let automatic = &self.config.automatic;
        for source in [SkillSource::Workspace, SkillSource::User] {
            let roots: Vec<_> = automatic
                .iter()
                .filter(|root| root.source == source)
                .collect();
            candidates.extend(collect_automatic_source(
                source,
                &roots,
                self.workspace.as_deref(),
                self.source_budget,
                &mut diagnostics,
                &self.cancellation,
            ));
        }
        let mut outcome = merge_candidates(candidates, diagnostics);
        outcome.roots = automatic
            .iter()
            .map(|root| AutomaticSkillRoot {
                source: root.source,
                root: std::fs::canonicalize(&root.root).unwrap_or_else(|_| root.root.clone()),
            })
            .collect();
        outcome.roots.sort();
        outcome.roots.dedup();
        outcome
    }
}

/// First valid definition wins. Repeated aliases of one file are inert.
fn merge_candidates(
    candidates: Vec<Candidate>,
    mut diagnostics: Vec<SkillDiagnostic>,
) -> SkillDiscoveryOutcome {
    let mut identities: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for candidate in candidates {
        if seen.insert(candidate.location.clone()) {
            identities
                .entry(candidate.name.clone())
                .or_default()
                .push(candidate);
        }
    }
    let mut packages = Vec::new();
    let mut provenance = Vec::new();
    let mut invalid = Vec::new();
    for (name, mut candidates) in identities {
        let index = candidates
            .iter()
            .position(|candidate| candidate.package.is_some())
            .unwrap_or(0);
        let winner = candidates.remove(index);
        let mut shadowed = Vec::new();
        let mut duplicates = vec![winner.location.clone()];
        for loser in candidates
            .into_iter()
            .filter(|candidate| candidate.package.is_some())
        {
            if winner.source == loser.source {
                duplicates.push(loser.location.clone());
            } else {
                diagnostics.push(SkillDiagnostic::Shadowed {
                    name: name.clone(),
                    effective_source: winner.source,
                    effective_location: winner.location.clone(),
                    shadowed_source: loser.source,
                    shadowed_location: loser.location.clone(),
                });
            }
            shadowed.push(ShadowedSkill {
                source: loser.source,
                location: loser.location,
            });
        }
        if duplicates.len() > 1 {
            diagnostics.push(SkillDiagnostic::DuplicateIdentity {
                source: winner.source,
                name: name.clone(),
                packages: duplicates,
            });
        }
        shadowed.sort();
        let record = SkillProvenance {
            name,
            source: winner.source,
            location: winner.location,
            shadowed,
        };
        if let Some(package) = winner.package {
            packages.push(package);
            provenance.push(record);
        } else {
            invalid.push(record);
        }
    }
    diagnostics.sort();
    diagnostics.dedup();
    SkillDiscoveryOutcome {
        invalid,
        roots: Vec::new(),
        packages,
        provenance,
        diagnostics,
    }
}

fn collect_automatic_source(
    source: SkillSource,
    roots: &[&AutomaticSkillRoot],
    workspace: Option<&Path>,
    budget: usize,
    diagnostics: &mut Vec<SkillDiagnostic>,
    cancellation: &crate::runtime::CancellationSignal,
) -> Vec<Candidate> {
    let mut files = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for root in roots {
        let owner = root.workspace_boundary(workspace);
        crate::bounded_file::record_read(&root.root);
        let canonical = match super::discovery::canonical_root(&root.root, owner) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                diagnostics.push(SkillDiagnostic::SourceRootMissing {
                    source,
                    root: root.root.display().to_string(),
                });
                continue;
            }
            Err(error) => {
                diagnostics.push(SkillDiagnostic::SourceRootInvalid {
                    source,
                    root: root.root.display().to_string(),
                    detail: error.to_string(),
                });
                continue;
            }
        };
        let boundary = owner.map(|_| canonical.as_path());
        let collection = super::discovery::collect(
            &root.root,
            super::discovery::is_shared(&root.root),
            boundary,
            cancellation,
        );
        if let Some(error) = collection.error {
            diagnostics.push(SkillDiagnostic::SourceRootInvalid {
                source,
                root: root.root.display().to_string(),
                detail: error.to_string(),
            });
            continue;
        }
        for file in collection.files {
            crate::bounded_file::record_read(&file);
            let path = match std::fs::canonicalize(&file) {
                Ok(path) if boundary.is_none_or(|boundary| path.starts_with(boundary)) => path,
                _ => continue,
            };
            if seen.insert(path.clone()) {
                files.push((file, path));
            }
        }
    }
    if files.len() > budget {
        let mut root_paths: Vec<_> = roots
            .iter()
            .map(|root| root.root.display().to_string())
            .collect();
        root_paths.sort();
        diagnostics.push(SkillDiagnostic::SourceBudgetExceeded {
            source,
            roots: root_paths,
            candidates: files.len(),
            limit: budget,
        });
        return Vec::new();
    }
    files
        .into_iter()
        .filter_map(|(file, canonical)| {
            let declared = file
                .file_name()
                .is_some_and(|name| name == SKILL_MARKDOWN_FILE);
            let fallback_name = file.parent()?.file_name()?.to_string_lossy().into_owned();
            let location = canonical.display().to_string();
            let result = discover_package(&canonical, &fallback_name, source, cancellation);
            match result {
                Ok(package) => Some(Candidate {
                    source,
                    name: package.name().to_owned(),
                    location: package.location.clone(),
                    package: Some(package),
                }),
                Err(cause) if declared => {
                    diagnostics.push(SkillDiagnostic::PackageInvalid {
                        source,
                        package: location.clone(),
                        cause,
                    });
                    Some(Candidate {
                        source,
                        name: fallback_name,
                        location,
                        package: None,
                    })
                }
                Err(_) => None, // Ordinary Markdown without a Skill definition is inert.
            }
        })
        .collect()
}

/// Parses, validates, and hashes one Skill instruction file.
fn discover_package(
    path: &Path,
    directory_name: &str,
    source: SkillSource,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<SkillPackage, SkillPackageError> {
    let location = path
        .to_str()
        .ok_or_else(|| SkillPackageError::UnrepresentableRoot {
            path: path.to_string_lossy().into_owned(),
        })?
        .to_owned();
    let malformed = |detail| SkillPackageError::MalformedFrontmatter {
        directory: directory_name.to_owned(),
        detail,
    };
    let header = super::frontmatter::read(path, cancellation).map_err(malformed)?;
    let markdown = std::str::from_utf8(&header).map_err(|error| malformed(error.to_string()))?;
    let mut frontmatter = parse_frontmatter(markdown).map_err(|failure| match failure {
        FrontmatterFailure::Malformed(detail) => malformed(detail),
        FrontmatterFailure::InvalidMetadata(detail) => SkillPackageError::MalformedMetadata {
            directory: directory_name.to_owned(),
            detail,
        },
    })?;
    if frontmatter.name.is_empty() {
        frontmatter.name = directory_name.to_owned();
    }
    validate_skill_name(&frontmatter.name).map_err(|detail| SkillPackageError::InvalidName {
        directory: directory_name.to_owned(),
        name: frontmatter.name.clone(),
        detail,
    })?;
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

    // Dependency metadata carries JSON, whose formatting/key order is not
    // definition semantics. Reuse the parsed normalized manifest, not a second parser.
    for (key, pins) in [
        (
            crate::skills::dependencies::PYTHON_DEPENDENCIES_METADATA_KEY,
            &dependencies.python,
        ),
        (
            crate::skills::dependencies::NODE_DEPENDENCIES_METADATA_KEY,
            &dependencies.node,
        ),
    ] {
        if frontmatter.metadata.contains_key(key) {
            frontmatter.metadata.insert(
                key.to_owned(),
                serde_json::to_string(pins).expect("normalized dependency pins serialize"),
            );
        }
    }
    frontmatter.description = description.to_owned();
    frontmatter.compatibility = frontmatter
        .compatibility
        .map(|value| value.trim().to_owned());
    let version_id = definition_version_id(&frontmatter);
    Ok(SkillPackage {
        id: SkillId::new(frontmatter.name.clone()),
        version_id,
        definition: frontmatter,
        dependencies,
        location,
        source,
    })
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
    #[serde(default)]
    name: serde_yaml::Value,
    #[serde(default)]
    description: serde_yaml::Value,
    #[serde(default)]
    metadata: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: serde_yaml::Value,
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
        name: frontmatter.name.as_str().unwrap_or_default().to_owned(),
        description: frontmatter
            .description
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        metadata,
        license: frontmatter.license,
        compatibility: frontmatter.compatibility,
        allowed_tools: frontmatter.allowed_tools,
        disable_model_invocation: frontmatter.disable_model_invocation
            == serde_yaml::Value::Bool(true),
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
    use crate::skills::source::automatic_skill_roots;
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
        std::fs::write(workspace_root.join(".git"), "").unwrap();
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

    #[test]
    fn pi_collections_discover_ancestor_packages_and_standalone_markdown() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        std::fs::write(workspace.root().join(".git"), "gitdir: elsewhere").unwrap();
        let project = workspace.root().join("src/project");
        std::fs::create_dir_all(&project).unwrap();
        let pi = project.join(".pi/skills");
        let shared = project.join(".agents/skills");
        write_skill(&home.join(".agents/skills"), "global", "Global", "");
        write_skill(&home.join(".pi/agent/skills"), "duplicate", "User Pi", "");
        write_skill(&shared.join("team"), "duplicate", "Shared project", "");
        write_skill(
            &shared.join("team/duplicate"),
            "asset",
            "Never scan inside a package",
            "",
        );
        write_skill(
            &workspace.root().join(".agents/skills"),
            "ancestor",
            "Repo root",
            "",
        );
        write_skill(
            &base.join(".agents/skills"),
            "outside",
            "Outside Git repo",
            "",
        );
        write_skill(&home.join("rustx/.agents/skills"), "old", "Old root", "");
        write_skill(&shared, "ignored", "Ignored", "");
        std::fs::write(shared.join(".fdignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(&pi).unwrap();
        std::fs::write(
            pi.join("review.md"),
            "---\nname: duplicate\ndescription: Pi project\n---\nPRIVATE_BODY",
        )
        .unwrap();
        std::fs::write(
            shared.join("ignored-root.md"),
            "---\nname: root-note\ndescription: Ignore loose root files\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(shared.join("loose")).unwrap();
        std::fs::write(
            shared.join("loose/guide.md"),
            "---\ndescription: Fallback name\n---\n",
        )
        .unwrap();
        let project = Workspace::new(project).unwrap();
        let outcome = SkillDiscovery::with_config(
            &project,
            SkillDiscoveryConfig {
                automatic: automatic_skill_roots(Some(&home), project.root()),
            },
        )
        .discover();
        assert_eq!(
            names(&outcome.packages),
            ["ancestor", "duplicate", "global", "loose"]
        );
        assert_eq!(outcome.packages[1].description(), "Pi project");
        assert!(
            outcome.packages[1]
                .location()
                .ends_with(".pi/skills/review.md")
        );
        assert!(
            outcome.packages[3]
                .location()
                .ends_with(".agents/skills/loose/guide.md")
        );
    }

    /// #280 (5): a missing automatic root is a benign empty set.
    #[test]
    fn cfg280_missing_automatic_roots_are_benign_empty_sets() {
        let (_temp, base, workspace) = workspace_fixture();
        let home = base.join("home");
        let outcome = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic: automatic_skill_roots(Some(&home), workspace.root()),
            },
        )
        .discover();
        assert!(outcome.packages.is_empty());
        assert_eq!(outcome.diagnostics.len(), 4);
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
                .discover();
        assert_eq!(names(&outcome.packages), ["debugging", "rust-review"]);
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [SkillDiagnostic::PackageInvalid { package, .. }] if package.ends_with("broken/SKILL.md")
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
        let global = (SkillSource::User, home.join(".agents/skills"));
        let project = (
            SkillSource::Workspace,
            workspace.root().join(".agents/skills"),
        );
        let forward =
            SkillDiscovery::with_config(&workspace, automatic(&[global.clone(), project.clone()]))
                .discover();
        let reversed =
            SkillDiscovery::with_config(&workspace, automatic(&[project, global])).discover();
        assert_eq!(forward, reversed);
        assert_eq!(names(&forward.packages), ["foo"]);
        assert_eq!(forward.packages[0].description(), "Workspace foo");
        assert_eq!(forward.packages[0].source(), SkillSource::Workspace);
        let provenance = &forward.provenance[0];
        assert_eq!(provenance.source, SkillSource::Workspace);
        assert_eq!(provenance.shadowed.len(), 1);
        assert_eq!(provenance.shadowed[0].source, SkillSource::User);
        assert!(
            provenance.shadowed[0]
                .location
                .starts_with(home.join(".agents/skills").to_str().expect("utf-8 home"))
        );
        assert!(matches!(
            forward.diagnostics.as_slice(),
            [SkillDiagnostic::Shadowed {
                effective_source: SkillSource::Workspace,
                shadowed_source: SkillSource::User,
                ..
            }]
        ));
    }

    #[test]
    fn malformed_project_skill_does_not_shadow_user_skill() {
        let (_temp, base, workspace) = workspace_fixture();
        let global = base.join("home/.agents/skills");
        let project = workspace.root().join(".agents/skills");
        write_skill(&global, "foo", "Global foo", "");
        write_skill(&project, "foo", "Workspace foo", "");
        std::fs::write(project.join("foo/SKILL.md"), "malformed").unwrap();
        let outcome = SkillDiscovery::with_config(
            &workspace,
            automatic(&[
                (SkillSource::User, global),
                (SkillSource::Workspace, project),
            ]),
        )
        .discover();
        assert_eq!(names(&outcome.packages), ["foo"]);
        assert_eq!(outcome.packages[0].source(), SkillSource::User);
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [SkillDiagnostic::PackageInvalid { .. }]
        ));
    }

    #[test]
    fn discovery_follows_contained_symlinks_and_dedupes_aliases() {
        let (_temp, base, workspace) = workspace_fixture();
        let root = workspace.root().join(".agents/skills");
        write_skill(&root, "valid", "Valid", "");
        write_skill(&base, "external", "Outside Workspace", "");
        std::os::unix::fs::symlink(root.join("valid"), root.join("alias")).unwrap();
        std::os::unix::fs::symlink(base.join("external"), root.join("external")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("cycle")).unwrap();
        let outcome =
            SkillDiscovery::with_config(&workspace, SkillDiscoveryConfig::workspace_root(root))
                .discover();
        assert_eq!(names(&outcome.packages), ["valid"]);
        assert!(outcome.diagnostics.is_empty());
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
                (SkillSource::User, home.join(".agents/skills")),
                (
                    SkillSource::Workspace,
                    workspace.root().join(".agents/skills"),
                ),
            ]),
        )
        .discover();
        assert_eq!(names(&outcome.packages), ["zeta"]);
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [SkillDiagnostic::SourceRootInvalid {
                source: SkillSource::User,
                ..
            }]
        ));
    }

    #[test]
    fn discovery_merges_bounded_roots_in_deterministic_identity_order() {
        let (_temp, base, workspace) = workspace_fixture();
        let user_agents = base.join("user/.agents/skills");
        let project_agents = workspace.root().join(".agents/skills");

        write_skill(&project_agents, "zeta", "Zeta", "");
        write_skill(&user_agents, "alpha", "Alpha", "");
        write_skill(&user_agents, "middle", "Middle", "");

        let outcome = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic: vec![
                    AutomaticSkillRoot {
                        source: SkillSource::Workspace,
                        root: project_agents,
                    },
                    AutomaticSkillRoot {
                        source: SkillSource::User,
                        root: user_agents,
                    },
                ],
            },
        )
        .discover();
        assert_eq!(names(&outcome.packages), vec!["alpha", "middle", "zeta"]);
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
                automatic: automatic_skill_roots(Some(&home), workspace.root()),
            },
        )
        .discover();
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
                (SkillSource::User, home.join(".agents/skills")),
                (
                    SkillSource::Workspace,
                    workspace.root().join(".agents/skills"),
                ),
            ]),
        )
        .discover();
        let aliased = SkillDiscovery::with_config(
            &workspace,
            automatic(&[
                (SkillSource::User, alias.join(".agents/skills")),
                (
                    SkillSource::Workspace,
                    workspace.root().join(".agents/skills"),
                ),
            ]),
        )
        .discover();
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
                (SkillSource::User, global.clone()),
                (SkillSource::Workspace, project.clone()),
            ])
        };

        // Exactly at the limit: the source publishes completely.
        let exact = SkillDiscovery::with_source_budget(config(), 3).discover();
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
        let over = SkillDiscovery::with_source_budget(config(), 2).discover();
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
        assert_eq!(*source, SkillSource::User);
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
                (SkillSource::User, global.clone()),
                (SkillSource::User, split.clone()),
            ];
            if reverse {
                roots.reverse();
            }
            automatic(&roots)
        };
        for reverse in [false, true] {
            let outcome = SkillDiscovery::with_source_budget(two_roots(reverse), 4).discover();
            assert_eq!(
                names(&outcome.packages),
                ["g-four", "g-one", "g-three", "g-two"]
            );
            let over = SkillDiscovery::with_source_budget(two_roots(reverse), 3).discover();
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
            let mut expected = vec![global.display().to_string(), split.display().to_string()];
            expected.sort();
            assert_eq!(roots, &expected);
            assert_eq!(*candidates, 4);
        }
    }

    #[test]
    fn hidden_skill_keeps_package_resources_for_progressive_disclosure() {
        let (_temp, _base, workspace) = workspace_fixture();
        let explicit = workspace.root().join(".agents/skills");
        write_skill(
            &explicit,
            "private-guide",
            "Private guide",
            "\ndisable-model-invocation: true",
        );
        let outcome =
            SkillDiscovery::with_config(&workspace, SkillDiscoveryConfig::workspace_root(explicit))
                .discover();
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
