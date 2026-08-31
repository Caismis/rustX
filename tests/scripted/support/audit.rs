//! Shared architectural assertions for the scripted suites.
//!
//! These are the Agent Loop settlement invariants every scripted scenario
//! relies on, owned once here instead of restated per suite:
//!
//! - exactly one attempt terminal ([`assert_single_terminal`]);
//! - the terminal event is the last event of the trace;
//! - the platform `AttemptOutcome` corresponds to the terminal fact
//!   ([`assert_outcome`] for the durable audit view,
//!   [`assert_result_outcome`] for the in-memory execution result);
//! - exact recorded traces ([`assert_trace`]).

use rustx::events::types::{AttemptOutcome, RuntimeEvent};

use crate::scripted_suites::common::DurableExecutionAudit;

/// The terminal events of an attempt.
pub(crate) fn terminal_events(events: &[RuntimeEvent]) -> Vec<&RuntimeEvent> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )
        })
        .collect()
}

/// Asserts exactly one terminal event and that it is the last event.
pub(crate) fn assert_single_terminal(events: &[RuntimeEvent]) -> &RuntimeEvent {
    let terminals = terminal_events(events);
    assert_eq!(terminals.len(), 1, "exactly one terminal event");
    assert_eq!(
        events.last(),
        Some(terminals[0]),
        "no runtime events may follow the terminal event"
    );
    terminals[0]
}

/// Asserts the platform outcome equals the outcome of the terminal event.
pub(crate) fn assert_outcome(result: &DurableExecutionAudit, expected: &AttemptOutcome) {
    assert_eq!(result.outcome, *expected, "platform outcome mismatch");
    let terminal = result.event_history.last().expect("terminal event");
    assert_eq!(
        AttemptOutcome::from_terminal_event(terminal),
        Some(expected.clone()),
        "outcome must match the terminal event"
    );
}

/// Asserts the exact recorded trace.
pub(crate) fn assert_trace(events: &[RuntimeEvent], expected: &[RuntimeEvent]) {
    assert_eq!(
        events,
        expected,
        "trace mismatch:\nactual:   {}\nexpected: {}",
        describe_trace(events),
        describe_trace(expected)
    );
}

fn describe_trace(events: &[RuntimeEvent]) -> String {
    events
        .iter()
        .map(|event| serde_json::to_string(event).expect("serialize event"))
        .collect::<Vec<_>>()
        .join("\n          ")
}
