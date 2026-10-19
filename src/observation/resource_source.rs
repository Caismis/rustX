//! Identity-preserving resource display source for one native generation.
use crate::runtime::identity::RuntimeResourceRevision;
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
#[derive(Clone, Debug)]
pub(crate) struct ResourceReadSource {
    source: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
    header: Arc<
        OnceLock<(
            RuntimeClientResourcesView,
            Vec<super::cut::CollectionFrontier>,
        )>,
    >,
}
impl ResourceReadSource {
    pub(crate) fn new(source: Arc<crate::runtime::resources::RuntimeResourceSnapshot>) -> Self {
        Self {
            source,
            header: Arc::default(),
        }
    }
    pub(crate) fn prepare(&self) {
        self.header.get_or_init(|| {
            // The original inspection contains growing maps with source-owned
            // key serializers. Never scan or allocate their complete encoding
            // merely to decide whether a display preview fits.
            let inspection: Option<crate::runtime::capability_inspection::CapabilityInspection> =
                None;
            let mut omitted = false;
            let context_files = self
                .source
                .project_context_files()
                .iter()
                .take(32)
                .filter_map(|file| {
                    let value = ContextFileRead {
                        path: &file.path,
                        bytes: file.content.len() as u64,
                    };
                    let result =
                        super::bounded::bounded_item_bytes(&value, 2048).and_then(|bytes| {
                            serde_json::from_slice::<RuntimeClientContextFile>(&bytes).ok()
                        });
                    omitted |= result.is_none();
                    result
                })
                .collect::<Vec<_>>();
            let frontiers = vec![
                super::cut::CollectionFrontier {
                    domain: "resources.inspection".into(),
                    revision: 0,
                    total: 1,
                    loaded: u64::from(inspection.is_some()),
                    next_offset: inspection.is_none().then_some(0),
                },
                super::cut::CollectionFrontier {
                    domain: "resources.context_files".into(),
                    revision: 0,
                    total: self.source.project_context_files().len() as u64,
                    loaded: context_files.len() as u64,
                    next_offset: if omitted {
                        Some(0)
                    } else {
                        (context_files.len() < self.source.project_context_files().len())
                            .then_some(context_files.len() as u64)
                    },
                },
            ];
            (
                RuntimeClientResourcesView {
                    inspection: inspection.map(Box::new),
                    revision: self.source.revision(),
                    context_files,
                    agent_profile: self.source.agent_profile().is_some(),
                },
                frontiers,
            )
        });
    }
    pub(crate) fn header(&self) -> RuntimeClientResourcesView {
        self.header
            .get()
            .expect("resource source prepared outside Projection")
            .0
            .clone()
    }
    pub(crate) fn frontiers(&self) -> Vec<super::cut::CollectionFrontier> {
        self.header.get().expect("source prepared").1.clone()
    }
}
#[derive(Serialize)]
pub(crate) struct ContextFileRead<'a> {
    path: &'a std::path::Path,
    bytes: u64,
}
impl Serialize for ResourceReadSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        struct Files<'a>(&'a crate::runtime::resources::RuntimeResourceSnapshot);
        impl Serialize for Files<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut seq =
                    serializer.serialize_seq(Some(self.0.project_context_files().len()))?;
                for file in self.0.project_context_files() {
                    seq.serialize_element(&ContextFileRead {
                        path: &file.path,
                        bytes: file.content.len() as u64,
                    })?;
                }
                seq.end()
            }
        }
        let mut view = serializer.serialize_struct("RuntimeClientResourcesView", 4)?;
        view.serialize_field("inspection", self.source.inspection())?;
        view.serialize_field("revision", &self.source.revision())?;
        view.serialize_field("context_files", &Files(&self.source))?;
        view.serialize_field("agent_profile", &self.source.agent_profile().is_some())?;
        view.end()
    }
}
impl schemars::JsonSchema for ResourceReadSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ResourceReadSource".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<RuntimeClientResourcesView>()
    }
}

/// Borrowed selection from the existing immutable resource owner. Containers
/// are paged directly; creating a page never serializes their complete body.
#[derive(Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub(crate) enum ResourceNode<'a> {
    Inspection(&'a crate::runtime::capability_inspection::CapabilityInspection),
    Agent(&'a crate::runtime::capability_inspection::AgentInspection),
    Definitions(&'a [crate::runtime::capability_inspection::ResourceDefinition]),
    Definition(&'a crate::runtime::capability_inspection::ResourceDefinition),
    Diagnostics(&'a [crate::runtime::capability_inspection::ResourceDiagnostic]),
    Diagnostic(&'a crate::runtime::capability_inspection::ResourceDiagnostic),
    Tools(&'a [crate::runtime::capability_inspection::ToolInspection]),
    Tool(&'a crate::runtime::capability_inspection::ToolInspection),
    Selections(&'a [crate::capabilities::selection::AgentToolSelection]),
    Selection(&'a crate::capabilities::selection::AgentToolSelection),
    Skills(&'a [crate::skills::SkillProvenance]),
    Skill(&'a crate::skills::SkillProvenance),
    ShadowedSkills(&'a [crate::skills::ShadowedSkill]),
    ShadowedSkill(&'a crate::skills::ShadowedSkill),
    SkillSource(&'a crate::skills::SkillSource),
    ToolOrigin(&'a crate::tools::types::ToolOrigin),
    Family(crate::runtime::capability_inspection::ResourceFamily),
    Location(&'a crate::runtime::resources::ResourceLocation),
    Subject(&'a crate::runtime::capability_inspection::ResourceDiagnosticSubject),
    Boolean(bool),
    SkillDiagnostics(&'a [crate::skills::SkillDiagnostic]),
    SkillDiagnostic(&'a crate::skills::SkillDiagnostic),
    AgentNames(&'a [crate::runtime::subagent::SubagentName]),
    AgentName(&'a crate::runtime::subagent::SubagentName),
    WorkflowNames(&'a [crate::runtime::workflow::WorkflowId]),
    WorkflowName(&'a crate::runtime::workflow::WorkflowId),
    Extensions(&'a [crate::runtime::capability_inspection::ExtensionInspection]),
    Extension(&'a crate::runtime::capability_inspection::ExtensionInspection),
    AgentDiagnostics(&'a [crate::runtime::agent_profile::AgentProfileDiagnostic]),
    AgentDiagnostic(&'a crate::runtime::agent_profile::AgentProfileDiagnostic),
    Agents(
        &'a std::collections::BTreeMap<
            crate::runtime::subagent::SubagentName,
            crate::runtime::capability_inspection::AgentInspection,
        >,
    ),
    Workflows(
        &'a std::collections::BTreeMap<
            crate::runtime::workflow::WorkflowId,
            crate::runtime::capability_inspection::WorkflowInspection,
        >,
    ),
    Sources(
        &'a std::collections::BTreeMap<
            crate::capabilities::ToolSourceId,
            crate::runtime::capability_inspection::SourceInspection,
        >,
    ),
    Workflow(&'a crate::runtime::capability_inspection::WorkflowInspection),
    WorkflowDiagnostics(&'a [crate::runtime::workflow::WorkflowAdmissionDiagnostic]),
    WorkflowDiagnostic(&'a crate::runtime::workflow::WorkflowAdmissionDiagnostic),
    Source(&'a crate::runtime::capability_inspection::SourceInspection),
    SourceEntry(ResourceSourceEntry<'a>),
    SourceIdentity(ResourceSourceIdentity<'a>),
    Text(&'a str),
    Path(&'a std::path::Path),
    Identity(&'a crate::runtime::capability_inspection::AgentIdentity),
    Null(()),
}
impl ResourceNode<'_> {
    pub(crate) fn children(&self) -> Option<usize> {
        Some(match self {
            Self::Inspection(_) => 8,
            Self::Agent(_) => 10,
            Self::Tool(_) | Self::Location(_) => 3,
            Self::Skill(_) | Self::Definition(_) | Self::Diagnostic(_) => 4,
            Self::ShadowedSkill(_) | Self::SourceEntry(_) | Self::SourceIdentity(_) => 2,
            Self::Workflow(v) => match v {
                crate::runtime::capability_inspection::WorkflowInspection::Enabled => 1,
                crate::runtime::capability_inspection::WorkflowInspection::Disabled(_) => 2,
            },
            Self::WorkflowDiagnostics(v) => v.len(),
            Self::ToolOrigin(v) => match v {
                crate::tools::types::ToolOrigin::Builtin => 0,
                _ => 1,
            },
            Self::Definitions(v) => v.len(),
            Self::Diagnostics(v) => v.len(),
            Self::Tools(v) => v.len(),
            Self::Selections(v) => v.len(),
            Self::Skills(v) => v.len(),
            Self::ShadowedSkills(v) => v.len(),
            Self::SkillDiagnostics(v) => v.len(),
            Self::AgentNames(v) => v.len(),
            Self::WorkflowNames(v) => v.len(),
            Self::Extensions(v) => v.len(),
            Self::AgentDiagnostics(v) => v.len(),
            Self::Agents(v) => v.len(),
            Self::Workflows(v) => v.len(),
            Self::Sources(v) => v.len(),
            _ => return None,
        })
    }
    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(v) => Some(v),
            Self::Path(v) => v.to_str(),
            Self::AgentName(v) => Some(v.as_str()),
            Self::WorkflowName(v) => Some(v.as_str()),
            _ => None,
        }
    }
}
impl ResourceReadSource {
    pub(crate) fn node<'a>(&'a self, path: &[String]) -> Option<ResourceNode<'a>> {
        if path.first()?.as_str() != "inspection" {
            return None;
        }
        let mut selected = ResourceNode::Inspection(self.source.inspection());
        for field in &path[1..] {
            let position = field
                .strip_prefix('@')
                .and_then(|value| value.parse::<usize>().ok());
            selected = if let Some(position) = position {
                self.child(&selected, position)?.1
            } else {
                self.named(&selected, field)?
            };
        }
        Some(selected)
    }
    fn named<'a>(&'a self, node: &ResourceNode<'a>, name: &str) -> Option<ResourceNode<'a>> {
        let position = match node {
            ResourceNode::Inspection(_) => [
                "definitions",
                "resource_diagnostics",
                "main",
                "agents",
                "workflows",
                "sources",
                "skills",
                "skill_diagnostics",
            ]
            .iter()
            .position(|field| *field == name),
            ResourceNode::Agent(_) => [
                "identity",
                "source",
                "tools",
                "agent_controls",
                "tool_selection",
                "skills",
                "agents",
                "workflows",
                "plugins",
                "diagnostics",
            ]
            .iter()
            .position(|field| *field == name),
            ResourceNode::SourceEntry(_) => ["identity", "state"]
                .iter()
                .position(|field| *field == name),
            ResourceNode::SourceIdentity(value) => [
                "type",
                match value {
                    ResourceSourceIdentity::Mcp { .. } => "server_id",
                    ResourceSourceIdentity::ManagedPython { .. } => "package",
                },
            ]
            .iter()
            .position(|field| *field == name),
            ResourceNode::Tool(_) => ["id", "name", "origin"]
                .iter()
                .position(|field| *field == name),
            ResourceNode::Skill(_) => ["name", "source", "location", "shadowed"]
                .iter()
                .position(|field| *field == name),
            ResourceNode::ShadowedSkill(_) => ["source", "location"]
                .iter()
                .position(|field| *field == name),
            ResourceNode::Definition(_) => ["family", "name", "location", "valid"]
                .iter()
                .position(|field| *field == name),
            ResourceNode::Diagnostic(_) => ["subject", "file", "field", "reason"]
                .iter()
                .position(|field| *field == name),
            ResourceNode::Location(_) => ["scope", "path", "shadowed"]
                .iter()
                .position(|field| *field == name),
            ResourceNode::Workflow(_) => ["status", "diagnostics"]
                .iter()
                .position(|field| *field == name),
            ResourceNode::ToolOrigin(value) => match (value, name) {
                (crate::tools::types::ToolOrigin::Mcp { .. }, "server_id")
                | (crate::tools::types::ToolOrigin::ManagedPython { .. }, "package") => Some(0),
                _ => None,
            },
            _ => None,
        };
        if let Some(position) = position {
            return Some(self.child(node, position)?.1);
        }
        match node {
            ResourceNode::Definition(value) if name == "name" => {
                Some(ResourceNode::Text(&value.name))
            }
            ResourceNode::Diagnostic(value) if name == "field" => {
                Some(ResourceNode::Text(&value.field))
            }
            ResourceNode::Diagnostic(value) if name == "reason" => {
                Some(ResourceNode::Text(&value.reason))
            }
            ResourceNode::Tool(value) if name == "name" => Some(ResourceNode::Text(&value.name)),
            ResourceNode::Tool(value) if name == "id" => {
                Some(ResourceNode::Text(value.id.as_str()))
            }
            ResourceNode::Skill(value) if name == "name" => Some(ResourceNode::Text(&value.name)),
            ResourceNode::Skill(value) if name == "location" => {
                Some(ResourceNode::Text(&value.location))
            }
            _ => None,
        }
    }
    pub(crate) fn child<'a>(
        &'a self,
        node: &ResourceNode<'a>,
        position: usize,
    ) -> Option<(Option<&'a str>, ResourceNode<'a>)> {
        use ResourceNode as Node;
        macro_rules! item {
            ($values:expr,$variant:ident) => {
                Some((None, Node::$variant($values.get(position)?)))
            };
        }
        match node {
            Node::Inspection(value) => inspection_child(value, position),
            Node::Agent(value) => agent_child(value, position),
            Node::Tool(_)
            | Node::Skill(_)
            | Node::ShadowedSkill(_)
            | Node::Definition(_)
            | Node::Diagnostic(_)
            | Node::Location(_)
            | Node::Workflow(_)
            | Node::WorkflowDiagnostics(_)
            | Node::ToolOrigin(_) => metadata_child(node, position),
            Node::ShadowedSkills(v) => item!(v, ShadowedSkill),
            Node::Definitions(v) => item!(v, Definition),
            Node::Diagnostics(v) => item!(v, Diagnostic),
            Node::Tools(v) => item!(v, Tool),
            Node::Selections(v) => item!(v, Selection),
            Node::Skills(v) => item!(v, Skill),
            Node::SkillDiagnostics(v) => item!(v, SkillDiagnostic),
            Node::AgentNames(v) => item!(v, AgentName),
            Node::WorkflowNames(v) => item!(v, WorkflowName),
            Node::Extensions(v) => item!(v, Extension),
            Node::AgentDiagnostics(v) => item!(v, AgentDiagnostic),
            Node::Agents(_) => {
                let (key, value) = self.source.inspection_agent_at(position)?;
                Some((Some(key.as_str()), Node::Agent(value)))
            }
            Node::Workflows(_) => {
                let (key, value) = self.source.inspection_workflow_at(position)?;
                Some((Some(key.as_str()), Node::Workflow(value)))
            }
            Node::Sources(_) | Node::SourceEntry(_) | Node::SourceIdentity(_) => {
                self.source_child(node, position)
            }
            _ => None,
        }
    }
    fn source_child<'a>(
        &'a self,
        node: &ResourceNode<'a>,
        position: usize,
    ) -> Option<(Option<&'a str>, ResourceNode<'a>)> {
        use ResourceNode as Node;
        match node {
            Node::Sources(values) => {
                let (key, _) = self.source.capability_availability().at(position)?;
                Some((
                    None,
                    Node::SourceEntry(ResourceSourceEntry {
                        identity: match key {
                            crate::capabilities::ToolSourceId::Mcp(id) => {
                                ResourceSourceIdentity::Mcp { server_id: id }
                            }
                            crate::capabilities::ToolSourceId::ManagedPython(package) => {
                                ResourceSourceIdentity::ManagedPython { package }
                            }
                        },
                        state: values.get(key)?,
                    }),
                ))
            }
            Node::SourceEntry(value) => match position {
                0 => Some((Some("identity"), Node::SourceIdentity(value.identity))),
                1 => Some((Some("state"), Node::Source(value.state))),
                _ => None,
            },
            Node::SourceIdentity(value) => match (*value, position) {
                (ResourceSourceIdentity::Mcp { .. }, 0) => Some((Some("type"), Node::Text("mcp"))),
                (ResourceSourceIdentity::ManagedPython { .. }, 0) => {
                    Some((Some("type"), Node::Text("managed_python")))
                }
                (ResourceSourceIdentity::Mcp { server_id }, 1) => {
                    Some((Some("server_id"), Node::Text(server_id.as_str())))
                }
                (ResourceSourceIdentity::ManagedPython { package }, 1) => {
                    Some((Some("package"), Node::Text(package)))
                }
                _ => None,
            },
            _ => None,
        }
    }
    pub(crate) fn context_file_count(&self) -> usize {
        self.source.project_context_files().len()
    }
    pub(crate) fn context_file_at(&self, position: u64) -> Option<ContextFileRead<'_>> {
        let file = self
            .source
            .project_context_files()
            .get(usize::try_from(position).ok()?)?;
        Some(ContextFileRead {
            path: &file.path,
            bytes: file.content.len() as u64,
        })
    }
}
impl schemars::JsonSchema for ContextFileRead<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ContextFileRead".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<RuntimeClientContextFile>()
    }
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct ResourceSourceEntry<'a> {
    identity: ResourceSourceIdentity<'a>,
    state: &'a crate::runtime::capability_inspection::SourceInspection,
}
#[derive(Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResourceSourceIdentity<'a> {
    ManagedPython {
        package: &'a str,
    },
    Mcp {
        server_id: &'a crate::runtime::identity::McpServerId,
    },
}
fn inspection_child(
    value: &crate::runtime::capability_inspection::CapabilityInspection,
    position: usize,
) -> Option<(Option<&str>, ResourceNode<'_>)> {
    use ResourceNode as Node;
    let (name, selected) = match position {
        0 => ("definitions", Node::Definitions(&value.definitions)),
        1 => (
            "resource_diagnostics",
            Node::Diagnostics(&value.resource_diagnostics),
        ),
        2 => (
            "main",
            value.main.as_ref().map_or(Node::Null(()), Node::Agent),
        ),
        3 => ("agents", Node::Agents(&value.agents)),
        4 => ("workflows", Node::Workflows(&value.workflows)),
        5 => ("sources", Node::Sources(&value.sources)),
        6 => ("skills", Node::Skills(&value.skills)),
        7 => (
            "skill_diagnostics",
            Node::SkillDiagnostics(&value.skill_diagnostics),
        ),
        _ => return None,
    };
    Some((Some(name), selected))
}
fn agent_child(
    value: &crate::runtime::capability_inspection::AgentInspection,
    position: usize,
) -> Option<(Option<&str>, ResourceNode<'_>)> {
    use ResourceNode as Node;
    let (name, selected) = match position {
        0 => ("identity", Node::Identity(&value.identity)),
        1 => (
            "source",
            value.source.as_deref().map_or(Node::Null(()), Node::Path),
        ),
        2 => ("tools", Node::Tools(&value.tools)),
        3 => ("agent_controls", Node::Tools(&value.agent_controls)),
        4 => ("tool_selection", Node::Selections(&value.tool_selection)),
        5 => ("skills", Node::Skills(&value.skills)),
        6 => ("agents", Node::AgentNames(&value.agents)),
        7 => ("workflows", Node::WorkflowNames(&value.workflows)),
        8 => ("plugins", Node::Extensions(&value.extensions)),
        9 => ("diagnostics", Node::AgentDiagnostics(&value.diagnostics)),
        _ => return None,
    };
    Some((Some(name), selected))
}
fn metadata_child<'a>(
    node: &ResourceNode<'a>,
    position: usize,
) -> Option<(Option<&'a str>, ResourceNode<'a>)> {
    use ResourceNode as Node;
    match node {
        Node::Tool(v) => match position {
            0 => Some((Some("id"), Node::Text(v.id.as_str()))),
            1 => Some((Some("name"), Node::Text(&v.name))),
            2 => Some((Some("origin"), Node::ToolOrigin(&v.origin))),
            _ => None,
        },
        Node::Skill(v) => match position {
            0 => Some((Some("name"), Node::Text(&v.name))),
            1 => Some((Some("source"), Node::SkillSource(&v.source))),
            2 => Some((Some("location"), Node::Text(&v.location))),
            3 => Some((Some("shadowed"), Node::ShadowedSkills(&v.shadowed))),
            _ => None,
        },
        Node::ShadowedSkill(v) => match position {
            0 => Some((Some("source"), Node::SkillSource(&v.source))),
            1 => Some((Some("location"), Node::Text(&v.location))),
            _ => None,
        },
        Node::Definition(v) => match position {
            0 => Some((Some("family"), Node::Family(v.family))),
            1 => Some((Some("name"), Node::Text(&v.name))),
            2 => Some((Some("location"), Node::Location(&v.location))),
            3 => Some((Some("valid"), Node::Boolean(v.valid))),
            _ => None,
        },
        Node::Diagnostic(v) => match position {
            0 => Some((Some("subject"), Node::Subject(&v.subject))),
            1 => Some((
                Some("file"),
                v.file.as_deref().map_or(Node::Null(()), Node::Path),
            )),
            2 => Some((Some("field"), Node::Text(&v.field))),
            3 => Some((Some("reason"), Node::Text(&v.reason))),
            _ => None,
        },
        Node::Location(v) => match position {
            0 => Some((
                Some("scope"),
                Node::Text(match v.scope {
                    crate::local_runtime::configuration::settings::SourceScope::User => "user",
                    crate::local_runtime::configuration::settings::SourceScope::Workspace => {
                        "workspace"
                    }
                }),
            )),
            1 => Some((Some("path"), Node::Path(&v.path))),
            2 => Some((
                Some("shadowed"),
                v.shadowed.as_deref().map_or(Node::Null(()), Node::Path),
            )),
            _ => None,
        },
        Node::Workflow(v) => match (v, position) {
            (crate::runtime::capability_inspection::WorkflowInspection::Enabled, 0) => {
                Some((Some("status"), Node::Text("enabled")))
            }
            (crate::runtime::capability_inspection::WorkflowInspection::Disabled(_), 0) => {
                Some((Some("status"), Node::Text("disabled")))
            }
            (
                crate::runtime::capability_inspection::WorkflowInspection::Disabled(diagnostics),
                1,
            ) => Some((Some("diagnostics"), Node::WorkflowDiagnostics(diagnostics))),
            _ => None,
        },
        Node::WorkflowDiagnostics(v) => Some((None, Node::WorkflowDiagnostic(v.get(position)?))),
        Node::ToolOrigin(v) => match (v, position) {
            (crate::tools::types::ToolOrigin::Mcp { server_id }, 0) => {
                Some((Some("server_id"), Node::Text(server_id.as_str())))
            }
            (crate::tools::types::ToolOrigin::ManagedPython { package }, 0) => {
                Some((Some("package"), Node::Text(package)))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The client-visible projection of one immutable runtime resource
/// generation.
///
/// It carries identity and provenance, never content: a client that wants
/// the text of a project context file reads the file, exactly as it would
/// read a `SKILL.md`. Publishing the bytes here would create a second copy
/// of request input inside a conversation projection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientResourcesView {
    /// Facts copied from the same immutable generation as this revision.
    /// Deferred inspection facts are loaded through finite native field pages.
    pub inspection: Option<Box<crate::runtime::capability_inspection::CapabilityInspection>>,
    /// The monotonic process-local resource generation.
    #[serde(default)]
    pub revision: RuntimeResourceRevision,
    /// The runtime-loaded project instruction files, root-most to
    /// workspace, in the exact order the runtime concatenated them.
    #[serde(default)]
    pub context_files: Vec<RuntimeClientContextFile>,
    /// Whether an immutable agent profile/persona is frozen into this
    /// generation. The persona text itself is runtime-owned request input.
    #[serde(default)]
    pub agent_profile: bool,
}
/// One runtime-loaded project instruction file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientContextFile {
    /// The canonical absolute host path the runtime read.
    pub path: String,
    /// The exact byte length of the loaded content.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub bytes: u64,
}
