//! The Session title projection: published once by the first turn, never
//! repainted, repaired exactly once after a failed commit, and repaired from
//! the Session root on cold branch loads.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;
use crate::durable::CanonicalStore;

// Wait on the catalog's native publication signal, not on elapsed time or retries.
async fn activity_published(f: &Fixture, id: &crate::local_runtime::session::SessionId) {
    let mut changes = f.manager.sessions.summary_invalidations().changes();
    loop {
        let catalog = f.manager.sessions.catalog.lock().await;
        let expected = catalog.activity_subject(id).unwrap().unwrap();
        if catalog.summary(id).unwrap().updated_at >= expected {
            return;
        }
        drop(catalog);
        changes.changed().await.unwrap();
    }
}

// P05 + P06 (runtime plane): the first turn's canonical commit publishes the
// normalized, 120-character-bounded projection while the turn is still
// parked; a later turn updates activity but never repaints the first-message preview.
#[tokio::test]
async fn the_first_turn_publishes_the_bounded_projection_and_later_turns_never_repaint() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        // Whitespace-heavy, multiline, multi-script, and long enough that the
        // 119th character is a non-space (so the kept prefix is not trimmed).
        let text = format!("  {}\n\n\t {}", "é".repeat(150), "汉".repeat(50));
        assert!(!text.contains("request-B"));
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            50,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text(&text),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        {
            let probe = probe.borrow();
            assert!(probe.attempted && probe.published);
        }
        // The companion rendering: the wire row must equal `preview_of` of
        // the same boundary, normalized to one line and bounded at 120 chars.
        let expected =
            crate::local_runtime::session::preview_of(&crate::message::types::UserMessageBlock {
                id: crate::runtime::identity::MessageId::new("p05-companion"),
                content: input(&text),
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: None,
            })
            .expect("text renders a line");
        assert_eq!(expected.chars().count(), 120);
        assert!(expected.ends_with('\u{2026}'));
        let before = crate::durable::conversation_store_opens_on_this_thread();
        let MethodResult::Sessions { sessions, .. } = call(
            &connection,
            51,
            Method::SessionList {
                query: None,
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("list")
        };
        let after = crate::durable::conversation_store_opens_on_this_thread();
        assert_eq!(after, before, "listing reads the persisted projection");
        let row = sessions
            .iter()
            .find(|row| row.id == target.session_id)
            .unwrap();
        assert_eq!(row.preview.as_deref(), Some(expected.as_str()));
        activity_published(&f, &target.session_id).await;
        let generation = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .document_generation();
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        // P06 (runtime plane): a second turn is an ordinary new commit, but
        // the one-shot publisher is spent — the settled first line survives
        // byte-identically while the new human activity commits separately.
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            53,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("a follow-up repaints nothing"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        completion.observed(&connection, &target).await;
        {
            let probe = probe.borrow();
            assert!(probe.finished && probe.published);
        }
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            53,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        activity_published(&f, &target.session_id).await;
        assert_eq!(summary.preview.as_deref(), Some(expected.as_str()));
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .document_generation(),
            generation + 1,
            "a later human turn publishes activity without repainting the preview"
        );
        f.close().await;
    })
    .await;
}

// P07 (runtime plane): a Session named before its first turn keeps the name
// and gains the first turn's projection underneath it.
#[tokio::test]
async fn a_projection_published_underneath_a_name_survives_over_the_wire() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::Session { session } = call(
            &connection,
            60,
            Method::SessionName {
                session_id: f.sessions[0].id.clone(),
                name: "kept through publication".into(),
            },
        )
        .await
        else {
            panic!("name")
        };
        assert_eq!(session.name.as_deref(), Some("kept through publication"));
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            61,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("named session first subject"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(probe.borrow().published);
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            62,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.name.as_deref(), Some("kept through publication"));
        assert_eq!(
            summary.preview.as_deref(),
            Some("named session first subject")
        );
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        f.close().await;
    })
    .await;
}

// P08 (runtime plane): once the root projection is settled, an agent-sourced
// boundary admitted to the same root runtime is just another commit — the
// projection stays byte-identical and the catalog generation never moves.
#[tokio::test]
async fn an_agent_sourced_boundary_never_repaints_the_settled_projection() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            63,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("human root subject"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(probe.borrow().published);
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        activity_published(&f, &target.session_id).await;
        let generation = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .document_generation();
        let live = f.manager.load(&target.session_id, None).await.unwrap();
        let runtime = live.inspect_runtime().expect("the runtime stays resident");
        let completion = AttemptCompletion::capture(&f, &target);
        runtime
            .submit_sourced_inbound(
                crate::message::types::UserSource::Agent {
                    agent_id: crate::runtime::identity::AgentId::new("parent-agent"),
                },
                input("an agent-authored repaint attempt"),
            )
            .unwrap();
        completion.observed(&connection, &target).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            64,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview.as_deref(), Some("human root subject"));
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .document_generation(),
            generation
        );
        f.close().await;
    })
    .await;
}

// P11: a failed projection commit (one-shot catalog write fault armed before
// the first turn) leaves canonical history untouched, is observable on the
// publisher probe, lists as `None`, and the explicit repair seam publishes
// exactly the first line afterwards.
#[tokio::test]
async fn a_failed_projection_commit_preserves_history_and_repairs_exactly_once() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let gate = crate::local_runtime::session_display_projection::publication_test_support::arm(
            &target.session_id,
        );
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            65,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("recovered after the fault"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        gate.parked(1).await;
        activity_published(&f, &target.session_id).await;
        // The independent activity write is settled; inject the fault into
        // exactly the parked preview publication.
        f.manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_before_rename();
        gate.release();
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        {
            let probe = probe.borrow();
            assert!(probe.attempted, "the commit rendered a line");
            assert!(!probe.published, "the armed fault failed the commit");
        }
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        // Canonical history is exactly the turn: one ordinary user boundary
        // and one assistant response.
        let path = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .database_path(&target.session_id, &target.conversation_id);
        let store =
            crate::durable::SqliteConversationStore::open(target.conversation_id.clone(), &path)
                .unwrap();
        let canonical = store.load_canonical().unwrap();
        assert_eq!(
            canonical
                .iter()
                .filter(|message| matches!(
                    message,
                    crate::message::types::MessageBlock::User(user)
                        if user.kind == crate::message::types::InboundKind::Message
                ))
                .count(),
            1
        );
        assert_eq!(
            canonical
                .iter()
                .filter(|message| matches!(
                    message,
                    crate::message::types::MessageBlock::Assistant(_)
                ))
                .count(),
            1
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            66,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview, None);
        // The fault was one-shot; the explicit repair seam derives and
        // publishes exactly the first line.
        assert_eq!(
            f.manager
                .sessions
                .repair_display_preview(&target.session_id)
                .await
                .unwrap(),
            crate::local_runtime::session_controller::DisplayPreviewRepair::Published
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            67,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(
            summary.preview.as_deref(),
            Some("recovered after the fault")
        );
        f.close().await;
    })
    .await;
}

// Image-only case (extra, app-server plane): a first boundary with no
// renderable text — an upload-only turn — settles the projection as absent
// through the publisher itself, and a later text turn never repaints it:
// the one-shot publisher is spent, repair/report seams treat the settled
// `None` as final.
#[tokio::test]
async fn an_upload_only_first_turn_settles_the_projection_absent_and_never_repaints() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let files = binary_upload(
            &connection,
            target.clone(),
            "pixel.png",
            &[137_u8, 80, 78, 71, 13, 10, 26, 10],
        )
        .await;
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            71,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Upload(files[0].receipt.clone())],
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        {
            let probe = probe.borrow();
            assert!(!probe.attempted, "no renderable text means no attempt");
            assert!(!probe.published);
        }
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            72,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview, None);
        // The one-shot publisher is spent: a later text turn commits, but
        // the settled `None` never gets repainted.
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            73,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("text after the upload"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        completion.observed(&connection, &target).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            74,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview, None);
        f.close().await;
    })
    .await;
}

/// Issue #386 regression A (native half): the real window between the
/// canonical commit and the display-projection commit.
///
/// The one-shot publisher is parked *after* it renders its line and *before*
/// it takes the catalog mutex, so an ordinary `session/summary` read really is
/// served — and really does answer `preview: null` — while publication is
/// still outstanding. Releasing publication then produces the Session-scoped
/// metadata invalidation a live client needs, with no further user turn and no
/// client-initiated retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_publication_serves_a_null_summary_then_invalidates_it() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let gate = crate::local_runtime::session_display_projection::publication_test_support::arm(
            &target.session_id,
        );
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let completion = AttemptCompletion::capture(&f, &target);
        let reply = call(
            &connection,
            80,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("live convergence subject"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        // The provider request proves the canonical user boundary committed.
        f.gates[0].wait_entered().await;
        // The publisher has derived its line and is parked before the catalog
        // mutex. It therefore cannot be blocking the read below.
        gate.parked(1).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            81,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(
            summary.preview, None,
            "canonical history exists and the projection is still unpublished"
        );

        // Consume the independently committed activity invalidation while the
        // preview is still parked, so coalescing cannot merge the two facts.
        loop {
            if let NotificationMethod::SummaryInvalidated {
                session_id,
                catalog_changed: true,
            } = connection.next_notification().await.notification
                && session_id == target.session_id
            {
                break;
            }
        }
        gate.release();
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(probe.borrow().published);
        // The native metadata owner announces the change by Session identity.
        let invalidated = loop {
            if let NotificationMethod::SummaryInvalidated {
                session_id,
                catalog_changed,
            } = connection.next_notification().await.notification
            {
                // The human commit separately invalidates recency. Wait for
                // the preview-only invalidation after releasing its gate.
                if !catalog_changed {
                    break session_id;
                }
            }
        };
        assert_eq!(
            invalidated, target.session_id,
            "the invalidation is routed by Session identity, not by attachment target"
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            82,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(
            summary.preview.as_deref(),
            Some("live convergence subject"),
            "the authoritative reread carries the committed projection"
        );
        f.gates[0].release();
        completion.observed(&connection, &target).await;
        f.close().await;
    })
    .await;
}

/// Issue #386 regression (App Server plane): a Session cold-loaded straight
/// onto a branch repairs its **root** subject, arms no publisher on the branch,
/// and writes nothing the second time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_loading_a_branch_repairs_the_projection_from_the_session_root() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let sessions = &f.manager.sessions;
        let session = f.sessions[0].clone();
        let root_node = session.active_node.clone();
        let text = |id: &str, text: &str| {
            crate::message::types::MessageBlock::User(crate::message::types::UserMessageBlock {
                id: crate::runtime::identity::MessageId::new(id),
                content: input(text),
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: None,
            })
        };
        let store = |access: &crate::local_runtime::session_controller::SessionAccess| {
            crate::durable::SqliteConversationStore::open(
                access.node.conversation_id.clone(),
                &access.database_path,
            )
            .unwrap()
        };
        let boundary = crate::runtime::identity::MessageId::new("root-subject-a");
        let revision = {
            let access = sessions
                .acquire_session(&session.id, Some(&root_node))
                .await
                .unwrap();
            let root = store(&access);
            root.append_canonical(&text("root-subject-a", "root subject A"))
                .unwrap();
            root.load_head().unwrap().revision
        };
        // Cut before the root's only boundary: the branch retains no root
        // message and can disagree with the root about its first message.
        let branch = sessions
            .branch_session_node(&session.id, &root_node, revision, &boundary)
            .await
            .unwrap()
            .session;
        let branch_node = branch.active_node.clone();
        let branch_canonical = {
            let access = sessions
                .acquire_session(&session.id, Some(&branch_node))
                .await
                .unwrap();
            let branch = store(&access);
            branch
                .append_canonical(&text("branch-subject-z", "branch subject Z"))
                .unwrap();
            branch.load_canonical().unwrap()
        };
        assert_eq!(
            sessions
                .read_session_summary(&session.id)
                .await
                .unwrap()
                .preview,
            None,
            "the Session starts with a missing projection"
        );

        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::Attached { target, .. } = call(
            &connection,
            90,
            Method::SessionAttach {
                session_id: session.id.clone(),
                node_id: Some(branch_node.clone()),
            },
        )
        .await
        else {
            panic!("attached")
        };
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            91,
            Method::SessionSummary {
                session_id: session.id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(
            summary.preview.as_deref(),
            Some("root subject A"),
            "the repair derives the Session's root subject, never the composed branch's"
        );
        assert!(
            crate::local_runtime::session_display_projection::display_projection_probe(&session.id)
                .is_none(),
            "a branch runtime is never the projection subject, so no publisher is armed"
        );
        // The branch's canonical history is untouched by Session metadata repair.
        assert_eq!(
            store(
                &sessions
                    .acquire_session(&session.id, Some(&branch_node))
                    .await
                    .unwrap()
            )
            .load_canonical()
            .unwrap(),
            branch_canonical
        );

        // Write-level idempotence across a *second cold composition*, not a
        // detach/reattach cycle: detach only releases an external claim, so the
        // branch runtime is unloaded through its real owner and its retirement
        // is proven before the repair-capable composition runs again.
        let generation = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .document_generation();
        let invalidations = sessions.summary_invalidations();
        let recorded = invalidations.recorded();
        let before = sessions.read_session(&session.id).await.unwrap();
        let revision_before = sessions.read_settings(&session.id).await.unwrap().0;
        let preview_before = sessions
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .preview;
        let root_canonical_before = store(
            &sessions
                .acquire_session(&session.id, Some(&root_node))
                .await
                .unwrap(),
        )
        .load_canonical()
        .unwrap();
        let conversation = target.conversation_id.clone();
        let incarnation = target.runtime_incarnation;
        let probe = f.manager.probe(&conversation);
        let compositions = probe.compositions.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(compositions, 1, "the branch composed exactly once so far");
        assert!(matches!(
            call(&connection, 92, Method::SessionDetach { target }).await,
            MethodResult::Detached {}
        ));
        f.manager.unload(&conversation).await.unwrap();
        assert_eq!(
            f.manager.residency(&conversation),
            crate::local_runtime::session_runtime_manager::ResidencyState::Unloaded,
            "the branch runtime is retired, not merely detached"
        );
        let MethodResult::Attached { target, .. } = call(
            &connection,
            93,
            Method::SessionAttach {
                session_id: session.id.clone(),
                node_id: Some(branch_node.clone()),
            },
        )
        .await
        else {
            panic!("attached")
        };
        assert_ne!(
            target.runtime_incarnation, incarnation,
            "reopening the branch produced a new runtime incarnation"
        );
        assert_eq!(
            probe.compositions.load(std::sync::atomic::Ordering::SeqCst),
            compositions + 1,
            "a second real composition ran the repair-capable startup path again"
        );
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .document_generation(),
            generation,
            "an already-correct projection is a write-level no-op"
        );
        assert_eq!(
            invalidations.recorded(),
            recorded,
            "a no-op repair announces nothing"
        );
        let after = sessions.read_session(&session.id).await.unwrap();
        assert_eq!(after.updated_at, before.updated_at);
        assert_eq!(
            sessions.read_settings(&session.id).await.unwrap().0,
            revision_before
        );
        assert_eq!(
            sessions
                .read_session_summary(&session.id)
                .await
                .unwrap()
                .preview,
            preview_before,
            "the already-correct root preview survives the second composition"
        );
        assert_eq!(
            store(
                &sessions
                    .acquire_session(&session.id, Some(&root_node))
                    .await
                    .unwrap()
            )
            .load_canonical()
            .unwrap(),
            root_canonical_before,
            "the root's canonical history is unchanged by the second repair"
        );
        assert_eq!(
            store(
                &sessions
                    .acquire_session(&session.id, Some(&branch_node))
                    .await
                    .unwrap()
            )
            .load_canonical()
            .unwrap(),
            branch_canonical
        );
        assert!(
            crate::local_runtime::session_display_projection::display_projection_probe(&session.id)
                .is_none(),
            "the reopened branch still arms no live root-preview publisher"
        );
        assert!(matches!(
            call(&connection, 94, Method::SessionDetach { target }).await,
            MethodResult::Detached {}
        ));
        f.close().await;
    })
    .await;
}
