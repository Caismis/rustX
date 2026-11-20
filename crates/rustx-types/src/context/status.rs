//! The typed Agent Status section vocabulary and its canonical renderer.
//!
//! These are the accepted-status contract shared by the context engine
//! (composition), the durable journal (observation descriptors) and runtime
//! read projections — not engine state.

use chrono::{DateTime, Utc};

/// Maximum rendered bytes for one Background job line in Agent Status.
pub const MAX_BACKGROUND_STATUS_TEXT_BYTES: usize = 256;
/// The global UTF-8 cap on one rendered Agent Status document.
pub const GLOBAL_AGENT_STATUS_BYTE_CAP: usize = 4096;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::message::types::{AgentStatusGenerationMetadata, AgentStatusModuleId};
use crate::tools::background::BackgroundExecutionSnapshot;
use crate::tools::todo::{TodoStatus, TodoStatusPresentation, TodoStatusTask};
use crate::tools::types::ToolProgress;

/// The structured data of one accepted Agent Status section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentStatusSectionData {
    /// The Time module's typed presentation payload.
    Temporal {
        /// The UTC clock instant captured for this generation.
        current_time: DateTime<Utc>,
        /// The configured IANA timezone, when known.
        timezone: Option<Tz>,
    },
    /// The Background module's typed presentation payload.
    BackgroundExecution {
        /// The bounded active execution entries in registry allocation order.
        executions: Vec<BackgroundExecutionSnapshot>,
        /// Active executions omitted by the module-local entry bound.
        omitted_count: usize,
    },
    /// The bounded conversation-owned Todo payload.
    Todo {
        /// The bounded semantic Todo presentation.
        presentation: TodoStatusPresentation,
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
    /// The stable Todo section id.
    pub const TODO: &'static str = "todo";

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatusSection {
    /// The stable section identity.
    pub id: AgentStatusSectionId,
    /// The typed section payload.
    pub data: AgentStatusSectionData,
}

/// One accepted Agent Status generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatus {
    /// The one Agent Status clock instant used for this generation.
    pub generated_at: DateTime<Utc>,
    /// Sections in rustX semantic source order.
    pub sections: Vec<AgentStatusSection>,
}

impl AgentStatus {
    /// Returns the durable descriptor attached to the canonical Agent Status
    /// message for this generation.
    ///
    /// # Panics
    ///
    /// Panics if called on a hand-constructed `AgentStatus` that contains no
    /// valid closed-engine sections. Production generations are assembled by
    /// the closed engine and always satisfy this invariant.
    #[must_use]
    pub fn generation_metadata(&self) -> AgentStatusGenerationMetadata {
        self.checked_generation_metadata()
            .expect("validated Agent Status generation")
    }

    pub fn checked_generation_metadata(&self) -> Result<AgentStatusGenerationMetadata, String> {
        let modules = self
            .sections
            .iter()
            .map(|section| match (section.id.as_str(), &section.data) {
                (AgentStatusSectionId::TEMPORAL, AgentStatusSectionData::Temporal { .. }) => {
                    Ok(AgentStatusModuleId::Time)
                }
                (
                    AgentStatusSectionId::BACKGROUND_EXECUTION,
                    AgentStatusSectionData::BackgroundExecution { .. },
                ) => Ok(AgentStatusModuleId::Background),
                (AgentStatusSectionId::TODO, AgentStatusSectionData::Todo { .. }) => {
                    Ok(AgentStatusModuleId::Todo)
                }
                _ => Err("unknown or inconsistent typed status section".to_owned()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        AgentStatusGenerationMetadata::new(self.generated_at, modules)
            .map_err(|error| error.to_string())
    }
}

pub fn bound_background_snapshot(
    mut snapshot: BackgroundExecutionSnapshot,
) -> BackgroundExecutionSnapshot {
    snapshot.tool_name = bound_status_text(
        snapshot
            .tool_name
            .chars()
            .take(MAX_BACKGROUND_STATUS_TEXT_BYTES)
            .collect(),
    )
    .into();
    snapshot.progress = snapshot.progress.map(|progress| ToolProgress {
        message: progress.message.map(bound_status_text),
        completed: progress.completed,
        total: progress.total,
    });
    snapshot
}

fn bound_status_text(text: String) -> String {
    bound_status_text_to(text, MAX_BACKGROUND_STATUS_TEXT_BYTES)
}

fn bound_status_text_to(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let marker = "…";
    let limit = max_bytes.saturating_sub(marker.len());
    let mut end = limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = text[..end].to_owned();
    bounded.push_str(marker);
    bounded
}

fn render_instant(instant: DateTime<Utc>, timezone: Option<Tz>) -> String {
    let timezone = effective_status_timezone(timezone);
    instant
        .with_timezone(&timezone)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

pub fn render_sections(sections: &[AgentStatusSection]) -> String {
    let mut lines = Vec::new();
    for section in sections {
        match &section.data {
            AgentStatusSectionData::Temporal {
                current_time,
                timezone,
            } => {
                lines.push(format!(
                    "Timezone: {}",
                    effective_status_timezone(*timezone).name()
                ));
                lines.push(format!(
                    "Current time: {}",
                    render_instant(*current_time, *timezone)
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
                lines.push("Background jobs:".to_owned());
                for execution in executions {
                    // The finite Job identity is directly actionable through
                    // job_status, job_wait and job_cancel.
                    let mut line = format!(
                        "- {} {} | {} | {}",
                        "job",
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
            AgentStatusSectionData::Todo { presentation } => {
                if presentation.active_count == 0 {
                    continue;
                }
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(format!(
                    "Todo: {} active ({} blocked, {} completed, {} deleted)",
                    presentation.active_count,
                    presentation.blocked_count,
                    presentation.completed_count,
                    presentation.deleted_count,
                ));
                if let Some(current) = &presentation.current {
                    lines.push(format_todo_task("Current", current));
                }
                for task in &presentation.tasks {
                    lines.push(format_todo_task("Next", task));
                }
                if presentation.omitted_count > 0 {
                    lines.push(format!(
                        "- … and {} more active Todo tasks",
                        presentation.omitted_count
                    ));
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

fn format_todo_task(label: &str, task: &TodoStatusTask) -> String {
    let blocked = if task.blocked { " | blocked" } else { "" };
    let subject = if task.status == TodoStatus::InProgress {
        task.active_form.as_deref().unwrap_or(&task.subject)
    } else {
        &task.subject
    };
    format!(
        "- {label} #{} | {subject} | {}{blocked}",
        task.id, task.status
    )
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
/// distinct labels: the returned value compares by exact zone identity.
#[must_use]
pub const fn effective_status_timezone(timezone: Option<Tz>) -> Tz {
    match timezone {
        Some(zone) => zone,
        None => chrono_tz::UTC,
    }
}
