//! Remote mutation ownership for native Write/Edit.
//!
//! The Server no longer opens, stages, or commits mutation targets: the
//! bound object is a logical admitted path plus one shared, once-consumed
//! [`PreparedRemoteMutation`] whose ticketed preparation lives on the
//! Session's admitted Execution Client. Exactly one commit or abort
//! decision consumes the owner; a drop before either arranges a bounded
//! remote abort — never a local filesystem touch and never an implicit
//! publish.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rustx_execution_server::PreparedMutation;

use crate::local_runtime::session_runtime_manager::SessionExecutionScope;
use crate::runtime::cancellation::{CancellationSignal, ExecutionCancellation};
use crate::runtime::types::CancellationReason;

/// The Write/Edit mutation claim bound before permission classification:
/// the logical interpreted target — the permission-UI path — plus the
/// shared remote owner of the live ticketed preparation. A
/// `BoundMutationTarget` exists only after a valid physical prepare on the
/// admitted Client; bind failures short-circuit before permission
/// classification and never reach this type.
///
/// `logical` is `interpret_path`'s workspace-anchored lexical target: the
/// physical path is the Client's retained descriptor chain, and no
/// Server-side `stat`, `canonicalize`, or re-resolution ever proves it.
#[derive(Clone)]
pub struct BoundMutationTarget {
    logical: PathBuf,
    prepared: Arc<PreparedRemoteMutation>,
}

impl std::fmt::Debug for BoundMutationTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The logical target and whether a live ticketed owner is held —
        // never ticket internals or payload bytes.
        formatter
            .debug_struct("BoundMutationTarget")
            .field("logical", &self.logical)
            .field("live", &self.prepared.live_for_debug())
            .finish()
    }
}

impl std::fmt::Debug for PreparedRemoteMutation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedRemoteMutation")
            .field("live", &self.live_for_debug())
            .finish_non_exhaustive()
    }
}

impl PartialEq for BoundMutationTarget {
    fn eq(&self, other: &Self) -> bool {
        self.logical == other.logical && std::sync::Arc::ptr_eq(&self.prepared, &other.prepared)
    }
}
impl Eq for BoundMutationTarget {}

impl BoundMutationTarget {
    /// The logical target the permission view and PLAN.md classification
    /// compare — the Client-proved interpretation, never re-resolved.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.logical
    }

    /// A bound target holding the shared remote owner.
    pub(crate) fn ready(logical: PathBuf, owner: Arc<PreparedRemoteMutation>) -> Self {
        Self {
            logical,
            prepared: owner,
        }
    }

    /// Aborts the live ticketed preparation — the explicit
    /// denied/cancelled-preflight cleanup. An already-consumed owner is
    /// a no-op; the owner's drop covers every other settlement path.
    pub(crate) async fn abort(&self) {
        let _ = self.prepared.abort().await;
    }

    /// The shared remote owner of the live ticketed preparation.
    pub(crate) fn prepared(&self) -> Arc<PreparedRemoteMutation> {
        self.prepared.clone()
    }
}

/// The shared remote owner of one ticketed preparation: `scope` is the
/// Session's non-owning execution route and `prepared` the once-consumed
/// ticket fact. `commit`/`abort` each consume it exactly once; dropping
/// before either arranges a bounded remote abort on the runtime.
pub struct PreparedRemoteMutation {
    scope: SessionExecutionScope,
    prepared: Mutex<Option<PreparedMutation>>,
}

impl PreparedRemoteMutation {
    pub(crate) fn new(scope: SessionExecutionScope, prepared: PreparedMutation) -> Self {
        Self {
            scope,
            prepared: Mutex::new(Some(prepared)),
        }
    }

    fn take(&self) -> Option<PreparedMutation> {
        self.prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Consumes the ticket and commits the publish crossing. A clean
    /// pre-crossing error best-effort aborts the ticket under a detached
    /// cancellation — a refusal that never reached the wire may leave it
    /// live. `WriteOutcomeUnknown` sends no spurious abort: the ticket is
    /// already consumed on whichever side published, and a second
    /// `AbortWrite` would hit a foreign or vacant slot. Cancellation
    /// normalization and `WriteOutcomeUnknown` → `OutcomeUnknown`
    /// mapping are the tool's.
    pub(crate) async fn commit(
        &self,
        cancellation: &ExecutionCancellation,
    ) -> std::io::Result<rustx_confined_fs::FileMetadata> {
        let Some(prepared) = self.take() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "prepared mutation already consumed",
            ));
        };
        match self.scope.commit_prepared(&prepared, cancellation).await {
            Ok(stamp) => Ok(stamp),
            Err(error) => {
                let unknown = error.get_ref().is_some_and(
                    <dyn std::error::Error + Send + Sync + 'static>::is::<
                        rustx_execution_server::WriteOutcomeUnknown,
                    >,
                );
                if !unknown {
                    let _ = self.abort_prepared_detached(&prepared).await;
                }
                Err(error)
            }
        }
    }

    /// Whether a live ticketed preparation is still held — Debug only.
    fn live_for_debug(&self) -> bool {
        self.prepared.lock().is_ok_and(|slot| slot.is_some())
    }

    /// Consumes the ticket and aborts the preparation — the explicit
    /// denied/cancelled-preflight cleanup path.
    pub(crate) async fn abort(&self) -> std::io::Result<()> {
        let Some(prepared) = self.take() else {
            return Ok(());
        };
        self.abort_prepared_detached(&prepared).await
    }

    /// One bounded abort under a detached cancellation: cleanup is never
    /// itself cancelled away by the cancelled waiter it serves.
    async fn abort_prepared_detached(&self, prepared: &PreparedMutation) -> std::io::Result<()> {
        let detached = ExecutionCancellation::detached(
            CancellationSignal::new(),
            CancellationReason::UserRequested,
        );
        self.scope.abort_prepared(prepared, &detached).await
    }
}

impl Drop for PreparedRemoteMutation {
    fn drop(&mut self) {
        let Ok(mut slot) = self.prepared.lock() else {
            return;
        };
        let Some(prepared) = slot.take() else {
            return;
        };
        drop(slot);
        // Arrange the abort independent of whatever waiter was cancelled:
        // on a live runtime the dispatch runs bounded under its own
        // detached cancellation; with no runtime the ticket honestly
        // stays occupied on the Client until the connection drops — the
        // capacity is never silently reused.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let scope = self.scope.clone();
            handle.spawn(async move {
                let detached = ExecutionCancellation::detached(
                    CancellationSignal::new(),
                    CancellationReason::UserRequested,
                );
                let _ = scope.abort_prepared(&prepared, &detached).await;
            });
        }
    }
}

/// Maps one remote commit outcome onto the tool's terminal result: a
/// pre-wire `Interrupted` normalizes to the settled cancellation,
/// `WriteOutcomeUnknown` to `OutcomeUnknown` — a racing cancellation can
/// never override it — and a clean refusal to the stable failure text.
pub(crate) fn commit_error_result(
    path: &str,
    cancellation: &ExecutionCancellation,
    error: &std::io::Error,
) -> crate::tools::types::ToolExecutionResult {
    use super::support::{cancelled_result, failed_result};
    if error.kind() == std::io::ErrorKind::Interrupted {
        return cancelled_result(cancellation.reason());
    }
    if error.get_ref().is_some_and(
        <dyn std::error::Error + Send + Sync + 'static>::is::<
            rustx_execution_server::WriteOutcomeUnknown,
        >,
    ) {
        return crate::tools::types::ToolExecutionResult {
            deliveries: Vec::new(),
            status: crate::tools::types::ToolExecutionStatus::OutcomeUnknown {
                detail: format!(
                    "the committed mutation outcome cannot be proven for {path}: {error}"
                ),
            },
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        };
    }
    failed_result(format!("cannot persist {path}: {error}"))
}
