//! Automatic Skill roots in pi discovery order.

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
    /// User-wide collections under the process-bound home.
    User,
    /// Project collections at the working directory and its ancestors.
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

impl AutomaticSkillRoot {
    pub(crate) fn workspace_boundary<'a>(&self, workspace: Option<&'a Path>) -> Option<&'a Path> {
        workspace
            .filter(|_| self.source.is_workspace_owned())
            .map(|workspace| {
                workspace
                    .ancestors()
                    .find(|ancestor| self.root == ancestor.join(".agents/skills"))
                    .unwrap_or(workspace)
            })
    }
}

/// Resolve the user-specific and shared Skill collections.
#[must_use]
pub fn user_skill_roots(home: &Path) -> Vec<AutomaticSkillRoot> {
    [".pi/agent/skills", ".agents/skills"]
        .into_iter()
        .map(|directory| AutomaticSkillRoot {
            source: SkillSource::User,
            root: home.join(directory),
        })
        .collect()
}

/// Project collections take precedence, in working-directory order.
/// The shared user directory is never classified as a project collection.
#[must_use]
pub fn automatic_skill_roots(home: Option<&Path>, workspace: &Path) -> Vec<AutomaticSkillRoot> {
    let mut roots = vec![AutomaticSkillRoot {
        source: SkillSource::Workspace,
        root: workspace.join(".pi/skills"),
    }];
    for directory in workspace.ancestors() {
        if home != Some(directory) {
            roots.push(AutomaticSkillRoot {
                source: SkillSource::Workspace,
                root: directory.join(".agents/skills"),
            });
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    if let Some(home) = home {
        roots.extend(user_skill_roots(home));
    }
    roots
}
