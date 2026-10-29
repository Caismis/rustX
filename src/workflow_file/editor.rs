//! Typed, bounded presentation data. No renderer or runtime state is persisted.
use crate::runtime::workflow::authoring::{WorkflowBlockStep, WorkflowNodeAddress, block_at};
use crate::runtime::workflow::{WorkflowDefinition, WorkflowNodeDefinition};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_EDITOR_BYTES: usize = 256 * 1024;
pub const MAX_PRESENTATION_DIAGNOSTICS: usize = 64;
pub const MAX_EDITOR_ENTRIES: usize = 1024;
pub const MAX_EDITOR_NOTES: usize = 128;
pub const MAX_EDITOR_COORDINATE: f64 = 1_000_000.0;
pub const MAX_EDITOR_DIMENSION: f64 = 8192.0;
pub const MAX_DISPLAY_TITLE_BYTES: usize = 128;
pub const MAX_DISPLAY_DESCRIPTION_BYTES: usize = 1024;
pub const MAX_DISPLAY_ICON_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPresentationDiagnostic {
    pub path: String,
    pub category: String,
    pub reason: String,
}
impl WorkflowPresentationDiagnostic {
    pub(super) fn new(path: &str, category: &str, reason: &str) -> Self {
        Self {
            path: path.into(),
            category: category.into(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDimensions {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNodePresentation {
    pub address: WorkflowNodeAddress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<WorkflowPosition>,
    /// Authored Loop container geometry, never measured DOM state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<WorkflowDimensions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNoteTheme {
    Neutral,
    Blue,
    Green,
    Amber,
    Rose,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNote {
    pub id: String,
    pub block: Vec<WorkflowBlockStep>,
    pub text: String,
    pub theme: WorkflowNoteTheme,
    pub position: WorkflowPosition,
    pub dimensions: WorkflowDimensions,
}

/// An exact position in a schema, independent of value-reference semantics.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowSchemaStep {
    Property { name: String },
    Items,
}

/// Presentation order targets a specific authored list. Schema `path` contains
/// property names (not JSON pointers); missing/new fields append in key order.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowOrderTarget {
    BlockInput {
        block: Vec<WorkflowBlockStep>,
        path: Vec<WorkflowSchemaStep>,
    },
    BlockOutput {
        block: Vec<WorkflowBlockStep>,
        path: Vec<WorkflowSchemaStep>,
    },
    NodeOutput {
        address: WorkflowNodeAddress,
        path: Vec<WorkflowSchemaStep>,
    },
    NodeInput {
        address: WorkflowNodeAddress,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFieldOrder {
    pub target: WorkflowOrderTarget,
    pub fields: Vec<String>,
}

/// How people see the Workflow in the library and editor. Never part of the
/// program: models only read the definition's own `description`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDisplay {
    /// The human name; the file id remains the native identity.
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// An Iconify icon name, `<prefix>:<name>` (e.g. `lucide:workflow`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

fn icon_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('-')
        && !part.ends_with('-')
        && !part.contains("--")
        && part
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl WorkflowDisplay {
    pub(super) fn valid(&self) -> bool {
        !self.title.trim().is_empty()
            && self.title.len() <= MAX_DISPLAY_TITLE_BYTES
            && self
                .description
                .as_ref()
                .is_none_or(|text| text.len() <= MAX_DISPLAY_DESCRIPTION_BYTES)
            && self.icon.as_ref().is_none_or(|icon| {
                icon.len() <= MAX_DISPLAY_ICON_BYTES
                    && icon
                        .split_once(':')
                        .is_some_and(|(prefix, name)| icon_part(prefix) && icon_part(name))
            })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEditorMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<WorkflowDisplay>,
    #[serde(default)]
    pub nodes: Vec<WorkflowNodePresentation>,
    #[serde(default)]
    pub notes: Vec<WorkflowNote>,
    #[serde(default)]
    pub orders: Vec<WorkflowFieldOrder>,
}

fn key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn block_valid(block: &[WorkflowBlockStep]) -> bool {
    block.len() <= crate::runtime::workflow::MAX_BLOCK_DEPTH
        && block.iter().all(|step| key(&step.node))
}
fn address_valid(address: &WorkflowNodeAddress) -> bool {
    block_valid(&address.block) && key(&address.node)
}
fn position_valid(position: WorkflowPosition) -> bool {
    [position.x, position.y]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= MAX_EDITOR_COORDINATE)
}
fn dimensions_valid(size: WorkflowDimensions) -> bool {
    [size.width, size.height]
        .iter()
        .all(|v| v.is_finite() && (16.0..=MAX_EDITOR_DIMENSION).contains(v))
}
fn unique<T: Ord>(values: impl Iterator<Item = T>) -> bool {
    let mut seen = BTreeSet::new();
    values.into_iter().all(|value| seen.insert(value))
}

impl WorkflowEditorMetadata {
    pub(super) fn validate(&self) -> Result<(), WorkflowPresentationDiagnostic> {
        let invalid =
            |reason| WorkflowPresentationDiagnostic::new("editor", "presentation", reason);
        if self.nodes.len() + self.notes.len() + self.orders.len() > MAX_EDITOR_ENTRIES
            || self.notes.len() > MAX_EDITOR_NOTES
        {
            return Err(invalid("editor exceeds 1024 entries or 128 notes"));
        }
        if self
            .display
            .as_ref()
            .is_some_and(|display| !display.valid())
        {
            return Err(WorkflowPresentationDiagnostic::new(
                "editor.display",
                "presentation",
                "display needs a non-empty title within 128 bytes, a description within 1024 bytes and an Iconify `<prefix>:<name>` icon",
            ));
        }
        if self.nodes.iter().any(|node| {
            !address_valid(&node.address)
                || node.title.as_ref().is_some_and(|s| s.len() > 256)
                || node.description.as_ref().is_some_and(|s| s.len() > 4096)
                || node.position.is_some_and(|p| !position_valid(p))
                || node.dimensions.is_some_and(|d| !dimensions_valid(d))
        }) || self.notes.iter().any(|note| {
            !block_valid(&note.block)
                || !key(&note.id)
                || note.text.len() > 16 * 1024
                || !position_valid(note.position)
                || !dimensions_valid(note.dimensions)
        }) {
            return Err(invalid(
                "invalid editor address, UTF-8 text bound or finite geometry (coordinates ±1000000; dimensions 16..8192)",
            ));
        }
        for order in &self.orders {
            let (valid, path) = match &order.target {
                WorkflowOrderTarget::BlockInput { block, path }
                | WorkflowOrderTarget::BlockOutput { block, path } => {
                    (block_valid(block), path.as_slice())
                }
                WorkflowOrderTarget::NodeOutput { address, path } => {
                    (address_valid(address), path.as_slice())
                }
                WorkflowOrderTarget::NodeInput { address } => (address_valid(address), &[][..]),
            };
            if !valid
                || path.len() > 32
                || path.iter().any(|step| matches!(step, WorkflowSchemaStep::Property { name } if name.is_empty() || name.len() > 64))
                || order.fields.len() > 256
                || order.fields.iter().any(|s| s.is_empty() || s.len() > 64)
                || !unique(order.fields.iter())
            {
                return Err(invalid(
                    "invalid field order address, count or duplicate field",
                ));
            }
        }
        if !unique(self.nodes.iter().map(|n| &n.address))
            || !unique(self.notes.iter().map(|n| &n.id))
            || !unique(self.orders.iter().map(|o| &o.target))
        {
            return Err(invalid("duplicate presentation identity"));
        }
        if serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > MAX_EDITOR_BYTES) {
            return Err(invalid("editor exceeds 256 KiB"));
        }
        Ok(())
    }

    pub(super) fn diagnostics(
        &self,
        definition: &WorkflowDefinition,
    ) -> Vec<WorkflowPresentationDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut add = |path: String| {
            if diagnostics.len() < MAX_PRESENTATION_DIAGNOSTICS {
                diagnostics.push(WorkflowPresentationDiagnostic::new(&path, "presentation_reference", "presentation target no longer exists or has changed kind; content is retained; repair or remove this entry"));
            } else if let Some(last) = diagnostics.last_mut() {
                *last = WorkflowPresentationDiagnostic::new(
                    "editor",
                    "presentation_truncated",
                    "additional orphaned presentation entries are omitted at the 64-diagnostic bound; all authored content is retained",
                );
            }
        };
        for (index, node) in self.nodes.iter().enumerate() {
            let target = node_at(definition, &node.address);
            if target.is_none()
                || (node.dimensions.is_some()
                    && !matches!(target, Some(WorkflowNodeDefinition::Loop { .. })))
            {
                add(format!("editor.nodes.{index}"));
            }
        }
        for (index, note) in self.notes.iter().enumerate() {
            if block_at(&definition.block, &note.block).is_none() {
                add(format!("editor.notes.{index}"));
            }
        }
        for (index, order) in self.orders.iter().enumerate() {
            let fields = order_fields(definition, &order.target);
            if fields
                .as_ref()
                .is_none_or(|fields| order.fields.iter().any(|f| !fields.contains(f)))
            {
                add(format!("editor.orders.{index}"));
            }
        }
        diagnostics
    }
}

fn node_at<'a>(
    definition: &'a WorkflowDefinition,
    address: &WorkflowNodeAddress,
) -> Option<&'a WorkflowNodeDefinition> {
    block_at(&definition.block, &address.block)?
        .nodes
        .get(&address.node)
}

fn order_fields(
    definition: &WorkflowDefinition,
    target: &WorkflowOrderTarget,
) -> Option<BTreeSet<String>> {
    use WorkflowOrderTarget::{BlockInput, BlockOutput, NodeInput, NodeOutput};
    let schema = match target {
        NodeInput { address } => {
            return match node_at(definition, address)? {
                WorkflowNodeDefinition::Agent { input, .. } => {
                    Some(input.keys().cloned().collect())
                }
                _ => None,
            };
        }
        BlockInput { block, .. } => block_at(&definition.block, block)?.input.clone(),
        BlockOutput { block, .. } => block_at(&definition.block, block)?.output.clone(),
        NodeOutput { address, .. } => {
            crate::runtime::workflow::authoring::declared_output(node_at(definition, address)?)
                .ok()??
        }
    };
    let (BlockInput { path, .. } | BlockOutput { path, .. } | NodeOutput { path, .. }) = target
    else {
        unreachable!()
    };
    let mut schema = &schema;
    for field in path {
        schema = match field {
            WorkflowSchemaStep::Property { name } => schema.get("properties")?.get(name)?,
            WorkflowSchemaStep::Items => schema.get("items")?,
        };
    }
    Some(
        schema
            .get("properties")?
            .as_object()?
            .keys()
            .cloned()
            .collect(),
    )
}
