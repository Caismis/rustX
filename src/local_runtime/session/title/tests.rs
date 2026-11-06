use super::*;

fn fixture() -> (tempfile::TempDir, SessionCatalog, SessionId, ConversationId) {
    let (directory, catalog, _) = super::super::tests::open_catalog();
    let id = catalog.persisted_session_ids()[0].clone();
    let conversation = catalog.snapshot(&id).unwrap().active_conversation_id;
    (directory, catalog, id, conversation)
}

fn publish(
    catalog: &mut SessionCatalog,
    id: &SessionId,
    conversation: &ConversationId,
) -> Result<bool, SessionError> {
    catalog.publish_generated_title(
        id,
        conversation,
        &MessageId::new("first-human"),
        &AuxiliaryCallId::parse("aux_36524fd8-f674-7fc2-8125-06d01fee0e18").unwrap(),
        "A bounded title",
    )
}

#[test]
fn publication_is_idempotent_catalog_metadata_and_manual_name_wins() {
    let (_directory, mut catalog, id, conversation) = fixture();
    let before = catalog.summary(&id).unwrap();
    let settings = catalog.settings_revision(&id).unwrap();
    let invalidations = catalog.summary_invalidations();
    let frontier = invalidations.frontier();
    assert!(
        catalog
            .list_page(Some("bounded title"), 0, 32)
            .unwrap()
            .sessions
            .is_empty()
    );
    assert!(publish(&mut catalog, &id, &conversation).unwrap());
    let published = invalidations.frontier();
    assert_eq!(
        invalidations.next_after(frontier),
        Some((published, id.clone(), true)),
        "a new search match invalidates Catalog membership, even when the Session was absent"
    );
    assert!(!publish(&mut catalog, &id, &conversation).unwrap());
    assert_eq!(invalidations.frontier(), published);
    let summary = catalog.summary(&id).unwrap();
    assert_eq!(summary.updated_at, before.updated_at);
    assert_eq!(catalog.settings_revision(&id).unwrap(), settings);
    assert!(invalidations.frontier() > frontier);
    assert_eq!(
        catalog
            .list_page(Some("bounded title"), 0, 32)
            .unwrap()
            .sessions
            .len(),
        1
    );
    catalog.rename(&id, "Manual name").unwrap();
    assert_eq!(
        catalog.snapshot(&id).unwrap().name.as_deref(),
        Some("Manual name")
    );
    assert!(!catalog.title_eligible(&id, &conversation));
    assert_eq!(
        catalog.summary(&id).unwrap().generated_title,
        summary.generated_title
    );
}

#[test]
fn rename_before_publication_wrong_root_and_deleted_session_refuse() {
    let (_directory, mut catalog, id, conversation) = fixture();
    assert!(!publish(&mut catalog, &id, &ConversationId::generate()).unwrap());
    assert!(
        !publish(
            &mut catalog,
            &SessionId::from_uuid(uuid::Uuid::now_v7()).unwrap(),
            &conversation
        )
        .unwrap()
    );
    catalog.rename(&id, "Chosen name").unwrap();
    assert!(!publish(&mut catalog, &id, &conversation).unwrap());
    assert!(catalog.summary(&id).unwrap().generated_title.is_none());

    let (_directory, mut catalog, id, conversation) = fixture();
    let super::super::deletion::SessionDeleteResult::Preview { preview } =
        catalog.delete_preview(&id)
    else {
        panic!("idle Session has a deletion preview");
    };
    let work = catalog
        .commit_delete(&id, &preview.target_revision)
        .unwrap()
        .unwrap();
    assert!(
        !publish(&mut catalog, &id, &conversation).unwrap(),
        "tombstone prevents late publication"
    );
    let cleaned = work.run();
    assert!(matches!(
        catalog.finish_delete(&work.record, cleaned),
        super::super::deletion::SessionDeleteResult::Deleted { .. }
    ));
    assert!(
        !publish(&mut catalog, &id, &conversation).unwrap(),
        "completed deletion cannot be undone"
    );
}

#[test]
fn visible_error_retains_exact_title_and_publishes_invalidation() {
    let (_directory, mut catalog, id, conversation) = fixture();
    let invalidations = catalog.summary_invalidations();
    let frontier = invalidations.frontier();
    catalog.arm_write_fault_before_rename();
    assert!(
        !publish(&mut catalog, &id, &conversation)
            .unwrap_err()
            .committed()
    );
    assert_eq!(invalidations.frontier(), frontier);
    assert!(catalog.summary(&id).unwrap().generated_title.is_none());
    catalog.arm_write_fault_after_rename();
    assert!(
        publish(&mut catalog, &id, &conversation)
            .unwrap_err()
            .committed()
    );
    assert!(invalidations.frontier() > frontier);
    assert_eq!(
        invalidations.next_after(frontier),
        Some((invalidations.frontier(), id.clone(), true)),
        "a visible title refreshes query membership even when durability is uncertain"
    );
    let frontier = invalidations.frontier();
    assert!(!publish(&mut catalog, &id, &conversation).unwrap());
    assert_eq!(invalidations.frontier(), frontier);
    let reopened = SessionCatalog::read_under_guard(&catalog.product)
        .unwrap()
        .unwrap();
    assert_eq!(
        reopened.summary(&id).unwrap().generated_title,
        catalog.summary(&id).unwrap().generated_title
    );
}

#[test]
fn copied_labels_keep_distinct_provenance_and_do_not_depend_on_the_cut() {
    let (_directory, mut catalog, id, conversation) = fixture();
    publish(&mut catalog, &id, &conversation).unwrap();
    let (_, state) = catalog.lineage(&id, None).unwrap();
    let mut prepared = catalog.prepare_session(&state, &[]).unwrap();
    catalog.copy_saved_label(&id, &mut prepared).unwrap();
    catalog.rename(&id, "Later source rename").unwrap();
    let copied = catalog
        .publish_session(&prepared, super::super::SessionNodeOrigin::New)
        .unwrap();
    assert!(copied.name.is_none());
    assert_eq!(
        copied.generated_title.unwrap().provenance,
        SessionTitleProvenance::Inherited {
            source_session: id.clone()
        }
    );
    let mut manual_copy = catalog.prepare_session(&state, &[]).unwrap();
    catalog.copy_saved_label(&id, &mut manual_copy).unwrap();
    assert_eq!(manual_copy.name.as_deref(), Some("Later source rename"));
    assert!(matches!(
        manual_copy.generated_title.unwrap().provenance,
        SessionTitleProvenance::Inherited { .. }
    ));
}

#[test]
fn generated_text_and_current_catalog_shape_are_bounded_and_strict() {
    for invalid in ["", "two\nlines", "a\u{0000}b", "a\u{2028}b"] {
        assert!(accept_title(invalid).is_err());
    }
    assert!(accept_title(&"界".repeat(121)).is_err());
    assert!(accept_title(&"界".repeat(120)).is_ok());
    let (_directory, catalog, id, _) = fixture();
    let mut value = serde_json::to_value(&catalog.document).unwrap();
    value["sessions"][id.as_str()]
        .as_object_mut()
        .unwrap()
        .remove("generated_title");
    assert!(serde_json::from_value::<super::super::CatalogDocument>(value).is_err());
}

#[tokio::test]
async fn native_clone_and_historical_fork_copy_saved_metadata_independently() {
    use crate::durable::CanonicalStore;
    let (directory, mut catalog, id, conversation) = fixture();
    let path = catalog.database_path(&id, &conversation);
    let store = crate::durable::SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store
        .append_canonical(&crate::message::types::MessageBlock::User(
            crate::message::types::UserMessageBlock {
                id: MessageId::new("first-human"),
                content: vec![crate::message::types::UserContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: "Original input".into(),
                    },
                )],
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: None,
            },
        ))
        .unwrap();
    let revision = store.load_head().unwrap().revision;
    drop(store);
    publish(&mut catalog, &id, &conversation).unwrap();
    drop(catalog);
    let controller =
        crate::local_runtime::session_controller::SessionController::open(directory.path())
            .unwrap();
    let clone = controller
        .clone_session(&id, None, revision)
        .await
        .unwrap()
        .session;
    let fork = controller
        .fork_session(&id, None, revision, Some(&MessageId::new("first-human")))
        .await
        .unwrap()
        .session;
    for target in [&clone, &fork] {
        assert_eq!(
            target.generated_title.as_ref().unwrap().text,
            "A bounded title"
        );
        assert_eq!(
            target.generated_title.as_ref().unwrap().provenance,
            SessionTitleProvenance::Inherited {
                source_session: id.clone()
            }
        );
        assert!(target.name.is_none());
    }
    let access = controller.acquire_session(&fork.id, None).await.unwrap();
    let fork_store = crate::durable::SqliteConversationStore::open_existing(
        access.node.conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    assert!(
        fork_store.load_canonical().unwrap().is_empty(),
        "saved label is independent of the selected cut"
    );
    drop(fork_store);
    drop(access);
    controller
        .rename_session(&id, "Renamed source")
        .await
        .unwrap();
    assert_eq!(
        controller
            .read_session_summary(&clone.id)
            .await
            .unwrap()
            .generated_title,
        clone.generated_title
    );
    let manual = controller
        .clone_session(&id, None, revision)
        .await
        .unwrap()
        .session;
    assert_eq!(manual.name.as_deref(), Some("Renamed source"));
    assert!(manual.generated_title.is_some());
}
