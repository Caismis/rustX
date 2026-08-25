//! Issue #47: Agent Loop conformance through the real provider boundary.
//!
//! Every test here composes the **real** local runtime — the real model
//! catalog, the resolved model binding, the production provider adapter, the
//! real HTTP client and streaming parser, the Agent Loop, the context
//! engine, the tool runtime, the capability plane, and the Runtime Client
//! projection — and points its catalog at an external scripted provider
//! process.
//!
//! ```text
//! this test driver
//!   |  fixed user/runtime actions
//!   v
//! LocalConversationRuntime  (real composition, no injected adapter)
//!   |
//!   v
//! real provider adapter -> real HTTP + SSE -> fake-provider (Python)
//! ```
//!
//! Nothing in rustX is substituted. The only scripted participant is the
//! external provider, and its scenario asserts the requests rustX produced
//! as strictly as this file asserts the runtime state rustX reached.
//!
//! The string constants below mirror `test-support/fake-provider/src/
//! fake_provider/scenarios/conformance.py`. A drift in either direction
//! fails: the scenario rejects a request it did not expect, and this driver
//! rejects a runtime state it did not expect.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::provider_emulator::ProviderEmulator;
use rustx::durable::ConversationStore;
use rustx::local_runtime::composition::{
    HeadlessConversationRuntime, LocalConversationRuntime, LocalRuntimeDependencies,
    LocalRuntimePaths,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{InboundKind, MessageBlock, UserContentBlock, UserSource};
use rustx::model::catalog::{MapCredentialEnvironment, ModelRef};
use rustx::model::session::{SessionModelConfig, SummaryModelPolicy};
use rustx::runtime::recovery::{AttemptRecoveryClass, ResumeDisposition};
use rustx::runtime_client::attachment::RuntimeAttachment;
use rustx::runtime_client::host::{EventDelivery, EventSubscription, RuntimeClientHost};
use rustx::runtime_client::snapshot::RuntimeClientAttemptPhase;
use rustx::runtime_client::types::RuntimeClientResult;
use rustx::runtime_client::{
    RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientAttemptFailure, RuntimeClientEvent,
    RuntimeClientOutcome,
};

// ---------------------------------------------------------------------------
// The scenario contract, mirrored from the Python scenarios
// ---------------------------------------------------------------------------

const CREDENTIAL_VARIABLE: &str = "RUSTX_CONFORMANCE_KEY";
const CREDENTIAL_VALUE: &str = "conformance-secret";

const CHAT_MODEL: &str = "chat-model";
const RESPONSES_MODEL: &str = "responses-model";
const ANTHROPIC_MODEL: &str = "anthropic-model";
const SUMMARY_MODEL: &str = "summary-model";
const SECOND_MODEL: &str = "second-model";

const TURN_ONE: &str = "conformance: turn one";
const TURN_TWO: &str = "conformance: turn two";
const READ_PROMPT: &str = "conformance: read the note";
const SKILL_PROMPT: &str = "conformance: follow the conformance skill";
const FIRST_ATTEMPT: &str = "conformance: first attempt";
const SECOND_ATTEMPT: &str = "conformance: second attempt";

const NOTE_PATH: &str = "note.txt";
const NOTE_MARKER: &str = "deterministic-note-payload-6d41";
const SKILL_NAME: &str = "conformance-skill";
const SKILL_DESCRIPTION: &str =
    "The deterministic workspace Skill of the issue 47 conformance harness.";
const SKILL_BODY_MARKER: &str = "skill-body-marker-a17c";
const COMPACTION_MARKER: &str = "compaction-filler-marker-93be";
const SUMMARY_TEXT: &str = "conformance summary: the assistant produced one long report.";

const TURN_THREE: &str = "conformance: turn three";
const FILLER_ONE_MARKER: &str = "compaction-filler-one-marker-51f0";
const FILLER_TWO_MARKER: &str = "compaction-filler-two-marker-b27d";
const SUMMARY_ONE_TEXT: &str = "conformance summary one: the assistant produced filler report one.";
const SUMMARY_TWO_TEXT: &str = "conformance summary two: the assistant produced filler report two.";

/// The compaction window and reserve.
///
/// The emulator's scripted turn-one reply is ~200 KB, which the frozen
/// `ceil(bytes / 4)` estimator values at roughly 53k tokens. Two bounds have
/// to hold at once, and both are structural, not statistical:
///
/// ```text
/// trigger:  window - reserve - output   <=  the turn-two request estimate
/// summary:  the selected span estimate  <=  window - summary output
/// ```
///
/// A 56k window with an 8k reserve satisfies both with several thousand
/// tokens of margin on each side: the primary request provably crosses its
/// soft limit, and the complete-message span provably fits the summary
/// model's own request budget. Compaction never splits a message to get
/// there.
const COMPACTION_CONTEXT_WINDOW: u64 = 56_000;
const COMPACTION_RESERVE_TOKENS: u64 = 8_192;

// ---------------------------------------------------------------------------
// The composed driver
// ---------------------------------------------------------------------------

/// One composed runtime plus the attachment and subscription a driver needs.
struct Driver {
    /// The temporary root, kept alive for the fixture lifetime.
    #[allow(dead_code)]
    root: tempfile::TempDir,
    runtime: LocalConversationRuntime,
    /// The one v1 attachment, kept alive for the subscription's lifetime.
    #[allow(dead_code)]
    _attachment: RuntimeAttachment,
    events: EventSubscription,
}

/// How a driver's catalog and session are shaped.
struct Setup {
    /// The model the session selects.
    model: String,
    /// The declared context window of every catalog model.
    context_window: u64,
    /// The session's safety reserve.
    reserve_tokens: u64,
    /// Tokens of recent history kept literal.
    keep_recent_tokens: u64,
    /// The explicit summary-model policy, when the session declares one.
    summary_model: Option<String>,
    /// Explicit Skill resources for this isolated provider fixture.
    skill_paths: Vec<PathBuf>,
    /// Keep the provider fixture independent from the invoking user's roots.
    no_skills: bool,
}

impl Setup {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            context_window: 128_000,
            reserve_tokens: 1_024,
            keep_recent_tokens: 8_192,
            summary_model: None,
            skill_paths: Vec::new(),
            no_skills: true,
        }
    }
}

impl Driver {
    /// Composes the real runtime against a running emulator.
    async fn start(emulator: &ProviderEmulator, setup: &Setup) -> Self {
        Self::start_in(tempfile::tempdir().expect("temp root"), emulator, setup).await
    }

    /// Composes the real runtime over a root a test has already populated
    /// (a Skill package must exist before the capability candidate is
    /// prepared, which happens inside composition).
    async fn start_in(root: tempfile::TempDir, emulator: &ProviderEmulator, setup: &Setup) -> Self {
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(
            root.path().join("models.json"),
            models_json(emulator, setup),
        )
        .expect("models.json");
        std::fs::write(root.path().join("rustx.json"), session_json(setup)).expect("rustx.json");

        let paths = LocalRuntimePaths {
            models: root.path().join("models.json"),
            config: root.path().join("rustx.json"),
            skill_paths: setup.skill_paths.clone(),
            no_skills: setup.no_skills,
            no_builtin_tools: false,
            no_tools: false,
            tools: None,
            exclude_tools: Vec::new(),
            workspace,
            runtime_root: root.path().join("private"),
        };
        let dependencies = LocalRuntimeDependencies {
            credentials: Arc::new(MapCredentialEnvironment::new([(
                CREDENTIAL_VARIABLE.to_owned(),
                CREDENTIAL_VALUE.to_owned(),
            )])),
            ..LocalRuntimeDependencies::default()
        };
        let runtime = LocalConversationRuntime::compose(&paths, &dependencies)
            .await
            .expect("the real runtime composes against the emulator catalog");
        let (attachment, result) = runtime
            .host()
            .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let RuntimeClientResult::Initialized { cursor, .. } = result else {
            panic!("initialize returns the snapshot");
        };
        let (events, _) = runtime
            .host()
            .subscribe_events(attachment.attachment_id(), cursor)
            .expect("subscribe");
        Self {
            root,
            runtime,
            // The attachment is held for the driver's lifetime: dropping it
            // detaches, which releases the subscription mid-test.
            _attachment: attachment,
            events,
        }
    }

    fn host(&self) -> &RuntimeClientHost {
        self.runtime.host()
    }

    /// Submits one fixed user turn.
    fn submit(&self, text: &str) {
        self.host()
            .submit_inbound(vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })])
            .expect("inbound accepted");
    }

    /// Consumes the observation stream up to and including the attempt's one
    /// terminal settlement.
    ///
    /// The settlement uniqueness invariant is asserted here rather than
    /// assumed: a second `AttemptSettled` fails, and the terminal event must
    /// be the last event of the attempt.
    async fn settle(&self) -> (Vec<RuntimeClientEvent>, RuntimeClientOutcome) {
        let mut observed = Vec::new();
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(30), self.events.next()).await
            {
                Ok(EventDelivery::Event(published)) => {
                    let event = published.event;
                    if let RuntimeClientEvent::AttemptSettled { outcome, .. } = &event {
                        let outcome = outcome.clone();
                        observed.push(event);
                        assert!(
                            !matches!(
                                self.events.try_next(),
                                EventDelivery::Event(published)
                                    if matches!(
                                        published.event,
                                        RuntimeClientEvent::AttemptSettled { .. }
                                    )
                            ),
                            "an attempt settles exactly once"
                        );
                        return (observed, outcome);
                    }
                    observed.push(event);
                }
                Ok(other) => panic!("the observation stream ended early: {other:?}"),
                Err(elapsed) => panic!(
                    "the attempt never settled ({elapsed}); observed {} event(s): {observed:?}",
                    observed.len()
                ),
            }
        }
    }

    /// The committed Assistant text of the conversation, concatenated.
    fn committed_assistant_text(&self) -> String {
        let (snapshot, _) = self.host().snapshot().expect("snapshot");
        snapshot
            .messages
            .iter()
            .filter_map(|message| match message {
                MessageBlock::Assistant(assistant) => Some(assistant),
                _ => None,
            })
            .flat_map(|assistant| assistant.content.iter())
            .filter_map(|block| match block {
                rustx::message::types::AssistantContentBlock::Text(text) => {
                    Some(text.text.as_str())
                }
                _ => None,
            })
            .collect()
    }
}

/// The catalog: one OpenAI-family provider and one Anthropic provider, both
/// pointed at the emulator. The credential is resolved by the runtime from
/// the environment exactly as in production.
fn models_json(emulator: &ProviderEmulator, setup: &Setup) -> String {
    let openai = emulator.openai_base_url();
    let anthropic = emulator.base_url();
    let window = setup.context_window;
    serde_json::json!({
        "providers": {
            "emulator": {
                "baseUrl": openai,
                "apiKey": format!("${CREDENTIAL_VARIABLE}"),
                "models": [
                    chat_model(CHAT_MODEL, window),
                    chat_model(SECOND_MODEL, window),
                    chat_model(SUMMARY_MODEL, window),
                    {
                        "id": RESPONSES_MODEL,
                        "protocol": "openai_responses",
                        "contextWindow": window,
                        "maxOutputTokens": 1024,
                        "capabilities": text_capabilities(),
                    },
                ],
            },
            "emulator-anthropic": {
                "baseUrl": anthropic,
                "apiKey": format!("${CREDENTIAL_VARIABLE}"),
                "models": [
                    {
                        "id": ANTHROPIC_MODEL,
                        "protocol": "anthropic_messages",
                        "contextWindow": window,
                        "maxOutputTokens": 1024,
                        "capabilities": text_capabilities(),
                    },
                ],
            },
        },
    })
    .to_string()
}

fn chat_model(id: &str, window: u64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "protocol": "openai_chat_completions",
        "contextWindow": window,
        "maxOutputTokens": 1024,
        "capabilities": text_capabilities(),
        "compat": {"chatReasoningReplay": "omit"},
    })
}

fn text_capabilities() -> serde_json::Value {
    serde_json::json!({
        "inputModalities": ["text"],
        "outputModalities": ["text"],
        "toolCalls": true,
        "reasoning": true,
    })
}

fn session_json(setup: &Setup) -> String {
    let mut model = serde_json::json!({"model": setup.model});
    if let Some(summary) = &setup.summary_model {
        model["summaryModel"] = serde_json::json!({"mode": "explicit", "model": summary});
    }
    serde_json::json!({
        "agentId": "agent-issue47",
        "model": model,
        "context": {
            "reserveTokens": setup.reserve_tokens,
            "keepRecentTokens": setup.keep_recent_tokens,
        },
    })
    .to_string()
}

/// The provider-request bodies the emulator recorded, as raw JSON text.
fn body_text(request: &serde_json::Value) -> String {
    serde_json::to_string(&request["body"]).expect("serialize the recorded body")
}

// ---------------------------------------------------------------------------
// Scenario A — a normal streamed turn, on every supported protocol
// ---------------------------------------------------------------------------

/// One fixed user input produces exactly one provider request, one canonical
/// agent commit, and exactly one successful terminal settlement — over each
/// of the three real provider protocols.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_normal_streamed_turn_settles_once_on_every_protocol() {
    for (scenario, model) in [
        (
            "openai_chat_streamed_turn",
            format!("emulator/{CHAT_MODEL}"),
        ),
        (
            "openai_responses_streamed_turn",
            format!("emulator/{RESPONSES_MODEL}"),
        ),
        (
            "anthropic_streamed_turn",
            format!("emulator-anthropic/{ANTHROPIC_MODEL}"),
        ),
    ] {
        let Some(emulator) = ProviderEmulator::start(scenario).await else {
            return;
        };
        let driver = Driver::start(&emulator, &Setup::new(&model)).await;

        driver.submit(TURN_ONE);
        let (events, outcome) = driver.settle().await;

        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: {outcome:?}"
        );
        assert_eq!(
            driver.committed_assistant_text(),
            "Hello world",
            "{scenario}: the streamed deltas commit as one canonical message"
        );
        // The scripted response leads with the protocol's full normal
        // reasoning lifecycle, so this also proves the real stream parser
        // accepts it rather than only the text subset.
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    RuntimeClientEvent::AssistantReasoningDelta { delta, .. } =>
                        Some(delta.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            "the conformance plan",
            "{scenario}: the reasoning block reached the runtime"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeClientEvent::AttemptSettled { .. }))
                .count(),
            1,
            "{scenario}: exactly one terminal settlement"
        );
        assert!(
            matches!(
                events.last(),
                Some(RuntimeClientEvent::AttemptSettled { .. })
            ),
            "{scenario}: the terminal event is last"
        );

        let requests = emulator.requests().await;
        assert_eq!(
            requests.len(),
            1,
            "{scenario}: exactly one provider request"
        );
        assert_eq!(requests[0]["model"], serde_json::json!(model_id(&model)));
        assert_eq!(
            requests[0]["credentialHeaders"]
                .as_array()
                .expect("credential headers")
                .len(),
            1,
            "{scenario}: the runtime authenticated the request itself"
        );
        assert!(
            !body_text(&requests[0]).contains(CREDENTIAL_VALUE),
            "{scenario}: a credential never reaches the request body"
        );
        assert!(
            body_text(&requests[0]).contains("<system-reminder>"),
            "{scenario}: Agent Status reached the provider as canonical context"
        );
        emulator.finish().await;
    }
}

fn model_id(reference: &str) -> &str {
    reference.split_once('/').expect("provider/model").1
}

// ---------------------------------------------------------------------------
// Scenario B — provider tool call -> real tool -> continuation
// ---------------------------------------------------------------------------

/// The provider requests a tool; rustX executes the **real** native Read
/// through `ConversationToolRuntime`; the continuation request carries the
/// real tool result. The emulator never touches the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_call_runs_the_real_tool_and_continues() {
    // The workspace exists before the emulator starts: the scripted tool
    // call carries the fixture's absolute path so the emulator can substitute
    // the concrete root at scenario build time.
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join(NOTE_PATH), format!("{NOTE_MARKER}\n")).expect("workspace note");
    let Some(emulator) =
        ProviderEmulator::start_with_workspace("tool_call_continuation", Some(&workspace)).await
    else {
        return;
    };
    let driver = Driver::start_in(
        root,
        &emulator,
        &Setup::new(&format!("emulator/{CHAT_MODEL}")),
    )
    .await;

    driver.submit(READ_PROMPT);
    let (events, outcome) = driver.settle().await;

    assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RuntimeClientEvent::ToolExecutionSettled { .. })),
        "the real tool plane executed the call: {events:?}"
    );
    assert!(driver.committed_assistant_text().contains(NOTE_MARKER));

    let requests = emulator.requests().await;
    assert_eq!(
        requests.len(),
        2,
        "one turn plus one tool continuation, and no extra model turn"
    );
    assert!(
        !body_text(&requests[0]).contains(NOTE_MARKER),
        "the first request cannot already carry a tool result"
    );
    assert!(
        body_text(&requests[1]).contains(NOTE_MARKER),
        "the continuation carries the result rustX's own Read produced"
    );
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario C — a real workspace Skill
// ---------------------------------------------------------------------------

/// rustX discovers a real workspace Skill, renders its compact catalog through
/// the normal Effective System Prompt path, and executes the real Read of the
/// Skill's own `SKILL.md`. Python implements no part of that.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_skill_reaches_the_provider_and_is_read_by_the_real_tool() {
    // The Skill must exist before composition: the capability candidate is
    // prepared and committed during `compose`. The workspace also exists
    // before the emulator starts so the capability snapshot can publish the
    // host location the scripted Read call uses.
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    let package = workspace.join(".agents/skills").join(SKILL_NAME);
    std::fs::create_dir_all(&package).expect("skill package");
    std::fs::write(
        package.join("SKILL.md"),
        format!(
            "---\nname: {SKILL_NAME}\ndescription: {SKILL_DESCRIPTION}\n---\n\n{SKILL_BODY_MARKER}\n"
        ),
    )
    .expect("SKILL.md");
    let Some(emulator) =
        ProviderEmulator::start_with_workspace("skill_read_turn", Some(&workspace)).await
    else {
        return;
    };
    let mut setup = Setup::new(&format!("emulator/{CHAT_MODEL}"));
    setup.skill_paths = vec![package];
    let driver = Driver::start_in(root, &emulator, &setup).await;

    let (snapshot, _) = driver.host().snapshot().expect("snapshot");
    assert_eq!(
        snapshot
            .capabilities
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec![SKILL_NAME],
        "rustX discovered the workspace Skill"
    );

    driver.submit(SKILL_PROMPT);
    let (_, outcome) = driver.settle().await;
    assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));

    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 2);
    let first = body_text(&requests[0]);
    assert!(
        first.contains(SKILL_NAME) && first.contains(SKILL_DESCRIPTION),
        "the Skill catalog reached the provider through the unified system-prompt path"
    );
    let published = snapshot.capabilities.skills[0].location.clone();
    assert!(
        published.ends_with(&format!(".agents/skills/{SKILL_NAME}/SKILL.md")),
        "the catalog publishes the package's host SKILL.md path: {published}"
    );
    assert!(
        first.contains(&published),
        "the provider receives that exact host location"
    );
    assert!(
        !first.contains(SKILL_BODY_MARKER),
        "the initial request contains routing metadata, not the full SKILL.md body"
    );
    let wire_messages = requests[0]["body"]["messages"]
        .as_array()
        .expect("Chat Completions messages");
    assert!(
        wire_messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"].to_string().contains("## Skills")
                && message["content"].to_string().contains(SKILL_NAME)
        }),
        "the provider saw Skill guidance in the translated system prompt"
    );
    assert!(
        !wire_messages.iter().any(|message| {
            message["role"] == "user" && message["content"].to_string().contains("## Skills")
        }),
        "the provider received a canonical User Skill catalog message"
    );
    assert!(
        body_text(&requests[1]).contains(SKILL_BODY_MARKER),
        "the continuation carries the SKILL.md the real Read returned"
    );
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario D — a deterministic provider failure
// ---------------------------------------------------------------------------

/// A provider HTTP failure settles the attempt exactly once as a normalized
/// model failure, with exactly one provider request: no accidental retry and
/// no extra model turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_failure_settles_once_without_a_retry() {
    let Some(emulator) = ProviderEmulator::start("provider_http_error").await else {
        return;
    };
    let driver = Driver::start(&emulator, &Setup::new(&format!("emulator/{CHAT_MODEL}"))).await;

    driver.submit(TURN_ONE);
    let (events, outcome) = driver.settle().await;

    let RuntimeClientOutcome::Failed { error } = &outcome else {
        panic!("a provider failure settles as Failed: {outcome:?}");
    };
    let RuntimeClientAttemptFailure::Model { message, .. } = error else {
        panic!("the failure is projected as a model failure: {error:?}");
    };
    assert!(
        !message.contains(CREDENTIAL_VALUE),
        "the projected failure never carries a credential"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeClientEvent::AttemptSettled { .. }))
            .count(),
        1
    );
    assert!(driver.committed_assistant_text().is_empty());
    assert_eq!(
        emulator.requests().await.len(),
        1,
        "a failing provider response is not retried into a second request"
    );
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario E — an interrupted stream, ordered by a gate rather than a sleep
// ---------------------------------------------------------------------------

/// Cancellation lands provably after the first streamed delta and before
/// anything the gate holds back.
///
/// The linearization point is the emulator's `gate_reached` observation, not
/// a timer: the driver waits for it, cancels, and then waits for the
/// provider to observe the client disconnect. Nothing sleeps.
///
/// This asserts one terminal cancellation settlement, no extra provider
/// request, and a physically closed provider connection.
///
/// Issue #12 (M9b) durable-start facts: the emulator gate is only reachable
/// **after** the model-turn start commit (the provider is never invoked
/// before it), so at the gate the request is durably started — exactly one
/// `ModelRequestStarted` and exactly one started Request Snapshot — and the
/// later cancellation settles that started request without ever
/// reclassifying it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_at_a_provider_gate_settles_once_and_closes_the_stream() {
    let Some(emulator) = ProviderEmulator::start("gated_stream_cancellation").await else {
        return;
    };
    let driver = Driver::start(&emulator, &Setup::new(&format!("emulator/{CHAT_MODEL}"))).await;

    driver.submit(TURN_ONE);

    // The provider has flushed "partial" and is suspended. Everything after
    // this point provably happens after that flush.
    emulator.await_gate("before-remaining-text").await;

    // M9b: the provider is only invoked after the durable model-turn start
    // commit, so with the request in flight the start fact already exists:
    // exactly one started Request Snapshot, bound to exactly one
    // `ModelRequestStarted` event.
    let snapshots = driver
        .runtime
        .runtime()
        .request_history()
        .page(None, 32)
        .expect("request snapshot page")
        .snapshots;
    assert_eq!(
        snapshots.len(),
        1,
        "exactly one started Request Snapshot exists before cancellation"
    );
    let store = rustx::durable::SqliteConversationStore::open(
        driver.runtime.runtime().conversation_id().clone(),
        &driver
            .root
            .path()
            .join("private")
            .join("artifacts")
            .join("conversation.sqlite"),
    )
    .expect("open the durable store for event assertions");
    let started_facts = store
        .read_events(None, 128)
        .expect("event journal")
        .events
        .iter()
        .filter(|envelope| {
            matches!(
                envelope.event,
                rustx::events::types::RuntimeEvent::ModelRequestStarted { .. }
            )
        })
        .count();
    assert_eq!(
        started_facts, 1,
        "exactly one request-start fact exists before cancellation"
    );

    let accepted = driver
        .host()
        .cancel_current_attempt()
        .expect("the in-flight attempt is cancellable");
    assert!(matches!(
        accepted,
        RuntimeClientResult::AttemptCancellationAccepted { .. }
    ));

    // The runtime really dropped the provider connection; this is observed,
    // not assumed.
    emulator.await_client_disconnect().await;
    emulator.release_gate("before-remaining-text").await;

    let (events, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Cancelled { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeClientEvent::AttemptSettled { .. }))
            .count(),
        1
    );
    assert!(
        !driver
            .committed_assistant_text()
            .contains("remainder that cancellation must prevent"),
        "content the gate held back can never be committed"
    );
    assert_eq!(
        emulator.requests().await.len(),
        1,
        "cancellation never starts another model turn"
    );
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario F — compaction through the real provider boundary
// ---------------------------------------------------------------------------

/// Compaction invokes the real provider and the rewritten Conversation
/// Surface reaches the next primary request, on both summary-model policies.
///
/// This is the composed, external proof of the M7.5 semantics:
///
/// ```text
/// real summary provider invocation
///   → canonical User(Runtime / CompactionSummary) Message Ledger commit
///   → Conversation Surface rewrite
///   → the next real provider request carries the summary
///   → the retired original filler is absent from the active provider context
///     while remaining a committed ledger fact
/// ```
///
/// The trigger is structural, not statistical: the catalog window, the
/// reserve, the output budget, and the emulator's scripted reply size fix
/// the threshold crossing exactly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
// The composed compaction proof is one scenario observed end to end; keeping
// it together preserves the request ordering that is the whole point.
#[allow(clippy::too_many_lines)]
async fn compaction_invokes_the_real_provider_on_both_summary_policies() {
    for (scenario, summary_model, expected_summary_model) in [
        ("compaction_session_summary", None, CHAT_MODEL),
        (
            "compaction_explicit_summary",
            Some(format!("emulator/{SUMMARY_MODEL}")),
            SUMMARY_MODEL,
        ),
    ] {
        let Some(emulator) = ProviderEmulator::start(scenario).await else {
            return;
        };
        let setup = Setup {
            context_window: COMPACTION_CONTEXT_WINDOW,
            reserve_tokens: COMPACTION_RESERVE_TOKENS,
            keep_recent_tokens: 256,
            summary_model,
            ..Setup::new(&format!("emulator/{CHAT_MODEL}"))
        };
        let driver = Driver::start(&emulator, &setup).await;

        driver.submit(TURN_ONE);
        let (_, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: the filling turn completes: {outcome:?}"
        );

        driver.submit(TURN_TWO);
        let (events, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: {outcome:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, RuntimeClientEvent::ContextCompacted { .. })),
            "{scenario}: the runtime committed the canonical compaction: {events:?}"
        );

        let (snapshot, _) = driver.host().snapshot().expect("snapshot");
        assert_eq!(
            snapshot.context.compaction_count, 1,
            "{scenario}: exactly one compaction"
        );
        let latest = snapshot
            .context
            .latest_compaction
            .as_ref()
            .expect("the committed compaction metadata");
        assert_eq!(
            latest.generation, 1,
            "{scenario}: one compaction generation"
        );

        // The runtime compaction summary is a canonical Message Ledger fact,
        // externally visible like any other committed message — not a
        // private context value.
        let summary_message = snapshot
            .messages
            .iter()
            .find_map(|message| match message {
                MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary => {
                    Some(user)
                }
                _ => None,
            })
            .expect("the canonical compaction summary is committed to the ledger");
        assert_eq!(
            summary_message.id, latest.summary_message_id,
            "{scenario}: the compaction metadata names the committed summary"
        );
        assert_eq!(
            summary_message.source,
            UserSource::Runtime,
            "{scenario}: the summary carries runtime provenance"
        );
        // Compaction never rewrote the ledger: the retired original is still
        // there, byte for byte.
        assert!(
            snapshot.messages.iter().any(|message| matches!(
                message,
                MessageBlock::Assistant(assistant)
                    if serde_json::to_string(assistant)
                        .expect("serialize the retired Assistant message")
                        .contains(COMPACTION_MARKER)
            )),
            "{scenario}: the retired original stays an immutable ledger fact"
        );

        let requests = emulator.requests().await;
        assert_eq!(
            requests.len(),
            3,
            "{scenario}: turn one, the summary invocation, and the compacted turn two"
        );
        assert_eq!(
            requests[1]["model"],
            serde_json::json!(expected_summary_model),
            "{scenario}: the summary used the configured summary model"
        );
        assert!(
            body_text(&requests[1]).contains(COMPACTION_MARKER),
            "{scenario}: the summary request carried the retired history"
        );
        let third = body_text(&requests[2]);
        assert!(
            third.contains(SUMMARY_TEXT) && !third.contains(COMPACTION_MARKER),
            "{scenario}: the rewritten surface replaced the retired history"
        );
        emulator.finish().await;
    }
}

// ---------------------------------------------------------------------------
// Scenario F2 — repeated compaction through the real provider boundary
// ---------------------------------------------------------------------------

/// Two committed compactions through the real provider boundary, on both
/// summary-model policies (Issue #27).
///
/// Where Scenario F proves one compaction, this scenario proves the
/// composition: the second compaction operates on the already-compacted
/// surface, its summary input retires the still-active first summary, and
/// retired bytes never reach the provider again — asserted externally on the
/// recorded wire bodies and internally on the canonical ledger, the frozen
/// request snapshots, and their historical reconstructions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
// The composed compaction proof is one scenario observed end to end; keeping
// it together preserves the request ordering that is the whole point.
#[allow(clippy::too_many_lines)]
async fn repeated_compaction_invokes_the_real_provider_on_both_summary_policies() {
    for (scenario, summary_model, expected_summary_model) in [
        ("compaction_twice_session_summary", None, CHAT_MODEL),
        (
            "compaction_twice_explicit_summary",
            Some(format!("emulator/{SUMMARY_MODEL}")),
            SUMMARY_MODEL,
        ),
    ] {
        let Some(emulator) = ProviderEmulator::start(scenario).await else {
            return;
        };
        let setup = Setup {
            context_window: COMPACTION_CONTEXT_WINDOW,
            reserve_tokens: COMPACTION_RESERVE_TOKENS,
            keep_recent_tokens: 256,
            summary_model,
            ..Setup::new(&format!("emulator/{CHAT_MODEL}"))
        };
        let driver = Driver::start(&emulator, &setup).await;

        driver.submit(TURN_ONE);
        let (_, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: the filling turn completes: {outcome:?}"
        );

        driver.submit(TURN_TWO);
        let (events_two, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: {outcome:?}"
        );
        assert!(
            events_two
                .iter()
                .any(|event| matches!(event, RuntimeClientEvent::ContextCompacted { .. })),
            "{scenario}: compaction #1 committed: {events_two:?}"
        );

        driver.submit(TURN_THREE);
        let (events_three, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: {outcome:?}"
        );

        // Exactly two committed compactions, in generation order.
        let generations: Vec<u64> = events_two
            .iter()
            .chain(&events_three)
            .filter_map(|event| match event {
                RuntimeClientEvent::ContextCompacted { context, .. } => context
                    .latest_compaction
                    .as_ref()
                    .map(|view| view.generation),
                _ => None,
            })
            .collect();
        assert_eq!(
            generations,
            [1, 2],
            "{scenario}: two compaction generations in order"
        );

        let (snapshot, _) = driver.host().snapshot().expect("snapshot");
        assert_eq!(
            snapshot.context.compaction_count, 2,
            "{scenario}: exactly two compactions"
        );
        assert_eq!(
            snapshot
                .context
                .latest_compaction
                .as_ref()
                .expect("the committed compaction metadata")
                .generation,
            2
        );

        // Both summaries are canonical ledger facts; both retired fillers
        // remain committed ledger bytes.
        let summaries: Vec<_> = snapshot
            .messages
            .iter()
            .filter(|message| {
                matches!(
                    message,
                    MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
                )
            })
            .collect();
        assert_eq!(
            summaries.len(),
            2,
            "{scenario}: two canonical compaction summaries"
        );
        let ledger = serde_json::to_string(&snapshot.messages).expect("serialize the ledger");
        assert!(
            ledger.contains(FILLER_ONE_MARKER) && ledger.contains(FILLER_TWO_MARKER),
            "{scenario}: the ledger retains both retired fillers verbatim"
        );
        assert!(ledger.contains(SUMMARY_ONE_TEXT) && ledger.contains(SUMMARY_TWO_TEXT));

        // The wire: five requests, each exactly what the scenario asserts.
        let requests = emulator.requests().await;
        assert_eq!(
            requests.len(),
            5,
            "{scenario}: three turns plus two summaries"
        );
        assert_eq!(
            requests[1]["model"],
            serde_json::json!(expected_summary_model),
            "{scenario}: summary #1 used the configured summary model"
        );
        assert_eq!(
            requests[3]["model"],
            serde_json::json!(expected_summary_model),
            "{scenario}: summary #2 used the configured summary model"
        );
        let bodies: Vec<String> = requests.iter().map(body_text).collect();
        assert!(bodies[0].contains(TURN_ONE));
        assert!(bodies[1].contains(FILLER_ONE_MARKER) && !bodies[1].contains(FILLER_TWO_MARKER));
        assert!(
            bodies[2].contains(SUMMARY_ONE_TEXT)
                && bodies[2].contains(TURN_TWO)
                && !bodies[2].contains(FILLER_ONE_MARKER),
            "{scenario}: the first rewritten surface replaced the retired history"
        );
        assert!(
            bodies[3].contains(SUMMARY_ONE_TEXT)
                && bodies[3].contains(FILLER_TWO_MARKER)
                && !bodies[3].contains(FILLER_ONE_MARKER),
            "{scenario}: the second compaction's span is the already-compacted surface"
        );
        assert!(
            bodies[4].contains(SUMMARY_TWO_TEXT)
                && bodies[4].contains(TURN_THREE)
                && !bodies[4].contains(FILLER_ONE_MARKER)
                && !bodies[4].contains(FILLER_TWO_MARKER)
                && !bodies[4].contains(SUMMARY_ONE_TEXT),
            "{scenario}: the second rewritten surface carries exactly the second summary"
        );

        // The frozen request snapshots reconstruct the three primary
        // requests through the real provider path, after both compactions.
        await_history_len(&driver, 3).await;
        let history = driver.host().request_history();
        let snapshots = common::request_snapshots(&history);
        assert_eq!(
            snapshots.len(),
            3,
            "{scenario}: three frozen primary requests"
        );
        let reconstructed: Vec<String> = snapshots
            .iter()
            .map(|snapshot| {
                let request = driver
                    .host()
                    .reconstruct_request(&snapshot.identity)
                    .expect("settled historical request reconstructs");
                serde_json::to_string(&request.messages).expect("serialize reconstructed messages")
            })
            .collect();
        assert!(
            reconstructed[0].contains(TURN_ONE) && !reconstructed[0].contains(FILLER_ONE_MARKER),
            "{scenario}: R1 reconstructs the pre-filler surface (the filler is its response)"
        );
        assert!(
            reconstructed[1].contains(SUMMARY_ONE_TEXT)
                && reconstructed[1].contains(TURN_TWO)
                && !reconstructed[1].contains(FILLER_ONE_MARKER),
            "{scenario}: R2 reconstructs the first rewritten surface"
        );
        assert!(
            reconstructed[2].contains(SUMMARY_TWO_TEXT)
                && reconstructed[2].contains(TURN_THREE)
                && !reconstructed[2].contains(FILLER_ONE_MARKER)
                && !reconstructed[2].contains(SUMMARY_ONE_TEXT),
            "{scenario}: R3 reconstructs the second rewritten surface"
        );

        emulator.finish().await;
    }
}

/// Waits for the post-settlement transfer of frozen request facts to the
/// runtime-owned append-only history.
async fn await_history_len(driver: &Driver, expected: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if common::request_snapshots(&driver.host().request_history()).len() == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request history transfer must settle");
}

// ---------------------------------------------------------------------------
// Scenario G — the immutable attempt model, observed from outside rustX
// ---------------------------------------------------------------------------

/// A session-model update that linearizes after admission never changes the
/// running attempt's model, and the next attempt uses the new one.
///
/// The proof is external: the emulator asserts the `model` field of both
/// requests, and the gate — not a sleep — establishes that the update
/// happened while the first attempt was mid-stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_attempt_keeps_its_frozen_model_while_the_session_moves_on() {
    let Some(emulator) = ProviderEmulator::start("frozen_attempt_model").await else {
        return;
    };
    let driver = Driver::start(&emulator, &Setup::new(&format!("emulator/{CHAT_MODEL}"))).await;

    driver.submit(FIRST_ATTEMPT);
    emulator.await_gate("session-model-updated").await;

    // The first attempt is provably in flight and has already been billed
    // its provider request.
    driver
        .host()
        .model_set(SessionModelConfig {
            summary_model: SummaryModelPolicy::Session,
            ..SessionModelConfig::of(
                ModelRef::parse(&format!("emulator/{SECOND_MODEL}"))
                    .expect("the second model reference parses"),
            )
        })
        .expect("the session model updates while an attempt runs");

    emulator.release_gate("session-model-updated").await;
    let (_, outcome) = driver.settle().await;
    assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));

    let (snapshot, _) = driver.host().snapshot().expect("snapshot");
    let attempt = snapshot.attempt.as_ref().expect("the settled attempt");
    assert!(matches!(
        attempt.phase,
        RuntimeClientAttemptPhase::Settled { .. }
    ));
    assert_eq!(
        attempt.model.primary.model.to_string(),
        format!("emulator/{CHAT_MODEL}"),
        "the settled attempt reports the model it froze"
    );
    assert_eq!(
        snapshot.model.configured.model.to_string(),
        format!("emulator/{SECOND_MODEL}"),
        "the session moved on"
    );

    driver.submit(SECOND_ATTEMPT);
    let (_, outcome) = driver.settle().await;
    assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));

    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["model"], serde_json::json!(CHAT_MODEL));
    assert_eq!(requests[1]["model"], serde_json::json!(SECOND_MODEL));
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario H — durable startup recovery against the real provider (Issue #12)
// ---------------------------------------------------------------------------

/// A crash after the request-start commit resends nothing, proven at the real
/// provider boundary.
///
/// The first runtime is composed inside its **own** Tokio runtime on a
/// dedicated thread. Dropping that Tokio runtime drops every runtime-owned
/// task at its current await point and closes the provider connection, which
/// is the closest honest model of process death available without spawning a
/// second binary — and, unlike a `kill`, it is exact: the driver knows the
/// provider had already received request #1 because it waited on the
/// provider-side gate, and it knows the client really vanished because it
/// waits on the provider's disconnect observation. Nothing sleeps.
///
/// ```text
/// runtime #1 -> request-start durable commit -> provider observes request #1
///            -> provider suspends at the gate
///            -> the Tokio runtime hosting runtime #1 is dropped   (crash)
/// same SQLite conversation reopened
///            -> runtime #2 recovers
/// ```
///
/// The scenario declares exactly one step, so a resent request would be an
/// unexpected provider request and would fail the run rather than pass
/// quietly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One crash, one recovery, one composed proof.
async fn a_crash_after_the_request_start_commit_never_resends_the_request() {
    let Some(emulator) = ProviderEmulator::start("restart_after_request_start").await else {
        return;
    };
    // The root outlives both runtimes: instance #2 reopens the exact same
    // durable conversation instance #1 died in.
    let root = tempfile::tempdir().expect("temp root");
    let setup = Setup::new(&format!("emulator/{CHAT_MODEL}"));
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(
        root.path().join("models.json"),
        models_json(&emulator, &setup),
    )
    .expect("models.json");
    std::fs::write(root.path().join("rustx.json"), session_json(&setup)).expect("rustx.json");
    let paths = LocalRuntimePaths {
        models: root.path().join("models.json"),
        config: root.path().join("rustx.json"),
        skill_paths: setup.skill_paths.clone(),
        no_skills: setup.no_skills,
        no_builtin_tools: false,
        no_tools: false,
        tools: None,
        exclude_tools: Vec::new(),
        workspace,
        runtime_root: root.path().join("private"),
    };

    // ---- runtime instance #1, in its own execution runtime ----
    let (crash_tx, crash_rx) = tokio::sync::oneshot::channel::<()>();
    let first_paths = paths.clone();
    let first = std::thread::spawn(move || {
        let execution = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("the first runtime's execution runtime");
        execution.block_on(async move {
            let runtime = HeadlessConversationRuntime::compose(&first_paths, &dependencies())
                .await
                .expect("the first runtime composes");
            runtime
                .runtime()
                .submit_inbound(vec![UserContentBlock::Text(TextBlock {
                    text: TURN_ONE.to_owned(),
                })])
                .expect("inbound accepted");
            // Park until the driver has observed the provider gate. The
            // attempt is in flight, parked on the provider stream.
            let _ = crash_rx.await;
        });
        // Dropping the execution runtime drops every runtime-owned task —
        // the attempt, the admission worker, the HTTP stream — at its await
        // point, and releases the durable store handle.
        drop(execution);
    });

    // The provider has received request #1 and flushed its first delta.
    // Everything after this point provably happens after that.
    emulator.await_gate("before-remaining-text").await;
    let requests_before_crash = emulator.requests().await;
    assert_eq!(requests_before_crash.len(), 1, "exactly one request so far");

    // Crash.
    crash_tx
        .send(())
        .expect("the first runtime is still parked");
    first.join().expect("the first runtime's thread exits");
    // The connection really went away; this is observed, not assumed.
    emulator.await_client_disconnect().await;
    emulator.release_gate("before-remaining-text").await;

    // ---- runtime instance #2 over the same durable conversation ----
    let recovered = HeadlessConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("the second runtime recovers the durable conversation");
    let report = recovered.runtime().recovery();

    // The crash boundary is the drop of the first execution runtime. The
    // durable journal decides the honest classification, and both landings
    // preserve the invariant under test: recovery resends nothing.
    //
    //   Landing A (the intended crash): the drop aborted the attempt while
    //   it was parked on the gated stream, so the journal holds
    //   `ModelRequestStarted` with no outcome and no terminal — the attempt
    //   classifies as an indeterminate external outcome, the started
    //   request is named explicitly, and continuation is blocked.
    //
    //   Landing B (platform-dependent drop semantics, observed on macOS):
    //   the runtime drop lets the woken attempt settle its terminal as the
    //   connection closes, so the journal honestly records the terminal —
    //   the classification is the absorbing `AlreadyTerminal`. The exact
    //   indeterminate classification is covered deterministically by the
    //   store-prefix test (`started_request_with_unknown_outcome_is_indeterminate_and_never_resent`)
    //   and the runtime test (`class_c_restart_issues_no_provider_request_of_its_own`);
    //   what this scenario adds is the real-provider no-resend proof, which
    //   must hold under both landings.
    match report.attempt_class() {
        AttemptRecoveryClass::IndeterminateExternalOutcome {
            attempt_id,
            model_request,
            ..
        } => {
            let request_id = model_request
                .clone()
                .expect("the started request is named explicitly");
            assert_eq!(
                report.resume(),
                ResumeDisposition::BlockedIndeterminate,
                "an unknown provider outcome never authorizes a continuation"
            );
            assert_eq!(
                report.reconciliation().attempt_terminal.as_ref(),
                Some(attempt_id),
                "the interrupted attempt is settled exactly once by recovery"
            );

            // The historical provider-neutral request reconstructs exactly,
            // from frozen durable facts alone — and reconstructability is
            // still not permission to replay. The identity comes from the
            // retained snapshot itself, never from a guessed turn ordinal.
            let history = recovered.runtime().request_history();
            let page = history.page(None, 8).expect("the retained snapshot page");
            let snapshot = page
                .snapshots
                .iter()
                .find(|snapshot| snapshot.request_id == request_id)
                .expect("the started request retained its immutable snapshot");
            assert_eq!(
                snapshot.identity.attempt_id, *attempt_id,
                "the snapshot belongs to the interrupted attempt"
            );
            let historical = history
                .reconstruct(&snapshot.identity)
                .expect("the historical request reconstructs");
            assert_eq!(
                historical.invocation.model, CHAT_MODEL,
                "the historical request keeps its frozen model, not the current one"
            );
            assert!(
                historical
                    .messages
                    .iter()
                    .any(|block| matches!(block, MessageBlock::User(user)
                    if user.content.iter().any(|content| matches!(
                        content,
                        UserContentBlock::Text(text) if text.text == TURN_ONE
                    )))),
                "the reconstruction is the exact historical request"
            );
        }
        AttemptRecoveryClass::AlreadyTerminal => {
            // The dropped runtime settled the attempt before the process
            // died; the durable journal is honest and absorbing.
            assert_eq!(
                report.resume(),
                ResumeDisposition::PendingInboundOnly,
                "an already-terminal attempt authorizes no continuation"
            );
            assert!(
                report.reconciliation().is_empty(),
                "a terminal attempt needs no recovery fact"
            );
            let history = recovered.runtime().request_history();
            let page = history.page(None, 8).expect("the retained snapshot page");
            assert_eq!(
                page.snapshots.len(),
                1,
                "the started request retained its immutable snapshot"
            );
        }
        other => {
            panic!(
                "a committed request start must classify as indeterminate (or, when the \
                 dropped runtime settled it first, as already terminal): {other:?}"
            );
        }
    }

    // The whole point: the provider saw the request exactly once, under
    // either crash landing. The scenario declares exactly one step, so any
    // resend would have been an unexpected provider request and would have
    // failed the run rather than pass quietly.
    assert_eq!(
        emulator.requests().await.len(),
        1,
        "recovery performed zero automatic provider resend"
    );
    emulator.finish().await;
}

/// The composition dependencies shared by both runtime instances above.
fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Arc::new(MapCredentialEnvironment::new([(
            CREDENTIAL_VARIABLE.to_owned(),
            CREDENTIAL_VALUE.to_owned(),
        )])),
        ..LocalRuntimeDependencies::default()
    }
}
