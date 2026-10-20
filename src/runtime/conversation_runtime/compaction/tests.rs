use crate::runtime::conversation_runtime::test_support::*;

/// Manual compaction is a first-class idle maintenance operation: it
/// uses the configured summary invocation, commits one Derived checkpoint summary
/// without an attempt identity, and returns the conversation to the
/// coordinator before reporting success.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one linear ownership scenario
async fn manual_compaction_commits_and_restores_the_idle_conversation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, model) = headless_runtime(
        &dir,
        vec![
            one_turn_script(),
            text_turn_script("compact factual summary"),
        ],
        None,
        None,
    )
    .await;
    let host = crate::runtime_client::host::RuntimeClientHost::new(
        crate::runtime_client::host::RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        },
    )
    .expect("native client source owner");
    runtime.activate();
    let history = text_content(&"important history ".repeat(512));
    let settled = runtime.settlement_signal();
    let notification = settled.notified();
    tokio::pin!(notification);
    notification.as_mut().enable();
    runtime.submit_inbound(history).expect("accepted");
    notification.await;
    assert!(runtime.coordinator_ledger().is_some());
    let canonical_before = runtime.inner.store.load_canonical().unwrap();

    let release_gate = Arc::new(Gate::default());
    let _release_guard = release_gate.arm_scoped();
    runtime
        .inner
        .probe
        .lock()
        .unwrap()
        .get_or_insert_with(CoordinatorProbe::default)
        .manual_compaction_settlement_gate = Some(release_gate.clone());
    let requester = runtime.clone();
    let request = tokio::spawn(async move {
        requester
            .compact_context(Some("manual-request-435".to_owned().try_into().unwrap()))
            .await
    });
    release_gate.wait_entered();
    assert!(runtime.has_manual_compaction());
    assert!(!runtime.has_current_attempt());
    assert!(
        !request.is_finished(),
        "response cannot precede ownership return"
    );
    assert_eq!(
        runtime
            .inner
            .store
            .load_head()
            .unwrap()
            .compaction_generation,
        1
    );
    let before_release = host
        .checkpoint()
        .expect("frozen native compaction-start cut");
    let manual = before_release
        .snapshot
        .context
        .manual_compaction
        .as_ref()
        .unwrap();
    assert_eq!(
        manual.request_id.as_ref().unwrap().as_str(),
        "manual-request-435"
    );
    assert!(
        !manual.released,
        "display completion cannot precede coordinator ownership return"
    );
    assert!(before_release.snapshot.context.latest_compaction.is_none());
    release_gate.release();
    let outcome = request.await.unwrap().expect("manual compaction");
    host.projection_settled().await;
    assert_eq!(outcome.generation, 1);
    assert!(outcome.estimated_tokens_after < outcome.tokens_before.input_tokens);
    assert!(!runtime.has_current_attempt());
    let active = runtime
        .coordinator_active_ids()
        .expect("restored canonical working set");
    assert!(
        active.is_empty(),
        "a Derived summary cannot acquire canonical identity"
    );
    let checkpoint = runtime.inner.store.projection_head().unwrap().unwrap();
    assert_eq!(checkpoint.candidate.checkpoint_id, outcome.checkpoint_id);
    assert_eq!(checkpoint.activation.activation, outcome.activation_id);
    assert!(matches!(
        runtime.inner.store.projected_history().unwrap().messages[0],
        crate::model::ModelInputMessage::Derived(_)
    ));
    assert_eq!(model.requests().len(), 2, "turn plus summary request");

    let trace = crate::runtime_client::trace::TraceProjection::new(runtime.inner.store.as_ref())
        .expect("durable trace");
    let row = trace
        .page(None, 32)
        .unwrap()
        .records
        .into_iter()
        .find(|record| record.kind == crate::observation::trace::TraceKind::Compaction)
        .expect("manual compaction is retained in the trajectory");
    assert_eq!(row.state, crate::observation::trace::TraceState::Completed);
    assert!(row.message_id.is_none());
    let start = runtime
        .inner
        .store
        .read_events(None, 64)
        .unwrap()
        .events
        .into_iter()
        .find_map(|event| match event.event {
            crate::events::types::RuntimeEvent::CompactionStarted { operation_id } => {
                Some(operation_id)
            }
            _ => None,
        })
        .expect("original manual operation identity");
    assert_eq!(row.native_id, Some(start.to_string()));
    assert!(row.location.attempt_id.is_none());
    assert!(
        row.collections.is_empty(),
        "no fabricated canonical summary"
    );
    let crate::runtime_client::RuntimeClientResult::ProjectionDetail { detail } = host
        .inner
        .projection_detail(outcome.checkpoint_id.clone(), 0, 32)
        .unwrap()
    else {
        panic!("immutable ProjectionHistory detail")
    };
    assert_eq!(detail.checkpoint.checkpoint_id, outcome.checkpoint_id);
    assert_eq!(detail.checkpoint.activation_id, outcome.activation_id);
    assert_eq!(detail.items.len(), 1);
    assert!(matches!(&detail.items[0].content.blocks[0],
        crate::observation::trace::TraceContentBlock::Text { text }
            if text.text == "compact factual summary"));

    let completed = host.checkpoint().expect("native compaction completion cut");
    let manual = completed
        .snapshot
        .context
        .manual_compaction
        .as_ref()
        .unwrap();
    assert_eq!(
        manual.request_id.as_ref().unwrap().as_str(),
        "manual-request-435"
    );
    assert!(manual.released);
    assert_eq!(
        completed
            .snapshot
            .context
            .latest_compaction
            .as_ref()
            .unwrap()
            .generation,
        1
    );
    assert_eq!(
        completed
            .snapshot
            .context
            .projection_head
            .as_ref()
            .unwrap()
            .checkpoint_id,
        outcome.checkpoint_id
    );
    assert_eq!(
        completed
            .snapshot
            .context
            .projection_head
            .as_ref()
            .unwrap()
            .activation_id,
        outcome.activation_id
    );
    assert_eq!(
        runtime.inner.store.load_canonical().unwrap(),
        canonical_before
    );
    assert!(completed.cursor > before_release.cursor);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compaction_lost_commit_ack_publishes_the_original_receipt_after_owner_release() {
    let directory = tempfile::tempdir().unwrap();
    let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000480");
    let sqlite =
        Arc::new(crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap());
    let (runtime, model) = headless_runtime_over_store_with(
        &directory,
        conversation.as_str(),
        sqlite.clone(),
        vec![
            one_turn_script(),
            text_turn_script("recovered factual summary"),
        ],
        None,
    )
    .await;
    let host = crate::runtime_client::RuntimeClientHost::new(
        crate::runtime_client::RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        },
    )
    .unwrap();
    runtime.activate();
    runtime
        .submit_inbound(text_content(&"important original fact ".repeat(512)))
        .unwrap();
    await_settled_ledger(&runtime, 0).await;
    host.projection_settled().await;
    let canonical = sqlite.load_canonical().unwrap();
    let before = host.checkpoint().unwrap();
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let subscription = attachment.subscribe_events(before.cursor).unwrap();
    let gate = Arc::new(Gate::default());
    let _release = gate.arm_scoped();
    runtime
        .inner
        .probe
        .lock()
        .unwrap()
        .get_or_insert_with(CoordinatorProbe::default)
        .manual_compaction_settlement_gate = Some(gate.clone());
    sqlite.arm_compaction_fault_script([
        crate::durable::sqlite::CompactionFaultOperation::AfterCommit,
    ]);
    let requester = runtime.clone();
    let request = tokio::spawn(async move { requester.compact_context(None).await });
    gate.wait_entered();
    assert!(runtime.has_manual_compaction());
    assert!(!request.is_finished());
    let original = recovered_manual_receipt(&sqlite, &host);
    gate.release();
    let outcome = request.await.unwrap().unwrap();
    assert_eq!(
        outcome.checkpoint_id,
        original.checkpoint.candidate.checkpoint_id
    );
    host.projection_settled().await;
    assert_recovered_manual_publication(&host, &subscription, &before, &original);
    assert_eq!(sqlite.load_canonical().unwrap(), canonical);
    assert_eq!(
        model.requests().len(),
        2,
        "lost acknowledgment never reruns the summary"
    );
    assert_eq!(
        sqlite
            .read_events(None, 64)
            .unwrap()
            .events
            .iter()
            .filter(|event| matches!(
                event.event,
                crate::events::types::RuntimeEvent::CompactionCompleted { .. }
            ))
            .count(),
        1
    );
    runtime.shutdown().await.unwrap();
}

/// Manual admission freezes capability-derived Skill guidance as
/// non-retirable request input. The same history can retire one message
/// without that guidance, but must retire two when the frozen catalog
/// makes the one-message candidate exceed the exact soft limit.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compaction_exact_fit_includes_frozen_skill_guidance() {
    fn estimator() -> Arc<dyn TokenEstimator> {
        Arc::new(ClosureTokenEstimator::new(
            |messages, effective_system_prompt, _tools| {
                let conversation = messages
                    .iter()
                    .map(|message| {
                        if matches!(
                            message,
                            crate::model::ModelInputMessage::Derived(derived)
                                if derived.transformations.contains(&crate::model::projected::SemanticTransformation::Summary)
                        ) {
                            10_000
                        } else {
                            350_000
                        }
                    })
                    .sum::<u64>();
                conversation
                    + if effective_system_prompt.is_empty() {
                        0
                    } else {
                        400_000
                    }
            },
        ))
    }

    fn history() -> Vec<MessageBlock> {
        vec![
            seed_user("oldest", "oldest fact"),
            seed_user("middle", "middle fact"),
            seed_user("recent", "recent fact"),
        ]
    }

    let without_skill = tempfile::tempdir().expect("temp dir");
    let (runtime_without_skill, _) = headless_runtime_with_options(
        &without_skill,
        vec![text_turn_script("S")],
        None,
        None,
        HeadlessRuntimeOptions {
            estimator: estimator(),
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 700_000,
                summary_output_cap: None,
            },
            initial_messages: history(),
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime_without_skill.activate();
    let without_outcome = runtime_without_skill
        .compact_context(None)
        .await
        .expect("one-message retirement fits without Skill guidance");
    assert_eq!(without_outcome.tokens_before.input_tokens, 1_050_000);
    assert_eq!(without_outcome.estimated_tokens_after, 710_000);
    assert_eq!(
        runtime_without_skill
            .coordinator_active_ids()
            .expect("restored without Skill guidance"),
        vec![
            crate::runtime::identity::MessageId::new("middle"),
            crate::runtime::identity::MessageId::new("recent"),
        ],
        "the retention target keeps the one-message candidate when it truly fits"
    );

    let with_skill = tempfile::tempdir().expect("temp dir");
    let skill = with_skill
        .path()
        .join("workspace/.agents/skills/exact-fit-skill");
    std::fs::create_dir_all(&skill).expect("skill package");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: exact-fit-skill\ndescription: Frozen fit guidance.\n---\n\nInstructions.\n",
    )
    .expect("SKILL.md");
    let (runtime_with_skill, _) = headless_runtime_with_options(
        &with_skill,
        vec![text_turn_script("S")],
        Some(with_native_read(ToolRegistry::new())),
        None,
        HeadlessRuntimeOptions {
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                skill.parent().unwrap(),
            ),
            estimator: estimator(),
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 700_000,
                summary_output_cap: None,
            },
            initial_messages: history(),
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime_with_skill.activate();
    let with_outcome = runtime_with_skill
        .compact_context(None)
        .await
        .expect("a larger retirement span fits with frozen Skill guidance");
    assert_eq!(with_outcome.tokens_before.input_tokens, 1_450_000);
    assert_eq!(with_outcome.estimated_tokens_after, 760_000);
    assert_eq!(
        runtime_with_skill
            .coordinator_active_ids()
            .expect("restored with Skill guidance"),
        vec![crate::runtime::identity::MessageId::new("recent"),],
        "the frozen catalog makes the one-message candidate fail, so planning retires two"
    );
}

/// A rejected manual compaction never strands the checked-out state. An
/// empty conversation has no retirable span, and ordinary inbound still
/// admits immediately after that typed failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compaction_no_progress_restores_state_for_future_admission() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) = headless_runtime(&dir, vec![one_turn_script()], None, None).await;
    runtime.activate();
    let error = runtime
        .compact_context(None)
        .await
        .expect_err("empty context cannot compact");
    assert!(matches!(
        error,
        ManualCompactionError::Context(ContextError {
            kind: ContextErrorKind::NoProgress,
            ..
        })
    ));
    assert!(runtime.coordinator_ledger().is_some());
    let trace = crate::runtime_client::trace::TraceProjection::new(runtime.inner.store.as_ref())
        .expect("durable trace");
    let row = trace
        .page(None, 32)
        .unwrap()
        .records
        .into_iter()
        .find(|record| record.kind == crate::observation::trace::TraceKind::Compaction)
        .expect("failed maintenance keeps its own trajectory row");
    assert_eq!(row.state, crate::observation::trace::TraceState::Failed);
    assert!(row.message_id.is_none());
    runtime
        .submit_inbound(text_content("works after rejected compaction"))
        .expect("accepted after failure");
    let _ = await_settled_ledger(&runtime, 0).await;
    assert!(!runtime.has_current_attempt());
}

/// Manual compaction never aborts or races an active Agent attempt. The
/// caller receives a typed busy error and the provider turn keeps its
/// original cancellation/settlement authority.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compaction_rejects_a_running_attempt_without_cancelling_it() {
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;

    let (release_sender, release_receiver) = model_release();
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, model) = headless_runtime(
        &dir,
        vec![vec![
            FakeStep::ParkUntilReleased(release_receiver),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
        None,
        None,
    )
    .await;
    runtime.activate();
    runtime
        .submit_inbound(text_content("keep this attempt running"))
        .expect("accepted");
    let mut parked = model.parked();
    within_liveness_guard("manual compaction busy model park", async {
        parked.wait_for(|is_parked| *is_parked).await
    })
    .await
    .expect("model park watch remains open");

    assert_eq!(
        runtime.compact_context(None).await,
        Err(ManualCompactionError::Busy)
    );
    assert!(runtime.has_current_attempt());
    release_sender.send(true).expect("release provider");
    let _ = await_settled_ledger(&runtime, 0).await;
    assert!(!runtime.has_current_attempt());
}
