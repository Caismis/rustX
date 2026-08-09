//! The conversation-owned tool runtime bundle.
//!
//! One conversation owns one [`ConversationToolRuntime`]: the canonical
//! workspace boundary, the artifact store, the explicit authorized tool
//! environment, and the authoritative background registry. An
//! `AgentExecution` receives a reference to the bundle; detached background
//! runners receive cloned handles. No process-global tool state exists.

use std::path::Path;
use std::sync::Arc;

use crate::events::RuntimeEventSink;
use crate::runtime::RuntimeClock;
use crate::runtime::SystemClock;
use crate::runtime::identity::ConversationId;
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::tools::artifacts::{ArtifactError, ArtifactStore};
use crate::tools::background::{BackgroundResources, ConversationBackgroundRegistry};
use crate::tools::environment::ToolEnvironment;
use crate::tools::workspace::{Workspace, WorkspaceError};

/// A conversation tool runtime construction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationRuntimeError {
    /// The workspace root is invalid.
    Workspace(WorkspaceError),
    /// The artifact root cannot be prepared.
    Artifacts(ArtifactError),
}

impl core::fmt::Display for ConversationRuntimeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Workspace(error) => write!(f, "{error}"),
            Self::Artifacts(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ConversationRuntimeError {}

/// The conversation-owned tool runtime of one conversation.
///
/// Construction canonicalizes the workspace root once and requires it to be
/// a directory; artifact storage must be placed outside the model workspace
/// so Glob/Grep cannot accidentally surface runtime artifact internals.
#[derive(Debug, Clone)]
pub struct ConversationToolRuntime {
    conversation_id: ConversationId,
    workspace: Workspace,
    artifacts: ArtifactStore,
    environment: ToolEnvironment,
    background: ConversationBackgroundRegistry,
}

impl ConversationToolRuntime {
    /// Creates the tool runtime of one conversation.
    ///
    /// A fresh inbound mailbox and the system clock are used by default;
    /// [`ConversationToolRuntime::with_mailbox`],
    /// [`ConversationToolRuntime::with_clock`],
    /// [`ConversationToolRuntime::with_event_sink`], and
    /// [`ConversationToolRuntime::with_environment`] override them.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationRuntimeError::Workspace`] when the workspace
    /// root is missing, not a directory, or cannot be canonicalized, and
    /// [`ConversationRuntimeError::Artifacts`] when the artifact root cannot
    /// be prepared.
    pub fn new(
        conversation_id: ConversationId,
        workspace_root: impl AsRef<Path>,
        artifacts_dir: impl AsRef<Path>,
    ) -> Result<Self, ConversationRuntimeError> {
        let workspace =
            Workspace::new(workspace_root).map_err(ConversationRuntimeError::Workspace)?;
        let artifacts = ArtifactStore::new(conversation_id.clone(), artifacts_dir)
            .map_err(ConversationRuntimeError::Artifacts)?;
        let mailbox = ConversationInboundMailbox::new(conversation_id.clone());
        Ok(Self {
            conversation_id: conversation_id.clone(),
            workspace: workspace.clone(),
            artifacts: artifacts.clone(),
            environment: ToolEnvironment::new(),
            background: ConversationBackgroundRegistry::new(
                conversation_id,
                BackgroundResources {
                    mailbox,
                    workspace,
                    artifacts,
                    environment: ToolEnvironment::new(),
                    clock: Arc::new(SystemClock),
                    event_sink: None,
                },
            ),
        })
    }

    /// Attaches the conversation inbound mailbox used for background
    /// terminal notifications.
    #[must_use]
    pub fn with_mailbox(mut self, mailbox: ConversationInboundMailbox) -> Self {
        self.background = self.rebuild_background(Some(mailbox), None, None, None);
        self
    }

    /// Attaches an explicit runtime clock for deterministic timestamps.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn RuntimeClock>) -> Self {
        self.background = self.rebuild_background(None, Some(clock), None, None);
        self
    }

    /// Attaches the narrow non-durable execution-fact sink.
    #[must_use]
    pub fn with_event_sink(mut self, sink: Arc<dyn RuntimeEventSink>) -> Self {
        self.background = self.rebuild_background(None, None, Some(sink), None);
        self
    }

    /// Attaches the explicit authorized tool environment.
    #[must_use]
    pub fn with_environment(mut self, environment: ToolEnvironment) -> Self {
        let environment_clone = environment.clone();
        self.environment = environment;
        self.background = self.rebuild_background(None, None, None, Some(environment_clone));
        self
    }

    /// The owning conversation.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The canonical workspace boundary.
    #[must_use]
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// The conversation artifact store.
    #[must_use]
    pub fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

    /// The explicit authorized tool environment.
    #[must_use]
    pub fn environment(&self) -> &ToolEnvironment {
        &self.environment
    }

    /// The authoritative conversation background registry.
    #[must_use]
    pub fn background(&self) -> &ConversationBackgroundRegistry {
        &self.background
    }

    /// The conversation inbound mailbox (for terminal background
    /// notifications).
    #[must_use]
    pub fn mailbox(&self) -> ConversationInboundMailbox {
        self.background.resources().mailbox.clone()
    }

    fn rebuild_background(
        &self,
        mailbox: Option<ConversationInboundMailbox>,
        clock: Option<Arc<dyn RuntimeClock>>,
        event_sink: Option<Arc<dyn RuntimeEventSink>>,
        environment: Option<ToolEnvironment>,
    ) -> ConversationBackgroundRegistry {
        let resources = &self.background.resources();
        ConversationBackgroundRegistry::new(
            self.conversation_id.clone(),
            BackgroundResources {
                mailbox: mailbox.unwrap_or_else(|| resources.mailbox.clone()),
                workspace: resources.workspace.clone(),
                artifacts: resources.artifacts.clone(),
                environment: environment.unwrap_or_else(|| resources.environment.clone()),
                clock: clock.unwrap_or_else(|| resources.clock.clone()),
                event_sink: event_sink.or_else(|| resources.event_sink.clone()),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ConversationToolRuntime;
    use crate::runtime::identity::ConversationId;
    use std::fs;

    #[test]
    fn construction_validates_the_workspace_root() {
        let dir = std::env::temp_dir().join(format!("rustx-crt-{}", std::process::id()));
        let runtime = ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            &dir,
            dir.join("artifacts"),
        );
        assert!(
            runtime.is_err(),
            "a missing workspace root must be rejected"
        );
        fs::create_dir_all(&dir).expect("create");
        let runtime = ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            &dir,
            dir.join("artifacts"),
        )
        .expect("runtime");
        assert_eq!(runtime.conversation_id(), &ConversationId::new("conv-1"));
        assert!(runtime.workspace().root().is_dir());
        assert!(runtime.artifacts().root().is_dir());
        fs::remove_dir_all(&dir).expect("remove");
    }
}
