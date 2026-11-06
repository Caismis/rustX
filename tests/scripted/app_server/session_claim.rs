//! Session residency claims across nodes: cold node routes, unrenderable
//! roots, claims through loading and unloading, and retained clients or
//! stale attachments that never own residency.
#![allow(clippy::too_many_lines)]
use super::*;
use crate::durable::CanonicalStore;

// Uses the durable graph owner to create two valid lineages before residency.
pub(super) async fn second_node(f: &Fixture) -> SessionSnapshot {
    use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
    let a = &f.sessions[0];
    let access = f
        .manager
        .sessions
        .acquire_session(&a.id, None)
        .await
        .unwrap();
    let store = crate::durable::SqliteConversationStore::open(
        a.active_conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    let boundary = crate::runtime::identity::MessageId::new("branch-boundary");
    store
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: boundary.clone(),
            content: input("branch seed"),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
        .unwrap();
    let revision = store.load_head().unwrap().revision;
    let branch = f
        .manager
        .sessions
        .branch_session_node(&a.id, &a.active_node, revision, &boundary)
        .await
        .unwrap()
        .session;
    f.manager
        .sessions
        .set_current_node(&a.id, &a.active_node)
        .await
        .unwrap();
    branch
}

/// An explicit cold node route composes exactly that node's lineage and never
/// publishes graph focus: the durable default node survives every routed load,
/// and after the one-time display-projection repair no routed load writes the
/// catalog at all. (Relocated from the retired launch client's `--node` route.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_node_routes_compose_the_routed_lineage_without_publishing_focus() {
    bounded(async {
        let f = Fixture::new().await;
        let branch = second_node(&f).await;
        let a = f.sessions[0].clone();
        assert_eq!(branch.id, a.id);
        assert_ne!(branch.active_node, a.active_node);
        let catalog = f.archive_root.join("sessions/catalog.json");
        let default = f.manager.sessions.read_session(&a.id).await.unwrap();
        assert_eq!(default.active_node, a.active_node);
        let mut settled_bytes = None;
        for (node, expected) in [
            (Some(&branch.active_node), &branch.active_conversation_id),
            (Some(&a.active_node), &a.active_conversation_id),
            (Some(&branch.active_node), &branch.active_conversation_id),
            (None, &a.active_conversation_id),
        ] {
            let loaded = f.manager.load(&a.id, node).await.unwrap();
            assert_eq!(loaded.conversation_id(), expected);
            f.manager.unload(loaded.conversation_id()).await.unwrap();
            assert_eq!(
                f.manager
                    .sessions
                    .read_session(&a.id)
                    .await
                    .unwrap()
                    .active_node,
                default.active_node,
                "a routed load never moves the durable default node"
            );
            let bytes = std::fs::read(&catalog).unwrap();
            if let Some(settled) = &settled_bytes {
                assert_eq!(&bytes, settled, "a routed load wrote the catalog");
            }
            settled_bytes = Some(bytes);
        }
        f.close().await;
    })
    .await;
}

/// Issue #386: an unrenderable first root message is a settled `None`. Neither
/// the branch's own renderable text nor a later root message may manufacture a
/// replacement projection when the manager cold-loads the root or a branch,
/// and no such load arms a publisher or writes the catalog. (Relocated from
/// the retired launch client's startup path onto the surviving composition.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_loads_never_manufacture_a_projection_for_an_unrenderable_root_subject() {
    bounded(async {
        use crate::message::types::{
            InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
        };
        let user = |id: &str, content: Vec<UserContentBlock>| {
            MessageBlock::User(UserMessageBlock {
                id: crate::runtime::identity::MessageId::new(id),
                content,
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })
        };
        let f = Fixture::with_session_count(None, 1).await;
        let sessions = &f.manager.sessions;
        let session = f.sessions[0].clone();
        let later = crate::runtime::identity::MessageId::new("later-root-text");
        let revision = {
            let access = sessions.acquire_session(&session.id, None).await.unwrap();
            let store = crate::durable::SqliteConversationStore::open(
                session.active_conversation_id.clone(),
                &access.database_path,
            )
            .unwrap();
            let artifacts = crate::tools::ArtifactStore::new(
                session.active_conversation_id.clone(),
                access.database_path.parent().unwrap(),
            )
            .unwrap();
            let mut png = Vec::new();
            png::Encoder::new(&mut png, 1, 1)
                .write_header()
                .unwrap()
                .write_image_data(&[0])
                .unwrap();
            let artifact_id = artifacts.put_bounded(&png).unwrap();
            store
                .append_canonical(&user(
                    "unrenderable-first",
                    vec![UserContentBlock::Image(
                        crate::message::content::ImageReference {
                            artifact_id,
                            alt: None,
                        },
                    )],
                ))
                .unwrap();
            // A later root message is renderable, and is still not the subject.
            store
                .append_canonical(&user("later-root-text", input("a later root message")))
                .unwrap();
            store.load_head().unwrap().revision
        };
        let branch_node = sessions
            .branch_session_node(&session.id, &session.active_node, revision, &later)
            .await
            .unwrap()
            .session
            .active_node;
        {
            let access = sessions
                .acquire_session(&session.id, Some(&branch_node))
                .await
                .unwrap();
            crate::durable::SqliteConversationStore::open(
                access.node.conversation_id.clone(),
                &access.database_path,
            )
            .unwrap()
            .append_canonical(&user(
                "branch-text",
                input("branch text is not the subject"),
            ))
            .unwrap();
        }
        sessions
            .set_current_node(&session.id, &session.active_node)
            .await
            .unwrap();
        let catalog = f.archive_root.join("sessions/catalog.json");
        let bytes = std::fs::read(&catalog).unwrap();
        for node in [Some(&branch_node), None] {
            let loaded = f.manager.load(&session.id, node).await.unwrap();
            assert!(
                crate::local_runtime::session_display_projection::display_projection_probe(
                    &session.id
                )
                .is_none(),
                "a settled None is never re-armed, on the root or on a branch"
            );
            f.manager.unload(loaded.conversation_id()).await.unwrap();
            assert_eq!(
                std::fs::read(&catalog).unwrap(),
                bytes,
                "an unrenderable subject writes nothing at all"
            );
        }
        assert_eq!(
            sessions
                .read_session_summary(&session.id)
                .await
                .unwrap()
                .preview,
            None
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_claim_excludes_other_nodes_through_loading_loaded_and_unloading() {
    bounded(async {
        let f = Fixture::new().await;
        let branch = second_node(&f).await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let other = f.manager.probe(&branch.active_conversation_id);
        probe.before_compose.arm();
        let loading = f.load(0);
        probe.before_compose.entered().await;
        let expected = RuntimeManagerError::SessionAlreadyResident {
            session_id: branch.id.clone(),
            resident_conversation: id.clone(),
            requested_conversation: branch.active_conversation_id.clone(),
        };
        assert_eq!(
            f.manager
                .load(&branch.id, Some(&branch.active_node))
                .await
                .unwrap_err(),
            expected
        );
        probe.before_compose.release();
        let a = loading.await.unwrap().unwrap();
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        b.client().submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        assert_eq!(
            f.manager
                .load(&branch.id, Some(&branch.active_node))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(
            f.manager
                .replace(&branch.id, Some(&branch.active_node))
                .await
                .unwrap_err(),
            expected
        );
        assert!(f.manager.is_current(&id, a.incarnation_id()));
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        assert!(live_b.has_current_attempt());
        probe.before_shutdown.arm();
        let manager = f.manager.clone();
        let unloading_id = id.clone();
        let unloading = tokio::spawn(async move { manager.unload(&unloading_id).await });
        probe.before_shutdown.entered().await;
        assert_eq!(
            f.manager
                .load(&branch.id, Some(&branch.active_node))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(other.compositions.load(Ordering::SeqCst), 0);
        probe.before_shutdown.release();
        unloading.await.unwrap().unwrap();
        let second = f
            .manager
            .load(&branch.id, Some(&branch.active_node))
            .await
            .unwrap();
        assert_eq!(other.compositions.load(Ordering::SeqCst), 1);
        assert!(
            f.manager
                .is_current(second.conversation_id(), second.incarnation_id())
        );
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        let done = live_b.settlement_signal().notified();
        f.gates[1].release();
        done.await;
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        f.manager.unload(second.conversation_id()).await.unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_client_and_identity_cannot_keep_unloaded_allocation_alive() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_session_count(None, 1).await;
        let identity = f.load(0).await.unwrap().unwrap();
        let client = identity.client();
        client.checkpoint().unwrap();
        let weak = identity.inspect_runtime().unwrap().weak_inner();
        f.manager.unload(identity.conversation_id()).await.unwrap();
        assert_eq!(
            f.manager.residency(identity.conversation_id()),
            ResidencyState::Unloaded
        );
        assert!(weak.upgrade().is_none());
        let SessionDeleteResult::Preview { preview } =
            f.manager.sessions.delete_preview(&f.sessions[0].id).await
        else {
            panic!("stale handles must not retain allocation")
        };
        assert!(matches!(
            f.manager
                .sessions
                .delete_session(&f.sessions[0].id, &preview.target_revision)
                .await
                .unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        // Both the production client and identity remain alive across real deletion.
        assert_eq!(
            client.submit_inbound(input("stale")).unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        assert_eq!(
            identity.client().checkpoint().unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_attachment_subscription_and_endpoint_do_not_own_residency() {
    Box::pin(bounded(async {
        use crate::runtime_client::host::EventDelivery;
        use crate::runtime_client::{RequestId, RuntimeClientError, RuntimeClientRequest};
        let f = Fixture::with_session_count(None, 1).await;
        let identity = f.load(0).await.unwrap().unwrap();
        let client = identity.client();
        let attachment = client.attach().unwrap().0;
        let checkpoint = attachment.checkpoint().unwrap();
        let subscription = attachment.subscribe_events(checkpoint.cursor).unwrap();
        let (endpoint, weak_host) = {
            let resident = identity.resident.upgrade().unwrap();
            let composition = resident.composition.lock().unwrap();
            let host = composition.as_ref().unwrap().host();
            (host.endpoint(), host.weak_inner())
        };
        let weak_runtime = identity.inspect_runtime().unwrap().weak_inner();
        f.manager.unload(identity.conversation_id()).await.unwrap();
        assert!(
            weak_host.upgrade().is_none(),
            "passive handles retain no host"
        );
        assert!(
            weak_runtime.upgrade().is_none(),
            "runtime resources released"
        );
        assert!(
            matches!(
                f.manager.sessions.delete_preview(&f.sessions[0].id).await,
                crate::local_runtime::session::deletion::SessionDeleteResult::Preview { .. }
            ),
            "native destructive preflight proves allocation access was released"
        );
        assert_eq!(subscription.next().await, EventDelivery::Closed);
        assert_eq!(
            attachment
                .handle_request(RuntimeClientRequest::CheckpointGet {
                    id: RequestId::new(1),
                })
                .error,
            Some(RuntimeClientError::NotAttached)
        );
        let replacement = f.load(0).await.unwrap().unwrap();
        assert_ne!(identity.incarnation_id(), replacement.incarnation_id());
        assert_eq!(
            client.validate(),
            Err(RuntimeManagerError::StaleIncarnation)
        );
        assert_eq!(subscription.try_next(), EventDelivery::Closed);
        assert_eq!(
            endpoint
                .handle_request(RuntimeClientRequest::Initialize {
                    id: RequestId::new(2),
                    protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
                })
                .error,
            Some(RuntimeClientError::NotAttached)
        );
        let _new_attachment = replacement.client().attach().unwrap().0;
        f.manager
            .unload(replacement.conversation_id())
            .await
            .unwrap();
        f.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_claim_survives_replacement_handoff_and_clears_on_failed_loading() {
    bounded(async {
        let f = Fixture::new().await;
        let branch = second_node(&f).await;
        let a = f.load(0).await.unwrap().unwrap();
        let probe = f.manager.probe(a.conversation_id());
        probe.before_compose.arm();
        probe.panic_once.store(true, Ordering::SeqCst);
        let manager = f.manager.clone();
        let session = f.sessions[0].id.clone();
        let replacement = tokio::spawn(async move { manager.replace(&session, None).await });
        probe.before_compose.entered().await;
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loading
        );
        assert!(a.inspect_runtime().is_none());
        assert!(matches!(
            f.manager.load(&branch.id, Some(&branch.active_node)).await,
            Err(RuntimeManagerError::SessionAlreadyResident { .. })
        ));
        probe.before_compose.release();
        assert!(replacement.await.unwrap().is_err());
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloaded
        );
        let second = f
            .manager
            .load(&branch.id, Some(&branch.active_node))
            .await
            .unwrap();
        f.manager.unload(second.conversation_id()).await.unwrap();
        f.close().await;
    })
    .await;
}
