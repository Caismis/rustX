//! The structured Agent Status projection (M4).
//!
//! Agent Status is the mandatory, provider-neutral, ephemeral runtime
//! projection that gives every rustX agent compact awareness of current
//! runtime facts on a fresh inbound turn. It is:
//!
//! - **mandatory**: every normal `AgentExecution` composes it whenever a
//!   pending fresh inbound turn exists; there is no disable flag and no
//!   legacy no-status execution mode;
//! - **historical**: once accepted it is a canonical Runtime-sourced User
//!   context fact in the Message Ledger and Surface;
//! - **provider-neutral**: composition produces structured sections, a
//!   canonical deterministic renderer turns them into one bounded text value,
//!   and Context Assembly admits that value through the normal context path.
//!
//! The composition flow is frozen:
//!
//! ```text
//! runtime facts
//!     → structured AgentStatus sections
//!     → canonical deterministic renderer
//!     → rendered Runtime context value
//!     → Context Assembly → canonical User message
//! ```
//!
//! The conversation background registry is authoritative; the executing
//! attempt samples a read-only active snapshot into the render context, and
//! the composer builds the runtime-owned background section. `ContextRuntime`
//! and the composer never own or mutate the background registry.
//!
//! A provider adapter never receives raw runtime state and never invents the
//! status text itself.
//!
//! Section ordering is deterministic and frozen:
//!
//! ```text
//! 1. mandatory temporal section
//! 2. background_execution when active entries exist
//! 3. extension providers in canonical section-identity order
//! ```
//!
//! A provider's section identity is runtime-owned registration metadata: it
//! is queried exactly once at registration, validated, and then frozen for
//! the lifetime of the composer. Post-registration changes to what a
//! provider's `section_id()` *would* return can never shadow a reserved id
//! or create duplicate identities.
//!
//! Built-in section **semantics** are runtime-owned at the type boundary:
//! an extension provider returns extension data only (structured
//! [`AgentStatusFact`] values), never the internal composed section
//! representation, so an extension is structurally incapable of constructing
//! the runtime-owned [`AgentStatusSectionData::Temporal`] variant or any
//! future built-in variant. The composer and built-in composition code are
//! the only places built-in section variants are constructed.
//!
//! The `background_execution` section is a runtime-owned built-in, not an
//! ordinary extension: the composer constructs it from the read-only active
//! background snapshot passed through the render context, and an extension
//! provider can never register under that reserved id or construct the
//! built-in section variant.

use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::context::error::{ContextError, ContextErrorKind};
use crate::tools::background::BackgroundExecutionSnapshot;

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
    /// The reserved id of the runtime-owned background-execution built-in
    /// section.
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

/// The structured data of one composed Agent Status section.
///
/// This is the internal composed representation, constructed only by the
/// composer/built-in composition code: built-in section variants
/// ([`AgentStatusSectionData::Temporal`] and any future built-in variant)
/// are runtime-owned and never expressible through the extension provider
/// contract, which returns structured extension facts only. Sections are
/// structured before rendering: the canonical renderer is the only place
/// status text is produced, and it owns labels, separators, and layout.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatusSectionData {
    /// The mandatory temporal facts, constructed by the composer from its
    /// clock and the render context.
    Temporal {
        /// The runtime clock value sampled at composition time.
        current_time: DateTime<Utc>,
        /// The conversation timezone, when known.
        timezone: Option<Tz>,
        /// The persisted timestamp of the final message of the pending
        /// `FreshInboundTurn`.
        inbound_message_time: DateTime<Utc>,
    },
    /// The runtime-owned background-execution section, constructed by the
    /// composer from the read-only active background snapshot. Extensions
    /// are structurally unable to construct this variant.
    BackgroundExecution {
        /// The active background executions in execution-allocation order.
        executions: Vec<BackgroundExecutionSnapshot>,
    },
    /// An extension section's ordered structured runtime facts.
    ///
    /// The composer converts a provider's extension facts into this variant;
    /// a provider can never construct it directly, only its fact payload.
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
#[derive(Debug, Clone)]
pub struct AgentStatusRenderContext {
    /// The persisted timestamp of the final message of the pending
    /// `FreshInboundTurn`.
    pub inbound_message_time: DateTime<Utc>,
    /// The per-execution/conversation IANA timezone, when known.
    pub timezone: Option<Tz>,
    /// The read-only active background-execution snapshot sampled from the
    /// authoritative conversation background registry by the executing
    /// attempt. Empty when no background executions are active; never
    /// mutated by the composer.
    pub background: Vec<BackgroundExecutionSnapshot>,
}

/// The narrow extension seam for Agent Status sections.
///
/// A provider has a stable section identity, receives the read-only render
/// context, may hold its own read-only authoritative subsystem state, and
/// returns **extension data only**: an ordered list of structured
/// [`AgentStatusFact`] values or an intentional absence. The provider
/// contract never exposes the internal composed section representation:
/// built-in section variants ([`AgentStatusSectionData::Temporal`] and any
/// future built-in variant) are runtime-owned and can only be constructed by
/// the composer, so an extension is structurally incapable of impersonating
/// built-in section semantics. A provider must never mutate runtime state
/// while rendering, and it never returns pre-rendered text: the canonical
/// renderer formats every fact. This is deliberately not a plugin ecosystem:
/// the seam exists so a future M5 background runtime can project its registry
/// through a read-only provider.
pub trait AgentStatusSectionProvider: Send + Sync {
    /// The stable section identity of this provider.
    fn section_id(&self) -> AgentStatusSectionId;

    /// Returns the structured extension facts, `None` when there is nothing
    /// useful to render, or an error when composition must fail.
    ///
    /// The returned facts are extension data only: the composer converts
    /// them into the internal composed section representation, and built-in
    /// section variants are never expressible here.
    ///
    /// # Errors
    ///
    /// Returns a context-preparation error; an actual provider failure is
    /// propagated and must never silently become an absent section.
    fn section(
        &self,
        context: &AgentStatusRenderContext,
    ) -> Result<Option<Vec<AgentStatusFact>>, ContextError>;
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
#[derive(Clone)]
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

impl Clone for AgentStatusComposer {
    fn clone(&self) -> Self {
        Self {
            clock: self.clock.clone(),
            providers: self.providers.clone(),
        }
    }
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

    /// The registered extension provider ids, in canonical identity order.
    #[must_use]
    pub fn provider_ids(&self) -> Vec<AgentStatusSectionId> {
        let mut ids = self
            .providers
            .iter()
            .map(|registered| registered.id.clone())
            .collect::<Vec<_>>();
        ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        ids
    }

    /// Composes one status snapshot: the mandatory temporal section first,
    /// then every extension section in stable logical identity order.
    ///
    /// The clock is sampled exactly once per invocation. The composer owns
    /// the conversion from extension output into the internal composed
    /// section representation: an extension's structured facts become a
    /// `Facts` section, and built-in section variants are constructed only
    /// here (or by built-in composition code), never by providers. An
    /// extension provider returning `None` contributes no section; a provider
    /// failure propagates as a context-preparation error and is never
    /// silently dropped.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::StatusFailed`] when an extension provider
    /// fails to produce its section.
    pub fn compose(&self, context: &AgentStatusRenderContext) -> Result<AgentStatus, ContextError> {
        let mut sections = vec![temporal_section(self.clock.now(), context)];
        if !context.background.is_empty() {
            sections.push(AgentStatusSection {
                id: AgentStatusSectionId::new(AgentStatusSectionId::BACKGROUND_EXECUTION),
                data: AgentStatusSectionData::BackgroundExecution {
                    executions: context.background.clone(),
                },
            });
        }
        let mut providers = self.providers.iter().collect::<Vec<_>>();
        providers.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        for registered in providers {
            match registered.provider.section(context) {
                Ok(Some(facts)) => sections.push(AgentStatusSection {
                    id: registered.id.clone(),
                    data: AgentStatusSectionData::Facts { facts },
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
            AgentStatusSectionData::BackgroundExecution { executions } => {
                if !executions.is_empty() {
                    if !lines.is_empty() {
                        lines.push(String::new());
                    }
                    lines.push("Background executions:".to_owned());
                    for execution in executions {
                        let mut line = format!(
                            "- {} | {} | {}",
                            execution.execution_id.as_str(),
                            execution.tool_name,
                            execution.state.name()
                        );
                        if let Some(progress) = &execution.progress
                            && let Some(message) = &progress.message
                        {
                            line.push_str(" | ");
                            line.push_str(message);
                        }
                        lines.push(line);
                    }
                }
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
