use super::*;
use crate::message::content::TextBlock;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::input::{
    CarryoverOmissionCounts, RenderedUnresolvedOutputCarryover, RequestOnlyModelContext,
    UnresolvedOutputSettlement,
};
use crate::model::projected::{
    CheckpointSourceRef, DerivedModelMessage, ModelMessageView, ProjectedMessage,
    SemanticTransformation,
};
use crate::runtime::identity::{CheckpointId, MessageId, PublicationStreamId};

fn user(index: usize) -> ModelInputMessage {
    ModelInputMessage::Canonical(MessageBlock::User(UserMessageBlock {
        id: MessageId::new(format!("selected-{index}")),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!("input {index}: 水 \\\" {{\"raw\":18446744073709551615}}\n"),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    }))
}

fn mixed(scope: &ConversationId) -> Vec<ModelInputMessage> {
    let original = user(1);
    let source = original.as_canonical().unwrap();
    let derived = ModelInputMessage::Derived(DerivedModelMessage {
        message: ProjectedMessage::from(source),
        sources: vec![CanonicalMessageRef::of(scope, source)],
        checkpoint_sources: vec![CheckpointSourceRef {
            conversation_id: scope.clone(),
            checkpoint_id: CheckpointId::new("original-checkpoint"),
            content_digest: "ab".repeat(32),
        }],
        transformations: vec![
            SemanticTransformation::Summary,
            SemanticTransformation::ImageDowngrade,
        ],
    });
    let request_only = ModelInputMessage::RequestOnly(
        RequestOnlyModelContext::UnresolvedOutputCarryover(RenderedUnresolvedOutputCarryover {
            source_stream_id: PublicationStreamId::new("original-unaccepted-stream"),
            source_settlement: UnresolvedOutputSettlement::Unaccepted,
            records: Vec::new(),
            omitted_blocks: CarryoverOmissionCounts::default(),
        }),
    );
    vec![
        original.clone(),
        derived.clone(),
        request_only,
        original,
        derived,
    ]
}

// This oracle retains the original flat representation, independently of the
// tree's producer freeze function and unfinished hash state.
fn flat_frozen(scope: &ConversationId, messages: &[ModelInputMessage]) -> Vec<FrozenModelInput> {
    messages
        .iter()
        .map(|message| match message {
            ModelInputMessage::Canonical(message) => {
                let bytes = serde_json::to_vec(message).unwrap();
                FrozenModelInput::Original(CanonicalMessageRef {
                    conversation_id: scope.clone(),
                    message_id: message.id().clone(),
                    content_digest: format!("{:x}", Sha256::digest(bytes)),
                })
            }
            ModelInputMessage::Derived(message) => FrozenModelInput::Derived(message.clone()),
            ModelInputMessage::RequestOnly(message) => {
                FrozenModelInput::RequestOnly(message.clone())
            }
        })
        .collect()
}

fn flat_semantic(messages: &[ModelInputMessage]) -> String {
    let mut digest = Sha256::new();
    for message in messages {
        let value = match message.view() {
            ModelMessageView::User(content) => serde_json::json!({"role":"user","content":content}),
            ModelMessageView::Assistant(content) => {
                serde_json::json!({"role":"assistant","content":content})
            }
            ModelMessageView::Tool(call_id, result) => match result.model_visible_content() {
                Ok(crate::tools::types::ModelToolResultContent::Parts(parts)) => {
                    serde_json::json!({"role":"tool","call_id":call_id,"parts":parts})
                }
                Ok(crate::tools::types::ModelToolResultContent::Text(text)) => {
                    serde_json::json!({"role":"tool","call_id":call_id,"text":text,"media":[]})
                }
                Err(_) => {
                    serde_json::json!({"role":"tool","call_id":call_id,"unsupported_content":true})
                }
            },
            ModelMessageView::RequestOnly(context) => {
                serde_json::json!({"role":"user","text":context.render()})
            }
        };
        digest.update(serde_json::to_vec(&value).unwrap());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

fn assert_avl(node: &InputNode) -> (usize, u8) {
    let (count, height) = match &node.kind {
        InputNodeKind::Leaf(_) => (1, 1),
        InputNodeKind::Branch { left, right } => {
            let (left_count, left_height) = assert_avl(left);
            let (right_count, right_height) = assert_avl(right);
            assert!(left_height.abs_diff(right_height) <= 1);
            (left_count + right_count, 1 + left_height.max(right_height))
        }
    };
    assert_eq!((node.count, node.height), (count, height));
    (count, height)
}

fn assert_flat(
    scope: &ConversationId,
    selected: &SelectedInputManifest,
    flat: &[ModelInputMessage],
) {
    assert_eq!(selected.len(), flat.len());
    assert_eq!(selected.iter().len(), flat.len());
    assert_eq!(
        serde_json::to_vec(selected).unwrap(),
        serde_json::to_vec(&flat_frozen(scope, flat)).unwrap()
    );
    assert_eq!(selected.messages().to_vec(), flat);
    assert_eq!(selected.messages().iter().len(), flat.len());
    if let Some(digest) = selected.semantic_digest() {
        assert_eq!(digest, flat_semantic(flat));
    }
    selected.validate_budget().unwrap();
    let mut request = crate::scripted_suites::common::simple_request(
        crate::model::ModelProtocol::OpenAiChatCompletions,
        "test",
        "selected",
    );
    request.messages = flat.to_vec().into();
    let mut snapshot = empty_snapshot(&request);
    snapshot
        .freeze_selected_input(scope, &request, selected)
        .unwrap();
    let fingerprint_bytes = serde_json::to_vec(&(
        flat_semantic(flat),
        &request.images,
        &request.effective_system_prompt,
        &request.tools,
        &request.invocation,
        &request.continuation,
        &request.response_schema,
    ))
    .unwrap();
    assert_eq!(
        snapshot.semantic_input_fingerprint,
        format!("{:x}", Sha256::digest(fingerprint_bytes))
    );
    assert_eq!(
        serde_json::to_vec(&snapshot.input).unwrap(),
        serde_json::to_vec(&flat_frozen(scope, flat)).unwrap()
    );
    if let Some(root) = &selected.root {
        assert_eq!(assert_avl(root).0, flat.len());
    }
}

#[test]
fn persistent_selected_input_matches_flat_serde_sha_and_exact_provenance_after_edits() {
    let scope = ConversationId::generate();
    let mut flat = mixed(&scope);
    let mut selected = SelectedInputManifest::from_messages(&scope, &flat);
    assert_flat(&scope, &selected, &flat);
    assert_eq!(selected.get(0), selected.get(3));
    assert_eq!(selected.get(1), selected.get(4));
    let old = selected.clone();
    let old_bytes = serde_json::to_vec(&old).unwrap();
    let old_digest = old.semantic_digest();
    for step in 0..96 {
        let message = mixed(&scope).remove(step % 5);
        match step % 3 {
            0 => {
                selected = selected.append(&scope, &message);
                flat.push(message);
            }
            1 => {
                let index = step % flat.len();
                selected = selected.replace(&scope, index, &message);
                flat[index] = message;
                assert!(selected.semantic_digest().is_none());
            }
            _ => {
                let index = step % (flat.len() + 1);
                selected = selected.insert(&scope, index, &message);
                flat.insert(index, message);
            }
        }
        assert_flat(&scope, &selected, &flat);
    }
    assert_eq!(serde_json::to_vec(&old).unwrap(), old_bytes);
    assert_eq!(old.semantic_digest(), old_digest);
    assert_flat(&scope, &old, &mixed(&scope));
}

fn assert_shared_leaves(
    original: &SelectedInputManifest,
    changed: &SelectedInputManifest,
    skip: Option<usize>,
    inserted: Option<usize>,
) {
    for index in 0..original.len() {
        if Some(index) == skip {
            continue;
        }
        let changed_index = index + usize::from(inserted.is_some_and(|at| index >= at));
        assert!(Arc::ptr_eq(
            node_at(original.root.as_ref().unwrap(), index),
            node_at(changed.root.as_ref().unwrap(), changed_index)
        ));
    }
}

fn assert_delta_work(work: (usize, usize, usize, usize), height: u8) {
    assert_eq!(work.0, 1, "only the new occurrence allocates a leaf");
    assert_eq!(work.2, 0, "an edit must not copy an old occurrence body");
    assert!(
        work.1 + work.3 <= 8 * usize::from(height) + 8,
        "path work: {work:?}, height {height}"
    );
}

#[test]
fn selected_append_insert_replace_share_old_leaves_with_logarithmic_actual_node_work() {
    let scope = ConversationId::generate();
    let message = user(90_000);
    for count in [100, 300, 1000, 8191] {
        let flat: Vec<_> = (0..count).map(user).collect();
        let old = SelectedInputManifest::from_messages(&scope, &flat);
        let old_bytes = serde_json::to_vec(&old).unwrap();
        let height = old.root.as_ref().unwrap().height;
        take_node_work();
        take_hash_work();
        let cloned = old.clone();
        assert_eq!(take_node_work(), (0, 0, 0, 0));
        assert_eq!(take_hash_work(), (0, 0));
        assert!(Arc::ptr_eq(
            old.root.as_ref().unwrap(),
            cloned.root.as_ref().unwrap()
        ));
        let appended = old.append(&scope, &message);
        assert_delta_work(take_node_work(), height);
        let hash = take_hash_work();
        assert_eq!(
            hash.0,
            serde_json::to_vec(message.as_canonical().unwrap())
                .unwrap()
                .len()
        );
        assert!(
            hash.1 > 0 && hash.1 < 512,
            "only the appended semantic bytes: {hash:?}"
        );
        assert_shared_leaves(&old, &appended, None, None);
        assert_eq!(
            appended.semantic_digest().unwrap(),
            flat_semantic(
                &flat
                    .iter()
                    .cloned()
                    .chain([message.clone()])
                    .collect::<Vec<_>>()
            )
        );
        take_node_work();
        let replaced = old.replace(&scope, count / 2, &message);
        assert_delta_work(take_node_work(), height);
        assert_shared_leaves(&old, &replaced, Some(count / 2), None);
        take_node_work();
        let inserted = old.insert(&scope, count / 2, &message);
        assert_delta_work(take_node_work(), height);
        assert_shared_leaves(&old, &inserted, None, Some(count / 2));
        assert_avl(appended.root.as_ref().unwrap());
        assert_avl(replaced.root.as_ref().unwrap());
        assert_avl(inserted.root.as_ref().unwrap());
        assert_eq!(serde_json::to_vec(&old).unwrap(), old_bytes);
    }
}

fn certify(selected: &mut SelectedInputManifest) {
    selected.certify_selection(
        1,
        2,
        crate::durable::projection::RequestHistoryCut {
            ledger: crate::context::resources::LedgerFrontier(3),
            checkpoint_id: None,
            activation: crate::context::checkpoint::ProjectionActivationId(0),
            base_frontier: crate::context::resources::LedgerFrontier(0),
        },
    );
}

fn seed_certificates(node: &InputNode) {
    node.digest.set([7; 32]).unwrap();
    assert_eq!(*node.budget.get_or_init(|| Ok((0, 0))), Ok((0, 0)));
    *node.stored.lock().unwrap() = Some(StoredInputNode {
        owner: 1,
        epoch: 2,
        id: 3,
        digest: [7; 32],
    });
    if let InputNodeKind::Branch { left, right } = &node.kind {
        seed_certificates(left);
        seed_certificates(right);
    }
}

fn assert_changed_path_uncertified(node: &InputNode, index: usize) {
    assert!(node.digest.get().is_none());
    assert!(node.budget.get().is_none());
    assert!(node.stored.lock().unwrap().is_none());
    assert!(node.actual.is_none());
    if let InputNodeKind::Branch { left, right } = &node.kind {
        if index < left.count {
            assert_changed_path_uncertified(left, index);
        } else {
            assert_changed_path_uncertified(right, index - left.count);
        }
    }
}

#[test]
fn direct_frozen_mutation_invalidates_producer_and_path_certificates_without_touching_old_root() {
    let scope = ConversationId::generate();
    let flat: Vec<_> = (0..100).map(user).collect();
    let mut selected = SelectedInputManifest::from_messages(&scope, &flat);
    certify(&mut selected);
    seed_certificates(selected.root.as_ref().unwrap());
    let old = selected.clone();
    let old_bytes = serde_json::to_vec(&old).unwrap();
    selected = selected.append(&scope, &user(101));
    assert_eq!(selected.edits.len(), 1);
    selected[50] = flat_frozen(&scope, &[user(200)]).remove(0);
    assert!(selected.origin.is_none());
    assert!(selected.edits.is_empty());
    assert!(selected.semantic_digest().is_none());
    assert_changed_path_uncertified(selected.root.as_ref().unwrap(), 50);
    assert_shared_leaves(&old, &selected, Some(50), None);
    assert_eq!(serde_json::to_vec(&old).unwrap(), old_bytes);
    assert!(old.root.as_ref().unwrap().digest.get().is_some());
    let mut pushed = old.clone();
    pushed.push(flat_frozen(&scope, &[user(201)]).remove(0));
    assert!(pushed.origin.is_none());
    assert!(pushed.edits.is_empty());
    assert!(pushed.semantic_digest().is_none());
    assert_eq!(serde_json::to_vec(&old).unwrap(), old_bytes);
}

fn empty_snapshot(request: &crate::model::ModelRequest) -> crate::model::RequestSnapshot {
    crate::model::RequestSnapshot::new(
        crate::model::RequestIdentity {
            attempt_id: crate::runtime::identity::AttemptId::new("selected-budget"),
            turn: crate::runtime::identity::TurnId::new("1"),
            retry_number: 0,
        },
        crate::context::resources::LedgerFrontier(1),
        request.effective_system_prompt.clone(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        request.invocation.clone(),
        128_000,
        None,
        false,
        request.tools.clone(),
        crate::runtime::identity::CapabilityRevision::new(1),
        crate::context::assembly::ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    )
}

#[test]
fn cold_freeze_rejects_overbudget_before_constructing_cloning_or_hashing_input() {
    let scope = ConversationId::generate();
    let mut request = crate::scripted_suites::common::simple_request(
        crate::model::ModelProtocol::OpenAiChatCompletions,
        "test",
        "original",
    );
    let mut snapshot = empty_snapshot(&request);
    let before = snapshot.clone();
    let source = request.messages[0].as_canonical().unwrap();
    let derived = DerivedModelMessage {
        message: ProjectedMessage::User {
            source: UserSource::Human,
            kind: crate::model::projected::ProjectedUserKind::Message,
            timestamp: None,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "x".repeat(8 * 1024 * 1024),
            })],
        },
        sources: vec![CanonicalMessageRef::of(&scope, source)],
        checkpoint_sources: Vec::new(),
        transformations: vec![SemanticTransformation::UploadRendering],
    };
    request.messages = vec![ModelInputMessage::Derived(derived)].into();
    take_node_work();
    take_hash_work();
    assert!(
        snapshot
            .freeze_input(&scope, &request)
            .unwrap_err()
            .contains("byte budget")
    );
    assert_eq!(take_node_work(), (0, 0, 0, 0));
    assert_eq!(take_hash_work(), (0, 0));
    assert_eq!(snapshot, before);
    request.messages = vec![user(0); 8193].into();
    take_node_work();
    take_hash_work();
    assert!(snapshot.freeze_input(&scope, &request).is_err());
    assert_eq!(take_node_work(), (0, 0, 0, 0));
    assert_eq!(take_hash_work(), (0, 0));
    assert_eq!(snapshot, before);
}
