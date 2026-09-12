//! Skill source identity, the session-level source policy, and the
//! automatic root resolution that policy selects (Issue #280).
//!
//! # Ownership
//!
//! A *source* answers exactly one question: **where may a Skill package be
//! discovered?** It never answers which Skills an Agent may see. Capability
//! selection consumes the merged effective catalog and is owned by
//! [`crate::runtime::agent_profile`].
//!
//! For v1 there are exactly two automatic sources:
//!
//! ```text
//! global    -> <home>/.agents/skills
//! workspace -> <workspace>/.agents/skills
//! ```
//!
//! The global root is resolved from the launch/environment owner's captured
//! home directory, never from a rustX configuration directory and never by
//! shell-style string expansion. There is no Admin/System/Plugin scope, no
//! nested-project scope, and no generic configurable root registry.
//!
//! [`SkillSource::Explicit`] is not an automatic source: it is the separate
//! launch authority behind `--skill <path>`. It is part of the same
//! precedence domain so that an explicitly launched package participates in
//! one merge, one validation contract, and one frozen generation rather
//! than a second Skill plane.
//!
//! # Precedence
//!
//! ```text
//! explicit > workspace > global
//! ```
//!
//! Precedence is an architectural rule carried by [`SkillSource`]'s own
//! ordering. It is deliberately **not** derived from `[skills].sources`
//! array order, from filesystem enumeration order, or from the order the
//! resolved roots happen to reach discovery.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The canonical Skill root directory name below a source's base directory.
pub const SKILLS_DIRECTORY: &str = ".agents";
/// The Skill package collection directory name.
pub const SKILLS_ROOT: &str = "skills";

/// One Skill source authority.
///
/// The declaration order **is** the precedence order: a later variant wins
/// the same logical Skill identity against an earlier one.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// `<home>/.agents/skills`.
    Global,
    /// `<workspace>/.agents/skills`.
    Workspace,
    /// An explicit `--skill <path>` launch authority.
    Explicit,
}

impl SkillSource {
    /// The stable lowercase identity used by diagnostics and documentation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Workspace => "workspace",
            Self::Explicit => "explicit",
        }
    }
}

impl core::fmt::Display for SkillSource {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The only source identities `[skills].sources` may name.
///
/// `explicit` is deliberately absent: explicit Skill paths are a launch
/// authority (`--skill`), not a scannable automatic root, so naming one here
/// would be a second way to spell the same thing.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticSkillSource {
    /// `<home>/.agents/skills`.
    Global,
    /// `<workspace>/.agents/skills`.
    Workspace,
}

impl AutomaticSkillSource {
    /// The corresponding discovery source identity.
    #[must_use]
    pub const fn source(self) -> SkillSource {
        match self {
            Self::Global => SkillSource::Global,
            Self::Workspace => SkillSource::Workspace,
        }
    }
}

/// One resolved automatic Skill source root.
///
/// The root is a *physical* location chosen by the launch/environment owner.
/// Discovery treats it as a bounded authority: a package found under it may
/// never escape it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AutomaticSkillRoot {
    /// The source authority this root belongs to.
    pub source: SkillSource,
    /// The physical collection directory. A missing directory is an empty
    /// set, never a launch failure.
    pub root: PathBuf,
}

/// The default source policy: both canonical automatic sources.
#[must_use]
pub fn default_automatic_sources() -> BTreeSet<AutomaticSkillSource> {
    [
        AutomaticSkillSource::Global,
        AutomaticSkillSource::Workspace,
    ]
    .into_iter()
    .collect()
}

/// Resolves the automatic roots selected by one source policy.
///
/// The returned order is canonical (by source), but discovery never derives
/// precedence from it.
#[must_use]
pub fn automatic_skill_roots(
    home: Option<&Path>,
    workspace: &Path,
    selected: &BTreeSet<AutomaticSkillSource>,
) -> Vec<AutomaticSkillRoot> {
    let mut roots = Vec::with_capacity(selected.len());
    for source in selected {
        let base = match source {
            // A launch without a captured home directory simply has no
            // global source; it is never a failure and never falls back to
            // a rustX configuration directory.
            AutomaticSkillSource::Global => match home {
                Some(home) => home,
                None => continue,
            },
            AutomaticSkillSource::Workspace => workspace,
        };
        roots.push(AutomaticSkillRoot {
            source: source.source(),
            root: base.join(SKILLS_DIRECTORY).join(SKILLS_ROOT),
        });
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::{AutomaticSkillSource, SkillSource, automatic_skill_roots};

    #[test]
    fn cfg280_precedence_is_carried_by_source_identity() {
        assert!(SkillSource::Global < SkillSource::Workspace);
        assert!(SkillSource::Workspace < SkillSource::Explicit);
    }

    #[test]
    fn cfg280_default_policy_resolves_exactly_the_two_canonical_roots() {
        let home = std::path::Path::new("/isolated/home");
        let workspace = std::path::Path::new("/isolated/workspace");
        let roots = automatic_skill_roots(
            Some(home),
            workspace,
            &[
                AutomaticSkillSource::Workspace,
                AutomaticSkillSource::Global,
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            roots
                .iter()
                .map(|root| (root.source, root.root.clone()))
                .collect::<Vec<_>>(),
            vec![
                (SkillSource::Global, home.join(".agents/skills")),
                (SkillSource::Workspace, workspace.join(".agents/skills")),
            ]
        );
    }

    #[test]
    fn cfg280_absent_home_drops_only_the_global_root() {
        let workspace = std::path::Path::new("/isolated/workspace");
        let roots = automatic_skill_roots(
            None,
            workspace,
            &[
                AutomaticSkillSource::Global,
                AutomaticSkillSource::Workspace,
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].source, SkillSource::Workspace);
    }
}
