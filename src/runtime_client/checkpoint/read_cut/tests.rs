use super::*;
use crate::runtime::identity::{ConversationId, ObservationCollectionCut};
use crate::runtime_client::{preflight, structural_json};
use std::collections::BTreeMap;

fn cut() -> CheckpointReadCut {
    CheckpointReadCut {
        canonical_foreground: None,
        live_publication: None,
        conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        activation_id: None,
        revision: 1,
        through: 2,
        source_cut: None,
        domain_revisions: super::super::MetadataFragment::default(),
        collection_cuts: super::super::MetadataFragment::default(),
    }
}
#[derive(Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlainCut<'a> {
    canonical_foreground: &'a Option<super::super::CanonicalForegroundReadCut>,
    live_publication: &'a Option<super::super::LivePublicationReadCut>,
    conversation_id: &'a ConversationId,
    activation_id: &'a Option<crate::runtime::identity::SubagentId>,
    revision: u64,
    through: u64,
    /// Immutable durable descriptor frontier captured with this display cut.
    source_cut: &'a Option<crate::durable::observation::ObservationReadCut>,
    domain_revisions: BTreeMap<String, u64>,
    collection_cuts: BTreeMap<String, ObservationCollectionCut>,
}
fn plain(cut: &CheckpointReadCut) -> PlainCut<'_> {
    let CheckpointReadCut {
        canonical_foreground,
        live_publication,
        conversation_id,
        activation_id,
        revision,
        through,
        source_cut,
        domain_revisions,
        collection_cuts,
    } = cut;
    PlainCut {
        canonical_foreground,
        live_publication,
        conversation_id,
        activation_id,
        revision: *revision,
        through: *through,
        source_cut,
        domain_revisions: domain_revisions
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect(),
        collection_cuts: collection_cuts
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect(),
    }
}
fn wire<T: Serialize + schemars::JsonSchema>(value: &T) -> Option<Vec<u8>> {
    let root = structural_json::root::<T>();
    serde_json::to_vec(&structural_json::Encoded {
        value,
        schema: &root,
        root: &root,
    })
    .ok()
}
fn check(cut: &CheckpointReadCut) {
    let original = plain(cut);
    assert_eq!(
        serde_json::to_vec(cut).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    assert_eq!(wire(cut), wire(&original));
    let size = cut.size();
    let budget = preflight::required_safe_integer_budget(&original, CHECKPOINT_TARGET_BYTES);
    assert_eq!(size.budget, budget);
    assert_eq!(
        size.exact,
        super::super::bounded_structural_len_with_budget(
            &original,
            CHECKPOINT_TARGET_BYTES,
            budget
        )
    );
    if let Some(bytes) = wire(cut).filter(|_| size.exact.is_some()) {
        let decoded: CheckpointReadCut =
            structural_json::decode(serde_json::from_slice(&bytes).unwrap()).unwrap();
        assert_eq!(&decoded, cut);
    }
}
#[test]
fn read_cut_shared_maps_preserve_plain_wire_schema_and_composed_size() {
    let mut value = cut();
    check(&value);
    for quantity in [0, 1, 127, 128, 9_007_199_254_740_991, u64::MAX] {
        value.revision = quantity;
        value.through = quantity;
        value
            .domain_revisions
            .insert(format!("domain-水\"\\-{quantity}"), quantity);
        value.collection_cuts.insert(
            format!("collection-{quantity}"),
            ObservationCollectionCut {
                generation: quantity,
                total: quantity,
            },
        );
        check(&value);
        let mut headed = value.clone();
        let locator = crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
            message_id: crate::runtime::identity::MessageId::new("header-水\"\\"),
        };
        headed.canonical_foreground = Some(super::super::CanonicalForegroundReadCut {
            locator: locator.clone(),
            total: quantity,
        });
        headed.live_publication = Some(super::super::LivePublicationReadCut {
            source: crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
                component: crate::runtime_client::content::CanonicalContentComponent::Payload,
                locator,
                block_index: crate::message::types::ContentBlockIndex::new(0),
            },
            revision: quantity,
            block_total: quantity,
            tool_total: quantity,
        });
        headed.activation_id = Some(crate::runtime::identity::SubagentId::new("activation"));
        headed.source_cut = Some(crate::durable::observation::ObservationReadCut {
            read: crate::durable::reading::ConversationReadCut {
                conversation_id: headed.conversation_id.clone(),
                journal: quantity,
                transcript: quantity,
                mutation_revision: quantity,
                ledger_through: quantity,
                surface_revision: crate::conversation::SurfaceRevision::new(quantity),
                transcript_total: quantity,
                pending_through: quantity,
                pending_total: quantity,
            },
            ledger_through: quantity,
            transcript_total: quantity,
            surface_revision: crate::conversation::SurfaceRevision::new(quantity),
            inherited_through: crate::durable::TranscriptCursor::new(quantity),
        });
        check(&headed);
    }
    let mut current = serde_json::to_value(schemars::schema_for!(CheckpointReadCut)).unwrap();
    let mut previous = serde_json::to_value(schemars::schema_for!(PlainCut)).unwrap();
    current.as_object_mut().unwrap().remove("title");
    previous.as_object_mut().unwrap().remove("title");
    assert_eq!(current, previous);
}
#[test]
fn read_cut_map_certificates_reuse_unchanged_roots_and_invalidate_mutation() {
    use super::super::metadata_fragment::take_work;
    let mut value = cut();
    value.domain_revisions.insert("a".into(), 1);
    take_work();
    let before = value.size();
    assert_eq!(take_work(), 2);
    let original = value.clone();
    let wire_before = wire(&original);
    value.revision += 1;
    value.through += 1;
    value.size();
    assert_eq!(
        take_work(),
        0,
        "changing only the cut header does not rewalk either map"
    );
    value.domain_revisions.insert("b".into(), u64::MAX);
    value.size();
    assert_eq!(take_work(), 1, "only the changed map is measured again");
    assert_eq!(original.size(), before);
    assert_eq!(wire(&original), wire_before);
    check(&value);
    check(&original);
    drop(original);
    value.collection_cuts.insert(
        "new".into(),
        ObservationCollectionCut {
            generation: 3,
            total: 4,
        },
    );
    value.size();
    take_work();
    value.collection_cuts.insert(
        "new".into(),
        ObservationCollectionCut {
            generation: 5,
            total: 6,
        },
    );
    value.size();
    assert_eq!(
        take_work(),
        1,
        "a unique owner also invalidates its old certificate"
    );
    check(&value);
}
#[test]
fn read_cut_map_exact_size_keeps_original_large_key_and_encoded_budget_boundaries() {
    for length in [
        0,
        127,
        16_384,
        CHECKPOINT_TARGET_BYTES - 512,
        CHECKPOINT_TARGET_BYTES,
        CHECKPOINT_TARGET_BYTES + 1,
    ] {
        let mut value = cut();
        value.domain_revisions.insert("x".repeat(length), 7);
        check(&value);
    }
    let json = serde_json::json!({"canonical_foreground":null,"live_publication":null,
        "conversation_id":"conv_36524fd8-f674-7fc2-8125-06d01fee0e18","activation_id":null,"revision":0,"through":0,
        "source_cut":null,"domain_revisions":{},"collection_cuts":{},"unknown":true});
    assert!(serde_json::from_value::<CheckpointReadCut>(json).is_err());
    let source = r#"{"canonical_foreground":null,"live_publication":null,"conversation_id":"conv_36524fd8-f674-7fc2-8125-06d01fee0e18","activation_id":null,"revision":0,"through":0,"source_cut":null,"domain_revisions":{"z":1,"a":2,"z":3},"collection_cuts":{}}"#;
    let decoded: CheckpointReadCut = serde_json::from_str(source).unwrap();
    assert_eq!(decoded.domain_revisions.get("z"), Some(&3));
    check(&decoded);
}
