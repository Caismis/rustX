//! Attempt-frozen image projection and provider-neutral fit accounting.
use crate::context::tokens::TokenEstimator;
use crate::message::types::{MessageBlock, UserContentBlock};
use crate::model::input::{ModelInputMessage, canonical_input};
use crate::tools::{
    artifacts::ArtifactStore,
    types::{ModelToolDefinition, ToolResultContent},
};
use std::sync::Arc;

pub(super) struct ImageEstimator {
    pub inner: Arc<dyn TokenEstimator>,
    pub artifacts: ArtifactStore,
    pub enabled: bool,
}
impl ImageEstimator {
    fn projected(&self, messages: &[ModelInputMessage]) -> (Vec<ModelInputMessage>, u64) {
        let mut rendered = messages.to_vec();
        crate::model::images::project(&mut rendered, self.enabled);
        let images = crate::model::images::resolve(&rendered, |id| {
            self.artifacts.read_bounded(id).map_err(|e| e.to_string())
        });
        let Ok(images) = images else {
            return (rendered, u64::MAX);
        };
        let mut costs = std::collections::BTreeMap::new();
        for (id, bytes) in images {
            let Ok(measurement) = crate::model::images::measure_png(&bytes) else {
                return (rendered, u64::MAX);
            };
            costs.insert(id, measurement.input_tokens());
        }
        let mut cost = 0_u64;
        for message in &rendered {
            match message.as_canonical() {
                Some(MessageBlock::User(user)) => {
                    for block in &user.content {
                        if let UserContentBlock::Image(image) = block {
                            cost = cost.saturating_add(costs[&image.artifact_id]);
                        }
                    }
                }
                Some(MessageBlock::Tool(tool)) => {
                    for block in &tool.result.content {
                        if let ToolResultContent::Image(image) = block {
                            cost = cost.saturating_add(costs[&image.artifact_id]);
                        }
                    }
                }
                _ => {}
            }
        }
        (rendered, cost)
    }
}
impl TokenEstimator for ImageEstimator {
    fn estimate_input(
        &self,
        messages: &[ModelInputMessage],
        prompt: &str,
        tools: &[ModelToolDefinition],
    ) -> u64 {
        let (rendered, cost) = self.projected(messages);
        self.inner
            .estimate_input(&rendered, prompt, tools)
            .saturating_add(cost)
    }
    fn estimate_conversation_input(&self, messages: &[MessageBlock]) -> u64 {
        let (rendered, cost) = self.projected(&canonical_input(messages));
        let canonical = rendered
            .into_iter()
            .filter_map(|m| match m {
                ModelInputMessage::Canonical(m) => Some(m),
                ModelInputMessage::RequestOnly(_) => None,
            })
            .collect::<Vec<_>>();
        self.inner
            .estimate_conversation_input(&canonical)
            .saturating_add(cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{
        engine::{ContextConfig, ContextEngine},
        tokens::{DefaultTokenEstimator, ObservedAnchor},
    };
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::message::{
        content::ImageReference,
        types::{InboundKind, UserMessageBlock, UserSource},
    };
    use crate::runtime::identity::{ConversationId, MessageId};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        png::Encoder::new(&mut bytes, width, height)
            .write_header()
            .unwrap()
            .write_image_data(&vec![0; (width * height) as usize])
            .unwrap();
        bytes
    }
    fn image(id: crate::runtime::ArtifactId, index: usize) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(format!("image-{index}")),
            content: vec![UserContentBlock::Image(ImageReference {
                artifact_id: id,
                alt: None,
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }
    #[test]
    fn image_dimensions_change_fit_cost_and_sqlite_replay_is_deterministic() {
        let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let mut costs = Vec::new();
        for (root, size) in roots.iter().zip([1, 2048]) {
            let conversation = ConversationId::generate();
            let artifacts = ArtifactStore::new(conversation.clone(), root.path()).unwrap();
            let id = artifacts.put_bounded(&png(size, size)).unwrap();
            let canonical = vec![image(id, 0)];
            let estimator = ImageEstimator {
                inner: Arc::new(DefaultTokenEstimator),
                artifacts: artifacts.clone(),
                enabled: true,
            };
            let cost = estimator.estimate_input(&canonical_input(&canonical), "", &[]);
            let plain = ImageEstimator {
                inner: Arc::new(DefaultTokenEstimator),
                artifacts: artifacts.clone(),
                enabled: false,
            };
            assert!(cost > plain.estimate_input(&canonical_input(&canonical), "", &[]) + 1000);
            let database = root.path().join("conversation.sqlite");
            let store = SqliteConversationStore::open(conversation.clone(), &database).unwrap();
            store.append_canonical(&canonical[0]).unwrap();
            drop(store);
            let reopened = SqliteConversationStore::open(conversation, &database).unwrap();
            assert_eq!(
                cost,
                estimator.estimate_input(
                    &canonical_input(&reopened.load_canonical().unwrap()),
                    "",
                    &[]
                )
            );
            let mut engine = ContextEngine::new(
                ContextConfig {
                    context_window_tokens: 4096,
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                },
                Arc::new(DefaultTokenEstimator),
            )
            .unwrap();
            engine.set_image_projection(artifacts, true);
            assert_eq!(
                engine.estimate_model_input(&canonical_input(&canonical), "", &[]),
                cost
            );
            let state = crate::conversation::ConversationState::from_messages(canonical).unwrap();
            let projection = engine.build_projection(&state, &[], None, "").unwrap();
            assert_eq!(projection.estimated_input.input_tokens, cost);
            assert_eq!(engine.should_compact(&projection, 1).unwrap(), size == 2048);
            assert_eq!(
                engine.fits_under_soft_limit(&projection, 1).unwrap(),
                size == 1
            );
            costs.push(cost);
        }
        assert!(costs[1] > costs[0] + 16_000);
    }

    #[test]
    fn newest_sixteen_projection_is_replayable_and_invalidates_changed_observed_prefix() {
        let root = tempfile::tempdir().unwrap();
        let conversation = ConversationId::generate();
        let artifacts = ArtifactStore::new(conversation.clone(), root.path()).unwrap();
        let bytes = png(1, 1);
        let mut canonical = Vec::new();
        // Same captured bytes, distinct native artifact identities.
        for index in 0..17 {
            canonical.push(image(artifacts.put_bounded(&bytes).unwrap(), index));
        }
        let original = canonical.clone();
        let mut first = canonical_input(&canonical[..16]);
        crate::model::images::project(&mut first, true);
        assert_eq!(
            crate::model::images::resolve(&first, |id| artifacts
                .read_bounded(id)
                .map_err(|e| e.to_string()))
            .unwrap()
            .len(),
            16
        );
        let anchor = ObservedAnchor::of_model_input(&first, "", &[]);
        let mut projected = canonical_input(&canonical);
        crate::model::images::project(&mut projected, true);
        let resolved = crate::model::images::resolve(&projected, |id| {
            artifacts.read_bounded(id).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(resolved.len(), 16);
        assert!(!resolved.contains_key(&crate::runtime::ArtifactId::new("artifact_1")));
        assert!(
            anchor
                .covered_prefix_model_input(&projected, "", &[])
                .is_none()
        );
        let mut text = canonical_input(&canonical);
        crate::model::images::project(&mut text, false);
        assert!(
            crate::model::images::resolve(&text, |_| panic!("text requests never read images"))
                .unwrap()
                .is_empty()
        );
        let database = root.path().join("conversation.sqlite");
        let store = SqliteConversationStore::open(conversation.clone(), &database).unwrap();
        for message in &canonical {
            store.append_canonical(message).unwrap();
        }
        drop(store);
        let reopened = SqliteConversationStore::open(conversation, &database).unwrap();
        let mut restored = canonical_input(&reopened.load_canonical().unwrap());
        crate::model::images::project(&mut restored, true);
        assert_eq!(restored, projected);
        assert_eq!(canonical, original);
        let repeated = vec![canonical[0].clone(); 17];
        let mut repeated_input = canonical_input(&repeated);
        assert!(crate::model::images::resolve(&repeated_input, |_| Ok(bytes.clone())).is_err());
        crate::model::images::project(&mut repeated_input, true);
        assert_eq!(crate::model::images::image_count(&repeated_input), 16);
        assert_eq!(
            crate::model::images::resolve(&repeated_input, |_| Ok(bytes.clone()))
                .unwrap()
                .len(),
            1
        );

        // A later selected cut can render the oldest captured snapshot again.
        assert_eq!(
            crate::model::images::resolve(&first, |id| artifacts
                .read_bounded(id)
                .map_err(|e| e.to_string()))
            .unwrap()[&crate::runtime::ArtifactId::new("artifact_1")],
            bytes
        );
    }
}
