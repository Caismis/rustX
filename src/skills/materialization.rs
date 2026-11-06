//! Child-side materialization of the exact Skill packages a parent
//! generation froze (Issue #145).
//!
//! # Why a copy, and why not discovery
//!
//! #144 froze each selected Skill's immutable identity (`SkillId` +
//! `SkillVersionId`) plus its model-visible catalog metadata — including a
//! host `SKILL.md` path. That path is a **locator**, never an identity: the
//! bytes behind it can change without the path changing, and an isolated
//! Issue #146 child may run in a different worktree entirely, where the
//! parent's path may not describe the same tree at all.
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
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<String, SkillMaterializationError> {
    let io = |path: &Path, error: std::io::Error| SkillMaterializationError::Io {
        path: path.display().to_string(),
        detail: error.to_string(),
    };
    let parent = destination_root.parent().ok_or_else(|| {
        io(
            destination_root,
            std::io::Error::other("destination has no parent"),
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    let staging = tempfile::Builder::new()
        .prefix(".skill-")
        .tempdir_in(parent)
        .map_err(|e| io(parent, e))?;
    let content = staging.path().join("content");
    std::fs::create_dir(&content).map_err(|e| io(&content, e))?;
    let content = content.canonicalize().map_err(|e| io(&content, e))?;
    let result = (|| {
        let source = crate::package_io::PackageDirectory::open(source_root)
            .map_err(|e| io(source_root, e))?;
        for relative in files {
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(SkillMaterializationError::UnsafePath {
                    path: relative.display().to_string(),
                });
            }
            crate::package_io::copy(&source, relative, &content.join(relative), cancellation)
                .map_err(|e| io(&source_root.join(relative), e))?;
        }
        let (observed, _) = crate::skills::identity::inspect_package(&content, files, cancellation)
            .map_err(|detail| SkillMaterializationError::Io {
                path: staging.path().display().to_string(),
                detail,
            })?;
        if observed != binding.version_id {
            return Err(SkillMaterializationError::IdentityMismatch {
                skill: binding.skill_id.as_str().to_owned(),
                expected: binding.version_id.as_str().to_owned(),
                observed: observed.as_str().to_owned(),
            });
        }
        crate::package_io::check_cancelled(cancellation).map_err(|e| io(destination_root, e))?;
        std::fs::rename(&content, destination_root).map_err(|e| io(destination_root, e))?;
        Ok(())
    })();
    // Cleanup is part of settlement. A failure includes the retained location;
    // the child runtime root remains the lifecycle owner's cleanup responsibility.
    let cleanup = staging.close().map_err(|e| io(parent, e));
    match (result, cleanup) {
        (Err(cause), Err(cleanup)) => {
            return Err(SkillMaterializationError::Io {
                path: parent.display().to_string(),
                detail: format!(
                    "{cause}; staging cleanup also failed: {cleanup}; the child runtime owner retains cleanup responsibility"
                ),
            });
        }
        (Err(cause), Ok(())) => return Err(cause),
        (Ok(()), Err(cleanup)) => return Err(cleanup),
        (Ok(()), Ok(())) => {}
    }
    let markdown = destination_root.join(crate::skills::package::SKILL_MARKDOWN_FILE);
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
        let root = dir.canonicalize().unwrap().join("source/pdf");
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
        let version = crate::skills::identity::package_version_id(&root, &files).expect("digest");
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
        let location = materialize_skill(
            &binding(version),
            &root,
            &files,
            &destination,
            &crate::runtime::CancellationSignal::new(),
        )
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
            materialize_skill(
                &binding(version),
                &root,
                &files,
                &destination,
                &crate::runtime::CancellationSignal::new()
            ),
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
                &destination,
                &crate::runtime::CancellationSignal::new(),
            ),
            Err(SkillMaterializationError::UnsafePath {
                path: "../escape.md".to_owned()
            })
        );
    }
}
