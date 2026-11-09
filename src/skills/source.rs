//! Fixed User and Workspace Skill collections.

use std::path::{Path, PathBuf};

/// The canonical Skill root directory name below a source's base directory.
pub const SKILLS_DIRECTORY: &str = ".agents";
/// The Skill package collection directory name.
pub const SKILLS_ROOT: &str = "skills";

/// One Skill source authority.
///
/// A valid Workspace definition takes precedence over a User definition.
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
    /// `<home>/rustx/.agents/skills`.
    User,
    /// `<workspace>/.agents/skills`.
    Workspace,
}

impl SkillSource {
    /// The stable lowercase identity used by diagnostics and documentation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }
}

impl core::fmt::Display for SkillSource {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One resolved automatic Skill source root.
///
/// The root is chosen by the launch/environment owner. Links offered there
/// keep this collection's scope even when their target lives elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct AutomaticSkillRoot {
    /// The source authority this root belongs to.
    pub source: SkillSource,
    /// The physical collection directory. A missing directory is an empty
    /// set, never a launch failure.
    pub root: PathBuf,
}

impl AutomaticSkillRoot {
    pub(crate) fn user(home: &Path) -> Self {
        Self {
            source: SkillSource::User,
            root: home.join("rustx").join(SKILLS_DIRECTORY).join(SKILLS_ROOT),
        }
    }

    pub(crate) fn workspace(workspace: &Path) -> Self {
        Self {
            source: SkillSource::Workspace,
            root: workspace.join(SKILLS_DIRECTORY).join(SKILLS_ROOT),
        }
    }
}

/// Resolve the rustX Skill collections for one Workspace.
#[must_use]
pub fn automatic_skill_roots(home: Option<&Path>, workspace: &Path) -> Vec<AutomaticSkillRoot> {
    let mut roots = vec![AutomaticSkillRoot::workspace(workspace)];
    roots.extend(home.map(AutomaticSkillRoot::user));
    roots
}
