//! Native Skills discovery and paginated instruction reads, without model work.
use super::configuration::observation::{digest, encode};
use super::configuration::settings::{SettingsError, SourceTarget};
use super::session_runtime_manager::SessionRuntimeManager;
use crate::skills::{SkillCatalogEntry, SkillDiagnostic, SkillProvenance};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::Path;

mod display;
use display::{SkillDisplay, read_display};
pub use display::{SkillDisplayWrite, SkillIconUpload};

const SKILL_PAGE_DATA_BYTES: usize = 131_072;
const SKILL_DESCRIPTION_PREVIEW_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillDescriptor {
    pub catalog: SkillCatalogEntry,
    pub provenance: SkillProvenance,
    pub metadata_revision: String,
    pub disable_model_invocation: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<SkillDisplay>,
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
    collection: &Path,
) -> Result<Option<SkillDescriptor>, SettingsError> {
    let mut skill = SkillDescriptor {
        catalog: SkillCatalogEntry {
            name: package.name().into(),
            description: package.description().into(),
            location: package.location().into(),
        },
        provenance: provenance.clone(),
        disable_model_invocation: package.disable_model_invocation(),
        metadata_revision: digest(&package.definition()).map_err(|_| SettingsError::Io)?,
        display: read_display(package, collection),
    };
    // Leave room for the surrounding array so every projected descriptor can
    // be paged on its own. Only the WebUI preview is shortened.
    let limit = SKILL_PAGE_DATA_BYTES - 2;
    if crate::bounded_file::json_fits(&skill, limit) {
        return Ok(Some(skill));
    }
    let description = &mut skill.catalog.description;
    if description.len() > SKILL_DESCRIPTION_PREVIEW_BYTES {
        description.truncate(description.floor_char_boundary(SKILL_DESCRIPTION_PREVIEW_BYTES));
        description.push('…');
        if crate::bounded_file::json_fits(&skill, limit) {
            return Ok(Some(skill));
        }
    }
    skill.provenance.shadowed.clear();
    if crate::bounded_file::json_fits(&skill, limit) {
        return Ok(Some(skill));
    }
    skill.display = None;
    Ok(crate::bounded_file::json_fits(&skill, limit).then_some(skill))
}
fn skill_collection(
    configuration: &super::configuration::UserConfigManager,
    target: &SourceTarget,
    source: crate::skills::SkillSource,
) -> std::path::PathBuf {
    configuration
        .resource_root(match source {
            crate::skills::SkillSource::User => &SourceTarget::User,
            crate::skills::SkillSource::Workspace => target,
        })
        .join(crate::skills::SKILLS_ROOT)
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
            let mut descriptors = Vec::new();
            let mut all_diagnostics = discovered.diagnostics;
            for package in &discovered.packages {
                let provenance = discovered
                    .provenance
                    .iter()
                    .find(|entry| entry.name == package.name())
                    .ok_or(SettingsError::Invalid)?;
                let collection = skill_collection(&configuration, &target, package.source());
                match descriptor(package, provenance, &collection)? {
                    Some(descriptor) => descriptors.push(descriptor),
                    None => all_diagnostics.push(SkillDiagnostic::PackageWarning {
                        source: provenance.source,
                        package: provenance.location.clone(),
                        detail: "Skill identity is too large to display".into(),
                    }),
                }
            }
            let revision =
                digest(&(&descriptors, &all_diagnostics)).map_err(|_| SettingsError::Io)?;
            if let Some(expected) = expected
                && expected != revision
            {
                return Err(SettingsError::Conflict {
                    scope: target.scope(),
                    expected,
                    actual: revision,
                });
            }
            let total = descriptors.len();
            if offset > total {
                return Err(SettingsError::Invalid);
            }
            let mut data = Vec::new();
            for descriptor in descriptors.into_iter().skip(offset).take(limit) {
                data.push(descriptor);
                if !crate::bounded_file::json_fits(&data, SKILL_PAGE_DATA_BYTES) {
                    data.pop();
                    break;
                }
            }
            let next_offset = (offset + data.len() < total).then_some(offset + data.len());
            let mut diagnostics = Vec::new();
            let diagnostic_count = all_diagnostics.len();
            for diagnostic in all_diagnostics.into_iter().take(32) {
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
            let collection = skill_collection(&configuration, &target, package.source());
            let skill =
                descriptor(package, provenance, &collection)?.ok_or(SettingsError::Invalid)?;
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
