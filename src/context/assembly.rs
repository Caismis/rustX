//! The single rustX-owned Context Assembly contract (Issue #55).
//!
//! Contributors only produce transient, bounded proposals against one finite
//! immutable invocation snapshot.  This module assigns semantic lanes,
//! stable contributor identities, trusted provenance, deterministic order,
//! and the exact system-section family.  It never owns canonical history,
//! Surface mutation, admission, cancellation, or provider dispatch.

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::conversation::SurfaceRevision;
use crate::message::types::{ContextKind, MessageBlock, UserContentBlock, UserSource};
use crate::runtime::identity::{
    AttemptId, CapabilityRevision, CertifiedExtensionIdentity, ContextContributorIdentity,
    ConversationId, NativeContextContributor,
};

/// The ABI version of the native context contribution contract.
pub const CONTEXT_COMPATIBILITY_ABI_VERSION: u32 = 3;

/// The finite user-context semantic lanes owned by rustX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserContextLane {
    /// Claimed inbound input. This is not extension-controlled.
    ClaimedInbound,
    /// The semantic lane of the **native runtime observation owner**
    /// ([`NativeContextContributor::RuntimeToolObservation`]).
    ///
    /// This lane describes *who owns the fact*, not *when the fact became
    /// eligible*. Deferred proposals produced by a certified extension keep
    /// that extension's own semantics and land in
    /// [`UserContextLane::ExtensionEnvironment`]; only the native runtime
    /// observation owner's facts belong here. The lane is native-reserved:
    /// no extension can claim it.
    ///
    /// The lane sits immediately after claimed inbound because a native
    /// runtime observation describes what the environment just did for the
    /// tool batch that precedes this step, while the request-time
    /// workspace/extension and Agent Status lanes describe the
    /// *current* step.
    RuntimeToolObservation,
    /// Generic certified-extension/environment context.
    ExtensionEnvironment,
    /// Native runtime/Agent Status context.
    AgentStatus,
}

impl UserContextLane {
    /// The contract's deterministic total order.
    pub const ALL: [Self; 4] = [
        Self::ClaimedInbound,
        Self::RuntimeToolObservation,
        Self::ExtensionEnvironment,
        Self::AgentStatus,
    ];

    /// Stable manifest spelling of one user-context lane.
    #[must_use]
    pub const fn manifest_name(self) -> &'static str {
        match self {
            Self::ClaimedInbound => "claimed_inbound",
            Self::RuntimeToolObservation => "runtime_tool_observation",
            Self::ExtensionEnvironment => "extension_environment",
            Self::AgentStatus => "agent_status",
        }
    }
}

/// The finite request-time system-section lanes owned by rustX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemSectionLane {
    /// Core runtime/system identity. Single-owner and native-reserved.
    CoreRuntimeIdentity,
    /// Agent profile/persona. Single-owner and native-reserved.
    AgentProfile,
    /// Runtime-loaded workspace/project instructions. Single-owner and
    /// native-reserved; never canonical conversation history.
    WorkspaceInstructions,
    /// Certified-extension sections, ordered by logical contributor identity.
    CertifiedExtension,
    /// Native capability guidance sections, including the request-time Skill
    /// catalog, rather than conversational User context facts.
    NativeCapabilityGuidance,
}

impl SystemSectionLane {
    /// The contract's deterministic total order.
    pub const ALL: [Self; 5] = [
        Self::CoreRuntimeIdentity,
        Self::AgentProfile,
        Self::WorkspaceInstructions,
        Self::CertifiedExtension,
        Self::NativeCapabilityGuidance,
    ];

    /// Stable manifest spelling of one system-section lane.
    #[must_use]
    pub const fn manifest_name(self) -> &'static str {
        match self {
            Self::CoreRuntimeIdentity => "core_runtime_identity",
            Self::AgentProfile => "agent_profile",
            Self::WorkspaceInstructions => "workspace_instructions",
            Self::CertifiedExtension => "certified_extension",
            Self::NativeCapabilityGuidance => "native_capability_guidance",
        }
    }
}

/// The kind of transient proposal a contributor may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextProposalKind {
    /// A bounded model-visible canonical User context fact.
    UserMessage,
}

impl ContextProposalKind {
    /// Every proposal kind accepted by the core assembly contract.
    pub const ALL: [Self; 1] = [Self::UserMessage];
}

/// The finite immutable input visible to one contributor invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContributorInputSnapshot {
    /// The admitted attempt identity.
    pub attempt_id: AttemptId,
    /// The owning conversation identity.
    pub conversation_id: ConversationId,
    /// The primary model turn number.
    pub turn: u32,
    /// The exact current Surface revision/reference.
    pub surface_revision: SurfaceRevision,
    /// The finite active Surface identities at invocation time.
    pub surface_ids: Vec<crate::runtime::identity::MessageId>,
    /// The claimed inbound batch, already committed by the host and copied
    /// into this immutable invocation value.
    pub claimed_inbound: Vec<MessageBlock>,
    /// The canonical workspace identity where a contributor needs it.
    pub workspace_root: PathBuf,
    /// The immutable capability generation observed by this attempt.
    pub capability_revision: CapabilityRevision,
}

/// Native values already sampled by rustX before assembly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeContextInput {
    /// Workspace/project instructions, when the native workspace owner has
    /// one for this request.
    pub workspace_instructions: Option<String>,
    /// Native Skill/capability guidance rendered from the attempt's immutable
    /// capability snapshot for the request-time Effective System Prompt.
    pub skill_guidance: Option<String>,
    /// The canonical rendered Agent Status snapshot.
    pub agent_status: Option<String>,
    /// Core runtime/system identity content for the effective system prompt.
    pub core_runtime_identity: Option<String>,
    /// Agent profile/persona content for the effective system prompt.
    pub agent_profile: Option<String>,
}

/// The semantic owner a deferred proposal is produced *for*.
///
/// # This is a reference, not a credential
///
/// A lifecycle observer is *bound* to a semantic owner; it never *establishes*
/// one. Registering an observer under a [`CertifiedExtensionIdentity`] proves
/// nothing about that extension: the identity is a plain validated string any
/// caller can construct. [`ContextAssembly`] resolves this reference against
/// its own registered extensions — the single semantic admission authority —
/// and rejects a producer it does not know. Certification, attestation, and
/// provenance therefore keep exactly one source of truth, and the lifecycle
/// seam cannot become a second extension registry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "producer", rename_all = "snake_case")]
pub enum DeferredContextProducer {
    /// The one rustX-owned native runtime observation owner
    /// ([`NativeContextContributor::RuntimeToolObservation`]). It needs no
    /// registration because rustX owns it.
    NativeRuntimeObservation,
    /// A certified extension, named by its logical key. The key is only a
    /// reference: Context Assembly must find a matching registered extension
    /// or the proposal is rejected.
    CertifiedExtension {
        /// The logical key of the extension this observer speaks for.
        identity: CertifiedExtensionIdentity,
    },
}

/// One deferred transient proposal staged by the Agent Loop before this
/// step's assembly (Issue #56).
///
/// # Timing is not provenance
///
/// A deferred proposal is one whose *eligibility* was established earlier —
/// by the immutable observation of a structurally settled tool batch — rather
/// than during this step's contributor invocation. That is a **lifecycle
/// timing** fact owned by the Agent Loop.
///
/// It says nothing about **semantic ownership**. The `producer` below is the
/// owner reference the Agent Loop stamped from the observer's *registration*,
/// never from anything the observer returned, and Context Assembly resolves it
/// to an authoritative registration before deriving the lane, the trusted
/// [`UserSource`], and the [`ContextKind`]. A certified extension therefore
/// keeps its extension provenance and its own lane when it produces deferred
/// context; nothing is rewritten into native runtime context because of when
/// it was produced, and nothing gains extension provenance without being a
/// registered extension.
///
/// The proposal is a [`UserMessageProposal`] and nothing else. A settled tool
/// batch is a conversational fact, so the deferred seam publishes
/// conversational context; the Effective System Prompt stays owned by the
/// request-time contributor path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredContextProposal {
    /// The semantic owner this proposal is produced for, pending resolution
    /// against the authoritative Context Assembly registration.
    pub producer: DeferredContextProducer,
    /// The transient bounded User context exactly as the producer returned it.
    pub proposal: UserMessageProposal,
}

/// Whether a contribution's eligibility was established before this step or
/// during it.
///
/// The phase is only an ordering tiebreak *inside* one `(lane, contributor)`
/// bucket: a deferred fact describes the tool batch that precedes the step, so
/// it precedes the same owner's request-time fact. It never selects a lane, a
/// provenance, or a semantic family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ContributionPhase {
    /// Staged before this step by a settled tool-batch observation.
    Deferred,
    /// Produced by this step's own assembly invocation.
    RequestTime,
}

/// The canonical user-context semantics of one contributor identity.
///
/// This is the single table that maps *semantic ownership* to a lane, a
/// trusted provenance, and a semantic family. Request-time and deferred
/// proposals of the same owner resolve through it identically, which is what
/// makes post-tool timing incapable of rewriting provenance.
///
/// `None` means the owner publishes no model-visible User context at all (it
/// is an effective-system-prompt owner), so such a proposal is invalid.
fn user_semantics(
    identity: &ContextContributorIdentity,
) -> Option<(UserContextLane, UserSource, ContextKind)> {
    match identity {
        ContextContributorIdentity::Native(owner) => match owner {
            NativeContextContributor::AgentStatus => Some((
                UserContextLane::AgentStatus,
                UserSource::Runtime,
                ContextKind::AgentStatus,
            )),
            NativeContextContributor::RuntimeToolObservation => Some((
                UserContextLane::RuntimeToolObservation,
                UserSource::Runtime,
                ContextKind::RuntimeToolObservation,
            )),
            NativeContextContributor::WorkspaceInstructions
            | NativeContextContributor::SkillGuidance
            | NativeContextContributor::CoreSystemIdentity
            | NativeContextContributor::AgentProfile => None,
        },
        ContextContributorIdentity::CertifiedExtension(extension) => Some((
            UserContextLane::ExtensionEnvironment,
            UserSource::Extension {
                contributor: extension.clone(),
            },
            ContextKind::ExtensionEnvironment,
        )),
    }
}

/// [`user_semantics`] as a fallible lookup: an owner that publishes no
/// model-visible User context cannot propose one, whatever its timing.
fn user_lane_of(
    identity: &ContextContributorIdentity,
) -> Result<(UserContextLane, UserSource, ContextKind), ContextAssemblyError> {
    user_semantics(identity).ok_or_else(|| {
        ContextAssemblyError::InvalidProposal(format!(
            "contributor {identity:?} owns no model-visible User context lane"
        ))
    })
}

/// A transient User context proposal. It contains no id, source, kind, lane,
/// or mutable runtime handle; rustX supplies all of those at admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessageProposal {
    /// Complete bounded User content.
    pub content: Vec<UserContentBlock>,
}

/// One transient proposal returned by a contributor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextProposal {
    /// A normal model-visible canonical User context fact.
    UserMessage(UserMessageProposal),
}

impl ContextProposal {
    /// The proposal kind.
    #[must_use]
    pub const fn kind(&self) -> ContextProposalKind {
        match self {
            Self::UserMessage(_) => ContextProposalKind::UserMessage,
        }
    }
}

/// The contributor API. Implementations receive only a finite immutable
/// snapshot and return transient typed proposals.
pub trait ContextContributor: Send + Sync {
    /// Produce bounded proposals for one primary model step.
    ///
    /// # Errors
    ///
    /// Returns a context assembly error when the contributor cannot produce
    /// a bounded proposal batch.
    fn contribute<'a>(
        &'a self,
        input: &'a ContributorInputSnapshot,
    ) -> BoxFuture<'a, Result<Vec<ContextProposal>, ContextAssemblyError>>;
}

impl<F> ContextContributor for F
where
    F: Fn(&ContributorInputSnapshot) -> Result<Vec<ContextProposal>, ContextAssemblyError>
        + Send
        + Sync,
{
    fn contribute<'a>(
        &'a self,
        input: &'a ContributorInputSnapshot,
    ) -> BoxFuture<'a, Result<Vec<ContextProposal>, ContextAssemblyError>> {
        Box::pin(async move { self(input) })
    }
}

/// The independent attestation/content generation recorded beside a logical
/// contributor identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContributorGeneration {
    /// Stable logical identity used for deterministic ordering.
    pub identity: ContextContributorIdentity,
    /// Optional attestation/package/content generation. It does not affect
    /// ordering.
    pub attestation: Option<String>,
}

/// One frozen context assembly generation accepted for a primary step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextGeneration {
    /// Monotonic per-attempt generation assigned by the Agent Loop.
    pub id: u64,
    /// The active contributor identities and their attestation generations.
    pub contributors: Vec<ContributorGeneration>,
}

impl ContextGeneration {
    /// Sets the Agent Loop-owned generation number.
    #[must_use]
    pub const fn with_id(mut self, id: u64) -> Self {
        self.id = id;
        self
    }
}

/// A canonical User message draft after assembly validation. It still has no
/// `MessageId`; the conversation admission owner allocates one exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedUserContext {
    /// Trusted provenance assigned by rustX.
    pub source: UserSource,
    /// Trusted semantic family assigned by rustX.
    pub kind: ContextKind,
    /// Complete User content.
    pub content: Vec<UserContentBlock>,
}

/// One accepted system section after lane/identity assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedSystemSection {
    /// The semantic system lane.
    pub lane: SystemSectionLane,
    /// The contributor that owns the section.
    pub contributor: ContextContributorIdentity,
    /// The exact section content.
    pub content: String,
}

/// The validated result of one assembly invocation. This value is transient
/// until the Agent Loop crosses its admission boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedContext {
    /// Canonical User context facts awaiting core `MessageId` allocation.
    pub user_messages: Vec<AcceptedUserContext>,
    /// Request-time system sections awaiting effective-prompt rendering.
    pub system_sections: Vec<AcceptedSystemSection>,
    /// The identity/generation explanation of the accepted assembly.
    pub generation: ContextGeneration,
}

/// A machine-readable projection of the real native contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCompatibilityManifest {
    /// Compatibility ABI version.
    pub abi_version: u32,
    /// All user context lanes in their canonical order.
    pub user_context_lanes: Vec<UserContextLane>,
    /// All system-section lanes in their canonical order.
    pub system_section_lanes: Vec<SystemSectionLane>,
    /// Native-reserved single-owner slots.
    pub reserved_native_slots: Vec<String>,
    /// Multi-extension lanes.
    pub multi_extension_slots: Vec<String>,
    /// Trusted provenance namespaces.
    pub provenance_namespaces: Vec<String>,
    /// Proposal kinds accepted by the core contract.
    pub allowed_proposal_kinds: Vec<ContextProposalKind>,
}

impl ContextCompatibilityManifest {
    /// Derives the manifest directly from the finite lane/provenance
    /// contract constants used by assembly validation.
    #[must_use]
    pub fn native() -> Self {
        Self {
            abi_version: CONTEXT_COMPATIBILITY_ABI_VERSION,
            user_context_lanes: UserContextLane::ALL.to_vec(),
            system_section_lanes: SystemSectionLane::ALL.to_vec(),
            reserved_native_slots: NativeContextContributor::ALL
                .into_iter()
                .map(|owner| owner.manifest_name().to_owned())
                .collect(),
            multi_extension_slots: vec![
                UserContextLane::ExtensionEnvironment
                    .manifest_name()
                    .to_owned(),
                SystemSectionLane::CertifiedExtension
                    .manifest_name()
                    .to_owned(),
            ],
            provenance_namespaces: crate::message::types::UserSource::PROVENANCE_NAMESPACES
                .into_iter()
                .map(str::to_owned)
                .collect(),
            allowed_proposal_kinds: ContextProposalKind::ALL.to_vec(),
        }
    }
}

/// Assembly failures are explicit and transactional: no proposal is
/// partially accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextAssemblyError {
    /// A logical extension identity failed canonicalization.
    InvalidContributorIdentity(String),
    /// An extension attempted to use a native-reserved logical key.
    ReservedNativeIdentity(String),
    /// A second single-owner contributor was registered.
    DuplicateSingleOwner(String),
    /// A contributor returned too many proposals.
    ProposalLimitExceeded,
    /// A proposal body was empty or too large.
    InvalidProposal(String),
    /// A contributor failed while producing proposals.
    ContributorFailed(String),
    /// A deferred proposal named a semantic owner this assembly never
    /// registered.
    ///
    /// Lifecycle registration binds an observer to an owner; it does not
    /// establish one. Only [`ContextAssembly::register_extension`] admits a
    /// certified extension, so an unknown producer is rejected outright rather
    /// than being given synthesized provenance.
    UnregisteredContributor(String),
}

impl core::fmt::Display for ContextAssemblyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidContributorIdentity(detail) => {
                write!(f, "invalid contributor identity: {detail}")
            }
            Self::ReservedNativeIdentity(identity) => {
                write!(f, "extension identity {identity:?} is native-reserved")
            }
            Self::DuplicateSingleOwner(slot) => {
                write!(f, "semantic slot {slot:?} already has an owner")
            }
            Self::ProposalLimitExceeded => {
                f.write_str("context contributor proposal limit exceeded")
            }
            Self::InvalidProposal(detail) => write!(f, "invalid context proposal: {detail}"),
            Self::ContributorFailed(detail) => write!(f, "context contributor failed: {detail}"),
            Self::UnregisteredContributor(identity) => write!(
                f,
                "deferred context names contributor {identity:?}, which is not a registered \
                 certified extension of this attempt"
            ),
        }
    }
}

impl std::error::Error for ContextAssemblyError {}

/// The bounded number of transient proposals one contributor may return for
/// one primary step.
pub const MAX_PROPOSALS_PER_CONTRIBUTOR: usize = 128;

/// The bounded number of deferred proposals one primary step may carry, over
/// all producers together.
///
/// The Agent Loop enforces this same bound at its observer transaction
/// boundary, so an unbounded observation pass is rejected before anything is
/// staged; assembly re-checks it because assembly never trusts its input.
pub const MAX_DEFERRED_CONTEXT_PROPOSALS: usize = 128;

const MAX_CONTEXT_TEXT_BYTES: usize = 1024 * 1024;

/// Validates one transient User context proposal against the bounded content
/// contract.
///
/// This is the exact content validation [`ContextAssembly::assemble`] applies.
/// It is public so the Agent Loop can run it at its observer transaction
/// boundary and reject an oversized or empty deferred proposal *before* it is
/// staged, instead of discovering it one step later.
///
/// # Errors
///
/// Returns [`ContextAssemblyError::InvalidProposal`] when the proposal body is
/// empty or exceeds the bounded context size.
pub fn validate_user_message_proposal(
    proposal: &UserMessageProposal,
) -> Result<(), ContextAssemblyError> {
    text_content(&proposal.content).map(|_| ())
}

#[derive(Clone)]
struct RegisteredExtension {
    identity: CertifiedExtensionIdentity,
    generation: ContributorGeneration,
    system_sections: Vec<String>,
    contributor: Arc<dyn ContextContributor>,
}

/// The rustX-owned Context Assembly registry and validator.
#[derive(Clone, Default)]
pub struct ContextAssembly {
    extensions: Vec<RegisteredExtension>,
}

impl core::fmt::Debug for ContextAssembly {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ContextAssembly")
            .field(
                "extensions",
                &self
                    .extensions
                    .iter()
                    .map(|extension| extension.identity.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ContextAssembly {
    /// Creates an empty assembly with only native core owners available.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            extensions: Vec::new(),
        }
    }

    /// Registers one certified extension. The logical key is canonicalized
    /// and is the sole ordering identity; attestation is recorded separately.
    ///
    /// # Errors
    ///
    /// Returns an error when the logical identity is invalid, native-reserved,
    /// or already registered.
    pub fn register_extension(
        &mut self,
        logical_key: impl Into<String>,
        attestation: Option<String>,
        contributor: Arc<dyn ContextContributor>,
    ) -> Result<CertifiedExtensionIdentity, ContextAssemblyError> {
        let identity = CertifiedExtensionIdentity::new(logical_key)
            .map_err(ContextAssemblyError::InvalidContributorIdentity)?;
        if is_reserved_extension_key(identity.as_str()) {
            return Err(ContextAssemblyError::ReservedNativeIdentity(
                identity.as_str().to_owned(),
            ));
        }
        if self
            .extensions
            .iter()
            .any(|registered| registered.identity == identity)
        {
            return Err(ContextAssemblyError::DuplicateSingleOwner(format!(
                "extension:{}",
                identity.as_str()
            )));
        }
        let generation = ContributorGeneration {
            identity: ContextContributorIdentity::CertifiedExtension(identity.clone()),
            attestation,
        };
        self.extensions.push(RegisteredExtension {
            identity: identity.clone(),
            generation,
            system_sections: Vec::new(),
            contributor,
        });
        Ok(identity)
    }

    /// Adds one immutable System section to a registered extension resource.
    /// The section is copied into every [`ContextAssembly`] clone and is
    /// therefore frozen by the owning Runtime Resource Snapshot. Dynamic
    /// request-time contributors can publish only conversational User facts.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown extension, empty/oversized content, or
    /// more than the bounded number of sections.
    pub fn register_extension_system_section(
        &mut self,
        identity: &CertifiedExtensionIdentity,
        content: impl Into<String>,
    ) -> Result<(), ContextAssemblyError> {
        let content = content.into();
        validate_text(&content, "extension system section")?;
        let registered = self
            .extensions
            .iter_mut()
            .find(|registered| &registered.identity == identity)
            .ok_or_else(|| {
                ContextAssemblyError::UnregisteredContributor(identity.as_str().to_owned())
            })?;
        if registered.system_sections.len() >= MAX_PROPOSALS_PER_CONTRIBUTOR {
            return Err(ContextAssemblyError::ProposalLimitExceeded);
        }
        registered.system_sections.push(content);
        Ok(())
    }

    /// Returns the registered extension identities in canonical logical order.
    #[must_use]
    pub fn extension_identities(&self) -> Vec<CertifiedExtensionIdentity> {
        let mut identities = self
            .extensions
            .iter()
            .map(|extension| extension.identity.clone())
            .collect::<Vec<_>>();
        identities.sort();
        identities
    }

    /// The mechanically derived native compatibility manifest.
    #[must_use]
    pub fn compatibility_manifest() -> ContextCompatibilityManifest {
        ContextCompatibilityManifest::native()
    }

    /// Returns the exact resource-frozen System sections plus native values
    /// for this generation. This path invokes no dynamic contributor and is
    /// shared by primary request assembly and manual compaction accounting.
    pub(crate) fn system_sections(
        &self,
        native: &NativeContextInput,
    ) -> Result<Vec<AcceptedSystemSection>, ContextAssemblyError> {
        let mut sections = native_system_sections(native)?;
        for registered in &self.extensions {
            for content in &registered.system_sections {
                sections.push(AcceptedSystemSection {
                    lane: SystemSectionLane::CertifiedExtension,
                    contributor: registered.generation.identity.clone(),
                    content: content.clone(),
                });
            }
        }
        sections.sort_by(|left, right| {
            left.lane
                .cmp(&right.lane)
                .then_with(|| left.contributor.cmp(&right.contributor))
        });
        Ok(sections)
    }

    /// Resolves one deferred producer reference to a trusted contributor
    /// identity and its **authoritative** generation.
    ///
    /// This is the single semantic admission authority for deferred context.
    /// A [`DeferredContextProducer::CertifiedExtension`] reference carries only
    /// a logical key; the registration recorded here — with its attestation —
    /// is what makes it an extension. A lifecycle observer that names an
    /// unregistered extension therefore gets no lane, no
    /// [`UserSource::Extension`] provenance, and no synthesized generation:
    /// the whole assembly fails before any context can be admitted.
    ///
    /// The native runtime observation owner needs no registration because
    /// rustX owns it; it has no attestation for the same reason.
    fn resolve_deferred_producer(
        &self,
        producer: &DeferredContextProducer,
    ) -> Result<ContributorGeneration, ContextAssemblyError> {
        match producer {
            DeferredContextProducer::NativeRuntimeObservation => Ok(native_generation(
                NativeContextContributor::RuntimeToolObservation,
            )),
            DeferredContextProducer::CertifiedExtension { identity } => self
                .extensions
                .iter()
                .find(|registered| &registered.identity == identity)
                .map(|registered| registered.generation.clone())
                .ok_or_else(|| {
                    ContextAssemblyError::UnregisteredContributor(identity.as_str().to_owned())
                }),
        }
    }

    /// Assembles deferred, native, and certified-extension proposals against
    /// one finite immutable input snapshot. Returned User messages have
    /// trusted source/kind but no `MessageId`; the Agent Loop allocates and
    /// commits ids at its one admission boundary. Request-time native system
    /// sections, including Skill capability guidance, are returned separately
    /// and never enter that User-message admission path.
    ///
    /// `deferred` carries the proposals whose eligibility the Agent Loop
    /// established earlier, at the observer transaction boundary of a settled
    /// tool batch. They are validated, laned, and given provenance here by the
    /// *same* rules as request-time proposals of the same semantic owner:
    /// assembly reads their producer identity and nothing else about their
    /// timing.
    ///
    /// # Errors
    ///
    /// Returns an error when native values, deferred proposals, or contributor
    /// proposals violate the bounded assembly contract.
    #[allow(clippy::too_many_lines)]
    pub async fn assemble(
        &self,
        input: &ContributorInputSnapshot,
        native: &NativeContextInput,
        deferred: &[DeferredContextProposal],
    ) -> Result<AcceptedContext, ContextAssemblyError> {
        let mut entries = Vec::new();
        let mut native_sections = self.system_sections(native)?;
        let mut generations = vec![
            native
                .workspace_instructions
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::WorkspaceInstructions)),
            native
                .skill_guidance
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::SkillGuidance)),
            native
                .agent_status
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::AgentStatus)),
            native
                .core_runtime_identity
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::CoreSystemIdentity)),
            native
                .agent_profile
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::AgentProfile)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        // Deferred context. The Agent Loop already staged these in canonical
        // `(ToolCall batch position, producer identity, proposal FIFO)` order,
        // so the enumerated sequence preserves exactly that order inside each
        // producer's own lane and physical tool completion timing can never
        // reach canonical history.
        //
        // `resolve_deferred_producer` is where the single semantic admission
        // authority is enforced: a producer reference becomes a trusted
        // identity and an authoritative generation only if this assembly
        // registered it. A lifecycle observer cannot mint extension
        // provenance by naming an extension.
        if deferred.len() > MAX_DEFERRED_CONTEXT_PROPOSALS {
            return Err(ContextAssemblyError::ProposalLimitExceeded);
        }
        for (sequence, staged) in deferred.iter().enumerate() {
            let generation = self.resolve_deferred_producer(&staged.producer)?;
            let (lane, source, kind) = user_lane_of(&generation.identity)?;
            entries.push(ContributionEntry {
                lane,
                identity: generation.identity.clone(),
                source,
                kind,
                content: text_content(&staged.proposal.content)?,
                phase: ContributionPhase::Deferred,
                sequence,
            });
            generations.push(generation);
        }

        if let Some(text) = &native.agent_status {
            entries.push(ContributionEntry::native_user(
                NativeContextContributor::AgentStatus,
                text.clone(),
            )?);
        }

        let mut extensions = self.extensions.clone();
        extensions.sort_by(|left, right| left.identity.cmp(&right.identity));
        for registered in extensions {
            let proposals = registered
                .contributor
                .contribute(input)
                .await
                .map_err(|error| ContextAssemblyError::ContributorFailed(error.to_string()))?;
            if proposals.len() > MAX_PROPOSALS_PER_CONTRIBUTOR {
                return Err(ContextAssemblyError::ProposalLimitExceeded);
            }
            for (sequence, proposal) in proposals.into_iter().enumerate() {
                match proposal {
                    ContextProposal::UserMessage(message) => {
                        let (lane, source, kind) = user_lane_of(&registered.generation.identity)?;
                        let text = text_content(&message.content)?;
                        entries.push(ContributionEntry {
                            lane,
                            identity: registered.generation.identity.clone(),
                            source,
                            kind,
                            content: text,
                            phase: ContributionPhase::RequestTime,
                            sequence,
                        });
                    }
                }
            }
            generations.push(registered.generation);
        }

        entries.sort_by(|left, right| {
            left.lane
                .cmp(&right.lane)
                .then_with(|| left.identity.cmp(&right.identity))
                .then_with(|| left.phase.cmp(&right.phase))
                .then_with(|| left.sequence.cmp(&right.sequence))
        });
        // Request-time system sections only: deferred post-tool proposals are
        // `UserMessageProposal` facts, so a section can never reach this path.
        // Sections sort by lane, native slots first, then certified-extension
        // lanes; within a lane the stable sort keeps each contributor's own
        // contributed order.
        native_sections.sort_by(|left, right| {
            left.lane
                .cmp(&right.lane)
                .then_with(|| left.contributor.cmp(&right.contributor))
        });
        // One semantic owner appears exactly once in the accepted generation:
        // deferred and request-time participation of the same registered
        // extension collapse to one authoritative generation, because the
        // deferred path resolves through this same registration and its
        // generations already carry the registered attestation. No extension
        // generation is synthesized.
        generations.sort_by(|left, right| {
            left.identity
                .cmp(&right.identity)
                .then_with(|| right.attestation.cmp(&left.attestation))
        });
        generations.dedup_by(|later, first| later.identity == first.identity);

        Ok(AcceptedContext {
            user_messages: entries
                .into_iter()
                .map(|entry| AcceptedUserContext {
                    source: entry.source,
                    kind: entry.kind,
                    content: entry.content,
                })
                .collect(),
            system_sections: native_sections,
            generation: ContextGeneration {
                id: 0,
                contributors: generations,
            },
        })
    }
}

#[derive(Debug)]
struct ContributionEntry {
    lane: UserContextLane,
    identity: ContextContributorIdentity,
    source: UserSource,
    kind: ContextKind,
    content: Vec<UserContentBlock>,
    phase: ContributionPhase,
    sequence: usize,
}

impl ContributionEntry {
    /// One request-time native User context fact. Its lane, provenance, and
    /// semantic family come from the same owner table the deferred path uses.
    fn native_user(
        contributor: NativeContextContributor,
        text: String,
    ) -> Result<Self, ContextAssemblyError> {
        let identity = ContextContributorIdentity::Native(contributor);
        let (lane, source, kind) =
            user_semantics(&identity).expect("this native owner publishes User context");
        validate_text(&text, "native context")?;
        Ok(Self {
            lane,
            identity,
            source,
            kind,
            content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                text,
            })],
            phase: ContributionPhase::RequestTime,
            sequence: 0,
        })
    }
}

/// One request-time native effective-system-prompt section.
fn native_section(
    lane: SystemSectionLane,
    contributor: NativeContextContributor,
    content: String,
) -> AcceptedSystemSection {
    AcceptedSystemSection {
        lane,
        contributor: ContextContributorIdentity::Native(contributor),
        content,
    }
}

/// Validates and accepts the statically sampled native Effective System
/// Prompt sections.
///
/// Agent execution and idle manual maintenance both use this constructor so
/// capability-derived guidance has one lane/owner/validation contract. The
/// returned sections are request input only: callers may account for them,
/// but must never commit them as conversational history.
pub(crate) fn native_system_sections(
    native: &NativeContextInput,
) -> Result<Vec<AcceptedSystemSection>, ContextAssemblyError> {
    let mut sections = Vec::new();
    if let Some(text) = &native.workspace_instructions {
        validate_text(text, "workspace instructions")?;
        sections.push(native_section(
            SystemSectionLane::WorkspaceInstructions,
            NativeContextContributor::WorkspaceInstructions,
            text.clone(),
        ));
    }
    if let Some(text) = &native.skill_guidance {
        validate_text(text, "native Skill capability guidance")?;
        sections.push(native_section(
            SystemSectionLane::NativeCapabilityGuidance,
            NativeContextContributor::SkillGuidance,
            text.clone(),
        ));
    }
    if let Some(text) = &native.core_runtime_identity {
        validate_text(text, "core runtime identity")?;
        sections.push(native_section(
            SystemSectionLane::CoreRuntimeIdentity,
            NativeContextContributor::CoreSystemIdentity,
            text.clone(),
        ));
    }
    if let Some(text) = &native.agent_profile {
        validate_text(text, "agent profile")?;
        sections.push(native_section(
            SystemSectionLane::AgentProfile,
            NativeContextContributor::AgentProfile,
            text.clone(),
        ));
    }
    sections.sort_by(|left, right| {
        left.lane
            .cmp(&right.lane)
            .then_with(|| left.contributor.cmp(&right.contributor))
    });
    Ok(sections)
}

fn native_generation(contributor: NativeContextContributor) -> ContributorGeneration {
    ContributorGeneration {
        identity: ContextContributorIdentity::Native(contributor),
        attestation: None,
    }
}

fn is_reserved_extension_key(key: &str) -> bool {
    NativeContextContributor::ALL
        .into_iter()
        .any(|owner| owner.logical_key() == key)
}

fn validate_text(text: &str, label: &str) -> Result<(), ContextAssemblyError> {
    if text.trim().is_empty() {
        return Err(ContextAssemblyError::InvalidProposal(format!(
            "{label} must not be empty"
        )));
    }
    if text.len() > MAX_CONTEXT_TEXT_BYTES {
        return Err(ContextAssemblyError::InvalidProposal(format!(
            "{label} exceeds the bounded context size"
        )));
    }
    Ok(())
}

fn text_content(
    content: &[UserContentBlock],
) -> Result<Vec<UserContentBlock>, ContextAssemblyError> {
    if content.is_empty() {
        return Err(ContextAssemblyError::InvalidProposal(
            "a User context proposal must contain content".to_owned(),
        ));
    }
    let bytes = content
        .iter()
        .map(|block| match block {
            UserContentBlock::Text(text) => text.text.len(),
            UserContentBlock::Image(_) | UserContentBlock::File(_) => 0,
        })
        .sum::<usize>();
    if bytes == 0 || bytes > MAX_CONTEXT_TEXT_BYTES {
        return Err(ContextAssemblyError::InvalidProposal(
            "a User context proposal must contain bounded text content".to_owned(),
        ));
    }
    Ok(content.to_vec())
}

/// Renders the exact provider-neutral Effective System Prompt from the
/// already-admitted request-time sections. Canonical history has no System
/// role and therefore cannot become executable instruction authority.
#[must_use]
pub fn render_effective_system_prompt(sections: &[AcceptedSystemSection]) -> String {
    sections
        .iter()
        .map(|section| section.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::content::TextBlock;
    use crate::runtime::identity::MessageId;

    fn input() -> ContributorInputSnapshot {
        ContributorInputSnapshot {
            attempt_id: AttemptId::new("attempt-1"),
            conversation_id: ConversationId::new("conversation-1"),
            turn: 1,
            surface_revision: SurfaceRevision::INITIAL,
            surface_ids: vec![MessageId::new("inbound")],
            claimed_inbound: Vec::new(),
            workspace_root: PathBuf::from("/workspace"),
            capability_revision: CapabilityRevision::new(3),
        }
    }

    #[tokio::test]
    async fn extension_order_uses_logical_identity_not_registration_order() {
        let first = Arc::new(|_: &ContributorInputSnapshot| {
            Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "z".to_owned(),
                })],
            })])
        });
        let second = Arc::new(|_: &ContributorInputSnapshot| {
            Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "a".to_owned(),
                })],
            })])
        });
        let mut left = ContextAssembly::new();
        left.register_extension("zeta", Some("v1".to_owned()), first)
            .expect("zeta");
        left.register_extension("alpha", Some("v1".to_owned()), second)
            .expect("alpha");
        let mut right = ContextAssembly::new();
        right
            .register_extension(
                "alpha",
                Some("v2".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| {
                    Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "a".to_owned(),
                        })],
                    })])
                }),
            )
            .expect("alpha");
        right
            .register_extension(
                "zeta",
                Some("v9".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| {
                    Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "z".to_owned(),
                        })],
                    })])
                }),
            )
            .expect("zeta");
        let a = left
            .assemble(&input(), &NativeContextInput::default(), &[])
            .await
            .expect("left");
        let b = right
            .assemble(&input(), &NativeContextInput::default(), &[])
            .await
            .expect("right");
        assert_eq!(a.user_messages, b.user_messages);
        assert_eq!(
            a.generation.contributors[0].identity,
            b.generation.contributors[0].identity
        );
        assert_ne!(
            a.generation.contributors[0].attestation,
            b.generation.contributors[0].attestation
        );
    }

    /// Every native semantic owner's logical key is reserved, in any casing.
    /// The cases are derived from the contract constant so a new or renamed
    /// native owner cannot silently become claimable by an extension.
    #[test]
    fn extension_cannot_claim_native_identity() {
        let mut assembly = ContextAssembly::new();
        for owner in NativeContextContributor::ALL {
            for key in [
                owner.logical_key().to_owned(),
                owner.logical_key().to_ascii_uppercase(),
            ] {
                let error = assembly
                    .register_extension(
                        key,
                        None,
                        Arc::new(|_: &ContributorInputSnapshot| Ok(Vec::new())),
                    )
                    .expect_err("reserved identity must reject");
                assert!(matches!(
                    error,
                    ContextAssemblyError::ReservedNativeIdentity(_)
                ));
            }
        }
    }

    #[tokio::test]
    async fn extension_provenance_and_identity_are_core_assigned() {
        let mut assembly = ContextAssembly::new();
        assembly
            .register_extension(
                "Example.Extension",
                Some("package-generation-1".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| {
                    Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "extension context".to_owned(),
                        })],
                    })])
                }),
            )
            .expect("register extension");
        let accepted = assembly
            .assemble(&input(), &NativeContextInput::default(), &[])
            .await
            .expect("assemble extension");
        assert_eq!(accepted.user_messages.len(), 1);
        assert_eq!(
            accepted.user_messages[0].source,
            UserSource::Extension {
                contributor: CertifiedExtensionIdentity::new("example.extension")
                    .expect("identity")
            }
        );
        assert_eq!(
            accepted.user_messages[0].kind,
            ContextKind::ExtensionEnvironment
        );
        let identity = &accepted.generation.contributors[0].identity;
        let encoded = serde_json::to_string(identity).expect("serialize identity");
        let decoded: ContextContributorIdentity =
            serde_json::from_str(&encoded).expect("deserialize identity");
        assert_eq!(&decoded, identity);
    }

    #[tokio::test]
    async fn native_provenance_is_assigned_by_core() {
        let assembly = ContextAssembly::new();
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput {
                    agent_status: Some("same bytes".to_owned()),
                    ..NativeContextInput::default()
                },
                &[],
            )
            .await
            .expect("native proposal");
        assert_eq!(accepted.user_messages[0].source, UserSource::Runtime);
        assert_eq!(accepted.user_messages[0].kind, ContextKind::AgentStatus);
    }

    #[tokio::test]
    async fn system_sections_use_native_slots_and_stable_extension_order() {
        let mut assembly = ContextAssembly::new();
        let zeta = assembly
            .register_extension(
                "zeta.extension",
                Some("package-v2".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| Ok(Vec::<ContextProposal>::new())),
            )
            .expect("zeta extension");
        assembly
            .register_extension_system_section(&zeta, "zeta section")
            .expect("zeta section");
        let alpha = assembly
            .register_extension(
                "alpha.extension",
                Some("package-v1".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| Ok(Vec::<ContextProposal>::new())),
            )
            .expect("alpha extension");
        assembly
            .register_extension_system_section(&alpha, "alpha section")
            .expect("alpha section");

        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput {
                    core_runtime_identity: Some("core identity".to_owned()),
                    agent_profile: Some("agent profile".to_owned()),
                    workspace_instructions: Some("workspace".to_owned()),
                    skill_guidance: Some("skill catalog".to_owned()),
                    ..NativeContextInput::default()
                },
                &[],
            )
            .await
            .expect("system sections assemble");
        assert_eq!(
            accepted
                .system_sections
                .iter()
                .map(|section| section.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "core identity",
                "agent profile",
                "workspace",
                "alpha section",
                "zeta section",
                "skill catalog"
            ]
        );
        assert_eq!(
            accepted.system_sections[0].contributor,
            ContextContributorIdentity::Native(NativeContextContributor::CoreSystemIdentity)
        );
        assert!(accepted.user_messages.is_empty());
        assert_eq!(
            accepted.system_sections[5].contributor,
            ContextContributorIdentity::Native(NativeContextContributor::SkillGuidance)
        );
        assert_eq!(
            render_effective_system_prompt(&accepted.system_sections),
            "core identity\n\nagent profile\n\nworkspace\n\nalpha section\n\nzeta section\n\nskill catalog"
        );
    }

    #[tokio::test]
    async fn absent_skill_catalog_adds_no_system_section() {
        let accepted = ContextAssembly::new()
            .assemble(&input(), &NativeContextInput::default(), &[])
            .await
            .expect("empty native context assembles");
        assert!(accepted.user_messages.is_empty());
        assert!(accepted.system_sections.is_empty());
        assert_eq!(
            render_effective_system_prompt(&accepted.system_sections),
            ""
        );
    }

    /// Helpers for the deferred-context suite.
    fn user_message(text: &str) -> UserMessageProposal {
        UserMessageProposal {
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
        }
    }

    fn user(text: &str) -> ContextProposal {
        ContextProposal::UserMessage(user_message(text))
    }

    fn native_deferred(text: &str) -> DeferredContextProposal {
        DeferredContextProposal {
            producer: DeferredContextProducer::NativeRuntimeObservation,
            proposal: user_message(text),
        }
    }

    fn extension_deferred(key: &str, text: &str) -> DeferredContextProposal {
        DeferredContextProposal {
            producer: DeferredContextProducer::CertifiedExtension {
                identity: CertifiedExtensionIdentity::new(key).expect("identity"),
            },
            proposal: user_message(text),
        }
    }

    fn extension_identity(key: &str) -> ContextContributorIdentity {
        ContextContributorIdentity::CertifiedExtension(
            CertifiedExtensionIdentity::new(key).expect("identity"),
        )
    }

    fn extension_source(key: &str) -> UserSource {
        UserSource::Extension {
            contributor: CertifiedExtensionIdentity::new(key).expect("identity"),
        }
    }

    /// An assembly with one registered extension that contributes the given
    /// request-time text, or nothing at all when `request_time` is `None`.
    fn assembly_with_extension(
        key: &'static str,
        attestation: Option<String>,
        request_time: Option<&'static str>,
    ) -> ContextAssembly {
        let mut assembly = ContextAssembly::new();
        assembly
            .register_extension(
                key,
                attestation,
                Arc::new(move |_: &ContributorInputSnapshot| {
                    Ok(request_time
                        .map(|text| vec![user(text)])
                        .unwrap_or_default())
                }),
            )
            .expect("register extension");
        assembly
    }

    fn texts(accepted: &AcceptedContext) -> Vec<String> {
        accepted
            .user_messages
            .iter()
            .map(|message| match &message.content[0] {
                UserContentBlock::Text(text) => text.text.clone(),
                _ => unreachable!("text proposals"),
            })
            .collect()
    }

    /// A deferred proposal owned by the **native** runtime observation owner
    /// receives native runtime provenance, the native-reserved lane, and the
    /// native semantic family — because of its producer, not because it
    /// arrived after a tool batch. It needs no extension registration.
    #[tokio::test]
    async fn native_deferred_proposals_receive_native_provenance() {
        let assembly = ContextAssembly::new();
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[native_deferred("A1"), native_deferred("A2")],
            )
            .await
            .expect("assemble native deferred context");
        assert_eq!(
            accepted
                .user_messages
                .iter()
                .map(|message| (message.source.clone(), message.kind))
                .collect::<Vec<_>>(),
            vec![
                (UserSource::Runtime, ContextKind::RuntimeToolObservation),
                (UserSource::Runtime, ContextKind::RuntimeToolObservation),
            ]
        );
        assert_eq!(texts(&accepted), vec!["A1".to_owned(), "A2".to_owned()]);
        assert_eq!(
            accepted.generation.contributors,
            vec![ContributorGeneration {
                identity: ContextContributorIdentity::Native(
                    NativeContextContributor::RuntimeToolObservation,
                ),
                attestation: None,
            }],
            "the accepted generation explains the deferred-context owner exactly once"
        );
    }

    /// A deferred proposal owned by a **registered** certified extension keeps
    /// that extension's identity, its extension provenance, its own semantic
    /// lane, and its **registered attestation**. The generation is the
    /// authoritative registration, never a value synthesized from the deferred
    /// reference.
    #[tokio::test]
    async fn registered_extension_deferred_context_uses_the_registered_generation() {
        let assembly =
            assembly_with_extension("example.extension", Some("package-7".to_owned()), None);
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[extension_deferred("example.extension", "deferred")],
            )
            .await
            .expect("assemble extension deferred context");
        assert_eq!(accepted.user_messages.len(), 1);
        assert_eq!(
            accepted.user_messages[0].source,
            extension_source("example.extension"),
            "extension provenance survives deferred timing"
        );
        assert_eq!(
            accepted.user_messages[0].kind,
            ContextKind::ExtensionEnvironment,
            "the semantic family follows the owner, not the timing"
        );
        assert_eq!(
            accepted.generation.contributors,
            vec![ContributorGeneration {
                identity: extension_identity("example.extension"),
                attestation: Some("package-7".to_owned()),
            }],
            "the authoritative registered attestation is used, not a synthesized one"
        );
    }

    /// A **post-tool-only** certified extension — one that never contributes
    /// request-time context — still produces deferred context through its
    /// authoritative registration. Registration is what makes it an extension;
    /// emitting request-time proposals is not.
    #[tokio::test]
    async fn a_post_tool_only_registered_extension_still_uses_its_registration() {
        let assembly = assembly_with_extension("observer.only", Some("package-3".to_owned()), None);
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[extension_deferred("observer.only", "post-tool only")],
            )
            .await
            .expect("assemble post-tool-only extension context");
        assert_eq!(texts(&accepted), vec!["post-tool only".to_owned()]);
        assert_eq!(
            accepted.user_messages[0].source,
            extension_source("observer.only")
        );
        assert_eq!(
            accepted.generation.contributors,
            vec![ContributorGeneration {
                identity: extension_identity("observer.only"),
                attestation: Some("package-3".to_owned()),
            }],
            "a producer that only defers is still explained by its registration"
        );
    }

    /// Context Assembly registration is the **only** semantic admission
    /// authority. A deferred proposal naming an extension this assembly never
    /// registered is rejected outright: no lane, no extension provenance, no
    /// synthesized generation, and no partially admitted context.
    #[tokio::test]
    async fn an_unregistered_extension_producer_is_rejected() {
        let assembly = assembly_with_extension("known.extension", None, Some("request-time"));
        let error = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[extension_deferred("unknown.extension", "unauthorized")],
            )
            .await
            .expect_err("an unregistered producer cannot mint extension provenance");
        assert_eq!(
            error,
            ContextAssemblyError::UnregisteredContributor("unknown.extension".to_owned())
        );

        // The rejection is transactional: a well-formed sibling proposal from
        // a registered producer does not rescue the batch either.
        let error = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[
                    native_deferred("native fact"),
                    extension_deferred("unknown.extension", "unauthorized"),
                ],
            )
            .await
            .expect_err("the whole deferred batch is rejected");
        assert_eq!(
            error,
            ContextAssemblyError::UnregisteredContributor("unknown.extension".to_owned())
        );
    }

    /// A native-reserved logical key can never become a registered extension,
    /// so a deferred producer naming one is rejected as unregistered — the
    /// native owner is reachable only through the native producer.
    #[tokio::test]
    async fn a_producer_cannot_claim_a_native_reserved_identity() {
        let mut assembly = ContextAssembly::new();
        let error = assembly
            .register_extension(
                NativeContextContributor::RuntimeToolObservation.logical_key(),
                None,
                Arc::new(|_: &ContributorInputSnapshot| Ok(Vec::new())),
            )
            .expect_err("the native key is reserved");
        assert!(matches!(
            error,
            ContextAssemblyError::ReservedNativeIdentity(_)
        ));

        let error = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[extension_deferred(
                    NativeContextContributor::RuntimeToolObservation.logical_key(),
                    "impersonation",
                )],
            )
            .await
            .expect_err("a reserved key is never a registered extension");
        assert_eq!(
            error,
            ContextAssemblyError::UnregisteredContributor(
                NativeContextContributor::RuntimeToolObservation
                    .logical_key()
                    .to_owned()
            )
        );
    }

    /// The same extension's deferred and request-time proposals are the same
    /// semantic fact family with the same provenance; only their order inside
    /// the owner's lane records that one describes the preceding tool batch.
    #[tokio::test]
    async fn deferred_and_request_time_context_of_one_owner_agree_on_semantics() {
        let assembly = assembly_with_extension(
            "example.extension",
            Some("package-1".to_owned()),
            Some("request-time"),
        );
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[extension_deferred("example.extension", "deferred")],
            )
            .await
            .expect("assemble mixed-phase extension context");
        assert_eq!(
            texts(&accepted),
            vec!["deferred".to_owned(), "request-time".to_owned()],
            "the deferred fact describes the preceding tool batch and precedes it"
        );
        assert_eq!(
            accepted.user_messages[0].source, accepted.user_messages[1].source,
            "one owner has one provenance regardless of timing"
        );
        assert_eq!(
            accepted.user_messages[0].kind,
            accepted.user_messages[1].kind
        );
        assert_eq!(
            accepted.generation.contributors,
            vec![ContributorGeneration {
                identity: extension_identity("example.extension"),
                attestation: Some("package-1".to_owned()),
            }],
            "one owner appears once, with its registered attestation"
        );
    }

    /// Deferred producers with different identities are ordered by lane and
    /// logical identity, exactly like request-time contributors. The staging
    /// order of the buffer decides nothing about relative placement between
    /// owners, so no registration order is observable.
    #[tokio::test]
    async fn deferred_producers_keep_deterministic_identity_order() {
        let mut assembly = ContextAssembly::new();
        for key in ["zeta.extension", "alpha.extension"] {
            assembly
                .register_extension(
                    key,
                    None,
                    Arc::new(|_: &ContributorInputSnapshot| Ok(Vec::new())),
                )
                .expect("register extension");
        }
        let staged = [
            extension_deferred("zeta.extension", "z1"),
            native_deferred("n1"),
            extension_deferred("alpha.extension", "a1"),
            extension_deferred("zeta.extension", "z2"),
            native_deferred("n2"),
            extension_deferred("alpha.extension", "a2"),
        ];
        let mut reversed = staged.clone();
        reversed.reverse();

        let forward = assembly
            .assemble(&input(), &NativeContextInput::default(), &staged)
            .await
            .expect("forward staging");
        let owners = |accepted: &AcceptedContext| {
            accepted
                .user_messages
                .iter()
                .map(|message| (message.source.clone(), message.kind))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            owners(&forward),
            vec![
                (UserSource::Runtime, ContextKind::RuntimeToolObservation),
                (UserSource::Runtime, ContextKind::RuntimeToolObservation),
                (
                    extension_source("alpha.extension"),
                    ContextKind::ExtensionEnvironment
                ),
                (
                    extension_source("alpha.extension"),
                    ContextKind::ExtensionEnvironment
                ),
                (
                    extension_source("zeta.extension"),
                    ContextKind::ExtensionEnvironment
                ),
                (
                    extension_source("zeta.extension"),
                    ContextKind::ExtensionEnvironment
                ),
            ],
            "owners are grouped by lane then logical identity"
        );
        assert_eq!(
            texts(&forward),
            vec![
                "n1".to_owned(),
                "n2".to_owned(),
                "a1".to_owned(),
                "a2".to_owned(),
                "z1".to_owned(),
                "z2".to_owned()
            ],
            "each owner keeps its own FIFO order"
        );

        let backward = assembly
            .assemble(&input(), &NativeContextInput::default(), &reversed)
            .await
            .expect("reversed staging");
        assert_eq!(
            owners(&backward),
            owners(&forward),
            "the owner order never depends on staging/registration order"
        );
        assert_eq!(
            texts(&backward),
            vec![
                "n2".to_owned(),
                "n1".to_owned(),
                "a2".to_owned(),
                "a1".to_owned(),
                "z2".to_owned(),
                "z1".to_owned()
            ],
            "only each owner's own FIFO order follows the buffer"
        );
    }

    /// Deferred proposals sit in their owner's lane inside the one total lane
    /// order, together with every request-time proposal of that lane.
    #[tokio::test]
    async fn deferred_context_uses_the_one_total_lane_order() {
        let assembly =
            assembly_with_extension("example.extension", None, Some("extension context"));
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput {
                    workspace_instructions: Some("workspace".to_owned()),
                    agent_status: Some("status".to_owned()),
                    ..NativeContextInput::default()
                },
                &[
                    native_deferred("A1"),
                    native_deferred("A2"),
                    extension_deferred("example.extension", "deferred extension"),
                ],
            )
            .await
            .expect("assemble deferred context");
        assert_eq!(
            accepted
                .user_messages
                .iter()
                .map(|message| (message.kind, message.source.clone()))
                .collect::<Vec<_>>(),
            vec![
                (ContextKind::RuntimeToolObservation, UserSource::Runtime),
                (ContextKind::RuntimeToolObservation, UserSource::Runtime),
                (
                    ContextKind::ExtensionEnvironment,
                    extension_source("example.extension")
                ),
                (
                    ContextKind::ExtensionEnvironment,
                    extension_source("example.extension")
                ),
                (ContextKind::AgentStatus, UserSource::Runtime),
            ],
            "the native observation lane is early; an extension's deferred fact stays in its own lane"
        );
        assert_eq!(
            texts(&accepted),
            vec![
                "A1".to_owned(),
                "A2".to_owned(),
                "deferred extension".to_owned(),
                "extension context".to_owned(),
                "status".to_owned(),
            ]
        );
        assert_eq!(
            accepted
                .system_sections
                .iter()
                .map(|section| section.content.as_str())
                .collect::<Vec<_>>(),
            vec!["workspace"]
        );
    }

    /// The deferred seam carries User context only, so an admitted deferred
    /// batch never contributes an effective-system-prompt section. Extension
    /// sections come only from the immutable resource registration.
    #[tokio::test]
    async fn deferred_context_never_reaches_the_effective_system_prompt() {
        let mut assembly = ContextAssembly::new();
        let identity = assembly
            .register_extension(
                "example.extension",
                None,
                Arc::new(|_: &ContributorInputSnapshot| Ok(Vec::<ContextProposal>::new())),
            )
            .expect("register extension");
        assembly
            .register_extension_system_section(&identity, "resource section")
            .expect("resource section");
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput {
                    core_runtime_identity: Some("core identity".to_owned()),
                    ..NativeContextInput::default()
                },
                &[
                    native_deferred("deferred user fact"),
                    extension_deferred("example.extension", "deferred extension fact"),
                ],
            )
            .await
            .expect("assemble deferred context");
        assert_eq!(
            accepted
                .system_sections
                .iter()
                .map(|section| section.content.as_str())
                .collect::<Vec<_>>(),
            vec!["core identity", "resource section"],
            "the extension section came from its immutable resource generation"
        );
        assert_eq!(
            render_effective_system_prompt(&accepted.system_sections),
            "core identity\n\nresource section",
            "no deferred text reaches the Effective System Prompt"
        );
        assert_eq!(
            texts(&accepted),
            vec![
                "deferred user fact".to_owned(),
                "deferred extension fact".to_owned()
            ],
            "the deferred facts are User context"
        );
    }

    /// A deferred batch is bounded exactly like a contributor batch.
    #[tokio::test]
    async fn deferred_context_is_bounded() {
        let assembly = ContextAssembly::new();
        let error = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &(0..=MAX_DEFERRED_CONTEXT_PROPOSALS)
                    .map(|index| native_deferred(&format!("proposal {index}")))
                    .collect::<Vec<_>>(),
            )
            .await
            .expect_err("an unbounded deferred batch is rejected");
        assert_eq!(error, ContextAssemblyError::ProposalLimitExceeded);
    }

    /// Deferred content is validated by the same bounded content contract.
    #[tokio::test]
    async fn deferred_content_is_validated() {
        let assembly = ContextAssembly::new();
        let error = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[native_deferred("")],
            )
            .await
            .expect_err("an empty deferred proposal is rejected");
        assert!(matches!(error, ContextAssemblyError::InvalidProposal(_)));
        assert!(validate_user_message_proposal(&user_message("")).is_err());
        assert!(validate_user_message_proposal(&user_message("bounded")).is_ok());
    }

    #[test]
    fn manifest_is_derived_from_contract_constants() {
        let manifest = ContextAssembly::compatibility_manifest();
        assert_eq!(
            CONTEXT_COMPATIBILITY_ABI_VERSION, 3,
            "resource-frozen extension System sections are a v3 ABI"
        );
        assert_eq!(manifest.user_context_lanes, UserContextLane::ALL);
        assert_eq!(manifest.system_section_lanes, SystemSectionLane::ALL);
        assert_eq!(manifest.abi_version, CONTEXT_COMPATIBILITY_ABI_VERSION);
        assert_eq!(
            manifest.reserved_native_slots,
            NativeContextContributor::ALL
                .into_iter()
                .map(|owner| owner.manifest_name().to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(manifest.allowed_proposal_kinds, ContextProposalKind::ALL);
        assert_eq!(
            manifest.provenance_namespaces,
            crate::message::types::UserSource::PROVENANCE_NAMESPACES
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
    }
}
