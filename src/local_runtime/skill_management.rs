//! Native Skills discovery and paginated instruction reads, without model work.
use super::configuration::observation::{digest, encode};
use super::configuration::settings::{SettingsError, SourceTarget};
use super::session_runtime_manager::SessionRuntimeManager;
use crate::skills::{SkillCatalogEntry, SkillDiagnostic, SkillProvenance};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillDescriptor {
    pub catalog: SkillCatalogEntry,
    pub provenance: SkillProvenance,
    pub metadata_revision: String,
    pub disable_model_invocation: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillPage {
    pub data: Vec<SkillDescriptor>,
    pub diagnostics: Vec<SkillDiagnostic>,
    pub diagnostics_truncated: bool,
    pub next_offset: Option<usize>,
    pub revision: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillContent {
    pub skill: SkillDescriptor,
    pub revision: String,
    pub kind: SkillContentKind,
    pub data_base64: String,
    pub next_offset: Option<u64>,
}
fn descriptor(
    package: &crate::skills::SkillPackage,
    provenance: &SkillProvenance,
) -> Result<SkillDescriptor, SettingsError> {
    Ok(SkillDescriptor {
        catalog: SkillCatalogEntry {
            name: package.name().into(),
            description: package.description().into(),
            location: package.location().into(),
        },
        provenance: provenance.clone(),
        disable_model_invocation: package.disable_model_invocation(),
        metadata_revision: digest(&package.definition()).map_err(|_| SettingsError::Io)?,
    })
}
impl SessionRuntimeManager {
    pub(crate) async fn skill_page(
        &self,
        target: SourceTarget,
        offset: usize,
        limit: usize,
        expected: Option<String>,
        owner: impl Send + 'static,
    ) -> Result<SkillPage, SettingsError> {
        if !(1..=32).contains(&limit) {
            return Err(SettingsError::Invalid);
        }
        let configuration = self.configuration.clone();
        tokio::task::spawn_blocking(move || {
            // Native discovery/read retains admission even if its RPC waiter leaves.
            let _owner = owner;
            #[cfg(test)]
            native_gate(&configuration.sources.config_path);
            target.validate()?;
            let discovered = configuration
                .discover_skills(target.workspace())
                .ok_or(SettingsError::Invalid)?;
            let descriptors: Vec<_> = discovered
                .packages
                .iter()
                .map(|package| {
                    let provenance = discovered
                        .provenance
                        .iter()
                        .find(|entry| entry.name == package.name())
                        .ok_or(SettingsError::Invalid)?;
                    descriptor(package, provenance)
                })
                .collect::<Result<_, _>>()?;
            let revision =
                digest(&(&descriptors, &discovered.diagnostics)).map_err(|_| SettingsError::Io)?;
            if let Some(expected) = expected
                && expected != revision
            {
                return Err(SettingsError::Conflict {
                    scope: target.scope(),
                    expected,
                    actual: revision,
                });
            }
            if offset > discovered.packages.len() {
                return Err(SettingsError::Invalid);
            }
            let mut data = Vec::new();
            for descriptor in descriptors.into_iter().skip(offset).take(limit) {
                data.push(descriptor);
                if !crate::bounded_file::json_fits(&data, 131_072) {
                    data.pop();
                    break;
                }
            }
            if data.is_empty() && offset < discovered.packages.len() {
                return Err(SettingsError::Invalid);
            }
            let next_offset =
                (offset + data.len() < discovered.packages.len()).then_some(offset + data.len());
            let mut diagnostics = Vec::new();
            let diagnostic_count = discovered.diagnostics.len();
            for diagnostic in discovered.diagnostics.into_iter().take(32) {
                diagnostics.push(diagnostic);
                if !crate::bounded_file::json_fits(&diagnostics, 65_536) {
                    diagnostics.pop();
                    break;
                }
            }
            Ok(SkillPage {
                data,
                diagnostics_truncated: diagnostic_count > diagnostics.len(),
                diagnostics,
                next_offset,
                revision,
            })
        })
        .await
        .map_err(|_| SettingsError::Io)?
    }
    #[allow(clippy::too_many_arguments)] // Native wire parameters and physical admission travel together.
    pub(crate) async fn skill_read(
        &self,
        target: SourceTarget,
        name: String,
        kind: SkillContentKind,
        offset: u64,
        max_bytes: usize,
        expected: Option<String>,
        owner: impl Send + 'static,
    ) -> Result<SkillContent, SettingsError> {
        if !(1..=65_536).contains(&max_bytes) {
            return Err(SettingsError::Invalid);
        }
        let configuration = self.configuration.clone();
        tokio::task::spawn_blocking(move || {
            // Native discovery/read retains admission even if its RPC waiter leaves.
            let _owner = owner;
            #[cfg(test)]
            native_gate(&configuration.sources.config_path);
            target.validate()?;
            let discovered = configuration
                .discover_skills(target.workspace())
                .ok_or(SettingsError::Invalid)?;
            let package = discovered
                .packages
                .iter()
                .find(|package| package.name() == name)
                .ok_or(SettingsError::Invalid)?;
            let provenance = discovered
                .provenance
                .iter()
                .find(|provenance| provenance.name == name)
                .ok_or(SettingsError::Invalid)?;
            let skill = descriptor(package, provenance)?;
            let (data_base64, next_offset, revision) = match kind {
                SkillContentKind::Instructions => {
                    let path = package.instruction_source();
                    let directory = crate::package_io::PackageDirectory::open(
                        path.parent().ok_or(SettingsError::Invalid)?,
                    )
                    .map_err(|_| SettingsError::Invalid)?;
                    let chunk = super::environment::files::read_range(
                        &directory,
                        Path::new(path.file_name().ok_or(SettingsError::Invalid)?),
                        offset,
                        max_bytes,
                        None,
                    )
                    .map_err(|_| SettingsError::Io)?;
                    (
                        chunk.data_base64,
                        chunk.next_offset,
                        chunk.metadata.revision,
                    )
                }
                SkillContentKind::Metadata => {
                    let (bytes, total) = encode(&package.definition(), offset, max_bytes)
                        .map_err(|_| SettingsError::Io)?;
                    if offset > total {
                        return Err(SettingsError::Invalid);
                    }
                    let end = offset + bytes.len() as u64;
                    (
                        base64::engine::general_purpose::STANDARD.encode(bytes),
                        (end < total).then_some(end),
                        skill.metadata_revision.clone(),
                    )
                }
            };
            let current = configuration
                .discover_skills(target.workspace())
                .ok_or(SettingsError::Invalid)?;
            if !current.packages.iter().any(|current| current == package)
                || !current
                    .provenance
                    .iter()
                    .any(|current| current == provenance)
            {
                return Err(SettingsError::Invalid);
            }
            if let Some(expected) = expected
                && expected != revision
            {
                return Err(SettingsError::Conflict {
                    scope: target.scope(),
                    expected,
                    actual: revision,
                });
            }
            Ok(SkillContent {
                skill,
                revision,
                kind,
                data_base64,
                next_offset,
            })
        })
        .await
        .map_err(|_| SettingsError::Io)?
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillContentKind {
    Instructions,
    Metadata,
}

#[cfg(test)]
type NativeGate = (
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);
#[cfg(test)]
static NATIVE_GATES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<std::path::PathBuf, NativeGate>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));
#[cfg(test)]
pub(crate) fn install_native_gate(path: std::path::PathBuf, gate: NativeGate) {
    assert!(NATIVE_GATES.lock().unwrap().insert(path, gate).is_none());
}
#[cfg(test)]
fn native_gate(path: &Path) {
    let gate = NATIVE_GATES.lock().unwrap().remove(path);
    if let Some((entered, release)) = gate {
        let _ = entered.send(());
        release.recv().unwrap();
    }
}
