//! Whole-child execution deadline regressions: timer start after ownership,
//! deterministic terminal winners, and frozen prepared deadlines.

use super::*;

/// Issue #191/#192 — ordinary explicit cancellation and deadline
/// cancellation are the same `Running -> Cancelling -> Cancelled`
/// lifecycle, but the committed reason keeps them distinguishable in
/// the authoritative snapshot and in the runtime-authored notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_and_deadline_cancellation_keep_their_distinct_committed_reasons() {
    let plane = plane(4);

    // Ordinary explicit cancellation.
    let explicit_child = stage_stubborn(&plane);
    let explicit = start(&plane, &start_spec("explicit cancel")).await;
    plane
        .registry
        .cancel(&explicit.subagent_id, CancellationReason::UserRequested)
        .expect("known child");
    explicit_child
        .cancelled_after_delegation(ChildResultStatus::Cancelled, None)
        .await;
    let explicit_settled = plane
        .registry
        .wait_until_settled(&explicit.subagent_id)
        .await
        .expect("settled");
    assert_eq!(explicit_settled.state, SubagentState::Cancelled);
    assert_eq!(
        explicit_settled.cancel_reason,
        Some(CancellationReason::UserRequested)
    );

    // Deadline expiry: the same ordinary cancellation model, driven by
    // the manual clock, with the deadline reason. The completion latch
    // is registered before the start commits, against the
    // deterministic second-child identity.
    let deadline_child = stage_stubborn(&plane);
    let deadline_id = SubagentId::for_conversation(&plane.conversation_id, 2);
    let deadline_done = plane.registry.watch_deadline_completion(&deadline_id);
    let deadline = start(&plane, &deadline_spec("deadline", 100)).await;
    assert_eq!(deadline.subagent_id, deadline_id);
    plane.monotonic_clock.advance(100);
    deadline_done.await.expect("the deadline cancellation ran");
    deadline_child
        .cancelled_after_delegation(ChildResultStatus::Cancelled, None)
        .await;
    let deadline_settled = plane
        .registry
        .wait_until_settled(&deadline.subagent_id)
        .await
        .expect("settled");
    assert_eq!(deadline_settled.state, SubagentState::Cancelled);
    assert_eq!(
        deadline_settled.cancel_reason,
        Some(CancellationReason::SubagentExecutionDeadlineExceeded)
    );

    // The parent-facing notices keep the same distinction.
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one pending batch");
    assert_eq!(pending.items.len(), 2, "one runtime notice per child");
    let texts = pending
        .items
        .iter()
        .map(|item| {
            assert!(matches!(
                item.message.source,
                crate::message::types::UserSource::Runtime
            ));
            match &item.message.content[0] {
                crate::message::types::UserContentBlock::Text(text) => text.text.clone(),
                other => panic!("a cancellation notice is text: {other:?}"),
            }
        })
        .collect::<Vec<_>>();
    assert!(texts[0].contains("requested by the user"), "{}", texts[0]);
    assert!(
        texts[1].contains("the subagent execution deadline expired"),
        "{}",
        texts[1]
    );
    assert!(
        !texts.iter().any(|text| text.contains("timed out")),
        "there is no TimedOut lifecycle: {texts:?}"
    );
}

/// The deadline has no authority during prepare or the durable ownership
/// critical section. Once ownership commits, advancing the manual clock
/// drives the ordinary cancellation path and the child still settles
/// through its driver and one terminal publication.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn deadline_starts_only_after_ownership_commit_and_settles_once() {
    let plane = plane(4);
    let hook = Arc::new(CommitBoundaryHook::default());
    plane.registry.install_commit_boundary_hook(hook.clone());
    let mut child = stage_exit0(&plane);
    let registry = plane.registry.clone();
    let spec = deadline_spec("deadline after commit", 100);
    let committer = tokio::spawn(async move {
        let prepared = registry
            .prepare(&spec, &CancellationSignal::new())
            .await
            .expect("prepared");
        registry.commit(prepared, &CancellationSignal::new()).await
    });

    hook.wait_until_entered();
    // The ownership mutex is parked before the durable ownership event.
    // No deadline task exists, so elapsed manual time cannot create a
    // record or a cancellation fact here.
    plane.monotonic_clock.advance(1_000);
    assert!(events(&plane).is_empty());

    hook.release();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
        .await
        .expect("commit liveness")
        .expect("committer")
        .expect("commit succeeds");
    let accepted = match outcome {
        SubagentStartOutcome::Accepted(accepted) => accepted,
        SubagentStartOutcome::RolledBack => panic!("ownership already committed"),
    };
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("accepted snapshot")
            .state,
        SubagentState::Running
    );
    assert_eq!(
        events(&plane)
            .iter()
            .filter(|event| matches!(
                event,
                crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                    subagent_id, ..
                } if *subagent_id == accepted.subagent_id
            ))
            .count(),
        1,
        "the deadline begins only after one durable ownership fact"
    );

    // The deadline was sampled at manual time 1_000 during ownership.
    // Reaching 1_100 now fires it, deterministically and without a wall
    // clock wait.
    plane.monotonic_clock.advance(100);
    let cancelling = wait_for_cancelling(&plane, &accepted.subagent_id).await;
    assert_eq!(cancelling.detail, None);
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
    assert!(matches!(
        child.read_frame().await,
        ParentFrame::Cancel {
            reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
        }
    ));
    // The staged test child has already exited; closing the controlled
    // protocol endpoint lets the ordinary driver prove settlement.
    drop(child.peer);

    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert_eq!(
        settled.detail.as_deref(),
        Some("the subagent execution deadline expired")
    );
    let pending = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("deadline notice");
    assert!(matches!(
        pending.items[0].message.source,
        crate::message::types::UserSource::Runtime
    ));
    let content = match &pending.items[0].message.content[0] {
        crate::message::types::UserContentBlock::Text(text) => &text.text,
        other => panic!("unexpected deadline notice content: {other:?}"),
    };
    assert!(content.contains("the subagent execution deadline expired"));
    assert_eq!(
        published_terminal_states(&plane, &accepted.subagent_id),
        vec![SubagentTerminalState::Cancelled]
    );
}

/// Both registry orderings of a deadline versus a physical success are
/// forced at the actual linearization boundary, with both contenders
/// genuinely live:
///
/// - **Deadline wins**: the child's success result is fully processed
///   physically, so terminal authority is a real contender parked by
///   the terminal-authority hook immediately before its
///   `... -> PublishingTerminal` commit; only then does the manual
///   clock fire the deadline, which commits `Running -> Cancelling`;
///   the released settlement observes the committed cancellation
///   authority and must publish one canonical `Cancelled` with the
///   deadline reason — never the success.
/// - **Terminal authority wins**: the manual clock fires the deadline,
///   which parks by the cancellation-boundary hook immediately before
///   its `Running -> Cancelling` commit; while parked, the terminal
///   settlement acquires the registry critical section and commits one
///   `Succeeded`; the released deadline's cancellation call returns as
///   a no-op.
///
/// Every interleaving is driven by hook/barrier synchronization plus
/// the manual monotonic clock — never by wall-clock sleeps — and the
/// published snapshot sequence is recorded exactly, proving one
/// observable `Running -> Cancelling` transition (or none) and one
/// final terminal publication.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn deadline_and_success_have_one_deterministic_terminal_winner() {
    // Ordering 1: the fired deadline wins the registry linearization
    // over a terminal-success contender parked immediately before its
    // terminal-authority commit.
    let deadline_wins_plane = plane(4);
    let terminal_authority = Arc::new(TerminalAuthorityHook::default());
    deadline_wins_plane
        .registry
        .install_terminal_authority_hook(terminal_authority.clone());
    let recorded = recording_observer(&deadline_wins_plane);
    let mut child = stage_exit0(&deadline_wins_plane);
    let accepted = start(
        &deadline_wins_plane,
        &deadline_spec("deadline wins the authority race", 100),
    )
    .await;
    assert_eq!(
        recorded.states(),
        vec![SubagentState::Stopping, SubagentState::Running]
    );
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));

    // The child's success result is consumed and every physical
    // boundary is settled; terminal authority is now a real contender
    // parked immediately before its `... -> PublishingTerminal` commit.
    // Terminal state is provably NOT committed yet.
    child
        .send_result(ChildResultStatus::Succeeded, Some("success contender"))
        .await;
    terminal_authority.wait_until_entered();
    assert_eq!(
        deadline_wins_plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("running snapshot")
            .state,
        SubagentState::Running,
        "the terminal contender is parked before its commit; Running is not yet Cancelling"
    );
    assert_eq!(
        recorded.states(),
        vec![SubagentState::Stopping, SubagentState::Running]
    );

    // Fire the manual deadline while terminal authority is parked: the
    // deadline contender acquires the registry mutex first and commits
    // `Running -> Cancelling` with its typed reason.
    deadline_wins_plane.monotonic_clock.advance(100);
    let cancelling = wait_for_cancelling(&deadline_wins_plane, &accepted.subagent_id).await;
    assert_eq!(cancelling.state, SubagentState::Cancelling);
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Cancelling
        ],
        "exactly one observable Running -> Cancelling transition"
    );

    // Release the paused terminal contender: it now observes the
    // committed cancellation authority under the same registry mutex
    // and cannot publish success.
    terminal_authority.release();
    let settled = deadline_wins_plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("deadline cancellation settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert_eq!(
        settled.detail.as_deref(),
        Some("the subagent execution deadline expired"),
        "the winning deadline reason is preserved"
    );
    assert_eq!(
        published_terminal_states(&deadline_wins_plane, &accepted.subagent_id),
        vec![SubagentTerminalState::Cancelled],
        "exactly one final terminal result, never the racing success"
    );
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Cancelling,
            SubagentState::Cancelled
        ],
        "one cancellation transition and one final terminal state"
    );
    // No timer survives terminal authority: further elapsed time is
    // inert and produces no extra publication.
    deadline_wins_plane.monotonic_clock.advance(100_000);
    assert_eq!(
        deadline_wins_plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("cancelled snapshot")
            .state,
        SubagentState::Cancelled
    );
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Cancelling,
            SubagentState::Cancelled
        ]
    );
    // The physical child was already settled when the deadline fired,
    // so the driver wire carries no cancellation frame at all.
    drain_control_frames(&mut child).await;
    drop(child.peer);

    // Ordering 2: terminal authority wins while the fired deadline is
    // parked immediately before its cancellation commit.
    let plane = plane(4);
    let cancellation_boundary = Arc::new(CancellationBoundaryHook::default());
    plane
        .registry
        .install_cancellation_boundary_hook(cancellation_boundary.clone());
    let recorded = recording_observer(&plane);
    let mut child = stage_exit0(&plane);
    // Register the fired-deadline completion latch before the child
    // starts: the owning commit claims it when it creates the deadline
    // task (the first child of a fresh plane is ordinal 1).
    let deadline_done = plane
        .registry
        .watch_deadline_completion(&SubagentId::for_conversation(&plane.conversation_id, 1));
    let accepted = start(&plane, &deadline_spec("terminal authority wins", 100)).await;
    assert_eq!(
        recorded.states(),
        vec![SubagentState::Stopping, SubagentState::Running]
    );
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));

    // Fire the manual deadline: the fired deadline task is now a live
    // cancellation contender parked immediately before its
    // `Running -> Cancelling` commit. The mutation has NOT occurred.
    plane.monotonic_clock.advance(100);
    cancellation_boundary.wait_until_parked();
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("running snapshot")
            .state,
        SubagentState::Running,
        "the deadline contender is parked before its commit"
    );
    assert_eq!(
        recorded.states(),
        vec![SubagentState::Stopping, SubagentState::Running]
    );

    // While the deadline contender is parked, terminal settlement
    // acquires the registry critical section and commits one
    // `PublishingTerminal`/`Succeeded`.
    child
        .send_result(ChildResultStatus::Succeeded, Some("terminal wins"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("success settled");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(
        settled.detail, None,
        "no cancellation reason replaces the success"
    );
    assert_eq!(
        published_terminal_states(&plane, &accepted.subagent_id),
        vec![SubagentTerminalState::Succeeded]
    );
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Succeeded
        ],
        "terminal authority won; no Cancelling was ever observable"
    );

    // Release the parked deadline contender: its cancellation call
    // returns as a no-op over a terminal record.
    cancellation_boundary.release();
    tokio::time::timeout(std::time::Duration::from_secs(10), deadline_done)
        .await
        .expect("fired deadline liveness")
        .expect("the fired deadline's cancellation call returned");
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("succeeded snapshot")
            .state,
        SubagentState::Succeeded
    );
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Succeeded
        ],
        "the released deadline produced no second transition"
    );
    assert_eq!(
        published_terminal_states(&plane, &accepted.subagent_id),
        vec![SubagentTerminalState::Succeeded],
        "no extra terminal publication"
    );
    // No timer effect after terminal authority, and no driver
    // cancellation ever reached the wire.
    plane.monotonic_clock.advance(100_000);
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("succeeded snapshot")
            .state,
        SubagentState::Succeeded
    );
    drain_control_frames(&mut child).await;
    drop(child.peer);
}

/// Both orderings of the deadline-versus-explicit-cancellation race are
/// forced at the actual `Running -> Cancelling` linearization boundary,
/// with both cancellation sources genuinely live:
///
/// - **Deadline wins**: explicit cancellation is spawned first and
///   parks by the cancellation-boundary hook immediately before the
///   commit; the manual clock then fires the deadline, which reaches
///   the same cancellation authority and is allowed to commit first;
///   the released explicit caller observes the committed reason and
///   cannot overwrite it.
/// - **Explicit wins**: the manual clock fires the deadline first and
///   it parks immediately before the commit; explicit cancellation is
///   then allowed to commit `UserRequested`; the released deadline
///   cannot overwrite the reason.
///
/// Each ordering proves exactly one observable `Running -> Cancelling`
/// transition, exactly one effective `Cancel` frame on the driver
/// wire, one winning reason preserved into the terminal detail, and
/// one canonical terminal publication. Synchronization is hook/barrier
/// driven with the manual monotonic clock — never wall-clock sleeps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn deadline_and_explicit_cancellation_preserve_the_first_reason() {
    // Ordering 1: the fired deadline wins; explicit cancellation is the
    // parked loser.
    let deadline_wins_plane = plane(4);
    let cancellation_boundary = Arc::new(CancellationBoundaryHook::default());
    deadline_wins_plane
        .registry
        .install_cancellation_boundary_hook(cancellation_boundary.clone());
    let recorded = recording_observer(&deadline_wins_plane);
    let mut child = stage_exit0(&deadline_wins_plane);
    let accepted = start(
        &deadline_wins_plane,
        &deadline_spec("deadline wins the cancel race", 100),
    )
    .await;
    assert_eq!(
        recorded.states(),
        vec![SubagentState::Stopping, SubagentState::Running]
    );
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));

    // Explicit cancellation is a live contender first: spawned and
    // parked immediately before the `Running -> Cancelling` commit.
    let explicit_registry = deadline_wins_plane.registry.clone();
    let explicit_id = accepted.subagent_id.clone();
    let explicit_cancel = tokio::spawn(async move {
        explicit_registry.cancel(&explicit_id, CancellationReason::UserRequested)
    });
    cancellation_boundary.wait_until_parked();
    assert_eq!(
        deadline_wins_plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("running snapshot")
            .state,
        SubagentState::Running,
        "the explicit contender is parked before its commit"
    );
    assert_eq!(
        recorded.states(),
        vec![SubagentState::Stopping, SubagentState::Running]
    );

    // Fire the manual deadline: it reaches the same cancellation
    // authority and is allowed to acquire/commit first.
    deadline_wins_plane.monotonic_clock.advance(100);
    let cancelling = wait_for_cancelling(&deadline_wins_plane, &accepted.subagent_id).await;
    assert_eq!(cancelling.state, SubagentState::Cancelling);
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Cancelling
        ],
        "exactly one observable Running -> Cancelling transition"
    );
    assert!(
        matches!(
            child.read_frame().await,
            ParentFrame::Cancel {
                reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
            }
        ),
        "exactly one effective Cancel frame, with the deadline reason"
    );

    // Release the parked explicit loser: it observes the committed
    // Cancelling and cannot replace the reason or send a second driver
    // command.
    cancellation_boundary.release();
    let losing_explicit = tokio::time::timeout(std::time::Duration::from_secs(10), explicit_cancel)
        .await
        .expect("explicit cancel liveness")
        .expect("explicit cancel task")
        .expect("known cancelling child");
    assert_eq!(losing_explicit.state, SubagentState::Cancelling);
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Cancelling
        ],
        "the losing explicit caller produced no transition"
    );

    // The late child result settles one canonical Cancelled preserving
    // the winning deadline reason.
    child
        .send_result(ChildResultStatus::Succeeded, Some("late success"))
        .await;
    let settled = deadline_wins_plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("deadline cancellation settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert_eq!(
        settled.detail.as_deref(),
        Some("the subagent execution deadline expired"),
        "the deadline reason is preserved; UserRequested never replaces it"
    );
    assert_eq!(
        published_terminal_states(&deadline_wins_plane, &accepted.subagent_id),
        vec![SubagentTerminalState::Cancelled],
        "one canonical terminal publication"
    );
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Cancelling,
            SubagentState::Cancelled
        ]
    );
    // One driver cancellation only: no second Cancel frame ever reached
    // the wire.
    drain_control_frames(&mut child).await;
    drop(child.peer);

    // Ordering 2: explicit cancellation wins; the fired deadline is the
    // parked loser.
    let plane = plane(4);
    let cancellation_boundary = Arc::new(CancellationBoundaryHook::default());
    plane
        .registry
        .install_cancellation_boundary_hook(cancellation_boundary.clone());
    let recorded = recording_observer(&plane);
    let mut child = stage_exit0(&plane);
    let deadline_done = plane
        .registry
        .watch_deadline_completion(&SubagentId::for_conversation(&plane.conversation_id, 1));
    let accepted = start(&plane, &deadline_spec("explicit wins the cancel race", 100)).await;
    assert_eq!(
        recorded.states(),
        vec![SubagentState::Stopping, SubagentState::Running]
    );
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));

    // Fire the manual deadline: the fired deadline task is a live
    // cancellation contender parked immediately before the commit.
    plane.monotonic_clock.advance(100);
    cancellation_boundary.wait_until_parked();
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("running snapshot")
            .state,
        SubagentState::Running,
        "the deadline contender is parked before its commit"
    );
    assert_eq!(
        recorded.states(),
        vec![SubagentState::Stopping, SubagentState::Running]
    );

    // Explicit cancellation is allowed to commit first (it passes the
    // parked deadline contender).
    let cancelling = plane
        .registry
        .cancel(&accepted.subagent_id, CancellationReason::UserRequested)
        .expect("known child");
    assert_eq!(cancelling.state, SubagentState::Cancelling);
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Cancelling
        ],
        "exactly one observable Running -> Cancelling transition"
    );
    assert!(
        matches!(
            child.read_frame().await,
            ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested)
            }
        ),
        "exactly one effective Cancel frame, with the explicit reason"
    );

    // Release the parked deadline loser: it cannot overwrite the
    // committed UserRequested reason.
    cancellation_boundary.release();
    tokio::time::timeout(std::time::Duration::from_secs(10), deadline_done)
        .await
        .expect("fired deadline liveness")
        .expect("the fired deadline's cancellation call returned");
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Cancelling
        ],
        "the released deadline produced no second transition"
    );

    // The late child result settles one canonical Cancelled preserving
    // the winning UserRequested reason.
    child
        .send_result(ChildResultStatus::Succeeded, Some("late success"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("explicit cancellation settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert_eq!(
        settled.detail.as_deref(),
        Some("requested by the user"),
        "the explicit reason is preserved; the deadline never replaces it"
    );
    assert_eq!(
        published_terminal_states(&plane, &accepted.subagent_id),
        vec![SubagentTerminalState::Cancelled],
        "one canonical terminal publication"
    );
    assert_eq!(
        recorded.states(),
        vec![
            SubagentState::Stopping,
            SubagentState::Running,
            SubagentState::Cancelling,
            SubagentState::Cancelled
        ]
    );
    // Advancing far past the deadline after the winner committed proves
    // the timer cannot fire again; one driver cancellation only.
    plane.monotonic_clock.advance(100_000);
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("cancelled snapshot")
            .state,
        SubagentState::Cancelled
    );
    drain_control_frames(&mut child).await;
    drop(child.peer);
}

/// The controlled child is held after `Delegate`, representing an active
/// model/tool pipeline with no terminal result yet. Deadline expiry sends
/// the typed cancel through the ordinary driver command channel; the
/// driver then consumes the late result and performs physical settlement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deadline_during_delegated_activity_uses_driver_settlement() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let accepted = start(&plane, &deadline_spec("active model and tool work", 100)).await;
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
    plane.monotonic_clock.advance(100);
    let cancelling = wait_for_cancelling(&plane, &accepted.subagent_id).await;
    assert_eq!(cancelling.state, SubagentState::Cancelling);
    let cancel = child.read_frame().await;
    assert!(matches!(
        cancel,
        ParentFrame::Cancel {
            reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
        }
    ));
    child
        .send_result(ChildResultStatus::Succeeded, Some("late active result"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("driver physically settled the child");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert_eq!(
        settled.detail.as_deref(),
        Some("the subagent execution deadline expired")
    );
    assert_eq!(
        published_terminal_states(&plane, &accepted.subagent_id),
        vec![SubagentTerminalState::Cancelled]
    );
    drop(child.peer);
}

/// Terminal candidate creation and deadline invalidation are committed
/// under the registry mutex. The durable terminal event is therefore the
/// final lifecycle event, and advancing the clock after settlement cannot
/// resurrect the deadline task.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deadline_terminal_publication_is_final_and_timer_is_cleaned_up() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let accepted = start(&plane, &deadline_spec("terminal ordering", 100)).await;
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
    plane.monotonic_clock.advance(100);
    wait_for_cancelling(&plane, &accepted.subagent_id).await;
    assert!(matches!(
        child.read_frame().await,
        ParentFrame::Cancel {
            reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
        }
    ));
    child
        .send_result(ChildResultStatus::Succeeded, Some("late terminal result"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Cancelled);

    let journal = events(&plane);
    let terminal_positions = journal
        .iter()
        .enumerate()
        .filter_map(|(position, event)| {
            matches!(
                event,
                crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                    subagent_id, ..
                } if *subagent_id == accepted.subagent_id
            )
            .then_some(position)
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_positions, vec![journal.len() - 1]);
    assert_eq!(
        published_terminal_states(&plane, &accepted.subagent_id),
        vec![SubagentTerminalState::Cancelled]
    );
    let event_count = journal.len();
    plane
        .monotonic_clock
        .advance(super::super::super::catalog::MAX_SUBAGENT_EXECUTION_DEADLINE_MS);
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("terminal snapshot")
            .state,
        SubagentState::Cancelled
    );
    assert_eq!(events(&plane).len(), event_count);
    drop(child.peer);
}

/// `prepare` owns a copy of the resolved deadline. Mutating the caller's
/// source specification after preparation cannot change the admitted
/// launch's firing point.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prepared_deadline_remains_frozen_after_source_spec_changes() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let mut source_spec = deadline_spec("frozen deadline", 100);
    let prepared = plane
        .registry
        .prepare(&source_spec, &CancellationSignal::new())
        .await
        .expect("prepared");
    assert_eq!(
        prepared
            .execution_deadline
            .expect("prepared deadline")
            .as_millis(),
        100
    );
    source_spec.authority.resolved.execution_deadline =
        Some(SubagentExecutionDeadline::from_millis(1_000).expect("valid replacement deadline"));
    let accepted = match plane
        .registry
        .commit(prepared, &CancellationSignal::new())
        .await
        .expect("commit")
    {
        SubagentStartOutcome::Accepted(accepted) => accepted,
        SubagentStartOutcome::RolledBack => panic!("commit unexpectedly rolled back"),
    };
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
    plane.monotonic_clock.advance(100);
    wait_for_cancelling(&plane, &accepted.subagent_id).await;
    assert!(matches!(
        child.read_frame().await,
        ParentFrame::Cancel {
            reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
        }
    ));
    child
        .send_result(ChildResultStatus::Succeeded, Some("late frozen result"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert_eq!(
        settled.detail.as_deref(),
        Some("the subagent execution deadline expired")
    );
    drop(child.peer);
}
