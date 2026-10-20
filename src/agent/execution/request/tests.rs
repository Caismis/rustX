use crate::agent::cancellation::AgentCancellation;
use crate::agent::execution::AgentExecution;
use crate::agent::execution::test_fixtures::*;
use crate::context::ContextRuntime;
use crate::conversation::ConversationState;
use crate::durable::inbox::ConversationStore;
use crate::events::types::AttemptOutcome;
use crate::message::types::{MessageBlock, UserContentBlock, UserSource};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{AttemptId, ConversationId};
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime, tool_runtime_with_store};
use crate::scripted_suites::support::fake::inbound_message;
use crate::tools::executor::ToolRegistry;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::watch;

/// A contributor whose bounded work is explicitly held at an awaited
/// boundary. The test releases it only after cancellation is observable,
/// proving that the final admission check—not the contributor's private
/// synchronization—decides whether transient proposals become facts.
struct AwaitingContributor {
    entered: watch::Sender<bool>,
    release: watch::Receiver<bool>,
    invocations: Arc<std::sync::atomic::AtomicUsize>,
}

impl crate::context::ContextContributor for AwaitingContributor {
    fn contribute<'a>(
        &'a self,
        _input: &'a crate::context::ContributorInputSnapshot,
    ) -> BoxFuture<
        'a,
        Result<Vec<crate::context::ContextProposal>, crate::context::ContextAssemblyError>,
    > {
        self.invocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.entered.send_replace(true);
        let mut release = self.release.clone();
        Box::pin(async move {
            release
                .wait_for(|released| *released)
                .await
                .expect("contributor release channel stays open");
            Ok(vec![crate::context::ContextProposal::UserMessage(
                crate::context::UserMessageProposal {
                    content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                        text: "awaited proposal".to_owned(),
                    })],
                },
            )])
        })
    }
}

/// Cancellation may become observable while a contributor is awaiting
/// bounded work. The contributor still settles its transient proposal,
/// then the one start arbitration linearizes cancellation before any
/// canonical context, Surface revision, snapshot, or provider request
/// exists (Issue #12, M9b).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_awaited_context_assembly_commits_nothing() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }]]));
    let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (entered, mut entered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    let mut assembly = crate::context::ContextAssembly::new();
    assembly
        .register_extension(
            "awaited.test",
            Some("package-v1".to_owned()),
            Arc::new(AwaitingContributor {
                entered,
                release: release_rx,
                invocations: Arc::clone(&invocations),
            }),
        )
        .expect("register awaited contributor");
    let request = request(&adapter);
    let runtime = ContextRuntime::for_attempt_with_assembly(
        crate::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(crate::context::DefaultTokenEstimator),
        Some(crate::context::AgentStatusEngine::default()),
        assembly,
        &request.model,
        crate::model::ModelTimeoutPolicy::default(),
        crate::scripted_suites::support::default_monotonic_clock(),
    )
    .expect("valid context runtime");
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        entered_rx
            .wait_for(|entered| *entered)
            .await
            .expect("awaited contributor entered");
        controller_cancellation.cancel();
        release.send_replace(true);
    });
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let result = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    controller.await.expect("cancellation controller");
    let snapshots = request_snapshot_history(tool_runtime.durable_store().as_ref());

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the awaited contributor is invoked once"
    );
    assert!(matches!(
        result.outcome,
        crate::events::types::AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert!(snapshots.is_empty());
    assert_eq!(
        result.conversation.revision(),
        crate::conversation::SurfaceRevision::INITIAL
    );
    assert!(result.messages().is_empty());
    assert_eq!(adapter.request_count(), 0);
}

/// Capacity suite: this test's value comes from its scale, so FAST skips
/// `::scale::` and FULL runs it (tests/README.md, "Capacity tests").
mod scale {
    use super::*;

    /// Exercise real preparation, native start, publication and provider dispatch after
    /// a warmed historical request. Only the explicit final oracle discloses history.
    #[tokio::test]
    async fn full_agent_plain_text_append_reuses_native_input_at_100_300_1000() {
        for count in [100usize, 300, 1000] {
            let adapter = Arc::new(ScriptedAdapter::new(vec![
                workflow_output_turn(&[], Some("warm original response")),
                workflow_output_turn(&[], Some("new original response")),
            ]));
            let store = Arc::new(
                crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                ))
                .unwrap(),
            );
            let tool_runtime = tool_runtime_with_store(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                Some(store.clone()),
            );
            let canonical: Vec<_> = (0..count)
                .map(|index| {
                    MessageBlock::User(inbound_message(
                        &format!("full-chain-{index}"),
                        &format!("original 水🙂 {index}"),
                        UserSource::Human,
                    ))
                })
                .collect();
            let mut first = request(&adapter);
            first.conversation = ConversationState::from_messages(canonical.clone()).unwrap();
            let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
                .await
                .into_lease();
            let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
            let warm = AgentExecution::new(
                first,
                lease,
                &cancellation,
                crate::scripted_suites::support::default_execution_policy(),
                runtime(&adapter),
                &tool_runtime,
                crate::agent::AttemptLifecycle::inert(),
            )
            .unwrap()
            .run()
            .await;
            assert!(matches!(warm.outcome, AttemptOutcome::Completed { .. }));
            let mut second = request(&adapter);
            second.attempt_id = AttemptId::new("full-chain-second");
            second.conversation = warm.conversation;
            let delta = MessageBlock::User(inbound_message(
                "full-chain-new-user",
                "new exact input",
                UserSource::Human,
            ));
            store.append_canonical(&delta).unwrap();
            second.conversation.commit(delta.clone()).unwrap();
            let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
                .await
                .into_lease();
            crate::model::selected_input::take_preparation_visits();
            crate::model::selected_input::take_materialized_items();
            crate::model::selected_input::take_hash_work();
            crate::durable::sqlite::take_request_input_reconstructions();
            let execution = AgentExecution::new(
                second,
                lease,
                &cancellation,
                crate::scripted_suites::support::default_execution_policy(),
                runtime(&adapter),
                &tool_runtime,
                crate::agent::AttemptLifecycle::inert(),
            )
            .unwrap();
            let result = execution.run().await;
            let visits = crate::model::selected_input::take_preparation_visits();
            let materialized = crate::model::selected_input::take_materialized_items();
            let hashes = crate::model::selected_input::take_hash_work();
            let reconstruction = crate::durable::sqlite::take_request_input_reconstructions();
            assert!(
                matches!(result.outcome, AttemptOutcome::Completed { .. }),
                "{count}: {:?}",
                result.outcome
            );
            assert_eq!(adapter.request_count(), 2);
            assert!(
                visits < 64,
                "native preparation visits only actualdelta, not oldhistory at {count}: {visits}"
            );
            assert_eq!(
                materialized, 0,
                "shared native input is never flattened during preparation at {count}"
            );
            assert_eq!(
                reconstruction,
                (0, 0),
                "commit evidence replaces a second historical Store reconstruction"
            );
            assert!(
                hashes.0 < 16_384 && hashes.1 < 16_384,
                "only genuine new input is hashed at {count}: {hashes:?}"
            );
            assert_full_agent_input_oracle(store.as_ref(), &adapter, &canonical, &delta, count);
        }
    }
}

fn assert_full_agent_input_oracle(
    store: &crate::durable::SqliteConversationStore,
    adapter: &Arc<ScriptedAdapter>,
    canonical: &[MessageBlock],
    delta: &MessageBlock,
    count: usize,
) {
    let requests = adapter.requests();
    let actual = requests.last().unwrap();
    assert_eq!(
        actual.messages.range(0..count).to_vec(),
        crate::model::input::canonical_input(canonical)
    );
    assert!(
        actual
            .messages
            .iter()
            .any(|item| item.as_canonical() == Some(delta))
    );
    let snapshots = store.read_request_snapshots(None, 16).unwrap().snapshots;
    let snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.identity.attempt_id == AttemptId::new("full-chain-second"))
        .unwrap();
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        *actual
    );
    let mut after = None;
    let mut terminals = 0;
    loop {
        let page = store.read_events(after, 32).unwrap();
        if page.events.is_empty() {
            break;
        }
        terminals += page
            .events
            .iter()
            .filter(|event| {
                event.attempt_id.as_ref() == Some(&AttemptId::new("full-chain-second"))
                    && AttemptOutcome::from_terminal_event(&event.event).is_some()
            })
            .count();
        assert!(page.next_sequence > after);
        after = page.next_sequence;
    }
    assert_eq!(terminals, 1);
}
