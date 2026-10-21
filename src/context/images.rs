//! Image accounting owner regressions. Production media preparation lives in
//! `model::media` and `ContextEngine`, shared with actual request staging.
#[cfg(test)]
mod tests {
    use crate::context::{
        engine::{ContextConfig, ContextEngine},
        tokens::{DefaultTokenEstimator, ObservedAnchor},
    };
    use crate::durable::CanonicalStore;
    use crate::durable::SqliteConversationStore;
    use crate::message::types::{MessageBlock, UserContentBlock};
    use crate::message::{
        content::ImageReference,
        types::{InboundKind, UserMessageBlock, UserSource},
    };
    use crate::model::input::canonical_input;
    use crate::runtime::identity::{ConversationId, MessageId};
    use crate::tools::artifacts::ArtifactStore;
    use std::sync::Arc;

    fn image_engine(
        artifacts: crate::tools::artifacts::ArtifactStore,
        enabled: bool,
    ) -> ContextEngine {
        let mut engine = ContextEngine::new(
            ContextConfig {
                context_window_tokens: 100_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(DefaultTokenEstimator),
        )
        .unwrap();
        engine.set_image_projection(artifacts, enabled);
        engine
    }

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
            let estimator = image_engine(artifacts.clone(), true);
            let cost = estimator.estimate_model_input(&canonical_input(&canonical).into(), "", &[]);
            let plain = image_engine(artifacts.clone(), false);
            assert!(
                cost > plain.estimate_model_input(&canonical_input(&canonical).into(), "", &[])
                    + 1000
            );
            let database = root.path().join("conversation.sqlite");
            let store = SqliteConversationStore::open(conversation.clone(), &database).unwrap();
            store.append_canonical(&canonical[0]).unwrap();
            drop(store);
            let reopened = SqliteConversationStore::open(conversation, &database).unwrap();
            assert_eq!(
                cost,
                estimator.estimate_model_input(
                    &canonical_input(&reopened.load_canonical().unwrap()).into(),
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
                engine.estimate_model_input(&canonical_input(&canonical).into(), "", &[]),
                cost
            );
            let projection = engine.build_projection_input(
                crate::conversation::SurfaceRevision::new(canonical.len() as u64),
                canonical_input(&canonical).into(),
                &[],
                None,
                "",
            );
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
        let anchor = ObservedAnchor::of_selected(
            crate::conversation::SurfaceRevision::new(16),
            &first.clone().into(),
            "",
            &[],
        );
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
                .covered_prefix_model_input(&projected.clone().into(), "", &[])
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
    #[test]
    fn frozen_image_price_requires_no_live_artifact_read() {
        let root = tempfile::tempdir().unwrap();
        let estimator = image_engine(
            ArtifactStore::new(ConversationId::generate(), root.path()).unwrap(),
            true,
        );
        let id = crate::runtime::identity::ArtifactId::new("missing-live-artifact");
        let mut request = crate::scripted_suites::common::simple_request(
            crate::model::ModelProtocol::OpenAiChatCompletions,
            "test",
            "",
        );
        request.messages = canonical_input(&[image(id.clone(), 0)]).into();
        request.images.insert(id, png(1, 1));
        assert_eq!(
            estimator.estimate_model_input(&request.messages, "", &[]),
            u64::MAX
        );
        let frozen = estimator.estimate_frozen_request(&request);
        assert!(frozen > 1_000 && frozen < u64::MAX);
    }
}
