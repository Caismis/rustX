//! Bounded hints for one Client-owned, non-recursive workspace observer.
use crate::{DecimalU64, ReadError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Maximum admitted native observers in one authenticated binding.
pub const MAX_WATCHES: usize = 32;
/// Maximum number of paths retained for one coalesced watch update.
pub const MAX_WATCH_HINT_PATHS: usize = 16;
/// Budget of the escaped path representation in one update.
pub const MAX_WATCH_HINT_BYTES: usize = 16 * 1024;

/// Server-created incarnation, fixed at watch admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WatchId(DecimalU64);

impl WatchId {
    /// Mint from a fresh nonzero Server counter.
    #[must_use]
    pub fn new(value: std::num::NonZeroU64) -> Self {
        Self(DecimalU64::new(value.get()))
    }
}

/// Data-lane registration and polling for authenticated Client watches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum WatchRequest {
    /// Observes an exact retained source under the bound Workspace.
    Register {
        /// Fresh watch identity for the caller.
        id: WatchId,
        /// Workspace-relative file or directory. Empty names the Workspace.
        path: PathBuf,
    },
    /// Collects the currently coalesced update for the watcher.
    Poll {
        /// The exact source registration identity.
        id: WatchId,
    },
}

/// Verified watcher receipt or one bounded delta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum WatchReply {
    /// The Client registered the exact descriptor.
    Registered {
        /// The registered identity.
        id: WatchId,
    },
    /// Coalesced changes since the last poll.
    Changed(WatchChange),
    /// Genuine admission or lookup refusal.
    Refused {
        /// Identifies the request.
        id: WatchId,
        /// Stable physical error code.
        kind: ReadError,
    },
    /// Native cleanup could not be confirmed.
    Unknown {
        /// Identifies the request.
        id: WatchId,
    },
}

/// One coalesced watch notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchChange {
    /// Exact registered incarnation.
    pub id: WatchId,
    /// Workspace-relative paths under the exact watched source.
    pub changed_paths: Vec<PathBuf>,
    /// The consumer must re-read to establish current state.
    pub resync_required: bool,
    /// Physical observer cleanup completed and no later hints can follow.
    pub retired: bool,
}
