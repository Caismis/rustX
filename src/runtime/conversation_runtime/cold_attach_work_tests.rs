//! Reopening a completed conversation must not read its retained history.
//!
//! Each fixture appends completed turns through the production append path,
//! closes the store, and then measures every SQL statement a cold reopen
//! executes: a fresh store, runtime construction with startup recovery and
//! subagent registry restoration, and the Runtime Client bootstrap cut. The work must not change with history.

use super::test_support::*;
use super::*;
use crate::durable::{CanonicalStore, SqliteConversationStore};

struct ColdAttach {
    rows: usize,
    steps: i64,
    statements: crate::durable::sqlite::MeasuredStatements,
}

async fn cold_attach(id: &ConversationId, path: &std::path::Path) -> ColdAttach {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteConversationStore::open(id.clone(), path).unwrap());
    let statements = {
        let _oracle = crate::runtime::recovery::suspend_frontier_oracle();
        store.begin_query_work();
        // A conversation-owned subagent registry restores its durable
        // Agent records during construction, as in the App Server.
        let (runtime, _, _registry) =
            headless_runtime_over_store_with_subagents(&dir, id.as_str(), store.clone(), None)
                .await;
        runtime
            .install_client_observation_bridge(Arc::new(PendingObservations::new()))
            .unwrap();
        let statements = store.finish_query_work();
        // Attach left the active working set in the durable store; its
        // first consumer hydrates exactly the durable Surface head.
        assert!(matches!(
            runtime.inner.state.lock().unwrap().conversation,
            super::conversation_slot::ConversationSlot::Durable
        ));
        assert_eq!(
            runtime.coordinator_active_ids().unwrap(),
            store.load_head().unwrap().active_message_ids
        );
        drop(runtime);
        statements
    };
    // The debug oracle recomputes the whole Journal fold and checks the
    // persisted frontier against it on the same history.
    crate::runtime::recovery::RecoveryEvidence::reconstruct(store.as_ref()).unwrap();
    ColdAttach {
        rows: statements.values().map(|work| work.1).sum(),
        steps: statements.values().map(|work| work.2).sum(),
        statements,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cold_attach_work_does_not_grow_with_history() {
    for (tool, sizes) in [(false, &[64, 512, 2048][..]), (true, &[64, 512][..])] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let id = ConversationId::generate();
        let mut appended = 0;
        let mut samples: Vec<(usize, ColdAttach)> = Vec::new();
        for &size in sizes {
            {
                let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
                if appended == 0 {
                    store.initialize(&[]).unwrap();
                }
                crate::durable::sqlite::append_completed_turns(
                    &store,
                    appended,
                    size - appended,
                    tool,
                );
                appended = size;
            }
            samples.push((size, cold_attach(&id, &path).await));
        }
        let (_, baseline) = &samples[0];
        for (size, sample) in &samples {
            println!(
                "tool={tool} turns={size}: rows={} steps={} statements={}",
                sample.rows,
                sample.steps,
                sample.statements.len()
            );
        }
        for (size, sample) in &samples[1..] {
            let grown: Vec<_> = sample
                .statements
                .iter()
                .filter(|(sql, work)| {
                    baseline
                        .statements
                        .get(*sql)
                        .is_none_or(|base| base.1 != work.1 || base.2 != work.2)
                })
                .map(|(sql, work)| (work, baseline.statements.get(sql), sql))
                .collect();
            assert!(
                sample.rows.abs_diff(baseline.rows) <= 8
                    && sample.steps.abs_diff(baseline.steps) <= 256,
                "tool={tool} turns={size}: rows {} vs {}, steps {} vs {}; changed: {grown:#?}",
                sample.rows,
                baseline.rows,
                sample.steps,
                baseline.steps,
            );
        }
    }
}
