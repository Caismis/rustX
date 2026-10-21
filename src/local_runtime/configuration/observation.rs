//! Source Settings read projections stay owned by the existing configuration
//! coordinator. This module only bounds its returned fields and byte reads.
use super::settings::{SourceSettings, SourceTarget};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;
pub const INLINE_BYTES: usize = 16_384;
pub const READ_BYTES: u32 = 65_536;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceFieldName {
    Target,
    ProcessPolicyImpacts,
    ProcessBindings,
    Application,
    ProspectiveApprovalMode,
    ProspectiveResources,
    ProspectiveDiagnostic,
    AbsentResourceRevision,
    ResourceRevisions,
    Resolved,
    SessionModels,
    Provenance,
    User,
    Workspace,
    UserRepair,
    WorkspaceRepair,
    RepairSequence,
    UserResourceRoot,
    WorkspaceResourceRoot,
    RuntimeRoot,
    UserMcp,
    WorkspaceMcp,
    Agents,
    UserModels,
    WorkspaceModels,
    ResolvedModels,
}
const FIELDS: [SourceFieldName; 23] = [
    SourceFieldName::Target,
    SourceFieldName::ProcessPolicyImpacts,
    SourceFieldName::ProcessBindings,
    SourceFieldName::Application,
    SourceFieldName::ProspectiveApprovalMode,
    SourceFieldName::ProspectiveResources,
    SourceFieldName::ProspectiveDiagnostic,
    SourceFieldName::AbsentResourceRevision,
    SourceFieldName::ResourceRevisions,
    SourceFieldName::Resolved,
    SourceFieldName::SessionModels,
    SourceFieldName::Provenance,
    SourceFieldName::User,
    SourceFieldName::Workspace,
    SourceFieldName::UserRepair,
    SourceFieldName::WorkspaceRepair,
    SourceFieldName::RepairSequence,
    SourceFieldName::UserResourceRoot,
    SourceFieldName::WorkspaceResourceRoot,
    SourceFieldName::RuntimeRoot,
    SourceFieldName::UserMcp,
    SourceFieldName::WorkspaceMcp,
    SourceFieldName::Agents,
];
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceFieldRef {
    pub revision: String,
    pub field: SourceFieldName,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub item_index: Option<u64>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub byte_length: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceField {
    pub name: SourceFieldName,
    pub value: Option<serde_json::Value>,
    pub reference: Option<SourceFieldRef>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundedSourceSettings {
    pub target: SourceTarget,
    pub revision: String,
    pub inline: Option<Box<SourceSettings>>,
    pub fields: Vec<SourceField>,
    pub collections: Vec<SourceCollection>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceCollection {
    pub field: SourceFieldName,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub total: u64,
    pub kind: String,
}
/// Encoded bytes alone cannot admit arbitrarily many small display objects.
pub(crate) fn count_bounded(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.len() <= 32 && items.iter().all(count_bounded),
        serde_json::Value::Object(items) => items.len() <= 32 && items.values().all(count_bounded),
        _ => true,
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceValueChunk {
    pub reference: SourceFieldRef,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub end_offset: u64,
    pub data_base64: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}

struct Hash(Sha256);
impl io::Write for Hash {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(crate) fn revision(source: &SourceSettings) -> Result<String, String> {
    digest(source)
}
pub(crate) fn digest(source: &impl Serialize) -> Result<String, String> {
    // JSON escaping can emit one or two bytes per write. Hash the same exact
    // stream in finite blocks instead of invoking SHA-256 for each escape on
    // every revision validation. This neither caches nor omits source evidence.
    let mut hash = io::BufWriter::with_capacity(8_192, Hash(Sha256::new()));
    serde_json::to_writer(&mut hash, source).map_err(|e| e.to_string())?;
    let hash = hash.into_inner().map_err(|e| e.to_string())?;
    Ok(format!("{:x}", hash.0.finalize()))
}
struct Range {
    offset: u64,
    limit: usize,
    total: u64,
    bytes: Vec<u8>,
}
impl io::Write for Range {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = u64::try_from(bytes.len()).map_err(io::Error::other)?;
        let end = self
            .total
            .checked_add(length)
            .filter(|end| *end <= crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
            .ok_or_else(|| io::Error::other("source value exceeds safe byte range"))?;
        if end > self.offset && self.bytes.len() < self.limit {
            let start = usize::try_from(self.offset.saturating_sub(self.total))
                .map_err(io::Error::other)?
                .min(bytes.len());
            let count = (self.limit - self.bytes.len()).min(bytes.len() - start);
            self.bytes.extend_from_slice(&bytes[start..start + count]);
        }
        self.total = end;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(crate) fn encode(
    value: &(impl Serialize + schemars::JsonSchema),
    offset: u64,
    limit: usize,
) -> Result<(Vec<u8>, u64), String> {
    let mut range = Range {
        offset,
        limit,
        total: 0,
        bytes: Vec::with_capacity(limit),
    };
    // The range keeps only requested bytes, but still counts the complete exact
    // encoding. Batch tiny escaped-string writes with the same finite scratch
    // bound used by revision hashing; do not allocate the full field.
    let mut writer = io::BufWriter::with_capacity(8_192, &mut range);
    crate::observation::structural_json::to_writer(&mut writer, value)
        .map_err(|e| e.to_string())?;
    writer.into_inner().map_err(|e| e.to_string())?;
    Ok((range.bytes, range.total))
}
fn field(
    source: &SourceSettings,
    name: SourceFieldName,
    offset: u64,
    limit: usize,
) -> Result<(Vec<u8>, u64), String> {
    if let Some(models) = models(source, name) {
        return encode(models, offset, limit);
    }
    if matches!(
        name,
        SourceFieldName::UserModels
            | SourceFieldName::WorkspaceModels
            | SourceFieldName::ResolvedModels
    ) {
        return encode(
            &std::collections::BTreeMap::<String, String>::new(),
            offset,
            limit,
        );
    }
    macro_rules! fields {($($variant:ident:$member:ident),+)=>{match name {$(SourceFieldName::$variant=>encode(&source.$member,offset,limit)),+, _=>unreachable!("derived model field handled above")}};}
    fields!(Target:target,ProcessPolicyImpacts:process_policy_impacts,ProcessBindings:process_bindings,Application:application,ProspectiveApprovalMode:prospective_approval_mode,ProspectiveResources:prospective_resources,ProspectiveDiagnostic:prospective_diagnostic,AbsentResourceRevision:absent_resource_revision,ResourceRevisions:resource_revisions,Resolved:resolved,SessionModels:session_models,Provenance:provenance,User:user,Workspace:workspace,UserRepair:user_repair,WorkspaceRepair:workspace_repair,RepairSequence:repair_sequence,UserResourceRoot:user_resource_root,WorkspaceResourceRoot:workspace_resource_root,RuntimeRoot:runtime_root,UserMcp:user_mcp,WorkspaceMcp:workspace_mcp,Agents:agents)
}
pub(crate) fn bounded(source: &SourceSettings) -> Result<BoundedSourceSettings, String> {
    let revision = revision(source)?;
    let (inline, total) = encode(source, 0, 524_288)?;
    let collections = collections(source)?;
    if total <= 524_288 {
        let value: serde_json::Value =
            serde_json::from_slice(&inline).map_err(|error| error.to_string())?;
        if count_bounded(&value) {
            return Ok(BoundedSourceSettings {
                target: source.target.clone(),
                revision,
                inline: Some(Box::new(source.clone())),
                fields: vec![],
                collections,
            });
        }
    }
    let mut fields = Vec::with_capacity(FIELDS.len());
    for name in FIELDS {
        let (bytes, total) = field(source, name, 0, INLINE_BYTES)?;
        let inline_value = (total <= INLINE_BYTES as u64)
            .then(|| serde_json::from_slice::<serde_json::Value>(&bytes))
            .transpose()
            .map_err(|error| error.to_string())?;
        let (value, reference) = if inline_value.as_ref().is_some_and(count_bounded) {
            (inline_value, None)
        } else {
            (
                None,
                Some(SourceFieldRef {
                    revision: revision.clone(),
                    field: name,
                    item_index: None,
                    byte_length: total,
                }),
            )
        };
        fields.push(SourceField {
            name,
            value,
            reference,
        });
    }
    Ok(BoundedSourceSettings {
        target: source.target.clone(),
        revision,
        inline: None,
        fields,
        collections,
    })
}
pub(crate) fn read(
    source: &SourceSettings,
    reference: &SourceFieldRef,
    offset: u64,
    max_bytes: u32,
) -> Result<SourceValueChunk, String> {
    if max_bytes == 0
        || max_bytes > READ_BYTES
        || offset > crate::observation::content::CONTENT_SAFE_INTEGER_MAX
        || offset
            .checked_add(u64::from(max_bytes))
            .is_none_or(|end| end > crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
    {
        return Err("invalid source byte range".into());
    }
    if reference.byte_length > crate::observation::content::CONTENT_SAFE_INTEGER_MAX
        || reference
            .item_index
            .is_some_and(|index| index > crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
    {
        return Err("invalid source read evidence".into());
    }
    if revision(source)? != reference.revision {
        return Err("source read cut expired".into());
    }
    let limit = usize::try_from(max_bytes).map_err(|e| e.to_string())?;
    let (bytes, total) = if let Some(index) = reference.item_index {
        item(source, reference.field, index, offset, limit)?
    } else {
        field(source, reference.field, offset, limit)?
    };
    if total != reference.byte_length || offset > total {
        return Err("source byte evidence does not match".into());
    }
    let end_offset = offset
        .checked_add(u64::try_from(bytes.len()).map_err(|e| e.to_string())?)
        .ok_or("source byte range overflow")?;
    if offset < total && end_offset == offset {
        return Err("source read did not advance".into());
    }
    Ok(SourceValueChunk {
        reference: reference.clone(),
        offset,
        end_offset,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        next_offset: (end_offset < total).then_some(end_offset),
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourcePageItem {
    pub index: u64,
    pub value: Option<serde_json::Value>,
    pub reference: Option<SourceFieldRef>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceFieldPage {
    pub revision: String,
    pub field: SourceFieldName,
    pub kind: String,
    pub total: u64,
    pub items: Vec<SourcePageItem>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}
fn models(
    source: &SourceSettings,
    name: SourceFieldName,
) -> Option<&std::collections::BTreeMap<String, crate::model::authoring::Model>> {
    match name {
        SourceFieldName::UserModels => source
            .user
            .authored
            .as_ref()
            .and_then(|source| source.models.as_ref()),
        SourceFieldName::WorkspaceModels => source
            .workspace
            .as_ref()
            .and_then(|source| source.authored.as_ref())
            .and_then(|source| source.models.as_ref()),
        SourceFieldName::ResolvedModels => source
            .resolved
            .as_ref()
            .and_then(|source| source.models.as_ref()),
        _ => None,
    }
}
fn collections(source: &SourceSettings) -> Result<Vec<SourceCollection>, String> {
    [
        SourceFieldName::ProcessPolicyImpacts,
        SourceFieldName::ResourceRevisions,
        SourceFieldName::Provenance,
        SourceFieldName::Agents,
        SourceFieldName::UserMcp,
        SourceFieldName::WorkspaceMcp,
        SourceFieldName::UserModels,
        SourceFieldName::WorkspaceModels,
        SourceFieldName::ResolvedModels,
        SourceFieldName::SessionModels,
    ]
    .into_iter()
    .map(|field| {
        let (total, kind) = count(source, field)?;
        Ok(SourceCollection {
            field,
            total: u64::try_from(total).map_err(|error| error.to_string())?,
            kind: kind.into(),
        })
    })
    .collect()
}
fn count(source: &SourceSettings, name: SourceFieldName) -> Result<(usize, &'static str), String> {
    Ok(match name {
        SourceFieldName::UserModels
        | SourceFieldName::WorkspaceModels
        | SourceFieldName::ResolvedModels => (
            models(source, name).map_or(0, std::collections::BTreeMap::len),
            "map",
        ),
        SourceFieldName::SessionModels => (
            match &source.session_models {
                Some(super::settings::SessionModelsView::Available { catalog, .. }) => {
                    catalog.models.len()
                }
                _ => 0,
            },
            "array",
        ),
        SourceFieldName::ProcessPolicyImpacts => (source.process_policy_impacts.len(), "map"),
        SourceFieldName::ResourceRevisions => (source.resource_revisions.len(), "map"),
        SourceFieldName::Provenance => (source.provenance.len(), "map"),
        SourceFieldName::Agents => (source.agents.len(), "array"),
        SourceFieldName::UserMcp => (
            source
                .user_mcp
                .authored
                .as_ref()
                .map_or(0, std::collections::BTreeMap::len),
            "map",
        ),
        SourceFieldName::WorkspaceMcp => (
            source
                .workspace_mcp
                .as_ref()
                .and_then(|source| source.authored.as_ref())
                .map_or(0, std::collections::BTreeMap::len),
            "map",
        ),
        _ => return Err("source field is not a pageable collection".into()),
    })
}
fn item(
    source: &SourceSettings,
    name: SourceFieldName,
    index: u64,
    offset: u64,
    limit: usize,
) -> Result<(Vec<u8>, u64), String> {
    let index = usize::try_from(index).map_err(|e| e.to_string())?;
    macro_rules! map {
        ($value:expr) => {
            encode(
                &$value
                    .iter()
                    .nth(index)
                    .ok_or("source page item out of range")?,
                offset,
                limit,
            )
        };
    }
    match name {
        SourceFieldName::UserModels
        | SourceFieldName::WorkspaceModels
        | SourceFieldName::ResolvedModels => {
            map!(models(source, name).ok_or("model collection unavailable")?)
        }
        SourceFieldName::SessionModels => match &source.session_models {
            Some(super::settings::SessionModelsView::Available { catalog, .. }) => encode(
                catalog.models.get(index).ok_or("model item out of range")?,
                offset,
                limit,
            ),
            _ => Err("model source unavailable".into()),
        },
        SourceFieldName::ProcessPolicyImpacts => map!(source.process_policy_impacts),
        SourceFieldName::ResourceRevisions => map!(source.resource_revisions),
        SourceFieldName::Provenance => map!(source.provenance),
        SourceFieldName::Agents => encode(
            source
                .agents
                .get(index)
                .ok_or("source page item out of range")?,
            offset,
            limit,
        ),
        SourceFieldName::UserMcp => map!(
            source
                .user_mcp
                .authored
                .as_ref()
                .ok_or("source MCP collection unavailable")?
        ),
        SourceFieldName::WorkspaceMcp => map!(
            source
                .workspace_mcp
                .as_ref()
                .and_then(|source| source.authored.as_ref())
                .ok_or("source MCP collection unavailable")?
        ),
        _ => Err("source field is not a pageable collection".into()),
    }
}
pub(crate) fn page(
    source: &SourceSettings,
    expected: &str,
    name: SourceFieldName,
    offset: u64,
    limit: usize,
) -> Result<SourceFieldPage, String> {
    if limit == 0 || limit > 32 || offset > crate::observation::content::CONTENT_SAFE_INTEGER_MAX {
        return Err("invalid source page bounds".into());
    }
    let revision = revision(source)?;
    if revision != expected {
        return Err("source read cut expired".into());
    }
    let (total, kind) = count(source, name)?;
    let total = u64::try_from(total).map_err(|e| e.to_string())?;
    if offset > total {
        return Err("source page out of range".into());
    }
    let end = offset
        .checked_add(u64::try_from(limit).map_err(|e| e.to_string())?)
        .ok_or("source page overflow")?
        .min(total);
    let mut items = Vec::with_capacity(limit);
    for index in offset..end {
        let (bytes, size) = item(source, name, index, 0, INLINE_BYTES)?;
        let inline_value = (size <= INLINE_BYTES as u64)
            .then(|| serde_json::from_slice::<serde_json::Value>(&bytes))
            .transpose()
            .map_err(|error| error.to_string())?;
        let (value, reference) = if inline_value.as_ref().is_some_and(count_bounded) {
            (inline_value, None)
        } else {
            (
                None,
                Some(SourceFieldRef {
                    revision: revision.clone(),
                    field: name,
                    item_index: Some(index),
                    byte_length: size,
                }),
            )
        };
        items.push(SourcePageItem {
            index,
            value,
            reference,
        });
    }
    Ok(SourceFieldPage {
        revision,
        field: name,
        kind: kind.into(),
        total,
        items,
        next_offset: (end < total).then_some(end),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceWriteReceipt {
    pub revision: String,
    pub configuration_revision: Option<u64>,
}

/// Bounded delivery of the Session catalog's immutable settings read cut.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundedSessionSettings {
    pub revision: u64,
    pub cwd: std::path::PathBuf,
    pub inline: Option<Box<crate::local_runtime::session::SessionPersistentState>>,
    pub reference: Option<SessionSettingsRef>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSettingsRef {
    pub revision: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub byte_length: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSettingsChunk {
    pub reference: SessionSettingsRef,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub end_offset: u64,
    pub data_base64: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}
pub(crate) fn session_settings(
    revision: u64,
    settings: &crate::local_runtime::session::SessionPersistentState,
) -> Result<BoundedSessionSettings, String> {
    let (bytes, total) = encode(settings, 0, 524_288)?;
    let inline_value = (total <= 524_288)
        .then(|| serde_json::from_slice::<serde_json::Value>(&bytes))
        .transpose()
        .map_err(|error| error.to_string())?;
    let (inline, reference) = if inline_value.as_ref().is_some_and(count_bounded) {
        (Some(Box::new(settings.clone())), None)
    } else {
        (
            None,
            Some(SessionSettingsRef {
                revision,
                byte_length: total,
            }),
        )
    };
    Ok(BoundedSessionSettings {
        revision,
        cwd: settings.cwd.clone(),
        inline,
        reference,
    })
}
pub(crate) fn session_settings_read(
    revision: u64,
    settings: &crate::local_runtime::session::SessionPersistentState,
    reference: &SessionSettingsRef,
    offset: u64,
    max_bytes: u32,
) -> Result<SessionSettingsChunk, String> {
    if revision != reference.revision {
        return Err("source read cut expired".into());
    }
    if max_bytes == 0
        || max_bytes > READ_BYTES
        || offset > crate::observation::content::CONTENT_SAFE_INTEGER_MAX
        || offset
            .checked_add(u64::from(max_bytes))
            .is_none_or(|end| end > crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
        || reference.byte_length > crate::observation::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err("invalid settings byte range".into());
    }
    let (bytes, total) = encode(
        settings,
        offset,
        usize::try_from(max_bytes).map_err(|e| e.to_string())?,
    )?;
    if total != reference.byte_length || offset > total {
        return Err("settings read evidence does not match".into());
    }
    let end_offset = offset
        .checked_add(u64::try_from(bytes.len()).map_err(|e| e.to_string())?)
        .ok_or("settings byte overflow")?;
    if offset < total && end_offset == offset {
        return Err("settings read did not advance".into());
    }
    Ok(SessionSettingsChunk {
        reference: reference.clone(),
        offset,
        end_offset,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        next_offset: (end_offset < total).then_some(end_offset),
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundedSessionApplication {
    pub scope: String,
    pub sources: Vec<SourceTarget>,
    pub version: u64,
    pub inline: Option<Box<super::application::ConfigurationApplication>>,
    pub reference: Option<SessionApplicationRef>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionApplicationRef {
    pub version: u64,
    pub revision: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub byte_length: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionApplicationChunk {
    pub reference: SessionApplicationRef,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub end_offset: u64,
    pub data_base64: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}
pub(crate) fn session_application(
    source: &super::application::ConfigurationApplication,
) -> Result<BoundedSessionApplication, String> {
    if source.sources.len() > 32 {
        return Err("application source owner count exceeds bounds".into());
    }
    let (bytes, total) = encode(source, 0, INLINE_BYTES)?;
    let inline = if total <= INLINE_BYTES as u64
        && count_bounded(&serde_json::from_slice(&bytes).map_err(|error| error.to_string())?)
    {
        Some(Box::new(source.clone()))
    } else {
        None
    };
    let reference = if inline.is_none() {
        Some(SessionApplicationRef {
            version: source.version,
            revision: digest(source)?,
            byte_length: total,
        })
    } else {
        None
    };
    Ok(BoundedSessionApplication {
        scope: source.scope.clone(),
        sources: source.sources.clone(),
        version: source.version,
        inline,
        reference,
    })
}
pub(crate) fn session_application_read(
    source: &super::application::ConfigurationApplication,
    reference: &SessionApplicationRef,
    offset: u64,
    max_bytes: u32,
) -> Result<SessionApplicationChunk, String> {
    if max_bytes == 0
        || max_bytes > READ_BYTES
        || offset
            .checked_add(u64::from(max_bytes))
            .is_none_or(|end| end > crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
    {
        return Err("invalid source read bounds".into());
    }
    if source.version != reference.version || digest(source)? != reference.revision {
        return Err("source read cut expired".into());
    }
    let (bytes, end) = encode(source, offset, max_bytes as usize)?;
    if end != reference.byte_length || offset > end || (offset < end && bytes.is_empty()) {
        return Err("invalid source read bounds".into());
    }
    let next = offset + bytes.len() as u64;
    Ok(SessionApplicationChunk {
        reference: reference.clone(),
        offset,
        end_offset: end,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        next_offset: (next < end).then_some(next),
    })
}

#[cfg(test)]
mod application_tests {
    use super::*;
    use crate::local_runtime::configuration::application::{
        ApplicationIdentity, ApplyUnit, ConfigurationApplication, UnitApplication,
    };
    #[test]
    fn configuration_revision_preserves_exact_json_bytes_across_buffer_boundaries() {
        for count in [0, 1, 8_191, 8_192, 8_193, 300_000] {
            let source = serde_json::json!({
                "escaped": "é\n\"\\".repeat(count),
                "integer": u64::MAX,
                "nested": [true, null, {"revision": 1}],
            });
            let original = serde_json::to_vec(&source).unwrap();
            assert_eq!(
                digest(&source).unwrap(),
                format!("{:x}", Sha256::digest(&original))
            );
            let mut changed = source;
            changed["nested"][2]["revision"] = 2.into();
            assert_ne!(
                digest(&changed).unwrap(),
                format!("{:x}", Sha256::digest(&original))
            );
        }
    }

    #[test]
    fn configuration_ranges_preserve_exact_structural_bytes_across_buffer_boundaries() {
        let source = serde_json::json!({"escaped": "é\n\"\\".repeat(30_000), "native": u64::MAX});
        let mut original = Vec::new();
        crate::observation::structural_json::to_writer(&mut original, &source).unwrap();
        for offset in [0, 8_191, 8_192, 8_193, original.len() - 1, original.len()] {
            for limit in [1, 17, 65_536] {
                let (bytes, total) = encode(&source, offset as u64, limit).unwrap();
                assert_eq!(total, original.len() as u64);
                assert_eq!(
                    bytes,
                    original[offset..(offset + limit).min(original.len())]
                );
                assert!(bytes.len() <= limit);
            }
        }
    }

    #[test]
    fn t15_application_diagnostics_are_bounded_recoverable_and_version_fenced() {
        let mut source = ConfigurationApplication {
            scope: "session-fixture".into(),
            sources: vec![SourceTarget::User],
            version: u64::MAX,
            desired: ApplicationIdentity {
                input_revision: Some("captured".into()),
                attempt: 9_007_199_254_740_993,
            },
            units: std::collections::BTreeMap::from([(
                ApplyUnit::Capabilities,
                UnitApplication::Failed {
                    diagnostic: "é\\\"failure".repeat(40_000),
                },
            )]),
            candidate: None,
        };
        let bounded = session_application(&source).unwrap();
        assert!(bounded.inline.is_none());
        assert!(serde_json::to_vec(&bounded).unwrap().len() < INLINE_BYTES);
        assert_eq!(bounded.sources, source.sources);
        let reference = bounded.reference.as_ref().unwrap();
        let mut bytes = Vec::new();
        let mut offset = 0;
        loop {
            let chunk = session_application_read(&source, reference, offset, READ_BYTES).unwrap();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(chunk.data_base64)
                .unwrap();
            assert!(decoded.len() <= 65_536);
            assert!(!decoded.is_empty());
            bytes.extend(decoded);
            let Some(next) = chunk.next_offset else {
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["version"], u64::MAX.to_string());
        assert_eq!(value["desired"]["attempt"], "9007199254740993");
        let restored: ConfigurationApplication =
            crate::observation::structural_json::decode(value).unwrap();
        assert_eq!(restored, source);
        let mut modified = reference.clone();
        modified.byte_length += 1;
        assert!(session_application_read(&source, &modified, 0, 1).is_err());
        assert!(
            session_application_read(&source, reference, reference.byte_length + 1, 1).is_err()
        );
        source.desired.attempt += 1;
        assert_eq!(
            session_application_read(&source, reference, 0, 1).unwrap_err(),
            "source read cut expired"
        );
    }
}
