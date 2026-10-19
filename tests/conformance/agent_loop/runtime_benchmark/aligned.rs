//! Aligned native observation adapters. No model/core/persistence substitutions.
use super::*;
use rustx::durable::presentation::{FactQuery, FactScope};
use rustx::local_runtime::LocalConversationRuntime;
use rustx::runtime_client::attachment::RuntimeAttachment;
use rustx::runtime_client::host::EventDelivery;
use rustx::runtime_client::types::{RequestId, RuntimeClientRequest, RuntimeClientResult};
use rustx::runtime_client::{RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientCursor};
use std::collections::BTreeMap;

#[derive(Default)]
struct Display {
    cursor: u64,
    records: u64,
    tools: u64,
    settled: usize,
    first_content: Option<Instant>,
    text: BTreeMap<(String, String), String>,
    ended: BTreeMap<String, RuntimeClientOutcome>,
    explicit_ended: BTreeMap<String, RuntimeClientOutcome>,
    checkpoint_terminal_sightings: usize,
    tool_results: Vec<serde_json::Value>,
    outcomes: Vec<RuntimeClientOutcome>,
}

impl Display {
    fn terminal(&mut self, id: &str, outcome: &RuntimeClientOutcome) {
        if let Some(previous) = self.ended.get(id) {
            assert_eq!(previous, outcome);
            return;
        }
        self.ended.insert(id.to_owned(), outcome.clone());
        self.outcomes.push(outcome.clone());
        self.settled += 1;
    }
    fn explicit_terminal(&mut self, id: &str, outcome: &RuntimeClientOutcome) {
        assert!(
            !self.explicit_ended.contains_key(id),
            "duplicate explicit AttemptSettled"
        );
        self.explicit_ended.insert(id.to_owned(), outcome.clone());
        self.terminal(id, outcome);
    }
    fn checkpoint_terminal(&mut self, id: &str, outcome: &RuntimeClientOutcome) {
        self.checkpoint_terminal_sightings += 1;
        self.terminal(id, outcome);
    }
    fn text_value(&mut self, id: &str, index: String, text: String) {
        let prior = self.text.entry((id.to_owned(), index)).or_default();
        assert!(text.starts_with(prior.as_str()), "visible prefix changed");
        let advanced = text.len() > prior.len();
        *prior = text;
        if advanced && self.first_content.is_none() {
            self.first_content = Some(Instant::now());
        }
    }
    fn descriptor(
        &mut self,
        descriptor: &rustx::runtime_client::snapshot::RuntimeClientMessageDescriptor,
        attachment: &RuntimeAttachment,
    ) {
        use rustx::runtime_client::content::{
            CanonicalContentComponent, ContentRepresentation, ContentSource,
        };
        if descriptor.role != "assistant" {
            return;
        }
        for binding in &descriptor.content {
            if binding.content_ref.representation != ContentRepresentation::Utf8Text {
                continue;
            }
            if let ContentSource::CanonicalMessageBlock {
                component: CanonicalContentComponent::Payload,
                block_index,
                ..
            } = &binding.content_ref.source
            {
                let text =
                    String::from_utf8(read_content(attachment, &binding.content_ref, 0)).unwrap();
                self.text_value(
                    descriptor.identity.as_ref().unwrap().as_str(),
                    format!("{block_index:?}"),
                    text,
                );
            }
        }
    }
    fn checkpoint(
        &mut self,
        checkpoint: &rustx::runtime_client::checkpoint::RuntimeClientCheckpoint,
        attachment: &RuntimeAttachment,
    ) {
        use rustx::runtime_client::snapshot::InFlightBlock;
        for descriptor in &checkpoint.snapshot.message_descriptors {
            self.descriptor(descriptor, attachment);
        }
        if let Some(attempt) = &checkpoint.snapshot.attempt {
            if let Some(inflight) = &attempt.in_flight {
                for (ordinal, block) in inflight.blocks.iter().enumerate() {
                    if let InFlightBlock::Text { block_index, text } = block {
                        let path = vec![
                            "attempt".to_owned(),
                            "in_flight".into(),
                            "blocks".into(),
                            ordinal.to_string(),
                            "text".into(),
                        ];
                        let value = if let Some(binding) = checkpoint
                            .content
                            .iter()
                            .find(|binding| binding.path == path && binding.deferred)
                        {
                            String::from_utf8(read_content(attachment, &binding.content_ref, 0))
                                .unwrap()
                        } else {
                            text.clone()
                        };
                        self.text_value(
                            inflight.message_id.as_str(),
                            format!("{block_index:?}"),
                            value,
                        );
                    }
                }
            }
            if let RuntimeClientAttemptPhase::Settled { outcome } = &attempt.phase {
                self.checkpoint_terminal(attempt.attempt_id.as_ref().unwrap().as_str(), outcome);
            }
        }
    }
    fn consume(&mut self, event: &RuntimeClientEvent, attachment: &RuntimeAttachment) {
        for fact in event.facts() {
            match fact {
                RuntimeClientEvent::BlockContentAdvanced {
                    message_id,
                    block_index,
                    kind,
                    base_length,
                    end_length,
                    inline_delta,
                    content_ref,
                    ..
                } if kind == "text" => {
                    let text = self
                        .text
                        .entry((message_id.to_string(), format!("{block_index:?}")))
                        .or_default();
                    assert_eq!(text.len() as u64, *base_length, "ordered visible prefix");
                    if let Some(delta) = inline_delta {
                        text.push_str(delta);
                    } else {
                        let bytes = read_content(attachment, content_ref, *base_length);
                        text.push_str(std::str::from_utf8(&bytes).unwrap());
                    }
                    assert_eq!(text.len() as u64, *end_length);
                    if !text.is_empty() && self.first_content.is_none() {
                        self.first_content = Some(Instant::now());
                    }
                }
                RuntimeClientEvent::ToolExecutionSettled { .. } => {
                    self.tools += 1;
                }
                RuntimeClientEvent::ToolResultCommitted {
                    result,
                    result_deferred,
                    ..
                } => {
                    assert!(!result_deferred, "small fixture result must be inline");
                    self.tool_results
                        .push(serde_json::to_value(result).unwrap());
                }
                RuntimeClientEvent::AttemptSettled {
                    attempt_id,
                    outcome,
                } => self.explicit_terminal(attempt_id.as_str(), outcome),
                RuntimeClientEvent::MessageCommitted {
                    message: MessageBlock::Assistant(message),
                    ..
                } => {
                    for (index, block) in message.content.iter().enumerate() {
                        if let rustx::message::types::AssistantContentBlock::Text(text) = block {
                            let index = format!(
                                "{:?}",
                                rustx::message::types::ContentBlockIndex::new(
                                    u32::try_from(index).unwrap()
                                )
                            );
                            let key = (message.id.to_string(), index.clone());
                            if let Some(full) = self
                                .text
                                .get(&key)
                                .filter(|full| full.len() > text.text.len())
                            {
                                // The finite inline display may follow a descriptor whose
                                // ContentRef we already consumed in full. Validate its exact
                                // prefix without replacing the larger authoritative content.
                                assert!(
                                    full.starts_with(&text.text),
                                    "inline display prefix changed"
                                );
                            } else {
                                self.text_value(message.id.as_str(), index, text.text.clone());
                            }
                        }
                    }
                }
                RuntimeClientEvent::MessageDescriptorCommitted { descriptor, .. } => {
                    self.descriptor(descriptor, attachment);
                }
                RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                    self.checkpoint(checkpoint, attachment);
                }
                _ => {}
            }
        }
    }
}

fn read_content(
    attachment: &RuntimeAttachment,
    reference: &rustx::runtime_client::content::ContentRef,
    mut offset: u64,
) -> Vec<u8> {
    use base64::Engine as _;
    let mut bytes = Vec::new();
    while offset < reference.readable_end {
        let response = attachment.handle_request(RuntimeClientRequest::ContentRead {
            id: RequestId::new(1),
            content_ref: reference.clone(),
            offset,
            max_bytes: 65536,
        });
        assert!(response.error.is_none(), "{response:?}");
        let Some(RuntimeClientResult::ContentChunk { chunk }) = response.result else {
            panic!("content chunk");
        };
        assert_eq!(chunk.content_ref, *reference);
        assert_eq!(chunk.offset, offset);
        assert!(chunk.end_offset > offset && chunk.end_offset <= reference.readable_end);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap();
        assert_eq!(part.len() as u64, chunk.end_offset - offset);
        bytes.extend(part);
        offset = chunk.end_offset;
    }
    bytes
}

fn verify_ordered_messages(actual: &[serde_json::Value], expected: &[MessageBlock]) {
    let mut projected = serde_json::to_value(expected).unwrap();
    for (value, message) in projected.as_array_mut().unwrap().iter_mut().zip(expected) {
        if let MessageBlock::Tool(tool) = message {
            // Public structural JSON encodes this native u64 as exact decimal text.
            // Compare that declared wire representation, retaining every digit.
            value["result"]["duration_ms"] = json!(tool.result.duration_ms.to_string());
        }
    }
    assert_eq!(
        actual,
        projected.as_array().unwrap(),
        "full ordered late-snapshot messages differ from native Originals"
    );
}

fn hydrate_display_value(
    mut value: serde_json::Value,
    bindings: &[rustx::runtime_client::checkpoint::ContentBinding],
    attachment: &RuntimeAttachment,
) -> serde_json::Value {
    for binding in bindings.iter().filter(|binding| binding.deferred) {
        let bytes = read_content(attachment, &binding.content_ref, 0);
        let full = match binding.content_ref.representation {
            rustx::runtime_client::content::ContentRepresentation::Utf8Text => {
                serde_json::Value::String(String::from_utf8(bytes).unwrap())
            }
            rustx::runtime_client::content::ContentRepresentation::CanonicalJsonV1
            | rustx::runtime_client::content::ContentRepresentation::RawJsonUtf8 => {
                serde_json::from_slice(&bytes).unwrap()
            }
            rustx::runtime_client::content::ContentRepresentation::BinaryBytes => {
                panic!("binary payload is outside these text/tool fixtures")
            }
        };
        let mut target = &mut value;
        for component in &binding.path {
            target = if target.is_array() {
                target.get_mut(component.parse::<usize>().unwrap()).unwrap()
            } else {
                target.get_mut(component).unwrap()
            };
        }
        *target = full;
    }
    value
}

#[test]
fn ordered_late_oracle_rejects_missing_reordered_and_changed_messages() {
    let expected: Vec<MessageBlock> = serde_json::from_value(json!([
        {"role":"assistant","id":"a","content":[{"type":"text","text":"first"}]},
        {"role":"assistant","id":"b","content":[{"type":"text","text":"second"}]}
    ]))
    .unwrap();
    let good = serde_json::to_value(&expected)
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    verify_ordered_messages(&good, &expected);
    let mut reordered = good.clone();
    reordered.swap(0, 1);
    let mut changed = good.clone();
    changed[0]["content"][0]["text"] = json!("mutated");
    for bad in [good[..1].to_vec(), reordered, changed] {
        assert!(std::panic::catch_unwind(|| verify_ordered_messages(&bad, &expected)).is_err());
    }
}

#[test]
fn display_prefix_rejects_mutation_and_does_not_retime_unchanged_content() {
    let mut display = Display::default();
    display.text_value("a", "0".into(), "first".into());
    display.first_content = None;
    display.text_value("a", "0".into(), "first".into());
    assert!(display.first_content.is_none());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| display.text_value(
            "a",
            "0".into(),
            "changed".into()
        )))
        .is_err()
    );
}

#[test]
fn explicit_terminal_duplicates_fail_while_checkpoint_sightings_remain_idempotent() {
    let mut display = Display::default();
    let outcome = RuntimeClientOutcome::Completed {
        finish_reason: rustx::model::ModelFinishReason::Stop,
        finish_reason_deferred: false,
        finish_reason_source: None,
    };
    display.checkpoint_terminal("attempt", &outcome);
    display.explicit_terminal("attempt", &outcome);
    display.checkpoint_terminal("attempt", &outcome);
    assert_eq!(display.outcomes.len(), 1);
    assert_eq!(display.checkpoint_terminal_sightings, 2);
    assert_eq!(display.explicit_ended.len(), 1);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            display.explicit_terminal("attempt", &outcome);
        }))
        .is_err()
    );
    assert_eq!(display.explicit_ended.len(), 1);
    assert_eq!(display.explicit_ended["attempt"], outcome);
}

struct NativeDriver {
    root: tempfile::TempDir,
    runtime: LocalConversationRuntime,
    store: Arc<SqliteConversationStore>,
    database: PathBuf,
    attachment: Arc<RuntimeAttachment>,
    initial_cursor: RuntimeClientCursor,
    display: Arc<std::sync::Mutex<Display>>,
    progress: tokio::sync::watch::Receiver<usize>,
    relay: Option<tokio::task::JoinHandle<()>>,
}
impl NativeDriver {
    async fn start(root: tempfile::TempDir, emulator: &ProviderEmulator, live: bool) -> Self {
        let setup = Setup::new("emulator/chat-model");
        std::fs::write(
            root.path().join("rustx.toml"),
            format!(
                "{}\n{}",
                session_json(&setup),
                models_json(emulator, &setup)
            ),
        )
        .unwrap();
        let paths = LaunchFixture {
            config: root.path().join("rustx.toml"),
            workspace: root.path().join("workspace"),
            runtime_root: root.path().join("private"),
            startup_session: rustx::local_runtime::StartupSession::Empty,
        };
        let runtime = paths
            .compose(&LocalRuntimeDependencies {
                status_clock: Arc::new(request_oracle::FixedStatusClock::default()),
                credentials: Some(Arc::new(MapCredentialEnvironment::new([(
                    CREDENTIAL_VARIABLE.to_owned(),
                    CREDENTIAL_VALUE.to_owned(),
                )]))),
                child_program: Some(PathBuf::from(env!("CARGO_BIN_EXE_rustx"))),
                ..LocalRuntimeDependencies::default()
            })
            .await
            .unwrap();
        let cid = runtime.runtime().conversation_id().clone();
        let sessions = rustx::local_runtime::SessionCatalog::open_existing(&paths.runtime_root)
            .unwrap()
            .unwrap()
            .persisted_session_ids();
        assert_eq!(sessions.len(), 1);
        let database = paths
            .runtime_root
            .join("sessions")
            .join(sessions[0].as_str())
            .join("conversations")
            .join(cid.as_str())
            .join("conversation.sqlite");
        let store = Arc::new(SqliteConversationStore::open_existing(cid, &database).unwrap());
        let (attachment, _) = runtime
            .host()
            .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let attachment = Arc::new(attachment);
        let initial_cursor = attachment.checkpoint().unwrap().cursor;
        let display = Arc::new(std::sync::Mutex::new(Display {
            cursor: initial_cursor.get(),
            ..Display::default()
        }));
        let (progress_tx, progress) = tokio::sync::watch::channel(0);
        let relay = if live {
            let subscription = attachment.subscribe_events(initial_cursor).unwrap();
            let target = Arc::clone(&display);
            let reader = Arc::clone(&attachment);
            Some(tokio::spawn(async move {
                loop {
                    match subscription.next().await {
                        EventDelivery::Event(event) => {
                            let mut state = target.lock().unwrap();
                            assert_eq!(event.cursor.get(), state.cursor + 1);
                            state.cursor = event.cursor.get();
                            state.records += 1;
                            state.consume(&event.event, &reader);
                            progress_tx.send_replace(state.settled);
                        }
                        EventDelivery::Closed => break,
                        other => panic!("aligned subscriber failed: {other:?}"),
                    }
                }
            }))
        } else {
            None
        };
        Self {
            root,
            runtime,
            store,
            database,
            attachment,
            initial_cursor,
            display,
            progress,
            relay,
        }
    }
    async fn observed(&mut self, turns: usize) {
        while *self.progress.borrow_and_update() < turns {
            self.progress.changed().await.unwrap();
        }
    }
    async fn late_snapshot(&self) -> serde_json::Value {
        let reader = Arc::clone(&self.attachment);
        let checkpoint = tokio::task::spawn_blocking(move || reader.checkpoint())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            checkpoint.snapshot.conversation_id,
            *self.runtime.runtime().conversation_id()
        );
        assert!(checkpoint.snapshot.durability_failure.is_none());
        assert!(matches!(
            checkpoint.snapshot.attempt.as_ref().unwrap().phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
        // A new subscription must either replay a contiguous retained prefix or
        // explicitly reject an evicted cut; it must never silently skip history.
        let replay = match self.attachment.subscribe_events(self.initial_cursor) {
            Ok(events) => {
                let mut cursor = self.initial_cursor.get();
                while cursor < checkpoint.cursor.get() {
                    match events.try_next() {
                        EventDelivery::Event(event) => {
                            assert_eq!(event.cursor.get(), cursor + 1);
                            cursor = event.cursor.get();
                        }
                        other => panic!("serviceable replay incomplete: {other:?}"),
                    }
                }
                json!({"kind":"contiguous","after":self.initial_cursor.get(),"through":cursor})
            }
            Err(rustx::runtime_client::RuntimeClientError::ResyncRequired { .. }) => {
                json!({"kind":"resync_required","after":self.initial_cursor.get(),"snapshot":checkpoint.cursor.get()})
            }
            Err(error) => panic!("late replay failed: {error:?}"),
        };
        // Read every historical message through the public, cut-bound page and
        // ContentRef surface. No bounded inline preview can stand in for an Original.
        let mut messages = Vec::new();
        let mut offset = 0;
        loop {
            let response =
                self.attachment
                    .handle_request(RuntimeClientRequest::CheckpointPageGet {
                        id: RequestId::new(2),
                        cut: checkpoint.read_cut.clone(),
                        domain: "messages".into(),
                        offset,
                        limit: 32,
                    });
            assert!(response.error.is_none(), "{response:?}");
            let Some(RuntimeClientResult::CheckpointPage { page }) = response.result else {
                panic!("historical checkpoint page");
            };
            assert_eq!(page.read_cut, checkpoint.read_cut);
            assert_eq!(page.domain, "messages");
            assert!(page.items.len() <= 32);
            for (index, item) in page.items.iter().enumerate() {
                assert_eq!(item.offset, offset + index as u64);
                messages.push(hydrate_display_value(
                    item.value.clone().expect("fixture display"),
                    &item.content,
                    &self.attachment,
                ));
            }
            offset += page.items.len() as u64;
            assert_eq!(page.next_offset, (offset < page.total).then_some(offset));
            if page.next_offset.is_none() {
                assert_eq!(offset, page.total);
                break;
            }
            assert!(!page.items.is_empty());
        }
        json!({"checkpoint":checkpoint,"replay":replay,"messages":messages})
    }
}

impl NativeDriver {
    fn canonical_history(&self) -> Vec<MessageBlock> {
        let cut = self
            .store
            .observation_read_cut()
            .expect("native history cut");
        let mut offset = 0;
        let mut messages = Vec::new();
        loop {
            let page = self
                .store
                .message_header_page(&cut, Some(offset), 32)
                .expect("bounded canonical history page");
            assert_eq!(page.cut, cut);
            assert_eq!(page.offset, offset);
            assert_eq!(page.total, cut.ledger_through);
            assert!(page.headers.len() <= 32);
            let ids: Vec<_> = page
                .headers
                .iter()
                .enumerate()
                .map(|(index, header)| {
                    assert_eq!(
                        header.ledger_position,
                        Some(offset + u64::try_from(index).expect("bounded header ordinal") + 1)
                    );
                    header
                        .identity
                        .clone()
                        .expect("small original fixture identity")
                })
                .collect();
            let originals = self
                .store
                .load_messages(&ids)
                .expect("selected original bodies");
            assert_eq!(originals.len(), ids.len());
            for (original, identity) in originals.iter().zip(&ids) {
                assert_eq!(original.id(), identity);
            }
            messages.extend(originals);
            offset += u64::try_from(page.headers.len()).expect("bounded page count");
            assert_eq!(page.next_offset, (offset < page.total).then_some(offset));
            if page.next_offset.is_none() {
                assert_eq!(u64::try_from(messages.len()).unwrap(), cut.ledger_through);
                return messages;
            }
            assert!(!page.headers.is_empty(), "native continuation advances");
        }
    }
}

pub(super) async fn measure(mode: &str) {
    let case = std::env::var("RUSTX_BENCH_CASE").expect("case");
    let output = PathBuf::from(std::env::var_os("RUSTX_BENCH_OUTPUT").expect("output"));
    let (history, measured) = match case.as_str() {
        "short-small" => (8, 32),
        "short-large" => (64, 32),
        "stream" | "large" | "tool" | "cancel" => (0, 1),
        _ => panic!("unknown benchmark case"),
    };
    let lifecycle_began = Instant::now();
    let mut lifecycle = Vec::new();
    lifecycle_sample(&mut lifecycle, "before_initialization", lifecycle_began);
    let root = tempfile::tempdir_in(output.parent().unwrap()).unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join(NOTE_PATH), NOTE_MARKER).unwrap();
    let scenario = format!("runtime_benchmark_{case}");
    // The external controller owns both siblings. Provider reaping can never
    // enter the product's /proc/self/io counters, even on failure paths.
    println!(
        "RUSTX_BENCH_READY {}",
        json!({"workspace":workspace,"scenario":scenario})
    );
    let mut ready = String::new();
    std::io::stdin().read_line(&mut ready).unwrap();
    let emulator = ProviderEmulator::externally_owned(
        &serde_json::from_str(&ready).expect("controller provider readiness"),
        &scenario,
    );
    lifecycle_sample(&mut lifecycle, "after_provider_start", lifecycle_began);
    let began = Instant::now();
    let mut driver = NativeDriver::start(root, &emulator, mode == "live-subscriber").await;
    let open_us = began.elapsed().as_micros();
    let initial = process_sample();
    lifecycle_sample(&mut lifecycle, "after_initialization", lifecycle_began);
    let mut samples = Vec::new();
    for turn in 0..history + measured {
        if turn == history {
            profiling::boundary("before_measured_turns");
        }
        let began = Instant::now();
        driver.display.lock().unwrap().first_content = None;
        let mut handoff = Box::pin(driver.runtime.runtime().settlement_signal().notified());
        handoff.as_mut().enable();
        driver
            .runtime
            .host()
            .submit_inbound(vec![UserContentBlock::Text(TextBlock {
                text: format!("benchmark turn {turn:04}"),
            })])
            .unwrap();
        let accepted_us = began.elapsed().as_micros();
        if case == "cancel" {
            emulator.await_gate("cancel").await;
            let snapshots = driver
                .runtime
                .runtime()
                .request_history()
                .page(None, 32)
                .unwrap()
                .snapshots;
            assert_eq!(snapshots.len(), 1);
            driver.runtime.host().cancel_current_attempt().unwrap();
            emulator.await_client_disconnect().await;
            emulator.release_gate("cancel").await;
        } else if case == "stream" {
            // Deliberate workload pacing, never synchronization for a race assertion.
            for delta in 0..128 {
                let gate = format!("delta-{delta}");
                emulator.await_gate(&gate).await;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                emulator.release_gate(&gate).await;
            }
        }
        handoff.await;
        // The native handoff denotes attempt settlement; quiescence is shutdown-only.
        if mode == "live-subscriber" {
            driver.observed(turn + 1).await;
        }
        let settled_us = began.elapsed().as_micros();
        let display = driver.display.lock().unwrap();
        if mode == "live-subscriber" {
            let outcome = display.outcomes.last().unwrap();
            assert!(if case == "cancel" {
                matches!(outcome, RuntimeClientOutcome::Cancelled { .. })
            } else {
                matches!(outcome, RuntimeClientOutcome::Completed { .. })
            });
            if case == "tool" {
                assert!(display.tools > 0);
            }
        }
        samples.push(json!({"observer_mode":mode,"turn":turn,"measured":turn>=history,
            "accepted_us":accepted_us,"settled_us":settled_us,
            "first_consumed_publication_us":display.first_content.map(|at|at.duration_since(began).as_micros()),
            "notification_records":display.records,"process":process_sample(),
            "footprint":footprint(driver.root.path())}));
    }
    lifecycle_sample(&mut lifecycle, "after_turns", lifecycle_began);
    let late_snapshot = driver.late_snapshot().await;
    // An evicted initial cursor leaves the original live subscription intact.
    // Close the real observer explicitly before joining it in either replay case.
    driver.attachment.detach();
    if let Some(relay) = driver.relay.take() {
        relay.await.unwrap();
    }
    lifecycle_sample(&mut lifecycle, "after_late_snapshot", lifecycle_began);
    let requests = emulator.requests().await;
    let expected_requests = history + measured + usize::from(case == "tool");
    assert_eq!(requests.len(), expected_requests);
    let history_messages = driver.canonical_history();
    verify_ordered_messages(
        late_snapshot["messages"].as_array().unwrap(),
        &history_messages,
    );
    let terminals = driver
        .store
        .read_presentation_events(&FactQuery {
            scope: FactScope::All,
            kinds: vec![
                "attempt_completed",
                "attempt_cancelled",
                "attempt_failed",
                "attempt_timed_out",
                "attempt_limit_exceeded",
            ],
            before: None,
            after: 0,
            ascending: true,
            through: driver.store.presentation_frontier().unwrap(),
            limit: 128,
        })
        .unwrap();
    assert_eq!(terminals.len(), history + measured);
    let mut attempt_ids = std::collections::BTreeSet::new();
    for terminal in &terminals {
        assert!(attempt_ids.insert(terminal.attempt_id.clone().unwrap()));
        assert!(if case == "cancel" {
            matches!(
                terminal.event,
                rustx::events::types::RuntimeEvent::AttemptCancelled { .. }
            )
        } else {
            matches!(
                terminal.event,
                rustx::events::types::RuntimeEvent::AttemptCompleted { .. }
            )
        });
    }
    if mode == "live-subscriber" {
        let display = driver.display.lock().unwrap();
        assert_eq!(display.outcomes.len(), history + measured);
        if case == "tool" {
            let expected_results: Vec<_> = history_messages
                .iter()
                .filter_map(|message| {
                    if let MessageBlock::Tool(tool) = message {
                        Some(serde_json::to_value(&tool.result).unwrap())
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(
                display.tool_results, expected_results,
                "exact native tool results in order"
            );
            assert_eq!(display.tool_results.len(), 1, "one exact tool settlement");
            assert!(
                serde_json::to_string(&display.tool_results[0])
                    .unwrap()
                    .contains(NOTE_MARKER),
                "real consumer must receive the tool output"
            );
        } else {
            assert!(display.tool_results.is_empty());
        }
        for message in &history_messages {
            if let MessageBlock::Assistant(assistant) = message {
                for (index, block) in assistant.content.iter().enumerate() {
                    if let rustx::message::types::AssistantContentBlock::Text(text) = block {
                        let key = (
                            assistant.id.to_string(),
                            format!(
                                "{:?}",
                                rustx::message::types::ContentBlockIndex::new(
                                    u32::try_from(index).unwrap()
                                )
                            ),
                        );
                        assert_eq!(
                            display.text.get(&key),
                            Some(&text.text),
                            "complete visible assistant text"
                        );
                    }
                }
            }
        }
    }
    let text: String = history_messages
        .iter()
        .filter_map(|m| match m {
            MessageBlock::Assistant(m) => Some(m),
            _ => None,
        })
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            rustx::message::types::AssistantContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    // Keep the frozen BEFORE oracle identical; this allocation is outside turn timing.
    #[allow(clippy::format_collect)]
    let expected = match case.as_str() {
        "large" => "L".repeat(32768),
        "stream" => (0..128).map(|i| format!("stream-delta-{i:04} ")).collect(),
        "tool" => NOTE_MARKER.to_owned(),
        "cancel" => String::new(),
        _ => (0..history + measured)
            .map(|i| format!("reply {i:04}"))
            .collect(),
    };
    assert_eq!(text, expected);
    let verified_requests = verify_requests(driver.store.clone(), &case, &workspace, &requests);
    lifecycle_sample(&mut lifecycle, "after_verification", lifecycle_began);
    let observer_validation = if mode == "live-subscriber" {
        let display = driver.display.lock().unwrap();
        json!({"oracle_version":2,"completed_attempts":display.outcomes.len(),"tool_results":display.tool_results,
            "checkpoint_terminal_sightings":display.checkpoint_terminal_sightings,
            "explicit_settlements":display.explicit_ended.iter().map(|(id,outcome)|json!({"attempt_id":id,"outcome":outcome})).collect::<Vec<_>>(),
            "text":display.text.iter().map(|((id,index),text)|json!({"message_id":id,"block_index":index,"text":text})).collect::<Vec<_>>()})
    } else {
        serde_json::Value::Null
    };
    let database = driver.database.clone();
    let cid = driver.runtime.runtime().conversation_id().clone();
    let began = Instant::now();
    driver.runtime.shutdown().await.unwrap();
    assert!(driver.runtime.runtime().is_quiescent());
    let shutdown_us = began.elapsed().as_micros();
    lifecycle_sample(&mut lifecycle, "after_shutdown", lifecycle_began);
    let root_path = driver.root.path().to_owned();
    driver.root.disable_cleanup(true);
    let before_close = footprint(&root_path);
    let began = Instant::now();
    drop(driver);
    let close_us = began.elapsed().as_micros();
    let closed_footprint = footprint(&root_path);
    lifecycle_sample(&mut lifecycle, "after_driver_drop", lifecycle_began);
    let began = Instant::now();
    // A final read-only owner can leave committed WAL frames after shutdown.
    // Include a normal writable open/close before deleting the allocation.
    drop(SqliteConversationStore::open(cid.clone(), &database).unwrap());
    let final_writer_close_us = began.elapsed().as_micros();
    let checkpoint_footprint = footprint(&root_path);
    let wal = std::path::PathBuf::from(format!("{}-wal", database.display()));
    match std::fs::metadata(wal) {
        Ok(info) => assert_eq!(info.len(), 0, "checkpoint left WAL frames"),
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
    }
    lifecycle_sample(&mut lifecycle, "after_final_writer_close", lifecycle_began);
    let began = Instant::now();
    let reopened = Arc::new(SqliteConversationStore::open_existing(cid, &database).unwrap());
    assert_eq!(reopened.load_canonical().unwrap(), history_messages);
    let reopened_requests = verify_requests(reopened.clone(), &case, &workspace, &requests);
    assert_eq!(
        reopened_requests, verified_requests,
        "exact frozen identities and input survive reopen"
    );
    let reopen_us = began.elapsed().as_micros();
    let verified_live_requests = verified_requests.len();
    let verified_reopened_requests = reopened_requests.len();
    drop(reopened);
    lifecycle_sample(&mut lifecycle, "after_reopen_verification", lifecycle_began);
    let began = Instant::now();
    std::fs::remove_dir_all(&root_path).unwrap();
    let cleanup_us = began.elapsed().as_micros();
    lifecycle_sample(&mut lifecycle, "after_cleanup", lifecycle_began);
    drop(emulator);
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "case":case,"schema":rustx::durable::sqlite::SQLITE_SCHEMA_VERSION,
            "protocol":APP_SERVER_PROTOCOL_VERSION,"history":history,"measured":measured,
            "requests":expected_requests,"open_us":open_us,"samples":samples,
            "initial_process":initial,"lifecycle":lifecycle,"workload_version":3,"model_time":request_oracle::MODEL_TIME,"measurement_version":4,"observer_mode":mode,"observer_surface":"LocalConversationRuntime/Runtime Client","late_snapshot":late_snapshot,"observer_validation":observer_validation,
            "wire_requests":requests,"live_requests":verified_requests,
            "reopened_requests":reopened_requests,"canonical":history_messages,
            "oracle_version":2,"verified_live_requests":verified_live_requests,
            "verified_reopened_requests":verified_reopened_requests,
            "shutdown_us":shutdown_us,"reopen_us":reopen_us,
            "final_writer_close_us":final_writer_close_us,"checkpoint_footprint":checkpoint_footprint,
            "before_close":before_close,"closed_footprint":closed_footprint,"close_us":close_us,"cleanup_us":cleanup_us,
            "oracle":"passed","sql_statements":null,"allocations":null
        }))
        .unwrap(),
    )
    .unwrap();
}
