//! Child-side materialization of the exact Skill packages a parent
//! generation froze (Issue #145).
//!
//! # Why a copy, and why not discovery
//!
//! #144 froze each selected Skill's immutable identity (`SkillId` +
//! `SkillVersionId`) plus its model-visible catalog metadata — including a
//! host `SKILL.md` path. That path is a **locator**, never an identity: the
//! bytes behind it can change without the path changing, and Issue #146 will
//! move the child into a different worktree entirely, where the parent's
//! path may not describe the same tree at all.
//!
//! So the child does the smallest thing that makes the frozen identity
//! authoritative again:
//!
//! ```text
//! frozen (source_root, files, version_id)
//!        |
//!        v  copy exactly `files`, nothing else
//! <child runtime root>/skills/<skill id>/
//!        |
//!        v  recompute package_version_id over the copy
//!   == frozen version_id ?  ->  remap catalog location onto the copy
//!                          ->  otherwise fail child preparation
//! ```
//!
//! Discovery is deliberately **not** restarted: no root is walked, no
//! candidate is admitted, and a Skill that appeared in the workspace after
//! the parent froze its selection cannot enter the child. The only inputs
//! are the frozen file list and the frozen digest.
//!
//! # Progressive disclosure is preserved
//!
//! Only the *bytes on disk* are materialized. No `SKILL.md` body and no
//! supporting file is loaded into the child's prompt: the model still sees
//! catalog metadata and still reaches a body through ordinary native Read,
//! now against a path inside the child's own runtime root.

use std::path::{Component, Path, PathBuf};

use crate::protocol::manifest::SkillBinding;

/// A Skill materialization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillMaterializationError {
    /// A frozen package-relative path is not a plain relative path.
    UnsafePath {
        /// The offending package-relative path.
        path: String,
    },
    /// A frozen file could not be copied out of the source root.
    Io {
        /// The path that failed.
        path: String,
        /// The failure detail.
        detail: String,
    },
    /// The materialized copy does not hash back to the frozen identity.
    IdentityMismatch {
        /// The Skill whose identity failed.
        skill: String,
        /// The frozen identity.
        expected: String,
        /// The identity of the materialized bytes.
        observed: String,
    },
    /// The materialized `SKILL.md` path is not representable as UTF-8.
    UnrepresentablePath {
        /// The Skill whose path failed.
        skill: String,
    },
}

impl core::fmt::Display for SkillMaterializationError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsafePath { path } => write!(
                formatter,
                "the frozen Skill file path {path:?} is not a plain package-relative path"
            ),
            Self::Io { path, detail } => {
                write!(formatter, "cannot materialize Skill file {path}: {detail}")
            }
            Self::IdentityMismatch {
                skill,
                expected,
                observed,
            } => write!(
                formatter,
                "the Skill {skill:?} materialized as {observed} but the invoking generation \
                 authorized {expected}: its source bytes changed after the parent froze them"
            ),
            Self::UnrepresentablePath { skill } => write!(
                formatter,
                "the materialized location of Skill {skill:?} is not valid UTF-8"
            ),
        }
    }
}

impl std::error::Error for SkillMaterializationError {}

/// Materializes one frozen Skill package into the child-private root and
/// proves the copy is exactly the version the parent authorized.
///
/// Returns the materialized `SKILL.md` location, which the caller uses to
/// remap the frozen catalog entry.
///
/// # Errors
///
/// Returns [`SkillMaterializationError`] for an unsafe frozen path, a copy
/// failure, or a digest that does not match the frozen `SkillVersionId`.
pub fn materialize_skill(
    binding: &SkillBinding,
    source_root: &Path,
    files: &[PathBuf],
    destination_root: &Path,
) -> Result<String, SkillMaterializationError> {
    std::fs::create_dir_all(destination_root).map_err(|error| SkillMaterializationError::Io {
        path: destination_root.display().to_string(),
        detail: error.to_string(),
    })?;
    for relative in files {
        // The frozen list comes from a package walk, but it crosses a
        // process boundary, so it is re-validated here rather than trusted:
        // only plain relative components may be written under the child
        // root.
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(SkillMaterializationError::UnsafePath {
                path: relative.display().to_string(),
            });
        }
        let from = source_root.join(relative);
        let to = destination_root.join(relative);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|error| SkillMaterializationError::Io {
                path: parent.display().to_string(),
                detail: error.to_string(),
            })?;
        }
        // A symlink in the source would copy its target's bytes and break
        // the digest contract, so the source metadata is checked first.
        let metadata =
            std::fs::symlink_metadata(&from).map_err(|error| SkillMaterializationError::Io {
                path: from.display().to_string(),
                detail: error.to_string(),
            })?;
        if !metadata.is_file() {
            return Err(SkillMaterializationError::Io {
                path: from.display().to_string(),
                detail: "the frozen Skill file is not a regular file".to_owned(),
            });
        }
        std::fs::copy(&from, &to).map_err(|error| SkillMaterializationError::Io {
            path: from.display().to_string(),
            detail: error.to_string(),
        })?;
    }
    let markdown = destination_root.join(crate::skills::package::SKILL_MARKDOWN_FILE);
    let markdown_bytes =
        std::fs::read(&markdown).map_err(|error| SkillMaterializationError::Io {
            path: markdown.display().to_string(),
            detail: error.to_string(),
        })?;
    // The one authority: the materialized bytes must hash back to the
    // frozen version identity, computed by exactly the same function that
    // produced it during parent discovery.
    let observed =
        crate::skills::identity::package_version_id(destination_root, files, &markdown_bytes)
            .map_err(|detail| SkillMaterializationError::Io {
                path: destination_root.display().to_string(),
                detail,
            })?;
    if observed != binding.version_id {
        return Err(SkillMaterializationError::IdentityMismatch {
            skill: binding.skill_id.as_str().to_owned(),
            expected: binding.version_id.as_str().to_owned(),
            observed: observed.as_str().to_owned(),
        });
    }
    markdown.to_str().map(str::to_owned).ok_or_else(|| {
        SkillMaterializationError::UnrepresentablePath {
            skill: binding.skill_id.as_str().to_owned(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{SkillMaterializationError, materialize_skill};
    use crate::protocol::manifest::SkillBinding;
    use crate::runtime::identity::{SkillId, SkillVersionId};
    use std::path::PathBuf;

    fn source(dir: &std::path::Path) -> (PathBuf, Vec<PathBuf>, SkillVersionId) {
        let root = dir.join("source/pdf");
        std::fs::create_dir_all(root.join("reference")).expect("dirs");
        std::fs::write(
            root.join("SKILL.md"),
            "---\nname: pdf\ndescription: A pdf skill\n---\nbody\n",
        )
        .expect("skill");
        std::fs::write(root.join("reference/guide.md"), "detail\n").expect("guide");
        let files = vec![
            PathBuf::from("SKILL.md"),
            PathBuf::from("reference/guide.md"),
        ];
        let markdown = std::fs::read(root.join("SKILL.md")).expect("read");
        let version =
            crate::skills::identity::package_version_id(&root, &files, &markdown).expect("digest");
        (root, files, version)
    }

    fn binding(version: SkillVersionId) -> SkillBinding {
        SkillBinding {
            skill_id: SkillId::new("pdf"),
            version_id: version,
        }
    }

    /// The materialized copy carries every frozen byte and hashes back to
    /// the frozen identity.
    #[test]
    fn a_frozen_package_materializes_and_verifies() {
        let dir = tempfile::tempdir().expect("temp");
        let (root, files, version) = source(dir.path());
        let destination = dir.path().join("child/skills/pdf");
        let location = materialize_skill(&binding(version), &root, &files, &destination)
            .expect("materialization");
        assert_eq!(location, destination.join("SKILL.md").display().to_string());
        assert_eq!(
            std::fs::read_to_string(destination.join("reference/guide.md")).expect("guide"),
            "detail\n"
        );
    }

    /// Source bytes that changed after the parent froze them fail closed
    /// rather than executing a different Skill version.
    #[test]
    fn changed_source_bytes_fail_closed() {
        let dir = tempfile::tempdir().expect("temp");
        let (root, files, version) = source(dir.path());
        std::fs::write(root.join("reference/guide.md"), "tampered\n").expect("tamper");
        let destination = dir.path().join("child/skills/pdf");
        assert!(matches!(
            materialize_skill(&binding(version), &root, &files, &destination),
            Err(SkillMaterializationError::IdentityMismatch { .. })
        ));
    }

    /// A frozen path that escapes the package root is refused before any
    /// byte is written outside the child root.
    #[test]
    fn an_escaping_path_is_refused() {
        let dir = tempfile::tempdir().expect("temp");
        let (root, _, version) = source(dir.path());
        let destination = dir.path().join("child/skills/pdf");
        assert_eq!(
            materialize_skill(
                &binding(version),
                &root,
                &[PathBuf::from("../escape.md")],
                &destination
            ),
            Err(SkillMaterializationError::UnsafePath {
                path: "../escape.md".to_owned()
            })
        );
    }
}
