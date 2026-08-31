//! Issue #140: Pi-style structured compaction summaries with cumulative
//! file-operation metadata.
//!
//! These regressions pin the Issue #140 contract deterministically:
//!
//! - the retired canonical span splits into semantic content (the scripted
//!   summarizer's structured Markdown body) and canonical tool facts (the
//!   deterministic extractor's typed [`CompactionSummaryMetadata`]);
//! - extraction reads native Read/Edit/Write tool calls only, deduplicates,
//!   orders deterministically, and lets modification win over read;
//! - cumulative metadata follows the selected compaction lineage, never a
//!   conversation-global accumulator: a previous summary contributes exactly
//!   when it is inside the selected retired span;
//! - generated summary prose is never parsed into metadata;
//! - the committed summary text appends the deterministic metadata-derived
//!   `<read-files>`/`<modified-files>` sections;
//! - the typed metadata round-trips the durable store exactly, and every
//!   failure mode — cancellation, summarizer failure, post-summary
//!   cannot-fit, durable commit failure — installs no partial transition.
//!
//! The engine-level tests drive the real `ContextEngine` planning and
//! preparation; the pipeline tests drive the shared `execute_compaction`
//! implementation that automatic and manual compaction both use, against a
//! real `SqliteConversationStore`. Every synchronization is an exact watch
//! channel or join, never a delay.

use std::sync::Arc;

use rustx::context::{
    AgentStatusConfig, AgentStatusEngine, CompactionBudgets, CompactionConstraints, CompactionPlan,
    ContextConfig, ContextEngine, ContextRuntime,
};
use rustx::conversation::{ConversationState, SurfaceSpan};
use rustx::durable::{ConversationStore, SqliteConversationStore};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, CompactionSummaryMetadata, InboundKind,
    MessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
use rustx::runtime::types::{TokenMeasurement, TokenMeasurementSource};
use rustx::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
use support::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};

use super::super::support;

/// The engine used by these regressions: a large window so planning admits
/// the full active prefix unless a test sizes it down deliberately.
fn engine(window: u64) -> ContextEngine {
    ContextEngine::new(
        ContextConfig {
            context_window_tokens: window,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(10, 10, 0)),
    )
    .expect("valid context configuration")
}

fn user(id: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!("content {id}"),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn file_call(id: &str, tool_id: &str, name: &str, path: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new(tool_id),
        name: name.to_owned(),
        arguments: serde_json::json!({ "path": path }),
    }
}

fn read_call(id: &str, path: &str) -> ToolCall {
    file_call(id, "tool-read", "read", path)
}

fn edit_call(id: &str, path: &str) -> ToolCall {
    file_call(id, "tool-edit", "edit", path)
}

fn write_call(id: &str, path: &str) -> ToolCall {
    file_call(id, "tool-write", "write", path)
}

fn assistant(id: &str, calls: Vec<ToolCall>) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: calls
            .into_iter()
            .map(AssistantContentBlock::ToolCall)
            .collect(),
    })
}

fn tool_result(id: &str, call: &ToolCall) -> MessageBlock {
    MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new(id),
        tool_call_id: call.id.clone(),
        tool_id: call.tool_id.clone(),
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 1,
            exit_code: Some(0),
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        },
    })
}

fn state(messages: Vec<MessageBlock>) -> ConversationState {
    ConversationState::from_messages(messages).expect("valid canonical history")
}

fn conversation() -> ConversationId {
    ConversationId::new("conv-issue140")
}

/// Plans and prepares one compaction of the whole active prefix, returning
/// the validated commit.
fn prepare_full_compaction(
    engine: &ContextEngine,
    state: &ConversationState,
    body: &str,
) -> rustx::conversation::PreparedCompactionCommit {
    let projection = engine
        .build_projection(state, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            state,
            &projection,
            &[],
            CompactionBudgets::new(1, 1, 1_000_000),
            &CompactionConstraints::default(),
        )
        .expect("plan");
    let (commit, _) = engine
        .prepare_compaction(state, &conversation(), &plan, body, &[])
        .expect("prepare");
    commit
}

/// The typed metadata of a prepared or committed compaction summary.
fn metadata_of(summary: &UserMessageBlock) -> &CompactionSummaryMetadata {
    match &summary.kind {
        InboundKind::CompactionSummary(metadata) => metadata,
        other => panic!("expected a compaction summary, found {other:?}"),
    }
}

/// The summary text of a prepared or committed compaction summary.
fn text_of(summary: &UserMessageBlock) -> &str {
    match &summary.content[0] {
        UserContentBlock::Text(text) => &text.text,
        other => panic!("a compaction summary is text: {other:?}"),
    }
}

/// The canonical first-compaction history of the issue contract:
/// Read A, Read B, Edit B, Write C.
fn file_operation_history() -> Vec<MessageBlock> {
    let reads = assistant(
        "a1",
        vec![read_call("c1", "/src/a.rs"), read_call("c2", "/src/b.rs")],
    );
    let writes = assistant(
        "a2",
        vec![edit_call("c3", "/src/b.rs"), write_call("c4", "/src/c.rs")],
    );
    let mut messages = vec![user("u1"), reads.clone()];
    if let MessageBlock::Assistant(assistant) = &reads {
        for block in &assistant.content {
            let AssistantContentBlock::ToolCall(call) = block else {
                unreachable!();
            };
            messages.push(tool_result(&format!("t-{}", call.id.as_str()), call));
        }
    }
    messages.push(writes.clone());
    if let MessageBlock::Assistant(assistant) = &writes {
        for block in &assistant.content {
            let AssistantContentBlock::ToolCall(call) = block else {
                unreachable!();
            };
            messages.push(tool_result(&format!("t-{}", call.id.as_str()), call));
        }
    }
    messages.push(user("u2"));
    messages
}

/// The first compaction classifies canonical Read/Edit/Write calls,
/// deduplicates, orders deterministically, and lets modification win over
/// read. The committed text appends the metadata-derived XML sections after
/// the model-produced body.
#[test]
fn first_compaction_extracts_typed_file_metadata() {
    let engine = engine(10_000);
    let history = state(file_operation_history());
    let commit = prepare_full_compaction(&engine, &history, "## Goal\n\nShip the feature.");
    let summary = commit.summary();
    let metadata = metadata_of(summary);
    assert_eq!(metadata.read_files(), &["/src/a.rs".to_owned()]);
    assert_eq!(
        metadata.modified_files(),
        &["/src/b.rs".to_owned(), "/src/c.rs".to_owned()]
    );
    assert_eq!(
        text_of(summary),
        "## Goal\n\nShip the feature.\n\n<read-files>\n/src/a.rs\n</read-files>\n\n<modified-files>\n/src/b.rs\n/src/c.rs\n</modified-files>",
        "the committed text is the body plus the deterministic derived sections"
    );
}

/// The second compaction merges the previous summary's typed metadata with
/// the new operations over the selected lineage: a file the first summary
/// listed as read moves to modified when the new history edits it.
#[test]
fn second_compaction_merges_lineage_metadata() {
    let engine = engine(10_000);
    let mut history = state(file_operation_history());
    let first = prepare_full_compaction(&engine, &history, "first summary");
    history.commit_compaction(first).expect("commit first");

    let reads = assistant(
        "a3",
        vec![read_call("c5", "/src/d.rs"), edit_call("c6", "/src/a.rs")],
    );
    // Canonical order: the calls commit before their results.
    history.commit(reads.clone()).expect("commit a3");
    let MessageBlock::Assistant(calls) = &reads else {
        unreachable!();
    };
    for block in &calls.content {
        let AssistantContentBlock::ToolCall(call) = block else {
            unreachable!();
        };
        history
            .commit(tool_result(&format!("t-{}", call.id.as_str()), call))
            .expect("commit result");
    }
    history.commit(user("u3")).expect("commit u3");

    let second = prepare_full_compaction(&engine, &history, "second summary");
    let metadata = metadata_of(second.summary());
    assert_eq!(metadata.read_files(), &["/src/d.rs".to_owned()]);
    assert_eq!(
        metadata.modified_files(),
        &[
            "/src/a.rs".to_owned(),
            "/src/b.rs".to_owned(),
            "/src/c.rs".to_owned()
        ]
    );
}

/// A previous compaction summary that is active but outside the selected
/// retired span contributes nothing: metadata follows the selected lineage,
/// never conversation-global state.
#[test]
fn a_summary_outside_the_selected_span_contributes_nothing() {
    let engine = engine(10_000);
    let mut history = state(file_operation_history());
    let first = prepare_full_compaction(&engine, &history, "first summary");
    let first_summary_id = first.summary().id.clone();
    history.commit_compaction(first).expect("commit first");
    history.commit(user("u3")).expect("commit u3");
    history.commit(user("u4")).expect("commit u4");

    // A valid selected span that excludes the active first summary.
    let plan = CompactionPlan {
        surface_revision: history.revision(),
        span: SurfaceSpan::new(MessageId::new("u3"), MessageId::new("u4")),
        retired: vec![user("u3"), user("u4")],
        estimated_before: TokenMeasurement {
            input_tokens: 0,
            source: TokenMeasurementSource::Estimated,
        },
        estimated_before_tokens: 1_000_000,
        planned_estimate_after: 0,
        summary_reservation: 0,
        summary_input_tokens: 0,
        effective_system_prompt: String::new(),
    };
    let (commit, _) = engine
        .prepare_compaction(&history, &conversation(), &plan, "second summary", &[])
        .expect("prepare");
    assert_eq!(
        commit.summary().kind.compaction_summary_metadata(),
        Some(&CompactionSummaryMetadata::empty()),
        "the retained summary {first_summary_id} is outside the span and must not contribute"
    );
}

/// Generated summary prose is never metadata authority: a scripted summary
/// that invents a file path keeps its prose but contributes no path to the
/// typed metadata, and no XML section appears for empty metadata.
#[test]
fn model_prose_never_enters_typed_metadata() {
    let engine = engine(10_000);
    let history = state(vec![user("u1"), user("u2")]);
    let commit = prepare_full_compaction(
        &engine,
        &history,
        "## Goal\n\nWe edited /fake/invented.rs together.",
    );
    assert_eq!(
        metadata_of(commit.summary()),
        &CompactionSummaryMetadata::empty(),
        "prose paths never become metadata"
    );
    assert_eq!(
        text_of(commit.summary()),
        "## Goal\n\nWe edited /fake/invented.rs together.",
        "empty metadata renders no XML sections, and prose is preserved untouched"
    );
}

// ---------------------------------------------------------------------------
// The shared compaction pipeline, against a real durable store
// ---------------------------------------------------------------------------

/// A fixed deterministic status clock.
struct FixedClock;

impl rustx::context::AgentStatusClock for FixedClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_754_000_000, 0).expect("fixed clock")
    }
}

fn runtime(window: u64, summarizer: Arc<FakeContextSummarizer>) -> ContextRuntime {
    ContextRuntime::with_scripted_summarizer(
        engine(window),
        summarizer,
        AgentStatusEngine::new(AgentStatusConfig::default(), Arc::new(FixedClock)),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

/// One pipeline execution of the shared compaction transition.
async fn run_pipeline(
    history: &[MessageBlock],
    store: &dyn ConversationStore,
    context: &ContextRuntime,
    cancellation: &CancellationSignal,
) -> Result<
    crate::context::compaction::ExecutedCompaction,
    crate::context::compaction::CompactionExecutionError,
> {
    store.initialize(history).expect("initialize store");
    let head = store.load_head().expect("durable head");
    let mut state = ConversationState::from_durable_head(
        history.to_vec(),
        head.active_message_ids.clone(),
        head.revision,
        head.compaction_generation,
    )
    .expect("hot state from the durable head");
    let result = crate::context::compaction::execute_compaction(
        &mut state,
        context,
        &conversation(),
        store,
        &[],
        None,
        "",
        &CompactionConstraints::default(),
        cancellation,
        crate::context::compaction::CompactionAttribution::default(),
    )
    .await;
    if result.is_ok() {
        // The hot state installed the committed summary exactly once.
        let summaries: Vec<_> = state
            .active_messages()
            .expect("active")
            .into_iter()
            .filter(|message| {
                matches!(
                    message,
                    MessageBlock::User(user) if user.kind.is_compaction_summary()
                )
            })
            .collect();
        assert_eq!(summaries.len(), 1, "one summary joins the active surface");
    }
    result
}

/// Asserts the complete failure-atomicity contract: neither the hot state
/// nor the durable authority shows any part of the aborted transition.
fn assert_no_transition(store: &dyn ConversationStore, history: &[MessageBlock]) {
    let head = store.load_head().expect("durable head");
    assert_eq!(
        head.revision,
        rustx::conversation::SurfaceRevision::new(history.len() as u64),
        "the durable Surface revision is untouched"
    );
    assert_eq!(head.compaction_generation, 0);
    assert!(
        !store
            .load_canonical()
            .expect("canonical")
            .iter()
            .any(|message| matches!(
                message,
                MessageBlock::User(user) if user.kind.is_compaction_summary()
            )),
        "no canonical summary was committed"
    );
}

/// A committed compaction installs the structured text and typed metadata in
/// one transition, and a reopen recovers them exactly from canonical durable
/// state.
#[tokio::test]
async fn committed_compaction_round_trips_through_the_durable_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rustx.sqlite");
    let history = file_operation_history();
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "## Goal\n\nShip the feature.".to_owned(),
    )]));
    let context = runtime(10_000, summarizer);
    let committed_text;
    let committed_metadata;
    {
        let store = SqliteConversationStore::open(conversation(), &path).expect("open store");
        let executed = run_pipeline(&history, &store, &context, &CancellationSignal::new())
            .await
            .expect("compaction commits");
        let MessageBlock::User(summary) = &executed.summary_block else {
            panic!("a compaction commits a canonical User summary");
        };
        committed_text = text_of(summary).to_owned();
        committed_metadata = metadata_of(summary).clone();
    }
    let reopened = SqliteConversationStore::open(conversation(), &path).expect("reopen store");
    let canonical = reopened.load_canonical().expect("load canonical");
    let summary = canonical
        .iter()
        .find_map(|message| match message {
            MessageBlock::User(user) if user.kind.is_compaction_summary() => Some(user),
            _ => None,
        })
        .expect("the summary survives restart");
    assert_eq!(text_of(summary), committed_text);
    assert_eq!(metadata_of(summary), &committed_metadata);
    assert_eq!(metadata_of(summary).read_files(), &["/src/a.rs".to_owned()]);
    assert_eq!(
        metadata_of(summary).modified_files(),
        &["/src/b.rs".to_owned(), "/src/c.rs".to_owned()]
    );
    let head = reopened.load_head().expect("durable head");
    assert_eq!(head.compaction_generation, 1);
}

/// A summary-model failure installs no part of the transition.
#[tokio::test]
async fn summarizer_failure_installs_nothing() {
    let history = file_operation_history();
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Fail(
        rustx::context::ContextError::new(
            rustx::context::ContextErrorKind::SummaryFailed,
            "scripted summary failure",
        ),
    )]));
    let context = runtime(10_000, summarizer);
    let result = run_pipeline(&history, &store, &context, &CancellationSignal::new()).await;
    assert!(result.is_err());
    assert_no_transition(&store, &history);
}

/// Cancellation observable before the compaction begins installs nothing.
#[tokio::test]
async fn cancellation_before_compaction_installs_nothing() {
    let history = file_operation_history();
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "never used".to_owned(),
    )]));
    let context = runtime(10_000, summarizer);
    let cancellation = CancellationSignal::new();
    cancellation.cancel();
    let result = run_pipeline(&history, &store, &context, &cancellation).await;
    assert!(result.is_err());
    assert_no_transition(&store, &history);
}

/// Cancellation observed while the summary is parked installs nothing.
#[tokio::test]
async fn cancellation_during_summary_installs_nothing() {
    let history = file_operation_history();
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    let summarizer = support::context::shared_summarizer(FakeContextSummarizer::new(vec![
        FakeSummaryStep::ParkUntilCancelled,
    ]));
    let context = runtime(10_000, summarizer.clone());
    let cancellation = CancellationSignal::new();
    let mut parked = summarizer.parked();
    let canceller = cancellation.clone();
    let cancel_when_parked = async move {
        parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("the summarizer parks");
        canceller.cancel();
    };
    let (result, ()) = tokio::join!(
        run_pipeline(&history, &store, &context, &cancellation),
        cancel_when_parked
    );
    assert!(result.is_err());
    assert_no_transition(&store, &history);
}

/// A post-summary cannot-fit failure installs nothing.
#[tokio::test]
async fn cannot_fit_after_summary_installs_nothing() {
    // Window 12 with output budget 1 gives a soft input limit of 11. The
    // plan reserves only the 1-token summary budget, so it is admitted; the
    // scripted 48-byte summary then estimates to 12 and fails the exact
    // post-summary fit check.
    let history = vec![user("u1"), user("u2")];
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "S".repeat(48),
    )]));
    let context = runtime(12, summarizer);
    let result = run_pipeline(&history, &store, &context, &CancellationSignal::new()).await;
    assert!(result.is_err());
    assert_no_transition(&store, &history);
}

/// A durable commit failure installs nothing, in either authority.
#[tokio::test]
async fn durable_commit_failure_installs_nothing() {
    let history = file_operation_history();
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    store.arm_compaction_fault_script([
        crate::durable::sqlite::CompactionFaultOperation::BeforeSummaryInsert,
    ]);
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "## Goal\n\nShip the feature.".to_owned(),
    )]));
    let context = runtime(10_000, summarizer);
    let result = run_pipeline(&history, &store, &context, &CancellationSignal::new()).await;
    assert!(matches!(
        result,
        Err(crate::context::compaction::CompactionExecutionError::Durable(_))
    ));
    assert_no_transition(&store, &history);
}
