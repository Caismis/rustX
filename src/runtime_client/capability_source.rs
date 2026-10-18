//! Borrowed read evidence for one immutable native capability generation.
//! Preparation owns bounded display headers; original definitions stay in the registry.
use super::snapshot::{
    CapabilitySourceView, CapabilityView, RuntimeClientSkill, RuntimeClientTool,
};
use crate::capabilities::{CapabilityAvailability, CapabilitySnapshot};
use serde::{
    Serialize,
    ser::{SerializeSeq, SerializeStruct},
};
use std::sync::{Arc, OnceLock};

mod prepared;
pub(crate) use prepared::PreparedCapabilityHead;

#[derive(Debug, Clone)]
pub(crate) struct CapabilityReadSource {
    snapshot: Arc<CapabilitySnapshot>,
    availability: CapabilityAvailability,
    model: Option<crate::model::catalog::ModelCapabilities>,
    prepared: Arc<OnceLock<PreparedCapabilityHead>>,
}
impl CapabilityReadSource {
    pub(crate) fn new(
        snapshot: Arc<CapabilitySnapshot>,
        availability: CapabilityAvailability,
        model: Option<crate::model::catalog::ModelCapabilities>,
    ) -> Self {
        Self {
            snapshot,
            availability,
            model,
            prepared: Arc::default(),
        }
    }
    pub(crate) fn with_model(
        &self,
        model: Option<crate::model::catalog::ModelCapabilities>,
    ) -> Self {
        Self {
            snapshot: Arc::clone(&self.snapshot),
            availability: self.availability.clone(),
            model,
            prepared: Arc::default(),
        }
    }
    pub(crate) fn revision(&self) -> crate::runtime::identity::CapabilityRevision {
        self.snapshot.revision()
    }
    fn allowed(&self, tool: &crate::tools::types::ToolDefinition) -> bool {
        self.model
            .as_ref()
            .map_or(tool.id.as_str() != "tool-read-image", |model| {
                crate::tools::executor::model_allows_tool(&tool.id, model)
            })
    }
    fn excluded_tool(&self) -> Option<usize> {
        let id = crate::runtime::identity::ToolId::new("tool-read-image");
        let ordinal = self.snapshot.tool_registry().definition_ordinal(&id)?;
        (!self.allowed(self.snapshot.tool_registry().definition_at(ordinal)?)).then_some(ordinal)
    }
    pub(crate) fn field_len(&self, field: &str) -> Option<usize> {
        Some(match field {
            "configured_tools" => self.snapshot.tool_registry().definition_refs().len(),
            "tools" => {
                self.snapshot.tool_registry().definition_refs().len()
                    - usize::from(self.excluded_tool().is_some())
            }
            "available_tools" => self.snapshot.available_tools().definition_refs().len(),
            "skills" => self
                .snapshot
                .model_skill_entries()
                .len()
                .min(self.snapshot.skills().visible_bindings().len()),
            "sources" => self.availability.len(),
            _ => return None,
        })
    }
    pub(crate) fn row_at(&self, field: &str, ordinal: u64) -> Option<CapabilityRowReadView<'_>> {
        let ordinal = usize::try_from(ordinal).ok()?;
        Some(match field {
            "configured_tools" => {
                CapabilityRowReadView::Tool(self.snapshot.tool_registry().definition_at(ordinal)?)
            }
            "tools" => {
                let index = ordinal
                    + usize::from(
                        self.excluded_tool()
                            .is_some_and(|excluded| excluded <= ordinal),
                    );
                CapabilityRowReadView::Tool(self.snapshot.tool_registry().definition_at(index)?)
            }
            "available_tools" => {
                CapabilityRowReadView::Tool(self.snapshot.available_tools().definition_at(ordinal)?)
            }
            "skills" => {
                let entry = self.snapshot.model_skill_entries().get(ordinal)?;
                let binding = self.snapshot.skills().visible_bindings().get(ordinal)?;
                CapabilityRowReadView::Skill(SkillReadView {
                    id: &binding.skill_id,
                    version_id: &binding.version_id,
                    name: &entry.name,
                    description: &entry.description,
                    location: &entry.location,
                })
            }
            "sources" => {
                let (source, state) = self.availability.at(ordinal)?;
                CapabilityRowReadView::Source(source_view(source, state))
            }
            _ => return None,
        })
    }
    pub(crate) fn selected_text(&self, field: &str, ordinal: u64, path: &[String]) -> Option<&str> {
        match self.row_at(field, ordinal)? {
            CapabilityRowReadView::Tool(tool) => match path {
                [key] if key == "id" => Some(tool.id.as_str()),
                [key] if key == "name" => Some(&tool.name),
                [key] if key == "description" => Some(&tool.description),
                [key, rest @ ..] if key == "input_schema" => {
                    let mut value = &tool.input_schema;
                    for component in rest {
                        value = if let Some(index) = component.strip_prefix('@') {
                            value.as_array()?.get(index.parse::<usize>().ok()?)?
                        } else {
                            value.as_object()?.get(component)?
                        };
                    }
                    value.as_str()
                }
                _ => None,
            },
            CapabilityRowReadView::Skill(skill) => match path {
                [key] => match key.as_str() {
                    "id" => Some(skill.id.as_str()),
                    "version_id" => Some(skill.version_id.as_str()),
                    "name" => Some(skill.name),
                    "description" => Some(skill.description),
                    "location" => Some(skill.location),
                    _ => None,
                },
                _ => None,
            },
            CapabilityRowReadView::Source(source) => match (path, source.source) {
                ([parent, key], SourceDescriptor::ManagedPython { package })
                    if parent == "source" && key == "package" =>
                {
                    Some(package)
                }
                ([parent, key], SourceDescriptor::Mcp { server_id })
                    if parent == "source" && key == "server_id" =>
                {
                    Some(server_id.as_str())
                }
                _ => None,
            },
        }
    }
    pub(crate) fn prepare(&self) {
        self.prepared.get_or_init(|| {
            fn header<T: Serialize, U: serde::de::DeserializeOwned>(
                domain: &str,
                values: impl Iterator<Item = T>,
                total: usize,
                frontiers: &mut Vec<super::checkpoint::CollectionFrontier>,
            ) -> Vec<U> {
                let mut scanned = 0;
                let mut omitted = false;
                let result = values
                    .take(32)
                    .filter_map(|value| {
                        scanned += 1;
                        let bytes = super::checkpoint::bounded_item_bytes(&value, 2048);
                        let item = bytes.and_then(|bytes| serde_json::from_slice(&bytes).ok());
                        omitted |= item.is_none();
                        item
                    })
                    .collect::<Vec<_>>();
                frontiers.push(super::checkpoint::CollectionFrontier {
                    domain: domain.into(),
                    revision: 0,
                    total: total as u64,
                    loaded: result.len() as u64,
                    next_offset: if omitted {
                        Some(0)
                    } else {
                        (scanned < total).then_some(scanned as u64)
                    },
                });
                result
            }
            let mut frontiers = Vec::with_capacity(5);
            let configured_tools: Vec<RuntimeClientTool> = header(
                "capabilities.configured_tools",
                self.snapshot.tool_registry().definition_refs(),
                self.snapshot.tool_registry().definition_refs().len(),
                &mut frontiers,
            );
            let tools = header(
                "capabilities.tools",
                self.snapshot
                    .tool_registry()
                    .definition_refs()
                    .filter(|tool| self.allowed(tool)),
                self.field_len("tools").expect("native tools field"),
                &mut frontiers,
            );
            let available_tools = header(
                "capabilities.available_tools",
                self.snapshot.available_tools().definition_refs(),
                self.snapshot.available_tools().definition_refs().len(),
                &mut frontiers,
            );
            let skills: Vec<RuntimeClientSkill> = header(
                "capabilities.skills",
                self.skill_views(),
                self.snapshot.model_skill_entries().len(),
                &mut frontiers,
            );
            let sources: Vec<CapabilitySourceView> = header(
                "capabilities.sources",
                self.source_views(),
                self.availability.len(),
                &mut frontiers,
            );
            PreparedCapabilityHead::new(
                &CapabilityView {
                    revision: self.snapshot.revision(),
                    configured_tools,
                    tools,
                    available_tools,
                    skills,
                    sources,
                },
                frontiers,
            )
        });
    }
    pub(crate) fn header(&self) -> CapabilityView {
        self.prepared
            .get()
            .expect("capability header prepared outside Projection")
            .header()
    }
    pub(crate) fn frontiers(&self) -> Vec<super::checkpoint::CollectionFrontier> {
        self.prepared_head().frontiers()
    }
    pub(crate) fn prepared_head(&self) -> PreparedCapabilityHead {
        self.prepared.get().expect("source prepared").clone()
    }
    fn skill_views(&self) -> impl Iterator<Item = SkillReadView<'_>> {
        self.snapshot
            .model_skill_entries()
            .iter()
            .zip(self.snapshot.skills().visible_bindings())
            .map(|(entry, binding)| SkillReadView {
                id: &binding.skill_id,
                version_id: &binding.version_id,
                name: &entry.name,
                description: &entry.description,
                location: &entry.location,
            })
    }
    fn source_views(&self) -> impl Iterator<Item = SourceReadView<'_>> {
        self.availability
            .iter()
            .map(|(source, state)| source_view(source, state))
    }
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum CapabilityRowReadView<'a> {
    Tool(&'a crate::tools::types::ToolDefinition),
    Skill(SkillReadView<'a>),
    Source(SourceReadView<'a>),
}
impl schemars::JsonSchema for CapabilityRowReadView<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CapabilityRowReadView".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        #[derive(schemars::JsonSchema)]
        #[serde(untagged)]
        #[allow(dead_code)]
        enum Row {
            Tool(RuntimeClientTool),
            Skill(RuntimeClientSkill),
            Source(CapabilitySourceView),
        }
        generator.subschema_for::<Row>()
    }
}
fn source_view<'a>(
    source: &'a crate::capabilities::ToolSourceId,
    state: &crate::capabilities::CapabilitySourceState,
) -> SourceReadView<'a> {
    SourceReadView {
        source: match source {
            crate::capabilities::ToolSourceId::ManagedPython(package) => {
                SourceDescriptor::ManagedPython { package }
            }
            crate::capabilities::ToolSourceId::Mcp(server_id) => {
                SourceDescriptor::Mcp { server_id }
            }
        },
        state: match state {
            crate::capabilities::CapabilitySourceState::Unprepared => {
                super::snapshot::CapabilitySourceStateView::Unprepared
            }
            crate::capabilities::CapabilitySourceState::Ready => {
                super::snapshot::CapabilitySourceStateView::Ready
            }
            crate::capabilities::CapabilitySourceState::Unavailable { .. } => {
                super::snapshot::CapabilitySourceStateView::Unavailable
            }
        },
    }
}
#[derive(Serialize)]
pub(crate) struct SkillReadView<'a> {
    id: &'a crate::runtime::identity::SkillId,
    version_id: &'a crate::runtime::identity::SkillVersionId,
    name: &'a str,
    description: &'a str,
    location: &'a str,
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SourceDescriptor<'a> {
    ManagedPython {
        package: &'a str,
    },
    Mcp {
        server_id: &'a crate::runtime::identity::McpServerId,
    },
}
#[derive(Serialize)]
pub(crate) struct SourceReadView<'a> {
    source: SourceDescriptor<'a>,
    state: super::snapshot::CapabilitySourceStateView,
}
// Opaque iterator types need not be Clone: repeat the native iterator only while serializing.
impl Serialize for CapabilityReadSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        struct Tools<'a>(&'a CapabilityReadSource, u8);
        impl Serialize for Tools<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut seq = serializer.serialize_seq(None)?;
                match self.1 {
                    2 => {
                        for value in self.0.snapshot.available_tools().definition_refs() {
                            seq.serialize_element(value)?;
                        }
                    }
                    _ => {
                        for value in self.0.snapshot.tool_registry().definition_refs() {
                            if self.1 == 0 || self.0.allowed(value) {
                                seq.serialize_element(value)?;
                            }
                        }
                    }
                }
                seq.end()
            }
        }
        struct Skills<'a>(&'a CapabilityReadSource);
        impl Serialize for Skills<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut seq = serializer.serialize_seq(None)?;
                for value in self.0.skill_views() {
                    seq.serialize_element(&value)?;
                }
                seq.end()
            }
        }
        struct Sources<'a>(&'a CapabilityReadSource);
        impl Serialize for Sources<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut seq = serializer.serialize_seq(None)?;
                for value in self.0.source_views() {
                    seq.serialize_element(&value)?;
                }
                seq.end()
            }
        }
        let mut view = serializer.serialize_struct("CapabilityView", 6)?;
        view.serialize_field("configured_tools", &Tools(self, 0))?;
        view.serialize_field("revision", &self.snapshot.revision())?;
        view.serialize_field("tools", &Tools(self, 1))?;
        view.serialize_field("available_tools", &Tools(self, 2))?;
        view.serialize_field("skills", &Skills(self))?;
        view.serialize_field("sources", &Sources(self))?;
        view.end()
    }
}
impl schemars::JsonSchema for CapabilityReadSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CapabilityReadSource".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<CapabilityView>()
    }
}
