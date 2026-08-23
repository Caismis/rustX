//! Skill package discovery, parsing, and validation (M6).
//!
//! # Skill root contract
//!
//! Current discovery is bounded to user/global and project roots, plus
//! explicit configuration and CLI paths. An accepted package is an ordinary
//! host directory: the model receives the host path of its `SKILL.md` and
//! reaches the package's own scripts, references, and assets by resolving the
//! relative spellings in `SKILL.md` against that directory. No virtual
//! namespace exists, so every native tool — Read, Bash, Grep, Glob — sees the
//! same paths.
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
//! or whose canonical path is not valid UTF-8, fails discovery explicitly
//! rather than being published in a lossy spelling.
//!
//! Discovery is one level only: direct child directories of the Skill
//! root, each containing a `SKILL.md`. Nested Skill packages are never
//! discovered recursively.
//!
//! # Discovery semantics
//!
//! - a missing automatic Skill root means an empty Skill set, not an error;
//! - hidden direct entries (names beginning with `.`) are ignored;
//! - ordinary unrelated files directly under an automatic Skill root are
//!   ignored;
//! - each non-hidden candidate directory must contain `SKILL.md`;
//! - malformed candidate packages fail the whole discovery transaction: one
//!   malformed Skill must never partially activate;
//! - symlinked Skill package roots and symlink entries inside a Skill
//!   package are rejected for M6 (this is Skill-package validation only;
//!   the general Workspace symlink contract for ordinary tools is
//!   unchanged);
//! - results are deterministically ordered by validated Skill name,
//!   independent of filesystem enumeration order.
//!
//! # Frontmatter contract
//!
//! `SKILL.md` is YAML frontmatter followed by Markdown instructions (the
//! Agent Skills standard format; no replacement format is invented). M6
//! validates the standard requirements:
//!
//! - `name`: 1-64 characters, lowercase letters, numbers, and hyphens
//!   only; must not start or end with a hyphen and must not contain
//!   consecutive hyphens; must match the parent directory name;
//! - `description`: non-empty, at most 1024 characters;
//! - `metadata`: a string-to-string map when present;
//! - `license`, `compatibility` (1-500 characters), and `allowed-tools`
//!   are parsed and preserved but M6 invents no runtime policy for them;
//! - the rustX dependency declaration keys are parsed from `metadata` (see
//!   [`crate::skills::dependencies`]).
//!
//! The model-visible catalog contains only the standard `name` and
//! `description` plus the host location of `SKILL.md`.
//!
//! # Resource boundary
//!
//! Skill packages remain current filesystem resources. M6 freezes discovered
//! identities, versions, catalog metadata, and dependency declarations at
//! preparation time. Package files themselves are read at use time through
//! ordinary tool semantics, and an external rewrite is observed only at the
//! next quiescent re-discovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::runtime::identity::{SkillId, SkillVersionId};
use crate::skills::dependencies::{DependencyManifest, parse_dependency_map};
use crate::skills::identity::package_version_id;
use crate::tools::workspace::Workspace;

/// The canonical Skill root directory name below the Workspace root.
pub const SKILLS_DIRECTORY: &str = ".agents";
/// The rustX project-local Skill root directory.
pub const RUSTX_SKILLS_DIRECTORY: &str = ".rustx";
pub const SKILLS_ROOT: &str = "skills";
/// The canonical primary instructions file name of a Skill package.
pub const SKILL_MARKDOWN_FILE: &str = "SKILL.md";

/// The maximum allowed length of a validated standard Skill name.
pub const MAX_SKILL_NAME_CHARS: usize = 64;
/// The maximum allowed length of a validated standard Skill description.
pub const MAX_SKILL_DESCRIPTION_CHARS: usize = 1024;
/// The maximum allowed length of the standard `compatibility` field.
pub const MAX_SKILL_COMPATIBILITY_CHARS: usize = 500;

/// A discovery/parsing/validation failure of a Skill package.
///
/// Every variant identifies the responsible Skill package; discovery fails
/// the whole transaction on any malformed candidate so one malformed Skill
/// can never partially activate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillPackageError {
    /// The Skill root exists but is not a directory.
    SkillRootNotDirectory(PathBuf),
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
    /// found. M6 rejects package-internal symlinks; this is Skill-package
    /// validation, not a change to normal Workspace semantics.
    UnsupportedSymlink { path: String },
    /// The canonical package root is not losslessly representable as UTF-8,
    /// so it cannot be published as a model-visible location.
    UnrepresentableRoot { path: String },
    /// A filesystem failure while reading the package.
    Io { path: String, detail: String },
    /// Two current roots expose the same logical Skill identity.
    DuplicateIdentity {
        /// The logical Skill name.
        name: String,
        /// The first deterministic package root.
        first: PathBuf,
        /// The conflicting package root.
        second: PathBuf,
    },
}

impl core::fmt::Display for SkillPackageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SkillRootNotDirectory(path) => {
                write!(f, "the skill root {} is not a directory", path.display())
            }
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
                write!(f, "skill package symlinks are rejected for M6: {path:?}")
            }
            Self::Io { path, detail } => write!(f, "cannot read {path:?}: {detail}"),
            Self::DuplicateIdentity {
                name,
                first,
                second,
            } => write!(
                f,
                "skill {name:?} is defined by both {} and {}",
                first.display(),
                second.display()
            ),
        }
    }
}

impl std::error::Error for SkillPackageError {}

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
}

impl SkillPackage {
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

    /// The canonical absolute host root of this package.
    #[must_use]
    pub(crate) fn root(&self) -> &Path {
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

/// Current Skill discovery roots and explicit package paths.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillDiscoveryConfig {
    /// Automatic/default collection roots. Missing roots are empty.
    pub automatic_roots: Vec<PathBuf>,
    /// Explicit collection roots, package directories, or `SKILL.md` paths.
    /// Missing explicit paths fail discovery.
    pub explicit_paths: Vec<PathBuf>,
}

impl SkillDiscoveryConfig {
    /// Returns the deterministic default user/global/project root order.
    #[must_use]
    pub fn default_for_workspace(workspace: &Workspace) -> Self {
        default_discovery_config(workspace)
    }
}

/// Discovers Skill packages across the current bounded root set.
#[derive(Debug, Clone)]
pub struct SkillDiscovery {
    config: SkillDiscoveryConfig,
}

impl SkillDiscovery {
    /// A discovery instance anchored to the canonical Workspace root.
    #[must_use]
    pub fn new(workspace: &Workspace) -> Self {
        Self::with_config(workspace, default_discovery_config(workspace))
    }

    /// Creates discovery with explicit current runtime roots.
    #[must_use]
    pub fn with_config(_workspace: &Workspace, config: SkillDiscoveryConfig) -> Self {
        Self { config }
    }

    /// Discovers every valid Skill package.
    ///
    /// Results are deterministically sorted by validated Skill name. Any
    /// malformed candidate fails the whole transaction; a missing Skill
    /// root yields an empty set.
    ///
    /// # Errors
    ///
    /// Returns [`SkillPackageError`] for a malformed Skill root, any
    /// malformed candidate package, or a candidate root that cannot be
    /// canonicalized or published as UTF-8.
    pub fn discover(&self) -> Result<Vec<SkillPackage>, SkillPackageError> {
        let mut candidates = Vec::<(String, PathBuf)>::new();
        for root in &self.config.automatic_roots {
            collect_root(root, false, &mut candidates)?;
        }
        for path in &self.config.explicit_paths {
            collect_root(path, true, &mut candidates)?;
        }
        candidates.sort();
        let mut packages = Vec::with_capacity(candidates.len());
        for (name, root) in candidates {
            // The single normalization point of the package root invariant:
            // every accepted package is canonical and absolute from here on,
            // whatever spelling the configured root or CLI path used.
            let root = canonical_package_root(&root)?;
            if let Some(previous) = packages
                .iter()
                .find(|package: &&SkillPackage| package.name() == name)
            {
                return Err(SkillPackageError::DuplicateIdentity {
                    name,
                    first: previous.root().to_path_buf(),
                    second: root,
                });
            }
            packages.push(discover_package(&root, &name)?);
        }
        packages.sort_by(|left, right| left.name().cmp(right.name()));
        Ok(packages)
    }
}

/// Resolves one candidate package root to its canonical absolute host path.
///
/// Discovery deliberately accepts relative paths, embedded `..`, and
/// ancestor symlinks as *input*; this is where all of them collapse to the
/// one address every downstream consumer sees.
fn canonical_package_root(root: &Path) -> Result<PathBuf, SkillPackageError> {
    std::fs::canonicalize(root).map_err(|error| SkillPackageError::Io {
        path: root.display().to_string(),
        detail: format!("cannot canonicalize the Skill package root: {error}"),
    })
}

fn default_discovery_config(workspace: &Workspace) -> SkillDiscoveryConfig {
    let mut automatic_roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        automatic_roots.push(home.join(RUSTX_SKILLS_DIRECTORY).join(SKILLS_ROOT));
        automatic_roots.push(home.join(SKILLS_DIRECTORY).join(SKILLS_ROOT));
    }
    automatic_roots.push(
        workspace
            .root()
            .join(RUSTX_SKILLS_DIRECTORY)
            .join(SKILLS_ROOT),
    );
    automatic_roots.push(workspace.root().join(SKILLS_DIRECTORY).join(SKILLS_ROOT));
    SkillDiscoveryConfig {
        automatic_roots,
        explicit_paths: Vec::new(),
    }
}

fn collect_root(
    path: &Path,
    explicit: bool,
    candidates: &mut Vec<(String, PathBuf)>,
) -> Result<(), SkillPackageError> {
    if !path.exists() {
        if explicit {
            return Err(SkillPackageError::Io {
                path: path.display().to_string(),
                detail: "explicit Skill path does not exist".to_owned(),
            });
        }
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| SkillPackageError::Io {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(SkillPackageError::UnsupportedSymlink {
            path: path.display().to_string(),
        });
    }
    if metadata.is_file() {
        if path.file_name().and_then(|name| name.to_str()) != Some(SKILL_MARKDOWN_FILE) {
            return Err(SkillPackageError::Io {
                path: path.display().to_string(),
                detail: "explicit Skill file must be named SKILL.md".to_owned(),
            });
        }
        let Some(root) = path.parent() else {
            return Err(SkillPackageError::Io {
                path: path.display().to_string(),
                detail: "explicit Skill file has no package directory".to_owned(),
            });
        };
        let Some(name) = root.file_name().and_then(|name| name.to_str()) else {
            return Err(SkillPackageError::Io {
                path: root.display().to_string(),
                detail: "explicit Skill package has no identity".to_owned(),
            });
        };
        candidates.push((name.to_owned(), root.to_path_buf()));
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(SkillPackageError::SkillRootNotDirectory(path.to_path_buf()));
    }
    if path.join(SKILL_MARKDOWN_FILE).is_file() {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Err(SkillPackageError::Io {
                path: path.display().to_string(),
                detail: "Skill package has no identity".to_owned(),
            });
        };
        candidates.push((name.to_owned(), path.to_path_buf()));
        return Ok(());
    }
    let entries = std::fs::read_dir(path).map_err(|error| SkillPackageError::Io {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    let mut children = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| SkillPackageError::Io {
            path: path.display().to_string(),
            detail: error.to_string(),
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| SkillPackageError::Io {
            path: entry.path().display().to_string(),
            detail: error.to_string(),
        })?;
        if file_type.is_symlink() {
            return Err(SkillPackageError::UnsupportedSymlink {
                path: entry.path().display().to_string(),
            });
        }
        if file_type.is_dir() {
            children.push((name, entry.path()));
        }
    }
    children.sort();
    candidates.extend(children);
    Ok(())
}

/// Parses, validates, and hashes one Skill package directory.
fn discover_package(root: &Path, directory_name: &str) -> Result<SkillPackage, SkillPackageError> {
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
    let markdown_bytes = std::fs::read(&skill_markdown).map_err(|error| SkillPackageError::Io {
        path: skill_markdown.display().to_string(),
        detail: error.to_string(),
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
    walk_package_files(root, root, &mut files)?;
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
) -> Result<(), SkillPackageError> {
    let entries = std::fs::read_dir(directory).map_err(|error| SkillPackageError::Io {
        path: directory.display().to_string(),
        detail: error.to_string(),
    })?;
    let mut paths = Vec::new();
    for entry in entries {
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
            walk_package_files(root, &path, files)?;
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
fn validate_skill_name(name: &str) -> Result<(), String> {
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
    use std::sync::Arc;

    use super::{SkillDiscovery, SkillDiscoveryConfig, parse_frontmatter};
    use crate::skills::SkillSnapshot;
    use crate::tools::Workspace;

    fn write_skill(root: &std::path::Path, name: &str, description: &str, extra: &str) {
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
    fn discovery_merges_bounded_roots_in_deterministic_identity_order() {
        let directory = tempfile::tempdir().expect("temporary root");
        let workspace_root = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        let project_rustx = workspace.root().join(".rustx/skills");
        let project_agents = workspace.root().join(".agents/skills");
        let explicit = directory.path().join("explicit/skills");
        write_skill(&project_agents, "zeta", "Zeta", "");
        write_skill(&project_rustx, "alpha", "Alpha", "");
        write_skill(&explicit, "middle", "Middle", "");

        let packages = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic_roots: vec![project_agents, project_rustx],
                explicit_paths: vec![explicit],
            },
        )
        .discover()
        .expect("roots discover");
        assert_eq!(
            packages
                .iter()
                .map(super::SkillPackage::name)
                .collect::<Vec<_>>(),
            vec!["alpha", "middle", "zeta"]
        );
        assert_eq!(
            packages[1].files(),
            &[
                std::path::PathBuf::from("SKILL.md"),
                std::path::PathBuf::from("references.md")
            ]
        );
    }

    #[test]
    fn no_automatic_roots_still_loads_explicit_skill_and_maps_resources() {
        let directory = tempfile::tempdir().expect("temporary root");
        let workspace_root = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        let explicit = directory.path().join("user/skills");
        write_skill(
            &explicit,
            "private-guide",
            "Private guide",
            "\ndisable-model-invocation: true",
        );
        let packages = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic_roots: Vec::new(),
                explicit_paths: vec![explicit],
            },
        )
        .discover()
        .expect("explicit Skill path");
        assert_eq!(packages.len(), 1);
        assert!(packages[0].disable_model_invocation());
        let snapshot = SkillSnapshot::new(packages.into_iter().map(Arc::new).collect());
        assert_eq!(snapshot.packages().len(), 1);
        assert!(snapshot.catalog_entries().is_empty());
        assert!(snapshot.visible_bindings().is_empty());
        assert_eq!(snapshot.bindings().len(), 1);
        // The package is hidden from the catalog but still tracked, so its
        // host location participates in snapshot equality.
        assert_eq!(snapshot.locations().len(), 1);
        assert!(snapshot.locations()[0].ends_with("private-guide/SKILL.md"));
    }

    #[test]
    fn duplicate_skill_identity_fails_independently_of_root_enumeration_order() {
        let directory = tempfile::tempdir().expect("temporary root");
        let workspace_root = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        write_skill(&first, "same", "First", "");
        write_skill(&second, "same", "Second", "");
        let error = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic_roots: vec![second, first],
                explicit_paths: Vec::new(),
            },
        )
        .discover()
        .expect_err("duplicate identity");
        assert!(error.to_string().contains("defined by both"));
    }
}
