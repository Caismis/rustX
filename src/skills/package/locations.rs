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
            let relative = Path::new(location)
                .strip_prefix(&source_root)
                .map_err(|_| io::ErrorKind::InvalidData)?;
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
