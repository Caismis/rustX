//! The FND-06 process-death boundary matrix.
//!
//! Every test below is one row of `docs/process-death-conformance.md`:
//!
//! ```text
//! durable facts before kill -> allowed after reopen -> forbidden -> recovery action
//! ```
//!
//! The kill is always a real `SIGKILL` of a real child process that is
//! provably frozen — either inside an instrumented durable transition, or in a
//! control rendezvous where it is executing nothing. Nothing here sleeps to
//! reach a state, polls for one, or infers a race from log ordering.

use crate::durable::{TranscriptEntry, TranscriptItem};
use crate::events::types::RuntimeEvent;
use crate::message::types::{AssistantContentBlock, InboundKind, MessageBlock};
use crate::publication::PublicationAuditKind;
use crate::runtime::recovery::{AttemptRecoveryClass, ResumeDisposition};
use crate::tools::types::ToolExecutionStatus;

use super::child;
use super::harness::{Durable, Lab};

// ---------------------------------------------------------------------------
// Shared assertions
// ---------------------------------------------------------------------------

/// Whether the durable authority holds the provider outcome P.
fn has_p(durable: &Durable) -> bool {
    durable.has_event(|event| matches!(event, RuntimeEvent::ModelRequestCompleted { .. }))
}

/// Whether the durable authority holds the canonical Assistant acceptance C.
fn has_c(durable: &Durable) -> bool {
    durable.has_event(|event| matches!(event, RuntimeEvent::AssistantMessageCommitted { .. }))
}

/// Whether any durable tool execution ever started.
fn has_tool_start(durable: &Durable) -> bool {
    durable.has_event(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
}

/// The canonical model-visible message shapes, in order.
fn shapes(messages: &[MessageBlock]) -> Vec<&'static str> {
    messages
        .iter()
        .map(|message| match message {
            MessageBlock::User(user) => match user.kind {
                InboundKind::CompactionSummary => "summary",
                InboundKind::Context(_) => "context",
                InboundKind::Message => "user",
            },
            MessageBlock::Assistant(assistant) => {
                if assistant
                    .content
                    .iter()
                    .any(|block| matches!(block, AssistantContentBlock::ToolCall(_)))
                {
                    "assistant-call"
                } else {
                    "assistant"
                }
            }
            MessageBlock::Tool(_) => "tool",
        })
        .collect()
}

/// The durable `P < U < C` implication, re-read from the reopened authority.
///
/// The store enforces `C => U => P` on the way in; this re-proves it on the way
/// out, after a real `SIGKILL` at an arbitrary boundary.
fn assert_publication_implication(durable: &Durable) {
    let unsettled = durable.unsettled_publications();
    if has_c(durable) {
        assert!(has_p(durable), "a canonical Assistant exists without P");
        assert!(
            unsettled.is_empty(),
            "a canonical Assistant leaves no unsettled publication stream: {unsettled:?}"
        );
    }
    for record in &unsettled {
        if record.reached_publication_terminal() {
            assert!(has_p(durable), "a publication terminal exists without P");
        }
    }
}

/// The publication audits the reopened transcript exposes, in order.
fn audit_kinds(entries: &[TranscriptEntry]) -> Vec<PublicationAuditKind> {
    entries
        .iter()
        .filter_map(|entry| match &entry.item {
            TranscriptItem::PublicationAudit { audit } => Some(audit.kind),
            _ => None,
        })
        .collect()
}

/// The rendered content of every publication audit in the reopened transcript.
fn audit_text(entries: &[TranscriptEntry]) -> String {
    entries
        .iter()
        .filter_map(|entry| match &entry.item {
            TranscriptItem::PublicationAudit { audit } => Some(format!("{:?}", audit.content)),
            _ => None,
        })
        .collect()
}

/// The canonical tool results, by call id.
fn tool_results(messages: &[MessageBlock]) -> Vec<(String, ToolExecutionStatus)> {
    messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => {
                Some((tool.tool_call_id.to_string(), tool.result.status.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The exact rendered System authority of one persisted request.
fn system_prompt(durable: &Durable, index: usize) -> String {
    durable
        .request_snapshots()
        .get(index)
        .unwrap_or_else(|| panic!("request snapshot {index} exists"))
        .effective_system_prompt
        .clone()
}

/// The model-facing tool names of one persisted request.
fn tool_names(durable: &Durable, index: usize) -> Vec<String> {
    durable
        .request_snapshots()
        .get(index)
        .unwrap_or_else(|| panic!("request snapshot {index} exists"))
        .tool_definitions
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Canonical history atomicity
// ---------------------------------------------------------------------------

/// Death inside the inbound acceptance transaction leaves no half-accepted
/// message: nothing is pending, nothing is canonical, and the conversation
/// reopens with no work at all.
#[test]
fn inbound_acceptance_is_atomic() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::TEXT_TURN, Some("before:accept_inbound"));
    process.wait_reached("before:accept_inbound");
    process.sigkill();

    let durable = lab.durable();
    assert!(durable.canonical().is_empty(), "nothing became canonical");
    assert!(
        durable.store().load_pending().expect("pending").is_empty(),
        "nothing became pending"
    );
    let report = durable.recover();
    assert_eq!(report.attempt_class(), &AttemptRecoveryClass::NotStarted);
    assert_eq!(report.pending_inbound(), 0);
    assert_eq!(report.resume(), ResumeDisposition::PendingInboundOnly);
}

/// Once the acceptance transaction committed, the message is durably pending
/// and nothing else: acceptance is not adoption.
#[test]
fn accepted_inbound_survives_as_pending_only() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::TEXT_TURN, Some("after:accept_inbound"));
    process.wait_reached("after:accept_inbound");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable.store().load_pending().expect("pending").len(),
        1,
        "the accepted message is durably pending"
    );
    assert!(durable.canonical().is_empty(), "nothing became canonical");
    let report = durable.recover();
    assert_eq!(report.attempt_class(), &AttemptRecoveryClass::NotStarted);
    assert_eq!(report.pending_inbound(), 1);
    assert_eq!(report.resume(), ResumeDisposition::PendingInboundOnly);
}

/// Accepted inbound that a process never activated to adopt stays exactly one
/// pending item, whatever happens to that process.
#[test]
fn accepted_inbound_is_never_adopted_by_a_dead_process() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::INBOUND_ONLY, None);
    process.wait_note_prefixed("accepted:");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(durable.store().load_pending().expect("pending").len(), 1);
    assert!(durable.canonical().is_empty());
    assert_eq!(durable.recover().pending_inbound(), 1);
}

/// Death inside the adoption transaction never produces half a canonical
/// adoption: the message is still pending and the Surface is untouched.
#[test]
fn pending_adoption_is_atomic() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::TEXT_TURN, Some("before:adopt_pending_batch"));
    process.wait_reached("before:adopt_pending_batch");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(durable.store().load_pending().expect("pending").len(), 1);
    assert!(durable.canonical().is_empty());
    assert_eq!(durable.recover().pending_inbound(), 1);
}

/// After adoption commits, the message is canonical and no longer pending —
/// never both, never neither.
#[test]
fn adopted_inbound_is_canonical_and_no_longer_pending() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::TEXT_TURN, Some("after:adopt_pending_batch"));
    process.wait_reached("after:adopt_pending_batch");
    process.sigkill();

    let durable = lab.durable();
    assert!(durable.store().load_pending().expect("pending").is_empty());
    assert_eq!(shapes(&durable.canonical()), vec!["user"]);
    let report = durable.recover();
    assert_eq!(report.pending_inbound(), 0);
    // Adoption commits before the attempt publishes its start fact, so this
    // window leaves an adopted turn with no attempt evidence at all. Nothing
    // external can have happened, so the canonical turn continues through one
    // new attempt instead of being stranded.
    assert_eq!(report.attempt_class(), &AttemptRecoveryClass::NotStarted);
    assert_eq!(report.resume(), ResumeDisposition::ContinueAdoptedTurn);
}

/// The same window one turn later, in an ordinary multi-turn conversation.
///
/// Nothing about the canonical Surface distinguishes this trailing human
/// message from the first turn's already-answered one, and the Event Journal
/// now holds a complete settled attempt, so canonical shape plus "did any
/// attempt ever exist" cannot tell the two apart. Only the durable answer
/// obligation the adoption transaction committed can.
#[test]
fn a_turn_adopted_after_a_settled_attempt_continues() {
    let lab = Lab::new();
    // The second occurrence of the boundary is the second turn's adoption.
    let mut process = lab.spawn_nth(child::SECOND_TURN, Some("after:adopt_pending_batch"), 2);
    process.wait_note("first-attempt-settled");
    process.resume();
    process.wait_reached("after:adopt_pending_batch");
    process.sigkill();

    let durable = lab.durable();
    assert!(
        durable.store().load_pending().expect("pending").is_empty(),
        "the second message is adopted, not pending"
    );
    assert_eq!(
        shapes(&durable.canonical()),
        vec!["user", "context", "assistant", "user"],
        "the first turn is answered and the second turn is canonical"
    );
    let report = durable.recover();
    // Every attempt in durable authority is terminal: the class carries no
    // information about the unanswered turn at all.
    assert_eq!(
        report.attempt_class(),
        &AttemptRecoveryClass::AlreadyTerminal
    );
    assert_eq!(
        report.resume(),
        ResumeDisposition::ContinueAdoptedTurn,
        "the turn adopted after the settled attempt is still owed an answer"
    );
}

/// A turn drained into a **live** attempt at a safe boundary, killed before
/// the model request that would carry it.
///
/// The attempt is still durably non-terminal and its request plane reports the
/// *previous* request's known outcome, so the attempt classification alone
/// answers "an external outcome is known, nothing to continue". The obligation
/// the drain's adoption transaction committed is what keeps the newly adopted
/// message answerable.
#[test]
fn a_turn_drained_into_a_live_attempt_continues() {
    let lab = Lab::new();
    // Occurrence 1 is the admission adoption of the first message; occurrence
    // 2 is the running attempt's safe-boundary drain of the second.
    let mut process = lab.spawn_nth(
        child::STREAMING_INBOUND,
        Some("after:adopt_pending_batch"),
        2,
    );
    process.wait_note("second-inbound-accepted");
    process.resume();
    process.wait_reached("after:adopt_pending_batch");
    process.sigkill();

    let durable = lab.durable();
    assert!(
        durable.store().load_pending().expect("pending").is_empty(),
        "the drained message left the Pending Inbound Inbox"
    );
    assert_eq!(
        shapes(&durable.canonical()),
        vec!["user", "context", "assistant", "user"],
        "the drained message is canonical behind the answered first turn"
    );
    assert!(
        has_p(&durable),
        "the previous request's provider outcome is durably known"
    );
    let report = durable.recover();
    assert!(
        matches!(
            report.attempt_class(),
            AttemptRecoveryClass::ExternalOutcomeKnown { .. }
        ),
        "the interrupted attempt carries a known external outcome, not a fresh one: {:?}",
        report.attempt_class()
    );
    assert_eq!(
        report.resume(),
        ResumeDisposition::ContinueAdoptedTurn,
        "the turn drained into the dead attempt is still owed an answer"
    );
}

/// The obligation is consumed by the request start that carries the turn to
/// the provider, so an answered conversation is never re-answered on reopen.
#[test]
fn an_answered_turn_is_not_continued_after_reopen() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::SECOND_TURN, None);
    process.wait_note("first-attempt-settled");
    process.sigkill();

    let durable = lab.durable();
    let report = durable.recover();
    assert_eq!(
        report.attempt_class(),
        &AttemptRecoveryClass::AlreadyTerminal
    );
    assert_eq!(
        report.resume(),
        ResumeDisposition::PendingInboundOnly,
        "a settled conversation owes nothing and starts nothing"
    );
}

/// Death immediately before the canonical Assistant transaction leaves no
/// Assistant message at all; death immediately after leaves exactly one.
#[test]
fn assistant_canonical_commit_is_atomic() {
    let before = Lab::new();
    let mut process = before.spawn(
        child::TEXT_TURN,
        Some("before:commit_canonical_publication"),
    );
    process.wait_reached("before:commit_canonical_publication");
    process.sigkill();
    let durable = before.durable();
    assert_eq!(shapes(&durable.canonical()), vec!["user", "context"]);
    assert!(!has_c(&durable));
    assert_publication_implication(&durable);

    let after = Lab::new();
    let mut process = after.spawn(child::TEXT_TURN, Some("after:commit_canonical_publication"));
    process.wait_reached("after:commit_canonical_publication");
    process.sigkill();
    let durable = after.durable();
    assert_eq!(
        shapes(&durable.canonical()),
        vec!["user", "context", "assistant"]
    );
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::AssistantMessageCommitted { .. })),
        1,
        "exactly one canonical Assistant acceptance"
    );
    assert_publication_implication(&durable);
}

/// The canonical `ToolResult` batch is one transaction: before it, the tool
/// turn has no result at all; after it, the result is canonical.
#[test]
fn tool_result_batch_commit_is_atomic() {
    let before = Lab::new();
    let mut process = before.spawn(child::TOOL_TURN, Some("before:append_canonical_batch"));
    process.wait_reached("before:append_canonical_batch");
    process.sigkill();
    let durable = before.durable();
    assert_eq!(
        shapes(&durable.canonical()),
        vec!["user", "context", "assistant-call"]
    );
    assert!(tool_results(&durable.canonical()).is_empty());

    let after = Lab::new();
    let mut process = after.spawn(child::TOOL_TURN, Some("after:append_canonical_batch"));
    process.wait_reached("after:append_canonical_batch");
    process.sigkill();
    let durable = after.durable();
    assert_eq!(
        shapes(&durable.canonical()),
        vec!["user", "context", "assistant-call", "tool"]
    );
    assert!(
        durable
            .recover()
            .reconciliation()
            .repaired_tool_results
            .is_empty(),
        "a committed batch needs no repair"
    );
}

/// The detached background terminal enters the conversation lineage through
/// one durable transaction: before it, no terminal message exists, and
/// recovery — not the dead process — publishes it exactly once.
#[test]
fn background_terminal_publication_is_atomic() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::BACKGROUND_TOOL,
        Some("after:event:background_execution_committed"),
    );
    process.wait_reached("after:event:background_execution_committed");
    process.sigkill();

    let durable = lab.durable();
    assert!(
        !durable
            .has_event(|event| matches!(event, RuntimeEvent::BackgroundTerminalPublished { .. })),
        "no half-published lineage terminal exists"
    );
    let report = durable.recover();
    assert_eq!(report.background_classes().len(), 1);
    assert_eq!(report.reconciliation().background_terminals.len(), 1);
    assert_eq!(
        durable.count_events(|event| matches!(
            event,
            RuntimeEvent::BackgroundTerminalPublished { .. }
        )),
        1
    );
}

// ---------------------------------------------------------------------------
// 2. Provider / publication / conversation separation
// ---------------------------------------------------------------------------

/// Frames released before the provider outcome do not make the provider
/// outcome durable: killed before P, the stream is Incomplete.
#[test]
fn kill_before_p_with_released_frames_is_incomplete() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::TEXT_TURN,
        Some("before:event:model_request_completed"),
    );
    process.wait_reached("before:event:model_request_completed");
    process.sigkill();

    let durable = lab.durable();
    assert!(!has_p(&durable), "P never committed");
    assert!(!has_c(&durable), "C never committed");
    let streams = durable.unsettled_publications();
    assert_eq!(streams.len(), 1, "one stream is open with staged frames");
    assert!(
        !streams[0].reached_publication_terminal(),
        "U never committed"
    );
    assert_publication_implication(&durable);

    let report = durable.recover();
    assert_eq!(report.publication_classes().len(), 1);
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Incomplete
    );
    assert_eq!(
        audit_kinds(&durable.transcript()),
        vec![PublicationAuditKind::Incomplete]
    );
    assert!(!has_c(&durable), "recovery never fabricates the Assistant");
}

/// A structural `assembler.finish()` rejection after frames were released is
/// Incomplete and produces no provider outcome at all.
#[test]
fn structural_finish_failure_is_incomplete_without_p() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::STRUCTURAL_FAILURE,
        Some("before:terminalize_publication_audit"),
    );
    process.wait_reached("before:terminalize_publication_audit");
    process.sigkill();

    let durable = lab.durable();
    assert!(!has_p(&durable), "the structural rejection precedes P");
    let streams = durable.unsettled_publications();
    assert_eq!(streams.len(), 1);
    assert!(!streams[0].reached_publication_terminal());
    assert_publication_implication(&durable);

    let report = durable.recover();
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Incomplete
    );
    assert!(
        !has_tool_start(&durable),
        "an incomplete proposal never became an execution"
    );
}

/// P committed and U not: the provider finished, but rustX never committed the
/// output for release, so the stream is Incomplete — Incomplete is defined on
/// the publication boundary, never the provider boundary.
#[test]
fn p_committed_and_u_missing_is_incomplete() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::TEXT_TURN,
        Some("after:event:model_request_completed"),
    );
    process.wait_reached("after:event:model_request_completed");
    process.sigkill();

    let durable = lab.durable();
    assert!(has_p(&durable));
    assert!(!has_c(&durable));
    let streams = durable.unsettled_publications();
    assert_eq!(streams.len(), 1);
    assert!(!streams[0].reached_publication_terminal());
    assert_publication_implication(&durable);

    let report = durable.recover();
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Incomplete
    );
    assert!(
        matches!(
            report.attempt_class(),
            AttemptRecoveryClass::ExternalOutcomeKnown { .. }
        ),
        "a durable provider outcome is known, never indeterminate: {:?}",
        report.attempt_class()
    );
}

/// U committed and C not: the released output was complete and was never
/// accepted as conversation history.
#[test]
fn u_committed_and_c_missing_is_unaccepted() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::TEXT_TURN, Some("after:commit_publication_terminal"));
    process.wait_reached("after:commit_publication_terminal");
    process.sigkill();

    let durable = lab.durable();
    assert!(has_p(&durable));
    assert!(!has_c(&durable));
    let streams = durable.unsettled_publications();
    assert!(streams[0].reached_publication_terminal(), "U committed");
    assert_publication_implication(&durable);

    let report = durable.recover();
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Unaccepted
    );
    assert_eq!(
        audit_kinds(&durable.transcript()),
        vec![PublicationAuditKind::Unaccepted]
    );
    assert!(
        !durable
            .canonical()
            .iter()
            .any(|message| matches!(message, MessageBlock::Assistant(_))),
        "an Unaccepted audit never becomes a canonical Assistant"
    );
}

/// A turn whose whole payload fits in the terminal transaction still reaches U
/// as one atomic final frame plus terminal marker, and still never precedes P.
#[test]
fn terminal_only_publication_reaches_u_atomically() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::TERMINAL_ONLY_TURN,
        Some("after:commit_publication_terminal"),
    );
    process.wait_reached("after:commit_publication_terminal");
    process.sigkill();

    let durable = lab.durable();
    let streams = durable.unsettled_publications();
    assert_eq!(streams.len(), 1);
    assert!(streams[0].reached_publication_terminal());
    assert!(has_p(&durable), "U may never precede P");
    assert_publication_implication(&durable);
    assert_eq!(
        durable.recover().publication_classes()[0].kind,
        PublicationAuditKind::Unaccepted
    );
}

/// C committed: the Message Ledger is the authority, the stream settled
/// canonically, and no audit is created.
#[test]
fn c_committed_settles_canonically_without_an_audit() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::TEXT_TURN, Some("after:commit_canonical_publication"));
    process.wait_reached("after:commit_canonical_publication");
    process.sigkill();

    let durable = lab.durable();
    assert!(has_p(&durable) && has_c(&durable));
    assert!(
        durable.unsettled_publications().is_empty(),
        "C settles the stream"
    );
    assert_publication_implication(&durable);

    let report = durable.recover();
    assert!(
        report.publication_classes().is_empty(),
        "an accepted stream never terminalizes as an audit"
    );
    assert!(
        audit_kinds(&durable.transcript()).is_empty(),
        "no audit item exists for accepted output"
    );
}

/// No boundary of the streaming pipeline can create a durable `U` without `P`
/// or a durable `C` without `U`.
#[test]
fn no_boundary_creates_u_without_p_or_c_without_u() {
    for gate in [
        "after:open_publication_stream",
        "after:stage_publication_frames",
        "before:event:model_request_completed",
        "after:event:model_request_completed",
        "before:commit_publication_terminal",
        "after:commit_publication_terminal",
        "before:commit_canonical_publication",
        "after:commit_canonical_publication",
    ] {
        let lab = Lab::new();
        let mut process = lab.spawn(child::TEXT_TURN, Some(gate));
        process.wait_reached(gate);
        process.sigkill();

        let durable = lab.durable();
        assert_publication_implication(&durable);
        // Recovery is the second chance to violate the implication, so it is
        // re-checked after the audits terminalize.
        durable.recover();
        assert_publication_implication(&durable);
    }
}

// ---------------------------------------------------------------------------
// 3. Model-proposed tool calls never become execution through audit
// ---------------------------------------------------------------------------

/// A complete proposal whose canonical acceptance never committed settles as
/// Unaccepted and never authorizes an execution — not before the crash, and
/// not through recovery.
#[test]
fn an_unaccepted_proposal_never_becomes_an_execution() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::TOOL_TURN,
        Some("before:commit_canonical_publication"),
    );
    process.wait_reached("before:commit_canonical_publication");
    process.sigkill();

    let durable = lab.durable();
    assert!(!has_c(&durable));
    assert!(!has_tool_start(&durable), "no execution started before C");

    let report = durable.recover();
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Unaccepted
    );
    assert!(
        !has_tool_start(&durable),
        "recovery never authorizes an audited proposal"
    );
    assert!(
        tool_results(&durable.canonical()).is_empty(),
        "an audited proposal has no canonical result slot to repair"
    );
}

/// A publication audit is never conversation history: it never enters the
/// Message Ledger, and it never appears in a later request's frozen context.
#[test]
fn a_publication_audit_never_reenters_model_context() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::TEXT_TURN, Some("after:commit_publication_terminal"));
    process.wait_reached("after:commit_publication_terminal");
    process.sigkill();
    let durable = lab.durable();
    durable.recover();

    assert!(
        !audit_text(&durable.transcript()).is_empty(),
        "the Unaccepted audit body exists"
    );
    assert!(
        !durable
            .canonical()
            .iter()
            .any(|message| matches!(message, MessageBlock::Assistant(_))),
        "the audit is not canonical history"
    );

    // A reopened runtime issues a new request; that request's frozen context
    // must contain only canonical message identities.
    let mut resumed = lab.spawn(child::COLD_RESUME, None);
    resumed.resume_until("settled");
    resumed.sigkill();

    let durable = lab.durable();
    let canonical_ids: Vec<String> = durable
        .canonical()
        .iter()
        .map(|message| crate::conversation::message_id_of(message).to_string())
        .collect();
    for snapshot in durable.request_snapshots() {
        for id in &snapshot.request_context_ids {
            assert!(
                canonical_ids.contains(&id.to_string()),
                "request context {id} is not a canonical message"
            );
        }
    }
    assert!(
        audit_kinds(&durable.transcript()).contains(&PublicationAuditKind::Unaccepted),
        "the audit remains a transcript-only fact after reopen"
    );
}

// ---------------------------------------------------------------------------
// 4. Tool external-outcome recovery
// ---------------------------------------------------------------------------

/// Killed before the execution start commit, no external side effect was ever
/// authorized: the canonical result slot settles as cancelled, not as an
/// unknown outcome and never by re-running the tool.
#[test]
fn kill_before_tool_execution_start_authorizes_nothing() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::TOOL_TURN,
        Some("before:event:tool_execution_started"),
    );
    process.wait_reached("before:event:tool_execution_started");
    process.sigkill();

    let durable = lab.durable();
    assert!(!has_tool_start(&durable));
    let report = durable.recover();
    assert_eq!(
        report.reconciliation().repaired_tool_results.len(),
        1,
        "the structurally missing result slot is repaired"
    );
    let results = tool_results(&durable.canonical());
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].1, ToolExecutionStatus::Cancelled { .. }),
        "an unstarted call settles as cancelled: {results:?}"
    );
    assert!(
        !has_tool_start(&durable),
        "recovery never starts the execution"
    );
}

/// Killed after the execution start commit, the external outcome is unknown
/// and stays unknown: continuation is blocked, the repaired result is
/// `Interrupted`, and nothing is inferred from workspace state.
#[test]
fn started_tool_with_unknown_outcome_stays_unknown() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::TOOL_TURN, Some("after:event:tool_execution_started"));
    process.wait_reached("after:event:tool_execution_started");
    process.sigkill();

    let durable = lab.durable();
    assert!(has_tool_start(&durable));
    assert!(
        !durable.has_event(|event| matches!(
            event,
            RuntimeEvent::ToolExecutionCompleted { .. } | RuntimeEvent::ToolExecutionFailed { .. }
        )),
        "no outcome is durable"
    );

    let report = durable.recover();
    assert!(matches!(
        report.attempt_class(),
        AttemptRecoveryClass::IndeterminateExternalOutcome { .. }
    ));
    assert_eq!(report.resume(), ResumeDisposition::BlockedIndeterminate);
    let canonical = durable.canonical();
    let results = tool_results(&canonical);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1, ToolExecutionStatus::Interrupted);
    assert!(
        !format!("{canonical:?}").contains("R1 note body"),
        "an unknown outcome is never reconstructed from workspace state"
    );
}

/// A durably known tool outcome that never reached its canonical settlement is
/// preserved exactly, and the attempt is never described as "nothing started".
#[test]
fn known_tool_outcome_is_preserved_into_the_canonical_slot() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::TOOL_TURN,
        Some("after:event:tool_execution_completed"),
    );
    process.wait_reached("after:event:tool_execution_completed");
    process.sigkill();

    let durable = lab.durable();
    assert!(tool_results(&durable.canonical()).is_empty());
    let report = durable.recover();
    assert!(matches!(
        report.attempt_class(),
        AttemptRecoveryClass::ExternalOutcomeKnown { .. }
    ));
    let canonical = durable.canonical();
    let results = tool_results(&canonical);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1, ToolExecutionStatus::Success);
    assert!(
        format!("{canonical:?}").contains("R1 note body"),
        "the exact durable outcome is repaired into the canonical slot"
    );
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. })),
        1,
        "recovery never re-executes the tool"
    );
}

// ---------------------------------------------------------------------------
// 5. Interaction ordering
// ---------------------------------------------------------------------------

/// A process that dies holding a pending waiter leaves only the requested
/// audit: no settlement, no execution, and no recreated waiter after reopen.
#[test]
fn a_pending_interaction_leaves_a_requested_audit_and_nothing_else() {
    let lab = Lab::new();
    lab.write_runtime_config("always");
    let mut process = lab.spawn(child::TOOL_APPROVAL_PENDING, None);
    process.wait_note("interaction-pending");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::InteractionRequested { .. })),
        1
    );
    assert!(
        !durable.has_event(|event| matches!(event, RuntimeEvent::InteractionSettled { .. })),
        "an unanswered waiter never settles"
    );
    assert!(!has_tool_start(&durable), "the tool never started");

    durable.recover();
    assert!(
        !durable.has_event(|event| matches!(event, RuntimeEvent::InteractionSettled { .. })),
        "recovery never invents a settlement"
    );

    // Reopening recreates no waiter and auto-executes nothing.
    let mut resumed = lab.spawn(child::COLD_RESUME, None);
    resumed.resume_until("settled");
    resumed.sigkill();
    let durable = lab.durable();
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::InteractionRequested { .. })),
        1,
        "no pending waiter is recreated"
    );
    assert!(
        !has_tool_start(&durable),
        "no tool auto-executes after reopen"
    );
    assert!(
        durable
            .transcript()
            .iter()
            .any(|entry| matches!(entry.item, TranscriptItem::InteractionRequested { .. })),
        "the requested audit remains historical transcript evidence"
    );
}

/// An approval that settled before the crash is historical audit evidence
/// only: it never authorizes an execution in the reopened runtime, and the
/// durable order `requested < settled < started` holds.
#[test]
fn a_settled_approval_never_authorizes_a_later_execution() {
    let lab = Lab::new();
    lab.write_runtime_config("always");
    let mut process = lab.spawn(
        child::TOOL_APPROVAL,
        Some("before:event:tool_execution_started"),
    );
    process.wait_reached("before:event:tool_execution_started");
    process.sigkill();

    let durable = lab.durable();
    let requested = durable
        .sequence_of(|event| matches!(event, RuntimeEvent::InteractionRequested { .. }))
        .expect("a requested audit");
    let settled = durable
        .sequence_of(|event| matches!(event, RuntimeEvent::InteractionSettled { .. }))
        .expect("a settled audit");
    assert!(requested < settled, "requested commits before settled");
    assert!(
        !has_tool_start(&durable),
        "the settlement precedes an execution start that never committed"
    );
    durable.recover();

    let mut resumed = lab.spawn(child::COLD_RESUME, None);
    resumed.resume_until("settled");
    resumed.sigkill();
    let durable = lab.durable();
    assert!(
        !has_tool_start(&durable),
        "the historical approval authorizes nothing after reopen"
    );
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::InteractionSettled { .. })),
        1,
        "the audit stays exactly one historical fact"
    );
}

// ---------------------------------------------------------------------------
// 6. Compaction
// ---------------------------------------------------------------------------

/// Killed while the compaction summary side request is in flight, nothing of
/// the compaction is durable and the old Surface remains authoritative.
#[test]
fn kill_during_the_compaction_summary_keeps_the_old_surface() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::COMPACTION, None);
    process.wait_note("settled");
    process.resume();
    process.wait_note("summary-in-flight");
    process.sigkill();

    let durable = lab.durable();
    assert!(
        !durable.has_event(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. })),
        "no compaction committed"
    );
    assert_eq!(
        shapes(&durable.surface()),
        vec!["user", "context", "assistant"],
        "the pre-compaction Surface is still authoritative"
    );
}

/// Killed inside the Surface Replace transaction, the compaction is entirely
/// absent; killed just after it, the exact planned span is replaced, the
/// Ledger is intact, and the retained Surface order is correct.
#[test]
fn compaction_surface_replace_is_atomic() {
    let before = Lab::new();
    let mut process = before.spawn(child::COMPACTION, Some("before:commit_compaction"));
    process.wait_note("settled");
    process.resume();
    process.wait_note("summary-in-flight");
    process.resume();
    process.wait_reached("before:commit_compaction");
    process.sigkill();
    let durable = before.durable();
    assert!(!durable.has_event(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. })));
    assert_eq!(
        shapes(&durable.surface()),
        vec!["user", "context", "assistant"]
    );
    let ledger_before = shapes(&durable.canonical());

    let after = Lab::new();
    let mut process = after.spawn(child::COMPACTION, Some("after:commit_compaction"));
    process.wait_note("settled");
    process.resume();
    process.wait_note("summary-in-flight");
    process.resume();
    process.wait_reached("after:commit_compaction");
    process.sigkill();
    let durable = after.durable();
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. })),
        1,
        "exactly one compaction fact"
    );
    assert_eq!(
        shapes(&durable.surface()),
        vec!["summary"],
        "the planned span is replaced by its summary"
    );
    let ledger_after = shapes(&durable.canonical());
    assert_eq!(
        &ledger_after[..ledger_before.len()],
        &ledger_before[..],
        "the historical Ledger prefix is untouched by Surface replacement"
    );
    assert_eq!(
        ledger_after.last(),
        Some(&"summary"),
        "the summary is appended, never substituted in place"
    );
}

/// Compaction is not a resource reload boundary: the request issued after a
/// committed compaction still carries the generation the runtime loaded, even
/// though the project instructions and the Skill catalog changed on disk in
/// between.
#[test]
fn compaction_never_refreshes_resource_derived_authority() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::COMPACTION, None);
    process.wait_note("settled");
    lab.write_project_instructions("R2 project instructions.");
    lab.write_skill_frontmatter("alpha", "R2 alpha summary");
    process.resume();
    process.wait_note("summary-in-flight");
    process.resume();
    let outcome = process.wait_note_prefixed("compaction:");
    assert_eq!(outcome, "compaction:ok");
    process.wait_note("post-compaction-settled");
    process.sigkill();

    let durable = lab.durable();
    let snapshots = durable.request_snapshots();
    assert!(
        snapshots.len() >= 2,
        "one request before and one after compaction"
    );
    let first = system_prompt(&durable, 0);
    let last = system_prompt(&durable, snapshots.len() - 1);
    assert!(first.contains("R1 project instructions."));
    assert!(first.contains("R1 alpha summary"));
    assert_eq!(
        first, last,
        "compaction neither refreshed nor removed resource-derived System authority"
    );
    assert_eq!(
        snapshots[0].runtime_resource_revision,
        snapshots[snapshots.len() - 1].runtime_resource_revision,
        "compaction is not a resource reload boundary"
    );
}

// ---------------------------------------------------------------------------
// 7. Context / System / resource / lineage authority
// ---------------------------------------------------------------------------

/// Editing every resource under a live runtime changes nothing about the
/// generation that runtime already loaded — while an already-discovered
/// Skill's *body* stays ordinary file content a later native Read observes.
#[test]
fn live_external_edits_never_expose_a_new_generation() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::LIVE_RESOURCE_EDIT, None);
    process.wait_note("first-attempt-settled");
    lab.write_project_instructions("R2 project instructions.");
    lab.write_skill("alpha", "R2 alpha summary", "R2 alpha body");
    lab.write_runtime_config("always");
    process.resume();
    process.wait_note("settled");
    process.sigkill();

    let durable = lab.durable();
    let snapshots = durable.request_snapshots();
    assert_eq!(snapshots.len(), 3, "one R1 request, then two more");
    let first = system_prompt(&durable, 0);
    assert!(first.contains("R1 project instructions."));
    assert!(first.contains("R1 alpha summary"));
    for index in 1..snapshots.len() {
        assert_eq!(
            system_prompt(&durable, index),
            first,
            "request {index} still sends the R1 generation"
        );
        assert_eq!(
            tool_names(&durable, index),
            tool_names(&durable, 0),
            "request {index} still sends the R1 Tool definitions"
        );
        assert_eq!(
            snapshots[index].runtime_resource_revision, snapshots[0].runtime_resource_revision,
            "request {index} is pinned to the same resource generation"
        );
    }

    // Pi-style progressive disclosure: the catalog is frozen at R1, but the
    // Skill body a native Read returns is the current file.
    assert!(
        format!("{:?}", durable.canonical()).contains("R2 alpha body"),
        "a later native Read observes the current SKILL.md body"
    );
}

/// An explicit reload publishes the complete new generation atomically: the
/// next attempt's first request carries the new project instructions, the new
/// Skill catalog, and the new Tool definitions together — and no canonical
/// message describes the change.
#[test]
fn explicit_reload_publishes_one_complete_generation() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::RELOAD, None);
    process.wait_note("settled");
    lab.write_project_instructions("R2 project instructions.");
    lab.write_skill_frontmatter("alpha", "R2 alpha summary");
    process.resume();
    assert_eq!(process.wait_note_prefixed("reload:"), "reload:ok");
    process.wait_note("reload-done");
    process.sigkill();

    let durable = lab.durable();
    let snapshots = durable.request_snapshots();
    assert_eq!(snapshots.len(), 2);
    let before = system_prompt(&durable, 0);
    let after = system_prompt(&durable, 1);
    assert!(before.contains("R1 project instructions."));
    assert!(before.contains("R1 alpha summary"));
    assert!(after.contains("R2 project instructions."));
    assert!(after.contains("R2 alpha summary"));
    assert_ne!(
        snapshots[0].runtime_resource_revision, snapshots[1].runtime_resource_revision,
        "the reload published a new generation"
    );
    assert_eq!(
        shapes(&durable.canonical()),
        vec![
            "user",
            "context",
            "assistant",
            "user",
            "context",
            "assistant"
        ],
        "reload creates no canonical message and no synthetic diff"
    );
    // The historical request still reconstructs its own old authority.
    let reconstructed = durable
        .store()
        .reconstruct_model_request(&snapshots[0].request_id)
        .expect("reconstruct the historical request");
    assert_eq!(reconstructed.effective_system_prompt, before);
}

/// A failed reload keeps the complete previous generation in place: the next
/// attempt still sends R1, and no partial R2 authority leaks anywhere.
#[test]
fn a_failed_reload_keeps_the_previous_generation() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::RELOAD, None);
    process.wait_note("settled");
    lab.write_project_instructions("R2 project instructions.");
    std::fs::write(lab.root().join("rustx.json"), "{ not json").expect("corrupt the config");
    process.resume();
    let reload = process.wait_note_prefixed("reload:");
    assert_ne!(reload, "reload:ok", "the reload failed: {reload}");
    process.wait_note("reload-done");
    process.sigkill();

    let durable = lab.durable();
    let snapshots = durable.request_snapshots();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(
        system_prompt(&durable, 1),
        system_prompt(&durable, 0),
        "the failed reload left the complete R1 authority in place"
    );
    assert!(system_prompt(&durable, 1).contains("R1 project instructions."));
    assert_eq!(
        snapshots[0].runtime_resource_revision, snapshots[1].runtime_resource_revision,
        "no generation was published"
    );
}

/// A reload requested while an attempt owns the session is refused as busy and
/// cannot mix generations.
#[test]
fn reload_while_an_attempt_owns_the_session_is_busy() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::RELOAD_BUSY, None);
    let reload = process.wait_note_prefixed("reload:");
    assert!(
        reload.contains("Busy") && reload.contains("Attempt"),
        "an owned session refuses reload: {reload}"
    );
    process.wait_note("settled");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(durable.request_snapshots().len(), 1);
    assert!(system_prompt(&durable, 0).contains("R1 project instructions."));
}

/// Reload ownership is per-owner, not "an attempt is running".
///
/// A compaction and a pending interaction each own the session in their own
/// right, and each refuses the reload with its own reason. Proving them
/// separately is what makes the ownership rule a rule rather than a property
/// of the attempt plane: a mixed generation must be impossible under *every*
/// owner, not only the one the attempt row happens to exercise.
#[test]
fn reload_while_a_compaction_owns_the_session_is_busy() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::RELOAD_BUSY_COMPACTION, None);
    let reload = process.wait_note_prefixed("reload:");
    assert!(
        reload.contains("Busy") && reload.contains("Compaction"),
        "a compaction owns the session and refuses reload: {reload}"
    );
    process.wait_note_prefixed("compaction:");
    process.sigkill();

    let durable = lab.durable();
    // The refused reload published no generation, so every historical request
    // — the answered turn and the compaction's own summary side request —
    // still carries R1.
    for index in 0..durable.request_snapshots().len() {
        assert!(
            system_prompt(&durable, index).contains("R1 project instructions."),
            "request {index} was assembled under a mixed generation"
        );
    }
}

/// The interaction owner of the same rule.
#[test]
fn reload_while_an_interaction_owns_the_session_is_busy() {
    let lab = Lab::new();
    lab.write_runtime_config("always");
    let mut process = lab.spawn(child::RELOAD_BUSY_INTERACTION, None);
    let reload = process.wait_note_prefixed("reload:");
    assert!(
        reload.contains("Busy") && reload.contains("Interaction"),
        "a pending interaction owns the session and refuses reload: {reload}"
    );
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable.request_snapshots().len(),
        1,
        "the refused reload admitted no request under a new generation"
    );
    assert!(system_prompt(&durable, 0).contains("R1 project instructions."));
    assert!(
        !has_tool_start(&durable),
        "the pending approval authorized nothing"
    );
}

/// The current Runtime Resource Snapshot is process-local, not durable
/// recovery authority: dying around the reload build/publish boundary can never
/// be recovered as a half-published mixed generation. The reopened process
/// simply performs a normal fresh resource load.
#[test]
fn death_around_the_reload_publish_boundary_reloads_from_scratch() {
    for gate in ["reload:prepared", "reload:published"] {
        let lab = Lab::new();
        let mut process = lab.spawn(child::RELOAD, Some(gate));
        process.wait_note("settled");
        lab.write_project_instructions("R2 project instructions.");
        lab.write_skill_frontmatter("alpha", "R2 alpha summary");
        process.resume();
        process.wait_reached(gate);
        process.sigkill();

        let durable = lab.durable();
        assert_eq!(
            durable.request_snapshots().len(),
            1,
            "{gate}: no request was admitted with a half-published generation"
        );
        assert!(system_prompt(&durable, 0).contains("R1 project instructions."));

        // The reopened process loads current resources once, before admitting.
        let mut resumed = lab.spawn(child::COLD_RESUME, None);
        resumed.resume_until("settled");
        resumed.sigkill();
        let durable = lab.durable();
        assert_eq!(
            durable.request_snapshots().len(),
            2,
            "{gate}: exactly one new request"
        );
        let after = system_prompt(&durable, 1);
        assert!(
            after.contains("R2 project instructions.") && after.contains("R2 alpha summary"),
            "{gate}: the reopened runtime loaded the current generation"
        );
        assert!(
            !after.contains("R1 project instructions."),
            "{gate}: no mixed generation survived"
        );
    }
}

/// Cold resume after external edits: the reopened runtime loads current
/// resources once for its first new request, while every historical fact — the
/// Ledger, the transcript, and the old `RequestSnapshot` — keeps its exact old
/// value.
#[test]
fn cold_resume_uses_current_resources_and_preserves_history() {
    let lab = Lab::new();
    let mut first = lab.spawn(child::TEXT_TURN, None);
    first.wait_note("settled");
    first.sigkill();

    let durable = lab.durable();
    let historical_ledger = durable.canonical();
    let historical_transcript = durable.transcript();
    let historical_snapshot = durable
        .request_snapshots()
        .into_iter()
        .next()
        .expect("the R1 request snapshot");
    assert!(
        historical_snapshot
            .effective_system_prompt
            .contains("R1 project instructions.")
    );

    lab.write_project_instructions("R2 project instructions.");
    lab.write_skill("alpha", "R2 alpha summary", "R2 alpha body");

    let mut second = lab.spawn(child::COLD_RESUME_READ, None);
    let recovery = second.wait_note_prefixed("recovery:");
    assert!(
        recovery.contains("PendingInboundOnly"),
        "the settled conversation reopens with nothing outstanding: {recovery}"
    );
    second.wait_note("settled");
    second.sigkill();

    let durable = lab.durable();
    let snapshots = durable.request_snapshots();
    assert!(snapshots.len() >= 2);
    let fresh = system_prompt(&durable, 1);
    assert!(fresh.contains("R2 project instructions."));
    assert!(fresh.contains("R2 alpha summary"));

    // The old snapshot still reconstructs its exact old authority without
    // reading current disk.
    assert_eq!(
        snapshots[0].effective_system_prompt,
        historical_snapshot.effective_system_prompt
    );
    assert_eq!(
        snapshots[0].tool_definitions,
        historical_snapshot.tool_definitions
    );

    // Historical conversation state is unchanged; only new turns are appended.
    let ledger = durable.canonical();
    assert_eq!(&ledger[..historical_ledger.len()], &historical_ledger[..]);
    let transcript = durable.transcript();
    assert_eq!(
        &transcript[..historical_transcript.len()],
        &historical_transcript[..]
    );
    assert!(
        !ledger
            .iter()
            .skip(historical_ledger.len())
            .any(|message| format!("{message:?}").contains("project instructions")),
        "no synthetic R1-to-R2 replacement message is appended"
    );
    // The Skill body entered history through a real native Read, by value.
    assert!(
        format!("{ledger:?}").contains("R2 alpha body"),
        "the new Read observed the current body"
    );
}

/// A native Read of a deleted, already-discovered Skill returns the normal read
/// error, and the historical `ToolResult` that captured the old body keeps its
/// exact old value.
#[test]
fn a_deleted_skill_leaves_history_by_value_and_reads_as_an_error() {
    let lab = Lab::new();
    let mut first = lab.spawn(child::COLD_RESUME_READ, None);
    first.resume_until("settled");
    first.sigkill();
    let durable = lab.durable();
    let historical = durable.canonical();
    assert!(
        format!("{historical:?}").contains("R1 alpha summary body"),
        "the historical ToolResult captured the R1 body"
    );

    lab.remove_skill("alpha");
    let mut second = lab.spawn(child::COLD_RESUME_READ, None);
    second.resume_until("settled");
    second.sigkill();

    let durable = lab.durable();
    let ledger = durable.canonical();
    assert_eq!(
        &ledger[..historical.len()],
        &historical[..],
        "reload never rewrites an old ToolResult"
    );
    let results = tool_results(&ledger);
    assert!(
        matches!(
            results.last(),
            Some((_, ToolExecutionStatus::Failed { .. }))
        ),
        "the deleted Skill reads as a normal read error: {results:?}"
    );
}

/// Invalid current resources fail runtime creation explicitly instead of
/// falling back to the historical generation as live authority.
#[test]
fn invalid_current_resources_fail_runtime_creation() {
    let lab = Lab::new();
    let mut first = lab.spawn(child::TEXT_TURN, None);
    first.wait_note("settled");
    first.sigkill();
    let snapshots_before = lab.durable().request_snapshots().len();

    std::fs::write(lab.root().join("rustx.json"), "{ not json").expect("corrupt the config");
    let mut second = lab.spawn(child::COMPOSE_ONLY, None);
    let outcome = second.wait_note_prefixed("compose:");
    second.sigkill();
    assert!(
        outcome.starts_with("compose:err"),
        "runtime creation fails explicitly: {outcome}"
    );

    assert_eq!(
        lab.durable().request_snapshots().len(),
        snapshots_before,
        "no request was admitted under a fallback generation"
    );
}

/// A process that died before its first model request leaves no durable
/// resource or Skill-guidance record at all; reopening simply loads current
/// resources for the future request.
#[test]
fn death_before_the_first_request_leaves_no_resource_record() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::TEXT_TURN, Some("before:commit_model_turn_start"));
    process.wait_reached("before:commit_model_turn_start");
    process.sigkill();

    let durable = lab.durable();
    assert!(
        durable.request_snapshots().is_empty(),
        "no RequestSnapshot exists"
    );
    let body = format!("{:?}", durable.canonical());
    assert!(
        !body.contains("R1 project instructions."),
        "no canonical project-instruction fact exists"
    );
    assert!(
        !body.contains("alpha summary"),
        "no canonical Skill-guidance fact exists"
    );

    lab.write_project_instructions("R2 project instructions.");
    let mut resumed = lab.spawn(child::COLD_RESUME, None);
    resumed.resume_until("settled");
    resumed.sigkill();
    assert!(system_prompt(&lab.durable(), 0).contains("R2 project instructions."));
}

// ---------------------------------------------------------------------------
// 8. Background / subagent recovery
// ---------------------------------------------------------------------------

/// Ownership committed and the child still running at process death: recovery
/// terminalizes the ownership exactly once, and repeating recovery changes
/// nothing.
#[test]
fn committed_background_ownership_terminalizes_exactly_once() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::BACKGROUND_TOOL,
        Some("after:event:background_execution_committed"),
    );
    process.wait_reached("after:event:background_execution_committed");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable.count_events(|event| matches!(
            event,
            RuntimeEvent::BackgroundExecutionCommitted { .. }
        )),
        1
    );
    let report = durable.recover();
    assert_eq!(report.background_classes().len(), 1);
    assert_eq!(report.reconciliation().background_terminals.len(), 1);

    // Recovery is absorbing: a second restart adds no second terminal.
    let repeated = durable.recover();
    assert!(repeated.reconciliation().background_terminals.is_empty());
    assert_eq!(
        durable.count_events(|event| matches!(
            event,
            RuntimeEvent::BackgroundTerminalPublished { .. }
        )),
        1
    );
}

/// A reopened runtime never resurrects the dead process's background
/// execution: no new ownership is committed and no old owner is reattached.
#[test]
fn a_reopened_runtime_never_relaunches_a_dead_background_execution() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::BACKGROUND_TOOL,
        Some("after:event:background_execution_committed"),
    );
    process.wait_reached("after:event:background_execution_committed");
    process.sigkill();

    let mut resumed = lab.spawn(child::COLD_RESUME, None);
    resumed.resume_until("settled");
    resumed.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable.count_events(|event| matches!(
            event,
            RuntimeEvent::BackgroundExecutionCommitted { .. }
        )),
        1,
        "no second ownership is committed"
    );
    assert_eq!(
        durable.count_events(|event| matches!(
            event,
            RuntimeEvent::BackgroundTerminalPublished { .. }
        )),
        1,
        "the unresolved ownership publishes exactly one terminal"
    );
}

/// A durably owned subagent child, alive at the moment of process death.
///
/// The subagent plane has its own durable lifecycle — its own ownership and
/// terminal facts, its own recovery evidence, its own ordinal domain, and its
/// own reconciliation — so it is proven directly rather than by analogy with
/// the background rows. The child process is real, in this test's process
/// group, and never answers: it is durably owned and unsettled exactly when
/// the `SIGKILL` lands.
#[test]
fn committed_subagent_ownership_terminalizes_exactly_once() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::SUBAGENT_TOOL,
        Some("after:event:subagent_ownership_committed"),
    );
    process.wait_reached("after:event:subagent_ownership_committed");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentOwnershipCommitted { .. })),
        1,
        "exactly one ownership fact crossed the durable boundary"
    );
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentTerminalPublished { .. })),
        0,
        "the owned child never settled before the kill"
    );

    let report = durable.recover();
    assert_eq!(
        report.subagent_classes().len(),
        1,
        "the owned-but-unsettled child is recovery evidence in its own right"
    );
    assert_eq!(
        report.reconciliation().subagent_terminals.len(),
        1,
        "recovery terminalizes the interrupted child exactly once"
    );
    assert!(
        report.highest_subagent_ordinal() >= 1,
        "the durable subagent ordinal domain is recovered for reseeding"
    );

    // Recovery is absorbing: repeating it adds no second terminal, and the
    // published terminal count stays at exactly one.
    let repeated = durable.recover();
    assert!(
        repeated.reconciliation().subagent_terminals.is_empty(),
        "a repeated recovery re-terminalizes nothing"
    );
    assert!(
        repeated.subagent_classes().is_empty(),
        "the settled child leaves the unresolved working set"
    );
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentTerminalPublished { .. })),
        1,
        "exactly one terminal publication exists after any number of restarts"
    );
}

/// Death immediately before the subagent terminal publication transaction.
///
/// The terminal candidate is durably known to the dead process only as
/// process-local driver state; the durable authority still holds an owned,
/// unpublished child. Recovery must publish exactly one terminal — never a
/// half-published lineage terminal, and never a second one.
#[test]
fn subagent_terminal_publication_is_atomic() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::SUBAGENT_SETTLED,
        Some("before:event:subagent_terminal_published"),
    );
    process.wait_reached("before:event:subagent_terminal_published");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentOwnershipCommitted { .. })),
        1
    );
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentTerminalPublished { .. })),
        0,
        "the publication transaction committed nothing"
    );

    let report = durable.recover();
    assert_eq!(
        report.subagent_classes().len(),
        1,
        "the unpublished child is still durably owned"
    );
    assert_eq!(report.reconciliation().subagent_terminals.len(), 1);
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentTerminalPublished { .. })),
        1,
        "recovery publishes the terminal exactly once"
    );
    assert!(
        durable
            .recover()
            .reconciliation()
            .subagent_terminals
            .is_empty(),
        "and never a second time"
    );
}

/// A reopened runtime never reattaches or relaunches the dead process's child,
/// and never re-adopts its historical identity.
///
/// A v1 child is one-shot and process-local: there is no reattach by
/// construction, and the durable proof is that a live reopened runtime commits
/// no second ownership, starts no second child, and reseeds its ordinal
/// allocator above every identity already in durable authority.
#[test]
fn a_reopened_runtime_never_readopts_a_dead_subagent() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::SUBAGENT_TOOL,
        Some("after:event:subagent_ownership_committed"),
    );
    process.wait_reached("after:event:subagent_ownership_committed");
    process.sigkill();

    let historical = lab.durable().recover().highest_subagent_ordinal();

    let mut resumed = lab.spawn(child::COLD_RESUME, None);
    resumed.resume_until("settled");
    resumed.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentOwnershipCommitted { .. })),
        1,
        "no second ownership is committed for the dead child"
    );
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentTerminalPublished { .. })),
        1,
        "the interrupted child publishes exactly one terminal, once"
    );
    assert_eq!(
        durable.recover().highest_subagent_ordinal(),
        historical,
        "the reopened runtime allocates above the historical ordinal instead          of re-adopting it"
    );
}

// ---------------------------------------------------------------------------
// 9. Transcript recovery
// ---------------------------------------------------------------------------

/// After a real process death the derived transcript still pages canonical
/// history, the interaction audit, the publication audit, and history retired
/// from the current Surface by compaction.
#[test]
fn the_transcript_pages_every_durable_owner_after_process_death() {
    // Canonical Assistant and ToolResult history.
    let tools = Lab::new();
    let mut process = tools.spawn(child::TOOL_TURN, None);
    process.wait_note("settled");
    process.sigkill();
    assert_eq!(
        tools
            .durable()
            .transcript()
            .iter()
            .filter(|entry| matches!(entry.item, TranscriptItem::Message { .. }))
            .count(),
        4,
        "user, assistant-with-call, tool result, assistant"
    );

    // Incomplete and Unaccepted publication audits.
    for (gate, kind) in [
        (
            "after:event:model_request_completed",
            PublicationAuditKind::Incomplete,
        ),
        (
            "after:commit_publication_terminal",
            PublicationAuditKind::Unaccepted,
        ),
    ] {
        let lab = Lab::new();
        let mut process = lab.spawn(child::TEXT_TURN, Some(gate));
        process.wait_reached(gate);
        process.sigkill();
        let durable = lab.durable();
        durable.recover();
        assert_eq!(audit_kinds(&durable.transcript()), vec![kind]);
    }

    // The interaction audit.
    let interaction = Lab::new();
    interaction.write_runtime_config("always");
    let mut process = interaction.spawn(child::TOOL_APPROVAL, None);
    process.wait_note("settled");
    process.sigkill();
    let entries = interaction.durable().transcript();
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry.item, TranscriptItem::InteractionRequested { .. }))
    );
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry.item, TranscriptItem::InteractionSettled { .. }))
    );

    // History retired from the current Surface by compaction.
    let compacted = Lab::new();
    let mut process = compacted.spawn(child::COMPACTION, Some("after:commit_compaction"));
    process.wait_note("settled");
    process.resume();
    process.wait_note("summary-in-flight");
    process.resume();
    process.wait_reached("after:commit_compaction");
    process.sigkill();
    let durable = compacted.durable();
    assert_eq!(shapes(&durable.surface()), vec!["summary"]);
    assert!(
        durable.transcript().len() > 1,
        "retired history still pages"
    );
}

/// Client presence at crash time is not part of the durable contract: a child
/// with no observation bridge at all produces the same durable state as one
/// with a client-facing consumer attached.
#[test]
fn client_presence_at_crash_time_changes_no_durable_result() {
    let gate = "after:commit_publication_terminal";

    let with_client = Lab::new();
    let mut process = with_client.spawn(child::TEXT_TURN, Some(gate));
    process.wait_reached(gate);
    process.sigkill();
    let durable = with_client.durable();
    durable.recover();
    let attached = (
        shapes(&durable.canonical()),
        audit_kinds(&durable.transcript()),
    );

    let without_client = Lab::new();
    let mut process = without_client.spawn(child::TEXT_TURN_NO_CLIENT, Some(gate));
    // The only rendezvous is the durable boundary itself. Waiting on the
    // child's own "submitted" note first would race the boundary the admission
    // worker reaches concurrently, and prove nothing extra: the child parks
    // owning its runtime, so reaching the boundary is what says the submit
    // travelled the whole real admission path.
    process.wait_reached(gate);
    process.sigkill();
    let durable = without_client.durable();
    durable.recover();
    let detached = (
        shapes(&durable.canonical()),
        audit_kinds(&durable.transcript()),
    );

    assert_eq!(attached, detached);
}

// ---------------------------------------------------------------------------
// Combination scenarios
// ---------------------------------------------------------------------------

/// A slow streaming model, a second inbound accepted while the stream is open,
/// and process death before the provider outcome: the two planes stay
/// independent and linearizable.
#[test]
fn streaming_output_and_pending_inbound_compose() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::STREAMING_INBOUND,
        Some("before:event:model_request_completed"),
    );
    process.wait_note("second-inbound-accepted");
    process.resume();
    process.wait_reached("before:event:model_request_completed");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable.store().load_pending().expect("pending").len(),
        1,
        "the inbound accepted during the stream is durable, independent work"
    );
    assert!(!has_p(&durable));
    assert_publication_implication(&durable);

    let report = durable.recover();
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Incomplete
    );
    assert_eq!(report.pending_inbound(), 1, "the pending item is preserved");
    assert_eq!(
        report.resume(),
        ResumeDisposition::BlockedIndeterminate,
        "an indeterminate provider outcome blocks continuation, not acceptance"
    );
}

/// A background-capable turn killed before the canonical Assistant boundary:
/// the open stream settles as exactly one audit, and because no proposal was
/// ever accepted, no background ownership exists to reattach.
#[test]
fn background_capable_turn_and_streaming_publication_compose() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::BACKGROUND_TOOL,
        Some("before:commit_canonical_publication"),
    );
    process.wait_reached("before:commit_canonical_publication");
    process.sigkill();

    let durable = lab.durable();
    assert_publication_implication(&durable);
    let report = durable.recover();
    assert_eq!(
        report.publication_classes().len(),
        1,
        "the open stream settles as exactly one audit"
    );
    assert!(
        !has_tool_start(&durable),
        "no proposal became an execution before C"
    );
    assert!(
        report.background_classes().is_empty(),
        "no background ownership was ever committed before C"
    );
}

/// A tool that already produced its canonical result, a continuation request in
/// flight, and a second inbound accepted during it: process death leaves the
/// settled tool result canonical, the continuation indeterminate, and the new
/// inbound pending.
#[test]
fn settled_tool_result_and_new_inbound_compose_with_an_indeterminate_request() {
    let lab = Lab::new();
    // The second acceptance is the inbound that arrives while the continuation
    // request is in flight, so the child freezes with all three facts durable:
    // the settled tool result, the started continuation request, and the newly
    // accepted inbound.
    let mut process = lab.spawn_nth(
        child::TOOL_CONTINUATION_INBOUND,
        Some("after:accept_inbound"),
        2,
    );
    process.wait_reached("after:accept_inbound");
    process.sigkill();

    let durable = lab.durable();
    let results = tool_results(&durable.canonical());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1, ToolExecutionStatus::Success);
    assert_eq!(durable.store().load_pending().expect("pending").len(), 1);

    let report = durable.recover();
    assert!(matches!(
        report.attempt_class(),
        AttemptRecoveryClass::IndeterminateExternalOutcome { .. }
    ));
    assert_eq!(report.resume(), ResumeDisposition::BlockedIndeterminate);
    assert_eq!(report.pending_inbound(), 1);
    assert_eq!(
        tool_results(&durable.canonical()),
        results,
        "the already-settled tool result is never repaired again"
    );
}

/// An approval settlement, the tool-start boundary, and a real process death
/// compose exactly as their owners specify: the settlement precedes the
/// external side effect, the outcome stays unknown, and the audit stays one
/// historical fact.
#[test]
fn approval_settlement_and_the_tool_start_boundary_compose() {
    let lab = Lab::new();
    lab.write_runtime_config("always");
    let mut process = lab.spawn(
        child::TOOL_APPROVAL,
        Some("after:event:tool_execution_started"),
    );
    process.wait_reached("after:event:tool_execution_started");
    process.sigkill();

    let durable = lab.durable();
    let settled = durable
        .sequence_of(|event| matches!(event, RuntimeEvent::InteractionSettled { .. }))
        .expect("a settled audit");
    let started = durable
        .sequence_of(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
        .expect("a started execution");
    assert!(
        settled < started,
        "the approval settles before any external side effect"
    );

    let report = durable.recover();
    assert_eq!(report.resume(), ResumeDisposition::BlockedIndeterminate);
    assert_eq!(
        tool_results(&durable.canonical())[0].1,
        ToolExecutionStatus::Interrupted
    );
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::InteractionSettled { .. })),
        1
    );
}
