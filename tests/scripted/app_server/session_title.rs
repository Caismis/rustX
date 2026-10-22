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
