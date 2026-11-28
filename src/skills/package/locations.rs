//! Translate catalog addresses while preserving the authoritative instruction reader.
use super::{SkillDiagnostic, SkillDiscoveryOutcome, SkillSource};
use std::io;
use std::path::{Path, PathBuf};

impl SkillDiscoveryOutcome {
    pub(super) fn publish_user_paths(
        &mut self,
        client_root: &Path,
        resolve: impl Fn(&Path) -> io::Result<PathBuf>,
    ) -> io::Result<()> {
        let Some(source_root) = self
            .roots
            .iter()
            .find(|root| root.source == SkillSource::User)
            .map(|root| root.root.clone())
        else {
            return Ok(());
        };
        let project = |location: &mut String| -> io::Result<()> {
            let address = Path::new(location);
            let relative = if address.is_absolute() {
                address
                    .strip_prefix(&source_root)
                    .map_err(io::Error::other)?
            } else {
                address
            };
            resolve(relative)?
                .to_str()
                .ok_or(io::ErrorKind::InvalidData)?
                .clone_into(location);
            Ok(())
        };
        for package in &mut self.packages {
            if package.source == SkillSource::User {
                project(&mut package.location)?;
            }
        }
        for provenance in self.provenance.iter_mut().chain(&mut self.invalid) {
            if provenance.source == SkillSource::User {
                project(&mut provenance.location)?;
            }
            for shadowed in &mut provenance.shadowed {
                if shadowed.source == SkillSource::User {
                    project(&mut shadowed.location)?;
                }
            }
        }
        for diagnostic in &mut self.diagnostics {
            match diagnostic {
                SkillDiagnostic::SourceRootMissing {
                    source: SkillSource::User,
                    root,
                }
                | SkillDiagnostic::SourceRootInvalid {
                    source: SkillSource::User,
                    root,
                    ..
                } => {
                    client_root
                        .to_str()
                        .ok_or(io::ErrorKind::InvalidData)?
                        .clone_into(root);
                }
                SkillDiagnostic::PackageInvalid {
                    source: SkillSource::User,
                    package,
                    ..
                }
                | SkillDiagnostic::PackageWarning {
                    source: SkillSource::User,
                    package,
                    ..
                } => project(package)?,
                SkillDiagnostic::DuplicateIdentity {
                    source: SkillSource::User,
                    packages,
                    ..
                } => {
                    for package in packages {
                        project(package)?;
                    }
                }
                SkillDiagnostic::Shadowed {
                    effective_source,
                    effective_location,
                    shadowed_source,
                    shadowed_location,
                    ..
                } => {
                    if *effective_source == SkillSource::User {
                        project(effective_location)?;
                    }
                    if *shadowed_source == SkillSource::User {
                        project(shadowed_location)?;
                    }
                }
                _ => {}
            }
        }
        for root in &mut self.roots {
            if root.source == SkillSource::User {
                client_root.clone_into(&mut root.root);
            }
        }
        self.diagnostics.sort();
        self.diagnostics.dedup();
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::{
        AutomaticSkillRoot, CollectionView, Native, collect_automatic_source, merge_candidates,
    };
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn a_non_utf8_host_ancestor_does_not_affect_client_skill_addresses() {
        let temp = tempfile::tempdir().unwrap();
        let root = AutomaticSkillRoot {
            source: SkillSource::User,
            root: temp
                .path()
                .canonicalize()
                .unwrap()
                .join(OsString::from_vec(vec![0xff]))
                .join("skills"),
        };
        let marker = root.root.join("guide/SKILL.md");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "---\nname: guide\ndescription: Example\n---\nBody").unwrap();
        let mut diagnostics = Vec::new();
        let candidates = collect_automatic_source(
            &root,
            &mut diagnostics,
            &crate::runtime::CancellationSignal::new(),
            &Native,
            CollectionView::BoundUser,
        )
        .unwrap();
        let mut outcome = merge_candidates(candidates, diagnostics);
        outcome.roots.push(root);
        let visible = Path::new("/client/rustx/.agents/skills");
        outcome
            .publish_user_paths(visible, |relative| Ok(visible.join(relative)))
            .unwrap();
        assert_eq!(outcome.packages.len(), 1);
        assert_eq!(
            Path::new(outcome.packages[0].location()),
            visible.join("guide/SKILL.md")
        );
        assert_eq!(outcome.packages[0].instruction_source(), marker);
        assert_eq!(outcome.diagnostics, [] as [SkillDiagnostic; 0]);
    }
}
