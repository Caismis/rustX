//! Native waiter-order regressions. Registration and pre-wait gates establish
//! the interleaving; neither spawn order nor timers are ordering evidence.
use super::*;

type Waiting = tokio::task::JoinHandle<Result<SubagentStartOutcome, SubagentStartError>>;

async fn prepared(plane: &TestPlane, name: &str) -> (PreparedSubagent, ScriptedChild) {
    let child = stage_exit0(plane);
    let prepared = plane
        .registry
        .prepare(&start_spec(name), &CancellationSignal::new())
        .await
        .expect("prepared");
    (prepared, child)
}

fn order(plane: &TestPlane) -> Vec<u64> {
    plane
        .registry
        .state
        .lock()
        .expect("state")
        .capacity_waiters
        .keys()
        .copied()
        .collect()
}

async fn register(plane: &TestPlane, prepared: PreparedSubagent) -> (Waiting, CancellationSignal) {
    let registered = plane.registry.watch_next_capacity_wait();
    let registry = plane.registry.clone();
    let cancellation = CancellationSignal::new();
    let signal = cancellation.clone();
    let task = tokio::spawn(async move { registry.commit_waiting(prepared, &signal).await });
    registered
        .await
        .expect("actual native wait registration frontier");
    (task, cancellation)
}

fn pause_next_wait(plane: &TestPlane) -> tokio::sync::oneshot::Sender<()> {
    let (release, pause) = tokio::sync::oneshot::channel();
    plane
        .registry
        .state
        .lock()
        .expect("state")
        .capacity_wait_pause = Some(pause);
    release
}

async fn accepted(task: Waiting) -> SubagentAccepted {
    match task.await.expect("wait task").expect("commit") {
        SubagentStartOutcome::Accepted(accepted) => accepted,
        SubagentStartOutcome::RolledBack => panic!("unexpected rollback"),
    }
}

async fn finish(plane: &TestPlane, child: ScriptedChild, id: &SubagentId) {
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(id)
        .await
        .expect("native capacity release");
}

fn assert_unowned(plane: &TestPlane, id: &SubagentId) {
    assert!(plane.registry.snapshot(id).is_none());
    assert!(!events(plane).iter().any(|event| matches!(event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { subagent_id, .. }
            if subagent_id == id)));
}

fn assert_retired(plane: &TestPlane, child: &ScriptedChild, id: &SubagentId) {
    assert_unowned(plane, id);
    assert!(
        !plane
            .runtime_root
            .join(format!("test-child-{}", child.pid))
            .exists()
    );
}

#[tokio::test]
async fn registered_native_ordinals_not_wake_or_registration_order_choose_commit() {
    let plane = plane(1);
    let active_child = stage_exit0(&plane);
    let active = start(&plane, &start_spec("active")).await;
    let (alpha, alpha_child) = prepared(&plane, "alpha").await;
    let alpha_id = alpha.subagent_id.clone();
    let (beta, beta_child) = prepared(&plane, "beta").await;
    let beta_id = beta.subagent_id.clone();
    // Reverse registration deliberately: alpha's native ordinal still wins.
    let release_beta = pause_next_wait(&plane);
    let (beta, _) = register(&plane, beta).await;
    let release_alpha = pause_next_wait(&plane);
    let (alpha, _) = register(&plane, alpha).await;
    assert_eq!(order(&plane), [2, 3]);
    finish(&plane, active_child, &active.subagent_id).await;
    let beta_rechecked = plane.registry.watch_next_capacity_wait();
    release_beta
        .send(())
        .expect("retry beta only after capacity release");
    // Alpha cannot poll. Beta observes free capacity and is denied by the
    // registry's ordered eligibility, then enters the real wait again.
    beta_rechecked
        .await
        .expect("non-head retry denied with capacity free");
    assert_unowned(&plane, &alpha_id);
    assert_unowned(&plane, &beta_id);
    assert_eq!(order(&plane), [2, 3]);
    release_alpha.send(()).expect("release eligible waiter");
    let alpha = accepted(alpha).await;
    assert_eq!(alpha.subagent_id, alpha_id);
    assert_unowned(&plane, &beta_id);
    finish(&plane, alpha_child, &alpha_id).await;
    let beta = accepted(beta).await;
    assert_eq!(beta.subagent_id, beta_id);
    finish(&plane, beta_child, &beta_id).await;
    assert!(order(&plane).is_empty());
}

#[tokio::test]
async fn cancelling_registered_head_removes_it_and_advances_successor() {
    let plane = plane(1);
    let active_child = stage_exit0(&plane);
    let active = start(&plane, &start_spec("active")).await;
    let (alpha, alpha_child) = prepared(&plane, "alpha").await;
    let alpha_id = alpha.subagent_id.clone();
    let (beta, beta_child) = prepared(&plane, "beta").await;
    let (alpha, cancel_alpha) = register(&plane, alpha).await;
    let (beta, _) = register(&plane, beta).await;
    assert_eq!(order(&plane), [2, 3]);
    cancel_alpha.cancel();
    assert!(matches!(
        alpha.await.expect("wait task").expect("rollback"),
        SubagentStartOutcome::RolledBack
    ));
    assert_retired(&plane, &alpha_child, &alpha_id);
    assert_eq!(order(&plane), [3]);
    finish(&plane, active_child, &active.subagent_id).await;
    let beta = accepted(beta).await;
    finish(&plane, beta_child, &beta.subagent_id).await;
    assert!(order(&plane).is_empty());
}

#[tokio::test]
async fn cancelling_non_head_preserves_remaining_native_order() {
    let plane = plane(1);
    let active_child = stage_exit0(&plane);
    let active = start(&plane, &start_spec("active")).await;
    let (alpha, alpha_child) = prepared(&plane, "alpha").await;
    let (beta, beta_child) = prepared(&plane, "beta").await;
    let beta_id = beta.subagent_id.clone();
    let (gamma, gamma_child) = prepared(&plane, "gamma").await;
    let (alpha, _) = register(&plane, alpha).await;
    let (beta, cancel_beta) = register(&plane, beta).await;
    let (gamma, _) = register(&plane, gamma).await;
    assert_eq!(order(&plane), [2, 3, 4]);
    cancel_beta.cancel();
    assert!(matches!(
        beta.await.expect("wait task").expect("rollback"),
        SubagentStartOutcome::RolledBack
    ));
    assert_retired(&plane, &beta_child, &beta_id);
    assert_eq!(order(&plane), [2, 4]);
    finish(&plane, active_child, &active.subagent_id).await;
    let alpha = accepted(alpha).await;
    finish(&plane, alpha_child, &alpha.subagent_id).await;
    let gamma = accepted(gamma).await;
    finish(&plane, gamma_child, &gamma.subagent_id).await;
    assert!(order(&plane).is_empty());
}

#[tokio::test]
async fn release_after_registration_before_changed_await_cannot_lose_wakeup() {
    let plane = plane(1);
    let active_child = stage_exit0(&plane);
    let active = start(&plane, &start_spec("active")).await;
    let (alpha, alpha_child) = prepared(&plane, "alpha").await;
    let release = pause_next_wait(&plane);
    let (alpha, _) = register(&plane, alpha).await;
    // Exact frontier: registered, subscription/check complete, but changed()
    // has not been polled. Release is fully settled before allowing await.
    finish(&plane, active_child, &active.subagent_id).await;
    assert_eq!(order(&plane), [2]);
    // Ordinary commit stays non-waiting and cannot steal this free slot.
    let (ordinary, ordinary_child) = prepared(&plane, "ordinary").await;
    let ordinary_id = ordinary.subagent_id.clone();
    assert!(matches!(
        plane
            .registry
            .commit(ordinary, &CancellationSignal::new())
            .await,
        Err(SubagentStartError::CapacityExceeded { max: 1 })
    ));
    assert_retired(&plane, &ordinary_child, &ordinary_id);
    release.send(()).expect("enter changed await after release");
    let alpha = accepted(alpha).await;
    finish(&plane, alpha_child, &alpha.subagent_id).await;
    assert!(order(&plane).is_empty());
    // Other side of the frontier: release preceded registration entirely.
    let (immediate, child) = prepared(&plane, "immediate").await;
    let SubagentStartOutcome::Accepted(immediate) = plane
        .registry
        .commit_waiting(immediate, &CancellationSignal::new())
        .await
        .expect("immediate commit")
    else {
        panic!("not cancelled")
    };
    assert!(order(&plane).is_empty());
    finish(&plane, child, &immediate.subagent_id).await;
}

#[tokio::test]
async fn abandoned_waiter_rolls_back_before_native_task_completion() {
    let plane = plane(1);
    let active_child = stage_exit0(&plane);
    let active = start(&plane, &start_spec("active")).await;
    let (pending, child) = prepared(&plane, "abandoned").await;
    let id = pending.subagent_id.clone();
    let (pending, _) = register(&plane, pending).await;
    let (finished, completion) = tokio::sync::oneshot::channel();
    plane
        .registry
        .state
        .lock()
        .expect("state")
        .capacity_wait_finished = Some(finished);
    pending.abort();
    assert!(pending.await.expect_err("aborted caller").is_cancelled());
    completion
        .await
        .expect("native task conclusively rolled back");
    assert_retired(&plane, &child, &id);
    assert!(order(&plane).is_empty());
    finish(&plane, active_child, &active.subagent_id).await;
}

#[tokio::test]
async fn zero_capacity_rejects_without_registering_waiter() {
    let plane = plane(0);
    let (pending, child) = prepared(&plane, "zero").await;
    let id = pending.subagent_id.clone();
    assert!(matches!(
        plane
            .registry
            .commit_waiting(pending, &CancellationSignal::new())
            .await,
        Err(SubagentStartError::CapacityExceeded { max: 0 })
    ));
    assert!(order(&plane).is_empty());
    assert_retired(&plane, &child, &id);
}

#[tokio::test]
async fn cancelling_eligible_head_wakes_successor_without_another_capacity_release() {
    let plane = plane(1);
    let active_child = stage_exit0(&plane);
    let active = start(&plane, &start_spec("active")).await;
    let (alpha, alpha_child) = prepared(&plane, "alpha").await;
    let alpha_id = alpha.subagent_id.clone();
    let (beta, beta_child) = prepared(&plane, "beta").await;
    let release_alpha = pause_next_wait(&plane);
    let (alpha, cancel_alpha) = register(&plane, alpha).await;
    let release_beta = pause_next_wait(&plane);
    let (beta, _) = register(&plane, beta).await;
    finish(&plane, active_child, &active.subagent_id).await;
    let beta_rechecked = plane.registry.watch_next_capacity_wait();
    release_beta
        .send(())
        .expect("retry successor after capacity release");
    beta_rechecked
        .await
        .expect("successor denied despite free capacity");
    cancel_alpha.cancel();
    release_alpha
        .send(())
        .expect("allow cancelled head to settle");
    assert!(matches!(
        alpha.await.expect("wait task").expect("rollback"),
        SubagentStartOutcome::RolledBack
    ));
    assert_retired(&plane, &alpha_child, &alpha_id);
    // No active child can release capacity now. Ticket removal itself must
    // notify beta, which was already sleeping after its denied recheck.
    let beta = accepted(beta).await;
    finish(&plane, beta_child, &beta.subagent_id).await;
    assert!(order(&plane).is_empty());
}

#[tokio::test]
async fn native_drain_cancels_registered_staging_and_owned_children() {
    let plane = plane(1);
    let mut active_child = stage_exit0(&plane);
    let active = start(&plane, &start_spec("active")).await;
    assert!(matches!(
        active_child.read_frame().await,
        ParentFrame::Delegate(_)
    ));
    let (pending, child) = prepared(&plane, "pending").await;
    let id = pending.subagent_id.clone();
    let (pending, _) = register(&plane, pending).await;
    plane
        .registry
        .cancel_all(CancellationReason::RuntimeShutdown);
    assert!(matches!(
        pending.await.expect("wait task").expect("rollback"),
        SubagentStartOutcome::RolledBack
    ));
    assert_retired(&plane, &child, &id);
    assert!(order(&plane).is_empty());
    assert!(matches!(
        active_child.read_frame().await,
        ParentFrame::Cancel { .. }
    ));
    active_child
        .send_result(ChildResultStatus::Cancelled, None)
        .await;
    plane
        .registry
        .wait_until_settled(&active.subagent_id)
        .await
        .expect("owned native settlement");
}
