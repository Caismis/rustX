// Frozen-source reads do not acquire the observation preparation owner.
use super::*;
use base64::Engine as _;

struct GatedProgressTool {
    stage: watch::Sender<u8>,
    report: watch::Receiver<bool>,
    release: watch::Receiver<bool>,
}

impl ToolExecutor for GatedProgressTool {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let stage = self.stage.clone();
        let mut report = self.report.clone();
        let mut release = self.release.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                stage.send_replace(1);
                report.wait_for(|ready| *ready).await.unwrap();
                context.progress.report(crate::tools::types::ToolProgress {
                    message: Some("native queued progress".into()),
                    completed: Some(1.0),
                    total: Some(2.0),
                });
                stage.send_replace(2);
                release.wait_for(|ready| *ready).await.unwrap();
                durable_tool_result()
            }),
            context.cancellation.clone(),
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::Meaningful
    }
}

fn tool_script(definition: &ToolDefinition) -> Vec<GatedStep> {
    let call = ToolCall {
        id: ToolCallId::new("pure-read-call"),
        tool_id: definition.id.clone(),
        name: definition.name.clone(),
        arguments: serde_json::json!({}),
    };
    vec![
        GatedStep::Emit(ModelEvent::Started),
        GatedStep::Emit(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCallStart {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: call.name.clone(),
            },
        }),
        GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(0),
            call_id: call.id.clone(),
            arguments_delta: "{}".into(),
        }),
        GatedStep::Emit(ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call,
        }),
        GatedStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ]
}

fn read_frozen_sources(
    inner: &super::super::ClientInner,
    checkpoint: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    original: &str,
) {
    let page = inner
        .checkpoint_page(&checkpoint.read_cut, "messages", 0, 32)
        .unwrap();
    assert_eq!(page.read_cut, checkpoint.read_cut);
    let _domain = inner.domain_page("capabilities", 0, 1).unwrap();
    let scalar = inner
        .checkpoint_value_read(
            &checkpoint.read_cut,
            "capabilities",
            0,
            &["revision".into()],
            0,
            64,
        )
        .unwrap();
    assert_eq!(scalar.cut, checkpoint.read_cut);
    assert!(scalar.next_offset.is_none());
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(scalar.bytes_base64)
            .unwrap(),
        serde_json::to_vec(&checkpoint.snapshot.capabilities.revision.get().to_string()).unwrap()
    );
    let reference = checkpoint
        .content
        .iter()
        .find(|binding| {
            binding.content_ref.representation
                == crate::observation::content::ContentRepresentation::Utf8Text
                && binding.content_ref.readable_end == original.len() as u64
        })
        .unwrap()
        .content_ref
        .clone();
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = inner.content_read(&reference, offset, 65_536).unwrap();
        assert_eq!(chunk.content_ref, reference);
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(chunk.data_base64)
                .unwrap(),
        );
        if let Some(next) = chunk.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(bytes, original.as_bytes());
    let _transcript = inner.transcript_page(None, 32).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t14_t19_pages_and_value_reads_leave_native_progress_queued() {
    let (stage, mut stages) = watch::channel(0);
    let (report, report_rx) = model_release();
    let (release, release_rx) = model_release();
    let (base, _, _) = ParkingBackgroundTool::new();
    let definition = ToolDefinition {
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        ..base.definition
    };
    let mut tools = ToolRegistry::new();
    tools
        .register(
            definition.clone(),
            Arc::new(GatedProgressTool {
                stage,
                report: report_rx,
                release: release_rx,
            }),
        )
        .unwrap();
    let (_, fixture) = host_fixture(
        vec![tool_script(&definition), one_turn_stop()],
        tools,
        status_engine(),
    )
    .await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let original = "frozen original π🙂".repeat(8_000);
    inner.submit_inbound(submit_content(&original)).unwrap();
    stages.wait_for(|stage| *stage == 1).await.unwrap();
    inner.park_projection_worker();
    let checkpoint = inner.checkpoint().unwrap();
    assert!(matches!(
        checkpoint.snapshot.attempt.as_ref().unwrap().phase,
        RuntimeClientAttemptPhase::Running
    ));
    report.send_replace(true);
    stages.wait_for(|stage| *stage == 2).await.unwrap();
    let queued = inner.queued_observation_records();
    assert!(
        queued > 0,
        "the actual executor reported queued native progress"
    );
    let preparation = inner.observation_preparation.lock().unwrap();
    let reader = Arc::clone(&inner);
    let before = checkpoint.clone();
    let expected = original.clone();
    let (completed, completion) = std::sync::mpsc::channel();
    let task = tokio::task::spawn_blocking(move || {
        read_frozen_sources(&reader, &before, &expected);
        completed.send(()).unwrap();
    });
    let result = completion.recv_timeout(BACKGROUND_LIVENESS_GUARD);
    drop(preparation);
    result.expect("all source reads complete while observation preparation is owned elsewhere");
    task.await.unwrap();
    assert_eq!(inner.queued_observation_records(), queued);
    let after = inner.checkpoint().unwrap();
    assert_eq!(
        after, checkpoint,
        "source reads leave the complete published cut unchanged"
    );
    inner.unpark_projection_worker();
    release.send_replace(true);
    let settlement = fixture.runtime.child_attempt_settlement(0).await;
    assert_eq!(
        Some(settlement.attempt_id),
        checkpoint.snapshot.attempt.as_ref().unwrap().attempt_id
    );
    fixture.runtime.shutdown().await.unwrap();
}
