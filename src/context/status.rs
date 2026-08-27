//! The closed, bounded Agent Status engine (Issue #129).
//!
//! Agent Status is optional, provider-independent runtime context for an
//! already-established primary model turn. In this issue the only production
//! delivery opportunity is [`FreshInboundStatusOpportunity`]. The engine does
//! not schedule work, create a turn, or prolong an attempt:
//!
//! ```text
//! FreshInbound
//!     -> capture authoritative state once
//!     -> evaluate frozen snapshots once
//!     -> validate the code-owned payload mapping
//!     -> apply module-local semantic bounds
//!     -> whole-section UTF-8-byte admission
//!     -> optional AgentStatus User context message
//! ```
//!
//! The known modules are deliberately represented by a closed Rust enum. This
//! is not a provider registry or an extension SDK: adding a module requires an
//! intentional source change to the enum and its semantic source order.
//!
//! Module failures are optional-context failures. A failed module is
//! quarantined in the attempt-local engine and the surviving modules continue;
//! the failure never becomes a Context Assembly or model-turn failure.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::runtime::identity::MessageId;
use crate::tools::background::{BackgroundExecutionSnapshot, ConversationBackgroundRegistry};
use crate::tools::types::ToolProgress;

/// The maximum number of active executions the Background module presents.
pub const MAX_BACKGROUND_STATUS_EXECUTIONS: usize = 8;

/// The maximum byte length of one dynamic Background status field.
///
/// This limit is applied to source fields before rendering. It is not a
/// substitute for the final Agent Status cap.
pub const MAX_BACKGROUND_STATUS_TEXT_BYTES: usize = 256;

/// The final defensive Agent Status rendering bound, measured in UTF-8 bytes.
pub const GLOBAL_AGENT_STATUS_BYTE_CAP: usize = 4096;

const DEFAULT_ENABLED: bool = true;

/// Launch-scoped Agent Status configuration.
///
/// Omitting `agentStatus` or either nested module enables both built-in
/// modules. Unknown fields remain rejected by the surrounding strict serde
/// boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct AgentStatusConfig {
    /// The Time module configuration.
    #[serde(default)]
    pub time: TimeStatusConfig,
    /// The Background module configuration.
    #[serde(default)]
    pub background: BackgroundStatusConfig,
}

/// Launch-scoped configuration for the Time status module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct TimeStatusConfig {
    /// Whether Time contributes to `FreshInbound` status.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The optional IANA timezone used only by Time presentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<Tz>,
}

impl Default for TimeStatusConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
            timezone: None,
        }
    }
}

/// Launch-scoped configuration for the Background status module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct BackgroundStatusConfig {
    /// Whether Background contributes to `FreshInbound` status.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

const fn default_enabled() -> bool {
    DEFAULT_ENABLED
}

impl Default for BackgroundStatusConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
        }
    }
}

/// The stable identity of one code-owned Agent Status module/section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatusModuleId {
    /// The Time module.
    Time,
    /// The Background module.
    Background,
}

impl AgentStatusModuleId {
    /// The stable section identity carried by the internal status value.
    #[must_use]
    pub const fn section_id(self) -> &'static str {
        match self {
            Self::Time => AgentStatusSectionId::TEMPORAL,
            Self::Background => AgentStatusSectionId::BACKGROUND_EXECUTION,
        }
    }

    /// The stable diagnostic name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Background => "background",
        }
    }
}

/// The one production Agent Status delivery opportunity implemented by this
/// issue. The inbound identity is retained separately from the status
/// message identity, which does not exist until Context Assembly stages it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshInboundStatusOpportunity {
    /// The final canonical inbound message that made this opportunity
    /// eligible.
    pub target_message_id: MessageId,
    /// The persisted timestamp of that inbound message.
    pub inbound_message_time: DateTime<Utc>,
}

/// The opportunities available to one logical primary step.
///
/// There is intentionally no `post_tool_batch` field until that opportunity
/// has a real production producer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentStatusOpportunitySet {
    /// The `FreshInbound` opportunity, when one is present.
    pub fresh_inbound: Option<FreshInboundStatusOpportunity>,
}

/// The structured data of one accepted Agent Status section.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatusSectionData {
    /// The Time module's typed presentation payload.
    Temporal {
        /// The UTC clock instant captured for this generation.
        current_time: DateTime<Utc>,
        /// The configured IANA timezone, when known.
        timezone: Option<Tz>,
        /// The persisted timestamp of the triggering inbound message.
        inbound_message_time: DateTime<Utc>,
    },
    /// The Background module's typed presentation payload.
    BackgroundExecution {
        /// The bounded active execution entries in registry allocation order.
        executions: Vec<BackgroundExecutionSnapshot>,
        /// Active executions omitted by the module-local entry bound.
        omitted_count: usize,
    },
}

/// The stable identity of one Agent Status section.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentStatusSectionId(String);

impl AgentStatusSectionId {
    /// The stable Time section id.
    pub const TEMPORAL: &'static str = "temporal";
    /// The stable Background section id.
    pub const BACKGROUND_EXECUTION: &'static str = "background_execution";

    /// Creates a section id for internal projection construction.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the raw section id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for AgentStatusSectionId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One accepted structured Agent Status section.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatusSection {
    /// The stable section identity.
    pub id: AgentStatusSectionId,
    /// The typed section payload.
    pub data: AgentStatusSectionData,
}

/// One accepted Agent Status generation.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatus {
    /// Sections in rustX semantic source order.
    pub sections: Vec<AgentStatusSection>,
}

/// The clock boundary of the Time module.
pub trait AgentStatusClock: Send + Sync {
    /// Returns the current UTC instant.
    fn now(&self) -> DateTime<Utc>;
}

/// The production UTC clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl AgentStatusClock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// One closed internal Agent Status module.
///
/// The array in [`AgentStatusEngine`] is the semantic source order. No map,
/// registration sequence, or lexical sort participates in composition.
enum AgentStatusModule {
    /// The built-in Time module.
    Time(TimeStatusModule),
    /// The built-in Background module.
    Background(BackgroundStatusModule),
}

impl AgentStatusModule {
    fn id(&self) -> AgentStatusModuleId {
        match self {
            Self::Time(_) => AgentStatusModuleId::Time,
            Self::Background(_) => AgentStatusModuleId::Background,
        }
    }

    fn enabled(&self) -> bool {
        match self {
            Self::Time(module) => module.config.enabled,
            Self::Background(module) => module.config.enabled,
        }
    }

    fn capture(
        &self,
        background: &ConversationBackgroundRegistry,
        seam: Option<&AgentStatusTestSeam>,
    ) -> Result<AgentStatusModuleSnapshot, ModuleFailurePhase> {
        let id = self.id();
        if let Some(seam) = seam {
            seam.record_capture(id);
            if seam.take_capture_failure(id) {
                return Err(ModuleFailurePhase::Capture);
            }
        }
        match self {
            Self::Time(module) => Ok(AgentStatusModuleSnapshot::Time(module.capture())),
            Self::Background(_) => Ok(AgentStatusModuleSnapshot::Background(
                BackgroundStatusModule::capture(background),
            )),
        }
    }

    fn evaluate(
        &self,
        snapshot: &AgentStatusModuleSnapshot,
        opportunity: &FreshInboundStatusOpportunity,
        seam: Option<&AgentStatusTestSeam>,
    ) -> Result<Option<AgentStatusPayload>, ModuleFailurePhase> {
        let id = self.id();
        if let Some(seam) = seam {
            seam.record_evaluate(id);
            if seam.take_evaluate_failure(id) {
                return Err(ModuleFailurePhase::Evaluate);
            }
        }
        let payload = match (self, snapshot) {
            (Self::Time(module), AgentStatusModuleSnapshot::Time(snapshot)) => {
                Some(module.evaluate(snapshot, opportunity))
            }
            (Self::Background(_), AgentStatusModuleSnapshot::Background(snapshot)) => {
                BackgroundStatusModule::evaluate(snapshot)
            }
            _ => return Err(ModuleFailurePhase::Evaluate),
        };
        if seam.is_some_and(|value| value.take_payload_mismatch(id)) {
            return Ok(Some(match id {
                AgentStatusModuleId::Time => AgentStatusPayload::BackgroundExecution {
                    executions: Vec::new(),
                    omitted_count: 0,
                },
                AgentStatusModuleId::Background => AgentStatusPayload::Temporal {
                    current_time: opportunity.inbound_message_time,
                    timezone: None,
                    inbound_message_time: opportunity.inbound_message_time,
                },
            }));
        }
        Ok(payload)
    }
}

/// The code-owned Time module.
struct TimeStatusModule {
    config: TimeStatusConfig,
    clock: Arc<dyn AgentStatusClock>,
}

impl TimeStatusModule {
    fn capture(&self) -> TimeStatusSnapshot {
        TimeStatusSnapshot {
            current_time: self.clock.now(),
        }
    }

    fn evaluate(
        &self,
        snapshot: &TimeStatusSnapshot,
        opportunity: &FreshInboundStatusOpportunity,
    ) -> AgentStatusPayload {
        AgentStatusPayload::Temporal {
            current_time: snapshot.current_time,
            timezone: self.config.timezone,
            inbound_message_time: opportunity.inbound_message_time,
        }
    }
}

/// The code-owned Background module.
struct BackgroundStatusModule {
    config: BackgroundStatusConfig,
}

impl BackgroundStatusModule {
    fn capture(background: &ConversationBackgroundRegistry) -> BackgroundStatusSnapshot {
        BackgroundStatusSnapshot {
            executions: background.active_snapshot(),
        }
    }

    fn evaluate(snapshot: &BackgroundStatusSnapshot) -> Option<AgentStatusPayload> {
        if snapshot.executions.is_empty() {
            return None;
        }
        let retained = snapshot
            .executions
            .iter()
            .take(MAX_BACKGROUND_STATUS_EXECUTIONS)
            .cloned()
            .map(bound_background_snapshot)
            .collect::<Vec<_>>();
        Some(AgentStatusPayload::BackgroundExecution {
            omitted_count: snapshot.executions.len().saturating_sub(retained.len()),
            executions: retained,
        })
    }
}

#[derive(Debug, Clone)]
enum AgentStatusModuleSnapshot {
    Time(TimeStatusSnapshot),
    Background(BackgroundStatusSnapshot),
}

#[derive(Debug, Clone)]
struct TimeStatusSnapshot {
    current_time: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct BackgroundStatusSnapshot {
    executions: Vec<BackgroundExecutionSnapshot>,
}

#[derive(Debug, Clone)]
enum AgentStatusPayload {
    Temporal {
        current_time: DateTime<Utc>,
        timezone: Option<Tz>,
        inbound_message_time: DateTime<Utc>,
    },
    BackgroundExecution {
        executions: Vec<BackgroundExecutionSnapshot>,
        omitted_count: usize,
    },
}

#[derive(Debug, Clone, Copy)]
enum ModuleFailurePhase {
    Capture,
    Evaluate,
    PayloadValidation,
}

impl ModuleFailurePhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Evaluate => "evaluate",
            Self::PayloadValidation => "payload_validation",
        }
    }
}

/// The attempt-owned closed Agent Status engine.
pub struct AgentStatusEngine {
    config: AgentStatusConfig,
    modules: [AgentStatusModule; 2],
    quarantined: HashSet<AgentStatusModuleId>,
    #[cfg(test)]
    test_seam: Option<AgentStatusTestSeam>,
}

impl Clone for AgentStatusEngine {
    fn clone(&self) -> Self {
        let clone = Self::new(self.config.clone(), self.clock());
        #[cfg(test)]
        let clone = {
            // The seam is shared so a runtime-created attempt can be counted
            // by its owning test, while the new engine still starts with
            // empty attempt-local quarantine state.
            let mut clone = clone;
            clone.test_seam = self.test_seam.clone();
            clone
        };
        clone
    }
}

impl core::fmt::Debug for AgentStatusEngine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AgentStatusEngine")
            .field("config", &self.config)
            .field("semantic_order", &["time", "background"])
            .field("quarantined", &self.quarantined)
            .finish_non_exhaustive()
    }
}

impl Default for AgentStatusEngine {
    fn default() -> Self {
        Self::new(AgentStatusConfig::default(), Arc::new(SystemClock))
    }
}

impl AgentStatusEngine {
    /// Constructs an attempt-owned engine from launch-scoped configuration.
    #[must_use]
    pub fn new(config: AgentStatusConfig, clock: Arc<dyn AgentStatusClock>) -> Self {
        Self {
            modules: [
                AgentStatusModule::Time(TimeStatusModule {
                    config: config.time.clone(),
                    clock,
                }),
                AgentStatusModule::Background(BackgroundStatusModule {
                    config: config.background.clone(),
                }),
            ],
            config,
            quarantined: HashSet::new(),
            #[cfg(test)]
            test_seam: None,
        }
    }

    /// Returns the launch-scoped configuration carried by this engine.
    #[must_use]
    pub fn config(&self) -> &AgentStatusConfig {
        &self.config
    }

    fn clock(&self) -> Arc<dyn AgentStatusClock> {
        match &self.modules[0] {
            AgentStatusModule::Time(module) => module.clock.clone(),
            AgentStatusModule::Background(_) => unreachable!("Time owns the engine clock"),
        }
    }

    /// Attaches the deterministic in-crate failure/counting seam.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_test_seam(mut self, seam: AgentStatusTestSeam) -> Self {
        self.test_seam = Some(seam);
        self
    }

    /// Captures, evaluates, validates, and bounds one `FreshInbound`
    /// generation. The engine's module array is traversed exactly in source
    /// order: Time, then Background.
    #[must_use]
    pub fn prepare(
        &mut self,
        opportunities: &AgentStatusOpportunitySet,
        background: &ConversationBackgroundRegistry,
    ) -> Option<AgentStatus> {
        let opportunity = opportunities.fresh_inbound.as_ref()?;
        #[cfg(test)]
        let seam = self.test_seam.clone();
        #[cfg(not(test))]
        let seam = None;
        let mut sections = Vec::new();
        for index in 0..self.modules.len() {
            let module = &self.modules[index];
            let id = module.id();
            if !module.enabled() || self.quarantined.contains(&id) {
                continue;
            }
            let result = (|| {
                let snapshot = module.capture(background, seam.as_ref())?;
                #[cfg(test)]
                if let Some(seam) = seam.as_ref() {
                    seam.run_after_capture(id);
                }
                let Some(payload) = module.evaluate(&snapshot, opportunity, seam.as_ref())? else {
                    return Ok(None);
                };
                validate_payload(id, payload)
            })();
            match result {
                Ok(Some(section)) => sections.push(section),
                Ok(None) => {}
                Err(phase) => self.quarantine(id, phase),
            }
        }
        let accepted = admit_sections(sections);
        (!accepted.sections.is_empty()).then_some(accepted)
    }

    fn quarantine(&mut self, id: AgentStatusModuleId, phase: ModuleFailurePhase) {
        self.quarantined.insert(id);
        tracing::warn!(
            module = id.as_str(),
            phase = phase.as_str(),
            "Agent Status module contribution quarantined for this attempt"
        );
    }
}

fn validate_payload(
    id: AgentStatusModuleId,
    payload: AgentStatusPayload,
) -> Result<Option<AgentStatusSection>, ModuleFailurePhase> {
    match (id, payload) {
        (
            AgentStatusModuleId::Time,
            AgentStatusPayload::Temporal {
                current_time,
                timezone,
                inbound_message_time,
            },
        ) => Ok(Some(AgentStatusSection {
            id: AgentStatusSectionId::new(id.section_id()),
            data: AgentStatusSectionData::Temporal {
                current_time,
                timezone,
                inbound_message_time,
            },
        })),
        (
            AgentStatusModuleId::Background,
            AgentStatusPayload::BackgroundExecution {
                executions,
                omitted_count,
            },
        ) if !executions.is_empty() || omitted_count > 0 => Ok(Some(AgentStatusSection {
            id: AgentStatusSectionId::new(id.section_id()),
            data: AgentStatusSectionData::BackgroundExecution {
                executions,
                omitted_count,
            },
        })),
        (AgentStatusModuleId::Background, AgentStatusPayload::BackgroundExecution { .. }) => {
            Ok(None)
        }
        _ => Err(ModuleFailurePhase::PayloadValidation),
    }
}

/// Applies the global defensive byte cap.
///
/// Admission is whole-section and semantic-order based. Every candidate is
/// rendered from scratch, so separators are accounted for using the retained
/// set. If a section is too large, later sections still get a chance to fit;
/// no rendered wrapper or UTF-8 string is ever byte-sliced.
fn admit_sections(sections: Vec<AgentStatusSection>) -> AgentStatus {
    let mut accepted = Vec::new();
    for section in sections {
        let mut candidate = accepted.clone();
        candidate.push(section.clone());
        if render_sections(&candidate).len() <= GLOBAL_AGENT_STATUS_BYTE_CAP {
            accepted.push(section);
        }
    }
    let status = AgentStatus { sections: accepted };
    let rendered = render_sections(&status.sections);
    assert!(
        rendered.len() <= GLOBAL_AGENT_STATUS_BYTE_CAP,
        "Agent Status renderer exceeded its global UTF-8 byte cap"
    );
    status
}

fn bound_background_snapshot(
    mut snapshot: BackgroundExecutionSnapshot,
) -> BackgroundExecutionSnapshot {
    snapshot.tool_name = bound_status_text(snapshot.tool_name);
    snapshot.progress = snapshot.progress.map(|progress| ToolProgress {
        message: progress.message.map(bound_status_text),
        completed: progress.completed,
        total: progress.total,
    });
    snapshot
}

fn bound_status_text(text: String) -> String {
    if text.len() <= MAX_BACKGROUND_STATUS_TEXT_BYTES {
        return text;
    }
    let marker = "…";
    let limit = MAX_BACKGROUND_STATUS_TEXT_BYTES.saturating_sub(marker.len());
    let mut end = limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = text[..end].to_owned();
    bounded.push_str(marker);
    bounded
}

fn render_instant(instant: DateTime<Utc>, timezone: Option<Tz>) -> String {
    match timezone {
        Some(timezone) => instant
            .with_timezone(&timezone)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
        None => instant.to_rfc3339_opts(SecondsFormat::Secs, true),
    }
}

fn render_sections(sections: &[AgentStatusSection]) -> String {
    let mut lines = Vec::new();
    for section in sections {
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
                if let Some(timezone) = timezone {
                    lines.push(format!("Timezone: {}", timezone.name()));
                }
                lines.push(format!(
                    "Inbound message time: {}",
                    render_instant(*inbound_message_time, *timezone)
                ));
            }
            AgentStatusSectionData::BackgroundExecution {
                executions,
                omitted_count,
            } => {
                if executions.is_empty() && *omitted_count == 0 {
                    continue;
                }
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
                if *omitted_count > 0 {
                    lines.push(format!("- … and {omitted_count} more active executions"));
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

/// Renders the already-admitted Agent Status generation.
///
/// The engine performs whole-section admission before this function is
/// called. The assertion protects the canonical renderer if a future caller
/// constructs a status outside the engine.
///
/// # Panics
///
/// Panics if the supplied status renders above the global UTF-8-byte cap.
#[must_use]
pub fn render_agent_status(status: &AgentStatus) -> String {
    let rendered = render_sections(&status.sections);
    assert!(
        rendered.len() <= GLOBAL_AGENT_STATUS_BYTE_CAP,
        "Agent Status renderer exceeded its global UTF-8 byte cap"
    );
    rendered
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct AgentStatusTestSeam {
    state: Arc<AgentStatusTestState>,
}

#[cfg(not(test))]
struct AgentStatusTestSeam;

#[cfg(not(test))]
#[allow(clippy::unused_self)]
impl AgentStatusTestSeam {
    fn record_capture(&self, _module: AgentStatusModuleId) {}

    fn take_capture_failure(&self, _module: AgentStatusModuleId) -> bool {
        false
    }

    fn record_evaluate(&self, _module: AgentStatusModuleId) {}

    fn take_evaluate_failure(&self, _module: AgentStatusModuleId) -> bool {
        false
    }

    fn take_payload_mismatch(&self, _module: AgentStatusModuleId) -> bool {
        false
    }
}

#[cfg(test)]
type AfterCaptureHook = Arc<dyn Fn(AgentStatusModuleId) + Send + Sync>;

#[cfg(test)]
struct AgentStatusTestState {
    capture_time: std::sync::atomic::AtomicUsize,
    capture_background: std::sync::atomic::AtomicUsize,
    evaluate_time: std::sync::atomic::AtomicUsize,
    evaluate_background: std::sync::atomic::AtomicUsize,
    capture_failure: std::sync::Mutex<Option<AgentStatusModuleId>>,
    evaluate_failure: std::sync::Mutex<Option<AgentStatusModuleId>>,
    payload_mismatch: std::sync::Mutex<Option<AgentStatusModuleId>>,
    after_capture: std::sync::Mutex<Option<AfterCaptureHook>>,
}

#[cfg(test)]
impl AgentStatusTestSeam {
    /// Creates an empty deterministic status test seam.
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(AgentStatusTestState {
                capture_time: std::sync::atomic::AtomicUsize::new(0),
                capture_background: std::sync::atomic::AtomicUsize::new(0),
                evaluate_time: std::sync::atomic::AtomicUsize::new(0),
                evaluate_background: std::sync::atomic::AtomicUsize::new(0),
                capture_failure: std::sync::Mutex::new(None),
                evaluate_failure: std::sync::Mutex::new(None),
                payload_mismatch: std::sync::Mutex::new(None),
                after_capture: std::sync::Mutex::new(None),
            }),
        }
    }

    /// Fails exactly one future capture of `module`.
    pub(crate) fn fail_capture_once(&self, module: AgentStatusModuleId) {
        *self
            .state
            .capture_failure
            .lock()
            .expect("capture failure lock") = Some(module);
    }

    /// Fails exactly one future evaluation of `module`.
    pub(crate) fn fail_evaluate_once(&self, module: AgentStatusModuleId) {
        *self
            .state
            .evaluate_failure
            .lock()
            .expect("evaluate failure lock") = Some(module);
    }

    /// Forces exactly one module/payload ownership mismatch.
    pub(crate) fn mismatch_once(&self, module: AgentStatusModuleId) {
        *self
            .state
            .payload_mismatch
            .lock()
            .expect("payload mismatch lock") = Some(module);
    }

    /// Installs a callback invoked between capture and evaluation.
    pub(crate) fn after_capture(
        &self,
        callback: impl Fn(AgentStatusModuleId) + Send + Sync + 'static,
    ) {
        *self.state.after_capture.lock().expect("after capture lock") = Some(Arc::new(callback));
    }

    /// Returns the exact capture count for a module.
    pub(crate) fn capture_count(&self, module: AgentStatusModuleId) -> usize {
        use std::sync::atomic::Ordering;
        match module {
            AgentStatusModuleId::Time => self.state.capture_time.load(Ordering::SeqCst),
            AgentStatusModuleId::Background => self.state.capture_background.load(Ordering::SeqCst),
        }
    }

    /// Returns the exact evaluation count for a module.
    pub(crate) fn evaluate_count(&self, module: AgentStatusModuleId) -> usize {
        use std::sync::atomic::Ordering;
        match module {
            AgentStatusModuleId::Time => self.state.evaluate_time.load(Ordering::SeqCst),
            AgentStatusModuleId::Background => {
                self.state.evaluate_background.load(Ordering::SeqCst)
            }
        }
    }

    fn record_capture(&self, module: AgentStatusModuleId) {
        use std::sync::atomic::Ordering;
        match module {
            AgentStatusModuleId::Time => {
                self.state.capture_time.fetch_add(1, Ordering::SeqCst);
            }
            AgentStatusModuleId::Background => {
                self.state.capture_background.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    fn record_evaluate(&self, module: AgentStatusModuleId) {
        use std::sync::atomic::Ordering;
        match module {
            AgentStatusModuleId::Time => {
                self.state.evaluate_time.fetch_add(1, Ordering::SeqCst);
            }
            AgentStatusModuleId::Background => {
                self.state
                    .evaluate_background
                    .fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    fn take_capture_failure(&self, module: AgentStatusModuleId) -> bool {
        let mut failure = self
            .state
            .capture_failure
            .lock()
            .expect("capture failure lock");
        if *failure == Some(module) {
            *failure = None;
            true
        } else {
            false
        }
    }

    fn take_evaluate_failure(&self, module: AgentStatusModuleId) -> bool {
        let mut failure = self
            .state
            .evaluate_failure
            .lock()
            .expect("evaluate failure lock");
        if *failure == Some(module) {
            *failure = None;
            true
        } else {
            false
        }
    }

    fn take_payload_mismatch(&self, module: AgentStatusModuleId) -> bool {
        let mut mismatch = self
            .state
            .payload_mismatch
            .lock()
            .expect("payload mismatch lock");
        if *mismatch == Some(module) {
            *mismatch = None;
            true
        } else {
            false
        }
    }

    fn run_after_capture(&self, module: AgentStatusModuleId) {
        if let Some(callback) = self
            .state
            .after_capture
            .lock()
            .expect("after capture lock")
            .as_ref()
        {
            callback(module);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::{ToolExecutionId, ToolId};
    use crate::tools::background::BackgroundLifecycle;

    fn opportunity() -> AgentStatusOpportunitySet {
        AgentStatusOpportunitySet {
            fresh_inbound: Some(FreshInboundStatusOpportunity {
                target_message_id: MessageId::new("inbound"),
                inbound_message_time: DateTime::from_timestamp(1_754_000_000, 0)
                    .expect("timestamp"),
            }),
        }
    }

    fn background_snapshot(index: usize, detail: &str) -> BackgroundExecutionSnapshot {
        BackgroundExecutionSnapshot {
            execution_id: ToolExecutionId::new(format!("exec-{index}")),
            tool_id: ToolId::new("background_task"),
            tool_name: "background_task".to_owned(),
            state: BackgroundLifecycle::Running,
            progress: Some(ToolProgress {
                message: Some(detail.to_owned()),
                completed: Some(1.0),
                total: Some(2.0),
            }),
            result: None,
        }
    }

    #[derive(Debug)]
    struct FixedClock(DateTime<Utc>);

    impl AgentStatusClock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    #[derive(Clone)]
    struct MutableClock(Arc<std::sync::Mutex<DateTime<Utc>>>);

    impl AgentStatusClock for MutableClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().expect("mutable clock lock")
        }
    }

    fn engine(config: AgentStatusConfig) -> AgentStatusEngine {
        AgentStatusEngine::new(
            config,
            Arc::new(FixedClock(
                DateTime::from_timestamp(1_754_000_001, 0).expect("timestamp"),
            )),
        )
    }

    fn empty_background() -> (
        crate::scripted_suites::common::ToolRuntimeFixture,
        ConversationBackgroundRegistry,
    ) {
        let fixture = crate::scripted_suites::common::tool_runtime("agent-status-tests");
        let registry = fixture.background().clone();
        (fixture, registry)
    }

    #[test]
    fn default_engine_delivers_time_before_background() {
        let (_fixture, registry) = empty_background();
        let mut engine = engine(AgentStatusConfig::default());
        let status = engine
            .prepare(&opportunity(), &registry)
            .expect("time status");
        assert_eq!(status.sections.len(), 1);
        assert_eq!(status.sections[0].id.as_str(), "temporal");
    }

    #[test]
    fn time_disabled_produces_no_time_contribution() {
        let (_fixture, registry) = empty_background();
        let mut engine = engine(AgentStatusConfig {
            time: TimeStatusConfig {
                enabled: false,
                timezone: None,
            },
            background: BackgroundStatusConfig::default(),
        });
        assert!(engine.prepare(&opportunity(), &registry).is_none());
    }

    #[test]
    fn background_disabled_keeps_time_without_background_contribution() {
        let (_fixture, registry) = empty_background();
        let mut engine = engine(AgentStatusConfig {
            time: TimeStatusConfig::default(),
            background: BackgroundStatusConfig { enabled: false },
        });
        let status = engine
            .prepare(&opportunity(), &registry)
            .expect("time status");
        assert_eq!(status.sections.len(), 1);
        assert_eq!(status.sections[0].id.as_str(), "temporal");
    }

    #[test]
    fn disabled_modules_produce_no_generation() {
        let (_fixture, registry) = empty_background();
        let mut engine = engine(AgentStatusConfig {
            time: TimeStatusConfig {
                enabled: false,
                timezone: None,
            },
            background: BackgroundStatusConfig { enabled: false },
        });
        assert!(engine.prepare(&opportunity(), &registry).is_none());
    }

    #[test]
    fn capture_and_evaluate_counts_are_once_per_generation() {
        let seam = AgentStatusTestSeam::new();
        let mut engine = engine(AgentStatusConfig::default()).with_test_seam(seam.clone());
        let (_fixture, registry) = empty_background();
        let _ = engine.prepare(&opportunity(), &registry);
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 1);
        assert_eq!(seam.evaluate_count(AgentStatusModuleId::Time), 1);
        assert_eq!(seam.capture_count(AgentStatusModuleId::Background), 1);
        assert_eq!(seam.evaluate_count(AgentStatusModuleId::Background), 1);
    }

    #[test]
    fn failures_quarantine_one_module_and_new_engine_retries_it() {
        let seam = AgentStatusTestSeam::new();
        seam.fail_capture_once(AgentStatusModuleId::Time);
        let (_fixture, registry) = empty_background();
        let mut first = engine(AgentStatusConfig::default()).with_test_seam(seam.clone());
        assert!(
            first.prepare(&opportunity(), &registry).is_none(),
            "a failed Time module leaves no useful status when Background is empty"
        );
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 1);
        let _ = first.prepare(&opportunity(), &registry);
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 1);

        let mut second = engine(AgentStatusConfig::default()).with_test_seam(seam.clone());
        let status = second.prepare(&opportunity(), &registry).expect("retry");
        assert_eq!(status.sections[0].id.as_str(), "temporal");
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 2);
    }

    #[test]
    fn evaluate_failure_and_payload_mismatch_are_isolated() {
        let seam = AgentStatusTestSeam::new();
        seam.fail_evaluate_once(AgentStatusModuleId::Time);
        let mut failure_engine = engine(AgentStatusConfig::default()).with_test_seam(seam.clone());
        let (_fixture, registry) = empty_background();
        assert!(failure_engine.prepare(&opportunity(), &registry).is_none());
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 1);
        assert_eq!(seam.evaluate_count(AgentStatusModuleId::Time), 1);

        let seam = AgentStatusTestSeam::new();
        seam.mismatch_once(AgentStatusModuleId::Time);
        let mut engine = engine(AgentStatusConfig::default()).with_test_seam(seam);
        assert!(engine.prepare(&opportunity(), &registry).is_none());
    }

    #[test]
    fn evaluation_uses_frozen_snapshot() {
        let fixed = DateTime::from_timestamp(1_754_000_001, 0).expect("timestamp");
        let later = DateTime::from_timestamp(1_854_000_001, 0).expect("timestamp");
        let clock = Arc::new(std::sync::Mutex::new(fixed));
        let seam = AgentStatusTestSeam::new();
        let clock_after_capture = Arc::clone(&clock);
        seam.after_capture(move |module| {
            if module == AgentStatusModuleId::Time {
                *clock_after_capture.lock().expect("mutable clock lock") = later;
            }
        });
        let mut engine =
            AgentStatusEngine::new(AgentStatusConfig::default(), Arc::new(MutableClock(clock)))
                .with_test_seam(seam);
        let (_fixture, registry) = empty_background();
        let status = engine.prepare(&opportunity(), &registry).expect("status");
        assert!(matches!(
            status.sections.first().map(|section| &section.data),
            Some(AgentStatusSectionData::Temporal { current_time, .. }) if *current_time == fixed
        ));
    }

    #[test]
    fn background_semantic_bounds_report_omitted_entries_and_bound_text() {
        let snapshots = (0..MAX_BACKGROUND_STATUS_EXECUTIONS + 3)
            .map(|index| background_snapshot(index, &"😀".repeat(400)))
            .collect::<Vec<_>>();
        let payload = BackgroundStatusModule::evaluate(&BackgroundStatusSnapshot {
            executions: snapshots,
        })
        .expect("background contribution");
        let AgentStatusPayload::BackgroundExecution {
            executions,
            omitted_count,
        } = payload
        else {
            panic!("wrong payload");
        };
        assert_eq!(executions.len(), MAX_BACKGROUND_STATUS_EXECUTIONS);
        assert_eq!(omitted_count, 3);
        assert!(
            executions[0]
                .progress
                .as_ref()
                .and_then(|progress| progress.message.as_ref())
                .expect("message")
                .len()
                <= MAX_BACKGROUND_STATUS_TEXT_BYTES
        );
    }

    #[test]
    fn global_admission_uses_utf8_bytes_whole_sections_and_continues() {
        let oversized = AgentStatusSection {
            id: AgentStatusSectionId::new("oversized"),
            data: AgentStatusSectionData::BackgroundExecution {
                executions: vec![background_snapshot(
                    0,
                    // The scalar count is below the cap, but the UTF-8 byte
                    // count plus wrapper overhead is above it. A scalar-count
                    // implementation would incorrectly admit this section.
                    &"😀".repeat(1_020),
                )],
                omitted_count: 0,
            },
        };
        let small = AgentStatusSection {
            id: AgentStatusSectionId::new("small"),
            data: AgentStatusSectionData::Temporal {
                current_time: DateTime::from_timestamp(1_754_000_001, 0).expect("timestamp"),
                timezone: None,
                inbound_message_time: DateTime::from_timestamp(1_754_000_000, 0)
                    .expect("timestamp"),
            },
        };
        let status = admit_sections(vec![oversized, small]);
        assert_eq!(status.sections.len(), 1);
        assert_eq!(status.sections[0].id.as_str(), "small");
        let rendered = render_agent_status(&status);
        assert!(rendered.len() <= GLOBAL_AGENT_STATUS_BYTE_CAP);
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
        assert!(!rendered.contains("oversized"));
    }

    /// A live background record proves that module order is source-owned and
    /// that a failure in Background leaves the surviving Time contribution
    /// available. The registry runner is released and awaited explicitly so
    /// this test has no leaked task or timing race.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)]
    async fn semantic_order_and_background_failure_isolation_are_deterministic() {
        let fixture = crate::scripted_suites::common::tool_runtime("agent-status-order");
        let invocation = crate::tools::types::ToolInvocation {
            call_id: crate::runtime::identity::ToolCallId::new("call-1"),
            tool_id: crate::runtime::identity::ToolId::new("tool-background"),
            tool_name: "background".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let (tool, release) = crate::scripted_suites::support::fake::FakeTool::parking(
            crate::scripted_suites::common::tool_policies(
                "background",
                "tool-background",
                crate::tools::types::ToolExecutionPolicy::ModelSelectable,
                crate::tools::types::ToolConcurrencyPolicy::Sequential,
            ),
            crate::scripted_suites::support::fake::success_result("done"),
        );
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .background()
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("background dispatch prepares");
        fixture
            .background()
            .commit_dispatch(prepared, &crate::runtime::CancellationSignal::new())
            .expect("background dispatch commits");
        let registry = fixture.background().clone();

        let mut ordered = engine(AgentStatusConfig::default());
        let status = ordered
            .prepare(&opportunity(), &registry)
            .expect("Time and Background contribute");
        let ids = status
            .sections
            .iter()
            .map(|section| section.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["temporal", "background_execution"]);
        let rendered = render_agent_status(&status);
        assert!(
            rendered.find("Current time:").expect("time line")
                < rendered
                    .find("Background executions:")
                    .expect("background line")
        );

        let mut time_disabled = engine(AgentStatusConfig {
            time: TimeStatusConfig {
                enabled: false,
                timezone: None,
            },
            background: BackgroundStatusConfig::default(),
        });
        let time_disabled_status = time_disabled
            .prepare(&opportunity(), &registry)
            .expect("Background survives with Time disabled");
        assert_eq!(
            time_disabled_status.sections[0].id.as_str(),
            "background_execution"
        );

        let mut background_disabled = engine(AgentStatusConfig {
            time: TimeStatusConfig::default(),
            background: BackgroundStatusConfig { enabled: false },
        });
        let background_disabled_status = background_disabled
            .prepare(&opportunity(), &registry)
            .expect("Time survives with Background disabled");
        assert_eq!(background_disabled_status.sections.len(), 1);
        assert_eq!(
            background_disabled_status.sections[0].id.as_str(),
            "temporal"
        );

        for phase in [
            ModuleFailurePhase::Capture,
            ModuleFailurePhase::Evaluate,
            ModuleFailurePhase::PayloadValidation,
        ] {
            let seam = AgentStatusTestSeam::new();
            match phase {
                ModuleFailurePhase::Capture => {
                    seam.fail_capture_once(AgentStatusModuleId::Background);
                }
                ModuleFailurePhase::Evaluate => {
                    seam.fail_evaluate_once(AgentStatusModuleId::Background);
                }
                ModuleFailurePhase::PayloadValidation => {
                    seam.mismatch_once(AgentStatusModuleId::Background);
                }
            }
            let mut failing = engine(AgentStatusConfig::default()).with_test_seam(seam);
            let surviving = failing
                .prepare(&opportunity(), &registry)
                .expect("Time survives a Background failure");
            assert_eq!(surviving.sections.len(), 1);
            assert_eq!(surviving.sections[0].id.as_str(), "temporal");
        }

        let execution_id = crate::runtime::identity::ToolExecutionId::new("exec_1");
        release.send_replace(true);
        registry.wait_until_terminal(&execution_id).await;
    }
}
