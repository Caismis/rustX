//! The structured Agent Status projection (M4).
//!
//! Agent Status is the mandatory, provider-neutral, ephemeral runtime
//! projection that gives every rustX agent compact awareness of current
//! runtime facts on a fresh inbound turn. It is:
//!
//! - **mandatory**: every normal `AgentExecution` composes it whenever a
//!   pending fresh inbound turn exists; there is no disable flag and no
//!   legacy no-status execution mode;
//! - **projection-only**: it is never canonical history, never checkpoint
//!   history, never returned in `AgentExecutionResult.messages`, and never
//!   emitted as a committed-message event;
//! - **provider-neutral**: composition produces structured sections, a
//!   canonical deterministic renderer turns them into one text attachment,
//!   and provider adapters own the final wire placement.
//!
//! The composition flow is frozen:
//!
//! ```text
//! runtime facts
//!     → structured AgentStatus sections
//!     → canonical deterministic renderer
//!     → rendered AgentStatusAttachment
//!     → provider wire compiler
//! ```
//!
//! A provider adapter never receives raw runtime state and never invents the
//! status text itself.
//!
//! The cross-layer [`AgentStatusAttachment`](crate::model::types::AgentStatusAttachment)
//! is a Layer 0 contract owned by `crate::model::types`: this module
//! *produces* the attachment through composition and rendering, but model
//! contracts never depend on context implementation modules.
//!
//! Section ordering is deterministic and frozen:
//!
//! ```text
//! 1. mandatory temporal section
//! 2. future mandatory built-in sections
//! 3. extension providers in explicit registration order
//! ```
//!
//! A provider's section identity is runtime-owned registration metadata: it
//! is queried exactly once at registration, validated, and then frozen for
//! the lifetime of the composer. Post-registration changes to what a
//! provider's `section_id()` *would* return can never shadow a reserved id
//! or create duplicate identities.
//!
//! The `background_execution` section id is reserved for the known M5
//! background-runtime integration and has no fake M4 implementation.

use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::context::error::{ContextError, ContextErrorKind};

/// The stable identity of one Agent Status section.
///
/// Built-in mandatory section ids are reserved: an extension provider can
/// never register, replace, or shadow them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentStatusSectionId(String);

impl AgentStatusSectionId {
    /// The reserved id of the mandatory temporal section.
    pub const TEMPORAL: &'static str = "temporal";
    /// The reserved id of the future M5 background-execution section.
    pub const BACKGROUND_EXECUTION: &'static str = "background_execution";

    /// Creates a section id.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The raw section id string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is a reserved built-in section id.
    #[must_use]
    pub fn is_reserved(&self) -> bool {
        self.0 == Self::TEMPORAL || self.0 == Self::BACKGROUND_EXECUTION
    }
}

impl core::fmt::Display for AgentStatusSectionId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The structured data of one Agent Status section.
///
/// Sections are structured before rendering: a provider returns structured
/// runtime facts, never provider wire text, and the canonical renderer is the
/// only place status text is produced. The renderer owns labels, separators,
/// and layout.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatusSectionData {
    /// The mandatory temporal facts.
    Temporal {
        /// The runtime clock value sampled at composition time.
        current_time: DateTime<Utc>,
        /// The conversation timezone, when known.
        timezone: Option<Tz>,
        /// The persisted timestamp of the final message of the pending
        /// `FreshInboundTurn`.
        inbound_message_time: DateTime<Utc>,
    },
    /// An extension section's ordered structured runtime facts.
    ///
    /// A provider contributes facts (`label` + `value`), never pre-rendered
    /// footer lines; the canonical renderer formats each fact deterministically.
    Facts {
        /// The ordered facts of the section.
        facts: Vec<AgentStatusFact>,
    },
}

/// One structured runtime fact of an extension status section.
///
/// This is the provider-neutral structured seam future runtime subsystems
/// (such as the M5 background runtime) populate with authoritative state. A
/// provider returns data; text formatting belongs to the canonical renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatusFact {
    /// The fact label, rendered verbatim by the canonical renderer.
    pub label: String,
    /// The fact value, rendered verbatim by the canonical renderer.
    pub value: String,
}

/// One composed Agent Status section.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatusSection {
    /// The stable section identity.
    pub id: AgentStatusSectionId,
    /// The structured section data.
    pub data: AgentStatusSectionData,
}

/// One composed Agent Status.
///
/// Composition is deterministic: the same clock instant, render context, and
/// provider set produce the same sections in the same order.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatus {
    /// The ordered sections.
    pub sections: Vec<AgentStatusSection>,
}

/// The read-only common runtime facts every status provider may observe.
///
/// Providers must never mutate runtime state while rendering; this context is
/// the only execution information the composition flow hands them besides
/// their own authoritative state.
#[derive(Debug, Clone, Copy)]
pub struct AgentStatusRenderContext {
    /// The persisted timestamp of the final message of the pending
    /// `FreshInboundTurn`.
    pub inbound_message_time: DateTime<Utc>,
    /// The per-execution/conversation IANA timezone, when known.
    pub timezone: Option<Tz>,
}

/// The narrow extension seam for Agent Status sections.
///
/// A provider has a stable section identity, receives the read-only render
/// context, may hold its own read-only authoritative subsystem state, and
/// returns structured section data ([`AgentStatusSectionData::Facts`]) or an
/// intentional absence. It must never mutate runtime state while rendering,
/// and it never returns pre-rendered text: the canonical renderer formats
/// every fact. This is deliberately not a plugin ecosystem: the seam exists
/// so a future M5 background runtime can project its registry through a
/// read-only provider.
pub trait AgentStatusSectionProvider: Send + Sync {
    /// The stable section identity of this provider.
    fn section_id(&self) -> AgentStatusSectionId;

    /// Returns the structured section data, `None` when there is nothing
    /// useful to render, or an error when composition must fail.
    ///
    /// # Errors
    ///
    /// Returns a context-preparation error; an actual provider failure is
    /// propagated and must never silently become an absent section.
    fn section(
        &self,
        context: &AgentStatusRenderContext,
    ) -> Result<Option<AgentStatusSectionData>, ContextError>;
}

/// The composition registration failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatusCompositionError {
    /// An extension tried to register a reserved built-in section id.
    ReservedSectionId(AgentStatusSectionId),
    /// An extension tried to register an id already registered.
    DuplicateSectionId(AgentStatusSectionId),
}

impl core::fmt::Display for AgentStatusCompositionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ReservedSectionId(id) => write!(
                f,
                "Agent Status section id {id:?} is reserved and cannot be provided by an extension"
            ),
            Self::DuplicateSectionId(id) => write!(
                f,
                "duplicate Agent Status section id {id:?}; extension ids must be unique"
            ),
        }
    }
}

/// The clock boundary of Agent Status composition.
///
/// The renderer and all assertions go through this narrow abstraction; no
/// production code calls `Utc::now()` directly. Tests use a fixed/scripted
/// clock so snapshots are deterministic.
pub trait AgentStatusClock: Send + Sync {
    /// The current UTC instant.
    fn now(&self) -> DateTime<Utc>;
}

/// The production clock: system UTC time.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl AgentStatusClock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// The mandatory temporal section facts, composed before any extension
/// section.
fn temporal_section(
    current_time: DateTime<Utc>,
    context: &AgentStatusRenderContext,
) -> AgentStatusSection {
    AgentStatusSection {
        id: AgentStatusSectionId::new(AgentStatusSectionId::TEMPORAL),
        data: AgentStatusSectionData::Temporal {
            current_time,
            timezone: context.timezone,
            inbound_message_time: context.inbound_message_time,
        },
    }
}

/// One registered extension section provider with its frozen identity.
///
/// The section identity is runtime-owned registration metadata: `section_id()`
/// is called exactly once at successful registration, validated, and stored
/// with the provider. Composition, provider ordering, diagnostics, and
/// provider listing all use the stored id; `section_id()` is never called
/// again for an already registered provider, so a stateful provider cannot
/// mutate its identity after registration.
struct RegisteredStatusProvider {
    id: AgentStatusSectionId,
    provider: Arc<dyn AgentStatusSectionProvider>,
}

/// The deterministic Agent Status composer.
///
/// The composer samples its clock exactly once per `compose` invocation:
/// one request preparation composes one status snapshot, and the composed
/// `AgentStatus` is then rendered once and reused throughout that
/// preparation's compaction planning and application. A new provider
/// invocation (a `ContextWindowExceeded` compact-and-retry) begins a new
/// request preparation and composes a fresh snapshot.
pub struct AgentStatusComposer {
    clock: Arc<dyn AgentStatusClock>,
    providers: Vec<RegisteredStatusProvider>,
}

impl core::fmt::Debug for AgentStatusComposer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AgentStatusComposer")
            .field("clock", &"<opaque agent status clock>")
            .field("provider_ids", &self.provider_ids())
            .finish()
    }
}

impl Default for AgentStatusComposer {
    fn default() -> Self {
        Self::new(Arc::new(SystemClock))
    }
}

impl AgentStatusComposer {
    /// Creates a composer over one clock. The mandatory temporal section is
    /// always composed; extension providers are registered explicitly.
    #[must_use]
    pub fn new(clock: Arc<dyn AgentStatusClock>) -> Self {
        Self {
            clock,
            providers: Vec::new(),
        }
    }

    /// Registers an extension section provider.
    ///
    /// The provider's section identity is queried exactly once here and then
    /// frozen as runtime-owned registration metadata: reserved ids and
    /// duplicate ids are rejected explicitly, and composition, ordering,
    /// diagnostics, and provider listing afterwards use the stored id. There
    /// is no "last one wins" behavior.
    ///
    /// # Errors
    ///
    /// Returns [`AgentStatusCompositionError::ReservedSectionId`] for a
    /// reserved id and [`AgentStatusCompositionError::DuplicateSectionId`]
    /// for a duplicate extension id.
    pub fn register(
        &mut self,
        provider: Arc<dyn AgentStatusSectionProvider>,
    ) -> Result<(), AgentStatusCompositionError> {
        let id = provider.section_id();
        if id.is_reserved() {
            return Err(AgentStatusCompositionError::ReservedSectionId(id));
        }
        if self.providers.iter().any(|registered| registered.id == id) {
            return Err(AgentStatusCompositionError::DuplicateSectionId(id));
        }
        self.providers
            .push(RegisteredStatusProvider { id, provider });
        Ok(())
    }

    /// The registered extension provider ids, in registration order.
    #[must_use]
    pub fn provider_ids(&self) -> Vec<AgentStatusSectionId> {
        self.providers
            .iter()
            .map(|registered| registered.id.clone())
            .collect()
    }

    /// Composes one status snapshot: the mandatory temporal section first,
    /// then every extension section in registration order.
    ///
    /// The clock is sampled exactly once per invocation. An extension
    /// provider returning `None` contributes no section; a provider failure
    /// propagates as a context-preparation error and is never silently
    /// dropped.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::StatusFailed`] when an extension provider
    /// fails to produce its section.
    pub fn compose(&self, context: &AgentStatusRenderContext) -> Result<AgentStatus, ContextError> {
        let mut sections = vec![temporal_section(self.clock.now(), context)];
        for registered in &self.providers {
            match registered.provider.section(context) {
                Ok(Some(data)) => sections.push(AgentStatusSection {
                    id: registered.id.clone(),
                    data,
                }),
                Ok(None) => {}
                Err(error) => {
                    return Err(ContextError::new(
                        ContextErrorKind::StatusFailed,
                        format!(
                            "Agent Status section provider {:?} failed: {}",
                            registered.id.as_str(),
                            error.message
                        ),
                    ));
                }
            }
        }
        Ok(AgentStatus { sections })
    }
}

/// Renders one temporal instant deterministically.
///
/// When the timezone is known, the UTC instant is rendered in that timezone
/// with its RFC3339 numeric offset; otherwise it is rendered in UTC. The
/// process/system local timezone is never consulted.
fn render_instant(instant: DateTime<Utc>, timezone: Option<Tz>) -> String {
    match timezone {
        Some(tz) => instant
            .with_timezone(&tz)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
        None => instant.to_rfc3339_opts(SecondsFormat::Secs, true),
    }
}

/// The canonical deterministic Agent Status renderer.
///
/// The output is a `<system-reminder>` footer. The temporal section renders
/// `Current time`, the IANA `Timezone` line (only when known), and
/// `Inbound message time`. Extension sections follow after a blank line, each
/// structured fact rendered as `label: value` in deterministic order. This is
/// the only place status text is produced; providers never construct provider
/// wire objects and never pre-render footer lines.
///
/// ```text
/// <system-reminder>
/// Current time: 2026-08-08T17:31:00+09:00
/// Timezone: Asia/Tokyo
/// Inbound message time: 2026-08-08T17:30:58+09:00
/// </system-reminder>
/// ```
#[must_use]
pub fn render_agent_status(status: &AgentStatus) -> String {
    let mut lines: Vec<String> = Vec::new();
    for section in &status.sections {
        match &section.data {
            AgentStatusSectionData::Temporal {
                current_time,
                timezone,
                inbound_message_time,
            } => {
                lines.push(format!(
                    "Current time: {}",
                    render_instant(*current_time, *timezone)
                ));
                if let Some(tz) = timezone {
                    lines.push(format!("Timezone: {}", tz.name()));
                }
                lines.push(format!(
                    "Inbound message time: {}",
                    render_instant(*inbound_message_time, *timezone)
                ));
            }
            AgentStatusSectionData::Facts { facts } => {
                if !facts.is_empty() {
                    if !lines.is_empty() {
                        lines.push(String::new());
                    }
                    for fact in facts {
                        lines.push(format!("{}: {}", fact.label, fact.value));
                    }
                }
            }
        }
    }
    let mut rendered = String::from("<system-reminder>\n");
    rendered.push_str(&lines.join("\n"));
    rendered.push('\n');
    rendered.push_str("</system-reminder>");
    rendered
}
