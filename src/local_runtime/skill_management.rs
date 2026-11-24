//! Native Skills discovery and paginated instruction reads, without model work.
use super::configuration::observation::{digest, encode};
use super::configuration::settings::{SettingsError, SourceTarget};
use super::session_runtime_manager::{LocalExecutionEndpoint, SessionRuntimeManager};
use crate::skills::{SkillCatalogEntry, SkillDiagnostic, SkillProvenance};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};

mod display;
mod operations;
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
    pub enabled_by_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_revision: Option<String>,
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
    authoring_collection: Option<&Path>,
) -> Result<Option<SkillDescriptor>, SettingsError> {
    let mut skill = SkillDescriptor {
        catalog: SkillCatalogEntry {
            name: package.name().into(),
            description: package.description().into(),
            location: package.location().into(),
        },
        provenance: provenance.clone(),
        disable_model_invocation: package.disable_model_invocation(),
        enabled_by_default: package.enabled_by_default(),
        operation_revision: authoring_collection
            .and_then(|collection| operations::operation_revision(package, collection)),
        metadata_revision: digest(&package.definition()).map_err(|_| SettingsError::Io)?,
        display: authoring_collection.and_then(|collection| read_display(package, collection)),
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
    bound: bool,
) -> Option<std::path::PathBuf> {
    let target = match source {
        crate::skills::SkillSource::User => &SourceTarget::User,
        crate::skills::SkillSource::Workspace if bound => return None,
        crate::skills::SkillSource::Workspace => target,
    };
    Some(
        configuration
            .resource_root(target)
            .join(crate::skills::SKILLS_ROOT),
    )
}

fn discover(
    configuration: &super::configuration::UserConfigManager,
    target: &SourceTarget,
    endpoint: Option<&LocalExecutionEndpoint>,
) -> Result<crate::skills::SkillDiscoveryOutcome, SettingsError> {
    if let Some(endpoint) = endpoint {
        return crate::skills::SkillDiscovery::with_config(crate::skills::SkillDiscoveryConfig {
            automatic: crate::skills::automatic_skill_roots(
                Some(&configuration.sources.home_directory),
                endpoint.workspace(),
            ),
        })
        .with_cancellation(configuration.resource_io.clone())
        .discover_bound(endpoint)
        .map_err(|_| SettingsError::ExecutionUnavailable);
    }
    target.validate()?;
    configuration
        .discover_skills(target.workspace())
        .ok_or(SettingsError::Invalid)
}

fn read_instructions(
    endpoint: Option<&LocalExecutionEndpoint>,
    path: &Path,
    offset: u64,
    max_bytes: usize,
) -> Result<(String, Option<u64>, String), SettingsError> {
    if let Some(endpoint) = endpoint {
        let deadline =
            std::time::Instant::now() + rustx_execution_protocol::transport::PAGE_REQUEST_TIMEOUT;
        let chunk = endpoint
            .read_workspace_source(path, deadline, |data, relative| {
                data.read_range_admitted(
                    relative,
                    offset,
                    u32::try_from(max_bytes).expect("bounded page"),
                    None,
                    deadline,
                    || Ok(()),
                    || (),
                )
            })
            .map_err(|_| SettingsError::ExecutionUnavailable)?;
        return Ok((
            base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
            chunk.next_offset,
            chunk.metadata.revision,
        ));
    }
    let directory =
        crate::package_io::PackageDirectory::open(path.parent().ok_or(SettingsError::Invalid)?)
            .map_err(|_| SettingsError::Invalid)?;
    let chunk = super::environment::files::read_range(
        &directory,
        Path::new(path.file_name().ok_or(SettingsError::Invalid)?),
        offset,
        max_bytes,
        None,
    )
    .map_err(|_| SettingsError::Io)?;
    Ok((
        chunk.data_base64,
        chunk.next_offset,
        chunk.metadata.revision,
    ))
}

impl SessionRuntimeManager {
    #[allow(clippy::too_many_arguments)] // Native wire parameters and physical admission travel together.
    pub(crate) async fn skill_page(
        &self,
        target: SourceTarget,
        endpoint: Option<Arc<LocalExecutionEndpoint>>,
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
            let discovered = discover(&configuration, &target, endpoint.as_deref())?;
            let mut descriptors = Vec::new();
            let mut all_diagnostics = discovered.diagnostics;
            for package in &discovered.packages {
                let provenance = discovered
                    .provenance
                    .iter()
                    .find(|entry| entry.name == package.name())
                    .ok_or(SettingsError::Invalid)?;
                let collection = skill_collection(
                    &configuration,
                    &target,
                    package.source(),
                    endpoint.is_some(),
                );
                match descriptor(package, provenance, collection.as_deref())? {
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
        endpoint: Option<Arc<LocalExecutionEndpoint>>,
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
            let discovered = discover(&configuration, &target, endpoint.as_deref())?;
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
            let collection = skill_collection(
                &configuration,
                &target,
                package.source(),
                endpoint.is_some(),
            );
            let skill = descriptor(package, provenance, collection.as_deref())?
                .ok_or(SettingsError::Invalid)?;
            let (data_base64, next_offset, revision) = match kind {
                SkillContentKind::Instructions => {
                    let workspace = endpoint
                        .as_deref()
                        .filter(|_| package.source() == crate::skills::SkillSource::Workspace);
                    read_instructions(workspace, package.instruction_source(), offset, max_bytes)?
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
            let current = discover(&configuration, &target, endpoint.as_deref())?;
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
