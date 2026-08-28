//! The one resolution boundary between a named definition and the runtime
//! generation that admits it (Issue #144).
//!
//! ```text
//! SubagentDefinition
//! + invoking RuntimeResourceSnapshot Rn
//! + invoking attempt model authority
//!        |
//!        v
//! ResolvedSubagentSpec   (frozen: every semantic identity the child needs)
//! ```
//!
//! # Authority
//!
//! Resolution reads the invoking generation's **available** capability
//! catalog — [`CapabilitySnapshot::available_tools`] — and the matching
//! capability-source availability of that same generation. It deliberately
//! never reads the parent model's active `ToolRegistry`:
//!
//! ```text
//! ParentActiveTools     ⊆ CapabilitySnapshot::available_tools()
//! SubagentResolvedTools ⊆ CapabilitySnapshot::available_tools()
//! SubagentResolvedTools ⊄ ParentActiveTools          (deliberately)
//! ```
//!
//! A named subagent is an independent projection of the authority admitted
//! into the invoking attempt's runtime generation: it may **narrow** that
//! authority but can never manufacture authority the generation does not
//! already hold.
//!
//! # Optionality
//!
//! Optionality belongs to *source availability*, never to a selection. Once
//! a definition explicitly selects a capability, that capability is required
//! for that invocation: an unavailable source makes the invocation fail
//! before ownership commit, and an unknown selection whose source authority
//! *is* present is a static configuration error that rejects
//! resource-generation preparation.

use serde::{Deserialize, Serialize};

use crate::capabilities::{
    AvailableToolCatalog, CapabilityAvailability, CapabilitySourceId, CapabilitySourceState,
};
use crate::model::invocation::ModelBindingRegistry;
use crate::model::session::SessionModelConfig;
use crate::runtime::identity::{McpServerId, ToolId, ToolVersionId};
use crate::runtime::resources::{ProjectContextFile, RuntimeResourceSnapshot};
use crate::skills::{SkillCatalogEntry, SkillSnapshot};
use crate::tools::types::{ToolDefinition, ToolOrigin};

use super::catalog::{
    SubagentCatalog, SubagentDefinition, SubagentDefinitionDigest, SubagentName,
    SubagentToolSelector,
};

/// One frozen capability identity of a resolved child.
///
/// The frozen form keeps the exact canonical identity of the origin the
/// capability came from, so the next issue's physical materialization can
/// realize the very definition this generation authorized instead of
/// re-resolving a name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResolvedSubagentTool {
    /// A runtime built-in/native capability.
    Builtin {
        /// The exact `ToolId` of the admitted definition.
        tool_id: ToolId,
        /// The canonical model-facing name.
        name: String,
        /// The exact admitted definition.
        definition: ToolDefinition,
    },
    /// One tool of one configured MCP server.
    Mcp {
        /// The authoritative MCP server identity.
        server_id: McpServerId,
        /// The exact `ToolId` of the admitted definition.
        tool_id: ToolId,
        /// The canonical tool name as the server publishes it.
        name: String,
        /// The exact admitted definition, which is the identity the child's
        /// physical MCP materialization must realize.
        definition: ToolDefinition,
    },
    /// One custom Python tool at its exact immutable version.
    Python {
        /// The exact `ToolId` of the admitted definition.
        tool_id: ToolId,
        /// The exact immutable tool version to execute.
        tool_version_id: ToolVersionId,
        /// The canonical model-facing name.
        name: String,
        /// The exact admitted definition.
        definition: ToolDefinition,
    },
}

impl ResolvedSubagentTool {
    /// The canonical model-facing name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Builtin { name, .. } | Self::Mcp { name, .. } | Self::Python { name, .. } => name,
        }
    }

    /// The exact admitted definition.
    #[must_use]
    pub const fn definition(&self) -> &ToolDefinition {
        match self {
            Self::Builtin { definition, .. }
            | Self::Mcp { definition, .. }
            | Self::Python { definition, .. } => definition,
        }
    }

    /// Whether this capability needs an external execution plane the child
    /// runtime does not physically materialize yet.
    #[must_use]
    pub const fn is_external_origin(&self) -> bool {
        matches!(self, Self::Mcp { .. } | Self::Python { .. })
    }

    /// The canonical selection text of this resolution, for diagnostics.
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Builtin { name, .. } => format!("builtin:{name}"),
            Self::Mcp {
                server_id, name, ..
            } => format!("mcp:{server_id}/{name}"),
            Self::Python { name, .. } => format!("python:{name}"),
        }
    }
}

/// The complete frozen launch specification of one named subagent child.
///
/// Every semantic identity the child needs is already decided here. The
/// child consumes this value and reinterprets nothing: it does not read
/// `rustx.jsonc`, discover project instructions, choose model state,
/// rediscover Skills, or widen Tool authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedSubagentSpec {
    /// The canonical agent name this child was started as.
    pub agent: SubagentName,
    /// The deterministic semantic identity of the definition at start.
    pub definition_digest: SubagentDefinitionDigest,
    /// The exact child instruction document, composed as the child's
    /// request-time `AgentProfile` System section.
    pub instructions: String,
    /// The frozen child model configuration: the definition's explicit
    /// selection, or the invoking attempt's frozen effective configuration.
    pub model: SessionModelConfig,
    /// The frozen capability identities, in canonical order.
    pub tools: Vec<ResolvedSubagentTool>,
    /// The frozen Skill catalog metadata the child may see. Bodies and
    /// supporting resources are **not** included: progressive disclosure is
    /// preserved and the child loads them through ordinary Skill semantics.
    pub skills: Vec<SkillCatalogEntry>,
    /// The frozen project instruction chain, in deterministic order.
    pub project_instructions: Vec<ProjectContextFile>,
}

impl ResolvedSubagentSpec {
    /// The frozen capabilities whose physical execution plane the child
    /// runtime cannot materialize yet, in canonical order.
    ///
    /// This is the temporary staged boundary of the follow-up external
    /// capability issue: semantic resolution is complete and exact, but a
    /// child that would need one of these capabilities must fail **before**
    /// durable ownership commit rather than start weaker than it was
    /// authorized to be.
    #[must_use]
    pub fn external_origin_requirements(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|tool| tool.is_external_origin())
            .map(ResolvedSubagentTool::canonical)
            .collect()
    }

    /// The canonical model-facing names of the frozen capability set.
    #[must_use]
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(ResolvedSubagentTool::name).collect()
    }
}

/// A typed resolution failure.
///
/// Every variant is decided **before** any child process is staged and
/// therefore long before durable ownership commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentResolutionError {
    /// The invoking generation's catalog admits no agent of that name.
    UnknownAgent {
        /// The requested name.
        agent: String,
        /// The admitted names of this generation, in canonical order.
        available: Vec<String>,
    },
    /// The definition selects a capability whose optional source is
    /// unavailable in this runtime generation. The runtime itself stays
    /// healthy; this invocation cannot start.
    SourceUnavailable {
        /// The offending selector.
        selector: String,
        /// The unavailable source.
        source: String,
        /// The source's bounded availability diagnostic.
        reason: String,
    },
    /// The definition selects a capability the invoking generation does not
    /// authorize, while the selector's source authority is present. This is
    /// a static configuration error.
    UnknownCapability {
        /// The offending selector.
        selector: String,
    },
    /// The definition selects a Skill the invoking generation did not admit.
    UnknownSkill {
        /// The offending Skill name.
        skill: String,
    },
    /// The definition selects an admitted Skill that the generation hides
    /// from model invocation. It is never silently omitted.
    SkillNotModelVisible {
        /// The offending Skill name.
        skill: String,
    },
    /// The definition names a model the admitted model authority cannot
    /// resolve.
    UnknownModel {
        /// The offending model reference.
        model: String,
        /// The resolution failure detail.
        detail: String,
    },
}

impl core::fmt::Display for SubagentResolutionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownAgent { agent, available } => {
                if available.is_empty() {
                    write!(
                        formatter,
                        "unknown subagent {agent:?}: this runtime admits no named subagent"
                    )
                } else {
                    write!(
                        formatter,
                        "unknown subagent {agent:?}: this runtime admits {}",
                        available.join(", ")
                    )
                }
            }
            Self::SourceUnavailable {
                selector,
                source,
                reason,
            } => write!(
                formatter,
                "the subagent requires {selector}, but capability source {source} is \
                 unavailable in this runtime generation: {reason}"
            ),
            Self::UnknownCapability { selector } => write!(
                formatter,
                "the subagent requires {selector}, which this runtime generation does not \
                 authorize"
            ),
            Self::UnknownSkill { skill } => write!(
                formatter,
                "the subagent requires Skill {skill:?}, which this runtime generation did \
                 not admit"
            ),
            Self::SkillNotModelVisible { skill } => write!(
                formatter,
                "the subagent requires Skill {skill:?}, which this runtime generation \
                 admitted but hides from model invocation"
            ),
            Self::UnknownModel { model, detail } => write!(
                formatter,
                "the subagent names model {model:?}, which the admitted model catalog \
                 cannot resolve: {detail}"
            ),
        }
    }
}

impl std::error::Error for SubagentResolutionError {}

/// The one resolution core shared by every capability origin and by both
/// resolution callers (invocation-time resolution and admission-time
/// validation of a prepared generation).
pub struct SubagentResolver;

impl SubagentResolver {
    /// Resolves one named agent against the runtime generation owned by the
    /// invoking attempt.
    ///
    /// `attempt_model` is the invoking attempt's **frozen** effective model
    /// configuration, not live mutable session state: a definition with no
    /// explicit model inherits exactly the model its invoking attempt was
    /// admitted with.
    ///
    /// # Errors
    ///
    /// Returns the first typed [`SubagentResolutionError`].
    pub fn resolve(
        resources: &RuntimeResourceSnapshot,
        agent: &SubagentName,
        attempt_model: &SessionModelConfig,
        models: &ModelBindingRegistry,
    ) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
        let catalog = resources.subagents();
        let definition =
            catalog
                .get(agent)
                .ok_or_else(|| SubagentResolutionError::UnknownAgent {
                    agent: agent.as_str().to_owned(),
                    available: catalog
                        .names()
                        .into_iter()
                        .map(|name| name.as_str().to_owned())
                        .collect(),
                })?;
        let capability = resources.capability();
        let tools = resolve_tools(
            definition,
            capability.available_tools(),
            resources.capability_availability(),
        )?;
        let skills = resolve_skills(definition, capability.skills())?;
        let model = resolve_model(definition, attempt_model, models)?;
        let project_instructions = resolve_project_instructions(definition, resources);
        Ok(ResolvedSubagentSpec {
            agent: definition.name().clone(),
            definition_digest: definition.digest().clone(),
            instructions: definition.instructions().to_owned(),
            model,
            tools,
            skills,
            project_instructions,
        })
    }

    /// Validates every definition of a prepared catalog against the
    /// capability/Skill/model authority of the generation being prepared.
    ///
    /// This is the resource-generation admission gate: a statically invalid
    /// definition rejects the whole candidate generation, so a failed reload
    /// leaves the previous generation completely authoritative. A selection
    /// whose *source* is merely unavailable is **not** a preparation
    /// failure — the runtime stays healthy and only that agent's invocation
    /// fails.
    ///
    /// # Errors
    ///
    /// Returns the first static violation, naming the offending agent.
    pub fn validate_catalog(
        catalog: &SubagentCatalog,
        available_tools: &AvailableToolCatalog,
        availability: &CapabilityAvailability,
        skills: &SkillSnapshot,
        models: &ModelBindingRegistry,
    ) -> Result<(), (SubagentName, SubagentResolutionError)> {
        for definition in catalog.definitions() {
            let named = |error: SubagentResolutionError| (definition.name().clone(), error);
            match resolve_tools(definition, available_tools, availability) {
                Ok(_) | Err(SubagentResolutionError::SourceUnavailable { .. }) => {}
                Err(error) => return Err(named(error)),
            }
            resolve_skills(definition, skills).map_err(named)?;
            if let Some(model) = definition.model() {
                models
                    .resolve(&SessionModelConfig::of(model.clone()).selection())
                    .map_err(|error| {
                        named(SubagentResolutionError::UnknownModel {
                            model: model.to_string(),
                            detail: error.to_string(),
                        })
                    })?;
            }
        }
        Ok(())
    }
}

/// Resolves every typed selector through the one capability-selection core.
fn resolve_tools(
    definition: &SubagentDefinition,
    available: &AvailableToolCatalog,
    availability: &CapabilityAvailability,
) -> Result<Vec<ResolvedSubagentTool>, SubagentResolutionError> {
    let mut resolved = Vec::with_capacity(definition.tools().len());
    for selector in definition.tools() {
        // Source availability is consulted first and only for origins that
        // *have* an optional source. An unavailable source is a runtime
        // health fact about this generation, not a statement that the
        // selection is wrong, so it is reported as its own typed outcome.
        if let Some(source) = optional_source(selector)
            && let Some(CapabilitySourceState::Unavailable { reason }) = availability.get(&source)
        {
            return Err(SubagentResolutionError::SourceUnavailable {
                selector: selector.canonical(),
                source: source.to_string(),
                reason: reason.clone(),
            });
        }
        let definition = available
            .tools()
            .iter()
            .map(|tool| &tool.definition)
            .find(|candidate| matches_selector(candidate, selector))
            .ok_or_else(|| SubagentResolutionError::UnknownCapability {
                selector: selector.canonical(),
            })?;
        resolved.push(freeze_tool(selector, definition));
    }
    Ok(resolved)
}

/// The optional capability source one selector depends on, when its origin
/// has one. Builtin capabilities are the runtime's own base registry and
/// have no availability state at all.
fn optional_source(selector: &SubagentToolSelector) -> Option<CapabilitySourceId> {
    match selector {
        SubagentToolSelector::Builtin { .. } => None,
        SubagentToolSelector::Mcp { server_id, .. } => {
            Some(CapabilitySourceId::Mcp(server_id.clone()))
        }
        SubagentToolSelector::Python { .. } => Some(CapabilitySourceId::Python),
    }
}

/// Whether one admitted definition is exactly the capability a selector
/// names. Origin identity participates, so a Builtin `read` and an MCP
/// server's `read` are never interchangeable.
fn matches_selector(definition: &ToolDefinition, selector: &SubagentToolSelector) -> bool {
    match (selector, &definition.origin) {
        (SubagentToolSelector::Builtin { name }, ToolOrigin::Builtin) => definition.name == *name,
        (
            SubagentToolSelector::Mcp {
                server_id,
                name: selected,
            },
            ToolOrigin::Mcp {
                server_id: admitted,
            },
        ) => definition.name == *selected && admitted == server_id,
        (SubagentToolSelector::Python { name }, ToolOrigin::Python { .. }) => {
            definition.name == *name
        }
        _ => false,
    }
}

/// Freezes one admitted definition into its exact source-qualified identity.
fn freeze_tool(
    selector: &SubagentToolSelector,
    definition: &ToolDefinition,
) -> ResolvedSubagentTool {
    match (selector, &definition.origin) {
        (SubagentToolSelector::Mcp { server_id, .. }, _) => ResolvedSubagentTool::Mcp {
            server_id: server_id.clone(),
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            definition: definition.clone(),
        },
        (SubagentToolSelector::Python { .. }, ToolOrigin::Python { tool_version_id }) => {
            ResolvedSubagentTool::Python {
                tool_id: definition.id.clone(),
                tool_version_id: tool_version_id.clone(),
                name: definition.name.clone(),
                definition: definition.clone(),
            }
        }
        _ => ResolvedSubagentTool::Builtin {
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            definition: definition.clone(),
        },
    }
}

/// Applies the exact Skill allowlist over the generation's admitted Skills.
///
/// Only catalog **metadata** is frozen: rustX progressive disclosure is
/// preserved, so a selected Skill's `SKILL.md` body and supporting resources
/// are still loaded through ordinary Skill semantics rather than preloaded
/// into the child's system prompt.
fn resolve_skills(
    definition: &SubagentDefinition,
    skills: &SkillSnapshot,
) -> Result<Vec<SkillCatalogEntry>, SubagentResolutionError> {
    let mut resolved = Vec::with_capacity(definition.skills().len());
    for selected in definition.skills() {
        match skills
            .catalog_entries()
            .iter()
            .find(|entry| entry.name == *selected)
        {
            Some(entry) => resolved.push(entry.clone()),
            None if skills
                .packages()
                .iter()
                .any(|package| package.name() == selected) =>
            {
                return Err(SubagentResolutionError::SkillNotModelVisible {
                    skill: selected.clone(),
                });
            }
            None => {
                return Err(SubagentResolutionError::UnknownSkill {
                    skill: selected.clone(),
                });
            }
        }
    }
    Ok(resolved)
}

/// Freezes the child's model configuration.
///
/// An explicit selection resolves through the admitted model authority and
/// fails closed. No explicit selection inherits the invoking attempt's own
/// frozen effective configuration — never live mutable session state and
/// never a composition-time capture.
fn resolve_model(
    definition: &SubagentDefinition,
    attempt_model: &SessionModelConfig,
    models: &ModelBindingRegistry,
) -> Result<SessionModelConfig, SubagentResolutionError> {
    let Some(model) = definition.model() else {
        return Ok(attempt_model.clone());
    };
    let config = SessionModelConfig::of(model.clone());
    models
        .resolve(&config.selection())
        .map_err(|error| SubagentResolutionError::UnknownModel {
            model: model.to_string(),
            detail: error.to_string(),
        })?;
    Ok(config)
}

/// Composes the child's frozen project instruction chain.
///
/// `inherit = true` prepends the invoking generation's normal chain, in its
/// own deterministic root-to-leaf order, before the definition's explicit
/// files in configured order. `inherit = false` freezes the explicit files
/// only. The child performs no ancestor discovery of its own in either case.
fn resolve_project_instructions(
    definition: &SubagentDefinition,
    resources: &RuntimeResourceSnapshot,
) -> Vec<ProjectContextFile> {
    let policy = definition.project_instructions();
    let mut files = if policy.inherit {
        resources.project_context_files().to_vec()
    } else {
        Vec::new()
    };
    files.extend(policy.files.iter().cloned());
    files
}

/// The bounded routing catalog rendered into the model-facing `subagent`
/// Tool description.
///
/// Generation is deterministic and bounded: agent names appear in canonical
/// order and each description is already bounded by definition admission.
#[must_use]
pub(crate) fn render_agent_routing(catalog: &SubagentCatalog) -> String {
    if catalog.is_empty() {
        return "This runtime admits no named subagent; the call always fails.".to_owned();
    }
    let mut rendered = String::from("Available agents:");
    for definition in catalog.definitions() {
        use std::fmt::Write as _;
        let _ = write!(
            rendered,
            "\n- {}: {}",
            definition.name(),
            definition.description()
        );
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{
        ResolvedSubagentTool, SubagentResolutionError, render_agent_routing, resolve_tools,
    };
    use crate::capabilities::{
        AvailableToolCatalog, CapabilityAvailability, CapabilitySourceId, CapabilitySourceState,
    };
    use crate::runtime::identity::{McpServerId, ToolId, ToolVersionId};
    use crate::runtime::subagent::catalog::{
        SubagentCatalog, SubagentDefinition, SubagentName, SubagentProjectInstructionPolicy,
        SubagentToolSelector,
    };
    use crate::tools::types::{
        ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin,
        ToolReplayPolicy,
    };

    fn tool(name: &str, origin: ToolOrigin) -> ToolDefinition {
        ToolDefinition {
            id: ToolId::new(format!("tool-{name}-{origin:?}")),
            name: name.to_owned(),
            description: format!("{name} tool"),
            input_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin,
        }
    }

    fn available() -> AvailableToolCatalog {
        AvailableToolCatalog::new(vec![
            tool("read", ToolOrigin::Builtin),
            tool("grep", ToolOrigin::Builtin),
            tool(
                "get_issue",
                ToolOrigin::Mcp {
                    server_id: McpServerId::new("github"),
                },
            ),
            tool(
                "repository_symbols",
                ToolOrigin::Python {
                    tool_version_id: ToolVersionId::new("sha256:abc"),
                },
            ),
        ])
    }

    fn definition(tools: Vec<SubagentToolSelector>) -> SubagentDefinition {
        SubagentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            "description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/explore.md"),
            None,
            tools,
            Vec::new(),
            SubagentProjectInstructionPolicy {
                inherit: true,
                files: Vec::new(),
            },
        )
        .expect("definition")
    }

    fn ready() -> CapabilityAvailability {
        let mut availability = CapabilityAvailability::new();
        availability.insert(CapabilitySourceId::Python, CapabilitySourceState::Ready);
        availability.insert(
            CapabilitySourceId::Mcp(McpServerId::new("github")),
            CapabilitySourceState::Ready,
        );
        availability
    }

    #[test]
    fn every_origin_freezes_its_exact_source_identity() {
        let resolved = resolve_tools(
            &definition(vec![
                SubagentToolSelector::Builtin {
                    name: "read".to_owned(),
                },
                SubagentToolSelector::Mcp {
                    server_id: McpServerId::new("github"),
                    name: "get_issue".to_owned(),
                },
                SubagentToolSelector::Python {
                    name: "repository_symbols".to_owned(),
                },
            ]),
            &available(),
            &ready(),
        )
        .expect("resolution");
        assert!(matches!(
            &resolved[0],
            ResolvedSubagentTool::Builtin { name, .. } if name == "read"
        ));
        assert!(matches!(
            &resolved[1],
            ResolvedSubagentTool::Mcp { server_id, name, .. }
                if server_id.as_str() == "github" && name == "get_issue"
        ));
        assert!(matches!(
            &resolved[2],
            ResolvedSubagentTool::Python { tool_version_id, name, .. }
                if tool_version_id.as_str() == "sha256:abc" && name == "repository_symbols"
        ));
        assert_eq!(
            resolved
                .iter()
                .filter(|tool| tool.is_external_origin())
                .count(),
            2
        );
    }

    #[test]
    fn origin_identity_is_never_collapsed_into_a_bare_name() {
        // The same bare name exists under two origins; a Builtin selector
        // must never resolve to the MCP capability and vice versa.
        let catalog = AvailableToolCatalog::new(vec![
            tool("search", ToolOrigin::Builtin),
            tool(
                "search",
                ToolOrigin::Mcp {
                    server_id: McpServerId::new("github"),
                },
            ),
        ]);
        let builtin = resolve_tools(
            &definition(vec![SubagentToolSelector::Builtin {
                name: "search".to_owned(),
            }]),
            &catalog,
            &ready(),
        )
        .expect("builtin resolution");
        assert!(matches!(builtin[0], ResolvedSubagentTool::Builtin { .. }));
        let mcp = resolve_tools(
            &definition(vec![SubagentToolSelector::Mcp {
                server_id: McpServerId::new("github"),
                name: "search".to_owned(),
            }]),
            &catalog,
            &ready(),
        )
        .expect("mcp resolution");
        assert!(matches!(mcp[0], ResolvedSubagentTool::Mcp { .. }));
        assert_eq!(
            resolve_tools(
                &definition(vec![SubagentToolSelector::Mcp {
                    server_id: McpServerId::new("other"),
                    name: "search".to_owned(),
                }]),
                &catalog,
                &ready(),
            ),
            Err(SubagentResolutionError::UnknownCapability {
                selector: "mcp:other/search".to_owned()
            })
        );
    }

    #[test]
    fn an_unavailable_source_is_distinct_from_an_invalid_selector() {
        let mut availability = ready();
        availability.insert(
            CapabilitySourceId::Mcp(McpServerId::new("github")),
            CapabilitySourceState::unavailable("the server refused the handshake"),
        );
        // The MCP capability is absent from the available catalog precisely
        // because its source failed; the outcome must still be the
        // source-unavailable fact, not "unknown capability".
        let catalog = AvailableToolCatalog::new(vec![tool("read", ToolOrigin::Builtin)]);
        assert!(matches!(
            resolve_tools(
                &definition(vec![SubagentToolSelector::Mcp {
                    server_id: McpServerId::new("github"),
                    name: "get_issue".to_owned(),
                }]),
                &catalog,
                &availability,
            ),
            Err(SubagentResolutionError::SourceUnavailable { .. })
        ));
        assert_eq!(
            resolve_tools(
                &definition(vec![SubagentToolSelector::Builtin {
                    name: "write".to_owned()
                }]),
                &catalog,
                &availability,
            ),
            Err(SubagentResolutionError::UnknownCapability {
                selector: "builtin:write".to_owned()
            })
        );
    }

    #[test]
    fn the_routing_description_is_deterministic_and_derived_from_the_catalog() {
        let catalog = SubagentCatalog::new([
            SubagentDefinition::new(
                SubagentName::parse("research").expect("name"),
                "Deep research.".to_owned(),
                "instructions".to_owned(),
                std::path::PathBuf::from("/w/research.md"),
                None,
                Vec::new(),
                Vec::new(),
                SubagentProjectInstructionPolicy {
                    inherit: true,
                    files: Vec::new(),
                },
            )
            .expect("definition"),
            SubagentDefinition::new(
                SubagentName::parse("explore").expect("name"),
                "Read-only exploration.".to_owned(),
                "instructions".to_owned(),
                std::path::PathBuf::from("/w/explore.md"),
                None,
                Vec::new(),
                Vec::new(),
                SubagentProjectInstructionPolicy {
                    inherit: true,
                    files: Vec::new(),
                },
            )
            .expect("definition"),
        ])
        .expect("catalog");
        assert_eq!(
            render_agent_routing(&catalog),
            "Available agents:\n- explore: Read-only exploration.\n- research: Deep research."
        );
        assert_eq!(
            render_agent_routing(&SubagentCatalog::empty()),
            "This runtime admits no named subagent; the call always fails."
        );
    }
}
