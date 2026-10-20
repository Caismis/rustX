//! Unused-Session classification against accepted work.

use super::super::SESSION_LIST_PAGE_LIMIT;
use super::*;

/// The one Session lifecycle classification every product path shares:
/// startup reuse, `/new`, and `/resume` visibility. An unused Session is
/// exactly one untouched `New` root node — no canonical history, no
/// durable Pending Inbound — and `/resume` never lists it. Any committed
/// message disqualifies the Session, and so does durable acceptance of a
/// prompt that was never adopted, a branch node, or clone/fork
/// provenance — even when the node's own conversation seed is empty.
#[test]
#[allow(clippy::too_many_lines)]
fn only_an_untouched_root_session_counts_as_unused() {
    let (directory, mut catalog, _config) = open_catalog();
    let visible = |catalog: &SessionCatalog| {
        catalog
            .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
            .expect("resume page")
            .sessions
            .into_iter()
            .map(|summary| summary.id)
            .collect::<Vec<_>>()
    };
    assert!(
        catalog
            .is_session_unused(&first_session(&catalog))
            .expect("fresh catalog")
    );
    assert!(
        visible(&catalog).len() == 1,
        "a created Session is visible even before user work"
    );

    // A Session-local model choice is metadata, not use.
    catalog
        .persist_model(&first_session(&catalog), config().initial_model().clone())
        .expect("persist Session model");
    assert!(
        catalog
            .is_session_unused(&first_session(&catalog))
            .expect("model choice only")
    );
    assert!(
        visible(&catalog).len() == 1,
        "metadata does not change usage classification"
    );

    // A message that was accepted durably but never adopted is already
    // this Session's work, however empty the canonical history still is.
    let (session_id, node, _) = catalog
        .lineage(&first_session(&catalog), None)
        .map(|(node, state)| (first_session(&catalog), node, state))
        .expect("root lineage");
    let pending_store = store_for(&catalog, &session_id, &node.conversation_id);
    pending_store
        .accept_inbound(crate::durable::InboundDraft {
            message_id: None,
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![text("pending")],
            timestamp: Utc::now(),
            correlation: None,
        })
        .expect("accept pending inbound");
    assert!(
        !catalog
            .is_session_unused(&first_session(&catalog))
            .expect("pending inbound")
    );
    assert_eq!(
        visible(&catalog),
        vec![session_id.clone()],
        "durable Pending Inbound acceptance is immediately resume-visible, \
             before canonical adoption, model start, or assistant output"
    );
    drop(pending_store);

    // A restart reads the same classification back while the prompt is
    // still only pending.
    let reopened = reopen_catalog(directory.path());
    assert!(
        !reopened
            .is_session_unused(&first_session(&catalog))
            .expect("reopened pending inbound")
    );
    assert_eq!(visible(&reopened), vec![session_id.clone()]);
    drop(reopened);

    let history = source_history();
    let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
    assert!(
        !catalog
            .is_session_unused(&first_session(&catalog))
            .expect("used root")
    );

    // Branching at the first user message leaves the active node's own
    // conversation empty, while the Session itself is anything but.
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let revision = source_store.load_head().expect("source head").revision;
    let source = lineage_at(&source_store, &source_conversation, revision);
    let (prepared, _) = catalog
        .prepare_tree_node(
            &source_session,
            &state(),
            &source,
            &MessageId::new("source-user-a"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("prepare tree node");
    catalog
        .publish_node(
            &source_session,
            &prepared,
            source_node,
            SessionNodeOrigin::New,
        )
        .expect("publish branch node");
    assert!(
        !catalog
            .is_session_unused(&first_session(&catalog))
            .expect("branched session")
    );

    // A newly published Session is unused again: an internal durable
    // shell beside the used one, persisted but not resume-visible.
    let prepared = catalog
        .prepare_session(&state(), &[])
        .expect("prepare new session");
    catalog
        .publish_session(&prepared, SessionNodeOrigin::New)
        .expect("publish new session");
    assert!(
        catalog
            .is_session_unused(&prepared.session_id)
            .expect("new session")
    );
    assert_eq!(
        visible(&catalog),
        vec![prepared.session_id.clone(), source_session.clone()],
        "both independent Sessions are visible"
    );
    assert_eq!(
        catalog.persisted_session_ids().len(),
        2,
        "the shell is still durable catalog state"
    );

    // A restart preserves the used/unused classification exactly.
    let reopened = reopen_catalog(directory.path());
    assert!(
        reopened
            .is_session_unused(&prepared.session_id)
            .expect("reopened new session")
    );
    assert_eq!(
        visible(&reopened),
        vec![prepared.session_id, source_session]
    );
}

/// The Issue #167 race regression. The classification used to combine
/// two independent durable reads — the current Surface head and the
/// current Pending Inbox — and adoption commits between them produced a
/// state that never existed: pre-adoption head + post-adoption empty
/// inbox = a false "unused". This test forces exactly that interleaving
/// through the real classifier: it parks `is_unused` after the
/// pre-adoption head observation, runs the Agent Loop's atomic adoption,
/// and then lets the classification finish. The monotonic acceptance
/// watermark must still settle it as used.
#[test]
fn adoption_racing_classification_cannot_hide_accepted_work() {
    let (directory, mut catalog, _config) = open_catalog();
    let (session_id, node, _) = catalog
        .lineage(&first_session(&catalog), None)
        .map(|(node, state)| (first_session(&catalog), node, state))
        .expect("root lineage");
    let store = store_for(&catalog, &session_id, &node.conversation_id);

    // Durable acceptance of user work, not yet adopted: the state the
    // race starts from.
    let accepted = store
        .accept_inbound(crate::durable::InboundDraft {
            message_id: None,
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![text("racing prompt")],
            timestamp: Utc::now(),
            correlation: None,
        })
        .expect("accept user work");
    assert_eq!(
        store.load_head().expect("head before adoption").revision,
        SurfaceRevision::INITIAL
    );

    // Park the classifier between its Surface-head observation and its
    // acceptance-watermark read: it has provably observed the
    // pre-adoption half of the durable state.
    let gate = catalog.arm_classification_gate();
    let classifying = {
        let catalog = catalog.clone();
        std::thread::spawn(move || catalog.is_session_unused(&first_session(&catalog)))
    };
    gate.wait_entered();

    // The Agent Loop's atomic adoption transition: canonical append,
    // Surface advance, and Pending removal in one commit.
    let adopted = store
        .adopt_pending_batch(accepted.sequence, None)
        .expect("adopt pending inbound");
    assert_eq!(adopted.items.len(), 1);
    assert!(
        store
            .load_pending()
            .expect("pending after adopt")
            .is_empty(),
        "the old torn read would now observe an empty Pending Inbox"
    );

    gate.release();
    let unused = classifying
        .join()
        .expect("classification thread")
        .expect("classification");
    assert!(
        !unused,
        "durable acceptance is monotonic: adoption can never make an accepted Session unused"
    );

    // The classification stays used after the adoption and across a
    // restart.
    assert!(
        !catalog
            .is_session_unused(&first_session(&catalog))
            .expect("post-adoption")
    );
    let reopened = reopen_catalog(directory.path());
    assert!(
        !reopened
            .is_session_unused(&first_session(&catalog))
            .expect("reopened")
    );
}
