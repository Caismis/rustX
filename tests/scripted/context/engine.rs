//! The Context Engine owner: provider-independent projection, token
//! accounting, compaction planning, and the semantic commit contract.
//!
//! Every test drives `ContextEngine` / `ConversationState` directly over
//! scripted estimators — no `AgentExecution`, no runtime composition. The
//! contracts owned here: complete-message current-Surface projection, exact
//! token accounting and observed-usage anchoring, threshold/budget
//! evaluation, complete tool-unit cut points, recent-suffix retention,
//! summary-span budgeting, no resurrection of retired canonical history,
//! the semantic commit shape (one summary + one replacement, progress rule),
//! continuation-ownership constraints of span selection, and the
//! source-level provider isolation of the context plane.
//!
//! The committed pipeline transition (summarize -> validate exact
//! post-summary fit -> durable commit -> hot-state installation) is owned by
//! `compaction_pipeline.rs`; `AgentExecution`/`ConversationRuntime`
//! composition is owned by `runtime_integration.rs` and
//! `runtime_multi_compaction.rs`.

use super::super::{common, support};

use std::sync::Arc;

use rustx::context::{
    AcceptedSystemSection, ClosureTokenEstimator, CompactionBudgets, ContextConfig, ContextEngine,
    ContextError, ContextErrorKind, DefaultTokenEstimator, ObservedAnchor, ProviderObservedInput,
    SummaryRequest, SystemSectionLane, TokenEstimator, render_effective_system_prompt,
};
use rustx::conversation::{
    ConversationState, SurfaceOp, SurfaceRevision, SurfaceSpan, summary_message_id,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, CompactionSummaryMetadata, InboundKind,
    MessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::ModelInputMessage;

use rustx::runtime::identity::{
    ContextContributorIdentity, ConversationId, MessageId, NativeContextContributor, ToolCallId,
    ToolId,
};

use rustx::runtime::types::{TokenMeasurement, TokenMeasurementSource};

use rustx::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
use support::context::ScriptedEstimator;

fn user(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn text_block(text: &str) -> AssistantContentBlock {
    AssistantContentBlock::Text(TextBlock {
        text: text.to_owned(),
    })
}

fn call_block(id: &str) -> AssistantContentBlock {
    AssistantContentBlock::ToolCall(ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
        arguments: serde_json::json!({}),
    })
}

fn assistant(id: &str, blocks: Vec<AssistantContentBlock>) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: blocks,
    })
}

fn tool_message(id: &str, call_id: &str) -> MessageBlock {
    MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new(id),
        tool_call_id: ToolCallId::new(call_id),
        tool_id: ToolId::new("tool-alpha"),
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![],
            duration_ms: 1,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        },
    })
}

fn engine(
    window: u64,
    reserve: u64,
    keep_recent: u64,
    estimator: Arc<dyn TokenEstimator>,
) -> ContextEngine {
    ContextEngine::new(
        ContextConfig {
            context_window_tokens: window,
            reserve_tokens: reserve,
            keep_recent_tokens: keep_recent,
        },
        estimator,
    )
    .expect("valid context configuration")
}

fn weighted(per_message: u64, per_block: u64, per_tool: u64) -> Arc<ScriptedEstimator> {
    Arc::new(ScriptedEstimator::new(per_message, per_block, per_tool))
}

fn scripted(
    per_message: u64,
    per_block: u64,
    per_tool: u64,
    overrides: &[(&str, u64)],
) -> Arc<ScriptedEstimator> {
    let mut estimator = ScriptedEstimator::new(per_message, per_block, per_tool);
    for (id, tokens) in overrides {
        estimator = estimator.with_override(id, *tokens);
    }
    Arc::new(estimator)
}

fn conversation() -> ConversationId {
    ConversationId::new("conv-1")
}

fn summary_id(generation: u64) -> MessageId {
    summary_message_id(&conversation(), generation)
}

/// One conversation state bootstrapped from ordered canonical messages.
fn state(messages: Vec<MessageBlock>) -> ConversationState {
    ConversationState::from_messages(messages).expect("bootstrap conversation")
}

/// The active Surface identities of a conversation state, as strings.
fn active_ids(state: &ConversationState) -> Vec<String> {
    state
        .active_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

/// The committed Message Ledger identities, in commit order, as strings.
fn ledger_ids(state: &ConversationState) -> Vec<String> {
    state
        .ledger()
        .audit_records()
        .iter()
        .map(message_id_of)
        .collect()
}

/// The projected model-visible identities of a projection, as strings.
fn projected_ids(projection: &rustx::context::ContextProjection) -> Vec<String> {
    projection.messages.iter().map(message_id_of).collect()
}

/// Plans, summarizes, and applies one compaction against a conversation
/// state, returning the committed record.
fn compact(
    engine: &ContextEngine,
    state: &mut ConversationState,
    summary_text: &str,
    budgets: CompactionBudgets,
) -> Result<rustx::conversation::CompactionRecord, ContextError> {
    compact_with(
        engine,
        state,
        summary_text,
        budgets,
        &rustx::context::CompactionConstraints::default(),
        &[],
    )
}

/// The same, with explicit constraints and tool definitions.
fn compact_with(
    engine: &ContextEngine,
    state: &mut ConversationState,
    summary_text: &str,
    budgets: CompactionBudgets,
    constraints: &rustx::context::CompactionConstraints<'_>,
    tools: &[rustx::tools::types::ModelToolDefinition],
) -> Result<rustx::conversation::CompactionRecord, ContextError> {
    let projection = engine.build_projection(state, tools, None, "")?;
    let plan = engine.plan_compaction(state, &projection, tools, budgets, constraints)?;
    let (commit, _) =
        engine.prepare_compaction(state, &conversation(), &plan, summary_text, tools)?;
    state
        .commit_compaction(commit)
        .map_err(|error| ContextError::new(ContextErrorKind::Internal, error.to_string()))
}

fn message_id_of(message: &MessageBlock) -> String {
    match message {
        MessageBlock::User(user) => user.id.as_str().to_owned(),
        MessageBlock::Assistant(assistant) => assistant.id.as_str().to_owned(),
        MessageBlock::Tool(tool) => tool.id.as_str().to_owned(),
    }
}

fn newly_retired_id(item: &MessageBlock) -> String {
    message_id_of(item)
}

// ---------------------------------------------------------------------------
// Issue #22 — drained inbound batches before M4 projection/compaction
// ---------------------------------------------------------------------------

/// A conversation state that has already compacted once: the `span` is
/// replaced by the canonical generation-1 runtime summary.
fn compacted_state(
    messages: Vec<MessageBlock>,
    span: SurfaceSpan,
    summary_text: &str,
) -> ConversationState {
    let mut state = state(messages);
    let summary = UserMessageBlock {
        id: summary_id(1),
        content: vec![UserContentBlock::Text(TextBlock {
            text: summary_text.to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary(CompactionSummaryMetadata::empty()),
        timestamp: None,
    };
    let commit = state
        .prepare_compaction(summary, span)
        .expect("a valid compaction span");
    state.commit_compaction(commit).expect("commit compaction");
    state
}

// ---------------------------------------------------------------------------
// Context assembly
// ---------------------------------------------------------------------------

/// A short conversation stays below the threshold: no compaction.
#[test]
fn short_history_requires_no_compaction() {
    let engine = engine(100, 10, 5, weighted(10, 10, 10));
    let state = state(vec![user("u1", "hi"), user("u2", "bye")]);
    let projection = engine
        .build_projection(&state, &[], None, "")
        .expect("projection");
    assert!(
        !engine
            .should_compact(&projection, 0)
            .expect("threshold decision")
    );
    assert!(engine.fits_under_soft_limit(&projection, 0).expect("fits"));
}

/// Request-time Skill capability guidance is not a Surface fact: compacting
/// canonical conversation messages leaves the frozen system section visible.
#[test]
fn compaction_cannot_remove_request_time_skill_catalog_guidance() {
    let skill_catalog = "## Skills\n\n<available_skills>\n  <skill>\n    <name>pdf</name>\n    <description>Create PDF documents.</description>\n    <location>/workspace/.agents/skills/pdf/SKILL.md</location>\n  </skill>\n</available_skills>";
    let sections = [AcceptedSystemSection {
        lane: SystemSectionLane::NativeCapabilityGuidance,
        contributor: ContextContributorIdentity::Native(NativeContextContributor::SkillGuidance),
        content: skill_catalog.to_owned(),
    }];
    let mut state = state(vec![user("u1", "first"), user("u2", "second")]);
    // The exact per-Skill locator is intentionally part of the frozen prompt;
    // leave enough deterministic room for the compact system section while
    // still compacting the canonical messages below it.
    let engine = engine(1_000, 0, 0, weighted(10, 0, 0));
    let before_prompt = render_effective_system_prompt(&sections);
    assert_eq!(before_prompt, skill_catalog);
    let projection = engine
        .build_projection(&state, &[], None, &before_prompt)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &state,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("a complete canonical span is compactable");
    let (commit, _) = engine
        .prepare_compaction(&state, &conversation(), &plan, "summary", &[])
        .expect("prepare compaction");
    state.commit_compaction(commit).expect("commit compaction");

    let after_messages = state
        .active_messages()
        .expect("active messages after compaction");
    let after_prompt = render_effective_system_prompt(&sections);
    assert_eq!(after_prompt, skill_catalog);
    assert!(
        after_messages.iter().all(|message| {
            !serde_json::to_string(message)
                .expect("serialize canonical message")
                .contains("## Skills")
        }),
        "the catalog remains outside canonical history after compaction"
    );
}

/// The projection is exactly the current Surface, in Surface order, as
/// complete canonical messages — and it is a pure function of that Surface
/// revision.
#[test]
fn projection_is_the_current_surface_in_order() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let state = compacted_state(
        vec![
            user("u1", "hi"),
            assistant("a1", vec![text_block("ok")]),
            user("u2", "more"),
        ],
        SurfaceSpan::new(MessageId::new("u1"), MessageId::new("a1")),
        "earlier summary",
    );
    let first = engine
        .build_projection(&state, &[], None, "")
        .expect("projection");
    let second = engine
        .build_projection(&state, &[], None, "")
        .expect("projection again");
    assert_eq!(first, second, "projection must be a pure function");
    assert_eq!(first.surface_revision, state.revision());
    assert_eq!(
        projected_ids(&first),
        vec![summary_id(1).as_str().to_owned(), "u2".to_owned(),],
        "the projection is exactly the active surface"
    );
    assert!(
        first.messages.iter().all(|message| matches!(
            message,
            MessageBlock::User(_) | MessageBlock::Assistant(_) | MessageBlock::Tool(_)
        )),
        "every projected item is a complete canonical message"
    );
    // Compaction never rewrote the ledger.
    assert_eq!(
        ledger_ids(&state),
        vec![
            "u1".to_owned(),
            "a1".to_owned(),
            "u2".to_owned(),
            summary_id(1).as_str().to_owned(),
        ]
    );
}

/// The same history produces the same estimate.
#[test]
fn same_context_produces_same_estimate() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let state = state(vec![
        user("u1", "hi"),
        assistant("a1", vec![text_block("ok")]),
    ]);
    let first = engine
        .build_projection(&state, &[], None, "")
        .expect("projection");
    let second = engine
        .build_projection(&state, &[], None, "")
        .expect("projection again");
    assert_eq!(first.estimated_input, second.estimated_input);
    assert_eq!(
        first.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
}

/// Tool definitions contribute to the planned request estimate.
#[test]
fn tool_definitions_contribute_to_the_request_estimate() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let state = state(vec![user("u1", "hi")]);
    let tools = vec![
        common::model_tool("alpha", "tool-alpha"),
        common::model_tool("beta", "tool-beta"),
    ];
    let without_tools = engine
        .build_projection(&state, &[], None, "")
        .expect("projection without tools");
    let with_tools = engine
        .build_projection(&state, &tools, None, "")
        .expect("projection with tools");
    assert_eq!(with_tools.estimated_input.input_tokens, 30);
    assert_eq!(without_tools.estimated_input.input_tokens, 10);
}

/// Tool definitions never satisfy the recent-conversation retention target:
/// the retention decision is a pure function of conversation content, while
/// the full request estimate still includes the tool overhead.
#[test]
fn tool_definitions_never_satisfy_the_recent_retention_target() {
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![text_block("x")]),
        user("u2", ""),
        assistant("a2", vec![text_block("y")]),
    ]);
    let tools = vec![common::model_tool("alpha", "tool-alpha")];
    // Target 20: with conversation weights of 10/10, retiring u1 and a1
    // retains exactly u2+a2 = 20. If the huge tool weight counted toward the
    // target, the engine would retire everything instead.
    let cheap = engine(10_000_000, 0, 20, weighted(10, 10, 0));
    let expensive = engine(10_000_000, 0, 20, weighted(10, 10, 1_000_000));
    let projection_cheap = cheap
        .build_projection(&history, &tools, None, "")
        .expect("projection");
    let projection_expensive = expensive
        .build_projection(&history, &tools, None, "")
        .expect("projection");
    let plan_cheap = cheap
        .plan_compaction(
            &history,
            &projection_cheap,
            &tools,
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let plan_expensive = expensive
        .plan_compaction(
            &history,
            &projection_expensive,
            &tools,
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // Identical retention decision: the tool weight changes the full request
    // estimate but never the recent-conversation target.
    assert_eq!(plan_cheap.span.end, MessageId::new("a1"));
    assert_eq!(plan_cheap.span.end, plan_expensive.span.end);
    // The full request estimate still reflects the tool overhead.
    assert!(
        plan_expensive.planned_estimate_after > plan_cheap.planned_estimate_after,
        "tool definitions still affect the full request estimate"
    );
}

// ---------------------------------------------------------------------------
// Token accounting
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Token accounting
// ---------------------------------------------------------------------------

/// A measurement recorded without a structural anchor applies only to
/// exactly the projection that was measured; everything else is a
/// deterministic estimate.
#[test]
fn provider_reported_usage_applies_only_to_the_exact_projection() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let history = state(vec![user("u1", "hi")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: projection.fingerprint(),
        input_tokens: 42,
        anchor: None,
    };
    let measured = engine
        .build_projection(&history, &[], Some(&observed), "")
        .expect("projection with observed usage");
    assert_eq!(measured.estimated_input.input_tokens, 42);
    assert_eq!(
        measured.estimated_input.source,
        TokenMeasurementSource::ProviderReported
    );

    // A different history is a different projection, and without an anchor
    // the measurement cannot be carried forward at all.
    let grown = state(vec![user("u1", "hi"), user("u2", "more")]);
    let estimated = engine
        .build_projection(&grown, &[], Some(&observed), "")
        .expect("projection with stale observation");
    assert_eq!(estimated.estimated_input.input_tokens, 20);
    assert_eq!(
        estimated.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
}

/// A provider measurement stays authoritative for the prefix it covers.
///
/// This is the difference between token accounting that drifts and token
/// accounting that does not. A whole-conversation estimate compounds
/// estimator error over every message ever sent; an anchored measurement
/// keeps the provider's own number for everything it measured and estimates
/// only what was appended since.
#[test]
fn a_provider_measurement_anchors_every_context_that_extends_it() {
    let engine = engine(10_000, 0, 0, weighted(10, 10, 10));
    let measured = state(vec![user("u1", ""), user("u2", "")]);
    let before = engine
        .build_projection(&measured, &[], None, "")
        .expect("projection");
    // The deterministic estimate of the measured context is 20; the provider
    // counted 900 for the very same request.
    assert_eq!(before.estimated_input.input_tokens, 20);
    let observed = ProviderObservedInput {
        fingerprint: before.fingerprint(),
        input_tokens: 900,
        anchor: Some(ObservedAnchor::of(&before.messages, "", &[])),
    };

    let mut grown = state(vec![user("u1", ""), user("u2", "")]);
    grown
        .commit(assistant("a1", vec![text_block("x")]))
        .expect("append the assistant turn");
    grown.commit(user("u3", "")).expect("append the next turn");
    let anchored = engine
        .build_projection(&grown, &[], Some(&observed), "")
        .expect("anchored projection");
    assert_eq!(
        anchored.estimated_input.source,
        TokenMeasurementSource::ProviderAnchored
    );
    assert_eq!(
        anchored.estimated_input.input_tokens, 920,
        "900 measured for the covered prefix, plus the estimate of exactly \
         the two messages appended since"
    );
}

/// The anchor covers the Effective System Prompt and the tool definitions of
/// the measured request, so a change to either refuses it outright. The
/// runtime never patches a stale measurement with a guessed delta.
#[test]
fn an_anchor_is_refused_when_the_non_conversation_input_changed() {
    let engine = engine(10_000, 0, 0, weighted(10, 10, 10));
    let measured = state(vec![user("u1", "")]);
    let before = engine
        .build_projection(&measured, &[], None, "")
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: before.fingerprint(),
        input_tokens: 900,
        anchor: Some(ObservedAnchor::of(&before.messages, "", &[])),
    };
    let mut grown = state(vec![user("u1", "")]);
    grown.commit(user("u2", "")).expect("append");

    assert_eq!(
        engine
            .build_projection(&grown, &[], Some(&observed), "")
            .expect("projection")
            .estimated_input
            .source,
        TokenMeasurementSource::ProviderAnchored,
        "unchanged non-conversation input anchors"
    );
    assert_eq!(
        engine
            .build_projection(&grown, &[], Some(&observed), "new guidance")
            .expect("projection")
            .estimated_input
            .source,
        TokenMeasurementSource::Estimated,
        "a changed Effective System Prompt refuses the anchor"
    );
    assert_eq!(
        engine
            .build_projection(
                &grown,
                &[common::model_tool("alpha", "tool-alpha")],
                Some(&observed),
                ""
            )
            .expect("projection")
            .estimated_input
            .source,
        TokenMeasurementSource::Estimated,
        "a changed tool set refuses the anchor"
    );
}

/// A Surface rewrite invalidates a stale provider-reported measurement: the
/// request context it measured no longer exists, so not even its anchor
/// survives. An ordinary append is the opposite case — the measured context
/// is still a prefix, so the measurement is carried forward.
#[test]
fn a_surface_rewrite_invalidates_a_stale_observed_measurement() {
    let engine = engine(10_000, 0, 0, weighted(10, 10, 10));
    let mut history = state(vec![user("u1", ""), user("u2", ""), user("u3", "")]);
    let before = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: before.fingerprint(),
        input_tokens: 42,
        anchor: Some(ObservedAnchor::of(&before.messages, "", &[])),
    };
    // The measurement applies to exactly the context it measured.
    assert_eq!(
        engine
            .build_projection(&history, &[], Some(&observed), "")
            .expect("measured projection")
            .estimated_input
            .source,
        TokenMeasurementSource::ProviderReported
    );
    // A surface rewrite establishes a new revision and new content: the old
    // measurement can never apply to it.
    compact(
        &engine,
        &mut history,
        "s1",
        CompactionBudgets::new(0, 0, 1_000_000),
    )
    .expect("compact");
    let after = engine
        .build_projection(&history, &[], Some(&observed), "")
        .expect("projection after the rewrite");
    assert_ne!(after.surface_revision, before.surface_revision);
    assert_eq!(
        after.estimated_input.source,
        TokenMeasurementSource::Estimated,
        "a surface rewrite must invalidate the stale observed measurement"
    );
    // An ordinary append is not a rewrite: the measured context is still an
    // ordered prefix, so its provider-reported cost is kept and only the
    // appended message is estimated.
    let mut appended = state(vec![user("u1", ""), user("u2", ""), user("u3", "")]);
    appended.commit(user("u4", "")).expect("append");
    let extended = engine
        .build_projection(&appended, &[], Some(&observed), "")
        .expect("projection after the append");
    assert_eq!(
        extended.estimated_input.source,
        TokenMeasurementSource::ProviderAnchored
    );
    assert_eq!(extended.estimated_input.input_tokens, 52);
}

/// Missing provider usage means the deterministic estimate, never a
/// fabricated measurement.
#[test]
fn missing_usage_falls_back_to_the_estimate() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let history = state(vec![user("u1", "hi")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    assert_eq!(projection.estimated_input.input_tokens, 10);
    assert_eq!(
        projection.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
}

/// An estimate never becomes provider usage: the measurement stays an
/// estimate with explicit provenance, and no `ModelUsage` is derived from
/// it anywhere in the context plane.
#[test]
fn estimates_never_become_model_usage() {
    let engine = engine(1_000, 10, 5, Arc::new(DefaultTokenEstimator));
    let history = state(vec![user("u1", "hi")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    assert_eq!(
        projection.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
    // `TokenMeasurement` is the only measurement type the projection
    // carries: there is no conversion path from an estimate to `ModelUsage`.
    assert!(matches!(
        projection.estimated_input,
        TokenMeasurement {
            source: TokenMeasurementSource::Estimated,
            ..
        }
    ));
}

/// The default estimator is deterministic and implements the documented
/// `ceil(bytes / 4)` formula over runtime-owned canonical serialization.
#[test]
fn default_estimator_formula_is_frozen() {
    let engine = engine(1_000, 10, 5, Arc::new(DefaultTokenEstimator));
    let history = state(vec![user("u1", "hi")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let expected =
        rustx::context::bytes_to_tokens(rustx::context::DefaultTokenEstimator::serialized_bytes(
            &rustx::model::input::canonical_input(&projection.messages),
            "",
            &[],
        ));
    assert_eq!(projection.estimated_input.input_tokens, expected);
}

// ---------------------------------------------------------------------------
// Threshold
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Threshold
// ---------------------------------------------------------------------------

/// Compaction triggers at `estimated >= soft_input_limit`; equality
/// compacts deterministically.
#[test]
fn threshold_equality_compacts() {
    let engine = engine(100, 0, 5, weighted(20, 20, 20));
    let at = engine
        .build_projection(
            &state(vec![
                user("u1", ""),
                user("u2", ""),
                user("u3", ""),
                user("u4", ""),
                user("u5", ""),
            ]),
            &[],
            None,
            "",
        )
        .expect("projection");
    assert_eq!(at.estimated_input.input_tokens, 100);
    assert!(
        engine
            .should_compact(&at, 0)
            .expect("at threshold: compact")
    );

    let below = engine
        .build_projection(
            &state(vec![
                user("u1", ""),
                user("u2", ""),
                user("u3", ""),
                user("u4", ""),
            ]),
            &[],
            None,
            "",
        )
        .expect("projection");
    assert_eq!(below.estimated_input.input_tokens, 80);
    assert!(
        !engine
            .should_compact(&below, 0)
            .expect("below threshold: no compaction")
    );

    let above = engine
        .build_projection(
            &state(vec![
                user("u1", ""),
                user("u2", ""),
                user("u3", ""),
                user("u4", ""),
                user("u5", ""),
                user("u6", ""),
            ]),
            &[],
            None,
            "",
        )
        .expect("projection");
    assert_eq!(above.estimated_input.input_tokens, 120);
    assert!(engine.should_compact(&above, 0).expect("above threshold"));
}

/// The soft limit accounts for the output budget and the reserve.
#[test]
fn soft_limit_respects_output_budget_and_reserve() {
    let engine = engine(200, 40, 5, weighted(10, 10, 10));
    assert_eq!(engine.soft_input_limit(0).expect("no output"), 160);
    assert_eq!(engine.soft_input_limit(60).expect("output budget"), 100);
    assert!(
        engine.soft_input_limit(160).is_err(),
        "window <= reserve + output must be rejected"
    );
    assert!(
        engine.soft_input_limit(161).is_err(),
        "window < reserve + output must be rejected"
    );
}

/// The primary output budget owns the soft input limit while the summary
/// invocation owns the reservation used by the planner's hard-fit choice.
/// A smaller summary than the primary leaves the recent-turn boundary viable.
#[test]
fn compaction_uses_primary_budget_for_soft_limit_and_smaller_summary_reservation() {
    let engine = engine(40, 0, 20, weighted(10, 10, 0));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![text_block("x")]),
        user("u2", ""),
        assistant("a2", vec![text_block("y")]),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(10, 5, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan fits with the smaller summary reservation");

    assert_eq!(plan.summary_reservation, 5);
    assert_eq!(plan.span.end, MessageId::new("a1"));
    assert!(plan.planned_estimate_after <= 30);
}

/// An explicit summary model with a larger output budget can force the
/// planner to retire a whole additional turn, even though the primary soft
/// input limit is unchanged. This proves the hard-fit decision uses the
/// summary reservation rather than merely observing a provider request.
#[test]
fn compaction_uses_larger_explicit_summary_reservation_for_hard_fit() {
    let engine = engine(40, 0, 20, weighted(10, 10, 0));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![text_block("x")]),
        user("u2", ""),
        assistant("a2", vec![text_block("y")]),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(10, 25, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("full compaction fits with the larger summary reservation");

    assert_eq!(plan.summary_reservation, 25);
    assert_eq!(plan.span.end, MessageId::new("a2"));
    assert!(plan.planned_estimate_after <= 30);
}

/// Issue #12 (M9b), Finding 1: the planner evaluates every compaction
/// candidate against the exact hypothetical post-compaction request —
/// retained Surface + staged request-scoped context + Effective System
/// Prompt + tools — through the same estimator, never as a scalar token
/// delta.
///
/// The estimator is deliberately non-additive: the staged `ctx` message
/// costs 10 tokens while the `marker` message is still active, but 100 once
/// compaction retires `marker`. A scalar-delta implementation would compute
/// the staged reservation against the full pre-compaction surface (10) and
/// therefore believe "retire only `marker`" fits; the exact hypothetical
/// evaluation charges 100 for the staged context against that candidate and
/// selects "retire `marker` + `recent1`" instead.
#[test]
fn staged_context_is_evaluated_exactly_per_compaction_candidate() {
    let estimator: Arc<dyn TokenEstimator> = Arc::new(ClosureTokenEstimator::new(
        |messages: &[ModelInputMessage],
         _effective_system_prompt: &str,
         _tools: &[rustx::tools::types::ModelToolDefinition]| {
            let canonical = messages
                .iter()
                .filter_map(ModelInputMessage::as_canonical)
                .cloned()
                .collect::<Vec<_>>();
            let marker_active = canonical
                .iter()
                .any(|message| message_id_of(message) == "marker");
            canonical
                .iter()
                .map(|message| match message_id_of(message).as_str() {
                    "marker" => 100,
                    "ctx" => {
                        if marker_active {
                            10
                        } else {
                            100
                        }
                    }
                    _ => {
                        if matches!(
                            message,
                            MessageBlock::User(user)
                                if user.kind.is_compaction_summary()
                        ) {
                            1
                        } else {
                            50
                        }
                    }
                })
                .sum()
        },
    ));
    let engine = engine(180, 0, 500, estimator);
    let history = state(vec![
        user("marker", ""),
        user("recent1", ""),
        user("recent2", ""),
    ]);
    let staged = vec![user("ctx", "staged request context")];
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 1, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: None,
                fresh_inbound: None,
                staged_request_context: &staged,
                carryover: None,
                carryover_anchor: None,
                estimate_correction: None,
            },
        )
        .expect("the exact hypothetical evaluation selects a fitting candidate");

    // The correct candidate retires `marker` + `recent1`: retiring only
    // `marker` leaves the staged context costing 100, so its exact
    // hypothetical request (1 + 50 + 50 + 100 = 201) exceeds the soft limit
    // of 180 even though the buggy scalar reservation (10) would have
    // declared it fitting.
    assert_eq!(plan.span.end, MessageId::new("recent1"));
    assert_eq!(plan.planned_estimate_after, 151);

    // The committed compaction leaves the exact staged request fitting — no
    // avoidable CannotFit after an insufficient compaction was committed.
    let (_, after) = engine
        .prepare_compaction(&history, &conversation(), &plan, "S", &[])
        .expect("the chosen plan prepares");
    let exact_after = engine.estimate_with_staged_context(&after, &staged, &[]);
    assert!(
        exact_after < engine.soft_input_limit(0).expect("soft limit"),
        "the committed compaction leaves the exact staged request fitting: {exact_after}"
    );
}

/// Impossible context configurations are rejected explicitly; no fallback
/// constant is hidden.
#[test]
fn invalid_configuration_is_rejected() {
    let error = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100,
            reserve_tokens: 100,
            keep_recent_tokens: 5,
        },
        weighted(10, 10, 10),
    )
    .expect_err("window == reserve must be rejected");
    assert_eq!(error.kind, ContextErrorKind::InvalidConfiguration);

    let error = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 90,
            reserve_tokens: 100,
            keep_recent_tokens: 5,
        },
        weighted(10, 10, 10),
    )
    .expect_err("window < reserve must be rejected");
    assert_eq!(error.kind, ContextErrorKind::InvalidConfiguration);

    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100,
            reserve_tokens: 99,
            keep_recent_tokens: 5,
        },
        weighted(10, 10, 10),
    )
    .expect("a one-token budget is legal");
    assert_eq!(
        engine
            .soft_input_limit(1)
            .expect_err("no room for output")
            .kind,
        ContextErrorKind::InvalidConfiguration
    );
    assert_eq!(engine.soft_input_limit(0).expect("one token"), 1);
}

// ---------------------------------------------------------------------------
// Whole cut points
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Whole cut points
// ---------------------------------------------------------------------------

/// A simple complete turn retires at a whole-turn boundary that covers the
/// turn's tool results.
#[test]
fn simple_complete_turn_boundary() {
    let engine = engine(100, 0, 5, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.start, MessageId::new("u1"));
    assert_eq!(plan.span.end, MessageId::new("t1"));
    assert_eq!(plan.retired.len(), 3, "the complete turn is retired whole");
}

/// Multiple tool calls of one Assistant message are never separated from their
/// results, and the Assistant message is never split: a span that would retire
/// one call without its result is structurally rejected, and the only
/// admissible spans keep every call together with its result.
#[test]
fn multiple_tool_calls_stay_with_their_results() {
    let engine = engine(10_000, 0, 0, weighted(100, 10, 100));
    let mut conversation_state = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1"), call_block("c2")]),
        tool_message("t1", "c1"),
        tool_message("t2", "c2"),
    ]);
    // The Assistant message can never be replaced without its results.
    for end in ["a1", "t1"] {
        let error = conversation_state
            .prepare_compaction(
                UserMessageBlock {
                    id: summary_id(1),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "s".to_owned(),
                    })],
                    source: UserSource::Runtime,
                    kind: InboundKind::CompactionSummary(CompactionSummaryMetadata::empty()),
                    timestamp: None,
                },
                SurfaceSpan::new(MessageId::new("u1"), MessageId::new(end)),
            )
            .expect_err("a tool pair may never be split");
        assert!(
            format!("{error}").contains("separate tool call"),
            "unexpected error: {error}"
        );
    }
    // The engine only ever plans a structurally complete span.
    let projection = engine
        .build_projection(&conversation_state, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &conversation_state,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("t2"));
    compact(
        &engine,
        &mut conversation_state,
        "s1",
        CompactionBudgets::new(0, 0, 1_000_000),
    )
    .expect("compact");
    assert_eq!(
        active_ids(&conversation_state),
        vec![summary_id(1).as_str()]
    );
    assert_eq!(
        ledger_ids(&conversation_state),
        vec![
            "u1".to_owned(),
            "a1".to_owned(),
            "t1".to_owned(),
            "t2".to_owned(),
            summary_id(1).as_str().to_owned(),
        ],
        "every retired original survives in the ledger"
    );
}

/// Orphan tool messages are malformed history, never guessed around.
#[test]
fn orphan_tool_message_is_rejected() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![user("u1", ""), tool_message("t1", "ghost")]);
    let error = engine
        .build_projection(&history, &[], None, "")
        .expect_err("malformed history");
    assert_eq!(error.kind, ContextErrorKind::MalformedHistory);
}

/// No tool-call/result edge crosses the chosen cut: turns are retired or
/// retained whole.
#[test]
fn no_edge_crosses_the_chosen_cut() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        assistant("a2", vec![call_block("c2")]),
        tool_message("t2", "c2"),
        user("u2", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let mut history = history;
    let record = compact(
        &engine,
        &mut history,
        "s1",
        CompactionBudgets::new(0, 0, 1_000_000),
    )
    .expect("compact");
    assert_eq!(record.generation, 1);
    // Only the summary and the final user message remain active: both turns
    // were retired whole, so no edge can cross the replacement boundary.
    assert_eq!(
        active_ids(&history),
        vec!["conv-1-summary-1".to_owned(), "u2".to_owned()]
    );
    let _ = projection;
}

/// Candidate selection is deterministic: the same plan twice.
#[test]
fn candidate_selection_is_deterministic() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
        assistant("a2", vec![call_block("c2")]),
        tool_message("t2", "c2"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let first = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let second = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan again");
    assert_eq!(first, second);
}

/// Retention is a token target over a deterministic inclusive Surface prefix,
/// and the canonical active sequence has no hidden System barrier.
#[test]
fn planner_selects_the_exact_inclusive_span_without_a_system_barrier() {
    let history = state(vec![
        user("old", "old history"),
        user("middle", "middle history"),
        user("recent", "recent history"),
        user("newest", "newest history"),
    ]);
    let retaining = engine(
        1_000,
        0,
        30,
        scripted(
            10,
            10,
            0,
            &[("old", 40), ("middle", 30), ("recent", 20), ("newest", 10)],
        ),
    );
    let projection = retaining
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = retaining
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.start, MessageId::new("old"));
    assert_eq!(plan.span.end, MessageId::new("middle"));
    assert_eq!(
        plan.retired.iter().map(message_id_of).collect::<Vec<_>>(),
        vec!["old", "middle"],
        "the retired span is the exact inclusive prefix selected by the token target"
    );
    assert_eq!(
        history.active_ids()[2..]
            .iter()
            .map(MessageId::as_str)
            .collect::<Vec<_>>(),
        vec!["recent", "newest"],
        "the retained suffix is the exact recent Surface suffix"
    );

    let all_history = engine(
        1_000,
        0,
        0,
        scripted(
            10,
            10,
            0,
            &[("old", 40), ("middle", 30), ("recent", 20), ("newest", 10)],
        ),
    );
    let all_projection = all_history
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let all_plan = all_history
        .plan_compaction(
            &history,
            &all_projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("the complete canonical prefix is compactable");
    assert_eq!(all_plan.span.start, MessageId::new("old"));
    assert_eq!(all_plan.span.end, MessageId::new("newest"));
    assert_eq!(
        all_plan
            .retired
            .iter()
            .map(message_id_of)
            .collect::<Vec<_>>(),
        vec!["old", "middle", "recent", "newest"],
        "canonical System authority cannot create an invisible planner barrier"
    );
}

/// Message count alone does not control the cut: the token target does.
#[test]
fn message_count_alone_does_not_control_the_cut() {
    // One huge message and two tiny ones; the target (25 tokens) keeps the
    // two tiny messages and retires the huge one.
    let engine = engine(1_000, 0, 25, scripted(10, 10, 10, &[("huge", 500)]));
    let history = state(vec![
        user("huge", ""),
        user("small1", ""),
        user("small2", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("huge"));
}

// ---------------------------------------------------------------------------
// Recent suffix retention
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Recent suffix retention
// ---------------------------------------------------------------------------

/// The retained suffix approximates the recent-token target.
#[test]
fn retained_suffix_approximates_the_recent_target() {
    let engine = engine(1_000, 0, 25, weighted(10, 10, 10));
    let history = state(vec![
        user("u1", ""),
        user("u2", ""),
        user("u3", ""),
        user("u4", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // Latest boundary retaining at least 25 tokens: retire one, keep three.
    assert_eq!(plan.span.end, MessageId::new("u1"));
    assert_eq!(plan.planned_estimate_after, 30);
}

/// Structural safety wins over the recent-token target: a would-be cut
/// inside a turn is skipped and the whole turn is retained.
#[test]
fn structural_rule_may_force_extra_retention() {
    let engine = engine(1_000, 0, 20, scripted(10, 10, 10, &[("t1", 100)]));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // The naive "keep the last two messages" cut would retire a1 but keep
    // t1, separating the call from its result; the valid cut retains the
    // whole turn (130 tokens) even though that exceeds the target.
    assert_eq!(plan.span.end, MessageId::new("u1"));
}

/// A token target may force retaining fewer messages when one message
/// dominates the token budget.
#[test]
fn token_target_may_retain_fewer_messages_than_recent() {
    let engine = engine(1_000, 0, 20, scripted(10, 10, 10, &[("big", 500)]));
    let history = state(vec![
        user("big", ""),
        user("m1", ""),
        user("m2", ""),
        user("m3", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // Target 20: retire the huge message and the next one, keeping exactly
    // the two recent small messages.
    assert_eq!(plan.span.end, MessageId::new("m1"));
}

// ---------------------------------------------------------------------------
// Complete-message compaction and repeated compaction
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Complete-message compaction and repeated compaction
// ---------------------------------------------------------------------------

/// Compaction operates on complete canonical messages only: a giant tool
/// result is retired intact with its owning turn, never split, and the whole
/// span reaches the summarizer.
#[test]
fn oversized_material_is_retired_as_complete_messages() {
    let engine = engine(60, 0, 5, scripted(10, 10, 10, &[("t1", 1_000)]));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.start, MessageId::new("u1"));
    assert_eq!(plan.span.end, MessageId::new("t1"));
    // The giant tool result is retired intact, as a complete canonical
    // message.
    assert!(
        plan.retired
            .iter()
            .any(|message| matches!(message, MessageBlock::Tool(tool) if tool.id.as_str() == "t1"))
    );
    assert_eq!(plan.summary_request().retired, plan.retired);
}

/// A single oversized message that must stay active produces an explicit
/// `CannotFit`, never a half-message Surface node.
///
/// The oversized fresh inbound message may not be retired, and no
/// complete-message span leaves a fitting request, so planning fails rather
/// than compiling a partial message.
#[test]
fn a_single_oversized_message_cannot_fit_instead_of_splitting() {
    let engine = engine(60, 0, 5, scripted(10, 10, 10, &[("huge", 1_000)]));
    let history = state(vec![user("u1", ""), user("huge", "")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let fresh = rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("huge")])
        .expect("fresh turn");
    let error = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: None,
                fresh_inbound: Some(&fresh),
                ..Default::default()
            },
        )
        .expect_err("no complete-message span fits");
    assert_eq!(error.kind, ContextErrorKind::CannotFit);
}

/// The planner applies the summary-model limit to the assembled summary
/// input, rather than to the number of retired canonical messages.
#[test]
fn a_span_never_exceeds_the_summary_model_input_limit() {
    let engine = engine(10_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![user("u1", ""), user("u2", ""), user("u3", "")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    // The deterministic assembly is one wrapper message, so the complete
    // summary request weighs ten tokens under this estimator.
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 25),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("u3"));
    assert_eq!(plan.summary_input_tokens, 10);
    // With no room for even one message, planning fails explicitly.
    let error = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 9),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect_err("no span fits the summary model");
    assert_eq!(error.kind, ContextErrorKind::CannotFit);
}

/// The summary-model bound is measured over the exact assembled User input,
/// not over the raw retired message serialization. Wrapper overhead can make a
/// raw span fit while the production summary request does not.
#[test]
fn summary_input_bound_accounts_for_instruction_json_and_wrapper_overhead() {
    let estimator = DefaultTokenEstimator;
    let engine = engine(1_000_000, 0, 0, Arc::new(estimator));
    let history = state(vec![user("u1", "raw retired content")]);
    let request = SummaryRequest {
        retired: vec![user("u1", "raw retired content")],
    };
    let raw_tokens = estimator.estimate_conversation_input(&request.retired);
    let assembled = request.model_input();
    let actual_tokens = estimator.estimate_conversation_input(&assembled.messages);
    assert!(
        actual_tokens > raw_tokens,
        "the canonical wrapper must cost tokens"
    );
    assert!(
        raw_tokens < actual_tokens,
        "the raw retired span must fit the deliberately one-token-too-small limit"
    );

    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let before_ids = history.active_ids().to_vec();
    let before_revision = history.revision();
    let before_ledger_len = history.ledger().len();
    let rejected = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, actual_tokens - 1),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect_err("raw fit must not hide the assembled request overflow");
    assert_eq!(rejected.kind, ContextErrorKind::CannotFit);
    assert_eq!(history.active_ids(), before_ids.as_slice());
    assert_eq!(history.revision(), before_revision);
    assert_eq!(history.ledger().len(), before_ledger_len);

    let accepted = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, actual_tokens),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("the exact assembled limit accepts the candidate");
    assert_eq!(accepted.summary_input_tokens, actual_tokens);
    assert_eq!(accepted.summary_request().model_input(), assembled);
}

/// The summary input budget selects the span, so tightening it selects a
/// strictly smaller one. This is the mechanism the replanning loop above
/// relies on, measured with the real frozen estimator rather than scripted
/// weights.
#[test]
fn a_tighter_summary_input_limit_selects_a_smaller_span() {
    let engine = engine(1_000_000, 0, 0, Arc::new(DefaultTokenEstimator));
    let history = state(vec![
        user("u1", &"alpha ".repeat(64)),
        user("u2", &"bravo ".repeat(64)),
        user("u3", &"charlie ".repeat(64)),
        user("u4", &"delta ".repeat(64)),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let wide = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("an unconstrained summary budget retires the whole run");
    assert_eq!(wide.span.end, MessageId::new("u4"));

    let narrow = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, wide.summary_input_tokens - 1),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("a tighter summary budget still admits a shorter span");
    assert!(
        narrow.retired.len() < wide.retired.len(),
        "a tighter summary budget must retire fewer messages: {} vs {}",
        narrow.retired.len(),
        wide.retired.len()
    );
    assert!(narrow.summary_input_tokens < wide.summary_input_tokens);
}

/// A measured estimate correction scales the budget it is applied to by the
/// observed ratio, and never to zero.
#[test]
fn an_estimate_correction_scales_budgets_by_the_observed_ratio() {
    let correction =
        rustx::context::EstimateCorrection::new(80_000, 100_000).expect("a real correction");
    assert_eq!(correction.apply(50_000), 40_000);
    assert_eq!(correction.apply(1), 1, "a corrected budget is never zero");
    assert_eq!(
        rustx::context::EstimateCorrection::UNQUANTIFIED.apply(4_000),
        3_000
    );
    assert_eq!(
        rustx::context::EstimateCorrection::new(0, 10),
        None,
        "a zero estimate carries no measurable error"
    );
    assert_eq!(
        rustx::context::EstimateCorrection::new(10, 10),
        None,
        "an observation that matches the estimate is not a correction"
    );
}

/// The correction constrains the request it was measured on, and no other.
///
/// An [`EstimateCorrection`] is the ratio between what this runtime
/// estimated for one *primary* request and what the provider counted for
/// that same request. It is a fact about that request, not a calibration of
/// a tokenizer: the deviation can come from the provider continuation, the
/// tool schemas, the effective system prompt, or request-specific fixed
/// overhead. The summary request carries none of those — no tools, no Agent
/// Status, no Skill catalog, no continuation — so the ratio is not evidence
/// about it even when both requests go to the same model. A stored
/// continuation alone can put a hundred thousand provider-counted tokens
/// behind the primary request that the summary request will never send.
///
/// So the corrected plan below still admits the span the uncorrected plan
/// admitted. A summary request that is genuinely too large is rejected by
/// the summary model, and the bounded shrink loop replans against that
/// measurement instead of a borrowed one.
#[test]
fn an_estimate_correction_never_crosses_into_the_summary_request() {
    let engine = engine(1_000_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![user("u1", ""), user("u2", ""), user("u3", "")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    // The assembled summary request weighs ten tokens under this estimator,
    // so a budget of thirty is comfortable. A four-to-one correction
    // measured on the primary request would compress it to seven and reject
    // the span — if it were allowed to travel there.
    let budgets = CompactionBudgets::new(0, 0, 30);
    let uncorrected = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            budgets,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("the summary budget admits the span");
    let corrected = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            budgets,
            &rustx::context::CompactionConstraints {
                estimate_correction: rustx::context::EstimateCorrection::new(1, 4),
                ..Default::default()
            },
        )
        .expect("a primary-request correction never shrinks the summary budget");
    assert_eq!(
        corrected.summary_input_tokens, uncorrected.summary_input_tokens,
        "the summary request is planned against its own budget"
    );
    assert!(corrected.summary_input_tokens <= 30);
}

/// Repeated compaction operates from the **current** Surface and never
/// rediscovers retired Ledger history.
///
/// ```text
/// ledger:  A B C D            surface: A B C D
/// first:   A B C D S1         surface: S1 D
/// grow:    A B C D S1 E F     surface: S1 D E F
/// second:  A B C D S1 E F S2  surface: S2 F
/// ```
#[test]
fn repeated_compaction_never_resurrects_retired_history() {
    let engine = engine(10_000, 0, 0, weighted(10, 10, 0));
    let budgets = CompactionBudgets::new(0, 0, 1_000_000);
    let mut history = state(vec![
        user("A", ""),
        user("B", ""),
        user("C", ""),
        user("D", ""),
    ]);

    // First compaction: A B C -> S1, D retained.
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let first = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            budgets,
            &rustx::context::CompactionConstraints {
                must_cover_through: None,
                fresh_inbound: None,
                ..Default::default()
            },
        )
        .expect("first plan");
    // Force the documented A B C -> S1 D shape by naming the span
    // explicitly; the planner's own choice is asserted separately.
    let _ = first;
    let summary1 = UserMessageBlock {
        id: summary_id(1),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "S1".to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary(CompactionSummaryMetadata::empty()),
        timestamp: None,
    };
    let commit = history
        .prepare_compaction(
            summary1,
            SurfaceSpan::new(MessageId::new("A"), MessageId::new("C")),
        )
        .expect("prepare first");
    let record1 = history.commit_compaction(commit).expect("commit first");
    assert_eq!(record1.generation, 1);
    assert_eq!(
        active_ids(&history),
        vec![summary_id(1).as_str().to_owned(), "D".to_owned()]
    );
    assert_eq!(
        ledger_ids(&history),
        vec![
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "D".to_owned(),
            summary_id(1).as_str().to_owned(),
        ]
    );

    // The conversation grows.
    history.commit(user("E", "")).expect("commit E");
    history.commit(user("F", "")).expect("commit F");
    assert_eq!(
        active_ids(&history),
        vec![
            summary_id(1).as_str().to_owned(),
            "D".to_owned(),
            "E".to_owned(),
            "F".to_owned(),
        ]
    );

    // Second compaction: the plan is derived from the current Surface, so
    // its span starts at the active S1 — never at the retired A.
    let projection2 = engine
        .build_projection(&history, &[], None, "")
        .expect("second projection");
    let second = engine
        .plan_compaction(
            &history,
            &projection2,
            &[],
            budgets,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("second plan");
    assert_eq!(second.span.start, summary_id(1));
    let retired_ids: Vec<String> = second.retired.iter().map(message_id_of).collect();
    assert!(
        !retired_ids
            .iter()
            .any(|id| matches!(id.as_str(), "A" | "B" | "C")),
        "the second compaction must not rediscover retired ledger history, got {retired_ids:?}"
    );
    // The still-active previous summary is simply one canonical message of
    // the selected span; there is no separate previous-summary channel.
    assert!(
        second
            .retired
            .iter()
            .any(|message| matches!(message, MessageBlock::User(user)
                if user.kind.is_compaction_summary())),
        "the previous summary is an ordinary canonical message of the span"
    );

    let summary2 = UserMessageBlock {
        id: summary_id(2),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "S2".to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary(CompactionSummaryMetadata::empty()),
        timestamp: None,
    };
    let commit2 = history
        .prepare_compaction(
            summary2,
            SurfaceSpan::new(summary_id(1), MessageId::new("E")),
        )
        .expect("prepare second");
    let record2 = history.commit_compaction(commit2).expect("commit second");
    assert_eq!(record2.generation, 2);
    assert_eq!(
        active_ids(&history),
        vec![summary_id(2).as_str().to_owned(), "F".to_owned()]
    );
    assert_eq!(
        ledger_ids(&history),
        vec![
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "D".to_owned(),
            summary_id(1).as_str().to_owned(),
            "E".to_owned(),
            "F".to_owned(),
            summary_id(2).as_str().to_owned(),
        ],
        "every committed fact survives both compactions"
    );

    // Historical reconstruction is exact and stable.
    assert_eq!(
        history
            .reconstruct(record1.surface_revision)
            .expect("reconstruct generation 1"),
        vec![summary_id(1), MessageId::new("D")]
    );
    assert_eq!(
        history
            .reconstruct(SurfaceRevision::new(4))
            .expect("reconstruct the pre-compaction surface"),
        vec![
            MessageId::new("A"),
            MessageId::new("B"),
            MessageId::new("C"),
            MessageId::new("D"),
        ]
    );
    // The surface operation log carries only the minimal vocabulary.
    assert_eq!(
        history
            .surface()
            .ops()
            .iter()
            .filter(|op| matches!(op, SurfaceOp::Replace { .. }))
            .count(),
        2
    );
}

/// The keyed Ledger reads one full projection + plan + prepare cycle needs
/// over a conversation with `retired` retired messages and five active ones.
///
/// The helper asserts the hard invariant on the way: the cycle performs zero
/// full-Ledger enumerations.
fn finite_reads_for(
    retired: usize,
    engine: &ContextEngine,
    budgets: CompactionBudgets,
) -> (u64, u64, u64, u64) {
    let mut history = state(
        (0..retired + 4)
            .map(|index| user(&format!("m{index}"), ""))
            .collect(),
    );
    // Retire everything but the final four messages.
    let summary = UserMessageBlock {
        id: summary_id(1),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "S".to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary(CompactionSummaryMetadata::empty()),
        timestamp: None,
    };
    let commit = history
        .prepare_compaction(
            summary,
            SurfaceSpan::new(
                MessageId::new("m0"),
                MessageId::new(format!("m{}", retired - 1)),
            ),
        )
        .expect("prepare");
    history.commit_compaction(commit).expect("commit");
    assert_eq!(history.active_ids().len(), 5);

    // Only from here is the instrumentation meaningful.
    history.ledger_access().reset();
    history.surface_access().reset();
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    assert_eq!(projection.messages.len(), 5);
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            budgets,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let (_, _) = engine
        .prepare_compaction(&history, &conversation(), &plan, "S2", &[])
        .expect("prepare the semantic commit");
    assert_eq!(
        history.ledger_access().enumerations(),
        0,
        "normal projection and compaction must never enumerate the ledger"
    );
    (
        history.ledger_access().keyed_reads(),
        history.surface_access().current_head_reads(),
        history.surface_access().history_enumerations(),
        history.surface_access().history_steps(),
    )
}

/// Normal current-Surface projection, planning, and preparation never
/// enumerate the Message Ledger or Surface history and never depend on
/// retired-history size.
///
/// The proof is a deterministic instrumentation counter, not a memory
/// measurement: `LedgerAccess::enumerations` and Surface historical reads
/// must stay at zero, while keyed/current-head work is a function of the
/// active Surface alone.
#[test]
fn normal_compaction_reads_only_the_current_surface() {
    let engine = engine(10_000_000, 0, 0, weighted(10, 10, 0));
    let budgets = CompactionBudgets::new(0, 0, 1_000_000);

    let small = finite_reads_for(20, &engine, budgets);
    let large = finite_reads_for(2_000, &engine, budgets);
    assert_eq!(
        small, large,
        "the read cost is a function of the active surface alone, not of retired history"
    );
}

// ---------------------------------------------------------------------------
// The compaction semantic commit
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The compaction semantic commit
// ---------------------------------------------------------------------------

/// The first compaction commits exactly one canonical runtime summary and
/// exactly one Surface replacement, with correct token provenance and an
/// untouched Message Ledger prefix.
#[test]
fn first_compaction_commits_one_summary_and_one_replacement() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.estimated_before.input_tokens, 310);
    assert_eq!(
        plan.estimated_before.source,
        TokenMeasurementSource::Estimated
    );
    let mut history = history;
    let before_revision = history.revision();
    let ledger_before = ledger_ids(&history);
    let (commit, rebuilt) = engine
        .prepare_compaction(&history, &conversation(), &plan, "s1", &[])
        .expect("prepare the semantic commit");
    // Preparation mutates nothing.
    assert_eq!(history.revision(), before_revision);
    assert_eq!(ledger_ids(&history), ledger_before);
    assert_eq!(commit.summary().id, summary_id(1));
    assert_eq!(commit.summary().source, UserSource::Runtime);
    assert!(commit.summary().kind.is_compaction_summary());
    assert_eq!(rebuilt.estimated_input.input_tokens, 101);
    assert_eq!(rebuilt.surface_revision, before_revision.next());

    let record = history.commit_compaction(commit).expect("commit");
    assert_eq!(record.generation, 1);
    assert_eq!(record.summary_message_id, summary_id(1));
    assert_eq!(record.surface_revision, before_revision.next());
    assert_eq!(record.replaced, plan.span);
    // Exactly one ledger append and exactly one surface replacement.
    assert_eq!(
        ledger_ids(&history),
        vec![
            "u1".to_owned(),
            "a1".to_owned(),
            "t1".to_owned(),
            "u2".to_owned(),
            summary_id(1).as_str().to_owned(),
        ],
        "compaction appends one canonical fact and rewrites nothing"
    );
    assert_eq!(
        history
            .surface()
            .ops()
            .iter()
            .filter(|op| matches!(op, SurfaceOp::Replace { .. }))
            .count(),
        1
    );
    // The summary is active exactly at the replaced span's position.
    assert_eq!(
        active_ids(&history),
        vec![summary_id(1).as_str().to_owned(), "u2".to_owned()]
    );
    // Every covered original is still an immutable, addressable ledger fact.
    assert!(matches!(
        history.ledger().get(&MessageId::new("a1")),
        Some(MessageBlock::Assistant(_))
    ));
}

/// The second compaction selects a span of the **current** Surface: the
/// still-active previous summary is simply one canonical message inside it,
/// and already-retired originals are never re-fed.
#[test]
fn second_compaction_selects_from_the_current_surface() {
    let engine = engine(10_000, 0, 150, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        user("u2", ""),
        user("u3", ""),
        user("u4", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let first = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("first plan");
    assert_eq!(first.span.end, MessageId::new("u2"));
    let mut history = history;
    let (commit1, _) = engine
        .prepare_compaction(&history, &conversation(), &first, "s1", &[])
        .expect("prepare first");
    history.commit_compaction(commit1).expect("commit first");
    assert_eq!(
        active_ids(&history),
        vec![
            summary_id(1).as_str().to_owned(),
            "u3".to_owned(),
            "u4".to_owned(),
        ]
    );
    history.commit(user("u5", "")).expect("commit u5");
    history.commit(user("u6", "")).expect("commit u6");

    let projection2 = engine
        .build_projection(&history, &[], None, "")
        .expect("second projection");
    let second = engine
        .plan_compaction(
            &history,
            &projection2,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("second plan");
    let request = second.summary_request();
    let selected: Vec<String> = request.retired.iter().map(newly_retired_id).collect();
    // Retired originals of the first compaction are never re-fed.
    for retired in ["u1", "u2"] {
        assert!(
            !selected.contains(&retired.to_owned()),
            "retired ledger history must never be rediscovered, got {selected:?}"
        );
    }
    assert_eq!(selected[0], summary_id(1).as_str());
    let (commit2, _) = engine
        .prepare_compaction(&history, &conversation(), &second, "s2", &[])
        .expect("prepare second");
    let record2 = history.commit_compaction(commit2).expect("commit second");
    assert_eq!(record2.generation, 2);
    assert_eq!(record2.summary_message_id, summary_id(2));
}

/// A summary at least as large as the replaced context makes no progress:
/// no canonical summary, no Surface rewrite, explicit error.
#[test]
fn no_progress_compaction_is_rejected() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![user("u1", "")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // A summary text of 400 bytes estimates 101 tokens >= the 100 before.
    let error = engine
        .prepare_compaction(&history, &conversation(), &plan, &"x".repeat(400), &[])
        .expect_err("no progress");
    assert_eq!(error.kind, ContextErrorKind::NoProgress);
    assert_eq!(history.revision(), SurfaceRevision::new(1));
    assert_eq!(history.ledger().len(), 1, "nothing was committed");
}

/// The anti-loop progress rule never compares a provider-reported
/// before-count against an estimated after-count: both sides of the
/// comparison are deterministic estimates of the actual projection content.
/// A provider-reported number far above the deterministic estimate must not
/// mask an estimate that grew.
#[test]
fn progress_rule_rejects_growth_even_when_provider_reported_before_is_larger() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![user("u1", ""), assistant("a1", vec![text_block("x")])]);
    // Provider-reported before = 1000; the deterministic estimate of the
    // same projection is 20.
    let plain_projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: plain_projection.fingerprint(),
        input_tokens: 1_000,
        anchor: None,
    };
    let projection = engine
        .build_projection(&history, &[], Some(&observed), "")
        .expect("provider-reported projection");
    assert_eq!(
        projection.estimated_input.source,
        TokenMeasurementSource::ProviderReported
    );
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(
        plan.estimated_before.input_tokens, 1_000,
        "the provider-reported measurement is preserved as metadata"
    );
    assert_eq!(
        plan.estimated_before_tokens, 20,
        "the progress comparison uses the deterministic estimate"
    );
    // The after estimate (31) grew relative to the deterministic before
    // (20): rejected, even though it is far below the provider-reported 1000.
    let error = engine
        .prepare_compaction(&history, &conversation(), &plan, &"x".repeat(120), &[])
        .expect_err("no progress");
    assert_eq!(error.kind, ContextErrorKind::NoProgress);
}

/// The reverse direction: a provider-reported before-count below the
/// deterministic estimate must not reject a compaction whose estimated
/// after-count decreased relative to the deterministic before.
#[test]
fn progress_rule_accepts_decrease_even_when_provider_reported_before_is_smaller() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![
        user("u1", ""),
        user("u2", ""),
        user("u3", ""),
        user("u4", ""),
        user("u5", ""),
        user("u6", ""),
    ]);
    // Provider-reported before = 50; the deterministic estimate of the same
    // projection is 60.
    let plain_projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: plain_projection.fingerprint(),
        input_tokens: 50,
        anchor: None,
    };
    let projection = engine
        .build_projection(&history, &[], Some(&observed), "")
        .expect("provider-reported projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.estimated_before_tokens, 60);
    // The after estimate (50) decreased from the deterministic before (60)
    // but is above the provider-reported 50-boundary of this test's before
    // measurement: progress must be accepted. A 200-byte summary weighs
    // exactly 50 tokens under the corrected ceiling division.
    let mut history = history;
    let (commit, rebuilt) = engine
        .prepare_compaction(&history, &conversation(), &plan, &"x".repeat(200), &[])
        .expect("progress accepted");
    let record = history.commit_compaction(commit).expect("commit");
    assert_eq!(record.generation, 1);
    assert_eq!(
        plan.estimated_before,
        TokenMeasurement {
            input_tokens: 50,
            source: TokenMeasurementSource::ProviderReported,
        },
        "the provider-reported measurement is preserved as plan metadata"
    );
    assert_eq!(rebuilt.estimated_input.input_tokens, 50);
}

/// Empty and whitespace-only summaries are rejected at the application
/// boundary: no summarizer can erase history through an empty summary.
#[test]
fn empty_and_whitespace_summaries_are_rejected() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![user("u1", ""), user("u2", "")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    for bad in ["", "   ", "\n\t "] {
        let error = engine
            .prepare_compaction(&history, &conversation(), &plan, bad, &[])
            .expect_err("empty summary must be rejected");
        assert_eq!(error.kind, ContextErrorKind::SummaryFailed);
    }
    assert_eq!(history.revision(), SurfaceRevision::new(2));
}

// ---------------------------------------------------------------------------
// Continuation constraint
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Continuation constraint
// ---------------------------------------------------------------------------

/// The continuation constraint retires the continuation-owning turn
/// completely; the owning Assistant message is never split.
#[test]
fn continuation_constraint_covers_the_owning_turn_completely() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
                ..Default::default()
            },
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("t1"));
    // The continuation-owning Assistant message and its complete tool-result
    // portion are both retired into the summary input.
    let retired_ids: Vec<String> = plan.retired.iter().map(newly_retired_id).collect();
    assert!(retired_ids.contains(&"a1".to_owned()));
    assert!(retired_ids.contains(&"t1".to_owned()));
}

/// A continuation-owning oversized turn is retired whole, never split.
#[test]
fn continuation_owner_is_never_split() {
    let engine = engine(60, 0, 5, weighted(10, 10, 10));
    let history = state(vec![
        user("u1", ""),
        assistant(
            "a1",
            vec![
                text_block("intro"),
                call_block("c1"),
                text_block("middle"),
                call_block("c2"),
                text_block("outro"),
            ],
        ),
        tool_message("t1", "c1"),
        tool_message("t2", "c2"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    // Without the constraint this turn would split (see the split test);
    // with the constraint it must be retired whole.
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
                ..Default::default()
            },
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("t2"));
}

// ---------------------------------------------------------------------------
// Agent loop integration: proactive compaction
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Provider isolation
// ---------------------------------------------------------------------------

/// Invalidating the incompatible opaque provider continuation has exactly
/// one ownership path.
///
/// A successful incompatible Surface rewrite must discard the continuation
/// exactly once, immediately after the semantic commit. The M4 loop cleared
/// it from two caller sites as well; this regression keeps that duplicate
/// from returning.
#[test]
fn continuation_invalidation_has_exactly_one_ownership_path() {
    let source = std::fs::read_to_string("src/agent/execution.rs").expect("read the agent loop");
    let body = source
        .split_once("#[cfg(test)]\nmod tests {")
        .map_or(source.as_str(), |(body, _)| body);
    assert_eq!(
        body.matches("self.pending_continuation = None;").count(),
        1,
        "the opaque provider continuation must be invalidated from exactly one place"
    );
    assert_eq!(
        body.matches("self.continuation_owner = None;").count(),
        2,
        "the continuation owner is set from the turn assembly and cleared once \
         by the post-surface-rewrite ownership path"
    );
}

/// `src/context/` is source-level isolated from provider SDK/wire
/// dependencies: no provider-private module or crate leaks into the context
/// plane.
#[test]
fn context_sources_contain_no_provider_dependencies() {
    let banned = [
        "async_openai",
        "reqwest",
        "adapter::openai",
        "adapter::anthropic",
        "OpenAiResponsesAdapter",
        "OpenAiChatCompletionsAdapter",
        "AnthropicMessagesAdapter",
        "eventsource_stream",
    ];
    let mut files = std::fs::read_dir("src/context")
        .expect("context directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect::<Vec<_>>();
    files.sort();
    assert!(!files.is_empty(), "context sources must exist");
    for file in files {
        let source = std::fs::read_to_string(&file).expect("read source");
        for pattern in banned {
            assert!(
                !source.contains(pattern),
                "{} contains provider-private dependency {:?}",
                file.display(),
                pattern
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------
