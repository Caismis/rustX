//! Bounded reads remain projections of the native configuration/application owner.
use super::{
    observation::{INLINE_BYTES, READ_BYTES, count_bounded, digest, encode},
    settings::EffectiveConfiguration,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveFieldName {
    ProcessBindings,
    Application,
    AdoptedBinding,
    SourceRevisions,
    Generation,
    Document,
    RootAgent,
    Context,
    ModelTimeout,
    ToolDeadline,
    ChildCapacity,
    ApprovalMode,
    Provenance,
    Resources,
    AvailableTools,
    SessionModel,
    EffectiveModel,
    AdmittedAttempt,
}
const FIELDS: [EffectiveFieldName; 18] = [
    EffectiveFieldName::ProcessBindings,
    EffectiveFieldName::Application,
    EffectiveFieldName::AdoptedBinding,
    EffectiveFieldName::SourceRevisions,
    EffectiveFieldName::Generation,
    EffectiveFieldName::Document,
    EffectiveFieldName::RootAgent,
    EffectiveFieldName::Context,
    EffectiveFieldName::ModelTimeout,
    EffectiveFieldName::ToolDeadline,
    EffectiveFieldName::ChildCapacity,
    EffectiveFieldName::ApprovalMode,
    EffectiveFieldName::Provenance,
    EffectiveFieldName::Resources,
    EffectiveFieldName::AvailableTools,
    EffectiveFieldName::SessionModel,
    EffectiveFieldName::EffectiveModel,
    EffectiveFieldName::AdmittedAttempt,
];
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectiveFieldRef {
    pub revision: String,
    pub field: EffectiveFieldName,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub item_index: Option<u64>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub byte_length: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectiveField {
    pub name: EffectiveFieldName,
    pub value: Option<serde_json::Value>,
    pub reference: Option<EffectiveFieldRef>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundedEffectiveConfiguration {
    pub revision: String,
    pub inline: Option<Box<EffectiveConfiguration>>,
    pub fields: Vec<EffectiveField>,
    pub collections: Vec<EffectiveCollection>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectiveCollection {
    pub field: EffectiveFieldName,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub total: u64,
    pub kind: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectiveValueChunk {
    pub reference: EffectiveFieldRef,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub end_offset: u64,
    pub data_base64: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}
fn field(
    source: &EffectiveConfiguration,
    name: EffectiveFieldName,
    offset: u64,
    limit: usize,
) -> Result<(Vec<u8>, u64), String> {
    macro_rules! fields {($($variant:ident:$member:ident),+)=>{match name {$(EffectiveFieldName::$variant=>encode(&source.$member,offset,limit)),+}};}
    fields!(ProcessBindings:process_bindings,Application:application,AdoptedBinding:adopted_binding,SourceRevisions:source_revisions,Generation:generation,Document:document,RootAgent:root_agent,Context:context,ModelTimeout:model_timeout,ToolDeadline:tool_deadline,ChildCapacity:child_capacity,ApprovalMode:approval_mode,Provenance:provenance,Resources:resources,AvailableTools:available_tools,SessionModel:session_model,EffectiveModel:effective_model,AdmittedAttempt:admitted_attempt)
}
pub(crate) fn bounded(
    source: &EffectiveConfiguration,
) -> Result<BoundedEffectiveConfiguration, String> {
    let revision = digest(source)?;
    let (bytes, total) = encode(source, 0, 524_288)?;
    let collections = [
        (
            EffectiveFieldName::SourceRevisions,
            source.source_revisions.len(),
            "map",
        ),
        (
            EffectiveFieldName::Provenance,
            source.provenance.len(),
            "map",
        ),
        (
            EffectiveFieldName::AvailableTools,
            source.available_tools.len(),
            "array",
        ),
    ]
    .into_iter()
    .map(|(field, total, kind)| {
        Ok(EffectiveCollection {
            field,
            total: u64::try_from(total).map_err(|error| error.to_string())?,
            kind: kind.into(),
        })
    })
    .collect::<Result<Vec<_>, String>>()?;
    if total <= 524_288 {
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if count_bounded(&value) {
            return Ok(BoundedEffectiveConfiguration {
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
                Some(EffectiveFieldRef {
                    revision: revision.clone(),
                    field: name,
                    item_index: None,
                    byte_length: total,
                }),
            )
        };
        fields.push(EffectiveField {
            name,
            value,
            reference,
        });
    }
    Ok(BoundedEffectiveConfiguration {
        revision,
        inline: None,
        fields,
        collections,
    })
}
fn item(
    source: &EffectiveConfiguration,
    name: EffectiveFieldName,
    index: u64,
    offset: u64,
    limit: usize,
) -> Result<(Vec<u8>, u64), String> {
    let index = usize::try_from(index).map_err(|e| e.to_string())?;
    match name {
        EffectiveFieldName::SourceRevisions => encode(
            &source
                .source_revisions
                .iter()
                .nth(index)
                .ok_or("configuration item out of range")?,
            offset,
            limit,
        ),
        EffectiveFieldName::Provenance => encode(
            &source
                .provenance
                .iter()
                .nth(index)
                .ok_or("configuration item out of range")?,
            offset,
            limit,
        ),
        EffectiveFieldName::AvailableTools => encode(
            source
                .available_tools
                .get(index)
                .ok_or("configuration item out of range")?,
            offset,
            limit,
        ),
        _ => Err("configuration field is not a pageable collection".into()),
    }
}
pub(crate) fn read(
    source: &EffectiveConfiguration,
    reference: &EffectiveFieldRef,
    offset: u64,
    max_bytes: u32,
) -> Result<EffectiveValueChunk, String> {
    if max_bytes == 0
        || max_bytes > READ_BYTES
        || offset > crate::observation::content::CONTENT_SAFE_INTEGER_MAX
        || offset
            .checked_add(u64::from(max_bytes))
            .is_none_or(|end| end > crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
        || reference.byte_length > crate::observation::content::CONTENT_SAFE_INTEGER_MAX
        || reference
            .item_index
            .is_some_and(|index| index > crate::observation::content::CONTENT_SAFE_INTEGER_MAX)
    {
        return Err("invalid configuration byte bounds".into());
    }
    if digest(source)? != reference.revision {
        return Err("source read cut expired".into());
    }
    let limit = usize::try_from(max_bytes).map_err(|e| e.to_string())?;
    let (bytes, total) = if let Some(index) = reference.item_index {
        item(source, reference.field, index, offset, limit)?
    } else {
        field(source, reference.field, offset, limit)?
    };
    if total != reference.byte_length || offset > total {
        return Err("configuration read evidence does not match".into());
    }
    let end_offset = offset
        .checked_add(u64::try_from(bytes.len()).map_err(|e| e.to_string())?)
        .ok_or("configuration byte overflow")?;
    if offset < total && end_offset == offset {
        return Err("configuration read did not advance".into());
    }
    Ok(EffectiveValueChunk {
        reference: reference.clone(),
        offset,
        end_offset,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        next_offset: (end_offset < total).then_some(end_offset),
    })
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectivePageItem {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub index: u64,
    pub value: Option<serde_json::Value>,
    pub reference: Option<EffectiveFieldRef>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectivePage {
    pub revision: String,
    pub field: EffectiveFieldName,
    pub kind: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub total: u64,
    pub items: Vec<EffectivePageItem>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}
pub(crate) fn page(
    source: &EffectiveConfiguration,
    revision: &str,
    name: EffectiveFieldName,
    offset: u64,
    limit: usize,
) -> Result<EffectivePage, String> {
    if offset > crate::observation::content::CONTENT_SAFE_INTEGER_MAX || limit == 0 || limit > 32 {
        return Err("invalid configuration page bounds".into());
    }
    if digest(source)? != revision {
        return Err("source read cut expired".into());
    }
    let (total, kind) = match name {
        EffectiveFieldName::SourceRevisions => (source.source_revisions.len(), "map"),
        EffectiveFieldName::Provenance => (source.provenance.len(), "map"),
        EffectiveFieldName::AvailableTools => (source.available_tools.len(), "array"),
        _ => return Err("configuration field is not a pageable collection".into()),
    };
    let total = u64::try_from(total).map_err(|e| e.to_string())?;
    if offset > total {
        return Err("configuration page out of range".into());
    }
    let end = offset
        .checked_add(u64::try_from(limit).map_err(|e| e.to_string())?)
        .ok_or("configuration page overflow")?
        .min(total);
    let mut items = Vec::with_capacity(limit);
    for index in offset..end {
        let (bytes, total) = item(source, name, index, 0, INLINE_BYTES)?;
        let inline_value = (total <= INLINE_BYTES as u64)
            .then(|| serde_json::from_slice::<serde_json::Value>(&bytes))
            .transpose()
            .map_err(|error| error.to_string())?;
        let (value, reference) = if inline_value.as_ref().is_some_and(count_bounded) {
            (inline_value, None)
        } else {
            (
                None,
                Some(EffectiveFieldRef {
                    revision: revision.into(),
                    field: name,
                    item_index: Some(index),
                    byte_length: total,
                }),
            )
        };
        items.push(EffectivePageItem {
            index,
            value,
            reference,
        });
    }
    Ok(EffectivePage {
        revision: revision.into(),
        field: name,
        kind: kind.into(),
        total,
        items,
        next_offset: (end < total).then_some(end),
    })
}
