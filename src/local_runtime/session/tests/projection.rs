//! Bounded list, summary, tree and history projections, and metadata
//! publication that never opens a conversation store at read time.

use super::super::{
    LineageSide, SESSION_LIST_PAGE_LIMIT, SESSION_NAME_LIMIT, SESSION_PREVIEW_LIMIT,
    SESSION_TREE_PAGE_LIMIT, SessionError, SessionNodeOrigin, preview_of,
};
use super::*;
use crate::durable::CanonicalStore;

#[test]
fn new_and_name_publish_metadata_without_mutating_old_history() {
    let (_directory, mut catalog, _config) = open_catalog();
    let history = source_history();
    let (source_conversation, source_session, _source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let before = source_store.load_canonical().expect("source history");

    let prepared = catalog
        .prepare_session(&state(), &[])
        .expect("prepare new session");
    let new_session_id = prepared.session_id.clone();
    let snapshot = catalog
        .publish_session(&prepared, SessionNodeOrigin::New)
        .expect("publish new session");

    assert_eq!(
        snapshot.name, None,
        "a published Session is born unnamed; nothing generates a label for it"
    );
    assert_ne!(snapshot.id, source_session);
    assert_ne!(
        snapshot.active_conversation_id, source_conversation,
        "new Session must own a new ConversationId"
    );
    assert_eq!(
        catalog
            .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
            .expect("list page")
            .sessions
            .len(),
        2,
        "both independent Sessions are visible"
    );
    assert_eq!(
        catalog.persisted_session_ids().len(),
        2,
        "the new empty shell is persisted without becoming history"
    );
    assert_eq!(
        source_store
            .load_canonical()
            .expect("source history after new"),
        before
    );

    let renamed = catalog
        .rename(&new_session_id, "  review\n branch  ")
        .expect("rename metadata");
    assert_eq!(
        renamed.name.as_deref(),
        Some("review branch"),
        "a name is normalized into the single line a selector row shows"
    );
    assert!(matches!(
        catalog.rename(&new_session_id, "   "),
        Err(SessionError::InvalidName)
    ));
    assert!(matches!(
        catalog.rename(&new_session_id, &"n".repeat(SESSION_NAME_LIMIT + 1)),
        Err(SessionError::InvalidName)
    ));
    let new_nodes = catalog
        .node_page(&new_session_id, 0, SESSION_TREE_PAGE_LIMIT)
        .expect("new node page")
        .nodes;
    let new_node = &new_nodes[0];
    let new_store = store_for(&catalog, &new_session_id, &new_node.conversation_id);
    assert!(
        new_store
            .load_canonical()
            .expect("new canonical history")
            .is_empty(),
        "new starts with only the intended empty bootstrap state"
    );
}

#[test]
fn activity_orders_before_paging_and_survives_metadata_edits_and_reopen() {
    let (_directory, mut catalog, _) = open_catalog();
    let oldest = first_session(&catalog);
    for _ in 0..34 {
        let prepared = catalog.prepare_session(&state(), &[]).unwrap();
        catalog
            .publish_session(&prepared, SessionNodeOrigin::New)
            .unwrap();
    }
    assert!(
        !catalog
            .list_page(None, 0, 32)
            .unwrap()
            .sessions
            .iter()
            .any(|s| s.id == oldest)
    );
    let time = chrono::DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let invalidations = catalog.summary_invalidations();
    let before = invalidations.frontier();
    assert!(catalog.publish_activity(&oldest, time).unwrap());
    assert!(invalidations.frontier() > before);
    let page = catalog.list_page(None, 0, 32).unwrap();
    assert_eq!(page.sessions[0].id, oldest);
    assert_eq!(page.next_offset, Some(32));
    assert_eq!(catalog.list_page(None, 32, 32).unwrap().sessions.len(), 3);
    catalog.rename(&oldest, "recent chat").unwrap();
    let revision = catalog.settings_revision(&oldest).unwrap();
    catalog
        .replace_settings(&oldest, revision, state())
        .unwrap();
    assert_eq!(catalog.summary(&oldest).unwrap().updated_at, time);
    assert!(
        !catalog
            .publish_activity(&oldest, time - chrono::Duration::hours(1))
            .unwrap()
    );
    let other = catalog.persisted_session_ids()[1].clone();
    catalog.publish_activity(&other, time).unwrap();
    let page = catalog.list_page(None, 0, 2).unwrap();
    let mut tied = vec![oldest.clone(), other];
    tied.sort();
    assert_eq!(
        page.sessions
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        tied
    );
    assert_eq!(
        catalog
            .list_page(Some("recent chat"), 0, 32)
            .unwrap()
            .sessions[0]
            .id,
        oldest
    );
    let reopened = SessionCatalog::read_under_guard(&catalog.product)
        .unwrap()
        .unwrap();
    assert_eq!(reopened.summary(&oldest).unwrap().updated_at, time);
}

#[test]
fn activity_repair_uses_durable_human_messages_not_agent_messages() {
    let (_directory, mut catalog, _) = open_catalog();
    let time = chrono::DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let MessageBlock::User(mut human) = user("activity-human", "hello") else {
        unreachable!()
    };
    human.timestamp = Some(time);
    let mut agent = human.clone();
    agent.id = MessageId::new("activity-agent");
    agent.source = UserSource::Runtime;
    agent.timestamp = Some(time + chrono::Duration::hours(1));
    let (_, id, _) = append_history(
        &catalog,
        &[MessageBlock::User(human), MessageBlock::User(agent)],
    );
    assert_eq!(catalog.activity_subject(&id).unwrap(), Some(time));
    catalog.publish_activity(&id, time).unwrap();
    assert_eq!(catalog.summary(&id).unwrap().updated_at, time);
}

#[test]
fn exact_summary_is_not_a_fuzzy_search_page() {
    let (_directory, mut catalog, _config) = open_catalog();
    let mut earlier = vec![first_session(&catalog)];
    for _ in 0..32 {
        let prepared = catalog.prepare_session(&state(), &[]).unwrap();
        catalog
            .publish_session(&prepared, SessionNodeOrigin::New)
            .unwrap();
        earlier.push(prepared.session_id.clone());
    }
    let target = earlier.remove(0);
    for id in &earlier {
        catalog
            .rename(id, &format!("mentions {}", target.as_str()))
            .unwrap();
    }
    let page = catalog.list_page(Some(target.as_str()), 0, 32).unwrap();
    assert_eq!(page.sessions.len(), 32);
    assert_eq!(page.next_offset, Some(32));
    assert!(page.sessions.iter().all(|row| row.id != target));
    assert_eq!(catalog.summary(&target).unwrap().id, target);
    assert!(matches!(
        catalog.summary(&super::super::SessionId::new(
            "ses_00000000-0000-7000-8000-000000000099"
        )),
        Err(SessionError::UnknownSession { .. })
    ));
}

#[test]
fn exact_and_list_summary_share_every_projected_field() {
    for (text, name) in [(true, None), (true, Some("Manual name")), (false, None)] {
        let (_directory, mut catalog, _config) = open_catalog();
        let message = if text {
            user("first", "  native\n preview  ")
        } else {
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("file-only"),
                content: vec![UserContentBlock::Image(
                    crate::message::content::ImageReference {
                        artifact_id: crate::runtime::identity::ArtifactId::new("artifact-1"),
                        alt: None,
                    },
                )],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })
        };
        append_history(&catalog, std::slice::from_ref(&message));
        let id = first_session(&catalog);
        // Production persists the projection after the first canonical
        // ordinary user commit; list/summary then read that stored line.
        if let MessageBlock::User(user) = &message
            && let Some(preview) = preview_of(user)
        {
            assert!(
                catalog.publish_display_preview(&id, &preview).unwrap(),
                "the first publication commits the projection"
            );
        }
        if let Some(name) = name {
            catalog.rename(&id, name).unwrap();
        }
        let before = catalog.snapshot(&id).unwrap();
        let exact = catalog.summary(&id).unwrap();
        let listed = catalog.list_page(None, 0, 32).unwrap().sessions.remove(0);
        assert_eq!(exact, listed);
        assert_eq!(exact.name.as_deref(), name);
        assert_eq!(exact.preview.as_deref(), text.then_some("native preview"));
        assert_eq!(exact.cwd, state().cwd);
        assert_eq!(exact.updated_at, before.created_at);
        assert_eq!(exact.active_node, before.active_node);
        assert_eq!(catalog.snapshot(&id).unwrap(), before);
    }
}

/// A Session carries no name until a user gives it one, so a `/resume`
/// row identifies it by what it opened with. Naming replaces that line
/// and nothing else: the identity, the lineage, and the conversation are
/// untouched, and the row remains searchable by either. A Session that
/// owns no durable user work yet has no row at all — it is an internal
/// shell, not history.
#[test]
fn an_unnamed_session_is_listed_by_its_first_user_message() {
    let (_directory, mut catalog, _config) = open_catalog();
    let row = |catalog: &SessionCatalog| {
        catalog
            .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
            .expect("session page")
            .sessions
            .into_iter()
            .next()
            .expect("the used Session is listed")
    };

    assert!(
        catalog
            .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
            .expect("session page")
            .sessions
            .len()
            == 1,
        "a Session that was never used is an internal shell, not a `/resume` row"
    );

    append_history(
        &catalog,
        &[
            user("first", "  restore\n  the auth module  "),
            user("second", "and then the session picker"),
        ],
    );
    // Production persists the projection after the first canonical
    // ordinary user commit; the row then reads that stored line.
    let MessageBlock::User(first_message) = user("first", "  restore\n  the auth module  ") else {
        unreachable!("user helper builds a User block")
    };
    let preview = preview_of(&first_message).expect("text message renders");
    assert!(
        catalog
            .publish_display_preview(&first_session(&catalog), &preview)
            .unwrap()
    );
    let used = row(&catalog);
    assert_eq!(
        used.preview.as_deref(),
        Some("restore the auth module"),
        "the row shows the Session's first user message as one collapsed line"
    );
    assert_eq!(used.name, None);
    assert!(
        !catalog
            .list_page(Some("auth module"), 0, SESSION_LIST_PAGE_LIMIT)
            .expect("content search")
            .sessions
            .is_empty(),
        "an unnamed row is searchable by the line it shows"
    );

    let session_id = catalog
        .snapshot(&first_session(&catalog))
        .expect("active snapshot")
        .id;
    catalog
        .rename(&session_id, "Refactor auth module")
        .expect("name the Session");
    let named = row(&catalog);
    assert_eq!(named.name.as_deref(), Some("Refactor auth module"));
    assert_eq!(
        named.preview.as_deref(),
        Some("restore the auth module"),
        "naming displaces the first message in the row, never in the Session"
    );
    assert_eq!(
        named.id, session_id,
        "a name is metadata; the identity a selection resolves is unchanged"
    );
}

/// The derived row line is bounded and text-only: a long first message is
/// cut with a visible mark, and a Session opened with no text at all has
/// no line to show rather than an invented one.
#[test]
fn a_derived_row_line_is_bounded_and_only_ever_text() {
    let (_directory, mut catalog, _config) = open_catalog();
    let long = "auth ".repeat(60);
    append_history(&catalog, &[user("long", long.as_str())]);
    let MessageBlock::User(long_message) = user("long", long.as_str()) else {
        unreachable!("user helper builds a User block")
    };
    let preview = preview_of(&long_message).expect("long text renders");
    assert!(
        catalog
            .publish_display_preview(&first_session(&catalog), &preview)
            .unwrap(),
        "production persists the derived line after the first canonical commit"
    );
    let preview = catalog
        .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
        .expect("session page")
        .sessions
        .swap_remove(0)
        .preview
        .expect("a long message still yields a line");
    assert_eq!(preview.chars().count(), SESSION_PREVIEW_LIMIT);
    assert!(preview.ends_with('\u{2026}'), "a cut line says it was cut");

    let (_directory, catalog, _config) = open_catalog();
    append_history(
        &catalog,
        &[MessageBlock::User(UserMessageBlock {
            id: MessageId::new("image-only"),
            content: vec![UserContentBlock::Image(
                crate::message::content::ImageReference {
                    artifact_id: crate::runtime::identity::ArtifactId::new("artifact-1"),
                    alt: None,
                },
            )],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })],
    );
    assert_eq!(
        catalog
            .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
            .expect("session page")
            .sessions
            .swap_remove(0)
            .preview,
        None
    );
}

/// P01–P03 fixture: `count` published Sessions, each with one ordinary
/// user message in its root store and the projection persisted exactly as
/// production publishes it after the first canonical commit. Even-indexed
/// Sessions are named so all three query kinds have real subjects. More
/// than one page of rows makes every continuation real.
fn catalog_with_projected_sessions(count: usize) -> (TempDir, SessionCatalog, Vec<SessionId>) {
    let (directory, mut catalog, _config) = open_catalog();
    let mut sessions = Vec::new();
    for index in 0..count {
        let session_id = if index == 0 {
            first_session(&catalog)
        } else {
            let prepared = catalog
                .prepare_session(&state(), &[])
                .expect("prepare Session");
            catalog
                .publish_session(&prepared, SessionNodeOrigin::New)
                .expect("publish Session")
                .id
        };
        let (node, _) = catalog.lineage(&session_id, None).expect("root lineage");
        let store = store_for(&catalog, &session_id, &node.conversation_id);
        let text = format!("topic row {index:02} unique-{index:02}");
        store
            .append_canonical(&user(&format!("user-{index:02}"), &text))
            .expect("append first user message");
        assert!(
            catalog
                .publish_display_preview(&session_id, &text)
                .expect("publish projection"),
            "the first publication commits the projection"
        );
        if index % 2 == 0 {
            catalog
                .rename(&session_id, &format!("Project {index:02}"))
                .expect("name the Session");
        }
        sessions.push(session_id);
    }
    (directory, catalog, sessions)
}

/// The store opens — if any — of the native list/search path happen on the
/// calling thread, so the per-thread delta brackets a call exactly; the
/// process-wide counter would be contaminated by parallel tests.
fn opens_on_this_thread() -> u64 {
    crate::durable::conversation_store_opens_on_this_thread()
}

// P01
#[test]
fn the_first_list_page_reads_persisted_projections_without_opening_a_store() {
    let (_directory, catalog, sessions) = catalog_with_projected_sessions(70);
    let opens = opens_on_this_thread();
    let page = catalog
        .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
        .expect("first page");
    assert_eq!(
        opens_on_this_thread() - opens,
        0,
        "listing one full page opens zero conversation stores"
    );
    assert_eq!(page.sessions.len(), SESSION_LIST_PAGE_LIMIT);
    assert_eq!(page.next_offset, Some(32));
    for (index, row) in page.sessions.iter().enumerate() {
        assert_eq!(&row.id, &sessions[69 - index], "rows are newest first");
        assert_eq!(
            row.preview.as_deref(),
            Some(format!("topic row {0:02} unique-{0:02}", 69 - index).as_str()),
            "every row carries its persisted projection"
        );
    }
}

// P02
#[test]
fn every_continuation_page_reads_persisted_projections_without_opening_a_store() {
    let (_directory, catalog, sessions) = catalog_with_projected_sessions(70);
    let opens = opens_on_this_thread();
    let page = catalog.list_page(None, 32, 32).expect("second page");
    assert_eq!(opens_on_this_thread() - opens, 0);
    assert_eq!(page.sessions.len(), 32);
    assert_eq!(page.next_offset, Some(64));
    for (index, row) in page.sessions.iter().enumerate() {
        assert_eq!(&row.id, &sessions[69 - 32 - index]);
    }
    let opens = opens_on_this_thread();
    let page = catalog.list_page(None, 64, 32).expect("last page");
    assert_eq!(opens_on_this_thread() - opens, 0);
    assert_eq!(page.sessions.len(), 6);
    assert_eq!(page.next_offset, None);
    for (index, row) in page.sessions.iter().enumerate() {
        assert_eq!(&row.id, &sessions[69 - 64 - index]);
    }
    // A filtered continuation pages the matching set the same way.
    let opens = opens_on_this_thread();
    let page = catalog
        .list_page(Some("topic row"), 32, 32)
        .expect("filtered continuation");
    assert_eq!(opens_on_this_thread() - opens, 0);
    assert_eq!(page.sessions.len(), 32);
    assert_eq!(page.next_offset, Some(64));
    assert_eq!(&page.sessions[0].id, &sessions[69 - 32]);
}

// P03 (native half; the App Server half lives in the scripted protocol suite)
#[test]
fn searching_by_identity_name_and_projection_opens_no_store() {
    let (_directory, catalog, sessions) = catalog_with_projected_sessions(70);
    // By SessionId substring.
    let needle: String = sessions[7].as_str().chars().take(20).collect();
    let opens = opens_on_this_thread();
    let page = catalog
        .list_page(Some(&needle), 0, SESSION_LIST_PAGE_LIMIT)
        .expect("identity search");
    assert_eq!(opens_on_this_thread() - opens, 0);
    assert!(
        page.sessions.iter().any(|row| row.id == sessions[7]),
        "the identity substring finds its Session"
    );
    assert!(
        page.sessions
            .iter()
            .all(|row| row.id.as_str().to_lowercase().contains(&needle)),
        "every identity hit matches the needle"
    );
    // By name (Session 42 is named "Project 42").
    let opens = opens_on_this_thread();
    let page = catalog
        .list_page(Some("project 42"), 0, SESSION_LIST_PAGE_LIMIT)
        .expect("name search");
    assert_eq!(opens_on_this_thread() - opens, 0);
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].id, sessions[42]);
    assert_eq!(page.sessions[0].name.as_deref(), Some("Project 42"));
    assert_eq!(
        page.sessions[0].preview.as_deref(),
        Some("topic row 42 unique-42")
    );
    // By projection substring (Session 69 is unnamed), case-insensitively.
    let opens = opens_on_this_thread();
    let page = catalog
        .list_page(Some("UNIQUE-69"), 0, SESSION_LIST_PAGE_LIMIT)
        .expect("projection search");
    assert_eq!(opens_on_this_thread() - opens, 0);
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].id, sessions[69]);
}

// P04
#[test]
fn an_empty_session_reads_as_the_identity_fallback_without_opening_a_store() {
    let (_directory, catalog, _config) = open_catalog();
    let empty = first_session(&catalog);
    let opens = opens_on_this_thread();
    let summary = catalog.summary(&empty).expect("empty Session summary");
    assert_eq!(opens_on_this_thread() - opens, 0);
    assert_eq!(
        summary.preview, None,
        "no ordinary user message exists yet: the row falls back to identity"
    );
    let opens = opens_on_this_thread();
    let row = catalog
        .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
        .expect("session page")
        .sessions
        .swap_remove(0);
    assert_eq!(opens_on_this_thread() - opens, 0);
    assert_eq!(row.id, empty);
    assert_eq!(row.preview, None);
}

// P06 (write-level half; the runtime half lives in the scripted protocol suite)
#[test]
fn a_second_projection_publication_is_a_write_level_no_op() {
    let (_directory, mut catalog, _config) = open_catalog();
    let id = first_session(&catalog);
    let before = catalog.snapshot(&id).expect("snapshot");
    let generation = catalog.document.generation;
    assert!(
        catalog
            .publish_display_preview(&id, "first line")
            .expect("first publication")
    );
    assert_eq!(catalog.document.generation, generation + 1);
    assert_eq!(
        catalog.snapshot(&id).expect("snapshot").updated_at,
        before.updated_at,
        "the projection never moves updated_at"
    );
    let generation = catalog.document.generation;
    assert!(
        !catalog
            .publish_display_preview(&id, "a later line")
            .expect("duplicate publication"),
        "a present projection declines the commit"
    );
    assert_eq!(
        catalog.document.generation, generation,
        "the declined publication performs no write"
    );
    assert_eq!(
        catalog.summary(&id).expect("summary").preview.as_deref(),
        Some("first line"),
        "the first projection is never overwritten"
    );
}

// P07 (catalog half; the rename-before-first-input half is scripted)
#[test]
fn a_projection_published_underneath_a_name_survives_renaming() {
    let (_directory, mut catalog, _config) = open_catalog();
    let id = first_session(&catalog);
    catalog
        .rename(&id, "auth rework")
        .expect("name the Session");
    assert!(
        catalog
            .publish_display_preview(&id, "restore the auth module")
            .expect("publish projection")
    );
    let summary = catalog.summary(&id).expect("summary");
    assert_eq!(summary.name.as_deref(), Some("auth rework"));
    assert_eq!(
        summary.preview.as_deref(),
        Some("restore the auth module"),
        "the projection publishes underneath the name"
    );
    let row = catalog
        .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
        .expect("session page")
        .sessions
        .swap_remove(0);
    assert_eq!(row, summary, "summary and list row are one projection");
    catalog.rename(&id, "billing rework").expect("rename");
    let renamed = catalog.summary(&id).expect("summary");
    assert_eq!(renamed.name.as_deref(), Some("billing rework"));
    assert_eq!(
        renamed.preview.as_deref(),
        Some("restore the auth module"),
        "renaming displaces the line in the row, never in the Session"
    );
}

// P08 (native half; the agent-sourced-input half is scripted)
#[test]
#[allow(clippy::too_many_lines)] // One commit/ABA/coalescing audit of the same native Session.
fn branching_and_switching_the_active_node_keeps_the_root_projection() {
    let (directory, mut catalog, _config) = open_catalog();
    let history = source_history();
    let (conversation, session, root_node) = append_history(&catalog, &history);
    assert!(
        catalog
            .publish_display_preview(&session, "A")
            .expect("publish root projection")
    );
    let store = store_for(&catalog, &session, &conversation);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let (branch, _) = catalog
        .prepare_tree_node(
            &session,
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            LineageSide::Before,
        )
        .expect("prepare branch node");
    assert_eq!(
        branch.display_preview, None,
        "a branch node is never the projection subject"
    );
    let branch_node = branch.node_id.clone();
    let ownership = catalog.summary_invalidations();
    let frontier = ownership.frontier();
    let a1 = catalog
        .summary(&session)
        .unwrap()
        .ownership_generation
        .parse::<u64>()
        .unwrap();
    catalog
        .publish_node(&session, &branch, root_node.clone(), SessionNodeOrigin::New)
        .expect("publish branch node");
    let published_generation = catalog.summary(&session).unwrap().ownership_generation;
    catalog
        .set_current_node(&session, Some(&branch_node))
        .expect("switch the active node");
    let retired = ownership.next_ownership_after(frontier).unwrap();
    assert_eq!(retired.1, session);
    assert_eq!(retired.2, a1);
    let summary = catalog.summary(&session).expect("summary");
    assert_eq!(
        summary.ownership_generation, published_generation,
        "repeated selection retains execution ownership"
    );
    let b = summary.ownership_generation.parse::<u64>().unwrap();
    assert!(b > a1);
    assert_eq!(
        ownership.next_ownership_after(retired.0),
        None,
        "same-node selection does not retire ownership"
    );
    assert_eq!(
        summary.active_node, branch_node,
        "the active selection moved to the branch"
    );
    assert_eq!(
        summary.preview.as_deref(),
        Some("A"),
        "the row keeps the root lineage's projection"
    );
    catalog
        .set_current_node(&session, Some(&root_node))
        .expect("switch back to the root");
    let a2 = catalog
        .summary(&session)
        .unwrap()
        .ownership_generation
        .parse::<u64>()
        .unwrap();
    assert!(a2 > b, "ABA must not reuse the original A generation");
    assert_eq!(
        ownership.next_ownership_after(frontier).unwrap().2,
        b,
        "coalescing retires through B, excluding A2"
    );
    let after_aba = ownership.frontier();
    catalog.arm_write_fault_before_rename();
    assert!(
        catalog
            .set_current_node(&session, Some(&branch_node))
            .is_err()
    );
    assert_eq!(
        catalog.summary(&session).unwrap().ownership_generation,
        a2.to_string()
    );
    assert!(ownership.next_ownership_after(after_aba).is_none());
    catalog.arm_write_fault_after_rename();
    assert!(
        catalog
            .set_current_node(&session, Some(&branch_node))
            .unwrap_err()
            .committed()
    );
    let b2 = catalog
        .summary(&session)
        .unwrap()
        .ownership_generation
        .parse::<u64>()
        .unwrap();
    assert!(b2 > a2);
    assert_eq!(
        reopen_catalog(directory.path())
            .summary(&session)
            .unwrap()
            .ownership_generation,
        b2.to_string(),
        "post-rename ownership is persisted even when directory durability is uncertain"
    );
    assert_eq!(ownership.next_ownership_after(after_aba).unwrap().2, a2);
    assert_eq!(
        catalog
            .summary(&session)
            .expect("summary")
            .preview
            .as_deref(),
        Some("A")
    );
}

// P09
#[allow(clippy::too_many_lines)] // Clone, fork, and post-compaction-head clone in one seed audit.
#[test]
fn clone_and_fork_carry_the_seed_projection_into_the_visibility_commit() {
    let (_directory, mut catalog, _config) = open_catalog();
    let history = vec![user("u1", "seed alpha"), user("u2", "later beta")];
    let (conversation, session, node) = append_history(&catalog, &history);
    // Deliberately never persist the source projection: the clone must
    // derive from the frozen seed, never copy the source row.
    let store = store_for(&catalog, &session, &conversation);
    let revision = store.load_head().expect("source head").revision;
    let source = lineage_at(&store, &conversation, revision);
    let clone = catalog
        .prepare_clone_session(&state(), &source)
        .expect("prepare clone");
    assert_eq!(
        clone.display_preview.as_deref(),
        Some("seed alpha"),
        "the clone's projection is derived from the frozen seed"
    );
    let clone_id = clone.session_id.clone();
    catalog
        .publish_session(
            &clone,
            SessionNodeOrigin::Clone {
                source_session: session.clone(),
                source_node: node.clone(),
                source_surface_revision: revision,
            },
        )
        .expect("publish clone");
    let opens = opens_on_this_thread();
    let page = catalog
        .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
        .expect("session page");
    assert_eq!(
        opens_on_this_thread() - opens,
        0,
        "the clone's row lists its born projection without a store open"
    );
    let row = page
        .sessions
        .iter()
        .find(|row| row.id == clone_id)
        .expect("clone row");
    assert_eq!(row.preview.as_deref(), Some("seed alpha"));

    // A fork cutting before the first user message retains no subject.
    let (fork_empty, editor) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("u1"),
            LineageSide::Before,
        )
        .expect("prepare pre-input fork");
    assert_eq!(editor, vec![text("seed alpha")]);
    assert_eq!(fork_empty.display_preview, None);
    let fork_empty_id = fork_empty.session_id.clone();
    catalog
        .publish_session(
            &fork_empty,
            SessionNodeOrigin::Fork {
                source_session: session.clone(),
                source_node: node.clone(),
                source_surface_revision: revision,
                source_message: MessageId::new("u1"),
                source_locator: fixture_source_locator(&store, "u1"),
                side: LineageSide::Before,
            },
        )
        .expect("publish pre-input fork");
    let page = catalog
        .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
        .expect("session page");
    assert_eq!(
        page.sessions
            .iter()
            .find(|row| row.id == fork_empty_id)
            .expect("fork row")
            .preview,
        None,
        "a fork whose cut excludes every ordinary user message has no projection"
    );

    // A fork cutting before the second user message retains the first.
    let (fork, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("u2"),
            LineageSide::Before,
        )
        .expect("prepare fork");
    assert_eq!(fork.display_preview.as_deref(), Some("seed alpha"));

    // A seed whose first chronological user boundary is not ordinary:
    // the projection subject is the first *ordinary* user message in the
    // retained seed, even when that is a later message.
    let (_directory, catalog, _config) = open_catalog();
    let history = vec![
        status("boot", "Boot"),
        user("first-ordinary", "later beta"),
        user("last", "last"),
    ];
    let (conversation, session, _node) = append_history(&catalog, &history);
    let store = store_for(&catalog, &session, &conversation);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let (fork, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("last"),
            LineageSide::Before,
        )
        .expect("prepare fork");
    assert_eq!(
        fork.display_preview.as_deref(),
        Some("later beta"),
        "an earlier non-ordinary boundary is never the subject"
    );
}

// P14
#[test]
fn a_missing_projection_is_not_repaired_at_list_time() {
    let (_directory, catalog, _config) = open_catalog();
    append_history(&catalog, &[user("first", "crash window subject")]);
    // The root store HAS a first user boundary but the projection was
    // never persisted (the crash window): listing must not open the store
    // to find out, and repair is an explicit seam, never a list-time one.
    let id = first_session(&catalog);
    let opens = opens_on_this_thread();
    assert_eq!(
        catalog.summary(&id).expect("summary").preview,
        None,
        "no fallback repair at summary time"
    );
    assert_eq!(opens_on_this_thread() - opens, 0);
    let opens = opens_on_this_thread();
    let row = catalog
        .list_page(None, 0, SESSION_LIST_PAGE_LIMIT)
        .expect("session page")
        .sessions
        .swap_remove(0);
    assert_eq!(opens_on_this_thread() - opens, 0);
    assert_eq!(row.preview, None, "no fallback repair at list time");
}

/// `/resume` pages and searches the resume-visible set only. Hidden
/// unused shells interspersed between used Sessions open no holes, shift
/// no offsets, duplicate no continuation rows, and are never matched by
/// a query — even when they carry a name.
#[test]
fn session_list_projection_is_bounded_searchable_and_continuable() {
    let (_directory, mut catalog, _config) = open_catalog();
    let mut allocated = vec![first_session(&catalog)];
    // Alternate used and untouched Sessions: session-1/3/5 own durable
    // user work; session-2/4 are internal shells that stay hidden.
    for index in 0..5 {
        if index > 0 {
            let prepared = catalog
                .prepare_session(&state(), &[])
                .expect("prepare paged session");
            catalog
                .publish_session(&prepared, SessionNodeOrigin::New)
                .expect("publish paged session");
            allocated.push(prepared.session_id.clone());
        }
        if index % 2 == 0 {
            let id = allocated[index].clone();
            let (node, _) = catalog.lineage(&id, None).unwrap();
            store_for(&catalog, &id, &node.conversation_id)
                .append_canonical(&user(
                    &format!("paged-user-{index}"),
                    &format!("paged session {index}"),
                ))
                .unwrap();
            catalog
                .rename(&id, &format!("paged session {index}"))
                .expect("name paged session");
        }
    }
    catalog
        .rename(&allocated[1].clone(), "hidden shell")
        .expect("name a hidden shell");
    assert_eq!(catalog.persisted_session_ids().len(), 5);

    let first = catalog
        .list_page(None, 0, 2)
        .expect("first bounded Session page");
    assert_eq!(first.sessions.len(), 2);
    assert_eq!(first.next_offset, Some(2));
    let second = catalog
        .list_page(None, first.next_offset.expect("continuation"), 2)
        .expect("second bounded Session page");
    assert_eq!(second.sessions.len(), 2);
    assert_eq!(second.next_offset, Some(4));
    let third = catalog.list_page(None, 4, 2).unwrap();
    assert_eq!(third.sessions.len(), 1);
    assert_eq!(third.next_offset, None);
    let ids = first
        .sessions
        .into_iter()
        .chain(second.sessions)
        .chain(third.sessions)
        .map(|summary| summary.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        allocated.into_iter().rev().collect::<Vec<_>>(),
        "offsets and next_offset describe the visible set: no holes, no duplicates"
    );

    let filtered = catalog
        .list_page(Some("paged session 2"), 0, 2)
        .expect("searchable bounded Session page");
    assert_eq!(filtered.sessions.len(), 1);
    assert_eq!(
        filtered.sessions[0].name.as_deref(),
        Some("paged session 2")
    );
    assert_eq!(filtered.next_offset, None);

    // A query that only a hidden shell could match returns nothing.
    assert!(
        catalog
            .list_page(Some("hidden shell"), 0, 2)
            .expect("hidden search page")
            .sessions
            .len()
            == 1,
        "search never reaches into hidden internal shells"
    );

    // Search paginates the visible matching set with the same
    // continuation contract.
    let searched = catalog
        .list_page(Some("paged session"), 0, 2)
        .expect("first searched page");
    assert_eq!(searched.sessions.len(), 2);
    assert_eq!(searched.next_offset, Some(2));
    let searched_next = catalog
        .list_page(
            Some("paged session"),
            searched.next_offset.expect("search continuation"),
            2,
        )
        .expect("second searched page");
    assert_eq!(searched_next.sessions.len(), 1);
    assert_eq!(searched_next.next_offset, None);

    assert!(matches!(
        catalog.list_page(None, 0, 0),
        Err(SessionError::Catalog { .. })
    ));
}

#[test]
fn tree_and_history_projections_are_bounded_and_continuable() {
    let (_directory, mut catalog, _config) = open_catalog();
    let history = source_history();
    let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let revision = source_store.load_head().expect("source head").revision;
    let source = lineage_at(&source_store, &source_conversation, revision);
    let (prepared, _) = catalog
        .prepare_tree_node(
            &source_session,
            &state(),
            &source,
            &MessageId::new("source-user-c"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .expect("prepare tree node");
    catalog
        .publish_node(
            &source_session,
            &prepared,
            source_node.clone(),
            SessionNodeOrigin::Fork {
                source_session: source_session.clone(),
                source_node: source_node.clone(),
                source_surface_revision: revision,
                source_message: MessageId::new("source-user-c"),
                source_locator: fixture_source_locator(&source_store, "source-user-c"),
                side: crate::local_runtime::session::LineageSide::Before,
            },
        )
        .expect("publish tree node");

    let node_first = catalog
        .node_page(&source_session, 0, 1)
        .expect("first bounded tree page");
    assert_eq!(node_first.nodes.len(), 1);
    assert_eq!(node_first.next_offset, Some(1));
    let node_second = catalog
        .node_page(
            &source_session,
            node_first.next_offset.expect("tree continuation"),
            1,
        )
        .expect("second bounded tree page");
    assert_eq!(node_second.nodes.len(), 1);
    assert_eq!(node_second.next_offset, None);

    let history_first = source_store
        .load_user_message_boundaries_page(revision, 0, 1)
        .expect("first bounded history page");
    assert_eq!(history_first.boundaries.len(), 1);
    assert_eq!(history_first.next_offset, Some(1));
    let history_second = source_store
        .load_user_message_boundaries_page(
            revision,
            history_first.next_offset.expect("history continuation"),
            1,
        )
        .expect("second bounded history page");
    assert_eq!(history_second.boundaries.len(), 1);
    assert_eq!(history_second.next_offset, None);
}
