//! Typed generation-scoped Skill discovery diagnostics (Issue #280).
//!
//! A Skill diagnostic is a **fact about one candidate generation**, not a log
//! line. It is computed once, during candidate resource-generation
//! construction, canonically ordered, and frozen into the generation's
//! [`SkillSnapshot`](crate::skills::SkillSnapshot). It is never re-derived or
//! re-emitted per model turn, and it never reaches the model-facing Skill
//! catalog.
//!
//! The type system distinguishes the causes #280 requires downstream
//! consumers to tell apart without string parsing:
//!
//! ```text
//! SourceRootMissing      an automatic root simply does not exist (benign)
//! SourceRootInvalid      an automatic root exists but cannot be scanned
//! PackageInvalid         one candidate failed Agent Skills validation
//! PackageEscapesSource   one candidate resolved outside its own source
//! DuplicateIdentity      one scope defines the same identity twice
//! Shadowed               a higher-precedence source won the same identity
//! ```
//!
//! Severity is deliberately separate from cause: a missing `~/.agents/skills`
//! and an intentional workspace-over-global shadow are ordinary
//! [`SkillDiagnosticSeverity::Fact`]s, while an excluded package is a
//! [`SkillDiagnosticSeverity::Warning`]. Nothing here is a launch failure.

use crate::skills::package::SkillPackageError;
use crate::skills::source::SkillSource;

/// Whether one diagnostic reports an ordinary fact or an exclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillDiagnosticSeverity {
    /// Nothing was excluded; the generation simply records what it observed.
    Fact,
    /// A candidate was excluded from the effective catalog.
    Warning,
}

/// One typed generation-scoped Skill discovery fact.
///
/// The variant declaration order, followed by the field order, **is** the
/// canonical diagnostic order: the derived [`Ord`] is the only sort key, so
/// no consumer can observe a filesystem-enumeration-dependent ordering.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillDiagnostic {
    /// A configured automatic source root does not exist. This is the normal
    /// state of a machine without global Skills and is never a failure.
    SourceRootMissing {
        /// The source whose root is absent.
        source: SkillSource,
        /// The resolved root path.
        root: String,
    },
    /// A configured automatic source root exists but cannot be used as a
    /// Skill collection directory. The source contributes no packages; every
    /// other source is unaffected.
    SourceRootInvalid {
        /// The source whose root is unusable.
        source: SkillSource,
        /// The resolved root path.
        root: String,
        /// The structural reason, preserved verbatim.
        detail: String,
    },
    /// One candidate package failed Agent Skills validation and is excluded.
    /// Unrelated valid packages in the same source still publish.
    PackageInvalid {
        /// The source the candidate was found under.
        source: SkillSource,
        /// The candidate package directory.
        package: String,
        /// The preserved typed validation cause.
        cause: SkillPackageError,
    },
    /// One candidate canonicalized outside the source root that offered it.
    /// Sources are bounded authorities, so the candidate is excluded rather
    /// than silently admitted under the wrong authority.
    PackageEscapesSource {
        /// The source that offered the candidate.
        source: SkillSource,
        /// The candidate package directory as offered.
        package: String,
        /// The canonical source root the candidate left.
        root: String,
    },
    /// Two or more distinct packages in the **same** scope resolve to one
    /// logical Skill identity. Every conflicting definition is excluded;
    /// discovery never picks a winner by enumeration order.
    DuplicateIdentity {
        /// The scope that defines the identity more than once.
        source: SkillSource,
        /// The contested logical Skill identity.
        name: String,
        /// Every conflicting package root, in canonical order.
        packages: Vec<String>,
    },
    /// A valid package lost the same logical identity to a valid package
    /// from a higher-precedence source. This is intentional shadowing, not
    /// an error.
    Shadowed {
        /// The contested logical Skill identity.
        name: String,
        /// The source that owns the effective package.
        effective_source: SkillSource,
        /// The effective package's `SKILL.md` location.
        effective_location: String,
        /// The source whose package was shadowed.
        shadowed_source: SkillSource,
        /// The shadowed package's `SKILL.md` location.
        shadowed_location: String,
    },
}

impl SkillDiagnostic {
    /// Whether this diagnostic excluded a candidate from the catalog.
    #[must_use]
    pub const fn severity(&self) -> SkillDiagnosticSeverity {
        match self {
            Self::SourceRootMissing { .. } | Self::Shadowed { .. } => SkillDiagnosticSeverity::Fact,
            Self::SourceRootInvalid { .. }
            | Self::PackageInvalid { .. }
            | Self::PackageEscapesSource { .. }
            | Self::DuplicateIdentity { .. } => SkillDiagnosticSeverity::Warning,
        }
    }

    /// The source this diagnostic belongs to, when it belongs to exactly one.
    #[must_use]
    pub const fn source(&self) -> Option<SkillSource> {
        match self {
            Self::SourceRootMissing { source, .. }
            | Self::SourceRootInvalid { source, .. }
            | Self::PackageInvalid { source, .. }
            | Self::PackageEscapesSource { source, .. }
            | Self::DuplicateIdentity { source, .. } => Some(*source),
            Self::Shadowed { .. } => None,
        }
    }
}

impl core::fmt::Display for SkillDiagnostic {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SourceRootMissing { source, root } => write!(
                formatter,
                "the {source} Skill source root {root:?} does not exist; it contributes no Skills"
            ),
            Self::SourceRootInvalid {
                source,
                root,
                detail,
            } => write!(
                formatter,
                "the {source} Skill source root {root:?} cannot be scanned: {detail}"
            ),
            Self::PackageInvalid {
                source,
                package,
                cause,
            } => write!(
                formatter,
                "the {source} Skill package {package:?} is excluded: {cause}"
            ),
            Self::PackageEscapesSource {
                source,
                package,
                root,
            } => write!(
                formatter,
                "the {source} Skill package {package:?} resolves outside its source root \
                 {root:?} and is excluded"
            ),
            Self::DuplicateIdentity {
                source,
                name,
                packages,
            } => write!(
                formatter,
                "the {source} Skill scope defines {name:?} more than once ({}); every \
                 conflicting definition is excluded",
                packages.join(", ")
            ),
            Self::Shadowed {
                name,
                effective_source,
                effective_location,
                shadowed_source,
                shadowed_location,
            } => write!(
                formatter,
                "Skill {name:?} resolves to the {effective_source} package \
                 {effective_location:?}; the {shadowed_source} package {shadowed_location:?} is \
                 shadowed"
            ),
        }
    }
}

/// The provenance record of one effective Skill identity.
///
/// This is generation/inspection metadata. It deliberately never enters the
/// model-facing catalog: a Skill's instructions are not improved by knowing
/// which root won it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct SkillProvenance {
    /// The effective logical Skill identity.
    pub name: String,
    /// The source that owns the effective package.
    pub source: SkillSource,
    /// The effective package's `SKILL.md` location.
    pub location: String,
    /// Valid same-identity packages that lost the merge, in canonical order.
    pub shadowed: Vec<ShadowedSkill>,
}

/// One valid package that a higher-precedence source shadowed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct ShadowedSkill {
    /// The source whose package was shadowed.
    pub source: SkillSource,
    /// The shadowed package's `SKILL.md` location.
    pub location: String,
}
