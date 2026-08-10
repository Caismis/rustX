//! Skill package discovery, parsing, and validation (M6).
//!
//! # Skill root contract
//!
//! There is exactly one Skill root, anchored to the canonical Workspace
//! root and never to Bash's mutable working directory:
//!
//! ```text
//! <workspace>/.agents/skills/<skill-name>/SKILL.md
//! ```
//!
//! Discovery is one level only: direct child directories of the Skill
//! root, each containing a `SKILL.md`. Nested Skill packages are never
//! discovered recursively.
//!
//! # Discovery semantics
//!
//! - a missing `.agents/skills/` directory means an empty Skill set, not an
//!   error;
//! - hidden direct entries (names beginning with `.`) are ignored;
//! - ordinary unrelated files directly under `.agents/skills/` are
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
//! `description`; host absolute paths never appear in model-visible Skill
//! metadata.
//!
//! # Workspace-file limitation
//!
//! Skill packages are ordinary Workspace files. M6 freezes discovered
//! identities, versions, catalog metadata, and dependency declarations at
//! preparation time, but it does not snapshot-mount the package content: an
//! external rewrite of `.agents/skills/...` after preparation is observed
//! only at the next quiescent re-discovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::runtime::identity::{SkillId, SkillVersionId};
use crate::skills::dependencies::{DependencyManifest, parse_dependency_map};
use crate::skills::identity::package_version_id;
use crate::tools::workspace::Workspace;

/// The canonical Skill root directory name below the Workspace root.
pub const SKILLS_DIRECTORY: &str = ".agents";
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
    /// A filesystem failure while reading the package.
    Io { path: String, detail: String },
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
            Self::UnsupportedSymlink { path } => {
                write!(f, "skill package symlinks are rejected for M6: {path:?}")
            }
            Self::Io { path, detail } => write!(f, "cannot read {path:?}: {detail}"),
        }
    }
}

impl std::error::Error for SkillPackageError {}

/// One discovered and validated Skill package.
///
/// The package is immutable after discovery: its `SkillVersionId` is
/// derived from the complete accepted package content, and its dependency
/// declarations are already parsed and normalized. The model-visible
/// catalog uses only `name` and `description`.
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
}

/// Discovers Skill packages under the canonical project-local Skill root.
#[derive(Debug, Clone)]
pub struct SkillDiscovery {
    workspace: Workspace,
}

impl SkillDiscovery {
    /// A discovery instance anchored to the canonical Workspace root.
    #[must_use]
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            workspace: workspace.clone(),
        }
    }

    /// Discovers every valid Skill package.
    ///
    /// Results are deterministically sorted by validated Skill name. Any
    /// malformed candidate fails the whole transaction; a missing Skill
    /// root yields an empty set.
    ///
    /// # Errors
    ///
    /// Returns [`SkillPackageError`] for a malformed Skill root or any
    /// malformed candidate package.
    pub fn discover(&self) -> Result<Vec<SkillPackage>, SkillPackageError> {
        let skills_root = self
            .workspace
            .root()
            .join(SKILLS_DIRECTORY)
            .join(SKILLS_ROOT);
        if !skills_root.exists() {
            return Ok(Vec::new());
        }
        if !skills_root.is_dir() {
            return Err(SkillPackageError::SkillRootNotDirectory(skills_root));
        }
        let mut candidates: Vec<(String, PathBuf)> = Vec::new();
        let entries = std::fs::read_dir(&skills_root).map_err(|error| SkillPackageError::Io {
            path: skills_root.display().to_string(),
            detail: error.to_string(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| SkillPackageError::Io {
                path: skills_root.display().to_string(),
                detail: error.to_string(),
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let file_type = entry.file_type().map_err(|error| SkillPackageError::Io {
                path: name.clone(),
                detail: error.to_string(),
            })?;
            if file_type.is_symlink() {
                // A symlinked package root is rejected for M6: the package
                // root must be a real directory inside the Skill root.
                return Err(SkillPackageError::UnsupportedSymlink {
                    path: entry.path().display().to_string(),
                });
            }
            if file_type.is_dir() {
                candidates.push((name, entry.path()));
            }
        }
        // Deterministic ordering by validated Skill name, independent of
        // filesystem enumeration order.
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        candidates
            .into_iter()
            .map(|(name, root)| discover_package(&root, &name))
            .collect()
    }
}

/// Parses, validates, and hashes one Skill package directory.
fn discover_package(root: &Path, directory_name: &str) -> Result<SkillPackage, SkillPackageError> {
    validate_skill_name(directory_name).map_err(|detail| SkillPackageError::InvalidName {
        directory: directory_name.to_owned(),
        name: directory_name.to_owned(),
        detail,
    })?;
    let skill_markdown = root.join(SKILL_MARKDOWN_FILE);
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
}

/// The frontmatter parse outcome distinguishes a malformed YAML block from
/// a metadata map that violates the standard string-to-string constraint.
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
    let body = markdown.strip_prefix("---\n").or_else(|| {
        markdown
            .strip_prefix("---\r\n")
            .or_else(|| markdown.strip_prefix("---\u{FEFF}"))
    });
    let Some(remainder) = body else {
        return Err(FrontmatterFailure::Malformed(
            "SKILL.md must start with a YAML frontmatter block".to_owned(),
        ));
    };
    let (yaml_block, _markdown_body) = remainder.split_once("\n---").ok_or_else(|| {
        FrontmatterFailure::Malformed(
            "SKILL.md frontmatter is missing its closing delimiter".to_owned(),
        )
    })?;
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
    })
}
