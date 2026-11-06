//! Production Title ownership through real native composition and provider IO.
use super::*;

async fn title_published(f: &Fixture) -> crate::local_runtime::session::SessionSummary {
    let mut changes = f.manager.sessions.summary_invalidations().changes();
    loop {
        let summary = f
            .manager
            .sessions
            .read_session_summary(&f.sessions[0].id)
            .await
            .unwrap();
        if summary.generated_title.is_some() {
            return summary;
        }
        changes.changed().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn title_publishes_independently_then_reopen_and_primary_changes_never_regenerate() {
    bounded(async {
        let f = Fixture::with_titles().await;
        let managed = f.load(0).await.unwrap().unwrap();
        managed
            .client()
            .submit_inbound(input("first title task"))
            .unwrap();
        f.gates[0].wait_entered().await;
        f.gates[1].wait_entered().await;
        let settings_revision = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(&f.sessions[0].id)
            .unwrap();
        f.gates[1].release();
        let summary = title_published(&f).await;
        assert_eq!(summary.generated_title.unwrap().text, "done");
        assert!(summary.name.is_none());
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .settings_revision(&f.sessions[0].id)
                .unwrap(),
            settings_revision
        );
        f.manager
            .sessions
            .rename_session(&f.sessions[0].id, "Chosen title")
            .await
            .unwrap();
        f.gates[0].release();
        managed
            .inspect_runtime()
            .unwrap()
            .child_attempt_settlement(0)
            .await;
        let request_count = f.provider.request_bodies().len();
        assert_eq!(request_count, 2);
        drop(managed);
        f.manager.unload(&f.id(0).await).await.unwrap();
        let reopened = f.load(0).await.unwrap().unwrap();
        let ordinal = reopened
            .inspect_runtime()
            .unwrap()
            .settlement_ordinal_for_test();
        reopened
            .client()
            .submit_inbound(input("later task"))
            .unwrap();
        reopened
            .inspect_runtime()
            .unwrap()
            .child_attempt_settlement(ordinal)
            .await;
        assert_eq!(f.provider.request_bodies().len(), request_count + 1);
        assert_eq!(
            f.manager
                .sessions
                .read_session_summary(&f.sessions[0].id)
                .await
                .unwrap()
                .name
                .as_deref(),
            Some("Chosen title")
        );
        drop(reopened);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_rename_during_title_inference_prevents_publication_without_erasing_start() {
    bounded(async {
        let f = Fixture::with_titles().await;
        let managed = f.load(0).await.unwrap().unwrap();
        managed
            .client()
            .submit_inbound(input("rename race"))
            .unwrap();
        f.gates[1].wait_entered().await;
        f.manager
            .sessions
            .rename_session(&f.sessions[0].id, "Manual wins")
            .await
            .unwrap();
        f.gates[0].release();
        f.gates[1].release();
        managed
            .inspect_runtime()
            .unwrap()
            .child_attempt_settlement(0)
            .await;
        managed
            .inspect_runtime()
            .unwrap()
            .join_title_for_test()
            .await;
        drop(managed);
        f.manager.unload(&f.id(0).await).await.unwrap();
        let summary = f
            .manager
            .sessions
            .read_session_summary(&f.sessions[0].id)
            .await
            .unwrap();
        assert_eq!(summary.name.as_deref(), Some("Manual wins"));
        assert!(summary.generated_title.is_none());
        assert_eq!(f.provider.request_bodies().len(), 2);
        f.close().await;
    })
    .await;
}

fn first_human() -> crate::message::types::MessageBlock {
    use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
    MessageBlock::User(UserMessageBlock {
        id: crate::runtime::identity::MessageId::new(uuid::Uuid::now_v7().to_string()),
        content: input("the title subject"),
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anthropic_title_preserves_follow_and_explicit_thinking_budgets_and_text_limits() {
    bounded(async {
        use crate::model::catalog::ModelRef;
        use crate::model::session::{AuxiliaryModelPolicy, SessionModelConfig};
        for (explicit, text, accepted) in [
            (false, "🦀".repeat(120), true),
            (true, "A concise title".into(), true),
            (true, "🦀".repeat(121), false),
        ] {
            let f = Fixture::with_anthropic_titles(&text).await;
            let id = &f.sessions[0].id;
            let (revision, mut settings) = f.manager.sessions.read_settings(id).await.unwrap();
            let mut model = SessionModelConfig::of(ModelRef::parse("local/a").unwrap());
            if explicit {
                model.title_model = AuxiliaryModelPolicy::Explicit {
                    model: ModelRef::parse("local/b").unwrap(),
                    profile: None,
                    request_params: serde_json::Map::default(),
                    max_output_tokens: Some(4096),
                };
            }
            settings.model = Some(model);
            f.manager
                .sessions
                .replace_settings(id, revision, settings)
                .await
                .unwrap();
            let managed = f.load(0).await.unwrap().unwrap();
            let runtime = managed.inspect_runtime().unwrap();
            runtime.append_title_source_for_test(&first_human());
            f.gates[1].wait_entered().await;
            f.gates[1].release();
            runtime.join_title_for_test().await;
            let summary = f.manager.sessions.read_session_summary(id).await.unwrap();
            assert_eq!(summary.generated_title.is_some(), accepted);
            if let Some(title) = summary.generated_title {
                assert_eq!(title.text, text);
                assert!(title.text.chars().count() <= 120 && title.text.len() <= 480);
            }
            assert!(!runtime.first_title_source().unwrap().unwrap().eligible);
            assert_eq!(
                f.provider.request_bodies().len(),
                1,
                "one confirmed Title start, no corrective retry"
            );
            let body: serde_json::Value =
                serde_json::from_str(&f.provider.request_body(0)).unwrap();
            assert_eq!(body["model"], if explicit { "b" } else { "a" });
            assert_eq!(
                events(&runtime)
                    .iter()
                    .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
                    .count(),
                1
            );
            drop(managed);
            f.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn title_lifecycle_busy_waits_for_commit_release_without_capacity_feedback() {
    bounded(async {
        let f = Fixture::with_titles().await;
        let managed = f.load(0).await.unwrap().unwrap();
        let runtime = managed.inspect_runtime().unwrap();
        // Drive the production owner directly so Pending proves it has no
        // runnable self-wake, without relying on scheduler timing or sleep.
        runtime.stop_title_for_test().await;
        runtime.append_title_source_for_test(&first_human());
        let lifecycle = runtime.title_lifecycle();
        let store = runtime.tool_runtime().durable_store();
        let (entered, held) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            lifecycle
                .with_running_commit(|| {
                    assert!(store.first_human_source().unwrap().unwrap().eligible);
                    // The SQLite query has returned; only lifecycle ownership is held.
                    entered.send(()).unwrap();
                    let _ = resume.recv();
                })
                .unwrap();
        });
        held.await.unwrap();
        let mut owner = Box::pin(crate::local_runtime::session_title::owner(
            Arc::downgrade(&f.manager.sessions.catalog),
            f.sessions[0].id.clone(),
            &runtime,
            runtime.title_metadata_changes_for_test(),
            runtime.title_capacity_changes(),
        ));
        let reads = runtime.count_title_reads_for_test();
        let capacity = runtime.title_capacity_changes();
        std::future::poll_fn(|context| {
            assert!(owner.as_mut().poll(context).is_pending());
            match reads.load(Ordering::SeqCst) {
                0 => std::task::Poll::Pending, // Catalog publication may precede the source read.
                1 => std::task::Poll::Ready(()),
                count => panic!("repeated source reads while lifecycle is held: {count}"),
            }
        })
        .await;
        assert!(
            !events(&runtime)
                .iter()
                .any(|event| matches!(event, RuntimeEvent::AuxiliaryCallStarted { .. })),
            "no durable Call before lifecycle admission"
        );

        let process = crate::runtime::auxiliary::capacity::ProcessCapacity::shared_process();
        let other = process.conversation().reserve_general().unwrap();
        drop(other);
        assert!(capacity.has_changed().unwrap());
        assert!(futures_util::poll!(owner.as_mut()).is_pending());
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "an unrelated coalesced capacity wake cannot resolve lifecycle contention"
        );

        drop(release);
        holder.join().unwrap();
        let worker = tokio::spawn(owner);
        f.gates[1].wait_entered().await;
        f.gates[1].release();
        worker.await.unwrap();
        assert_eq!(
            reads.load(Ordering::SeqCst),
            2,
            "release permits exactly one new authoritative check"
        );
        assert!(
            f.manager
                .sessions
                .read_session_summary(&f.sessions[0].id)
                .await
                .unwrap()
                .generated_title
                .is_some()
        );
        assert_eq!(f.provider.request_bodies().len(), 1);
        assert_eq!(
            events(&runtime)
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::AuxiliaryCallStarted { .. }))
                .count(),
            1
        );
        drop(managed);
        f.close().await;
    })
    .await;
}
