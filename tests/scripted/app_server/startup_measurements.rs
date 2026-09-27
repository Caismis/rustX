//! Opt-in native benchmark, identical on the issue's reference and implementation.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "isolated startup measurement; run explicitly with --ignored --nocapture"]
async fn issue419_measure_native_cold_load() {
    for sample in 1..=3 {
        let f = Fixture::with_session_count(None, 0).await;
        let start = std::time::Instant::now();
        let created = f
            .manager
            .create_session(SessionPersistentState {
                cwd: f.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap();
        let create_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
        let attach = std::time::Instant::now();
        let runtime = f.manager.load(&created.session.id, None).await.unwrap();
        let load_ms = attach.elapsed().as_secs_f64() * 1000.0;
        assert!(runtime.client().snapshot().unwrap().0.model.is_some());
        println!(
            "NATIVE_STARTUP sample={sample} session={} create_ms={create_ms:.3} cold_load_ready_ms={load_ms:.3}",
            created.session.id
        );
        f.manager
            .unload(&created.session.active_conversation_id)
            .await
            .unwrap();
    }
}
