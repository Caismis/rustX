//! Fixed User and Workspace Skill roots and whole-package precedence.

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

    /// Whether containment is checked against the Workspace resource root.
    #[must_use]
    pub const fn is_workspace_owned(self) -> bool {
        matches!(self, Self::Workspace)
    }
}

impl core::fmt::Display for SkillSource {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One resolved automatic Skill source root.
///
/// The root is a *physical* location chosen by the launch/environment owner.
/// Discovery treats it as a bounded authority: a package found under it may
/// never escape it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct AutomaticSkillRoot {
    /// The source authority this root belongs to.
    pub source: SkillSource,
    /// The physical collection directory. A missing directory is an empty
    /// set, never a launch failure.
    pub root: PathBuf,
}

/// Resolve the two ordinary Skill collection roots from process bindings.
#[must_use]
pub fn automatic_skill_roots(home: Option<&Path>, workspace: &Path) -> Vec<AutomaticSkillRoot> {
    let mut roots = Vec::new();
    if let Some(home) = home {
        roots.push(AutomaticSkillRoot {
            source: SkillSource::User,
            root: home.join("rustx/.agents/skills"),
        });
    }
    roots.push(AutomaticSkillRoot {
        source: SkillSource::Workspace,
        root: workspace.join(".agents/skills"),
    });
    roots
}
