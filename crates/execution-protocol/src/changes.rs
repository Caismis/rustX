//! Bounded per-attempt Workspace observation on the admitted Client.
use crate::process::ProcessOperationKey;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Maximum retained paths in a summary.
pub const MAX_CHANGE_FILES: usize = 500;
/// Maximum encoded bytes for one completed summary or comparison.
pub const MAX_CHANGE_BYTES: usize = 896 * 1024;

/// Commands for one Client-owned snapshot record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ChangeRequest {
    /// Begin the physical baseline before the first workspace operation.
    Begin { key: ProcessOperationKey },
    /// Verify that the baseline is ready.
    Baseline { key: ProcessOperationKey },
    /// Capture the original of a confined file before its first commit.
    Capture {
        key: ProcessOperationKey,
        path: PathBuf,
    },
    /// Take the final snapshot and compare it with the baseline.
    Finish { key: ProcessOperationKey },
    /// Observe the final bounded summary.
    Summary { key: ProcessOperationKey },
    /// Begin one comparison for a previously returned summary index.
    Compare {
        key: ProcessOperationKey,
        index: usize,
    },
    /// Observe the exact requested comparison.
    Comparison {
        key: ProcessOperationKey,
        index: usize,
    },
    /// Release a record and its private physical ownership.
    Release { key: ProcessOperationKey },
}

impl ChangeRequest {
    /// The immutable operation identity shared by every record command.
    #[must_use]
    pub fn key(&self) -> &ProcessOperationKey {
        match self {
            Self::Begin { key }
            | Self::Baseline { key }
            | Self::Capture { key, .. }
            | Self::Finish { key }
            | Self::Summary { key }
            | Self::Compare { key, .. }
            | Self::Comparison { key, .. }
            | Self::Release { key } => key,
        }
    }
}

/// Bounded physical answers. The Server owns canonical publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ChangeReply {
    /// Accepted control of the exact record.
    Accepted,
    /// Physical work is still being driven by the Client.
    Pending,
    /// Baseline is usable.
    BaselineReady,
    /// Completed physical summary.
    Summary(Option<ChangesSummary>),
    /// Exact indexed comparison.
    Comparison {
        index: usize,
        comparison: Option<FileComparison>,
    },
    /// Invalid, foreign, retired, failed or over-capacity record.
    Unavailable,
}

/// One file's actual net change during an attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangedFile {
    pub path: String,
    pub display: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
    pub oversized: bool,
}

/// Bounded paths and counts from one completed observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangesSummary {
    pub files: Vec<ChangedFile>,
    pub total: usize,
    pub added: usize,
    pub deleted: usize,
}

/// Three lines of shared context around a group of edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComparisonHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub old_text: String,
    pub new_text: String,
}

/// Bounded comparison of the recorded endpoints of one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileComparison {
    pub hunks: Vec<ComparisonHunk>,
    pub binary: bool,
    pub oversized: bool,
    pub truncated: bool,
}
