//! Child authorization and freeze boundary consuming the shared Agent Profile
//! resolved in the admitted resource generation.
//!
//! ```text
//! NamedAgentDefinition
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
//! capability-source availability of that same generation. That catalog is
//! the outer ceiling on everything below:
//!
//! ```text
//! SubagentResolvedTools ⊆ CapabilitySnapshot::available_tools()
//! ```
//!
//! A named subagent is an independent projection of the authority admitted
//! into the invoking attempt's runtime generation: it may **narrow** that
//! authority but can never manufacture authority the generation does not
//! already hold.
//!
//! ## The parent registry: frozen authority, never a live read (Issue #258)
//!
//! Resolution never consults a **live, currently mutable** parent
//! `ToolRegistry`. Doing so would let a reload widen an attempt that was
//! already admitted, and would make one resolution's outcome depend on when
//! it ran rather than on what the invoking attempt actually holds.
//!
//! It does, however, depend on the invoking attempt's model-facing registry —
//! as a **value frozen at attempt admission**, not as a reference to a
//! changing one:
//!
//! ```text
//! attempt admission     CapabilitySnapshot::tool_registry()
//!                              |  captured once, by exact ToolId
//!                              v
//!                       InvokingAgentAuthority   (a typed frozen value)
//!                              |  passed in as an argument
//!                              v
//!                       SubagentResolver::resolve
//! ```
//!
//! Three concepts stay distinct, and conflating any two of them is a
//! privilege bug:
//!
//! ```text
//! available_tools()   the whole GENERATION's catalog. The outer ceiling of
//!                     every path below, and deliberately WIDER than what the
//!                     invoking model was admitted with.
//!
//! tool_registry()     the exact model-facing registry of the INVOKING
//!                     ATTEMPT, frozen into `InvokingAgentAuthority`. It is
//!                     the ceiling on what an INVOCATION-SCOPED OVERRIDE may
//!                     add, so a main-model caller cannot delegate a
//!                     capability it was never admitted with.
//!
//! role defaults       a named definition's own selections are authorized
//!                     INDEPENDENTLY, against the generation catalog. A role
//!                     may legitimately hold a capability the invoking model
//!                     does not, so its defaults never narrow to
//!                     `InvokingAgentAuthority`.
//! ```
//!
//! So `SubagentResolvedTools ⊄ ParentActiveTools` remains true — a role's own
//! defaults are not bounded by the invoking model's registry — while an
//! override's *additions* are bounded by exactly that frozen registry. A
//! trusted Workflow program carries its own typed authority and is bounded by
//! the generation catalog instead; see [`SubagentOverrideAuthority`].
//!
//! # Complete profiles and dynamic replacements
//!
//! Complete named defaults consume the generation's shared resolved Agent
//! Profile. Valid but unavailable selections have already been diagnosed and
//! suppressed. Dynamic replacements are different: every requested capability
//! must pass the typed invocation authorization boundary before the common
//! profile resolver decides composition. An unauthorized override is refused,
//! never silently suppressed into a different request.
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
use crate::runtime::identity::{SkillId, SkillVersionId, SourceToolIdentity, ToolId};
use crate::runtime::resources::{ProjectContextFile, RuntimeResourceSnapshot};
use crate::skills::{SkillCatalogEntry, SkillSnapshot};
use crate::tools::mcp::{McpServerBinding, McpServerBindings};
use crate::tools::types::ToolDefinition;

use super::catalog::{
    AgentCatalog, NamedAgentDefinition, NamedAgentDefinitionDigest, SubagentExecutionDeadline,
    SubagentName,
};
use super::invocation::{SubagentInvocationOverride, SubagentOverrideError};
use crate::capabilities::selection::AgentToolSelection;
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
    /// One ordinary Tool published by an exact `ToolSource`.
    Source {
        /// The authoritative typed source identity.
        source_id: crate::capabilities::ToolSourceId,
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
        identity: SourceToolIdentity,
    },
}

impl ResolvedSubagentTool {
    /// The canonical model-facing name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Builtin { name, .. } | Self::Source { name, .. } => name,
        }
    }

    /// The exact admitted definition.
    #[must_use]
    pub const fn definition(&self) -> &ToolDefinition {
        match self {
            Self::Builtin { definition, .. } | Self::Source { definition, .. } => definition,
        }
    }

    /// The canonical selection text of this resolution, for diagnostics.
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Builtin { name, .. } => format!("builtin:{name}"),
            Self::Source {
                source_id, name, ..
            } => format!("source:{source_id}/{name}"),
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
/// selected source:github/get_issue -> sources = { github: <binding> }
///                                     (never every configured server)
/// selected nothing external       ->  empty
/// ```
///
/// Each binding is keyed by typed source identity. The Managed Python owner
/// has already prepared its immutable execution binding; the child reuses it
/// through the existing MCP-compatible materializer without package discovery.
///
/// The child never reads `rustx.toml` to obtain any of this: a
/// configuration edit between the parent's freeze and the child's
/// composition cannot change which server the child connects or which store
/// it opens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedSubagentMaterialization {
    /// Exactly the native materialization bindings the frozen source selection requires.
    pub sources: BTreeMap<crate::capabilities::ToolSourceId, McpServerBinding>,
}

impl ResolvedSubagentMaterialization {
    /// Whether this child needs any externally sourced execution plane at
    /// all. A child with no external requirement composes exactly the
    /// deterministic base-only plane it always did.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// The complete frozen launch specification of one named subagent child.
///
/// Every semantic identity the child needs is already decided here. The
/// child consumes this value and reinterprets nothing: it does not read
/// `rustx.toml`, discover project instructions, choose model state,
/// rediscover Skills, or widen Tool authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedSubagentSpec {
    /// The canonical agent name this child was started as.
    pub agent: SubagentName,
    /// The deterministic semantic identity of the definition at start.
    pub definition_digest: NamedAgentDefinitionDigest,
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
    /// decision physically and never reopens `models.toml` as semantic
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
/// # The rule the framing obeys
///
/// > The digest identifies the semantic final frozen execution profile, not
/// > its authoring history, and no behavior-affecting frozen field may be
/// > omitted.
///
/// Both halves are load-bearing, and they are why this is not
/// [`NamedAgentDefinitionDigest`]:
///
/// ```text
/// definition_digest  identity of the SOURCE named definition
///                    (its routing description, its DEFAULT tool/Skill/
///                    extension selections, its authored spellings)
///
/// profile_digest     identity of the FINAL FROZEN effective child
///                    execution contract, after replacement
/// ```
///
/// A default that an invocation completely replaced is authoring history and
/// must not survive into this identity; a routing description never executes
/// at all. So `definition_digest` is deliberately **not** in this preimage,
/// even though [`ResolvedSubagentSpec`] carries it as independent source
/// provenance. Every behavior-affecting field it summarizes — instructions,
/// deadline, workspace policy, project instruction chain, the effective tool
/// and Skill selections, the effective extension composition — is framed here
/// directly, as its final frozen value.
///
/// # What the framing covers
///
/// ```text
/// agent name                 the named role identity the child runs as
/// instructions               the exact child instruction document
/// execution deadline         the frozen whole-lifecycle bound
/// workspace policy           shared workspace vs Git worktree + its flag
/// frozen model decision      the complete non-binding semantics of the
///                            primary invocation (see
///                            `frame_frozen_model_invocation`)
/// frozen summary policy      "follows the session primary" vs an explicit
///                            invocation framed by the SAME helper, so an
///                            explicit summary model is framed exactly as
///                            completely as the primary one
/// effective tools            origin, exact ToolId, model-facing name, the
///                            COMPLETE frozen ToolDefinition the child
///                            executes (description, canonical input schema,
///                            and the execution/concurrency/approval/replay
///                            policies), and — for an MCP tool — its frozen
///                            cross-process identity (see
///                            `frame_tool_definition`)
/// effective Skills           exact SkillId + SkillVersionId binding and the
///                            model-visible name AND description, both of
///                            which cross to the child verbatim (see
///                            `compute_profile_digest`)
/// project instruction chain  path AND content, in order
/// materialization plane      exactly the external source identities the
///                            effective selection requires. The bindings
///                            behind them are physical (transport, resource
///                            root) or secret (credentials); the one part
///                            that IS behavior — the invocation policy each
///                            server imposes on its tools — is already framed
///                            exactly, through each MCP tool's cross-process
///                            identity above
/// effective extensions       the closed composition's EFFECTIVE framing:
///                            an omitted timezone frames as the UTC it
///                            actually renders, and a DISABLED Time
///                            contributor's timezone frames as one inactive
///                            sentinel because no zone executes
/// ```
///
/// # What the framing deliberately excludes
///
/// ```text
/// source-definition-only     definition_digest, and with it the role's
///   provenance               routing description and its replaced-away
///                            default tool/Skill/extension selections
/// desired model config       FrozenModelSpec::configured is the descriptive
///                            record of what was ASKED for; the resolved
///                            invocations are the authority and are framed
/// provider binding material  provider id, endpoint, credential source, and
///                            any admitted credential value
/// execution identities       SubagentId, conversation/agent ids, tool call id
/// timestamps                 nothing time-derived enters the preimage
/// physical paths             the staging root, the Skill source root, the
///                            child's remapped Skill locations, the MCP
///                            server's physical launch plane
/// raw payload formatting     the override JSON's key order, whitespace,
///                            duplicate entries, or whether an override was
///                            written at all
/// ```
///
/// Excluding the raw payload is the whole point: the digest names the
/// *profile*, so a caller cannot make two identical children look different
/// by respelling their request, and cannot make two different children look
/// identical either. Excluding provider binding material keeps credential
/// data out of the preimage of a value the runtime projects and durably
/// commits — and it is a real exclusion, not an omission: rotating a key or
/// repointing an endpoint leaves the identity unchanged.
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
///
/// The revisions so far, each an *interpretation* change rather than an added
/// field, which is why each took a new version rather than a compatibility
/// mode — there is exactly one canonical reading of a given version:
///
/// ```text
/// v1 -> v2  stopped folding `definition_digest` into the preimage (an
///           EFFECTIVE profile identity must not depend on
///           source-definition-only provenance), and framed an explicit
///           summary invocation as completely as the primary one.
///
/// v2 -> v3  frames the COMPLETE frozen `ToolDefinition` of every resolved
///           Tool instead of `builtin:{tool_id}:{name}` (a stable capability
///           id is not a digest of the semantics it was frozen with), frames
///           a Skill's model-visible description (which crosses to the child
///           verbatim rather than being re-derived from `version_id`), and
///           stops letting a DISABLED Time contributor's timezone
///           distinguish two behaviorally identical profiles.
/// ```
///
/// Both `v2 -> v3` corrections change what an existing preimage means: the
/// same frozen specification hashes to a different value, and pairs that
/// collided under `v2` no longer do. Since `profile_digest` is durably
/// committed at ownership and never recomputed, a stale `v2` row must be
/// visibly a different framing rather than silently reinterpreted under the
/// corrected rules — so the version moves even though `v2` never shipped and
/// no compatibility mode exists.
pub const SUBAGENT_EXECUTION_PROFILE_DIGEST_VERSION: &str = "rustx-subagent-profile-v3";

impl SubagentExecutionProfileDigest {
    /// The stable textual form `sha256:<64 lowercase hex characters>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Restores this identity from the durable ownership fact that committed
    /// it.
    ///
    /// This is the **only** way to obtain a profile digest without a frozen
    /// [`ResolvedSubagentSpec`] in hand, and it exists so recovery can restore
    /// what a child actually started with. Recovery must never recompute the
    /// value: the role definition and the resource generation are both mutable
    /// and may have changed since the commit, so recomputation would silently
    /// relabel an already-running child.
    #[must_use]
    pub fn from_committed_fact(committed: String) -> Self {
        Self(committed)
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
        // Destructured for the same reason the framing helpers are: a new
        // field of the frozen contract cannot reach a child without a
        // deliberate decision about whether it identifies the child.
        let Self {
            agent,
            // The one deliberate exclusion, and the whole point of the split:
            // source-definition provenance is not effective-execution
            // identity. See `SubagentExecutionProfileDigest`.
            definition_digest: _,
            execution_deadline,
            workspace_policy,
            instructions,
            model,
            tools,
            skills,
            project_instructions,
            materialization,
            extensions,
        } = self;
        compute_profile_digest(&ProfileFraming {
            agent,
            execution_deadline: *execution_deadline,
            workspace_policy: *workspace_policy,
            instructions,
            model,
            tools,
            skills,
            project_instructions,
            materialization,
            extensions,
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
    /// this also covers a configuration no authority source holds — a
    /// contributor switched on that neither the role nor the invoking Agent
    /// switched on, or a timezone neither one *effectively renders*. The
    /// detail names the first uncovered contributor, not the whole
    /// composition, because the ceiling is a per-contributor union.
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
    /// # Panics
    /// Panics if an internally constructed generation violates catalog/profile coherence.
    #[allow(
        clippy::too_many_lines,
        reason = "one child authorization and freeze boundary"
    )]
    pub fn resolve(
        request: &SubagentResolution<'_>,
    ) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
        let resources = request.resources;
        let catalog = resources.subagents();
        let admission = match request.domain {
            SubagentDomain::Main => resources.delegatable_agents(),
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

        // A requested invocation replacement remains a strict contract. Profile
        // suppression is never used to make an invalid/unauthorized override succeed.
        if invocation.tools.is_some() {
            resolve_tools(
                &selected_tools,
                capability.available_tools(),
                resources.capability_availability(),
            )?;
        }
        if invocation.skills.is_some() {
            resolve_skills(&selected_skills, capability.skills())?;
        }
        if invocation.extensions.is_some()
            && let Some(unsupported) = crate::extensions::unsupported_child_scope(&extensions)
        {
            return Err(SubagentResolutionError::ExtensionScopeUnsupported {
                extension: unsupported.extension.into(),
                reason: unsupported.reason.into(),
            });
        }
        let mut profile = definition.profile().clone();
        profile.tools = selected_tools;
        profile.skills = selected_skills;
        profile.extensions = extensions;
        let resolved = if invocation.is_empty() {
            resources
                .resolved_agent(definition.name())
                .expect("generation resolved its complete catalog")
                .clone()
        } else {
            use crate::runtime::agent_profile::{
                AgentProfileAuthority, AgentScope, resolve_agent_profile,
            };
            resolve_agent_profile(
                &profile,
                &AgentProfileAuthority {
                    tools: capability.available_tools(),
                    availability: resources.capability_availability(),
                    skills: capability.skills(),
                    agents: &catalog.names().into_iter().cloned().collect(),
                    workflows: resources.workflows().admitted(),
                    scope: AgentScope::OneShotChild,
                },
            )
        };
        let tools = resolved
            .tools
            .iter()
            .map(|definition| {
                let selector = definition.origin.source().map_or_else(
                    || AgentToolSelection::Builtin {
                        name: definition.name.clone(),
                    },
                    |source_id| AgentToolSelection::Source {
                        source_id,
                        name: definition.name.clone(),
                    },
                );
                freeze_tool(&selector, definition)
            })
            .collect::<Vec<_>>();
        let skills = resolve_skills(&resolved.skills, capability.skills())?;
        let extensions = resolved.extensions;

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
        catalog: &AgentCatalog,
        _available_tools: &AvailableToolCatalog,
        _availability: &CapabilityAvailability,
        skills: &SkillSnapshot,
        models: &ModelBindingRegistry,
    ) -> Result<(), (SubagentName, SubagentResolutionError)> {
        for definition in catalog.definitions() {
            let named = |error: SubagentResolutionError| (definition.name().clone(), error);
            Self::validate_definition_local_references(definition, skills, &mut |model| {
                FrozenModelSpec::freeze(models, model)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .map_err(named)?;
        }
        Ok(())
    }

    /// Validate local Skill/model references independently of physical binding.
    pub(crate) fn validate_local_references(
        catalog: &AgentCatalog,
        skills: &SkillSnapshot,
        mut model_check: impl FnMut(&SessionModelConfig) -> Result<(), String>,
    ) -> Result<(), (SubagentName, SubagentResolutionError)> {
        for definition in catalog.definitions() {
            let named = |error: SubagentResolutionError| (definition.name().clone(), error);
            Self::validate_definition_local_references(definition, skills, &mut model_check)
                .map_err(named)?;
        }
        Ok(())
    }

    fn validate_definition_local_references(
        definition: &NamedAgentDefinition,
        _skills: &SkillSnapshot,
        model_check: &mut impl FnMut(&SessionModelConfig) -> Result<(), String>,
    ) -> Result<(), SubagentResolutionError> {
        if let Some(model) = definition.model() {
            model_check(model).map_err(|detail| SubagentResolutionError::UnknownModel {
                model: model.model.to_string(),
                detail,
            })?;
        }
        Ok(())
    }
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
    selected: &[AgentToolSelection],
    available: &AvailableToolCatalog,
    availability: &CapabilityAvailability,
) -> Result<Vec<ResolvedSubagentTool>, SubagentResolutionError> {
    let mut result = Vec::new();
    for selector in selected {
        let definitions = crate::capabilities::selection::project(
            selector,
            available.tools().iter().map(|tool| &tool.definition),
            availability,
        )
        .map_err(|error| match error {
            crate::capabilities::selection::ToolSelectionError::SourceUnavailable {
                selector,
                source,
                reason,
            } => SubagentResolutionError::SourceUnavailable {
                selector,
                source: source.to_string(),
                reason: reason.to_string(),
            },
            crate::capabilities::selection::ToolSelectionError::ExactToolAbsent {
                source,
                name,
            } => SubagentResolutionError::UnknownCapability {
                selector: format!("source:{source}/{name}"),
            },
            crate::capabilities::selection::ToolSelectionError::UnknownCapability { selector } => {
                SubagentResolutionError::UnknownCapability { selector }
            }
        })?;
        result.extend(
            definitions
                .into_iter()
                .map(|definition| freeze_tool(selector, definition)),
        );
    }
    result.sort_by_key(ResolvedSubagentTool::canonical);
    Ok(result)
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
    definition: &NamedAgentDefinition,
    available: &AvailableToolCatalog,
    availability: &CapabilityAvailability,
) -> BTreeSet<ToolId> {
    definition
        .tools()
        .iter()
        .filter_map(|selector| {
            crate::capabilities::selection::project(
                selector,
                available.tools().iter().map(|tool| &tool.definition),
                availability,
            )
            .ok()
        })
        .flatten()
        .map(|definition| definition.id.clone())
        .collect()
}

/// The exact Skill identities the **named role** itself authorizes.
fn role_skill_authority(
    definition: &NamedAgentDefinition,
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
/// Four properties of this function are the whole point:
///
/// - it runs **per dimension**. Tools, Skills, and extensions are separate
///   authorization domains, and holding one never implies holding another;
/// - it is a real **union**, taken at each dimension's own semantic
///   granularity. Tools and Skills are sets of exact identities, so their
///   union is a set union. An extension composition is not a set, so its
///   union is taken per behavior-affecting contributor by
///   [`crate::extensions::authorize_delegated_extensions`] — a request whose
///   contributors are independently covered by the role and by the invoking
///   Agent is authorized, and nothing is manufactured from neither;
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
    definition: &NamedAgentDefinition,
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
                | ResolvedSubagentTool::Source { tool_id, .. } => tool_id,
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
    if invocation.extensions.is_some() {
        // The two sources are combined by the extension vocabulary's own
        // owner, per behavior-affecting contributor. A whole-composition
        // "role authorizes it OR the parent authorizes it" is strictly
        // narrower than the union this contract specifies: it refuses a
        // request whose contributors are each legitimately held, only
        // because no single source holds all of them at once.
        crate::extensions::authorize_delegated_extensions(
            definition.extensions(),
            &invoking.extensions,
            extensions,
        )
        .map_err(|refusal| SubagentResolutionError::UnauthorizedExtension {
            extension: refusal.extension.to_owned(),
            detail: refusal.detail,
        })?;
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
fn freeze_tool(
    _selector: &AgentToolSelection,
    definition: &ToolDefinition,
) -> ResolvedSubagentTool {
    match definition.origin.source() {
        Some(source_id) => ResolvedSubagentTool::Source {
            identity: crate::tools::mcp::source_tool_identity(&source_id, definition),
            source_id,
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            definition: definition.clone(),
        },
        None => ResolvedSubagentTool::Builtin {
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
    let mut sources = BTreeMap::new();
    for tool in tools {
        match tool {
            ResolvedSubagentTool::Builtin { .. } => {}
            ResolvedSubagentTool::Source {
                source_id, name, ..
            } => {
                let server_id = &crate::tools::mcp::source_server_id(source_id);
                if sources.contains_key(source_id) {
                    continue;
                }
                let binding = configured.get(server_id).ok_or_else(|| {
                    SubagentResolutionError::SourceUnavailable {
                        selector: format!("mcp:{server_id}/{name}"),
                        source: format!("mcp:{server_id}"),
                        reason: "the runtime generation configures no such MCP server".to_owned(),
                    }
                })?;
                sources.insert(source_id.clone(), binding.clone());
            }
        }
    }
    Ok(ResolvedSubagentMaterialization { sources })
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
/// child can then have no opinion about a `models.toml` that changed in the
/// meantime, because it never consults one.
fn resolve_model(
    definition: &NamedAgentDefinition,
    attempt_model: &SessionModelConfig,
    models: &ModelBindingRegistry,
) -> Result<FrozenModelSpec, SubagentResolutionError> {
    let configured = match definition.model() {
        None => attempt_model.clone(),
        Some(model) => model.clone(),
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
    definition: &NamedAgentDefinition,
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
    // The frozen model *decision*, never the provider binding. `configured`
    // is excluded with it: it is the descriptive record of what was asked
    // for, and the resolved invocations below are the authority.
    let crate::model::frozen::FrozenModelSpec {
        configured: _,
        primary,
        summary,
    } = framing.model;
    frame_frozen_model_invocation(&mut hasher, "model", primary);
    // The summary policy is two distinct facts — "summaries follow the
    // session primary" and "summaries use this separately frozen
    // invocation" — and the explicit one is framed by the SAME helper as the
    // primary, so no behavior-affecting summary field can be dropped from
    // the identity while the equivalent primary field is kept.
    match summary {
        crate::model::frozen::FrozenSummaryModel::Session => {
            field(&mut hasher, "model_summary", "session");
        }
        crate::model::frozen::FrozenSummaryModel::Explicit(invocation) => {
            field(&mut hasher, "model_summary", "explicit");
            frame_frozen_model_invocation(&mut hasher, "model_summary", invocation);
        }
    }
    // Tool and Skill selections are semantically unordered and arrive
    // canonically ordered and deduplicated, so the framing preserves that
    // order without imposing a second one.
    count(&mut hasher, "tools", framing.tools.len());
    for tool in framing.tools {
        // Each variant is destructured completely: every frozen field of a
        // resolved Tool is framed, and adding one is a compile error until it
        // is classified. The child consumes the whole frozen `ToolDefinition`,
        // so a stable `ToolId` is never accepted as a summary of it.
        match tool {
            ResolvedSubagentTool::Builtin {
                tool_id,
                name,
                definition,
            } => {
                field(&mut hasher, "tool", "builtin");
                field(&mut hasher, "tool.tool_id", tool_id.as_str());
                field(&mut hasher, "tool.name", name);
                frame_tool_definition(&mut hasher, "tool.definition", definition);
            }
            ResolvedSubagentTool::Source {
                source_id,
                tool_id,
                name,
                definition,
                identity,
            } => {
                field(&mut hasher, "tool", "source");
                field(&mut hasher, "tool.source_id", &source_id.to_string());
                field(&mut hasher, "tool.tool_id", tool_id.as_str());
                field(&mut hasher, "tool.name", name);
                frame_tool_definition(&mut hasher, "tool.definition", definition);
                // `identity` is framed in addition to, not instead of, the
                // definition. It is an independently frozen field that gates
                // the child's startup: the child recomputes it from its own
                // `tools/list` and refuses to run unless it matches, so a spec
                // carrying a different frozen identity for the same definition
                // is a different — failing — execution profile. It is framed
                // here as that frozen value, and this digest never performs
                // the verification itself.
                field(&mut hasher, "tool.source_identity", identity.as_str());
            }
        }
    }
    count(&mut hasher, "skills", framing.skills.len());
    for skill in framing.skills {
        // Destructured for the same reason the Tool framing is: every frozen
        // field is classified, and adding one is a compile error.
        let ResolvedSubagentSkill {
            binding,
            catalog_entry,
            // A materialization SOURCE, not an identity. The child copies
            // from it and then proves the copy hashes back to `version_id`.
            source_root: _,
            // Represented exactly by `version_id`, which hashes every
            // package-relative path AND its bytes in sorted order: the file
            // set cannot change without the version identity changing.
            files: _,
        } = skill;
        field(&mut hasher, "skill.id", binding.skill_id.as_str());
        field(&mut hasher, "skill.version_id", binding.version_id.as_str());
        let crate::skills::SkillCatalogEntry {
            name,
            description,
            // The host `SKILL.md` path. The child REMAPS it onto its own
            // materialized copy, so the parent's spelling never executes.
            location: _,
        } = catalog_entry;
        // Both model-visible fields cross to the child verbatim — only
        // `location` is remapped — so both are part of what the child's model
        // actually sees. `description` in particular drives progressive
        // disclosure: it is how the model decides whether to open the Skill at
        // all. It is NOT covered by `version_id`, even though it is parsed
        // from the hashed `SKILL.md`, because the child trusts this frozen
        // string rather than re-deriving it from the materialized package.
        field(&mut hasher, "skill.name", name);
        field(&mut hasher, "skill.description", description);
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
        "materialization_sources",
        framing.materialization.sources.len(),
    );
    for server_id in framing.materialization.sources.keys() {
        field(
            &mut hasher,
            "materialization_source",
            &server_id.to_string(),
        );
    }
    field(
        &mut hasher,
        "extensions",
        &framing.extensions.effective_digest_framing(),
    );
    SubagentExecutionProfileDigest(format!("sha256:{:x}", hasher.finalize()))
}

/// Frames the complete semantic contract of one frozen [`ToolDefinition`]
/// under `prefix`.
///
/// # Why a `ToolId` is not enough
///
/// A `ToolId` is a stable *capability* identity, not a digest of the runtime
/// and model-facing semantics a definition was frozen with. The same
/// `tool-read` identity can be frozen with a different description, a
/// different input schema, or different execution, concurrency, approval, or
/// replay policies, and the child executes **the frozen definition** — not a
/// definition it looks up by id. Framing `builtin:{tool_id}:{name}` therefore
/// collapsed materially different child execution contracts into one identity.
///
/// # The classification
///
/// Every field of [`ToolDefinition`] is destructured, so adding one is a
/// compile error until it is decided. All nine are **included**:
///
/// ```text
/// id                  the capability identity the child resolves against
/// name                the model-facing name the model emits calls with
/// origin              Builtin vs a specific MCP server: the same name from
///                     two origins is two different capabilities
/// description         model-facing prose; it changes what the model does
/// input_schema        the accepted-argument contract, framed through the
///                     rustX-owned canonical JSON writer
/// execution_policy    attempt-owned vs conversation-owned vs model-selected
/// concurrency_policy  in-batch sequential barrier vs parallel group
/// approval_policy     whether an eligible invocation stops for a human
/// replay_policy       whether the runtime may re-execute after an unknown
///                     outcome
/// ```
///
/// `replay_policy` is included deliberately rather than by omission. It is
/// today's frozen declaration of whether automatic re-execution after a crash
/// is permitted, it crosses the Runtime Client boundary into the child's
/// observable tool projection, and its own documentation defers the consuming
/// recovery policy to a later milestone. Excluding it would require proving it
/// can never affect behavior, which is exactly what that deferral refuses to
/// promise; a durable identity committed today must not silently equate an
/// `Idempotent` tool with a `Never` one.
///
/// Nothing is excluded: [`ToolDefinition`] carries no physical, secret, or
/// execution-correlation field. Physical materialization detail lives in
/// [`ResolvedSubagentMaterialization`], and the model-selectable invocation
/// metadata the runtime adds is added to the *compiled* model-facing
/// definition, never to this canonical one.
fn frame_tool_definition(hasher: &mut Sha256, prefix: &str, definition: &ToolDefinition) {
    // Destructured so a new field of the frozen definition cannot be added
    // without deciding whether it belongs in the child's execution identity.
    let ToolDefinition {
        id,
        name,
        description,
        input_schema,
        execution_policy,
        concurrency_policy,
        approval_policy,
        replay_policy,
        origin,
    } = definition;
    let key = |suffix: &str| format!("{prefix}.{suffix}");
    field(hasher, &key("id"), id.as_str());
    field(hasher, &key("name"), name);
    match origin {
        crate::tools::types::ToolOrigin::ManagedPython { package } => {
            field(hasher, &key("origin"), &format!("python:{package}"));
        }
        crate::tools::types::ToolOrigin::Builtin => field(hasher, &key("origin"), "builtin"),
        crate::tools::types::ToolOrigin::Mcp { server_id } => {
            field(
                hasher,
                &key("origin"),
                &format!("mcp:{}", server_id.as_str()),
            );
        }
    }
    field(hasher, &key("description"), description);
    // The rustX-owned canonical JSON writer, shared with the cross-process
    // MCP Tool identity: object keys sorted recursively, array order
    // preserved because it is semantic in JSON Schema, and numbers and
    // escapes written by rustX rather than by whatever `serde_json`'s map
    // implementation and feature flags happen to do in this build.
    field(
        hasher,
        &key("input_schema"),
        &crate::tools::mcp::identity::canonical_json(input_schema),
    );
    field(
        hasher,
        &key("execution_policy"),
        &format!("{execution_policy:?}"),
    );
    field(
        hasher,
        &key("concurrency_policy"),
        &format!("{concurrency_policy:?}"),
    );
    field(
        hasher,
        &key("approval_policy"),
        &format!("{approval_policy:?}"),
    );
    field(hasher, &key("replay_policy"), &format!("{replay_policy:?}"));
}

/// Frames the complete **non-binding semantics** of one frozen model
/// invocation under `prefix`.
///
/// One helper, used for both the primary invocation and an explicit summary
/// invocation, is the contract: the two are the same kind of frozen decision,
/// and framing them by two hand-written field lists is exactly how a summary
/// model ends up identified by three fields while the primary is identified
/// by eleven. Adding a semantic field to [`FrozenModelInvocation`] is a
/// single edit here that reaches both.
///
/// Everything [`FrozenModelInvocation`] freezes is framed except
/// `binding` — the provider id, endpoint, declared credential source, and the
/// in-process admitted credential. That exclusion is the deliberate
/// security/identity contract: a digest the runtime projects and durably
/// commits must not take credential material into its preimage, and must not
/// change when a key rotates or an endpoint is repointed at the same model.
fn frame_frozen_model_invocation(
    hasher: &mut Sha256,
    prefix: &str,
    invocation: &crate::model::frozen::FrozenModelInvocation,
) {
    // Destructured so a new semantic field of the frozen invocation cannot be
    // added without deciding whether it belongs in the identity.
    let crate::model::frozen::FrozenModelInvocation {
        binding: _,
        model,
        protocol,
        context_window,
        model_max_output_tokens,
        max_output_tokens,
        reasoning_profile,
        reasoning_enabled,
        request_params,
        capabilities,
        declared_capabilities,
        compat,
    } = invocation;
    let key = |suffix: &str| format!("{prefix}.{suffix}");
    field(hasher, &key("model"), &model.to_string());
    field(hasher, &key("protocol"), &format!("{protocol:?}"));
    field(hasher, &key("context_window"), &context_window.to_string());
    field(
        hasher,
        &key("model_max_output_tokens"),
        &model_max_output_tokens.to_string(),
    );
    field(
        hasher,
        &key("effective_max_output_tokens"),
        &max_output_tokens.to_string(),
    );
    match reasoning_profile {
        // An absent profile and a profile named "absent" are different
        // decisions, so absence is framed with a spelling no id can carry.
        None => field(hasher, &key("reasoning_profile"), "\u{0}absent"),
        Some(profile) => field(hasher, &key("reasoning_profile"), profile.as_str()),
    }
    field(
        hasher,
        &key("reasoning_enabled"),
        &reasoning_enabled.to_string(),
    );
    field(
        hasher,
        &key("request_params"),
        &canonical_json(request_params),
    );
    field(hasher, &key("capabilities"), &canonical_json(capabilities));
    field(
        hasher,
        &key("declared_capabilities"),
        &canonical_json(declared_capabilities),
    );
    // Framed field by field rather than through `canonical_json`. `ModelCompat`
    // serializes in its *authoring* shape — a translation field is emitted only
    // when the catalog spelled it out — so an authored value equal to its own
    // default and an omitted one serialize differently while behaving
    // identically. Its `PartialEq` already says which five values are the
    // semantics; the framing says exactly the same thing.
    let crate::model::catalog::ModelCompat {
        chat_max_tokens_field,
        chat_stream_usage,
        chat_reasoning_replay,
        chat_tool_protocol,
        responses_storage,
        explicit_fields: _,
    } = compat;
    field(
        hasher,
        &key("compat.chat_max_tokens_field"),
        &format!("{chat_max_tokens_field:?}"),
    );
    field(
        hasher,
        &key("compat.chat_stream_usage"),
        &format!("{chat_stream_usage:?}"),
    );
    match chat_reasoning_replay {
        None => field(hasher, &key("compat.chat_reasoning_replay"), "\u{0}absent"),
        Some(replay) => field(
            hasher,
            &key("compat.chat_reasoning_replay"),
            &format!("{replay:?}"),
        ),
    }
    field(
        hasher,
        &key("compat.chat_tool_protocol"),
        &format!("{chat_tool_protocol:?}"),
    );
    field(
        hasher,
        &key("compat.responses_storage"),
        &format!("{responses_storage:?}"),
    );
}

/// Serializes one arbitrary value into rustX canonical JSON text.
///
/// This is a thin generic adapter over
/// [`crate::tools::mcp::identity::canonical_json`], deliberately rather than a
/// second canonicalizer: the profile digest and the cross-process MCP Tool
/// identity must agree on what "the same JSON document" means, and one writer
/// is how that stays true. It owns object-key ordering, array order, number
/// formatting and string escaping itself, so no `serde_json` map
/// implementation or feature flag can move a digest.
fn canonical_json<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value).map_or_else(
        |_| "\u{0}unserializable".to_owned(),
        |value| crate::tools::mcp::identity::canonical_json(&value),
    )
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
pub(crate) fn render_agent_routing(catalog: &AgentCatalog) -> String {
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
    use super::{ResolvedSubagentTool, SubagentResolutionError, freeze_tool, render_agent_routing};
    use crate::capabilities::selection::AgentToolSelection;
    use crate::capabilities::{
        AvailableToolCatalog, CapabilityAvailability, CapabilitySourceState, ToolSourceId,
    };
    use crate::runtime::identity::{McpServerId, ToolId};
    use crate::runtime::subagent::catalog::{AgentCatalog, NamedAgentDefinition, SubagentName};
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

    #[test]
    fn agent_all_freezes_finite_exact_tools_from_only_the_supplied_generation() {
        use crate::capabilities::ToolSourceId;
        for source in [
            ToolSourceId::Mcp(McpServerId::new("github")),
            ToolSourceId::ManagedPython("data-analysis".into()),
        ] {
            let origin = match &source {
                ToolSourceId::Mcp(id) => ToolOrigin::Mcp {
                    server_id: id.clone(),
                },
                ToolSourceId::ManagedPython(package) => ToolOrigin::ManagedPython {
                    package: package.clone(),
                },
            };
            let r1 = catalog(vec![tool("b", origin.clone()), tool("a", origin.clone())]);
            let r2 = catalog(vec![
                tool("c", origin.clone()),
                tool("a", origin.clone()),
                tool("b", origin),
            ]);
            let availability = [(source.clone(), CapabilitySourceState::Ready)].into();
            let all = definition(vec![AgentToolSelection::All {
                source_id: source.clone(),
            }]);
            let frozen = resolve_tools(&all, &r1, &availability).unwrap();
            let names = |tools: &[ResolvedSubagentTool]| {
                tools
                    .iter()
                    .map(|tool| {
                        let ResolvedSubagentTool::Source {
                            source_id,
                            name,
                            definition,
                            identity,
                            ..
                        } = tool
                        else {
                            panic!("All must freeze exact source Tools")
                        };
                        assert_eq!(source_id, &source);
                        assert_eq!(name, &definition.name);
                        assert_eq!(
                            identity,
                            &crate::tools::mcp::source_tool_identity(source_id, definition)
                        );
                        name.clone()
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(names(&frozen), ["a", "b"]);
            assert_eq!(
                names(&resolve_tools(&all, &r2, &availability).unwrap()),
                ["a", "b", "c"]
            );
            assert_eq!(
                names(&resolve_tools(&all, &r1, &availability).unwrap()),
                ["a", "b"]
            );
            assert_eq!(names(&frozen), ["a", "b"]);
        }
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
                ToolOrigin::ManagedPython {
                    package: "symbols".into(),
                },
            ),
        ])
    }

    fn resolve_tools(
        definition: &NamedAgentDefinition,
        available: &AvailableToolCatalog,
        availability: &CapabilityAvailability,
    ) -> Result<Vec<ResolvedSubagentTool>, SubagentResolutionError> {
        super::resolve_tools(definition.tools(), available, availability)
    }

    fn definition(tools: Vec<AgentToolSelection>) -> NamedAgentDefinition {
        NamedAgentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            crate::runtime::agent_profile::AgentProfile {
                description: "description".to_owned(),
                instructions: "instructions".to_owned(),
                model: None,
                execution_deadline: None,
                tools,
                skills: Vec::new(),
                project_instructions:
                    crate::runtime::agent_profile::AgentProjectInstructionPolicy {
                        inherit: true,
                        files: Vec::new(),
                    },
                workspace_policy: WorkspacePolicy::SharedWorkspace,
                extensions: crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                )
                .and_todo(),
                agents: std::collections::BTreeSet::default(),
                workflows: std::collections::BTreeSet::default(),
            },
            std::path::PathBuf::from("/w/explore.md"),
        )
        .expect("definition")
    }

    fn ready() -> CapabilityAvailability {
        let mut availability = CapabilityAvailability::new();
        availability.insert(
            ToolSourceId::ManagedPython("symbols".into()),
            CapabilitySourceState::Ready,
        );
        availability.insert(
            ToolSourceId::Mcp(McpServerId::new("github")),
            CapabilitySourceState::Ready,
        );
        availability
    }

    #[test]
    fn every_origin_freezes_its_exact_source_identity() {
        let resolved = resolve_tools(
            &definition(vec![
                AgentToolSelection::Builtin {
                    name: "read".to_owned(),
                },
                AgentToolSelection::Source {
                    source_id: crate::capabilities::ToolSourceId::try_from(String::from("github"))
                        .unwrap(),
                    name: "get_issue".to_owned(),
                },
                AgentToolSelection::Source {
                    source_id: crate::capabilities::ToolSourceId::try_from(String::from(
                        "python:symbols",
                    ))
                    .unwrap(),
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
            ResolvedSubagentTool::Source { source_id, name, .. }
                if source_id.to_string() == "github" && name == "get_issue"
        ));
        assert!(matches!(
            &resolved[2],
            ResolvedSubagentTool::Source { source_id, name, .. }
                if source_id.to_string() == "python:symbols" && name == "repository_symbols"
        ));
        assert_eq!(
            resolved
                .iter()
                .filter(|tool| matches!(tool, ResolvedSubagentTool::Source { .. }))
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
                &AgentToolSelection::Builtin {
                    name: "read".to_owned(),
                },
                &tool("read", ToolOrigin::Builtin),
            ),
            freeze_tool(
                &AgentToolSelection::Source {
                    source_id: crate::capabilities::ToolSourceId::Mcp(github.clone()),
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
            plane.sources.keys().collect::<Vec<_>>(),
            vec![&crate::capabilities::ToolSourceId::Mcp(github.clone())],
            "the unrelated configured server is never frozen for this child"
        );
        assert!(
            !plane
                .sources
                .contains_key(&crate::capabilities::ToolSourceId::Mcp(unrelated))
        );
        assert!(!plane.is_empty());

        // A Builtin-only agent needs no external plane whatsoever.
        let builtin_only = vec![freeze_tool(
            &AgentToolSelection::Builtin {
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
            &AgentToolSelection::Source {
                source_id: crate::capabilities::ToolSourceId::try_from(String::from("github"))
                    .unwrap(),
                name: "get_issue".to_owned(),
            },
            &definition,
        );
        let ResolvedSubagentTool::Source { identity, .. } = &frozen else {
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
            &definition(vec![AgentToolSelection::Builtin {
                name: "search".to_owned(),
            }]),
            &catalog,
            &ready(),
        )
        .expect("builtin resolution");
        assert!(matches!(builtin[0], ResolvedSubagentTool::Builtin { .. }));
        let mcp = resolve_tools(
            &definition(vec![AgentToolSelection::Source {
                source_id: crate::capabilities::ToolSourceId::try_from(String::from("github"))
                    .unwrap(),
                name: "search".to_owned(),
            }]),
            &catalog,
            &ready(),
        )
        .expect("mcp resolution");
        assert!(matches!(mcp[0], ResolvedSubagentTool::Source { .. }));
        assert_eq!(
            resolve_tools(
                &definition(vec![AgentToolSelection::Source {
                    source_id: crate::capabilities::ToolSourceId::try_from(String::from("other"))
                        .unwrap(),
                    name: "search".to_owned(),
                }]),
                &catalog,
                &ready(),
            ),
            Err(SubagentResolutionError::SourceUnavailable {
                selector: "source:other/search".to_owned(),
                source: "other".into(),
                reason: "source is not defined or discovered".into()
            })
        );
    }

    #[test]
    fn an_unavailable_source_is_distinct_from_an_invalid_selector() {
        let mut availability = ready();
        availability.insert(
            ToolSourceId::Mcp(McpServerId::new("github")),
            CapabilitySourceState::unavailable("the server refused the handshake"),
        );
        // The MCP capability is absent from the available catalog precisely
        // because its source failed; the outcome must still be the
        // source-unavailable fact, not "unknown capability".
        let catalog = catalog(vec![tool("read", ToolOrigin::Builtin)]);
        assert!(matches!(
            resolve_tools(
                &definition(vec![AgentToolSelection::Source {
                    source_id: crate::capabilities::ToolSourceId::try_from(String::from("github"))
                        .unwrap(),
                    name: "get_issue".to_owned(),
                }]),
                &catalog,
                &availability,
            ),
            Err(SubagentResolutionError::SourceUnavailable { .. })
        ));
        assert_eq!(
            resolve_tools(
                &definition(vec![AgentToolSelection::Builtin {
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
        tools: Vec<AgentToolSelection>,
        extensions: crate::extensions::NativeAgentExtensions,
    ) -> NamedAgentDefinition {
        NamedAgentDefinition::new(
            SubagentName::parse("reviewer").expect("name"),
            crate::runtime::agent_profile::AgentProfile {
                description: "description".to_owned(),
                instructions: "instructions".to_owned(),
                model: None,
                execution_deadline: None,
                tools,
                skills: Vec::new(),
                project_instructions:
                    crate::runtime::agent_profile::AgentProjectInstructionPolicy {
                        inherit: true,
                        files: Vec::new(),
                    },
                workspace_policy: WorkspacePolicy::SharedWorkspace,
                extensions,
                agents: std::collections::BTreeSet::default(),
                workflows: std::collections::BTreeSet::default(),
            },
            std::path::PathBuf::from("/w/reviewer.md"),
        )
        .expect("definition")
    }

    fn requested_tools(names: &[&str]) -> super::SubagentInvocationOverride {
        super::SubagentInvocationOverride {
            tools: Some(crate::capabilities::selection::ToolSelectionDocument {
                builtin: names.iter().map(|name| (*name).to_owned()).collect(),
                sources: std::collections::BTreeMap::new(),
            }),
            ..super::SubagentInvocationOverride::default()
        }
    }

    fn authorize(
        invocation: &super::SubagentInvocationOverride,
        definition: &NamedAgentDefinition,
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
            vec![AgentToolSelection::Builtin {
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
            vec![AgentToolSelection::Builtin {
                name: "read".to_owned(),
            }],
            crate::extensions::NativeAgentExtensions::none(),
        );
        // Another role holding `write` changes nothing: the ceiling reads
        // this role's own selection and the invoking profile, never the
        // union of every admitted definition.
        let _other_role = role_with(
            vec![AgentToolSelection::Builtin {
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
                sources: [(
                    crate::capabilities::ToolSourceId::Mcp(McpServerId::new("github")),
                    crate::capabilities::selection::SourceToolSelection::Exact(vec![
                        "search".to_owned(),
                    ]),
                )]
                .into_iter()
                .collect(),
            }),
            ..super::SubagentInvocationOverride::default()
        };
        assert_eq!(
            authorize(&mcp_request, &role, &available, &parent),
            Err(SubagentResolutionError::UnauthorizedTool {
                selector: "source:github/search".to_owned()
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
            vec![AgentToolSelection::Builtin {
                name: "read".to_owned(),
            }],
            crate::extensions::NativeAgentExtensions::with_agent_status(
                crate::context::AgentStatusConfig::default(),
            )
            .and_todo(),
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

    /// The extension half of the ceiling is a real **union**, taken at the
    /// vocabulary's own granularity — not "one source authorizes the whole
    /// composition".
    ///
    /// The role renders UTC Time with Background off; the invoking Agent has
    /// Time off and Background on. Neither authorizes the combined request by
    /// itself, and together they authorize it exactly, with nothing
    /// manufactured.
    #[test]
    fn sub258_extension_authority_is_the_union_of_role_and_invoking_contributors() {
        let available = available();
        let compose = |time: bool, background: bool| {
            crate::extensions::NativeAgentExtensions::with_agent_status(
                crate::context::AgentStatusConfig {
                    time: crate::context::TimeStatusConfig {
                        enabled: time,
                        timezone: None,
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
        let both_on = request(serde_json::json!({
            "agentStatus": {"time": {"enabled": true}, "background": {"enabled": true}}
        }));

        let time_role = role_with(Vec::new(), compose(true, false));
        let background_parent =
            super::InvokingAgentAuthority::from_identities([], [], compose(false, true));

        assert!(
            matches!(
                authorize(
                    &both_on,
                    &time_role,
                    &available,
                    &super::InvokingAgentAuthority::none()
                ),
                Err(SubagentResolutionError::UnauthorizedExtension { .. })
            ),
            "the role alone holds no Background"
        );
        assert!(
            matches!(
                authorize(
                    &both_on,
                    &role_with(Vec::new(), crate::extensions::NativeAgentExtensions::none()),
                    &available,
                    &background_parent
                ),
                Err(SubagentResolutionError::UnauthorizedExtension { .. })
            ),
            "the invoking Agent alone holds no Time"
        );
        assert_eq!(
            authorize(&both_on, &time_role, &available, &background_parent),
            Ok(()),
            "independently authorized contributors from both sources combine"
        );
    }

    /// Tools, Skills, and extensions stay three separate authorization
    /// domains: holding one never authorizes another, and an extension
    /// composition is never satisfiable out of tool or Skill authority.
    #[test]
    fn sub258_the_three_authorization_domains_stay_independent() {
        let available = available();
        let status = crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        );
        // A caller holding every tool the generation knows, and nothing else.
        let tool_rich_parent = super::InvokingAgentAuthority::from_identities(
            available
                .definitions()
                .into_iter()
                .map(|definition| definition.id),
            [],
            crate::extensions::NativeAgentExtensions::none(),
        );
        let bare_role = role_with(Vec::new(), crate::extensions::NativeAgentExtensions::none());

        // Tool authority does not become extension authority.
        let extension_request = super::SubagentInvocationOverride {
            extensions: Some(crate::extensions::NativeAgentExtensionSelection::of(
                &status,
            )),
            ..super::SubagentInvocationOverride::default()
        };
        assert!(matches!(
            authorize(
                &extension_request,
                &bare_role,
                &available,
                &tool_rich_parent
            ),
            Err(SubagentResolutionError::UnauthorizedExtension { .. })
        ));

        // ...and extension authority does not become tool authority.
        let extension_rich_parent =
            super::InvokingAgentAuthority::from_identities([], [], status.clone());
        assert!(matches!(
            authorize(
                &requested_tools(&["grep"]),
                &bare_role,
                &available,
                &extension_rich_parent
            ),
            Err(SubagentResolutionError::UnauthorizedTool { .. })
        ));

        // The same caller that cannot delegate the extension can still
        // delegate the tools it actually holds, so the refusal above is a
        // domain fact and not an inert path that refuses everything.
        assert_eq!(
            authorize(
                &requested_tools(&["grep"]),
                &bare_role,
                &available,
                &tool_rich_parent
            ),
            Ok(())
        );
    }

    /// Agent Status and Todo support one-shot child scope; Goal is root-only.
    #[test]
    fn sub258_every_supported_extension_is_child_scope_supported() {
        for composition in [
            crate::extensions::NativeAgentExtensions::none(),
            crate::extensions::NativeAgentExtensions::with_agent_status(
                crate::context::AgentStatusConfig::default(),
            )
            .and_todo(),
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
        let catalog = AgentCatalog::new([
            NamedAgentDefinition::new(
                SubagentName::parse("research").expect("name"),
                crate::runtime::agent_profile::AgentProfile {
                    description: "Deep research.".to_owned(),
                    instructions: "instructions".to_owned(),
                    model: None,
                    execution_deadline: None,
                    tools: Vec::new(),
                    skills: Vec::new(),
                    project_instructions:
                        crate::runtime::agent_profile::AgentProjectInstructionPolicy {
                            inherit: true,
                            files: Vec::new(),
                        },
                    workspace_policy: WorkspacePolicy::SharedWorkspace,
                    extensions: crate::extensions::NativeAgentExtensions::with_agent_status(
                        crate::context::AgentStatusConfig::default(),
                    )
                    .and_todo(),
                    agents: std::collections::BTreeSet::default(),
                    workflows: std::collections::BTreeSet::default(),
                },
                std::path::PathBuf::from("/w/research.md"),
            )
            .expect("definition"),
            NamedAgentDefinition::new(
                SubagentName::parse("explore").expect("name"),
                crate::runtime::agent_profile::AgentProfile {
                    description: "Read-only exploration.".to_owned(),
                    instructions: "instructions".to_owned(),
                    model: None,
                    execution_deadline: None,
                    tools: Vec::new(),
                    skills: Vec::new(),
                    project_instructions:
                        crate::runtime::agent_profile::AgentProjectInstructionPolicy {
                            inherit: true,
                            files: Vec::new(),
                        },
                    workspace_policy: WorkspacePolicy::SharedWorkspace,
                    extensions: crate::extensions::NativeAgentExtensions::with_agent_status(
                        crate::context::AgentStatusConfig::default(),
                    )
                    .and_todo(),
                    agents: std::collections::BTreeSet::default(),
                    workflows: std::collections::BTreeSet::default(),
                },
                std::path::PathBuf::from("/w/explore.md"),
            )
            .expect("definition"),
        ])
        .expect("catalog");
        assert_eq!(
            render_agent_routing(&catalog),
            "Available agents:\n- explore: Read-only exploration.\n- research: Deep research."
        );
        assert_eq!(
            render_agent_routing(&AgentCatalog::empty()),
            "This runtime admits no named subagent; the call always fails."
        );
    }

    // ---------------------------------------------------------------
    // Issue #258: the effective execution-profile digest.
    //
    // These are spec-level: they build a complete frozen contract and change
    // exactly one field at a time, which is the only way to prove that a
    // behavior-affecting field is in the preimage and a non-semantic one is
    // not. The role-level equivalences (a replaced-away default, an override
    // restating the defaults, the Tool and Workflow paths agreeing) are
    // proven against real composed generations in tests/subagent/overrides.rs.
    // ---------------------------------------------------------------
    use super::{FrozenModelSpec, ResolvedSubagentSkill, ResolvedSubagentSpec, SessionModelConfig};
    use crate::runtime::ProjectContextFile;
    use crate::runtime::identity::{SkillId, SkillVersionId};
    use crate::runtime::subagent::catalog::SubagentExecutionDeadline;

    fn frozen_invocation() -> crate::model::frozen::FrozenModelInvocation {
        crate::model::frozen::FrozenModelInvocation {
            binding: crate::model::frozen::FrozenProviderBinding {
                resolved_credential: None,
                provider: crate::model::catalog::ProviderId::new("local"),
                base_url: "http://127.0.0.1:9/v1".to_owned(),
                credential: crate::model::catalog::CredentialSource::Environment(
                    "RUSTX_TEST_KEY".to_owned(),
                ),
            },
            model: crate::model::catalog::ModelRef::parse("local/model-a").expect("model"),
            protocol: crate::model::types::ModelProtocol::OpenAiChatCompletions,
            context_window: 128_000,
            model_max_output_tokens: 4096,
            max_output_tokens: 512,
            reasoning_profile: None,
            reasoning_enabled: false,
            request_params: crate::model::invocation::RequestParams::new(),
            capabilities: crate::model::catalog::ModelCapabilities::text_only(true, false),
            declared_capabilities: crate::model::catalog::ModelCapabilities::text_only(true, true),
            compat: crate::model::catalog::ModelCompat::default(),
        }
    }

    fn frozen_spec() -> ResolvedSubagentSpec {
        let definition = role_with(Vec::new(), crate::extensions::NativeAgentExtensions::none());
        ResolvedSubagentSpec {
            agent: definition.name().clone(),
            definition_digest: definition.digest().clone(),
            execution_deadline: None,
            workspace_policy: WorkspacePolicy::SharedWorkspace,
            instructions: "instructions".to_owned(),
            model: FrozenModelSpec {
                configured: SessionModelConfig::of(
                    crate::model::catalog::ModelRef::parse("local/model-a").expect("model"),
                ),
                primary: frozen_invocation(),
                summary: crate::model::frozen::FrozenSummaryModel::Session,
            },
            tools: Vec::new(),
            skills: Vec::new(),
            project_instructions: Vec::new(),
            materialization: super::ResolvedSubagentMaterialization::default(),
            extensions: crate::extensions::NativeAgentExtensions::none(),
        }
    }

    /// The profile digest identifies the **effective execution profile**, so
    /// source-definition-only provenance must not reach its preimage.
    ///
    /// `definition_digest` carries the role's routing description and its
    /// default tool/Skill/extension selections — things that either never
    /// execute, or that an override may have replaced entirely. It stays on
    /// the frozen contract as independent provenance and stays out of the
    /// effective identity.
    #[test]
    fn sub258_the_profile_digest_is_independent_of_the_source_definition_digest() {
        let spec = frozen_spec();
        let other_definition = NamedAgentDefinition::new(spec.agent.clone(), crate::runtime::agent_profile::AgentProfile { description: // A completely different routing description, and defaults that
            // an override would have replaced away.
            "An entirely different routing description.".to_owned(), instructions: "instructions".to_owned(), model: None, execution_deadline: None, tools: vec![AgentToolSelection::Builtin {
                name: "read".to_owned(),
            }], skills: vec!["some-skill".to_owned()], project_instructions: crate::runtime::agent_profile::AgentProjectInstructionPolicy {
                inherit: true,
                files: Vec::new(),
            }, workspace_policy: WorkspacePolicy::SharedWorkspace, extensions: crate::extensions::NativeAgentExtensions::with_agent_status(crate::context::AgentStatusConfig::default()).and_todo(), agents: std::collections::BTreeSet::default(), workflows: std::collections::BTreeSet::default() }, std::path::PathBuf::from("/w/reviewer.md"))
        .expect("definition");

        let mut relabelled = spec.clone();
        relabelled.definition_digest = other_definition.digest().clone();
        assert_ne!(
            spec.definition_digest, relabelled.definition_digest,
            "the two source definitions really are different"
        );
        assert_eq!(
            spec.profile_digest(),
            relabelled.profile_digest(),
            "one effective execution contract is one effective profile identity"
        );
    }

    /// Every behavior-affecting field of the frozen contract is in the
    /// preimage. One mutation per profile, all distinct.
    #[test]
    #[allow(clippy::too_many_lines)] // one field-by-field framing matrix
    fn sub258_every_behavior_affecting_frozen_field_changes_the_profile_digest() {
        let base = frozen_spec();
        let mut variants = vec![base.clone()];

        let mut renamed = base.clone();
        renamed.agent = SubagentName::parse("auditor").expect("name");
        variants.push(renamed);

        let mut reinstructed = base.clone();
        reinstructed.instructions = "different instructions".to_owned();
        variants.push(reinstructed);

        let mut deadlined = base.clone();
        deadlined.execution_deadline =
            Some(SubagentExecutionDeadline::from_millis(60_000).expect("deadline"));
        variants.push(deadlined);

        let mut worktree = base.clone();
        worktree.workspace_policy = WorkspacePolicy::GitWorktree {
            require_clean_parent: true,
        };
        variants.push(worktree);

        let mut dirty_worktree = base.clone();
        dirty_worktree.workspace_policy = WorkspacePolicy::GitWorktree {
            require_clean_parent: false,
        };
        variants.push(dirty_worktree);

        let mut with_tool = base.clone();
        with_tool.tools = vec![ResolvedSubagentTool::Builtin {
            tool_id: ToolId::new("tool-read"),
            name: "read".to_owned(),
            definition: tool("read", ToolOrigin::Builtin),
        }];
        variants.push(with_tool);

        let mut guided = base.clone();
        guided.project_instructions = vec![ProjectContextFile {
            path: std::path::PathBuf::from("/w/AGENTS.md"),
            content: "guidance".to_owned(),
        }];
        variants.push(guided);

        let mut reguided = base.clone();
        reguided.project_instructions = vec![ProjectContextFile {
            path: std::path::PathBuf::from("/w/AGENTS.md"),
            content: "different guidance".to_owned(),
        }];
        variants.push(reguided);

        let mut composed = base.clone();
        composed.extensions = crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )
        .and_todo();
        variants.push(composed);

        assert_distinct(&variants, "every behavior-affecting frozen field");
    }

    /// An **explicit** summary invocation is framed exactly as completely as
    /// the primary one.
    ///
    /// The v1 framing reduced it to model + protocol + effective output
    /// budget, so two children whose summarization behaved materially
    /// differently collided into one identity. The shared
    /// `frame_frozen_model_invocation` closes that by construction; this
    /// proves it category by category.
    #[test]
    fn sub258_every_explicit_summary_model_field_changes_the_profile_digest() {
        let base = frozen_spec();
        let explicit = |mutate: &dyn Fn(&mut crate::model::frozen::FrozenModelInvocation)| {
            let mut invocation = frozen_invocation();
            mutate(&mut invocation);
            let mut spec = base.clone();
            spec.model.summary =
                crate::model::frozen::FrozenSummaryModel::Explicit(Box::new(invocation));
            spec
        };

        let mut variants = vec![
            // "follows the session primary" and "an explicit invocation that
            // happens to equal the primary" are two different policies.
            base.clone(),
            explicit(&|_| {}),
            explicit(&|invocation| {
                invocation.model =
                    crate::model::catalog::ModelRef::parse("local/model-b").expect("model");
            }),
            explicit(&|invocation| {
                invocation.protocol = crate::model::types::ModelProtocol::OpenAiResponses;
            }),
            explicit(&|invocation| invocation.context_window = 64_000),
            explicit(&|invocation| invocation.model_max_output_tokens = 8192),
            explicit(&|invocation| invocation.max_output_tokens = 256),
            explicit(&|invocation| {
                invocation.reasoning_profile =
                    Some(crate::model::catalog::ReasoningProfileId::new("low"));
            }),
            explicit(&|invocation| {
                invocation.reasoning_profile =
                    Some(crate::model::catalog::ReasoningProfileId::new("high"));
            }),
            explicit(&|invocation| invocation.reasoning_enabled = true),
            explicit(&|invocation| {
                invocation
                    .request_params
                    .insert("temperature".to_owned(), serde_json::json!(0.2));
            }),
            explicit(&|invocation| {
                invocation.capabilities =
                    crate::model::catalog::ModelCapabilities::text_only(false, false);
            }),
            explicit(&|invocation| {
                invocation.declared_capabilities =
                    crate::model::catalog::ModelCapabilities::text_only(false, false);
            }),
            explicit(&|invocation| {
                invocation.compat.chat_reasoning_replay =
                    Some(crate::model::catalog::ChatReasoningReplay::Omit);
            }),
            explicit(&|invocation| {
                invocation.compat.chat_stream_usage =
                    crate::model::catalog::ChatStreamUsage::Unsupported;
            }),
        ];
        // The same mutations on the PRIMARY invocation must also each change
        // the identity, so the shared helper is proven on both users.
        let primary_mutations: [&dyn Fn(&mut crate::model::frozen::FrozenModelInvocation); 7] = [
            &|invocation| invocation.context_window = 32_000,
            &|invocation| invocation.model_max_output_tokens = 1024,
            &|invocation| invocation.max_output_tokens = 128,
            &|invocation| invocation.reasoning_enabled = true,
            &|invocation| {
                invocation
                    .request_params
                    .insert("top_p".to_owned(), serde_json::json!(0.9));
            },
            &|invocation| {
                invocation.capabilities =
                    crate::model::catalog::ModelCapabilities::text_only(false, true);
            },
            &|invocation| {
                invocation.compat.chat_stream_usage =
                    crate::model::catalog::ChatStreamUsage::Unsupported;
            },
        ];
        for mutate in primary_mutations {
            let mut spec = base.clone();
            mutate(&mut spec.model.primary);
            variants.push(spec);
        }

        assert_distinct(&variants, "every framed frozen model field");
    }

    /// Provider binding and credential material are excluded, and that
    /// exclusion is a real contract rather than an omission: rotating a
    /// credential or repointing an endpoint at the same model leaves the
    /// identity unchanged, on the primary and on an explicit summary alike.
    ///
    /// `FrozenModelSpec::configured` is excluded with them: it is the
    /// descriptive record of what was *asked for*, while the resolved
    /// invocations are the authority the child executes.
    #[test]
    fn sub258_provider_binding_and_desired_configuration_stay_out_of_the_digest() {
        let base = frozen_spec();
        let identity = base.profile_digest();

        let rebind = |invocation: &mut crate::model::frozen::FrozenModelInvocation| {
            invocation.binding.provider = crate::model::catalog::ProviderId::new("elsewhere");
            invocation.binding.base_url = "https://example.invalid/v1".to_owned();
            invocation.binding.credential =
                crate::model::catalog::CredentialSource::Literal("a-rotated-secret".to_owned());
            invocation.binding.resolved_credential = Some(
                crate::model::catalog::ResolvedCredential::new("a-rotated-secret".to_owned()),
            );
        };

        let mut rebound = base.clone();
        rebind(&mut rebound.model.primary);
        assert_eq!(
            rebound.profile_digest(),
            identity,
            "the primary provider binding is not part of the effective identity"
        );

        let mut explicit_summary = base.clone();
        explicit_summary.model.summary =
            crate::model::frozen::FrozenSummaryModel::Explicit(Box::new(frozen_invocation()));
        let explicit_identity = explicit_summary.profile_digest();
        let mut rebound_summary = explicit_summary.clone();
        if let crate::model::frozen::FrozenSummaryModel::Explicit(invocation) =
            &mut rebound_summary.model.summary
        {
            rebind(invocation);
        }
        assert_eq!(
            rebound_summary.profile_digest(),
            explicit_identity,
            "an explicit summary's provider binding is excluded exactly like the primary's"
        );

        let mut reconfigured = base.clone();
        reconfigured.model.configured.max_output_tokens = Some(64);
        reconfigured
            .model
            .configured
            .request_params
            .insert("temperature".to_owned(), serde_json::json!(1.5));
        assert_eq!(
            reconfigured.profile_digest(),
            identity,
            "the desired configuration is projection detail; the resolved invocation is authority"
        );
    }

    /// `ModelCompat` serializes in its authoring shape — a translation field
    /// appears only when the catalog spelled it out — so the framing must not
    /// go through that serializer.
    ///
    /// Two compats that are semantically equal, one of which authored a value
    /// equal to its own default, must be one effective profile; two that
    /// differ in any of the five translation decisions must not.
    #[test]
    fn sub258_compat_framing_follows_its_semantics_not_its_authored_shape() {
        let base = frozen_spec();
        let mut authored_default = base.clone();
        authored_default.model.primary.compat = serde_json::from_value(serde_json::json!({
            "chatStreamUsage": crate::model::catalog::ChatStreamUsage::default(),
        }))
        .expect("an explicitly authored default compat");
        assert_eq!(
            authored_default.model.primary.compat, base.model.primary.compat,
            "the two compats really are semantically equal"
        );
        assert_ne!(
            serde_json::to_value(authored_default.model.primary.compat).expect("encode"),
            serde_json::to_value(base.model.primary.compat).expect("encode"),
            "...and they really do serialize differently, which is the trap"
        );
        assert_eq!(
            authored_default.profile_digest(),
            base.profile_digest(),
            "one effective translation behavior is one effective profile"
        );

        let mut differing = base.clone();
        differing.model.primary.compat.responses_storage =
            crate::model::catalog::ResponsesStorageMode::Stateless;
        assert_ne!(
            differing.profile_digest(),
            base.profile_digest(),
            "a different translation decision is a different effective profile"
        );
    }

    /// The framing is versioned, and the version is in the preimage.
    #[test]
    fn sub258_the_profile_framing_is_versioned() {
        assert_eq!(
            super::SUBAGENT_EXECUTION_PROFILE_DIGEST_VERSION,
            "rustx-subagent-profile-v3",
            "the corrected framing is not one of the readings it replaced"
        );
        assert!(
            frozen_spec()
                .profile_digest()
                .as_str()
                .starts_with("sha256:")
        );
    }

    // ---------------------------------------------------------------
    // Issue #258: a stable capability id is not a semantic contract.
    // ---------------------------------------------------------------

    /// The exact `ToolDefinition` a Builtin variant is frozen with, used as
    /// the fixed point every mutation below departs from by one field.
    fn frozen_builtin_definition() -> ToolDefinition {
        ToolDefinition {
            id: ToolId::new("tool-read"),
            name: "read".to_owned(),
            description: "Reads one file.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        }
    }

    /// A frozen contract carrying exactly one Builtin tool, whose outer
    /// `tool_id` and model-facing `name` are the SAME in every variant.
    fn spec_with_builtin(definition: ToolDefinition) -> ResolvedSubagentSpec {
        let mut spec = frozen_spec();
        spec.tools = vec![ResolvedSubagentTool::Builtin {
            tool_id: ToolId::new("tool-read"),
            name: "read".to_owned(),
            definition,
        }];
        spec
    }

    /// A Builtin `ToolId` is a stable **capability** identity, never a digest
    /// of the semantics the definition was frozen with.
    ///
    /// The child consumes the frozen `ToolDefinition` — it does not look one
    /// up by id — so the same `tool-read` identity frozen with a different
    /// approval policy, execution policy, concurrency policy, replay policy,
    /// description, or input schema is a materially different child execution
    /// contract. Framing `builtin:{tool_id}:{name}` collapsed all of them into
    /// one identity; this is the regression that says it cannot again.
    ///
    /// Every variant below keeps `tool_id` and `name` byte-identical, so the
    /// only thing that can separate the digests is the definition itself.
    #[test]
    fn sub258_a_builtin_tool_id_never_summarizes_its_frozen_definition() {
        let base = frozen_builtin_definition();
        let mut variants = vec![spec_with_builtin(base.clone())];

        let mut redescribed = base.clone();
        redescribed.description = "Reads one file, carefully.".to_owned();
        variants.push(spec_with_builtin(redescribed));

        let mut reschematized = base.clone();
        reschematized.input_schema = serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "limit": {"type": "number"}},
            "required": ["path"]
        });
        variants.push(spec_with_builtin(reschematized));

        let mut backgrounded = base.clone();
        backgrounded.execution_policy = ToolExecutionPolicy::BackgroundOnly;
        variants.push(spec_with_builtin(backgrounded));

        let mut model_selected = base.clone();
        model_selected.execution_policy = ToolExecutionPolicy::ModelSelectable;
        variants.push(spec_with_builtin(model_selected));

        let mut parallel = base.clone();
        parallel.concurrency_policy = ToolConcurrencyPolicy::Parallel;
        variants.push(spec_with_builtin(parallel));

        let mut gated = base.clone();
        gated.approval_policy = ToolApprovalPolicy::Always;
        variants.push(spec_with_builtin(gated));

        // `replay_policy` participates deliberately: it is the frozen
        // declaration of whether re-execution after an unknown outcome is
        // permitted, and it must not be silently equated with `Never`.
        let mut replayable = base.clone();
        replayable.replay_policy = ToolReplayPolicy::Idempotent;
        variants.push(spec_with_builtin(replayable));

        // The premise of the whole test: the stable identity really is stable
        // across every variant, so nothing but the definition can be talking.
        for spec in &variants {
            let ResolvedSubagentTool::Builtin { tool_id, name, .. } =
                spec.tools.first().expect("one frozen tool")
            else {
                panic!("the variants are Builtin tools");
            };
            assert_eq!(tool_id, &ToolId::new("tool-read"));
            assert_eq!(name, "read");
        }

        assert_distinct(&variants, "a frozen Builtin ToolDefinition field");
    }

    /// The input schema is framed through the rustX-owned canonical JSON
    /// writer, so two semantically identical schemas are one profile however
    /// their object keys were inserted, and object key order can never move a
    /// durable identity.
    ///
    /// The writer sorts keys itself rather than trusting them to arrive
    /// sorted, which is what keeps this true if `serde_json`'s map
    /// implementation is ever switched to one that preserves insertion order.
    #[test]
    fn sub258_builtin_input_schema_framing_is_object_key_order_independent() {
        let ordered = |keys: [(&str, serde_json::Value); 3]| {
            let mut object = serde_json::Map::new();
            for (key, value) in keys {
                object.insert(key.to_owned(), value);
            }
            serde_json::Value::Object(object)
        };
        let properties = serde_json::json!({"path": {"type": "string"}});
        let forwards = ordered([
            ("additionalProperties", serde_json::json!(false)),
            ("properties", properties.clone()),
            ("type", serde_json::json!("object")),
        ]);
        let backwards = ordered([
            ("type", serde_json::json!("object")),
            ("properties", properties),
            ("additionalProperties", serde_json::json!(false)),
        ]);

        // The framing really does canonicalize rather than echo an order.
        assert_eq!(
            crate::tools::mcp::identity::canonical_json(&forwards),
            r#"{"additionalProperties":false,"properties":{"path":{"type":"string"}},"type":"object"}"#
        );

        let mut first = frozen_builtin_definition();
        first.input_schema = forwards;
        let mut second = frozen_builtin_definition();
        second.input_schema = backwards;
        assert_eq!(
            spec_with_builtin(first).profile_digest(),
            spec_with_builtin(second).profile_digest(),
            "one semantic schema is one effective profile"
        );

        // Array order, by contrast, is semantic in JSON Schema and is never
        // sorted away.
        let mut ascending = frozen_builtin_definition();
        ascending.input_schema = serde_json::json!({"required": ["a", "b"]});
        let mut descending = frozen_builtin_definition();
        descending.input_schema = serde_json::json!({"required": ["b", "a"]});
        assert_ne!(
            spec_with_builtin(ascending).profile_digest(),
            spec_with_builtin(descending).profile_digest(),
            "JSON Schema array order is meaning"
        );
    }

    /// An MCP tool frames the same complete definition **and** its frozen
    /// cross-process identity, and neither substitutes for the other.
    ///
    /// `SourceToolIdentity` commits the server-published contract — name,
    /// description, canonical schema, and the three invocation policies — but
    /// deliberately not `replay_policy` or `ToolId`. Relying on it alone would
    /// therefore reintroduce exactly the collision this issue corrects, on the
    /// MCP side. The identity stays framed because it is an independently
    /// frozen field that gates the child's startup: the child recomputes it
    /// from its own `tools/list` and refuses to run on a mismatch.
    #[test]
    fn sub258_an_mcp_tool_frames_both_its_definition_and_its_frozen_identity() {
        let server = McpServerId::new("github");
        let definition = ToolDefinition {
            id: ToolId::new("mcp-github-get_issue"),
            name: "get_issue".to_owned(),
            description: "Reads one issue.".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Mcp {
                server_id: server.clone(),
            },
        };
        let identity_of = |definition: &ToolDefinition| {
            crate::tools::mcp::identity::definition_identity(definition)
                .expect("an MCP definition has a cross-process identity")
        };
        let spec_with = |definition: ToolDefinition| {
            let mut spec = frozen_spec();
            spec.tools = vec![ResolvedSubagentTool::Source {
                source_id: crate::capabilities::ToolSourceId::Mcp(server.clone()),
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
                identity: identity_of(&definition),
                definition,
            }];
            spec
        };

        // A field the cross-process identity does not cover still separates
        // two effective profiles, because the complete definition is framed.
        let mut replayable = definition.clone();
        replayable.replay_policy = ToolReplayPolicy::Idempotent;
        assert_eq!(
            identity_of(&definition),
            identity_of(&replayable),
            "MCP cross-process identity behavior is unchanged by this issue"
        );
        assert_ne!(
            spec_with(definition.clone()).profile_digest(),
            spec_with(replayable).profile_digest(),
            "a frozen field outside the MCP identity still identifies the profile"
        );

        // A field the cross-process identity does cover moves both.
        let mut gated = definition.clone();
        gated.approval_policy = ToolApprovalPolicy::Always;
        assert_ne!(identity_of(&definition), identity_of(&gated));
        assert_ne!(
            spec_with(definition.clone()).profile_digest(),
            spec_with(gated).profile_digest()
        );

        // And the frozen identity is itself framed: a specification that
        // crossed the boundary carrying an identity its definition does not
        // derive is a different — failing — execution profile.
        let mut mismatched = spec_with(definition.clone());
        let ResolvedSubagentTool::Source { identity, .. } =
            mismatched.tools.first_mut().expect("one frozen tool")
        else {
            panic!("the variant is an MCP tool");
        };
        *identity = crate::runtime::identity::SourceToolIdentity::new("sha256:00");
        assert_ne!(
            spec_with(definition).profile_digest(),
            mismatched.profile_digest(),
            "the frozen cross-process identity is part of the frozen contract"
        );
    }

    /// The same semantic Builtin definition arriving through the real
    /// `freeze_tool` path digests identically however it was constructed, and
    /// the physical materialization plane behind an MCP tool stays out.
    ///
    /// The first half proves the framing is tested against the actual frozen
    /// `ResolvedSubagentTool` rather than only against a hash helper; the
    /// second half proves the deliberate exclusions did not accidentally
    /// become inclusions when the tool framing grew.
    #[test]
    fn sub258_the_tool_framing_covers_the_real_frozen_value_and_excludes_the_physical_plane() {
        let definition = frozen_builtin_definition();
        let frozen = freeze_tool(
            &AgentToolSelection::Builtin {
                name: "read".to_owned(),
            },
            &definition,
        );
        let mut resolved = frozen_spec();
        resolved.tools = vec![frozen];
        assert_eq!(
            resolved.profile_digest(),
            spec_with_builtin(definition).profile_digest(),
            "the framing identifies the value the resolver actually freezes"
        );

        // Transport, credentials and resource root are physical or secret;
        // only the server identity a required source contributes is framed.
        let server = McpServerId::new("github");
        let binding = |command: &str| crate::tools::mcp::McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            activation: crate::capabilities::activation::SourceActivation::default(),
            resource_workspace: None,
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: command.to_owned(),
                args: Vec::new(),
                cwd: None,
                environment: std::collections::BTreeMap::new(),
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
        };
        let plane = |command: &str| {
            let mut spec = frozen_spec();
            spec.materialization = super::ResolvedSubagentMaterialization {
                sources: [(
                    crate::capabilities::ToolSourceId::Mcp(server.clone()),
                    binding(command),
                )]
                .into_iter()
                .collect(),
            };
            spec
        };
        assert_eq!(
            plane("/usr/bin/server").profile_digest(),
            plane("/opt/relocated/server").profile_digest(),
            "a physical launch plane never identifies an effective profile"
        );
    }

    /// A DISABLED Time contributor never executes, so no timezone spelling may
    /// separate two otherwise identical effective profiles — while an ENABLED
    /// one's effective zone still does.
    ///
    /// This is the spec-level half of the rule; `crate::extensions` proves the
    /// same thing on the framing itself, and proves that the *source
    /// definition* digest deliberately keeps distinguishing an authored zone.
    #[test]
    fn sub258_a_disabled_time_contributor_has_no_effective_timezone_in_the_profile() {
        let spec_with_time = |enabled: bool, timezone: Option<chrono_tz::Tz>| {
            let mut spec = frozen_spec();
            spec.extensions = crate::extensions::NativeAgentExtensions::with_agent_status(
                crate::context::AgentStatusConfig {
                    time: crate::context::TimeStatusConfig { enabled, timezone },
                    background: crate::context::BackgroundStatusConfig { enabled: true },
                },
            );
            spec
        };

        let disabled_utc = spec_with_time(false, Some(chrono_tz::UTC)).profile_digest();
        assert_eq!(
            disabled_utc,
            spec_with_time(false, Some(chrono_tz::Asia::Shanghai)).profile_digest(),
            "a zone that never renders cannot identify a frozen child"
        );
        assert_eq!(
            disabled_utc,
            spec_with_time(false, None).profile_digest(),
            "an omitted zone is the same inactive configuration"
        );

        let enabled_utc = spec_with_time(true, Some(chrono_tz::UTC)).profile_digest();
        assert_ne!(
            enabled_utc,
            spec_with_time(true, Some(chrono_tz::Asia::Shanghai)).profile_digest(),
            "an enabled contributor's zone is behavior"
        );
        assert_eq!(
            enabled_utc,
            spec_with_time(true, None).profile_digest(),
            "an omitted zone renders UTC, so the two are one effective profile"
        );
        assert_ne!(
            enabled_utc, disabled_utc,
            "enabling Time is itself behavior and stays framed"
        );
    }

    /// A Skill's model-visible **description** is part of the child's
    /// execution contract, and the physical plane behind it is not.
    ///
    /// The same audit that corrected the Builtin Tool framing found the same
    /// shape here: `version_id` is a content digest of the package, but the
    /// child does not re-derive `catalog_entry` from the materialized bytes —
    /// it takes the parent's frozen strings verbatim and only remaps
    /// `location`. So the description reaches the child's model exactly as
    /// frozen, drives progressive disclosure, and must identify the profile.
    #[test]
    fn sub258_a_skill_description_identifies_the_profile_and_its_paths_do_not() {
        let skill = |description: &str, source_root: &str, location: &str| {
            let mut spec = frozen_spec();
            spec.skills = vec![ResolvedSubagentSkill {
                binding: crate::protocol::manifest::SkillBinding {
                    skill_id: SkillId::new("code-review"),
                    version_id: SkillVersionId::new("sha256:abc"),
                },
                catalog_entry: crate::skills::SkillCatalogEntry {
                    name: "code-review".to_owned(),
                    description: description.to_owned(),
                    location: location.to_owned(),
                },
                source_root: std::path::PathBuf::from(source_root),
                files: vec![std::path::PathBuf::from("SKILL.md")],
            }];
            spec
        };
        let base = skill(
            "Reviews a diff.",
            "/w/.skills/code-review",
            "/w/.skills/code-review/SKILL.md",
        );

        assert_ne!(
            base.profile_digest(),
            skill(
                "Reviews a diff, adversarially.",
                "/w/.skills/code-review",
                "/w/.skills/code-review/SKILL.md"
            )
            .profile_digest(),
            "the description the model reads is part of the child's contract"
        );
        assert_eq!(
            base.profile_digest(),
            skill(
                "Reviews a diff.",
                "/other/root/code-review",
                "/other/root/code-review/SKILL.md"
            )
            .profile_digest(),
            "a host source root and a remapped location are physical, not identity"
        );

        let mut renamed = base.clone();
        renamed.skills[0].catalog_entry.name = "review".to_owned();
        assert_ne!(base.profile_digest(), renamed.profile_digest());

        let mut reversioned = base.clone();
        reversioned.skills[0].binding.version_id = SkillVersionId::new("sha256:def");
        assert_ne!(base.profile_digest(), reversioned.profile_digest());
    }

    fn assert_distinct(specs: &[ResolvedSubagentSpec], what: &str) {
        let digests: Vec<_> = specs
            .iter()
            .map(|spec| spec.profile_digest().as_str().to_owned())
            .collect();
        let mut unique = digests.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            digests.len(),
            "{what} must change the effective profile identity"
        );
    }
}
