//! Read original Child interaction bytes through the native Unix inspection route.
use super::*;
use base64::Engine as _;

pub(super) fn bind(
    plane: &ParentPlane,
    child: &ChildFixture,
    wired: &WiredChild,
) -> crate::local_runtime::live_inspection::LiveConversationInspectionServer {
    let path = plane
        .registry
        .agent_inspection_path(&wired.accepted.child_agent_id)
        .expect("exact admitted Child inspection path");
    crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
        path,
        child.host.clone(),
        wired.accepted.subagent_id.clone(),
    )
    .expect("original Child read-only Unix endpoint")
}

pub(super) async fn body(
    attachment: &crate::runtime_client::RuntimeAttachment,
    header: &crate::observation::interaction::RuntimeClientInteractionHeader,
) -> Arc<crate::runtime::InteractionRequest> {
    use crate::runtime_client::{RequestId, RuntimeClientRequest, RuntimeClientResult};
    assert!(header.body.is_none());
    assert!(!header.body_complete);
    let mut bytes = Vec::new();
    let mut offset = 0;
    let mut frontier = None;
    loop {
        let response = attachment
            .handle_request_async(RuntimeClientRequest::InteractionValueRead {
                id: RequestId::new(467),
                interaction: header.interaction.clone(),
                path: Vec::new(),
                offset,
                max_bytes: 65_536,
            })
            .await;
        let Some(RuntimeClientResult::InteractionValueChunk {
            interaction,
            path,
            offset: actual,
            bytes_base64,
            readable_end,
            next_offset,
        }) = response.result
        else {
            panic!("original routed interaction read failed: {response:?}");
        };
        assert_eq!(interaction, header.interaction);
        assert!(path.is_empty());
        assert_eq!(actual, offset);
        assert_eq!(*frontier.get_or_insert(readable_end), readable_end);
        assert!(readable_end <= 16 * 1024 * 1024);
        assert!(bytes_base64.len() <= 87_384);
        let part = base64::engine::general_purpose::STANDARD
            .decode(bytes_base64)
            .unwrap();
        assert!(!part.is_empty() && part.len() <= 65_536);
        offset = offset
            .checked_add(u64::try_from(part.len()).unwrap())
            .unwrap();
        assert!(offset <= readable_end);
        bytes.extend(part);
        if let Some(next) = next_offset {
            assert_eq!(next, offset);
            assert!(next < readable_end);
        } else {
            assert_eq!(offset, readable_end);
            break;
        }
    }
    let request: crate::runtime::InteractionRequest =
        crate::observation::structural_json::decode(serde_json::from_slice(&bytes).unwrap())
            .unwrap();
    assert_eq!(request.interaction_ref(), header.interaction);
    assert_eq!(request.id, header.request_id);
    assert_eq!(request.conversation_id, header.conversation_id);
    assert_eq!(request.execution, header.execution);
    let kind = match &request.kind {
        crate::runtime::interaction::InteractionKind::Approval { .. } => {
            crate::observation::interaction::RuntimeClientInteractionKind::Approval
        }
        crate::runtime::interaction::InteractionKind::Questionnaire { .. } => {
            crate::observation::interaction::RuntimeClientInteractionKind::Questionnaire
        }
        crate::runtime::interaction::InteractionKind::Review { .. } => {
            crate::observation::interaction::RuntimeClientInteractionKind::Review
        }
    };
    assert_eq!(kind, header.kind);
    Arc::new(request)
}
