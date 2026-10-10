use super::*;
use crate::message::content::{MAX_INPUT_ATTACHMENTS, UploadedFileRef};

fn attachments(count: usize) -> Vec<UploadedFileRef> {
    (0..count)
        .map(|index| UploadedFileRef {
            batch_id: "trusted-domain-fixture".into(),
            name: format!("attachment-{index}.txt"),
        })
        .collect()
}

async fn reject_without_effects(plane: &TestPlane, agent: &AgentId) {
    let before = plane.registry.agent_snapshot(agent).unwrap();
    let journal = serde_json::to_value(events(plane)).unwrap();
    let admission = || {
        let state = plane.registry.state.lock().unwrap();
        (
            state.next_ordinal,
            state.records.len(),
            state.prepared_policies.len(),
            state.staged_overrides.len(),
        )
    };
    let before_admission = admission();
    let too_many = attachments(MAX_INPUT_ATTACHMENTS + 1);
    // This must return before reserving an activation, preparing a child or
    // scheduling a guidance write. There is no executor yield to hide a task.
    let mut rejected = Box::pin(plane.registry.send_message_with_attachments(
        agent,
        "",
        &too_many,
        AgentActivationOrigin::ClientControl,
        CancellationSignal::new(),
    ));
    assert!(matches!(
        futures_util::poll!(&mut rejected),
        std::task::Poll::Ready(Err(AgentControlError::Message(_)))
    ));
    assert_eq!(plane.registry.agent_snapshot(agent).unwrap(), before);
    assert_eq!(admission(), before_admission);
    assert_eq!(serde_json::to_value(events(plane)).unwrap(), journal);
    assert_eq!(
        plane
            .registry
            .outstanding_guidance_tickets(&before.latest_activation),
        0
    );
}

#[tokio::test]
async fn active_attachment_limit_precedes_guidance_delivery_and_accepts_attachment_only() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("initial")).await;
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
    reject_without_effects(&plane, &accepted.child_agent_id).await;
    let files = attachments(MAX_INPUT_ATTACHMENTS);
    let mut send = Box::pin(plane.registry.send_message_with_attachments(
        &accepted.child_agent_id,
        "",
        &files,
        AgentActivationOrigin::ClientControl,
        CancellationSignal::new(),
    ));
    assert!(futures_util::poll!(&mut send).is_pending());
    let ParentFrame::Guidance(guidance) = child.read_frame().await else {
        panic!("guidance")
    };
    // The first and only envelope after rejection is the valid input; no
    // rejected input can reach the child inbox or cause a model request.
    assert_eq!(guidance.attachments, files);
    assert!(guidance.message.is_empty());
    super::super::super::ipc::write_child_frame(
        &mut child.peer,
        &ChildFrame::GuidanceResult(super::super::super::ipc::GuidanceResultFrame {
            guidance_id: guidance.guidance_id,
            outcome: super::super::super::ipc::ChildGuidanceOutcome::Accepted,
        }),
    )
    .await
    .unwrap();
    assert!(!send.await.unwrap().resumed);
    child
        .send_result(ChildResultStatus::Succeeded, Some("done"))
        .await;
    drop(child);
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn inactive_attachment_limit_precedes_activation_and_accepts_attachment_only() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("initial")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    let mut resumed = stage_exit0(&plane);
    reject_without_effects(&plane, &accepted.child_agent_id).await;
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
    let files = attachments(MAX_INPUT_ATTACHMENTS);
    let mut send = Box::pin(plane.registry.send_message_with_attachments(
        &accepted.child_agent_id,
        "",
        &files,
        AgentActivationOrigin::ClientControl,
        CancellationSignal::new(),
    ));
    assert!(futures_util::poll!(&mut send).is_pending());
    let delegate = resumed.accept_delegate().await;
    assert_eq!(delegate.attachments, files);
    let receipt = send.await.unwrap();
    assert!(receipt.resumed);
    assert_ne!(receipt.activation_id, accepted.subagent_id);
    assert_eq!(plane.registry.all_snapshots().len(), 2);
    resumed
        .send_result(ChildResultStatus::Succeeded, Some("done"))
        .await;
    drop(resumed);
    plane
        .registry
        .wait_until_settled(&receipt.activation_id)
        .await
        .unwrap();
}
