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
//!
//! The two callers of the per-selector core are therefore asymmetric, and
//! the asymmetry is the whole point:
//!
//! ```text
//! resolve_tools                   (invocation)  fail fast on the first
//!                                               unsatisfiable selector
//! validate_selectors_for_admission (admission)  inspect EVERY selector;
//!                                               tolerate an unavailable
//!                                               source only for that one
//! ```
//!
//! Admission may not stop at an unavailable source: an offline MCP server
//! listed before a misspelled selector would otherwise let a statically
//! invalid definition into a published generation.
//!
//! # The parent decides; the child materializes
//!
//! Everything in [`ResolvedSubagentSpec`] is a decision made *here*, in the
//! parent, against the invoking attempt's admitted authority. The child
//! performs physical materialization only. Three representations carry that
//! contract: the model crosses as a completely resolved
//! [`FrozenModelSpec`] rather than a desired configuration plus a catalog
//! path, each Builtin capability crosses as its exact admitted
//! `ToolDefinition` rather than a name, and each Skill crosses as its
//! immutable `SkillId` + `SkillVersionId` binding rather than a host path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::capabilities::{AvailableToolCatalog, CapabilityAvailability, CapabilitySnapshot};
use crate::extensions::NativeAgentExtensions;
use crate::model::frozen::FrozenModelSpec;
use crate::model::invocation::ModelBindingRegistry;
use crate::model::session::SessionModelConfig;
use crate::protocol::manifest::SkillBinding;
use crate::runtime::identity::{McpServerId, McpToolIdentity, SkillId, SkillVersionId, ToolId};
use crate::runtime::resources::{ProjectContextFile, RuntimeResourceSnapshot};
use crate::skills::{SkillCatalogEntry, SkillSnapshot};
use crate::tools::mcp::{McpServerBinding, McpServerBindings};
use crate::tools::types::ToolDefinition;

use super::catalog::{
    SubagentCatalog, SubagentDefinition, SubagentDefinitionDigest, SubagentExecutionDeadline,
    SubagentName,
};
use super::invocation::{SubagentInvocationOverride, SubagentOverrideError};
use crate::capabilities::selection::ToolSelector;
use crate::runtime::workspace::WorkspacePolicy;

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
        /// The deterministic **cross-process** semantic identity of that
        /// definition (Issue #145).
        ///
        /// The child connects the server itself, performs its own
        /// `tools/list`, recomputes this digest from what the server
        /// actually publishes, and refuses to start unless it matches. The
        /// process-local MCP invalidation epoch cannot serve this purpose:
        /// it stabilizes one process's catalog read and has no meaning in
        /// another process.
        identity: McpToolIdentity,
    },
}

impl ResolvedSubagentTool {
    /// The canonical model-facing name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Builtin { name, .. } | Self::Mcp { name, .. } => name,
        }
    }

    /// The exact admitted definition.
    #[must_use]
    pub const fn definition(&self) -> &ToolDefinition {
        match self {
            Self::Builtin { definition, .. } | Self::Mcp { definition, .. } => definition,
        }
    }

    /// The canonical selection text of this resolution, for diagnostics.
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Builtin { name, .. } => format!("builtin:{name}"),
            Self::Mcp {
                server_id, name, ..
            } => format!("mcp:{server_id}/{name}"),
        }
    }
}

/// One frozen Skill authorization of a resolved child.
///
/// Two different things travel together here, and the distinction is the
/// point:
///
/// - `binding` is the **immutable identity** of the exact Skill version the
///   invoking generation admitted (`SkillId` + `SkillVersionId`). It is what
///   a physical Skill materialization must realize, and it is what makes an
///   old frozen specification unambiguous after the host filesystem has
///   moved on. A host path is *not* an identity: the bytes behind a path can
///   change without the path changing.
/// - `catalog_entry` is the **model-visible metadata** of that Skill: name,
///   description, and the host `SKILL.md` location the model passes to Read.
///   It is exactly what progressive disclosure needs and nothing more — no
///   `SKILL.md` body and no supporting resource ever crosses this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedSubagentSkill {
    /// The exact immutable `SkillId` + `SkillVersionId` the generation
    /// admitted.
    pub binding: SkillBinding,
    /// The model-visible catalog metadata of that same Skill, as the parent
    /// generation rendered it. The child **remaps** `location` onto its own
    /// materialized copy; every other field crosses verbatim.
    pub catalog_entry: SkillCatalogEntry,
    /// The canonical absolute host root of the admitted package.
    ///
    /// This is a materialization **source**, never an identity: the child
    /// copies exactly `files` from here and then proves the copy hashes back
    /// to `binding.version_id`. A source whose bytes moved on after the
    /// parent froze them therefore fails closed instead of being executed.
    pub source_root: PathBuf,
    /// The exact package-relative file set of the admitted package, in the
    /// generation's deterministic order.
    pub files: Vec<PathBuf>,
}

/// The frozen **physical materialization plane** of one resolved child
/// (Issue #145).
///
/// The rest of [`ResolvedSubagentSpec`] freezes *semantic* identity: which
/// Tool, which version, which Skill. This value freezes what the child needs
/// in order to physically realize those identities with runtimes it owns
/// itself — and nothing more. It carries only the sources the selected
/// capabilities actually require:
///
/// ```text
/// selected mcp:github/get_issue   ->  mcp_servers = { github: <binding> }
///                                     (never every configured server)
/// selected nothing external       ->  empty
/// ```
///
/// Managed Python tool packages cross as ordinary frozen MCP bindings under
/// their synthesized server identities (Issue #174); there is no
/// Python-specific materialization channel.
///
/// The child never reads `rustx.jsonc` to obtain any of this: a
/// configuration edit between the parent's freeze and the child's
/// composition cannot change which server the child connects or which store
/// it opens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedSubagentMaterialization {
    /// Exactly the MCP servers whose tools this child selected, keyed by
    /// the one authoritative server identity.
    pub mcp_servers: BTreeMap<McpServerId, McpServerBinding>,
}

impl ResolvedSubagentMaterialization {
    /// Whether this child needs any externally sourced execution plane at
    /// all. A child with no external requirement composes exactly the
    /// deterministic base-only plane it always did.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mcp_servers.is_empty()
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
    /// The optional whole-lifecycle execution deadline frozen by definition
    /// resolution. The registry starts its monotonic countdown only after
    /// durable ownership commits.
    pub execution_deadline: Option<SubagentExecutionDeadline>,
    /// The definition-level project workspace policy resolved before any
    /// child process or lease is staged.
    pub workspace_policy: WorkspacePolicy,
    /// The exact child instruction document, composed as the child's
    /// request-time `AgentProfile` System section.
    pub instructions: String,
    /// The frozen child model **authority**: the completely resolved
    /// invocation (provider binding, protocol, context window, output
    /// budget, reasoning profile, effective request parameters, effective
    /// capabilities, compat) of the definition's explicit selection, or of
    /// the invoking attempt's frozen effective configuration.
    ///
    /// A resolved invocation crosses the boundary, never a desired
    /// `SessionModelConfig` plus a catalog path: the child materializes this
    /// decision physically and never reopens `models.jsonc` as semantic
    /// authority, so a catalog edit between parent freeze and child
    /// composition cannot change what the child runs.
    pub model: FrozenModelSpec,
    /// The frozen capability identities, in canonical order.
    pub tools: Vec<ResolvedSubagentTool>,
    /// The frozen Skill authorizations: exact version identity plus the
    /// model-visible catalog metadata. Bodies and supporting resources are
    /// **not** included: progressive disclosure is preserved and the child
    /// loads them through ordinary Skill semantics.
    pub skills: Vec<ResolvedSubagentSkill>,
    /// The frozen project instruction chain, in deterministic order.
    pub project_instructions: Vec<ProjectContextFile>,
    /// The frozen physical materialization plane of the selected external
    /// capabilities (Issue #145).
    pub materialization: ResolvedSubagentMaterialization,
    /// The frozen **native Agent Extension composition** of this child
    /// (Issue #256, invocation-scoped by Issue #258).
    ///
    /// It is the named definition's composition, or the composition an
    /// authorized invocation override replaced it with. The invoking root
    /// Agent's own composition is never *inherited*: it participates only as
    /// one half of the explicit delegation ceiling, so a child composes an
    /// extension only because a definition authored it or a caller asked for
    /// it and was entitled to. The child process materializes exactly this
    /// value and rereads no configuration document, role file, or later
    /// resource generation to reinterpret which extensions it owns.
    pub extensions: crate::extensions::NativeAgentExtensions,
}

/// The deterministic semantic identity of one **effective** child execution
/// profile (Issue #258).
///
/// # What the framing covers
///
/// ```text
/// agent name
/// source definition digest
/// frozen model decision      model ref, protocol, context window,
///                            model/effective output budget, reasoning
///                            profile and semantics, effective + declared
///                            capabilities, compat, effective request params
/// instructions
/// project instruction chain  path AND content, in order
/// workspace policy
/// execution deadline
/// effective tools            origin, exact ToolId, model-facing name, and
///                            the cross-process MCP identity where one exists
/// effective Skills           exact SkillId + SkillVersionId binding and the
///                            model-visible name
/// effective extensions       the closed composition's canonical framing
/// materialization plane      exactly the external source identities the
///                            effective selection requires
/// ```
///
/// # What the framing deliberately excludes
///
/// ```text
/// execution identities       SubagentId, conversation/agent ids, tool call id
/// timestamps                 nothing time-derived enters the preimage
/// physical paths             the staging root, the Skill source root, the
///                            child's remapped Skill locations
/// raw payload formatting     the override JSON's key order, whitespace,
///                            duplicate entries, or whether an override was
///                            written at all
/// provider binding material  endpoints and credential sources
/// ```
///
/// Excluding the raw payload is the whole point: the digest names the
/// *profile*, so a caller cannot make two identical children look different
/// by respelling their request, and cannot make two different children look
/// identical either. Excluding provider binding material keeps credential
/// data out of the preimage of a value the runtime projects.
///
/// Collections that carry no meaning in their order — the Tool and Skill
/// selections — are already canonically ordered and deduplicated before they
/// reach the framing. Collections whose order *is* meaning — the project
/// instruction chain — are framed in their exact order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubagentExecutionProfileDigest(String);

/// The canonical framing version of [`SubagentExecutionProfileDigest`].
///
/// It is part of the hashed preimage, exactly like the definition digest's
/// version: a later milestone that admits a new profile-defining field bumps
/// this constant so two framings can never collide into one digest.
pub const SUBAGENT_EXECUTION_PROFILE_DIGEST_VERSION: &str = "rustx-subagent-profile-v1";

impl SubagentExecutionProfileDigest {
    /// The stable textual form `sha256:<64 lowercase hex characters>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for SubagentExecutionProfileDigest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl ResolvedSubagentSpec {
    /// The canonical model-facing names of the frozen capability set.
    #[must_use]
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(ResolvedSubagentTool::name).collect()
    }

    /// The deterministic identity of this **effective execution profile**
    /// (Issue #258).
    ///
    /// [`Self::definition_digest`] identifies the source role; this value
    /// identifies the child that role plus an authorized override actually
    /// produced. Two children of one role with materially different effective
    /// tools, Skills, or extensions differ here; two semantically equivalent
    /// resolutions — no override, and an override restating the defaults —
    /// agree here, and so do the Tool and Workflow paths for equivalent
    /// authorized inputs.
    ///
    /// It is **derived**, not stored, and that is deliberate. Every input is
    /// already part of this frozen contract, so the identity is frozen
    /// exactly as strongly as the contract is, while a second stored copy
    /// could disagree with the very specification it labels. The child
    /// recomputes the same value from the same frozen bytes; it never
    /// consults configuration, a role file, or a later generation to do so.
    ///
    /// It is identity and diagnostic/recovery correlation over an
    /// already-authorized contract, and never an authorization token: no
    /// resolution, admission, or execution path consults it to decide what a
    /// child may do.
    ///
    /// See [`SubagentExecutionProfileDigest`] for the exact included and
    /// excluded fields.
    #[must_use]
    pub fn profile_digest(&self) -> SubagentExecutionProfileDigest {
        compute_profile_digest(&ProfileFraming {
            agent: &self.agent,
            definition_digest: &self.definition_digest,
            execution_deadline: self.execution_deadline,
            workspace_policy: self.workspace_policy,
            instructions: &self.instructions,
            model: &self.model,
            tools: &self.tools,
            skills: &self.skills,
            project_instructions: &self.project_instructions,
            materialization: &self.materialization,
            extensions: &self.extensions,
        })
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
    /// The invocation override is structurally invalid, independently of any
    /// authority or availability question (Issue #258).
    InvalidOverride {
        /// The structural violation.
        error: SubagentOverrideError,
    },
    /// The invocation override requests a capability the caller may not
    /// delegate: it is neither part of the named role's own authority nor
    /// part of the invoking model's frozen admitted execution profile
    /// (Issue #258).
    ///
    /// This is emphatically **not** "unknown": the runtime generation
    /// authorizes the capability, and some other caller could legitimately
    /// select it. This caller cannot.
    UnauthorizedTool {
        /// The offending selector.
        selector: String,
    },
    /// The invocation override requests a Skill the caller may not delegate.
    UnauthorizedSkill {
        /// The offending Skill name.
        skill: String,
    },
    /// The invocation override requests an extension composition the caller
    /// may not delegate.
    ///
    /// Naming an extension is not permission to configure it arbitrarily, so
    /// this also covers a configuration the caller's own authority does not
    /// hold — a contributor switched on that neither the role nor the
    /// invoking Agent switched on, or a timezone neither one renders.
    UnauthorizedExtension {
        /// The offending extension.
        extension: String,
        /// The bounded reason the delegation ceiling does not cover it.
        detail: String,
    },
    /// The effective extension composition names an extension a one-shot
    /// child cannot own. Authorization and child-scope support are
    /// independent checks, and this one fails even for a fully entitled
    /// caller.
    ExtensionScopeUnsupported {
        /// The offending extension.
        extension: String,
        /// The bounded reason the one-shot child scope cannot own it.
        reason: String,
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
                "the invocation requires {selector}, but capability source {source} is \
                 unavailable in this runtime generation: {reason}"
            ),
            Self::UnknownCapability { selector } => write!(
                formatter,
                "the invocation requires {selector}, which this runtime generation does not \
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
            Self::InvalidOverride { error } => {
                write!(formatter, "invalid subagent invocation override: {error}")
            }
            Self::UnauthorizedTool { selector } => write!(
                formatter,
                "the invocation override requests {selector}, which is neither part of this \
                 agent's own authority nor part of the invoking agent's frozen capabilities"
            ),
            Self::UnauthorizedSkill { skill } => write!(
                formatter,
                "the invocation override requests Skill {skill:?}, which is neither part of \
                 this agent's own authority nor visible to the invoking agent"
            ),
            Self::UnauthorizedExtension { extension, detail } => write!(
                formatter,
                "the invocation override requests extension {extension:?} beyond the caller's \
                 delegation authority: {detail}"
            ),
            Self::ExtensionScopeUnsupported { extension, reason } => write!(
                formatter,
                "extension {extension:?} is not supported by one-shot subagent execution: \
                 {reason}"
            ),
        }
    }
}

impl std::error::Error for SubagentResolutionError {}

/// The independent profile-admission domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentDomain {
    /// Profiles callable by the main Agent's `subagent` capability.
    Main,
    /// Profiles callable by Workflow Agent and Parallel nodes.
    Workflow,
}

/// **Who** may ask for an invocation-scoped override (Issue #258).
///
/// This is a typed native input supplied by the launch site, never a field of
/// the payload being authorized. A model emits `override`; it cannot emit the
/// authority under which its `override` is judged, cannot select an admission
/// domain, and cannot supply an authority snapshot of its own.
///
/// ```text
/// main `subagent` Tool   ->  DelegatedByModel   ceiling = role ∪ invoking Agent
/// Workflow Agent node    ->  TrustedProgram     ceiling = the admitted generation
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentOverrideAuthority {
    /// Model-generated input, bounded by the dynamic delegation ceiling.
    ///
    /// For each dimension `d`:
    ///
    /// ```text
    /// allowed[d] = authorized_role_baseline[d] ∪ invoking_agent_frozen_authority[d]
    /// ```
    ///
    /// The union is an authorization **ceiling**, not a merge: it decides
    /// what may be requested, never what the child ends up selecting.
    DelegatedByModel,
    /// Trusted static program data admitted by the caller's own generation.
    ///
    /// A Workflow Agent node's override is part of the compiled program, is
    /// validated at compilation and at resource-generation preparation
    /// against that generation's authority, and is not reachable from model
    /// output, node input values, or task text. It may therefore legitimately
    /// exceed both the role's defaults and the invoking main model's active
    /// capabilities. It never exceeds the admitted generation: every
    /// selection still resolves through the same catalog, availability, and
    /// Skill visibility rules.
    TrustedProgram,
}

/// The invoking Agent's **frozen admitted execution profile** — the only
/// parent-side contribution to the dynamic delegation ceiling (Issue #258).
///
/// Every field is captured from values the invoking attempt was admitted
/// with, at the same admission linearization that froze its resource
/// generation:
///
/// ```text
/// tools       CapabilitySnapshot::tool_registry()       the exact model-facing
///                                                       registry of this attempt,
///                                                       NOT available_tools()
/// skills      CapabilitySnapshot::model_skill_entries() the Skills this model can
///                                                       actually see, which is empty
///                                                       when the #234 Read gate is shut
/// extensions  the composition the invoking runtime is executing against
/// ```
///
/// The distinctions are load-bearing. `available_tools()` is the whole
/// generation's catalog and would let a model delegate a capability it was
/// never admitted with. A live registry would let a reload widen an
/// already-admitted attempt. Capabilities held only by another role, or
/// merely compiled into the executable, appear in neither.
///
/// Authority is compared by **exact native identity** — `ToolId`, and
/// `SkillId` + `SkillVersionId` — never by model-facing name, so a Builtin
/// `search` can never stand in for an MCP server's `search`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvokingAgentAuthority {
    tools: BTreeSet<ToolId>,
    skills: BTreeSet<(SkillId, SkillVersionId)>,
    extensions: NativeAgentExtensions,
}

impl InvokingAgentAuthority {
    /// Captures one attempt's frozen model-facing profile.
    ///
    /// `capability` must be the snapshot the invoking attempt holds a lease
    /// on, and `extensions` the composition its runtime is executing against.
    #[must_use]
    pub fn frozen(capability: &CapabilitySnapshot, extensions: NativeAgentExtensions) -> Self {
        let visible: BTreeSet<&str> = capability
            .model_skill_entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        Self {
            tools: capability
                .tool_registry()
                .definitions()
                .into_iter()
                .map(|definition| definition.id)
                .collect(),
            skills: capability
                .skills()
                .packages()
                .iter()
                .filter(|package| visible.contains(package.name()))
                .map(|package| (package.id().clone(), package.version_id().clone()))
                .collect(),
            extensions,
        }
    }

    /// The authority of a caller that holds no model-facing profile of its
    /// own, and can therefore delegate nothing beyond the role baseline.
    #[must_use]
    pub fn none() -> Self {
        Self {
            tools: BTreeSet::new(),
            skills: BTreeSet::new(),
            extensions: NativeAgentExtensions::none(),
        }
    }

    /// Builds one authority directly from exact identities.
    ///
    /// Production reaches this type only through [`Self::frozen`], so that a
    /// parent contribution can never be assembled from anything but a real
    /// frozen snapshot. The direct form exists for the resolver's own unit
    /// tests, where the identities under test are the point and composing a
    /// whole runtime would obscure it.
    #[cfg(test)]
    pub(crate) fn from_identities(
        tools: impl IntoIterator<Item = ToolId>,
        skills: impl IntoIterator<Item = (SkillId, SkillVersionId)>,
        extensions: NativeAgentExtensions,
    ) -> Self {
        Self {
            tools: tools.into_iter().collect(),
            skills: skills.into_iter().collect(),
            extensions,
        }
    }
}

/// One complete resolution request (Issue #258).
///
/// Every input the frozen contract depends on is named here explicitly, so
/// both launch sites reach the same algorithm with the same shape and neither
/// can smuggle in an implicit authority source.
pub struct SubagentResolution<'a> {
    /// The invoking attempt's immutable runtime resource generation.
    pub resources: &'a RuntimeResourceSnapshot,
    /// The named role to resolve.
    pub agent: &'a SubagentName,
    /// The invoking attempt's **frozen effective** model configuration.
    pub attempt_model: &'a SessionModelConfig,
    /// The admitted model binding authority of that same generation.
    pub models: &'a ModelBindingRegistry,
    /// The independent profile-admission domain of this launch site.
    pub domain: SubagentDomain,
    /// The invocation-scoped capability override, when the caller supplied
    /// one. `None` and an empty override are deliberately equivalent.
    pub invocation: Option<&'a SubagentInvocationOverride>,
    /// The caller's typed delegation authority mode.
    pub authority: SubagentOverrideAuthority,
    /// The invoking Agent's frozen admitted execution profile. It is read
    /// only under [`SubagentOverrideAuthority::DelegatedByModel`].
    pub invoking: &'a InvokingAgentAuthority,
}

/// The one resolution core shared by every capability origin and by both
/// resolution callers (invocation-time resolution and admission-time
/// validation of a prepared generation).
pub struct SubagentResolver;

impl SubagentResolver {
    /// Resolves and freezes one child execution contract.
    ///
    /// The order is the contract:
    ///
    /// ```text
    /// 1. admission          is this role callable in this domain at all
    /// 2. structural         is the override a well-formed child selection
    /// 3. replacement        effective[d] = override[d] if present else definition[d]
    /// 4. dependency         do the EFFECTIVE selections resolve in this generation
    /// 5. authorization      may this caller delegate the effective selections
    /// 6. child scope        can a one-shot child own the effective extensions
    /// 7. freeze             model, instructions, guidance, materialization, digest
    /// ```
    ///
    /// Step 3 preceding step 4 is what makes a replaced-away default stop
    /// being a requirement: a role whose default MCP tool is offline still
    /// starts when the invocation replaced that dimension, because the
    /// offline selector is no longer part of the effective invocation at all.
    /// The role's separate catalog-admission validation is untouched by this
    /// and still rejects a statically invalid definition.
    ///
    /// Every failure is decided here, before any child process is staged and
    /// long before durable ownership commits.
    ///
    /// # Errors
    ///
    /// Returns the first typed [`SubagentResolutionError`].
    pub fn resolve(
        request: &SubagentResolution<'_>,
    ) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
        let resources = request.resources;
        let catalog = resources.subagents();
        let admission = match request.domain {
            SubagentDomain::Main => resources.subagent_main_admission(),
            SubagentDomain::Workflow => resources.subagent_workflow_admission(),
        };
        let definition = catalog
            .get(request.agent)
            .filter(|_| admission.contains(request.agent))
            .ok_or_else(|| SubagentResolutionError::UnknownAgent {
                agent: request.agent.as_str().to_owned(),
                available: admission
                    .iter()
                    .map(|name| name.as_str().to_owned())
                    .collect(),
            })?;
        let empty = SubagentInvocationOverride::default();
        let invocation = request.invocation.unwrap_or(&empty);
        invocation
            .validate_spelling()
            .map_err(|error| SubagentResolutionError::InvalidOverride { error })?;

        let capability = resources.capability();
        // Replacement happens before the effective invocation's dependencies
        // are resolved, so a replaced-away default is not a requirement.
        let selected_tools = invocation.effective_tools(definition);
        let selected_skills = invocation.effective_skills(definition);
        let extensions = invocation.effective_extensions(definition);

        let tools = resolve_tools(
            &selected_tools,
            capability.available_tools(),
            resources.capability_availability(),
        )?;
        let skills = resolve_skills(&selected_skills, capability.skills())?;

        if request.authority == SubagentOverrideAuthority::DelegatedByModel {
            authorize_delegation(
                invocation,
                definition,
                &tools,
                &skills,
                &extensions,
                capability.available_tools(),
                capability.skills(),
                resources.capability_availability(),
                request.invoking,
            )?;
        }
        if let Some(unsupported) = crate::extensions::unsupported_child_scope(&extensions) {
            return Err(SubagentResolutionError::ExtensionScopeUnsupported {
                extension: unsupported.extension.to_owned(),
                reason: unsupported.reason.to_owned(),
            });
        }

        let model = resolve_model(definition, request.attempt_model, request.models)?;
        let project_instructions = resolve_project_instructions(definition, resources);
        let materialization = resolve_materialization(&tools, capability.mcp_servers())?;
        Ok(ResolvedSubagentSpec {
            agent: definition.name().clone(),
            definition_digest: definition.digest().clone(),
            execution_deadline: definition.execution_deadline(),
            workspace_policy: definition.workspace_policy(),
            instructions: definition.instructions().to_owned(),
            model,
            tools,
            skills,
            project_instructions,
            materialization,
            extensions,
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
            validate_selectors_for_admission(definition, available_tools, availability)
                .map_err(named)?;
            Self::validate_definition_local_references(definition, skills, &mut |model| {
                FrozenModelSpec::freeze(models, &SessionModelConfig::of(model.clone()))
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .map_err(named)?;
        }
        Ok(())
    }

    /// Validate local Skill/model references independently of physical binding.
    pub(crate) fn validate_local_references(
        catalog: &SubagentCatalog,
        skills: &SkillSnapshot,
        mut model_check: impl FnMut(&crate::model::catalog::ModelRef) -> Result<(), String>,
    ) -> Result<(), (SubagentName, SubagentResolutionError)> {
        for definition in catalog.definitions() {
            let named = |error: SubagentResolutionError| (definition.name().clone(), error);
            Self::validate_definition_local_references(definition, skills, &mut model_check)
                .map_err(named)?;
        }
        Ok(())
    }

    fn validate_definition_local_references(
        definition: &SubagentDefinition,
        skills: &SkillSnapshot,
        model_check: &mut impl FnMut(&crate::model::catalog::ModelRef) -> Result<(), String>,
    ) -> Result<(), SubagentResolutionError> {
        resolve_skills(definition.skills(), skills)?;
        if let Some(model) = definition.model() {
            model_check(model).map_err(|detail| SubagentResolutionError::UnknownModel {
                model: model.to_string(),
                detail,
            })?;
        }
        Ok(())
    }
}

/// Subagent-specific admission and physical identity freezing after generic
/// capability selection. Source matching and availability classification belong
/// exclusively to `capabilities::selection`; both Subagent call sites consume
/// that owner here before freezing the child launch identity.
fn resolve_selector(
    selector: &ToolSelector,
    available: &AvailableToolCatalog,
    availability: &CapabilityAvailability,
) -> Result<ResolvedSubagentTool, SubagentResolutionError> {
    let definition =
        crate::capabilities::selection::resolve_selector(selector, available, availability)
            .map_err(|error| match error {
                crate::capabilities::selection::ToolSelectionError::SourceUnavailable {
                    selector,
                    source,
                    reason,
                } => SubagentResolutionError::SourceUnavailable {
                    selector,
                    source,
                    reason,
                },
                crate::capabilities::selection::ToolSelectionError::UnknownCapability {
                    selector,
                } => SubagentResolutionError::UnknownCapability { selector },
            })?;
    if definition
        .id
        .as_str()
        .starts_with(crate::runtime::workflow::WORKFLOW_TOOL_ID_PREFIX)
    {
        return Err(SubagentResolutionError::UnknownCapability {
            selector: selector.canonical(),
        });
    }
    Ok(freeze_tool(selector, definition))
}

/// **Invocation-time** resolution of the *effective* selection: fail fast on
/// the first selector that cannot be satisfied, for any reason. A child must
/// never start weaker than the invocation it was authorized with.
///
/// The input is the effective selection — the definition's own selectors, or
/// the ones an invocation override replaced them with — never the definition
/// directly. A requested capability is therefore never silently dropped, and
/// a *replaced-away* default is never required.
fn resolve_tools(
    selected: &[ToolSelector],
    available: &AvailableToolCatalog,
    availability: &CapabilityAvailability,
) -> Result<Vec<ResolvedSubagentTool>, SubagentResolutionError> {
    selected
        .iter()
        .map(|selector| resolve_selector(selector, available, availability))
        .collect()
}

/// The exact native identities the **named role** itself authorizes.
///
/// A role's own selection is legitimate authority even when the invoking
/// model does not expose that capability: a named role is an independent
/// projection of the generation, and requiring every role default to appear
/// in the parent's registry would regress existing named-role behavior.
///
/// A role selector whose source is unavailable in this generation
/// contributes nothing here, and needs to contribute nothing: a caller
/// requesting that same selector fails earlier, at dependency resolution,
/// with the source-availability fact rather than an authority verdict.
fn role_tool_authority(
    definition: &SubagentDefinition,
    available: &AvailableToolCatalog,
    availability: &CapabilityAvailability,
) -> BTreeSet<ToolId> {
    definition
        .tools()
        .iter()
        .filter_map(|selector| {
            crate::capabilities::selection::resolve_selector(selector, available, availability).ok()
        })
        .map(|definition| definition.id.clone())
        .collect()
}

/// The exact Skill identities the **named role** itself authorizes.
fn role_skill_authority(
    definition: &SubagentDefinition,
    skills: &SkillSnapshot,
) -> BTreeSet<(SkillId, SkillVersionId)> {
    definition
        .skills()
        .iter()
        .filter_map(|selected| {
            skills
                .packages()
                .iter()
                .find(|package| package.name() == selected)
        })
        .map(|package| (package.id().clone(), package.version_id().clone()))
        .collect()
}

/// The dynamic delegation ceiling of a model-generated override.
///
/// ```text
/// allowed[d] = authorized_role_baseline[d] ∪ invoking_agent_frozen_authority[d]
/// ```
///
/// Three properties of this function are the whole point:
///
/// - it runs **per dimension**. Tools, Skills, and extensions are separate
///   authorization domains, and holding one never implies holding another;
/// - it compares **exact native identities**, never model-facing names or
///   role prose, so a same-named capability from another source cannot
///   substitute for an authorized one;
/// - it runs only over the dimensions the caller actually *overrode*. A
///   dimension that came from the definition is already the role's own
///   authority by construction, so re-checking it against the parent's
///   registry would be exactly the named-role regression this contract
///   forbids.
#[allow(clippy::too_many_arguments)] // one authorization boundary, three domains
fn authorize_delegation(
    invocation: &SubagentInvocationOverride,
    definition: &SubagentDefinition,
    tools: &[ResolvedSubagentTool],
    skills: &[ResolvedSubagentSkill],
    extensions: &crate::extensions::NativeAgentExtensions,
    available: &AvailableToolCatalog,
    admitted_skills: &SkillSnapshot,
    availability: &CapabilityAvailability,
    invoking: &InvokingAgentAuthority,
) -> Result<(), SubagentResolutionError> {
    if invocation.tools.is_some() {
        let allowed: BTreeSet<ToolId> = role_tool_authority(definition, available, availability)
            .union(&invoking.tools)
            .cloned()
            .collect();
        for tool in tools {
            let id = match tool {
                ResolvedSubagentTool::Builtin { tool_id, .. }
                | ResolvedSubagentTool::Mcp { tool_id, .. } => tool_id,
            };
            if !allowed.contains(id) {
                return Err(SubagentResolutionError::UnauthorizedTool {
                    selector: tool.canonical(),
                });
            }
        }
    }
    if invocation.skills.is_some() {
        let allowed: BTreeSet<(SkillId, SkillVersionId)> =
            role_skill_authority(definition, admitted_skills)
                .union(&invoking.skills)
                .cloned()
                .collect();
        for skill in skills {
            let identity = (
                skill.binding.skill_id.clone(),
                skill.binding.version_id.clone(),
            );
            if !allowed.contains(&identity) {
                return Err(SubagentResolutionError::UnauthorizedSkill {
                    skill: skill.catalog_entry.name.clone(),
                });
            }
        }
    }
    if invocation.extensions.is_some()
        && !definition.extensions().authorizes(extensions)
        && !invoking.extensions.authorizes(extensions)
    {
        return Err(SubagentResolutionError::UnauthorizedExtension {
            extension: "agentStatus".to_owned(),
            detail: "neither this agent's own composition nor the invoking agent's composition \
                     authorizes the requested extension configuration"
                .to_owned(),
        });
    }
    Ok(())
}

/// **Admission-time** validation: inspect *every* selector of a definition
/// and reject the candidate generation for any static invalidity, wherever
/// it appears in canonical order.
///
/// The two failure classes are deliberately not symmetric:
///
/// ```text
/// source authority absent  -> tolerate THAT selector, keep validating
///                             (the runtime stays healthy; only an
///                              invocation needing it fails)
/// source authority present
///   but selection unknown  -> reject the candidate generation
/// ```
///
/// Tolerating an unavailable source must therefore never *stop* validation:
/// an offline MCP server listed before a misspelled Builtin selector or a
/// `python:<folder>` id naming no managed package would otherwise smuggle a
/// statically invalid definition into a published generation.
fn validate_selectors_for_admission(
    definition: &SubagentDefinition,
    available: &AvailableToolCatalog,
    availability: &CapabilityAvailability,
) -> Result<(), SubagentResolutionError> {
    validate_metadata_selectors(definition, &available.definitions(), availability)
}

pub(crate) fn validate_metadata_selectors(
    definition: &SubagentDefinition,
    available: &[crate::tools::types::ToolDefinition],
    availability: &CapabilityAvailability,
) -> Result<(), SubagentResolutionError> {
    for selector in definition.tools() {
        match crate::capabilities::selection::resolve_metadata(selector, available, availability) {
            Ok(selected)
                if !selected
                    .id
                    .as_str()
                    .starts_with(crate::runtime::workflow::WORKFLOW_TOOL_ID_PREFIX) => {}
            Err(crate::capabilities::selection::ToolSelectionError::SourceUnavailable {
                ..
            }) => {}
            _ => {
                return Err(SubagentResolutionError::UnknownCapability {
                    selector: selector.canonical(),
                });
            }
        }
    }
    Ok(())
}

/// Freezes one admitted definition into its exact source-qualified identity.
fn freeze_tool(selector: &ToolSelector, definition: &ToolDefinition) -> ResolvedSubagentTool {
    match (selector, &definition.origin) {
        (ToolSelector::Mcp { server_id, .. }, _) => ResolvedSubagentTool::Mcp {
            server_id: server_id.clone(),
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            // The expected cross-process identity is derived here, once,
            // from the exact definition this generation admitted. The child
            // recomputes it from its own catalog read and compares.
            identity: crate::tools::mcp::identity::mcp_tool_identity(
                server_id,
                &definition.name,
                &definition.description,
                &definition.input_schema,
                definition.execution_policy,
                definition.concurrency_policy,
                definition.approval_policy,
            ),
            definition: definition.clone(),
        },
        _ => ResolvedSubagentTool::Builtin {
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            definition: definition.clone(),
        },
    }
}

/// Applies the exact Skill allowlist over the generation's admitted Skills.
///
/// Two things are frozen per selected Skill, and only two:
///
/// - the immutable `SkillId` + `SkillVersionId` binding of the exact package
///   this generation admitted, which is the identity a later physical
///   materialization must realize;
/// - the model-visible catalog **metadata** of that package.
///
/// A `SKILL.md` body and its supporting resources are deliberately absent:
/// rustX progressive disclosure is preserved, so a selected Skill's content
/// is still loaded through ordinary Skill semantics rather than preloaded
/// into the child's system prompt. A Skill the generation hides from model
/// invocation fails closed rather than being silently omitted.
fn resolve_skills(
    selected_skills: &[String],
    skills: &SkillSnapshot,
) -> Result<Vec<ResolvedSubagentSkill>, SubagentResolutionError> {
    let mut resolved = Vec::with_capacity(selected_skills.len());
    for selected in selected_skills {
        let Some(package) = skills
            .packages()
            .iter()
            .find(|package| package.name() == selected)
        else {
            return Err(SubagentResolutionError::UnknownSkill {
                skill: selected.clone(),
            });
        };
        // Model visibility is a package-level fact of the generation, so it
        // is read from the package rather than inferred from the metadata
        // set the generation happened to render.
        let Some(catalog_entry) = skills
            .catalog_entries()
            .iter()
            .find(|entry| entry.name == *selected)
        else {
            return Err(SubagentResolutionError::SkillNotModelVisible {
                skill: selected.clone(),
            });
        };
        resolved.push(ResolvedSubagentSkill {
            binding: SkillBinding {
                skill_id: package.id().clone(),
                version_id: package.version_id().clone(),
            },
            catalog_entry: catalog_entry.clone(),
            source_root: package.materialization_root().to_path_buf(),
            files: package.files().to_vec(),
        });
    }
    Ok(resolved)
}

/// Freezes the physical materialization plane the selected capabilities
/// require — and only what they require.
///
/// This is where "selected-only" becomes a **structural** property rather
/// than a discipline the child has to remember: a child whose frozen
/// specification names one MCP server has no way to learn about a second
/// one, because no other binding ever crosses the boundary.
///
/// A selection whose source authority has disappeared from the generation
/// between capability admission and this freeze is refused here, before any
/// process is staged.
fn resolve_materialization(
    tools: &[ResolvedSubagentTool],
    configured: &McpServerBindings,
) -> Result<ResolvedSubagentMaterialization, SubagentResolutionError> {
    let mut mcp_servers = BTreeMap::new();
    for tool in tools {
        match tool {
            ResolvedSubagentTool::Builtin { .. } => {}
            ResolvedSubagentTool::Mcp {
                server_id, name, ..
            } => {
                if mcp_servers.contains_key(server_id) {
                    continue;
                }
                let binding = configured.get(server_id).ok_or_else(|| {
                    SubagentResolutionError::SourceUnavailable {
                        selector: format!("mcp:{server_id}/{name}"),
                        source: format!("mcp:{server_id}"),
                        reason: "the runtime generation configures no such MCP server".to_owned(),
                    }
                })?;
                mcp_servers.insert(server_id.clone(), binding.clone());
            }
        }
    }
    Ok(ResolvedSubagentMaterialization { mcp_servers })
}

/// Freezes the child's model **authority**.
///
/// An explicit selection resolves through the admitted model authority and
/// fails closed. No explicit selection inherits the invoking attempt's own
/// frozen effective configuration — never live mutable session state and
/// never a composition-time capture.
///
/// Either way the result is a completely resolved invocation, not a desired
/// configuration: the parent decides the provider binding, protocol, context
/// window, output budget, reasoning-profile semantics, effective request
/// parameters, effective capabilities, and compat metadata exactly once,
/// here, against the registry the invoking attempt was admitted with. The
/// child can then have no opinion about a `models.jsonc` that changed in the
/// meantime, because it never consults one.
fn resolve_model(
    definition: &SubagentDefinition,
    attempt_model: &SessionModelConfig,
    models: &ModelBindingRegistry,
) -> Result<FrozenModelSpec, SubagentResolutionError> {
    let configured = match definition.model() {
        None => attempt_model.clone(),
        Some(model) => SessionModelConfig::of(model.clone()),
    };
    FrozenModelSpec::freeze(models, &configured).map_err(|error| {
        SubagentResolutionError::UnknownModel {
            model: configured.model.to_string(),
            detail: error.to_string(),
        }
    })
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

/// The exact inputs of the effective-profile framing.
///
/// Passing them as one named value rather than a long argument list keeps the
/// framing auditable: every field of this struct is in the preimage, and
/// nothing that is not a field of this struct can be.
struct ProfileFraming<'a> {
    agent: &'a SubagentName,
    definition_digest: &'a SubagentDefinitionDigest,
    execution_deadline: Option<SubagentExecutionDeadline>,
    workspace_policy: WorkspacePolicy,
    instructions: &'a str,
    model: &'a FrozenModelSpec,
    tools: &'a [ResolvedSubagentTool],
    skills: &'a [ResolvedSubagentSkill],
    project_instructions: &'a [ProjectContextFile],
    materialization: &'a ResolvedSubagentMaterialization,
    extensions: &'a crate::extensions::NativeAgentExtensions,
}

/// Computes the deterministic effective-profile digest.
///
/// The preimage is the same line-oriented, length-prefixed encoding the
/// definition digest uses, so no two distinct profiles can frame to one
/// preimage by concatenation. See [`SubagentExecutionProfileDigest`] for the
/// complete included/excluded field contract.
#[allow(clippy::too_many_lines)] // the framing IS the semantic input boundary
fn compute_profile_digest(framing: &ProfileFraming<'_>) -> SubagentExecutionProfileDigest {
    let mut hasher = Sha256::new();
    hasher.update(SUBAGENT_EXECUTION_PROFILE_DIGEST_VERSION.as_bytes());
    hasher.update(b"\n");
    field(&mut hasher, "agent", framing.agent.as_str());
    field(
        &mut hasher,
        "definition_digest",
        framing.definition_digest.as_str(),
    );
    field(&mut hasher, "instructions", framing.instructions);
    match framing.execution_deadline {
        None => field(&mut hasher, "execution_deadline", "\u{0}absent"),
        Some(deadline) => field(
            &mut hasher,
            "execution_deadline",
            &format!("millis:{}", deadline.as_millis()),
        ),
    }
    let workspace = match framing.workspace_policy {
        WorkspacePolicy::SharedWorkspace => "shared".to_owned(),
        WorkspacePolicy::GitWorktree {
            require_clean_parent,
        } => format!("git_worktree:require_clean_parent={require_clean_parent}"),
    };
    field(&mut hasher, "workspace_policy", &workspace);
    // The frozen model *decision*, never the provider binding: an endpoint
    // and a credential source are physical materialization inputs, and a
    // digest the runtime projects must not take credential material into its
    // preimage or change when a key rotates.
    let primary = &framing.model.primary;
    field(&mut hasher, "model", &primary.model.to_string());
    field(
        &mut hasher,
        "model_protocol",
        &format!("{:?}", primary.protocol),
    );
    field(
        &mut hasher,
        "model_context_window",
        &primary.context_window.to_string(),
    );
    field(
        &mut hasher,
        "model_max_output_tokens",
        &primary.model_max_output_tokens.to_string(),
    );
    field(
        &mut hasher,
        "effective_max_output_tokens",
        &primary.max_output_tokens.to_string(),
    );
    match &primary.reasoning_profile {
        None => field(&mut hasher, "reasoning_profile", "\u{0}absent"),
        Some(profile) => field(&mut hasher, "reasoning_profile", profile.as_str()),
    }
    field(
        &mut hasher,
        "reasoning_enabled",
        &primary.reasoning_enabled.to_string(),
    );
    field(
        &mut hasher,
        "model_capabilities",
        &canonical_json(&primary.capabilities),
    );
    field(
        &mut hasher,
        "model_declared_capabilities",
        &canonical_json(&primary.declared_capabilities),
    );
    field(
        &mut hasher,
        "model_compat",
        &canonical_json(&primary.compat),
    );
    field(
        &mut hasher,
        "model_request_params",
        &canonical_json(&primary.request_params),
    );
    field(
        &mut hasher,
        "model_summary",
        &canonical_summary(&framing.model.summary),
    );
    // Tool and Skill selections are semantically unordered and arrive
    // canonically ordered and deduplicated, so the framing preserves that
    // order without imposing a second one.
    count(&mut hasher, "tools", framing.tools.len());
    for tool in framing.tools {
        match tool {
            ResolvedSubagentTool::Builtin { tool_id, name, .. } => {
                field(
                    &mut hasher,
                    "tool",
                    &format!("builtin:{}:{name}", tool_id.as_str()),
                );
            }
            ResolvedSubagentTool::Mcp {
                server_id,
                tool_id,
                name,
                identity,
                ..
            } => field(
                &mut hasher,
                "tool",
                &format!(
                    "mcp:{server_id}:{}:{name}:{}",
                    tool_id.as_str(),
                    identity.as_str()
                ),
            ),
        }
    }
    count(&mut hasher, "skills", framing.skills.len());
    for skill in framing.skills {
        // The immutable version binding is the identity; the host source root
        // and the child's remapped location are physical paths and are not.
        field(
            &mut hasher,
            "skill",
            &format!(
                "{}:{}:{}",
                skill.binding.skill_id.as_str(),
                skill.binding.version_id.as_str(),
                skill.catalog_entry.name
            ),
        );
    }
    // The project instruction chain's order is meaning, so it is framed
    // exactly as resolved, content included.
    count(
        &mut hasher,
        "project_instruction_files",
        framing.project_instructions.len(),
    );
    for file in framing.project_instructions {
        field(
            &mut hasher,
            "project_instruction_path",
            &file.path.display().to_string(),
        );
        field(&mut hasher, "project_instruction_content", &file.content);
    }
    count(
        &mut hasher,
        "materialization_mcp_servers",
        framing.materialization.mcp_servers.len(),
    );
    for server_id in framing.materialization.mcp_servers.keys() {
        field(
            &mut hasher,
            "materialization_mcp_server",
            server_id.as_str(),
        );
    }
    field(
        &mut hasher,
        "extensions",
        &framing.extensions.digest_framing(),
    );
    SubagentExecutionProfileDigest(format!("sha256:{:x}", hasher.finalize()))
}

/// Serializes one value into a key-ordered canonical JSON text.
///
/// `serde_json` preserves map insertion order in this build, so a plain
/// `to_string` would let two semantically identical values frame differently.
/// Recursively sorting object keys removes that freedom.
fn canonical_json<T: Serialize>(value: &T) -> String {
    fn sort(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(fields) => {
                let ordered: BTreeMap<String, serde_json::Value> = fields
                    .into_iter()
                    .map(|(key, value)| (key, sort(value)))
                    .collect();
                serde_json::Value::Object(ordered.into_iter().collect())
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(sort).collect())
            }
            value => value,
        }
    }
    serde_json::to_value(value).map_or_else(
        |_| "\u{0}unserializable".to_owned(),
        |value| sort(value).to_string(),
    )
}

/// Frames the frozen summary-model policy without its provider binding.
fn canonical_summary(summary: &crate::model::frozen::FrozenSummaryModel) -> String {
    match summary {
        crate::model::frozen::FrozenSummaryModel::Session => "session".to_owned(),
        crate::model::frozen::FrozenSummaryModel::Explicit(invocation) => format!(
            "explicit:{}:{:?}:{}",
            invocation.model, invocation.protocol, invocation.max_output_tokens
        ),
    }
}

fn field(hasher: &mut Sha256, key: &str, value: &str) {
    hasher.update(format!("{key}={}\n", value.len()).as_bytes());
    hasher.update(value.as_bytes());
    hasher.update(b"\n");
}

fn count(hasher: &mut Sha256, key: &str, value: usize) {
    hasher.update(format!("{key}#{value}\n").as_bytes());
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
        ResolvedSubagentTool, SubagentResolutionError, freeze_tool, render_agent_routing,
        validate_selectors_for_admission,
    };
    use crate::capabilities::selection::ToolSelector;
    use crate::capabilities::{
        AvailableToolCatalog, CapabilityAvailability, CapabilitySourceId, CapabilitySourceState,
    };
    use crate::runtime::identity::{McpServerId, ToolId};
    use crate::runtime::subagent::catalog::{
        SubagentCatalog, SubagentDefinition, SubagentName, SubagentProjectInstructionPolicy,
    };
    use crate::runtime::workspace::WorkspacePolicy;
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

    fn catalog(definitions: Vec<ToolDefinition>) -> AvailableToolCatalog {
        struct UnusedExecutor;
        impl crate::tools::executor::ToolExecutor for UnusedExecutor {
            fn start<'a>(
                &'a self,
                _: crate::tools::types::ToolInvocation,
                _: crate::tools::executor::ToolExecutionContext<'a>,
            ) -> crate::tools::executor::ToolExecutionHandle<'a> {
                panic!("selector tests never execute tools")
            }
            fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
                crate::tools::deadline::ToolProgressCapability::None
            }
        }
        AvailableToolCatalog::new(
            definitions
                .into_iter()
                .map(|definition| {
                    crate::tools::executor::ToolRegistration::plain(
                        definition,
                        std::sync::Arc::new(UnusedExecutor),
                    )
                })
                .collect(),
        )
    }

    fn available() -> AvailableToolCatalog {
        catalog(vec![
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
                ToolOrigin::Mcp {
                    // A managed Python package surfaces under its synthesized
                    // server identity (Issue #174).
                    server_id: McpServerId::new("python:symbols"),
                },
            ),
        ])
    }

    fn resolve_tools(
        definition: &SubagentDefinition,
        available: &AvailableToolCatalog,
        availability: &CapabilityAvailability,
    ) -> Result<Vec<ResolvedSubagentTool>, SubagentResolutionError> {
        super::resolve_tools(definition.tools(), available, availability)
    }

    fn definition(tools: Vec<ToolSelector>) -> SubagentDefinition {
        SubagentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            "description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/explore.md"),
            None,
            None,
            tools,
            Vec::new(),
            SubagentProjectInstructionPolicy {
                inherit: true,
                files: Vec::new(),
            },
            WorkspacePolicy::SharedWorkspace,
            crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
        )
        .expect("definition")
    }

    fn ready() -> CapabilityAvailability {
        let mut availability = CapabilityAvailability::new();
        availability.insert(
            CapabilitySourceId::Mcp(McpServerId::new("python:symbols")),
            CapabilitySourceState::Ready,
        );
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
                ToolSelector::Builtin {
                    name: "read".to_owned(),
                },
                ToolSelector::Mcp {
                    server_id: McpServerId::new("github"),
                    name: "get_issue".to_owned(),
                },
                ToolSelector::Mcp {
                    server_id: McpServerId::new("python:symbols"),
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
            ResolvedSubagentTool::Mcp { server_id, name, .. }
                if server_id.as_str() == "python:symbols" && name == "repository_symbols"
        ));
        assert_eq!(
            resolved
                .iter()
                .filter(|tool| matches!(tool, ResolvedSubagentTool::Mcp { .. }))
                .count(),
            2,
            "both externally sourced origins keep their exact source-qualified identity"
        );
    }

    /// The frozen materialization plane carries **only** the sources the
    /// selection actually needs. This is what makes "connect only the
    /// required MCP servers" structural in the child: no other binding ever
    /// crosses the boundary, so the child has nothing to widen to.
    #[test]
    fn the_materialization_plane_freezes_only_required_sources() {
        use super::{ResolvedSubagentMaterialization, resolve_materialization};
        let github = McpServerId::new("github");
        let unrelated = McpServerId::new("filesystem");
        let binding = || crate::tools::mcp::McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            activation: crate::capabilities::activation::SourceActivation::Enabled,
            resource_workspace: None,
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: "server".to_owned(),
                args: Vec::new(),
                cwd: None,
                environment: std::collections::BTreeMap::new(),
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
        };
        let configured: crate::tools::mcp::McpServerBindings =
            [(github.clone(), binding()), (unrelated.clone(), binding())]
                .into_iter()
                .collect();

        let tools = vec![
            freeze_tool(
                &ToolSelector::Builtin {
                    name: "read".to_owned(),
                },
                &tool("read", ToolOrigin::Builtin),
            ),
            freeze_tool(
                &ToolSelector::Mcp {
                    server_id: github.clone(),
                    name: "get_issue".to_owned(),
                },
                &tool(
                    "get_issue",
                    ToolOrigin::Mcp {
                        server_id: github.clone(),
                    },
                ),
            ),
        ];
        let plane = resolve_materialization(&tools, &configured).expect("plane");
        assert_eq!(
            plane.mcp_servers.keys().collect::<Vec<_>>(),
            vec![&github],
            "the unrelated configured server is never frozen for this child"
        );
        assert!(!plane.mcp_servers.contains_key(&unrelated));
        assert!(!plane.is_empty());

        // A Builtin-only agent needs no external plane whatsoever.
        let builtin_only = vec![freeze_tool(
            &ToolSelector::Builtin {
                name: "read".to_owned(),
            },
            &tool("read", ToolOrigin::Builtin),
        )];
        assert_eq!(
            resolve_materialization(&builtin_only, &configured).expect("plane"),
            ResolvedSubagentMaterialization::default()
        );
    }

    /// The frozen MCP identity is the deterministic cross-process digest of
    /// the exact admitted definition, not a restatement of its name.
    #[test]
    fn a_frozen_mcp_selection_carries_its_cross_process_identity() {
        let definition = tool(
            "get_issue",
            ToolOrigin::Mcp {
                server_id: McpServerId::new("github"),
            },
        );
        let frozen = freeze_tool(
            &ToolSelector::Mcp {
                server_id: McpServerId::new("github"),
                name: "get_issue".to_owned(),
            },
            &definition,
        );
        let ResolvedSubagentTool::Mcp { identity, .. } = &frozen else {
            panic!("an MCP selector freezes an MCP identity: {frozen:?}");
        };
        assert_eq!(
            identity,
            &crate::tools::mcp::identity::definition_identity(&definition)
                .expect("an MCP definition has an MCP identity"),
            "the frozen identity is derived from the exact admitted definition"
        );
        assert!(identity.as_str().starts_with("sha256:"));
    }

    #[test]
    fn origin_identity_is_never_collapsed_into_a_bare_name() {
        // The same bare name exists under two origins; a Builtin selector
        // must never resolve to the MCP capability and vice versa.
        let catalog = catalog(vec![
            tool("search", ToolOrigin::Builtin),
            tool(
                "search",
                ToolOrigin::Mcp {
                    server_id: McpServerId::new("github"),
                },
            ),
        ]);
        let builtin = resolve_tools(
            &definition(vec![ToolSelector::Builtin {
                name: "search".to_owned(),
            }]),
            &catalog,
            &ready(),
        )
        .expect("builtin resolution");
        assert!(matches!(builtin[0], ResolvedSubagentTool::Builtin { .. }));
        let mcp = resolve_tools(
            &definition(vec![ToolSelector::Mcp {
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
                &definition(vec![ToolSelector::Mcp {
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
        let catalog = catalog(vec![tool("read", ToolOrigin::Builtin)]);
        assert!(matches!(
            resolve_tools(
                &definition(vec![ToolSelector::Mcp {
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
                &definition(vec![ToolSelector::Builtin {
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

    // ---------------------------------------------------------------
    // Issue #258: the dynamic delegation ceiling.
    //
    // The resolver owns authorization, so the ceiling is proven here, at its
    // owner, with the exact native identities under test rather than behind a
    // composed runtime that would obscure which identity decided the verdict.
    // ---------------------------------------------------------------

    fn role_with(
        tools: Vec<ToolSelector>,
        extensions: crate::extensions::NativeAgentExtensions,
    ) -> SubagentDefinition {
        SubagentDefinition::new(
            SubagentName::parse("reviewer").expect("name"),
            "description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/reviewer.md"),
            None,
            None,
            tools,
            Vec::new(),
            SubagentProjectInstructionPolicy {
                inherit: true,
                files: Vec::new(),
            },
            WorkspacePolicy::SharedWorkspace,
            extensions,
        )
        .expect("definition")
    }

    fn requested_tools(names: &[&str]) -> super::SubagentInvocationOverride {
        super::SubagentInvocationOverride {
            tools: Some(crate::capabilities::selection::ToolSelectionDocument {
                builtin: names.iter().map(|name| (*name).to_owned()).collect(),
                mcp: std::collections::BTreeMap::new(),
            }),
            ..super::SubagentInvocationOverride::default()
        }
    }

    fn authorize(
        invocation: &super::SubagentInvocationOverride,
        definition: &SubagentDefinition,
        available: &AvailableToolCatalog,
        invoking: &super::InvokingAgentAuthority,
    ) -> Result<(), SubagentResolutionError> {
        let selected = invocation.effective_tools(definition);
        let tools = super::resolve_tools(&selected, available, &ready()).expect("resolution");
        let extensions = invocation.effective_extensions(definition);
        super::authorize_delegation(
            invocation,
            definition,
            &tools,
            &[],
            &extensions,
            available,
            &crate::skills::SkillSnapshot::new(Vec::new()),
            &ready(),
            invoking,
        )
    }

    fn tool_id_of(available: &AvailableToolCatalog, name: &str) -> ToolId {
        available
            .definitions()
            .into_iter()
            .find(|definition| {
                definition.name == name
                    && definition.origin == crate::tools::types::ToolOrigin::Builtin
            })
            .expect("the fixture catalog admits the capability")
            .id
    }

    /// The three delegable outcomes of the ceiling, on one fixture: a role
    /// default the parent does not hold, a parent capability the role does
    /// not hold, and a combination of both.
    #[test]
    fn sub258_role_authority_and_parent_authority_both_delegate() {
        let available = available();
        let role = role_with(
            vec![ToolSelector::Builtin {
                name: "read".to_owned(),
            }],
            crate::extensions::NativeAgentExtensions::none(),
        );
        // The parent holds `grep` and deliberately does NOT hold `read`: a
        // role default must stay delegable even when the invoking model does
        // not expose that capability at all.
        let parent = super::InvokingAgentAuthority::from_identities(
            [tool_id_of(&available, "grep")],
            [],
            crate::extensions::NativeAgentExtensions::none(),
        );
        assert!(
            authorize(&requested_tools(&["read"]), &role, &available, &parent).is_ok(),
            "role-only authority remains usable"
        );
        assert!(
            authorize(&requested_tools(&["grep"]), &role, &available, &parent).is_ok(),
            "parent-only frozen authority may be explicitly delegated"
        );
        assert!(
            authorize(
                &requested_tools(&["read", "grep"]),
                &role,
                &available,
                &parent
            )
            .is_ok(),
            "a valid combination of role and parent authority succeeds"
        );
    }

    /// Authority the generation knows but neither the role nor the invoking
    /// model holds is not delegable. This is the difference between the
    /// available catalog and the frozen admitted profile.
    #[test]
    fn sub258_generation_only_and_other_role_only_authority_is_rejected() {
        let available = catalog(vec![
            tool("read", ToolOrigin::Builtin),
            tool("grep", ToolOrigin::Builtin),
            tool("write", ToolOrigin::Builtin),
        ]);
        let role = role_with(
            vec![ToolSelector::Builtin {
                name: "read".to_owned(),
            }],
            crate::extensions::NativeAgentExtensions::none(),
        );
        // Another role holding `write` changes nothing: the ceiling reads
        // this role's own selection and the invoking profile, never the
        // union of every admitted definition.
        let _other_role = role_with(
            vec![ToolSelector::Builtin {
                name: "write".to_owned(),
            }],
            crate::extensions::NativeAgentExtensions::none(),
        );
        let parent = super::InvokingAgentAuthority::from_identities(
            [tool_id_of(&available, "read")],
            [],
            crate::extensions::NativeAgentExtensions::none(),
        );
        assert_eq!(
            authorize(&requested_tools(&["write"]), &role, &available, &parent),
            Err(SubagentResolutionError::UnauthorizedTool {
                selector: "builtin:write".to_owned()
            }),
            "a capability only the generation authorizes is refused, not silently dropped"
        );
    }

    /// Authority is an exact native identity. A same-named capability from
    /// another source is a different capability and cannot substitute.
    #[test]
    fn sub258_same_named_capabilities_from_different_sources_cannot_substitute() {
        let available = catalog(vec![
            tool("search", ToolOrigin::Builtin),
            tool(
                "search",
                ToolOrigin::Mcp {
                    server_id: McpServerId::new("github"),
                },
            ),
        ]);
        let role = role_with(Vec::new(), crate::extensions::NativeAgentExtensions::none());
        // The parent holds the Builtin `search` and nothing else.
        let parent = super::InvokingAgentAuthority::from_identities(
            [tool_id_of(&available, "search")],
            [],
            crate::extensions::NativeAgentExtensions::none(),
        );
        assert!(
            authorize(&requested_tools(&["search"]), &role, &available, &parent).is_ok(),
            "the exact Builtin identity the parent holds is delegable"
        );
        let mcp_request = super::SubagentInvocationOverride {
            tools: Some(crate::capabilities::selection::ToolSelectionDocument {
                builtin: Vec::new(),
                mcp: [(McpServerId::new("github"), vec!["search".to_owned()])]
                    .into_iter()
                    .collect(),
            }),
            ..super::SubagentInvocationOverride::default()
        };
        assert_eq!(
            authorize(&mcp_request, &role, &available, &parent),
            Err(SubagentResolutionError::UnauthorizedTool {
                selector: "mcp:github/search".to_owned()
            }),
            "a matching display name is not authorization"
        );
    }

    /// Tools, Skills, and extensions are separate authorization domains, and
    /// a dimension the caller did not override is never re-judged against the
    /// parent's registry.
    #[test]
    fn sub258_an_unoverridden_dimension_is_never_judged_against_parent_authority() {
        let available = available();
        let role = role_with(
            vec![ToolSelector::Builtin {
                name: "read".to_owned(),
            }],
            crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
        );
        // A caller that holds nothing at all: the role's own defaults must
        // still resolve, because they are the role's authority by
        // construction.
        let empty_parent = super::InvokingAgentAuthority::none();
        let no_override = super::SubagentInvocationOverride::default();
        assert!(
            authorize(&no_override, &role, &available, &empty_parent).is_ok(),
            "no override means no delegation question at all"
        );
        // Overriding only Skills leaves the role's tools and extensions
        // untouched and unjudged.
        let skills_only = super::SubagentInvocationOverride {
            skills: Some(Vec::new()),
            ..super::SubagentInvocationOverride::default()
        };
        assert!(authorize(&skills_only, &role, &available, &empty_parent).is_ok());
    }

    /// Naming an extension is not permission to configure it. The typed
    /// calculation is proven field by field against both authority sources.
    #[test]
    #[allow(clippy::too_many_lines)] // one exhaustive authorization matrix
    fn sub258_extension_authorization_is_configuration_exact() {
        let available = available();
        let utc = "Etc/UTC".parse::<chrono_tz::Tz>().expect("timezone");
        let shanghai = "Asia/Shanghai".parse::<chrono_tz::Tz>().expect("timezone");
        let compose = |time: bool, timezone: Option<chrono_tz::Tz>, background: bool| {
            crate::extensions::NativeAgentExtensions::with_agent_status(
                crate::context::AgentStatusConfig {
                    time: crate::context::TimeStatusConfig {
                        enabled: time,
                        timezone,
                    },
                    background: crate::context::BackgroundStatusConfig {
                        enabled: background,
                    },
                },
            )
        };
        let request = |value: serde_json::Value| super::SubagentInvocationOverride {
            extensions: Some(
                serde_json::from_value::<crate::extensions::NativeAgentExtensionSelection>(value)
                    .expect("selection parses"),
            ),
            ..super::SubagentInvocationOverride::default()
        };

        // The role composes Agent Status with Time on, UTC, Background off.
        let role = role_with(Vec::new(), compose(true, Some(utc), false));
        let bare_parent = super::InvokingAgentAuthority::none();

        // Removing the extension entirely is narrowing and always allowed.
        assert!(
            authorize(
                &request(serde_json::json!({})),
                &role,
                &available,
                &bare_parent
            )
            .is_ok(),
            "an empty extension override composes nothing and needs no authority"
        );
        // Restating the role's own composition is allowed.
        assert!(
            authorize(
                &request(serde_json::json!({
                    "agentStatus": {"time": {"enabled": true, "timezone": "Etc/UTC"},
                                    "background": {"enabled": false}}
                })),
                &role,
                &available,
                &bare_parent
            )
            .is_ok()
        );
        // Switching a contributor off is narrowing and allowed.
        assert!(
            authorize(
                &request(serde_json::json!({
                    "agentStatus": {"time": {"enabled": false}, "background": {"enabled": false}}
                })),
                &role,
                &available,
                &bare_parent
            )
            .is_ok()
        );
        // Switching a contributor the authority does not hold ON is refused.
        assert!(matches!(
            authorize(
                &request(serde_json::json!({
                    "agentStatus": {"time": {"enabled": true, "timezone": "Etc/UTC"},
                                    "background": {"enabled": true}}
                })),
                &role,
                &available,
                &bare_parent
            ),
            Err(SubagentResolutionError::UnauthorizedExtension { .. })
        ));
        // A timezone neither authority renders is refused: permission to name
        // the extension is not permission to configure it.
        assert!(matches!(
            authorize(
                &request(serde_json::json!({
                    "agentStatus": {"time": {"timezone": "Asia/Shanghai"},
                                    "background": {"enabled": false}}
                })),
                &role,
                &available,
                &bare_parent
            ),
            Err(SubagentResolutionError::UnauthorizedExtension { .. })
        ));
        // The invoking Agent's own composition is the other half of the
        // ceiling, and it authorizes exactly what it holds.
        let shanghai_parent = super::InvokingAgentAuthority::from_identities(
            [],
            [],
            compose(true, Some(shanghai), true),
        );
        assert!(
            authorize(
                &request(serde_json::json!({
                    "agentStatus": {"time": {"timezone": "Asia/Shanghai"},
                                    "background": {"enabled": true}}
                })),
                &role,
                &available,
                &shanghai_parent
            )
            .is_ok(),
            "root composition is legitimate authority for an explicit authorized override"
        );
        // A role that composes no Agent Status at all, with a caller that
        // composes none either, cannot manufacture the extension.
        let bare_role = role_with(Vec::new(), crate::extensions::NativeAgentExtensions::none());
        assert!(matches!(
            authorize(
                &request(serde_json::json!({"agentStatus": {}})),
                &bare_role,
                &available,
                &bare_parent
            ),
            Err(SubagentResolutionError::UnauthorizedExtension { .. })
        ));
    }

    /// The whole closed extension vocabulary is supported in one-shot child
    /// scope today. The assertion is deliberately exhaustive so that adding a
    /// member without deciding its scope is caught here rather than in
    /// production.
    #[test]
    fn sub258_every_supported_extension_is_child_scope_supported() {
        for composition in [
            crate::extensions::NativeAgentExtensions::none(),
            crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
            crate::extensions::NativeAgentExtensions::with_agent_status(
                crate::context::AgentStatusConfig {
                    time: crate::context::TimeStatusConfig {
                        enabled: false,
                        timezone: None,
                    },
                    background: crate::context::BackgroundStatusConfig { enabled: true },
                },
            ),
        ] {
            assert!(
                crate::extensions::unsupported_child_scope(&composition).is_none(),
                "{composition:?} must be decided for child scope"
            );
        }
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
                None,
                Vec::new(),
                Vec::new(),
                SubagentProjectInstructionPolicy {
                    inherit: true,
                    files: Vec::new(),
                },
                WorkspacePolicy::SharedWorkspace,
                crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
            )
            .expect("definition"),
            SubagentDefinition::new(
                SubagentName::parse("explore").expect("name"),
                "Read-only exploration.".to_owned(),
                "instructions".to_owned(),
                std::path::PathBuf::from("/w/explore.md"),
                None,
                None,
                Vec::new(),
                Vec::new(),
                SubagentProjectInstructionPolicy {
                    inherit: true,
                    files: Vec::new(),
                },
                WorkspacePolicy::SharedWorkspace,
                crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
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

    /// Admission and invocation are asymmetric on purpose, and the
    /// asymmetry must not degrade into "stop at the first unavailable
    /// source".
    ///
    /// The definition below lists an offline MCP selector first in canonical
    /// order followed by a statically invalid one against a *ready* source.
    /// A validator that treated the unavailable source as sufficient would
    /// never reach the invalid selector and would admit the definition.
    #[test]
    fn admission_validates_every_selector_past_an_unavailable_source() {
        let mut availability = ready();
        availability.insert(
            CapabilitySourceId::Mcp(McpServerId::new("github")),
            CapabilitySourceState::Unavailable {
                reason: "the server did not start".to_owned(),
            },
        );
        let definition = definition(vec![
            ToolSelector::Mcp {
                server_id: McpServerId::new("github"),
                name: "get_issue".to_owned(),
            },
            ToolSelector::Mcp {
                server_id: McpServerId::new("python:symbols"),
                name: "not_a_real_tool".to_owned(),
            },
        ]);
        assert_eq!(
            definition.tools().first(),
            Some(&ToolSelector::Mcp {
                server_id: McpServerId::new("github"),
                name: "get_issue".to_owned(),
            }),
            "the unavailable selector really is inspected first"
        );

        // Invocation stays fail-fast: the first unsatisfiable selector wins.
        assert!(matches!(
            resolve_tools(&definition, &available(), &availability),
            Err(SubagentResolutionError::SourceUnavailable { .. })
        ));

        // Admission keeps going and rejects the static invalidity.
        assert!(matches!(
            validate_selectors_for_admission(&definition, &available(), &availability),
            Err(SubagentResolutionError::UnknownCapability { selector })
                if selector == "mcp:python:symbols/not_a_real_tool"
        ));
    }

    /// A definition whose *only* unsatisfiable selector is an unavailable
    /// optional source stays admissible: the runtime is healthy and only an
    /// invocation of that agent fails.
    #[test]
    fn an_unavailable_source_alone_never_rejects_admission() {
        let mut availability = ready();
        availability.insert(
            CapabilitySourceId::Mcp(McpServerId::new("github")),
            CapabilitySourceState::Unavailable {
                reason: "the server did not start".to_owned(),
            },
        );
        let definition = definition(vec![
            ToolSelector::Builtin {
                name: "read".to_owned(),
            },
            ToolSelector::Mcp {
                server_id: McpServerId::new("github"),
                name: "get_issue".to_owned(),
            },
        ]);
        assert!(validate_selectors_for_admission(&definition, &available(), &availability).is_ok());
        assert!(matches!(
            resolve_tools(&definition, &available(), &availability),
            Err(SubagentResolutionError::SourceUnavailable { .. })
        ));
    }
}
