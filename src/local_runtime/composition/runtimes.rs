//! The final, already-active runtime handles over the shared core: the
//! interactive Runtime Client runtime and the headless runtime.

use super::super::configuration::AdmittedSessionConfig;
use super::LocalConversationCore;
use super::LocalRuntimeDependencies;
use super::LocalRuntimeError;
use crate::capabilities::CapabilityCoordinator;
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime_client::endpoint::RuntimeClientEndpoint;
use crate::runtime_client::host::RuntimeClientHost;
use crate::tools::runtime::ConversationToolRuntime;

/// The composed interactive local conversation runtime.
///
/// It owns the semantic owners of the process (see
/// [`LocalConversationCore`]) plus the Runtime Client host — the
/// projection/control adapter over the runtime — and the endpoint handed
/// to a transport is derived from that adapter.
pub struct LocalConversationRuntime {
    pub(super) core: LocalConversationCore,
    pub(super) host: RuntimeClientHost,
}

/// Failure of a composition owner's shutdown, distinct from execution state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalConversationShutdownError {
    /// The native semantic owner did not prove shutdown.
    NativeRuntimeShutdown(crate::runtime::conversation_runtime::ShutdownError),
    /// Semantic shutdown succeeded, but joining the presentation worker failed.
    ProjectionJoinFailure(LocalProjectionJoinFailure),
}

/// Finite classification of a failed presentation join; no panic payload is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalProjectionJoinFailure {
    /// The worker task was cancelled.
    Cancelled,
    /// The worker task panicked.
    Panicked,
}

impl std::fmt::Display for LocalConversationShutdownError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NativeRuntimeShutdown(_) => "native runtime shutdown remains unproven",
            Self::ProjectionJoinFailure(LocalProjectionJoinFailure::Cancelled) => {
                "presentation worker join failed: cancelled"
            }
            Self::ProjectionJoinFailure(LocalProjectionJoinFailure::Panicked) => {
                "presentation worker join failed: panicked"
            }
        })
    }
}

impl std::error::Error for LocalConversationShutdownError {}

impl std::fmt::Debug for LocalConversationRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalConversationRuntime")
            .field("conversation_id", self.tool_runtime().conversation_id())
            .finish_non_exhaustive()
    }
}

impl LocalConversationRuntime {
    /// Composes the interactive runtime from validated resolved launch inputs.
    ///
    /// The shared semantic composition (see
    /// [`LocalConversationCore::compose`]) is built once, the Runtime
    /// Client host binds over the inert runtime, and the runtime is then
    /// activated. The returned runtime is already active.
    ///
    /// # Errors
    ///
    /// Returns the first composition failure. Every failure happens before
    /// any protocol output exists.
    pub async fn compose(
        paths: &AdmittedSessionConfig,
        dependencies: &LocalRuntimeDependencies,
    ) -> Result<Self, LocalRuntimeError> {
        LocalConversationCore::compose(paths, dependencies)
            .await?
            .into_interactive()
    }

    /// Performs the one shared Inactive -> Running lifecycle transition.
    ///
    /// The client host-binding decision is frozen by the time this runs,
    /// the admission worker starts, and semantic execution may begin. This
    /// is infallible by construction, which is what lets a caller place its
    /// own durable commit immediately before it.
    pub(crate) fn activate(&self) {
        self.core.runtime.activate();
    }

    /// The one conversation runtime coordinator of this process.
    #[must_use]
    pub const fn runtime(&self) -> &ConversationRuntime {
        &self.core.runtime
    }

    /// The resource preparation this Session composition was built with.
    pub(crate) fn resource_loader(
        &self,
    ) -> Option<&std::sync::Arc<super::resources::LocalRuntimeResourceLoader>> {
        self.core.resource_loader.as_ref()
    }

    /// The one Runtime Client host (projection/control adapter) of this
    /// process.
    #[must_use]
    pub const fn host(&self) -> &RuntimeClientHost {
        &self.host
    }

    /// The one conversation tool runtime of this process.
    #[must_use]
    pub const fn tool_runtime(&self) -> &ConversationToolRuntime {
        &self.core.tool_runtime
    }

    /// The one capability coordinator of this process.
    #[must_use]
    pub const fn capability(&self) -> &CapabilityCoordinator {
        &self.core.capability
    }

    /// Drain the semantic runtime, then join its downstream presentation worker
    /// and all physical reads owned by that worker. Unlike `runtime().shutdown()`,
    /// successful completion proves the presentation side has terminated.
    /// Cancelling this wait retains the original worker in the host; a subsequent
    /// call joins that same worker. The composition and any explicitly retained
    /// external leases still own storage until their handles are dropped.
    ///
    /// # Errors
    /// Returns the native settlement failure, or an explicit presentation join failure.
    pub async fn shutdown(&self) -> Result<(), LocalConversationShutdownError> {
        self.core
            .shutdown()
            .await
            .map_err(LocalConversationShutdownError::NativeRuntimeShutdown)?;
        self.host.inner.drain_projection().await.map_err(|error| {
            LocalConversationShutdownError::ProjectionJoinFailure(if error.is_cancelled() {
                LocalProjectionJoinFailure::Cancelled
            } else {
                LocalProjectionJoinFailure::Panicked
            })
        })
    }

    /// Creates the Runtime Client endpoint a transport wraps.
    #[must_use]
    pub fn endpoint(&self) -> RuntimeClientEndpoint {
        RuntimeClientEndpoint::new(&self.host)
    }
}

/// The composed headless local conversation runtime.
///
/// The same semantic composition as the interactive runtime
/// ([`LocalConversationCore`]), activated with **no** Runtime Client host:
/// no projection, no attachment policy, no protocol endpoint. Headless
/// drivers publish ordinary inbound through
/// [`ConversationRuntime::submit_inbound`](crate::runtime::conversation_runtime::ConversationRuntime::submit_inbound)
/// and await settlement through the runtime's settlement signal.
pub struct HeadlessConversationRuntime {
    pub(super) core: LocalConversationCore,
}

impl std::fmt::Debug for HeadlessConversationRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeadlessConversationRuntime")
            .field("conversation_id", self.tool_runtime().conversation_id())
            .finish_non_exhaustive()
    }
}

impl HeadlessConversationRuntime {
    /// Explicitly prepare and adopt completed MCP catalogs, refresh invalidated
    /// declarations, or retry unavailable configured MCP sources.
    ///
    /// Headless hosts have no Session application UI. They choose this safe
    /// adoption boundary themselves; a late initializer never changes the
    /// current Attempt or implicitly adopts changed model tool declarations.
    /// The source configuration remains the composition's admitted capture.
    ///
    /// # Errors
    /// Returns `Busy` during execution, `NotReady` when no catalog update is needed,
    /// or the existing native adoption conflict/preparation failure.
    pub async fn adopt_ready_capabilities(
        &self,
    ) -> Result<(), crate::runtime::adoption::AdoptionError> {
        use crate::runtime::adoption::AdoptionError;

        self.core
            .runtime
            .idle_epoch()
            .map_err(|_| AdoptionError::Busy)?;
        let recovery = self.core.capability.needs_mcp_refresh();
        if !recovery && !self.core.capability.has_startup_updates() {
            return Err(AdoptionError::NotReady);
        }
        let loader = self
            .core
            .resource_loader
            .as_ref()
            .ok_or(AdoptionError::NotReady)?;
        // Capability preparation already retains its counted owner through
        // settlement. A caller-held admission here would make the native
        // explicit-adoption idle gate reject this operation's own guard.
        let mut capture = loader.initial_capture();
        let selection = self.core.runtime.model_view().configured;
        capture.input.model = Some(selection.clone());
        let cancellation = crate::runtime::cancellation::CancellationSignal::new();
        let prepared = self
            .core
            .runtime
            .prepare_configuration(
                |capability| {
                    if recovery {
                        loader.prepare(capability, capture, &cancellation)
                    } else {
                        loader.prepare_readiness(capability, capture, &cancellation)
                    }
                },
                selection,
            )
            .await
            .map_err(|diagnostic| AdoptionError::Failed { diagnostic })?;
        let baseline = prepared.baseline;
        let mut candidate = Some(prepared);
        let outcome = self
            .core
            .runtime
            .adopt_configuration(&mut candidate, baseline, true, || Ok(()));
        if let Some(mut candidate) = candidate
            && let Some(capability) = candidate.capability.take()
        {
            capability.retire_uncommitted().await;
        }
        self.core.runtime.settle_configuration_resources().await;
        outcome.map(|_| ())
    }

    /// Composes the headless runtime from validated resolved launch inputs.
    ///
    /// The shared semantic composition (see
    /// [`LocalConversationCore::compose`]) is built once and activated
    /// directly, with no Runtime Client host ever constructed. The
    /// returned runtime is already active.
    ///
    /// # Errors
    ///
    /// Returns the first composition failure. Every failure happens before
    /// any protocol output exists.
    pub async fn compose(
        paths: &AdmittedSessionConfig,
        dependencies: &LocalRuntimeDependencies,
    ) -> Result<Self, LocalRuntimeError> {
        Ok(LocalConversationCore::compose(paths, dependencies)
            .await?
            .into_headless())
    }

    /// The one conversation runtime coordinator of this runtime.
    #[must_use]
    pub const fn runtime(&self) -> &ConversationRuntime {
        &self.core.runtime
    }

    /// The one conversation tool runtime of this runtime.
    #[must_use]
    pub const fn tool_runtime(&self) -> &ConversationToolRuntime {
        &self.core.tool_runtime
    }

    /// The one capability coordinator of this runtime.
    #[must_use]
    pub const fn capability(&self) -> &CapabilityCoordinator {
        &self.core.capability
    }

    /// Drain the headless composition's semantic owners; it has no presentation worker.
    ///
    /// # Errors
    /// Returns the native runtime settlement failure.
    pub async fn shutdown(
        &self,
    ) -> Result<(), crate::runtime::conversation_runtime::ShutdownError> {
        self.core.shutdown().await
    }
}
