//! Explicit, bounded model connectivity check against one saved source revision.
use super::configuration::settings::SourceTarget;
use super::session_runtime_manager::SessionRuntimeManager;
use crate::message::content::TextBlock;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::catalog::{ModelCatalog, ModelRef};
use crate::model::invocation::{ModelBindingRegistry, ModelSelection};
use crate::model::{ModelEvent, ModelInputMessage, ModelRequest, ModelStreamItem};
use crate::runtime::CancellationSignal;
use crate::runtime::identity::MessageId;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

/// Both source revisions shown to the user when they requested a test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelProbeSource {
    pub user_revision: String,
    pub workspace_revision: Option<String>,
}

/// Provider responses are reduced to a finite outcome. Raw upstream details
/// (including echoed credentials, headers, and configured request parameters)
/// never leave this check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelProbeOutcome {
    Connected,
    Rejected,
    Unavailable,
    TimedOut,
}

impl SessionRuntimeManager {
    pub(crate) async fn probe_model(
        &self,
        target: SourceTarget,
        model: ModelRef,
        source: ModelProbeSource,
    ) -> ModelProbeOutcome {
        let cancellation = CancellationSignal::new();
        let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let owner = self.clone();
            let resolved = tokio::task::spawn_blocking(move || {
                let (layer, _) = owner
                    .configuration
                    .capture_authored_settings(&target, Some(&source))
                    .ok()?;
                let catalog = ModelCatalog::from_authoring(
                    super::configuration::semantics::authored_catalog(&layer?),
                )
                .ok()?;
                let registry =
                    ModelBindingRegistry::new(catalog.resolve(&owner.credentials).ok()?).ok()?;
                registry.resolve(&ModelSelection::of(model)).ok()
            })
            .await;
            let Ok(Some(invocation)) = resolved else {
                return ModelProbeOutcome::Unavailable;
            };
            let mut config = invocation.invocation_config();
            // An authored preset can require a larger output budget, such
            // as Anthropic thinking. Test that request as configured.
            if config.request_params.is_empty() {
                config.max_output_tokens = config.max_output_tokens.min(256);
            }
            let request = ModelRequest {
                response_schema: None,
                response_byte_limit: Some(256 * 1024),
                images: Default::default(),
                invocation: config,
                messages: vec![ModelInputMessage::Canonical(MessageBlock::User(
                    UserMessageBlock {
                        id: MessageId::new("model-probe"),
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "Reply with OK.".into(),
                        })],
                        source: UserSource::Human,
                        kind: InboundKind::Message,
                        timestamp: None,
                    },
                ))]
                .into(),
                tools: Vec::new(),
                effective_system_prompt: String::new(),
                continuation: None,
                continuation_source: None,
            };
            let mut stream = invocation.adapter().stream(request, cancellation.clone());
            while let Some(item) = stream.next().await {
                match item {
                    ModelStreamItem::Event(ModelEvent::Completed { .. }) => {
                        return ModelProbeOutcome::Connected;
                    }
                    ModelStreamItem::Event(ModelEvent::Failed { .. }) => break,
                    _ => {}
                }
            }
            ModelProbeOutcome::Rejected
        })
        .await;
        result.unwrap_or_else(|_| {
            cancellation.cancel();
            ModelProbeOutcome::TimedOut
        })
    }
}
