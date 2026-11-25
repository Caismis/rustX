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

fn pending_execution(f: &Fixture, session: &SessionId) -> bool {
    rustx_execution_server::RecoveryFence::session_pending(
        &f.archive_root,
        &rustx_execution_protocol::SessionId::new(session.as_str()),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_resume_reaps_eight_rejected_clients_and_allows_same_session_retry() {
    Box::pin(bounded(async {
        let f = Fixture::with_session_count(None, 16).await;
        f.manager
            .configure_local_sessions(f.workspaces.to_vec())
            .unwrap();
        let invalid = f.workspaces[0].join("rustx.toml");
        let valid = std::fs::read(&invalid).unwrap();
        std::fs::write(&invalid, "[environment\n").unwrap();
        for session in f.sessions.iter().step_by(2) {
            let failure = f.manager.load(&session.id, None).await.unwrap_err();
            assert!(failure.to_string().contains("rustx.toml"), "{failure}");
            assert!(
                f.manager
                    .execution
                    .configuration_endpoint(&session.id)
                    .unwrap()
                    .is_none()
            );
            assert!(!pending_execution(&f, &session.id));
            assert!(
                !f.manager
                    .sessions
                    .configuration_bindings
                    .lock()
                    .unwrap()
                    .contains_key(&session.id)
            );
        }
        let healthy = f
            .manager
            .create_session(SessionPersistentState::from_input(
                &SessionConfigInput::new(f.workspaces[1].clone()),
            ))
            .await
            .unwrap();
        assert!(
            f.manager
                .execution_endpoint(&healthy.session.id)
                .unwrap()
                .control()
                .is_some()
        );

        std::fs::write(&invalid, valid).unwrap();
        let original = &f.sessions[0];
        let resumed = f.manager.load(&original.id, None).await.unwrap();
        assert_eq!(resumed.conversation_id(), &original.active_conversation_id);
        assert!(
            f.manager
                .execution_endpoint(&original.id)
                .unwrap()
                .control()
                .is_some()
        );
        assert_eq!(f.provider.request_bodies(), Vec::<String>::new());
        assert_eq!(f.host.drain().await, Vec::<String>::new());
        f.manager.join_local_sessions().await.unwrap();
        assert!(!pending_execution(&f, &original.id));
        assert!(!pending_execution(&f, &healthy.session.id));
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_resume_shares_client_admission_across_cold_nodes() {
    Box::pin(bounded(async {
        for reject in [true, false] {
            let f = Fixture::with_session_count(None, 1).await;
            let branch = second_node(&f).await;
            let root = &f.sessions[0];
            f.manager
                .configure_local_sessions(f.workspaces.to_vec())
                .unwrap();
            let (entered, waiting) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            f.manager.configuration.test_hooks.insert(
                "before_resumed_client_preparation",
                move || {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                },
            );
            let root_probe = f.manager.probe(&root.active_conversation_id);
            root_probe.before_recovery_inspection.arm();
            if reject {
                f.manager
                    .configuration
                    .test_hooks
                    .fail_once("after_initial_runtime_configuration");
            }
            let loading = f.load(0);
            waiting.await.unwrap();
            assert!(
                f.manager
                    .execution
                    .configuration_endpoint(&root.id)
                    .unwrap_err()
                    .to_string()
                    .contains("preparation in progress")
            );
            let contender = f.manager.probe(&branch.active_conversation_id);
            let start_branch = || {
                let manager = f.manager.clone();
                let session = branch.id.clone();
                let node = branch.active_node.clone();
                tokio::spawn(async move { manager.load(&session, Some(&node)).await })
            };
            let early = start_branch();
            contender.joined(1).await;
            release.send(()).unwrap();
            root_probe.before_recovery_inspection.entered().await;
            let provisional = f.manager.execution_endpoint(&root.id).unwrap();
            let pair = provisional.control().expect("real provisional Client");
            assert!(pending_execution(&f, &root.id));
            assert!(
                !f.manager
                    .sessions
                    .configuration_bindings
                    .lock()
                    .unwrap()
                    .contains_key(&root.id)
            );
            let later = start_branch();
            contender.joined(2).await;
            root_probe.before_recovery_inspection.release();
            let first = loading.await.unwrap();
            let early = early.await.unwrap().unwrap_err();
            let later = later.await.unwrap().unwrap_err();
            if reject {
                let failure = first.unwrap_err();
                assert!(
                    failure
                        .to_string()
                        .contains("injected composition failure after configuration"),
                    "{failure}"
                );
                assert_eq!(early, failure);
                assert_eq!(later, failure);
                assert!(provisional.control().is_none());
                assert!(
                    f.manager
                        .execution
                        .configuration_endpoint(&root.id)
                        .unwrap()
                        .is_none()
                );
                assert!(!pending_execution(&f, &root.id));
                assert!(
                    !f.manager
                        .sessions
                        .configuration_bindings
                        .lock()
                        .unwrap()
                        .contains_key(&root.id)
                );
            } else {
                let first = first.unwrap();
                let expected = RuntimeManagerError::SessionAlreadyResident {
                    session_id: root.id.clone(),
                    resident_conversation: root.active_conversation_id.clone(),
                    requested_conversation: branch.active_conversation_id.clone(),
                };
                assert_eq!(early, expected);
                assert_eq!(later, expected);
                let assert_same_client = || {
                    let endpoint = f.manager.execution_endpoint(&root.id).unwrap();
                    assert!(Arc::ptr_eq(&provisional, &endpoint));
                    assert!(endpoint.control().unwrap().same_pair(&pair));
                };
                assert_same_client();
                f.manager.unload(first.conversation_id()).await.unwrap();
                f.manager
                    .configuration
                    .test_hooks
                    .fail_once("after_initial_runtime_configuration");
                let failure = start_branch().await.unwrap().unwrap_err();
                assert!(
                    failure.to_string().contains("injected composition failure"),
                    "{failure}"
                );
                assert_same_client();
                assert_eq!(
                    f.load(0).await.unwrap().unwrap().conversation_id(),
                    first.conversation_id()
                );
                assert_same_client();
            }
            assert_eq!(f.provider.request_bodies(), Vec::<String>::new());
            assert_eq!(f.host.drain().await, Vec::<String>::new());
            f.manager.join_local_sessions().await.unwrap();
            assert!(!pending_execution(&f, &root.id));
        }
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_resume_failed_physical_settlement_retains_execution_fence() {
    Box::pin(bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let session = &f.sessions[0];
        f.manager
            .configure_local_sessions(f.workspaces.to_vec())
            .unwrap();
        let probe = f.manager.probe(&session.active_conversation_id);
        probe.before_resident_publication.arm();
        let loading = f.load(0);
        probe.before_resident_publication.entered().await;
        let endpoint = f.manager.execution_endpoint(&session.id).unwrap();
        assert!(endpoint.control().is_some());
        endpoint.cut_pair();
        probe.before_resident_publication.release();
        let failure = loading.await.unwrap().unwrap_err();
        assert!(
            failure
                .to_string()
                .contains("the local Client or its Workspace grant is unavailable"),
            "{failure}"
        );
        assert!(pending_execution(&f, &session.id));
        let replacement = f.load(0).await.unwrap().unwrap_err();
        assert!(
            replacement
                .to_string()
                .contains("execution binding is closed; recovery is required"),
            "{replacement}"
        );
        assert_eq!(f.provider.request_bodies(), Vec::<String>::new());
        let failures = f.host.drain().await;
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("revocation is unconfirmed")),
            "{failures:?}"
        );
        f.manager.join_local_sessions().await.unwrap();
        assert!(pending_execution(&f, &session.id));
    }))
    .await;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResumeCut {
    Reservation,
    CapturedConfiguration,
    BeforeComposition,
    Prepared,
    Activated,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_resume_drain_cancels_every_admission_phase_and_settles_cleanly() {
    Box::pin(bounded(async {
        for cut in [
            ResumeCut::Reservation,
            ResumeCut::CapturedConfiguration,
            ResumeCut::BeforeComposition,
            ResumeCut::Prepared,
            ResumeCut::Activated,
        ] {
            let f = Fixture::with_session_count(None, 1).await;
            f.manager
                .configure_local_sessions(f.workspaces.to_vec())
                .unwrap();
            let session = &f.sessions[0];
            let id = &session.active_conversation_id;
            let probe = f.manager.probe(id);
            let gate = match cut {
                ResumeCut::Reservation | ResumeCut::CapturedConfiguration => None,
                ResumeCut::BeforeComposition => Some(&probe.before_recovery_inspection),
                ResumeCut::Prepared => Some(&probe.before_prepared_admission),
                ResumeCut::Activated => Some(&probe.before_resident_publication),
            };
            let hook = match cut {
                ResumeCut::Reservation => Some("before_resumed_client_preparation"),
                ResumeCut::CapturedConfiguration => Some("before_bound_initial_publication"),
                _ => None,
            };
            let capture_pause = hook.map(|hook| {
                let (entered, waiting) = tokio::sync::oneshot::channel();
                let (release, released) = std::sync::mpsc::channel();
                f.manager.configuration.test_hooks.insert(hook, move || {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                });
                (waiting, release)
            });
            let capacity = f.manager.available_local_client_slots();
            if let Some(gate) = gate {
                gate.arm();
            }
            let loading = f.load(0);
            let release = if let Some((waiting, release)) = capture_pause {
                waiting.await.unwrap();
                Some(release)
            } else {
                gate.expect("runtime gate").entered().await;
                None
            };
            {
                let registry = f.manager.registry.0.lock().unwrap();
                let Entry::Loading(flight) = &registry.entries[id] else {
                    panic!("runtime has not been published at {cut:?}");
                };
                assert_eq!(
                    flight
                        .initialization
                        .prepared_runtime()
                        .map(|runtime| runtime.is_activated()),
                    match cut {
                        ResumeCut::Prepared => Some(false),
                        ResumeCut::Activated => Some(true),
                        _ => None,
                    },
                    "{cut:?}"
                );
            }
            let endpoint = if cut == ResumeCut::Reservation {
                assert!(
                    f.manager
                        .execution_endpoint(&session.id)
                        .unwrap_err()
                        .to_string()
                        .contains("preparation in progress")
                );
                assert!(!pending_execution(&f, &session.id));
                None
            } else {
                let endpoint = f.manager.execution_endpoint(&session.id).unwrap();
                assert!(endpoint.control().is_some(), "{cut:?}");
                assert!(pending_execution(&f, &session.id), "{cut:?}");
                Some(endpoint)
            };
            assert_eq!(
                f.manager.available_local_client_slots(),
                capacity - 1,
                "{cut:?}"
            );
            f.host.begin_drain();
            let host = f.host.clone();
            let draining = tokio::spawn(async move { host.drain().await });
            probe
                .unloads_joined
                .subscribe()
                .wait_for(|n| *n == 1)
                .await
                .unwrap();
            if let Some(release) = release {
                release.send(()).unwrap();
            } else {
                gate.expect("runtime gate").release();
            }
            assert_eq!(
                loading.await.unwrap().unwrap_err(),
                RuntimeManagerError::LoadingCancelled,
                "{cut:?}"
            );
            assert_eq!(draining.await.unwrap(), Vec::<String>::new(), "{cut:?}");
            f.manager.join_local_sessions().await.unwrap();
            assert_eq!(f.manager.residency(id), ResidencyState::Unloaded, "{cut:?}");
            assert!(
                endpoint.is_none_or(|endpoint| endpoint.control().is_none()),
                "{cut:?}"
            );
            assert!(!pending_execution(&f, &session.id), "{cut:?}");
            assert_eq!(
                f.manager.available_local_client_slots(),
                capacity,
                "{cut:?}"
            );
            assert_eq!(f.provider.request_bodies(), Vec::<String>::new(), "{cut:?}");
            assert!(
                !f.manager
                    .sessions
                    .configuration_bindings
                    .lock()
                    .unwrap()
                    .contains_key(&session.id),
                "{cut:?}"
            );
        }
    }))
    .await;
}
