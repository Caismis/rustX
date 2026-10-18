use crate::model::SelectedInputManifest;
use crate::model::projected::{CanonicalMessageRef, DerivedModelMessage, ProjectedMessage};

// The previous two-copy algorithm is an independent semantics oracle. It
// deliberately performs its original body comparisons only inside this test.
fn legacy_materialize(snapshot: &MediaSnapshot, messages: &mut [ModelInputMessage]) {
    snapshot.validate_render_budget(messages.iter()).unwrap();
    count_copy(messages);
    let original = messages.to_vec();
    snapshot.uploads.apply(messages).unwrap();
    count_copy(messages);
    let uploaded = messages.to_vec();
    if let Some(enabled) = snapshot.enabled {
        crate::model::images::project(messages, enabled);
    }
    if let Some(scope) = &snapshot.conversation {
        for ((current, before), uploaded) in messages.iter_mut().zip(&original).zip(&uploaded) {
            if let Some(source) = before.as_canonical() {
                let upload = before != uploaded;
                let image = uploaded != current;
                current.mark_transformed(
                    source,
                    scope,
                    if upload {
                        SemanticTransformation::UploadRendering
                    } else {
                        SemanticTransformation::ImageDowngrade
                    },
                );
                if image
                    && let ModelInputMessage::Derived(derived) = current
                    && !derived
                        .transformations
                        .contains(&SemanticTransformation::ImageDowngrade)
                {
                    derived
                        .transformations
                        .push(SemanticTransformation::ImageDowngrade);
                }
            }
        }
    }
}

#[test]
fn selected_media_copy_accounting_distinguishes_passes_items_and_exact_source_bytes() {
    let scope = ConversationId::generate();
    let file = crate::message::content::UploadedFileRef {
        batch_id: "batch".into(),
        name: "source.txt".into(),
    };
    let snapshot = MediaSnapshot {
        uploads: UploadProjection {
            files: vec![crate::model::uploads::ResolvedUpload {
                file: file.clone(),
                path: "/session/source.txt".into(),
            }],
        },
        conversation: Some(scope.clone()),
        ..Default::default()
    };
    let original = [user(0, vec![UserContentBlock::UploadedFile(file)])];
    let manifest = SelectedInputManifest::from_messages(&scope, &original);
    let mut uploaded = original.clone();
    snapshot.uploads.apply(&mut uploaded).unwrap();
    let mut expected = original.clone();
    legacy_materialize(&snapshot, &mut expected);
    let source_bytes = serde_json::to_vec(&original[0]).unwrap().len()
        + serde_json::to_vec(&uploaded[0]).unwrap().len()
        + serde_json::to_vec(&expected[0]).unwrap().len();
    let mut selected = manifest.messages();
    take_work();
    assert_eq!(
        snapshot
            .materialize_selected_with_changes(&mut selected, Some(&manifest))
            .unwrap(),
        vec![0]
    );
    let work = take_work();
    assert_eq!(selected.to_vec().as_slice(), expected.as_slice());
    assert_eq!(
        work.projections, 3,
        "media selection, provenance projection, replacement"
    );
    assert_eq!(work.copied_items, 3);
    assert_eq!(work.copied_bytes, source_bytes);
    assert_eq!(work.original_hashes, 0);
    assert_eq!(work.changed_items, 1);

    // The same three passes over two changed occurrences copy six items.
    let repeated = [original[0].clone(), original[0].clone()];
    let manifest = SelectedInputManifest::from_messages(&scope, &repeated);
    let mut selected = manifest.messages();
    take_work();
    snapshot
        .materialize_selected_with_changes(&mut selected, Some(&manifest))
        .unwrap();
    let work = take_work();
    assert_eq!(work.projections, 3);
    assert_eq!(work.copied_items, 6);
    assert_eq!(work.copied_bytes, 2 * source_bytes);
}

#[test]
fn selected_media_noop_does_not_copy_or_rehash_unchanged_canonical_and_derived_bodies() {
    let scope = ConversationId::generate();
    let mut input: Vec<_> = (0..128)
        .map(|index| {
            user(
                index,
                vec![UserContentBlock::Text(TextBlock {
                    text: "unchanged 水".repeat(512),
                })],
            )
        })
        .collect();
    let original = input[0].as_canonical().unwrap();
    input[0] = ModelInputMessage::Derived(DerivedModelMessage {
        message: ProjectedMessage::from(original),
        sources: vec![CanonicalMessageRef::of(&scope, original)],
        checkpoint_sources: Vec::new(),
        transformations: vec![SemanticTransformation::Summary],
    });
    let selected = SelectedInputManifest::from_messages(&scope, &input);
    let snapshot = MediaSnapshot {
        enabled: Some(true),
        conversation: Some(scope),
        ..Default::default()
    };
    let mut expected = input.clone();
    take_work();
    legacy_materialize(&snapshot, &mut expected);
    assert!(take_work().copied_bytes > 128 * 4096);
    take_work();
    assert!(
        snapshot
            .materialize_with_changes(&mut input, Some(&selected))
            .unwrap()
            .is_empty()
    );
    let work = take_work();
    assert_eq!(work.copied_bytes, 0);
    assert_eq!(work.original_hashes, 0);
    assert_eq!(work.changed_items, 0);
    assert_eq!(input, expected);
}

fn mixed_input(scope: &ConversationId, image_id: &ArtifactId) -> Vec<ModelInputMessage> {
    let mut input = vec![
        user(
            0,
            vec![UserContentBlock::Text(TextBlock {
                text: "unchanged".repeat(1024),
            })],
        ),
        user(1, vec![reference(), image(image_id.clone())]),
        user(2, vec![reference(), image(image_id.clone())]),
    ];
    let source = input[2].as_canonical().unwrap();
    input[2] = ModelInputMessage::Derived(DerivedModelMessage {
        message: ProjectedMessage::from(source),
        sources: vec![CanonicalMessageRef::of(scope, source)],
        checkpoint_sources: Vec::new(),
        transformations: vec![SemanticTransformation::Summary],
    });
    input.extend((3..20).map(|index| user(index, vec![image(image_id.clone())])));
    input
}

#[test]
fn selected_media_deltas_equal_legacy_upload_image_semantics_and_keep_exact_frozen_order() {
    let scope = ConversationId::generate();
    let id = ArtifactId::new("selected-image");
    let original = mixed_input(&scope, &id);
    let uploads = Uploads {
        calls: Arc::default(),
        signal: None,
        path: "/session/file&.txt".into(),
    };
    for enabled in [false, true] {
        let snapshot = MediaSnapshot {
            uploads: uploads.resolve(&original).unwrap(),
            images: [(id.clone(), png())].into(),
            enabled: Some(enabled),
            conversation: Some(scope.clone()),
            ..Default::default()
        };
        let mut expected = original.clone();
        legacy_materialize(&snapshot, &mut expected);
        let expected_changes: Vec<_> = original
            .iter()
            .zip(&expected)
            .enumerate()
            .filter_map(|(index, (before, after))| (before != after).then_some(index))
            .collect();
        let mut actual = original.clone();
        let mut selected = SelectedInputManifest::from_messages(&scope, &actual);
        take_work();
        let changes = snapshot
            .materialize_with_changes(&mut actual, Some(&selected))
            .unwrap();
        assert_eq!(changes, expected_changes);
        assert_eq!(take_work().original_hashes, 0);
        assert_eq!(actual, expected);
        for index in changes {
            selected = selected.replace(&scope, index, &actual[index]);
        }
        assert_eq!(
            serde_json::to_vec(&selected).unwrap(),
            serde_json::to_vec(&SelectedInputManifest::from_messages(&scope, &actual)).unwrap()
        );
        assert!(
            snapshot
                .materialize_with_changes(&mut actual, Some(&selected))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            actual, expected,
            "repeat materialization adds no duplicate transformation"
        );
    }
}

#[test]
fn selected_media_preserves_inherited_original_scope_and_rejects_a_different_selection() {
    let inherited = ConversationId::generate();
    let local = ConversationId::generate();
    let input = vec![user(1, vec![reference()])];
    let selected = SelectedInputManifest::from_messages(&inherited, &input);
    let uploads = Uploads {
        calls: Arc::default(),
        signal: None,
        path: "/session/file.txt".into(),
    };
    let snapshot = MediaSnapshot {
        uploads: uploads.resolve(&input).unwrap(),
        conversation: Some(local),
        ..Default::default()
    };
    let mut actual = input.clone();
    assert_eq!(
        snapshot
            .materialize_with_changes(&mut actual, Some(&selected))
            .unwrap(),
        vec![0]
    );
    let ModelInputMessage::Derived(derived) = &actual[0] else {
        panic!("actual upload rendering is Derived")
    };
    let crate::model::projected::FrozenModelInput::Original(source) = selected.get(0).unwrap()
    else {
        panic!("original source")
    };
    assert_eq!(derived.sources, vec![source.clone()]);
    assert_eq!(derived.sources[0].conversation_id, inherited);
    let wrong = SelectedInputManifest::from_messages(&inherited, &[user(99, Vec::new())]);
    assert!(
        snapshot
            .materialize_with_changes(&mut input.clone(), Some(&wrong))
            .is_err()
    );
}

fn legacy_resource_ids(
    snapshot: &MediaSnapshot,
    historical: &[ModelInputMessage],
    complete: &[ModelInputMessage],
) -> Result<BTreeSet<ArtifactId>, String> {
    let mut ids = BTreeSet::new();
    for input in [historical, complete] {
        snapshot.validate_render_budget(input)?;
        let mut rendered = input.to_vec();
        snapshot.uploads.apply(&mut rendered)?;
        if let Some(enabled) = snapshot.enabled {
            crate::model::images::project(&mut rendered, enabled);
        }
        if snapshot.enabled.is_some() {
            ids.extend(image_ids(&rendered));
        }
    }
    Ok(ids)
}

fn tool_image(index: usize, id: &ArtifactId) -> ModelInputMessage {
    use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef, ToolMessageBlock};
    use crate::runtime::identity::{ToolCallId, ToolId};
    let mut result = crate::scripted_suites::support::fake::success_result("tool result");
    result
        .content
        .push(ToolResultContent::Image(ImageReference {
            artifact_id: id.clone(),
            alt: None,
        }));
    ModelInputMessage::Canonical(MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new(format!("result-{index}")),
        tool_call_id: ToolCallId::new(format!("call-{index}")),
        occurrence: ToolCallOccurrenceRef::new(
            MessageId::new(format!("assistant-{index}")),
            ContentBlockIndex::new(0),
        ),
        tool_id: ToolId::new("image-tool"),
        result,
    }))
}

#[test]
fn borrowed_resource_capture_equals_rendered_oracle_for_occurrences_views_and_errors() {
    let directory = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::new(ConversationId::generate(), directory.path()).unwrap();
    let good = artifacts.put_bounded(&png()).unwrap();
    let corrupt = artifacts.put_bounded(b"not a PNG").unwrap();
    let mut historical = vec![user(0, vec![image(corrupt.clone())])];
    historical.extend((1..=17).map(|index| {
        if index % 2 == 0 {
            tool_image(index, &good)
        } else {
            user(index, vec![image(good.clone())])
        }
    }));
    let mut complete = historical.clone();
    complete.push(user(99, vec![reference(), image(good.clone())]));
    let uploads = Uploads {
        calls: Arc::default(),
        signal: None,
        path: "/session/file.txt".into(),
    };
    let signal = CancellationSignal::new();
    let manifest = SelectedInputManifest::from_messages(artifacts.conversation_id(), &historical);
    for enabled in [None, Some(false), Some(true)] {
        let oracle = MediaSnapshot {
            uploads: uploads.resolve(&complete).unwrap(),
            enabled,
            conversation: Some(artifacts.conversation_id().clone()),
            ..Default::default()
        };
        let expected = legacy_resource_ids(&oracle, &historical, &complete).unwrap();
        let actual = MediaSnapshot::capture(
            None,
            &manifest.messages(),
            &complete,
            Some(&artifacts),
            enabled,
            Some(&uploads),
            &signal,
        )
        .unwrap();
        assert_eq!(
            actual.images.keys().cloned().collect::<BTreeSet<_>>(),
            expected
        );
        for (id, bytes) in &actual.images {
            assert_eq!(*bytes, artifacts.read_bounded(id).unwrap());
        }
        assert!(
            !actual.images.contains_key(&corrupt),
            "oldest image is outside actual occurrence frontier"
        );
    }
    complete.push(user(100, vec![image(corrupt.clone())]));
    let oracle = MediaSnapshot {
        enabled: Some(true),
        ..Default::default()
    };
    assert!(
        legacy_resource_ids(&oracle, &historical, &complete[..complete.len() - 1]).is_err(),
        "the upload still requires its real Session owner"
    );
    assert!(
        MediaSnapshot::capture(
            None,
            &manifest.messages(),
            &complete,
            Some(&artifacts),
            Some(true),
            Some(&uploads),
            &signal
        )
        .is_err(),
        "a newly selected corrupt resource is still rejected"
    );
    let cancelled = CancellationSignal::new();
    cancelled.cancel();
    assert_eq!(
        MediaSnapshot::capture(
            None,
            &manifest.messages(),
            &complete,
            Some(&artifacts),
            Some(true),
            Some(&uploads),
            &cancelled
        )
        .unwrap_err(),
        "media preparation cancelled"
    );
}
