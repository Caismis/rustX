//! Issue #467 source lifetime across real registry/control/inspection IPC.
use super::*;
use crate::runtime_client::agent_observation::{AgentObservation, ObservationSource};
use crate::runtime_client::{RuntimeClientRequest, RuntimeClientResult};

async fn observe(
    attachment: &crate::runtime_client::RuntimeAttachment,
    agent_id: &AgentId,
    after: Option<crate::runtime_client::agent_observation::AgentObservationCursor>,
) -> AgentObservation {
    let response = attachment
        .handle_request_async(RuntimeClientRequest::AgentObserve {
            id: crate::runtime_client::RequestId::new(467),
            agent_id: agent_id.clone(),
            after,
        })
        .await;
    match response.result {
        Some(RuntimeClientResult::AgentObserve { observation }) => observation,
        _ => panic!("observation failed: {response:?}"),
    }
}

/// Install the same production control loop for a fresh physical activation.
/// The process owns real kill/reap resources; only its executable is a stand-in.
fn stage_resume(
    plane: &ParentPlane,
    child: &ChildFixture,
) -> tokio::task::JoinHandle<Result<(), crate::local_runtime::subagent_child::ChildExit>> {
    let (driver, peer) = tokio::net::UnixStream::pair().unwrap();
    let (observation_driver, observation_peer) = tokio::net::UnixStream::pair().unwrap();
    let process = tokio::process::Command::new("sh")
        .args(["-c", "true"])
        .spawn()
        .unwrap();
    let physical_root = plane.runtime_root.join("observation-resume-process");
    std::fs::create_dir_all(&physical_root).unwrap();
    plane.registry.push_staged_override(StagedChild::for_test(
        process,
        driver,
        observation_driver,
        physical_root,
    ));
    let parent_id = plane.parent_agent_id.clone();
    let runtime = child.runtime.clone();
    tokio::spawn(async move {
        let mut dispatcher =
            crate::local_runtime::dispatcher::ChildControlDispatcher::start(peer, observation_peer);
        let handle = dispatcher.handle();
        runtime.install_interaction_route(Arc::new(
            crate::local_runtime::subagent_child::ChildInteractionRoute::new(handle.clone()),
        ));
        let result = crate::local_runtime::subagent_child::serve_child_delegation(
            &mut dispatcher,
            &handle,
            parent_id,
            runtime,
            None,
        )
        .await;
        dispatcher.shutdown().await;
        result
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t18_live_durable_only_and_reactivation_preserve_source_and_execution_identity() {
    let directory = inspection_tempdir().unwrap();
    let (parent, host) = parent_runtime_host_plane(
        &directory,
        "conv_01900000-0000-7000-8000-000000000467",
        Vec::new(),
    )
    .await;
    let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000468");
    let store_path = crate::runtime::subagent::child_conversation_store_path(
        &parent.plane.runtime_root,
        &crate::runtime::identity::SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
        &conversation,
    );
    let artifacts = store_path.parent().unwrap().to_path_buf();
    let (release, released) = support::fake::model_release();
    let child = child_fixture_scoped_at(
        &directory,
        &conversation,
        vec![parking_answer_script("retained answer", released)],
        ToolRegistry::new(),
        Vec::new(),
        artifacts.clone(),
        Some(SubagentId::for_conversation(
            parent.runtime.conversation_id(),
            1,
        )),
    )
    .await;
    let wired = launch_wired_child(&parent.plane, &child, "first activation").await;
    await_model_parks(&child.model, 1).await;
    let agent = wired.accepted.child_agent_id.clone();
    let socket = parent.plane.registry.agent_inspection_path(&agent).unwrap();
    let mut inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            socket.clone(),
            child.host.clone(),
            wired.accepted.subagent_id.clone(),
        )
        .unwrap();
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let live = observe(&attachment, &agent, None).await;
    let AgentObservation::Checkpoint {
        source: ObservationSource::Live,
        cursor: Some(old),
        ..
    } = live
    else {
        panic!("expected live checkpoint: {live:?}");
    };
    assert_eq!(old.activation_id, wired.accepted.subagent_id);
    assert_eq!(
        child.model.requests().len(),
        1,
        "inspection cannot start model work"
    );
    release.send(true).unwrap();
    await_serve(wired.serve).await;
    parent
        .plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .unwrap();
    inspection.shutdown().await.unwrap();
    child.runtime.shutdown().await.unwrap();

    let durable = observe(&attachment, &agent, None).await;
    let AgentObservation::Checkpoint {
        source: ObservationSource::DurableOnly,
        cursor: None,
        checkpoint,
        ..
    } = durable
    else {
        panic!("expected retained authoritative history: {durable:?}");
    };
    assert!(!checkpoint.snapshot.messages.is_empty());
    assert!(matches!(
        observe(&attachment, &agent, Some(old.clone())).await,
        AgentObservation::ResyncRequired {
            source: ObservationSource::DurableOnly,
            ..
        }
    ));

    let retained =
        assert_embedded_durable_trace(&host, &agent, &checkpoint, child.store.as_ref()).await;
    assert_reactivation(
        &directory,
        &parent,
        &attachment,
        artifacts,
        old,
        &host,
        retained,
    )
    .await;
    parent.runtime.shutdown().await.unwrap();
}

/// The physical inspection read starts while the child is live. A real Registry
/// settlement before the socket closes authorizes a source transition; the same
/// closed socket while execution remains active must stay unavailable.
async fn closing_inspection_case(settle: bool, following: bool, trace_detail: bool) {
    let directory = inspection_tempdir().unwrap();
    let (parent, host) = parent_runtime_host_plane(
        &directory,
        "conv_01900000-0000-7000-8000-000000000461",
        Vec::new(),
    )
    .await;
    let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000462");
    let store_path = crate::runtime::subagent::child_conversation_store_path(
        &parent.plane.runtime_root,
        &crate::runtime::identity::SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
        &conversation,
    );
    let (release, released) = support::fake::model_release();
    let child = child_fixture_scoped_at(
        &directory,
        &conversation,
        vec![parking_answer_script("retained closing answer", released)],
        ToolRegistry::new(),
        Vec::new(),
        store_path.parent().unwrap().to_path_buf(),
        Some(SubagentId::for_conversation(
            parent.runtime.conversation_id(),
            1,
        )),
    )
    .await;
    let wired = launch_wired_child(&parent.plane, &child, "closing inspection").await;
    await_model_parks(&child.model, 1).await;
    child.host.projection_settled().await;
    let agent = wired.accepted.child_agent_id.clone();
    let activation = wired.accepted.subagent_id.clone();
    let path = parent.plane.registry.agent_inspection_path(&agent).unwrap();
    let listener = tokio::net::UnixListener::bind(path).unwrap();
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let after =
        following.then(
            || crate::runtime_client::agent_observation::AgentObservationCursor {
                agent_id: agent.clone(),
                conversation_id: conversation.clone(),
                activation_id: activation.clone(),
                cursor: child.host.checkpoint().unwrap().cursor,
            },
        );
    let mut release = Some(release);
    let mut serve = Some(wired.serve);
    let record_id = child
        .host
        .checkpoint()
        .unwrap()
        .snapshot
        .trace
        .records
        .iter()
        .find(|record| record.kind == crate::observation::trace::TraceKind::Request)
        .expect("parked child has a native request record")
        .id
        .clone();
    let request = if trace_detail {
        RuntimeClientRequest::AgentTraceDetail {
            id: crate::runtime_client::RequestId::new(461),
            agent_id: agent.clone(),
            record_id: record_id.clone(),
        }
    } else {
        RuntimeClientRequest::AgentObserve {
            id: crate::runtime_client::RequestId::new(461),
            agent_id: agent.clone(),
            after,
        }
    };
    let read = attachment.handle_request_async(request);
    let close = async {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = tokio::io::BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        let request: RuntimeClientRequest = serde_json::from_str(&line).unwrap();
        assert!(matches!(request, RuntimeClientRequest::Initialize { .. }));
        let response = crate::runtime_client::RuntimeClientResponse {
            id: request.id(),
            error: None,
            result: Some(RuntimeClientResult::Initialized {
                attachment_id: crate::runtime_client::types::AttachmentId::new(
                    "closing-inspection",
                ),
                conversation_id: conversation.clone(),
                agent_id: agent.clone(),
                activation_id: Some(activation.clone()),
                protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
                read_only: true,
                capabilities: vec!["checkpoint".into()],
            }),
        };
        let mut bytes = serde_json::to_vec(&response).unwrap();
        bytes.push(b'\n');
        stream.get_mut().write_all(&bytes).await.unwrap();
        line.clear();
        stream.read_line(&mut line).await.unwrap();
        let request: RuntimeClientRequest = serde_json::from_str(&line).unwrap();
        assert!(if following {
            matches!(request, RuntimeClientRequest::ObserveEvents { .. })
        } else {
            matches!(request, RuntimeClientRequest::CheckpointGet { .. })
        });
        if settle {
            release.take().unwrap().send(true).unwrap();
            await_serve(serve.take().unwrap()).await;
            parent
                .plane
                .registry
                .wait_until_settled(&activation)
                .await
                .unwrap();
        }
        // This is a physical IPC failure, not a fabricated Runtime Client error.
        drop(stream);
    };
    let (response, ()) = tokio::time::timeout(LIVENESS, async { tokio::join!(read, close) })
        .await
        .unwrap();
    if trace_detail && settle {
        let Some(RuntimeClientResult::DomainPage { page }) = &response.result else {
            panic!("Trace detail must consume the terminal handoff: {response:?}")
        };
        assert_eq!(page.read_cut.conversation_id, conversation);
        assert_eq!(page.read_cut.activation_id.as_ref(), Some(&activation));
        assert_eq!(page.read_cut.revision, 0, "detail uses the retained source");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].identity, record_id);
    } else if settle {
        assert!(
            matches!(
                response.result,
                Some(RuntimeClientResult::AgentObserve {
                    observation: AgentObservation::ResyncRequired {
                        source: ObservationSource::DurableOnly,
                        ..
                    }
                })
            ),
            "terminal handoff is not a failed read: {response:?}"
        );
        let AgentObservation::Checkpoint {
            source: ObservationSource::DurableOnly,
            checkpoint,
            cursor: None,
            ..
        } = observe(&attachment, &agent, None).await
        else {
            panic!("next read must own retained authoritative history")
        };
        assert!(
            checkpoint
                .snapshot
                .messages
                .iter()
                .any(|message| format!("{message:?}").contains("retained closing answer"))
        );
    } else {
        assert!(
            matches!(
                response.error,
                Some(crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. })
            ),
            "a socket failure cannot fabricate settlement: {response:?}"
        );
        assert!(response.result.is_none());
        release.take().unwrap().send(true).unwrap();
        await_serve(serve.take().unwrap()).await;
        parent
            .plane
            .registry
            .wait_until_settled(&activation)
            .await
            .unwrap();
    }
    assert_eq!(
        child.model.requests().len(),
        1,
        "observation does not start model work"
    );
    child.runtime.shutdown().await.unwrap();
    parent.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_inspection_close_after_settlement_requires_retained_source_for_initial_and_following_reads()
 {
    for following in [false, true] {
        closing_inspection_case(true, following, false).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trace_detail_read_started_live_recovers_the_same_child_after_settlement() {
    closing_inspection_case(true, false, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_inspection_close_while_active_does_not_invent_retained_source_or_execution_outcome() {
    for following in [false, true] {
        closing_inspection_case(false, following, false).await;
    }
}

struct RetainedTraceField {
    cut: crate::observation::cut::CheckpointReadCut,
    domain: String,
    item_offset: u64,
    path: Vec<String>,
    encoded_bytes: u64,
}

fn original_terminal_field(trace: &crate::observation::trace::TracePage) -> RetainedTraceField {
    use crate::observation::trace::{TraceFieldKind, TraceFieldSource, TraceKind, TraceState};
    let terminal = trace
        .records
        .iter()
        .find(|record| record.kind == TraceKind::Attempt && record.state == TraceState::Completed)
        .expect("original Child terminal in the cold checkpoint Trace head");
    let TraceFieldSource::Value {
        domain,
        item_offset,
        path,
        encoded_bytes,
        ..
    } = terminal
        .fields
        .iter()
        .find(|field| {
            matches!(
                field,
                TraceFieldSource::Value {
                    field: TraceFieldKind::Result,
                    ..
                }
            )
        })
        .expect("original terminal result selector")
    else {
        unreachable!()
    };
    RetainedTraceField {
        cut: trace.read_cut.clone().unwrap(),
        domain: domain.clone(),
        item_offset: *item_offset,
        path: path.clone(),
        encoded_bytes: *encoded_bytes,
    }
}

async fn assert_embedded_durable_trace(
    host: &crate::runtime_client::RuntimeClientHost,
    agent: &AgentId,
    checkpoint: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    store: &dyn ConversationStore,
) -> RetainedTraceField {
    use crate::observation::trace::TraceKind;
    use base64::Engine as _;
    let before = store.observation_read_cut().unwrap();
    let trace = &checkpoint.snapshot.trace;
    let cut = trace
        .read_cut
        .as_ref()
        .expect("embedded Trace owns its original read cut");
    assert_eq!(cut.conversation_id, checkpoint.snapshot.conversation_id);
    assert_eq!(cut.activation_id, checkpoint.read_cut.activation_id);
    assert!(cut.activation_id.is_some());
    assert_eq!(cut.revision, 0);
    assert_eq!(cut.through, cut.source_cut.as_ref().unwrap().read.journal);
    let field = original_terminal_field(trace);
    let chunk = host
        .inner
        .agent_checkpoint_value_read(
            agent,
            cut,
            &field.domain,
            field.item_offset,
            &field.path,
            (0, 65_536),
        )
        .await
        .unwrap();
    assert_eq!(chunk.cut, *cut);
    assert_eq!(chunk.readable_end, field.encoded_bytes);
    assert_eq!(chunk.next_offset, None);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap(),
        serde_json::to_vec(&ModelFinishReason::Stop).unwrap()
    );
    let request = trace
        .records
        .iter()
        .find(|record| record.kind == TraceKind::Request)
        .unwrap();
    let domain = format!(
        "trace.records.{}.detail",
        request.id.strip_prefix("trace:").unwrap()
    );
    let page = host
        .inner
        .agent_checkpoint_page(agent, cut, &domain, 0, 32)
        .await
        .unwrap();
    assert_eq!(page.read_cut, *cut);
    assert_eq!(page.domain, domain);
    assert!(!page.items.is_empty());
    let mut foreign = cut.clone();
    foreign.activation_id = Some(SubagentId::new("foreign-activation"));
    assert!(matches!(
        host.inner
            .agent_checkpoint_value_read(
                agent,
                &foreign,
                &field.domain,
                field.item_offset,
                &field.path,
                (0, 65_536)
            )
            .await,
        Err(crate::runtime_client::RuntimeClientError::ReadCutExpired)
    ));
    assert_eq!(store.observation_read_cut().unwrap(), before);
    field
}

async fn assert_reactivation(
    directory: &tempfile::TempDir,
    parent: &ParentRuntimePlane,
    attachment: &crate::runtime_client::RuntimeAttachment,
    artifacts: std::path::PathBuf,
    old: crate::runtime_client::agent_observation::AgentObservationCursor,
    host: &crate::runtime_client::RuntimeClientHost,
    retained: RetainedTraceField,
) {
    let (release_again, next_model_gate) = support::fake::model_release();
    let resumed = child_fixture_scoped_at(
        directory,
        &old.conversation_id,
        vec![parking_answer_script("second answer", next_model_gate)],
        ToolRegistry::new(),
        Vec::new(),
        artifacts,
        Some(SubagentId::for_conversation(
            parent.runtime.conversation_id(),
            2,
        )),
    )
    .await;
    let serving = stage_resume(&parent.plane, &resumed);
    let accepted = parent
        .plane
        .registry
        .send_message(
            &old.agent_id,
            "second activation",
            crate::runtime::subagent::AgentActivationOrigin::ClientControl,
            CancellationSignal::new(),
        )
        .await
        .unwrap();
    assert_ne!(accepted.activation_id, old.activation_id);
    assert!(matches!(
        host.inner
            .agent_checkpoint_value_read(
                &old.agent_id,
                &retained.cut,
                &retained.domain,
                retained.item_offset,
                &retained.path,
                (0, 65_536),
            )
            .await,
        Err(crate::runtime_client::RuntimeClientError::ReadCutExpired)
    ));
    await_model_parks(&resumed.model, 1).await;
    let mut inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            parent
                .plane
                .registry
                .agent_inspection_path(&old.agent_id)
                .unwrap(),
            resumed.host.clone(),
            accepted.activation_id.clone(),
        )
        .unwrap();
    assert!(matches!(
        observe(attachment, &old.agent_id, Some(old.clone())).await,
        AgentObservation::ResyncRequired {
            source: ObservationSource::Live,
            ..
        }
    ));
    let fresh = observe(attachment, &old.agent_id, None).await;
    let AgentObservation::Checkpoint {
        source: ObservationSource::Live,
        cursor: Some(cursor),
        checkpoint,
        ..
    } = fresh
    else {
        panic!("expected reactivated source: {fresh:?}");
    };
    assert_eq!(cursor.activation_id, accepted.activation_id);
    assert_eq!(cursor.conversation_id, old.conversation_id);
    assert!(
        checkpoint.snapshot.attempt.is_some(),
        "DurableOnly did not fabricate a final execution outcome"
    );
    assert_eq!(resumed.model.requests().len(), 1);
    release_again.send(true).unwrap();
    await_serve(serving).await;
    parent
        .plane
        .registry
        .wait_until_settled(&accepted.activation_id)
        .await
        .unwrap();
    inspection.shutdown().await.unwrap();
    resumed.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t20_real_unix_listener_bounds_and_reaps_its_physical_workset() {
    let directory = inspection_tempdir().unwrap();
    let child = child_fixture_with_activation(
        &directory,
        &ConversationId::new("conv_01900000-0000-7000-8000-000000000469"),
        Vec::new(),
        ToolRegistry::new(),
        Vec::new(),
        SubagentId::new("inspection-workset"),
    )
    .await;
    let path = directory.path().join("workset.sock");
    let mut inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            path.clone(),
            child.host.clone(),
            SubagentId::new("inspection-workset"),
        )
        .unwrap();
    let mut workset = inspection.physical_workset();
    let mut clients = Vec::new();
    for index in 0..8 {
        let (mut reader, mut writer) = connect_live_inspection(&path).await;
        let response = live_inspection_call(
            &mut reader,
            &mut writer,
            RuntimeClientRequest::Initialize {
                id: crate::runtime_client::RequestId::new(index),
                protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
            },
        )
        .await;
        assert!(matches!(
            response.result,
            Some(RuntimeClientResult::Initialized {
                read_only: true,
                ..
            })
        ));
        clients.push((reader, writer));
    }
    assert_eq!(*workset.borrow(), 8);
    let mut refused = tokio::net::UnixStream::connect(&path).await.unwrap();
    let mut record = String::new();
    assert_eq!(
        tokio::io::BufReader::new(&mut refused)
            .read_line(&mut record)
            .await
            .unwrap(),
        0
    );
    assert_eq!(*workset.borrow(), 8);

    // Repeated turnover proves finished JoinSet entries are reaped while the
    // listener continues, rather than accumulating until server shutdown.
    for index in 0..32 {
        drop(clients.pop().unwrap());
        while *workset.borrow_and_update() != 7 {
            workset.changed().await.unwrap();
        }
        let (mut reader, mut writer) = connect_live_inspection(&path).await;
        let response = live_inspection_call(
            &mut reader,
            &mut writer,
            RuntimeClientRequest::Initialize {
                id: crate::runtime_client::RequestId::new(index + 8),
                protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
            },
        )
        .await;
        assert!(matches!(
            response.result,
            Some(RuntimeClientResult::Initialized { .. })
        ));
        assert_eq!(*workset.borrow(), 8);
        clients.push((reader, writer));
    }
    assert!(child.model.requests().is_empty());
    inspection.shutdown().await.unwrap();
    assert_eq!(*workset.borrow(), 0);
    for (mut reader, writer) in clients {
        drop(writer);
        assert!(reader.next_line().await.unwrap().is_none());
    }
    child.runtime.shutdown().await.unwrap();
}

/// Root retains only finite Child interaction headers. Deferred bytes are read
/// through the actual Unix inspection endpoint from the originating coordinator.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t11_t12_child_pending_body_uses_its_own_unix_source_and_exact_root_cut() {
    use base64::Engine as _;
    let directory = inspection_tempdir().unwrap();
    let (parent, host) = parent_runtime_host_plane(
        &directory,
        "conv_01900000-0000-7000-8000-000000000481",
        Vec::new(),
    )
    .await;
    let (tools, calls, executor_started) = child_approval_tools();
    let arguments = serde_json::json!({"target": "é\\\"".repeat(40_000)});
    let call = support::fake::ScriptedCall {
        id: "call-child-source",
        tool_id: "tool-approved",
        name: "approved_tool",
        arguments: arguments.clone(),
    };
    let mut script = vec![FakeStep::Emit(started())];
    script.extend(
        support::fake::tool_call_events(0, &call)
            .into_iter()
            .map(FakeStep::Emit),
    );
    script.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let child = child_fixture_with_activation(
        &directory,
        &ConversationId::new("conv_01900000-0000-7000-8000-000000000482"),
        vec![script, answer_script("declined child source")],
        tools,
        Vec::new(),
        SubagentId::for_conversation(parent.runtime.conversation_id(), 1),
    )
    .await;
    let published = parent.plane.registry.interaction_published.notified();
    tokio::pin!(published);
    published.as_mut().enable();
    let wired = launch_wired_child(&parent.plane, &child, "inspect finite pending header").await;
    let socket = parent
        .plane
        .registry
        .agent_inspection_path(&wired.accepted.child_agent_id)
        .unwrap();
    let mut inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            socket,
            child.host.clone(),
            wired.accepted.subagent_id.clone(),
        )
        .unwrap();
    if parent
        .plane
        .registry
        .pending_interaction_head()
        .head
        .is_empty()
    {
        published.await;
    }
    // Finish the reliable header drain, then hold unrelated disposable Activity
    // at the existing native preparation gate throughout this read-only proof.
    assert!(
        !host
            .inner
            .checkpoint()
            .unwrap()
            .snapshot
            .pending_interactions
            .is_empty()
    );
    host.inner.park_projection_worker();
    let checkpoint = host.inner.checkpoint().unwrap();
    let header = checkpoint.snapshot.pending_interactions.first().unwrap();
    assert!(header.body.is_none());
    assert!(!header.body_complete);
    assert!(serde_json::to_vec(header).unwrap().len() < 16_384);
    let source_offset = header.source_offset;
    let argument_domain = format!("pending_interactions.{source_offset}.request.kind.arguments");
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = host
            .inner
            .checkpoint_value_read_async(
                &checkpoint.read_cut,
                "pending_interactions",
                source_offset,
                &[],
                (offset, 8192),
            )
            .await
            .unwrap();
        assert_eq!(chunk.cut, checkpoint.read_cut);
        assert_eq!(chunk.offset, offset);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(!part.is_empty());
        assert!(part.len() <= 8192);
        bytes.extend(part);
        let Some(next) = chunk.next_offset else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    let body: crate::runtime::interaction::RoutedInteraction =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body.interaction, header.interaction);
    assert_eq!(body.source, header.source);
    let crate::runtime::interaction::InteractionKind::Approval {
        arguments: actual, ..
    } = &body.request.kind
    else {
        panic!("approval")
    };
    assert_eq!(actual, &arguments);
    let fields = host
        .inner
        .checkpoint_page_async(&checkpoint.read_cut, &argument_domain, 0, 32)
        .await
        .unwrap();
    assert_eq!(fields.total, 1);
    assert_eq!(fields.items[0].key.as_ref().unwrap().preview, "target");
    assert!(fields.items[0].deferred);
    let selected = host
        .inner
        .checkpoint_value_read_async(&checkpoint.read_cut, &argument_domain, 0, &[], (0, 8192))
        .await
        .unwrap();
    let expected = serde_json::to_vec(&arguments["target"]).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(selected.bytes_base64)
            .unwrap(),
        expected[..8192]
    );
    assert_eq!(selected.readable_end, expected.len() as u64);
    let key = host
        .inner
        .checkpoint_value_read_async(
            &checkpoint.read_cut,
            &argument_domain,
            0,
            &["$key".into()],
            (0, 16),
        )
        .await
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(key.bytes_base64)
            .unwrap(),
        br#""target""#
    );
    assert_eq!(
        host.inner.checkpoint().unwrap().cursor,
        checkpoint.cursor,
        "reads never publish a display transition"
    );
    let mut foreign = checkpoint.read_cut.clone();
    foreign.conversation_id = child.store.conversation_id().clone();
    assert!(
        host.inner
            .checkpoint_value_read_async(
                &foreign,
                "pending_interactions",
                source_offset,
                &[],
                (0, 1)
            )
            .await
            .is_err()
    );
    assert!(calls.borrow().is_empty());
    assert!(!*executor_started.borrow());
    host.inner.unpark_projection_worker();
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    attachment
        .cancel_interaction(&header.interaction)
        .await
        .unwrap();
    parent
        .plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .unwrap();
    await_serve(wired.serve).await;
    inspection.shutdown().await.unwrap();
    attachment.detach();
    child.runtime.shutdown().await.unwrap();
    parent.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_t12_t20_nested_pending_reads_follow_authenticated_direct_routes_over_two_unix_peers() {
    Box::pin(nested_pending_fixture(false)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t18_stopping_real_unix_source_is_unavailable_without_a_false_execution_outcome() {
    Box::pin(nested_pending_fixture(true)).await;
}

#[allow(clippy::too_many_lines)] // One real three-Runtime fixture holds the same source through its seal boundary.
async fn nested_pending_fixture(stop_at_seal: bool) {
    use base64::Engine as _;
    let root_dir = inspection_tempdir().unwrap();
    let middle_dir = inspection_tempdir().unwrap();
    let leaf_dir = inspection_tempdir().unwrap();
    let (root, root_host) = parent_runtime_host_plane(
        &root_dir,
        "conv_01900000-0000-7000-8000-000000000491",
        Vec::new(),
    )
    .await;
    let (release, middle_gate) = support::fake::model_release();
    let (middle, middle_host) = parent_runtime_host_plane_for_activation(
        &middle_dir,
        "conv_01900000-0000-7000-8000-000000000492",
        vec![
            parking_answer_script("middle answer", middle_gate),
            answer_script("nested result"),
        ],
        SubagentId::for_conversation(root.runtime.conversation_id(), 1),
    )
    .await;
    let middle_child = ChildFixture {
        runtime: middle.runtime.clone(),
        host: middle_host.clone(),
        model: middle.model.clone(),
        store: middle.plane.store.clone(),
        clock: Arc::new(ManualMonotonicClock::new()),
    };
    let root_child = launch_wired_child(
        &root.plane,
        &middle_child,
        "hold middle while inspecting nested source",
    )
    .await;
    await_model_parks(&middle.model, 1).await;
    let middle_inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            root.plane
                .registry
                .agent_inspection_path(&root_child.accepted.child_agent_id)
                .unwrap(),
            middle_host.clone(),
            root_child.accepted.subagent_id.clone(),
        )
        .unwrap();
    let mut middle_inspection = Some(middle_inspection);
    let (tools, calls, _) = child_approval_tools();
    let arguments = serde_json::json!({"target":"🙂\\\"".repeat(30_000)});
    let call = support::fake::ScriptedCall {
        id: "nested-approved",
        tool_id: "tool-approved",
        name: "approved_tool",
        arguments: arguments.clone(),
    };
    let mut script = vec![FakeStep::Emit(started())];
    script.extend(
        support::fake::tool_call_events(0, &call)
            .into_iter()
            .map(FakeStep::Emit),
    );
    script.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let leaf = child_fixture_with_activation(
        &leaf_dir,
        &ConversationId::new("conv_01900000-0000-7000-8000-000000000493"),
        vec![script, answer_script("nested denied")],
        tools,
        Vec::new(),
        SubagentId::for_conversation(middle.runtime.conversation_id(), 1),
    )
    .await;
    let published = root.plane.registry.interaction_published.notified();
    tokio::pin!(published);
    published.as_mut().enable();
    let nested = launch_wired_child(&middle.plane, &leaf, "request nested approval").await;
    let mut leaf_inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            middle
                .plane
                .registry
                .agent_inspection_path(&nested.accepted.child_agent_id)
                .unwrap(),
            leaf.host.clone(),
            nested.accepted.subagent_id.clone(),
        )
        .unwrap();
    if root
        .plane
        .registry
        .pending_interaction_head()
        .head
        .is_empty()
    {
        published.await;
    }
    // Drain the reliable header before freezing unrelated disposable Activity.
    // The preparation gate waits for any in-flight native drain to finish;
    // both real inspection peers stay live while reads prove no publication.
    assert!(
        !root_host
            .inner
            .checkpoint()
            .unwrap()
            .snapshot
            .pending_interactions
            .is_empty()
    );
    root_host.inner.park_projection_worker();
    let before = root_host.inner.checkpoint().unwrap();
    assert_eq!(root_host.inner.queued_lifecycle_records(), 0);
    let mut owner_cut = before.read_cut.collection_cuts["agents"];
    for _ in 0..256 {
        owner_cut = root
            .plane
            .registry
            .republish_lifecycle_for_test(&root_child.accepted.subagent_id);
    }
    let checkpoint = root_host.inner.checkpoint().unwrap();
    assert_eq!(
        checkpoint.cursor.get(),
        before.cursor.get() + 256,
        "each real Registry owner publication has one complete native cursor"
    );
    assert_eq!(
        root_host.inner.queued_lifecycle_records(),
        0,
        "parked observation worker cannot accumulate reliable lifecycle handles"
    );
    // The real Registry version watch can publish one independent disposable
    // Activity after the final owner callback retires the previous slot.
    assert!(
        root_host.inner.queued_observation_records() <= 1,
        "the one active Root Child has at most one disposable Activity slot"
    );
    assert_eq!(
        checkpoint.read_cut.collection_cuts["agents"], owner_cut,
        "native source generation is captured at the same Registry owner callback"
    );
    let header = checkpoint.snapshot.pending_interactions.first().unwrap();
    assert!(header.body.is_none());
    assert_eq!(
        header.interaction.conversation_id,
        *leaf.runtime.conversation_id()
    );
    let crate::runtime::interaction::InteractionSource::Subagent { subagent_id, .. } =
        &header.source
    else {
        panic!("originating Leaf metadata")
    };
    assert_eq!(*subagent_id, nested.accepted.subagent_id);
    let (_, direct_cid, direct_activation) = root
        .plane
        .registry
        .interaction_inspection_source(&header.interaction)
        .unwrap();
    assert_eq!(direct_cid, *middle.runtime.conversation_id());
    assert_eq!(direct_activation, root_child.accepted.subagent_id);
    let domain = format!(
        "pending_interactions.{}.request.kind.arguments",
        header.source_offset
    );
    let page_result = root_host
        .inner
        .checkpoint_page_async(&checkpoint.read_cut, &domain, 0, 32)
        .await;
    if let Err(error) = &page_result {
        let inspection = root_host.inner.admit_attachment_identity(true).unwrap();
        let replay = root_host
            .inner
            .observe_events(
                inspection.attachment_id(),
                crate::runtime_client::RuntimeClientCursor::new(checkpoint.cursor.get() - 1),
            )
            .await;
        panic!(
            "nested source page: {error:?}; captured={:?}; current={:?}; native_replay={replay:?}",
            checkpoint.read_cut,
            root_host.published_read_cut()
        );
    }
    let page = page_result.unwrap();
    assert_eq!(page.total, 1);
    assert!(page.items[0].deferred);
    let expected = serde_json::to_vec(&arguments["target"]).unwrap();
    let (mut offset, mut bytes) = (0, Vec::new());
    loop {
        let chunk = root_host
            .inner
            .checkpoint_value_read_async(&checkpoint.read_cut, &domain, 0, &[], (offset, 8192))
            .await
            .unwrap();
        assert_eq!(chunk.cut, checkpoint.read_cut);
        let data = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(!data.is_empty());
        assert!(data.len() <= 8192);
        bytes.extend(data);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(bytes, expected);
    assert_eq!(leaf.model.requests().len(), 1);
    assert_eq!(middle.model.requests().len(), 1);
    assert_eq!(calls.borrow().len(), 0);
    assert_eq!(
        root_host.inner.checkpoint().unwrap().cursor,
        checkpoint.cursor
    );
    root_host.inner.unpark_projection_worker();
    if stop_at_seal {
        let seal = Arc::new(crate::runtime::subagent::CommitBoundaryHook::default());
        root.plane.registry.install_seal_boundary_hook(seal.clone());
        release.send(true).unwrap();
        let waiting = seal.clone();
        tokio::task::spawn_blocking(move || waiting.wait_until_entered())
            .await
            .unwrap();
        let stopping = root
            .plane
            .registry
            .snapshot(&root_child.accepted.subagent_id)
            .unwrap();
        assert_eq!(stopping.state, SubagentState::Stopping);
        assert!(!stopping.is_settled());
        // The disposable inspection carrier closes independently while the
        // real control driver remains gated at its native Stopping boundary.
        let mut closing_inspection = middle_inspection.take().unwrap();
        closing_inspection.shutdown().await.unwrap();
        let read = root_host
            .inner
            .checkpoint_page_async(&checkpoint.read_cut, &domain, 0, 32)
            .await;
        let after_read = root
            .plane
            .registry
            .snapshot(&root_child.accepted.subagent_id)
            .unwrap();
        // Cancel through the Leaf's existing action owner while the Middle's
        // physical seal is still gated. Granting that seal first lets Middle
        // shutdown retire the Leaf interaction before this cleanup command.
        let cancellation = async {
            let (attachment, _) = leaf
                .host
                .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)?;
            attachment.cancel_interaction(&header.interaction).await
        }
        .await;
        // Release even if cancellation or an assertion fails, so fixture cleanup
        // never leaves a blocked physical driver holding the process alive.
        seal.release();
        assert_eq!(after_read.state, stopping.state);
        assert!(
            !after_read.is_settled(),
            "read failure cannot settle the native execution"
        );
        assert!(
            matches!(
                read,
                Err(
                    crate::runtime_client::RuntimeClientError::ContentSourceUnavailable
                        | crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. }
                )
            ),
            "stopping source classification: {read:?}"
        );
        cancellation.unwrap();
    } else {
        let (attachment, _) = root_host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        attachment
            .cancel_interaction(&header.interaction)
            .await
            .unwrap();
    }
    await_serve(nested.serve).await;
    middle
        .plane
        .registry
        .wait_until_settled(&nested.accepted.subagent_id)
        .await
        .unwrap();
    leaf_inspection.shutdown().await.unwrap();
    leaf.runtime.shutdown().await.unwrap();
    let _ = root_child.stop_serve.send(());
    await_serve(root_child.serve).await;
    let _ = release.send(true);
    if let Some(mut inspection) = middle_inspection {
        inspection.shutdown().await.unwrap();
    }
    middle.runtime.shutdown().await.unwrap();
    root.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t20_real_unix_shutdown_joins_admitted_physical_source_worker() {
    let directory = inspection_tempdir().unwrap();
    let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000474");
    let child = child_fixture_with_activation(
        &directory,
        &conversation,
        Vec::new(),
        ToolRegistry::new(),
        Vec::new(),
        SubagentId::new("inspection-physical-shutdown"),
    )
    .await;
    let path = directory.path().join("physical-shutdown.sock");
    let mut inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            path.clone(),
            child.host.clone(),
            SubagentId::new("inspection-physical-shutdown"),
        )
        .unwrap();
    let workset = inspection.physical_workset();
    let (mut reader, mut writer) = connect_live_inspection(&path).await;
    let initialized = live_inspection_call(
        &mut reader,
        &mut writer,
        RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        },
    )
    .await;
    assert!(matches!(
        initialized.result,
        Some(RuntimeClientResult::Initialized { .. })
    ));
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    crate::local_runtime::live_inspection::install_trace_read_gate(
        conversation,
        (entered, release_rx),
    );
    let request = RuntimeClientRequest::TraceLocateTool {
        id: crate::runtime_client::RequestId::new(2),
        locator: crate::runtime_client::trace::TraceToolLocator {
            occurrence: crate::observation::tool::RuntimeClientToolOccurrence {
                locator: crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                    position: 1,
                },
                block_index: crate::message::types::ContentBlockIndex::new(0),
            },
            call_id: None,
            tool_id: None,
        },
    };
    let record = format!("{}\n", serde_json::to_string(&request).unwrap());
    writer.write_all(record.as_bytes()).await.unwrap();
    entered_rx.await.unwrap();
    {
        let mut shutdown = Box::pin(inspection.shutdown());
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        assert_eq!(
            *workset.borrow(),
            1,
            "a cancelled shutdown waiter does not release actual source work"
        );
    }
    // A cancelled waiter retains the listener's original JoinHandle in place.
    let mut shutdown = Box::pin(inspection.shutdown());
    assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
    assert_eq!(*workset.borrow(), 1);
    release.send(()).unwrap();
    shutdown.await.unwrap();
    assert_eq!(*workset.borrow(), 0);
    assert!(reader.next_line().await.unwrap().is_none());
    assert!(child.model.requests().is_empty());
    child.runtime.shutdown().await.unwrap();
}

struct ChildOccupancySourceFault {
    connection: rusqlite::Connection,
    original: Option<Vec<u8>>,
}
impl ChildOccupancySourceFault {
    fn new(path: &std::path::Path) -> Self {
        let connection = rusqlite::Connection::open(path).unwrap();
        let original: Option<Vec<u8>> = connection
            .query_row(
                "SELECT occupancy_observation FROM bootstrap_identity WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            original.is_some(),
            "the original finite occupancy Source is present"
        );
        assert_eq!(
            connection
                .execute(
                    "UPDATE bootstrap_identity SET occupancy_observation=NULL WHERE id=1",
                    []
                )
                .unwrap(),
            1
        );
        Self {
            connection,
            original,
        }
    }
    fn repair(&mut self) {
        assert_eq!(
            self.connection
                .execute(
                    "UPDATE bootstrap_identity SET occupancy_observation=?1 WHERE id=1",
                    [self.original.as_deref()]
                )
                .unwrap(),
            1
        );
    }
}
impl Drop for ChildOccupancySourceFault {
    fn drop(&mut self) {
        self.repair();
    }
}

async fn assert_child_source_unavailable(
    attachment: &crate::runtime_client::RuntimeAttachment,
    agent: &AgentId,
    after: Option<crate::runtime_client::agent_observation::AgentObservationCursor>,
    expected: &str,
) {
    let response = attachment
        .handle_request_async(RuntimeClientRequest::AgentObserve {
            id: crate::runtime_client::RequestId::new(478),
            agent_id: agent.clone(),
            after,
        })
        .await;
    assert!(response.result.is_none(), "no Live resync or cold fallback");
    assert!(
        matches!(
            response.error.as_ref(),
            Some(crate::runtime_client::RuntimeClientError::ObservationUnavailable { message })
                if message.contains(expected)
        ),
        "expected typed Child Source failure containing {expected:?}: {response:?}"
    );
}

async fn assert_child_source_failure_and_repair(
    attachment: &crate::runtime_client::RuntimeAttachment,
    child: &ChildFixture,
    agent: &AgentId,
    activation: &SubagentId,
    store_path: &std::path::Path,
) {
    struct Unpark<'a>(&'a crate::runtime_client::RuntimeClientHost);
    impl Drop for Unpark<'_> {
        fn drop(&mut self) {
            self.0.inner.unpark_projection_worker();
        }
    }
    child.host.inner.park_projection_worker();
    let _unpark = Unpark(&child.host);
    let AgentObservation::Checkpoint {
        source: ObservationSource::Live,
        cursor: Some(before),
        checkpoint,
        ..
    } = observe(attachment, agent, None).await
    else {
        panic!("actual live Child checkpoint")
    };
    let native = child.store.observation_read_cut().unwrap();
    let mut fault = ChildOccupancySourceFault::new(store_path);
    child
        .host
        .fail_projection_read_for_test("held Child Source fault");
    assert_child_source_unavailable(
        attachment,
        agent,
        Some(before.clone()),
        "held Child Source fault",
    )
    .await;
    child.host.inner.unpark_projection_worker();
    assert_child_source_unavailable(attachment, agent, None, "occupancy").await;
    assert_eq!(child.store.observation_read_cut().unwrap(), native);
    assert_eq!(child.model.requests().len(), 1);
    fault.repair();
    let AgentObservation::Checkpoint {
        checkpoint: repaired,
        cursor: Some(repaired_cursor),
        ..
    } = observe(attachment, agent, None).await
    else {
        panic!("public Child checkpoint repairs the same native Source")
    };
    assert_eq!(repaired_cursor.activation_id, before.activation_id);
    assert_eq!(repaired_cursor.cursor.get(), before.cursor.get() + 1);
    assert_eq!(repaired.read_cut.source_cut, checkpoint.read_cut.source_cut);
    assert_eq!(repaired.read_cut.activation_id.as_ref(), Some(activation));
    let AgentObservation::Events {
        after,
        cursor,
        events,
    } = observe(attachment, agent, Some(before.clone())).await
    else {
        panic!("same activation resumes retained native events")
    };
    assert_eq!(after, before);
    assert_eq!(cursor.activation_id, before.activation_id);
    assert_eq!(cursor.conversation_id, before.conversation_id);
    assert_eq!(cursor.cursor.get(), before.cursor.get() + 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].cursor, cursor.cursor);
    assert!(events[0].event.facts().iter().any(|fact| matches!(
        fact,
        crate::runtime_client::event::RuntimeClientEvent::ReadDomainsUpdated { .. }
    )));
    assert_eq!(child.store.observation_read_cut().unwrap(), native);
    assert_eq!(child.model.requests().len(), 1);
    assert_eq!(checkpoint.read_cut.activation_id.as_ref(), Some(activation));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t18_child_source_failure_recovers_over_unix_at_the_same_activation_without_native_work() {
    let directory = inspection_tempdir().unwrap();
    let (parent, host) = parent_runtime_host_plane(
        &directory,
        "conv_01900000-0000-7000-8000-000000000478",
        Vec::new(),
    )
    .await;
    let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000479");
    let store_path = crate::runtime::subagent::child_conversation_store_path(
        &parent.plane.runtime_root,
        &crate::runtime::identity::SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
        &conversation,
    );
    let (release, released) = support::fake::model_release();
    let child = child_fixture_scoped_at(
        &directory,
        &conversation,
        vec![parking_answer_script(
            "Source repair preserves execution",
            released,
        )],
        ToolRegistry::new(),
        Vec::new(),
        store_path.parent().unwrap().to_path_buf(),
        Some(SubagentId::for_conversation(
            parent.runtime.conversation_id(),
            1,
        )),
    )
    .await;
    let wired = launch_wired_child(&parent.plane, &child, "held source").await;
    await_model_parks(&child.model, 1).await;
    let agent = &wired.accepted.child_agent_id;
    let mut inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            parent.plane.registry.agent_inspection_path(agent).unwrap(),
            child.host.clone(),
            wired.accepted.subagent_id.clone(),
        )
        .unwrap();
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    assert_child_source_failure_and_repair(
        &attachment,
        &child,
        agent,
        &wired.accepted.subagent_id,
        &store_path,
    )
    .await;
    release.send(true).unwrap();
    await_serve(wired.serve).await;
    parent
        .plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .unwrap();
    inspection.shutdown().await.unwrap();
    child.runtime.shutdown().await.unwrap();
    parent.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t20_real_unix_shutdown_retires_the_exact_pending_observe_before_its_deadline() {
    struct Unpark<'a>(&'a crate::runtime_client::host::ClientInner);
    impl Drop for Unpark<'_> {
        fn drop(&mut self) {
            self.0.unpark_projection_worker();
        }
    }
    let directory = inspection_tempdir().unwrap();
    let child = child_fixture_with_activation(
        &directory,
        &ConversationId::new("conv_01900000-0000-7000-8000-000000000104"),
        Vec::new(),
        ToolRegistry::new(),
        Vec::new(),
        SubagentId::new("inspection-observe-retirement"),
    )
    .await;
    child.host.inner.park_projection_worker();
    let unpark = Unpark(&child.host.inner);
    let before = child.host.inner.checkpoint().unwrap();
    let native = child.store.observation_read_cut().unwrap();
    let path = directory.path().join("observe-retirement.sock");
    let mut inspection =
        crate::local_runtime::live_inspection::LiveConversationInspectionServer::bind(
            path.clone(),
            child.host.clone(),
            SubagentId::new("inspection-observe-retirement"),
        )
        .unwrap();
    let workset = inspection.physical_workset();
    let (mut reader, mut writer) = connect_live_inspection(&path).await;
    let initialized = live_inspection_call(
        &mut reader,
        &mut writer,
        RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        },
    )
    .await;
    let Some(RuntimeClientResult::Initialized { attachment_id, .. }) = initialized.result else {
        panic!("the exact read-only endpoint was admitted");
    };
    let request = RuntimeClientRequest::ObserveEvents {
        id: crate::runtime_client::RequestId::new(2),
        after_cursor: before.cursor,
    };
    writer
        .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
        .await
        .unwrap();
    // Native subscription state establishes admission of this actual socket's
    // empty wait. The guard does not create the interleaving or retry a read.
    tokio::time::timeout(LIVENESS, async {
        while child
            .host
            .inner
            .inspection_subscriber_for_test(&attachment_id)
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual ObserveEvents subscription is installed");
    assert_eq!(*workset.borrow(), 1);
    tokio::time::timeout(std::time::Duration::from_secs(5), inspection.shutdown())
        .await
        .expect("server retirement does not wait for the ten-second observation deadline")
        .expect("the original listener joins successfully");
    assert_eq!(*workset.borrow(), 0);
    assert!(reader.next_line().await.unwrap().is_none());
    assert!(!path.exists());
    assert!(
        child
            .host
            .inner
            .inspection_subscriber_for_test(&attachment_id)
            .is_none()
    );
    assert_eq!(child.host.inner.checkpoint().unwrap(), before);
    assert_eq!(child.store.observation_read_cut().unwrap(), native);
    assert!(child.model.requests().is_empty());
    drop(unpark);
    child.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frozen_child_message_continuation_survives_live_admission_and_binds_registry_owner() {
    use crate::durable::reading::ConversationWindowAt;
    use crate::observation::cut::CheckpointReadCut;
    use crate::runtime_client::RuntimeClientError;
    use base64::Engine as _;

    let directory = inspection_tempdir().unwrap();
    let (parent, host) = parent_runtime_host_plane(
        &directory,
        "conv_01900000-0000-7000-8000-000000000601",
        Vec::new(),
    )
    .await;
    let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000602");
    let path = crate::runtime::subagent::child_conversation_store_path(
        &parent.plane.runtime_root,
        &crate::runtime::identity::SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
        &conversation,
    );
    let suffix = "Original frozen child instruction. ".repeat(4096);
    let mut message = support::fake::inbound_message("frozen-instruction", "", UserSource::Human);
    message.content = (0..32)
        .map(|n| {
            UserContentBlock::Text(crate::message::content::TextBlock {
                text: format!("Instruction part {n}"),
            })
        })
        .collect();
    message
        .content
        .push(UserContentBlock::Text(crate::message::content::TextBlock {
            text: suffix.clone(),
        }));
    let (release, released) = support::fake::model_release();
    let child = child_fixture_scoped_at(
        &directory,
        &conversation,
        vec![
            parking_answer_script("first answer", released),
            answer_script("follow-up answer"),
        ],
        ToolRegistry::new(),
        vec![MessageBlock::User(message)],
        path.parent().unwrap().to_path_buf(),
        Some(SubagentId::for_conversation(
            parent.runtime.conversation_id(),
            1,
        )),
    )
    .await;
    let wired = launch_wired_child(&parent.plane, &child, "inspect the frozen instruction").await;
    await_model_parks(&child.model, 1).await;
    let agent = &wired.accepted.child_agent_id;
    // No live inspection server: frozen pages and immutable canonical bytes
    // belong to the Registry-bound Child store even during live execution.
    let original = host
        .inner
        .agent_transcript_window(agent, &ConversationWindowAt::Latest, 32)
        .unwrap();
    let source = original.page.source_cut.clone().unwrap();
    let cut = CheckpointReadCut {
        conversation_id: conversation.clone(),
        activation_id: Some(wired.accepted.subagent_id.clone()),
        revision: 0,
        through: source.read.journal,
        source_cut: Some(source.clone()),
        canonical_foreground: None,
        live_publication: None,
        domain_revisions: std::collections::BTreeMap::new(),
        collection_cuts: std::collections::BTreeMap::new(),
    };
    let domain = "messages.message:frozen-instruction.blocks";
    let before = host
        .inner
        .agent_checkpoint_page(agent, &cut, domain, 32, 32)
        .await
        .unwrap();
    assert_eq!(before.read_cut, cut);
    assert_eq!(before.total, 33);
    assert_eq!(before.items.len(), 1);
    let reference = before.items[0]
        .content
        .iter()
        .find(|binding| {
            matches!(
                binding.content_ref.source,
                crate::observation::content::ContentSource::CanonicalMessageBlock {
                    component: crate::observation::content::CanonicalContentComponent::Payload,
                    ..
                }
            )
        })
        .unwrap()
        .content_ref
        .clone();
    host.inner
        .agent_send_message(
            agent,
            "Append guidance while the child is still running".into(),
            Vec::new(),
        )
        .await
        .unwrap();
    let current = child.store.observation_read_cut().unwrap();
    assert!(source.read.reconstructible_from(&current.read));
    assert_ne!(
        current, source,
        "live admission advances the pending/source frontier"
    );
    assert_eq!(
        child.model.requests().len(),
        1,
        "the live Child remains parked"
    );
    child.host.inner.projection_settled().await;
    let after = host
        .inner
        .agent_checkpoint_page(agent, &cut, domain, 32, 32)
        .await
        .unwrap();
    assert_eq!(
        after, before,
        "the same C0 window survives independent C1 admission"
    );
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = host
            .inner
            .agent_content_read(agent, &reference, offset, 65_536)
            .await
            .unwrap();
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(chunk.data_base64)
                .unwrap(),
        );
        if let Some(next) = chunk.next_offset {
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(String::from_utf8(bytes).unwrap(), suffix);
    for foreign in [
        CheckpointReadCut {
            activation_id: Some(SubagentId::new("foreign-activation")),
            ..cut.clone()
        },
        CheckpointReadCut {
            conversation_id: parent.runtime.conversation_id().clone(),
            ..cut.clone()
        },
        CheckpointReadCut {
            through: cut.through + 1,
            ..cut.clone()
        },
    ] {
        assert!(matches!(
            host.inner
                .agent_checkpoint_page(agent, &foreign, domain, 32, 32)
                .await,
            Err(RuntimeClientError::ReadCutExpired)
        ));
    }
    assert_eq!(
        child.store.observation_read_cut().unwrap(),
        current,
        "inspection is read-only"
    );
    release.send(true).unwrap();
    await_serve(wired.serve).await;
    parent
        .plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .unwrap();
    child.runtime.shutdown().await.unwrap();
    parent.runtime.shutdown().await.unwrap();
}
