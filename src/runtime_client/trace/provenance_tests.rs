use super::{TraceInputProvenance, TraceMessageRole};
use crate::durable::SqliteConversationStore;
use crate::durable::{CanonicalStore, ObservationStore};
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::ModelInputMessage;
use crate::model::projected::{
    CanonicalMessageRef, CheckpointSourceRef, DerivedModelMessage, ProjectedMessage,
    ProjectedUserKind, SemanticTransformation,
};
use crate::runtime::identity::{CheckpointId, ConversationId, MessageId};

fn original(scope: &ConversationId, id: &str) -> CanonicalMessageRef {
    CanonicalMessageRef {
        conversation_id: scope.clone(),
        message_id: MessageId::new(id),
        content_digest: "a".repeat(64),
    }
}

#[test]
fn giant_original_provenance_retains_scope_digest_and_exact_canonical_metadata_source() {
    let scope = ConversationId::generate();
    let native = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("original🙂".repeat(9000)),
        content: Vec::new(),
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    let ancestor = ConversationId::generate();
    assert_ne!(ancestor, scope);
    let frozen = crate::model::projected::FrozenModelInput::Original(CanonicalMessageRef::of(
        &ancestor, &native,
    ));
    let crate::model::projected::FrozenModelInput::Original(authority) = frozen else {
        unreachable!()
    };
    let store = SqliteConversationStore::in_memory(scope.clone()).unwrap();
    store.initialize(std::slice::from_ref(&native)).unwrap();
    let header = store.message_header(native.id()).unwrap().unwrap();
    assert!(
        authority.matches(&native),
        "native reconstruction verifies the exact original"
    );
    let mut view = super::request::request_message_with_original(
        &ModelInputMessage::Canonical(native.clone()),
        &scope,
        Some(&authority),
    );
    super::request::bind_original_provenance(&mut view, &header).unwrap();
    assert_eq!(view.role, TraceMessageRole::User);
    assert!(view.message_id.is_none());
    assert!(view.truncated);
    assert!(crate::runtime_client::checkpoint::bounded_bytes(&view, 4096).is_some());
    let TraceInputProvenance::Original {
        source,
        conversation_id,
        content_digest,
        locator,
        message_id_source,
    } = view.provenance
    else {
        panic!("original provenance")
    };
    assert!(source.is_none());
    assert_eq!(conversation_id, ancestor);
    assert_eq!(conversation_id, authority.conversation_id);
    assert_eq!(content_digest, authority.content_digest);
    assert_eq!(
        locator,
        Some(
            crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
                position: 1
            }
        )
    );
    let binding = message_id_source
        .as_deref()
        .expect("original identity remains explicitly readable");
    assert_eq!(binding.source_path, Some(vec!["id".into()]));
    assert!(binding.deferred);
    assert!(matches!(
        binding.content_ref.source,
        crate::runtime_client::content::ContentSource::CanonicalMessageBlock {
            component: crate::runtime_client::content::CanonicalContentComponent::MessageMetadata,
            ..
        }
    ));
    assert_eq!(
        store
            .load_messages(std::slice::from_ref(native.id()))
            .unwrap(),
        [native]
    );
}

#[test]
fn derived_provenance_admits_exact_byte_bounded_prefixes_without_copying_giant_aliases() {
    let scope = ConversationId::generate();
    let first = original(&scope, "first-original");
    let huge = original(&scope, &"giant-original".repeat(7000));
    let checkpoint = CheckpointSourceRef {
        conversation_id: scope.clone(),
        checkpoint_id: CheckpointId::new("first-checkpoint"),
        content_digest: "b".repeat(64),
    };
    let derived = DerivedModelMessage {
        message: ProjectedMessage::User {
            content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                text: "summary".into(),
            })],
            source: UserSource::Runtime,
            kind: ProjectedUserKind::CompactionContext,
            timestamp: None,
        },
        sources: vec![first.clone(), huge.clone(), first.clone()],
        checkpoint_sources: vec![
            checkpoint.clone(),
            CheckpointSourceRef {
                checkpoint_id: CheckpointId::new("giant-checkpoint".repeat(7000)),
                ..checkpoint.clone()
            },
            checkpoint.clone(),
        ],
        transformations: vec![SemanticTransformation::Summary; 1000],
    };
    let view =
        super::request::request_message(&ModelInputMessage::Derived(derived.clone()), &scope);
    assert!(view.truncated);
    assert!(view.message_id.is_none());
    assert!(view.source.as_ref().unwrap().len() < 512);
    assert!(crate::runtime_client::checkpoint::bounded_bytes(&view, 8192).is_some());
    let TraceInputProvenance::Derived {
        sources,
        checkpoint_sources,
        transformations,
        truncated,
    } = view.provenance
    else {
        panic!("derived provenance")
    };
    assert!(truncated);
    assert_eq!(sources, [first]);
    assert_eq!(checkpoint_sources, [checkpoint]);
    assert_eq!(transformations, vec![SemanticTransformation::Summary; 16]);
    assert_eq!(
        derived.sources[1], huge,
        "native source evidence is unchanged"
    );
    assert_eq!(
        derived.checkpoint_sources[1].checkpoint_id.as_str().len(),
        "giant-checkpoint".len() * 7000
    );
}
