//! Local diagnostic scalars, enabled only by `issue-387-profile`.
//!
//! Phase elapsed times are inclusive; item visits belong only to the innermost
//! phase. No scope is retained across an await. HTTP encoding carries only its
//! bounded context into the real serializer and performs no additional walk.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use serde::{Serialize, Serializer};

use crate::model::ModelProtocol;

static NEXT_OPERATION: AtomicU64 = AtomicU64::new(1);
static ENABLED: OnceLock<bool> = OnceLock::new();

fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var("RUSTX_ADAPTER_PROFILE").is_ok_and(|value| value == "1"))
}

fn bounded_label(request_id: &str) -> Arc<str> {
    let mut end = request_id.len().min(128);
    while !request_id.is_char_boundary(end) {
        end -= 1;
    }
    Arc::from(&request_id[..end])
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct InputLayout {
    /// Actual native selected history before this request's Context/RequestOnly
    /// assembly; includes a current user already adopted at that native cut.
    /// This is not an estimate of cross-turn old work or structural reuse.
    pub(crate) selected_history_count: usize,
    pub(crate) request_only_at: Option<usize>,
}
impl InputLayout {
    fn historical(self, index: usize) -> bool {
        if self.request_only_at == Some(index) {
            return false;
        }
        let rank = index - usize::from(self.request_only_at.is_some_and(|at| at < index));
        rank < self.selected_history_count
    }
}

#[derive(Clone, Serialize)]
struct NativeContext {
    request_id: Arc<str>,
    input: InputLayout,
}
#[derive(Clone, Serialize)]
struct Context {
    operation: u64,
    protocol: ModelProtocol,
    origin: &'static str,
    native: Option<NativeContext>,
    #[serde(skip)]
    emit: bool,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Validate,
    Modalities,
    ImageCount,
    Continuation,
    RequestCorrelation,
    Tools,
    Translate,
    TranslateMessages,
    NativeCorrelation,
    WireCorrelation,
    TypedToValue,
    FinalParams,
    HttpEncode,
}
#[derive(Default, Serialize)]
struct Work {
    selected_value_borrows: u64,
    item_visits: u64,
    selected_history_item_visits: u64,
    request_delta_item_visits: u64,
    unclassified_item_visits: u64,
    block_visits: u64,
    wire_item_visits: u64,
    wire_field_visits: u64,
}
#[derive(Clone)]
struct Frame {
    context: Context,
    work: Rc<RefCell<Work>>,
}
thread_local! {
    static NATIVE: RefCell<Option<NativeContext>> = const { RefCell::new(None) };
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
}

pub(crate) struct NativeScope(Option<NativeContext>);
impl NativeScope {
    pub(crate) fn new(request_id: &str, input: InputLayout) -> Self {
        if !enabled() {
            return Self(None);
        }
        // Native identifiers are diagnostic labels, never read authority.
        let native = NativeContext {
            request_id: bounded_label(request_id),
            input,
        };
        Self(NATIVE.with(|slot| slot.replace(Some(native))))
    }
}
impl Drop for NativeScope {
    fn drop(&mut self) {
        NATIVE.with(|slot| slot.replace(self.0.take()));
    }
}

pub(crate) struct RequestScope(Option<Context>);
impl RequestScope {
    pub(crate) fn new(protocol: ModelProtocol, origin: &'static str) -> Self {
        if !enabled() {
            return Self(None);
        }
        let context = Context {
            operation: NEXT_OPERATION.fetch_add(1, Ordering::Relaxed),
            protocol,
            origin,
            native: NATIVE.with(|slot| slot.borrow().clone()),
            emit: true,
        };
        Self(CONTEXT.with(|slot| slot.replace(Some(context))))
    }
}
impl Drop for RequestScope {
    fn drop(&mut self) {
        CONTEXT.with(|slot| slot.replace(self.0.take()));
    }
}

pub(crate) struct PhaseScope(Option<ActivePhase>);
struct ActivePhase {
    previous: Option<Frame>,
    context: Context,
    phase: Phase,
    work: Rc<RefCell<Work>>,
    started: Instant,
}
pub(crate) fn phase(phase: Phase) -> PhaseScope {
    phase_in(phase, CONTEXT.with(|slot| slot.borrow().clone()))
}
fn phase_in(phase: Phase, context: Option<Context>) -> PhaseScope {
    let Some(context) = context else {
        return PhaseScope(None);
    };
    let work = Rc::new(RefCell::new(Work::default()));
    let next = Frame {
        context: context.clone(),
        work: work.clone(),
    };
    PhaseScope(Some(ActivePhase {
        previous: FRAME.with(|slot| slot.replace(Some(next))),
        context,
        phase,
        work,
        started: Instant::now(),
    }))
}
impl Drop for PhaseScope {
    fn drop(&mut self) {
        let Some(active) = &mut self.0 else {
            return;
        };
        let elapsed_us = active.started.elapsed().as_micros();
        FRAME.with(|slot| slot.replace(active.previous.take()));
        let value = record(
            &active.context,
            active.phase,
            elapsed_us,
            &active.work.borrow(),
        );
        if active.context.emit {
            eprintln!("{value}");
        }
    }
}

fn record(context: &Context, phase: Phase, elapsed_us: u128, work: &Work) -> serde_json::Value {
    serde_json::json!({
        "adapter_profile": 1,
        "pid": std::process::id(),
        "context": context,
        "phase": phase,
        "elapsed_us": elapsed_us,
        "work": work,
        // Explicit rank-aware visits are a subset of these real native
        // borrows. Prefix hashing or helper traversals remain unclassified.
        "unclassified_selected_borrows": work.selected_value_borrows.saturating_sub(work.item_visits),
    })
}

pub(crate) fn borrowed_visit() {
    update(|work| work.selected_value_borrows += 1);
}

pub(crate) fn item(index: usize) {
    FRAME.with(|slot| {
        if let Some(frame) = &*slot.borrow() {
            let mut work = frame.work.borrow_mut();
            work.item_visits += 1;
            match frame.context.native.as_ref() {
                Some(native) if native.input.historical(index) => {
                    work.selected_history_item_visits += 1;
                }
                Some(_) => work.request_delta_item_visits += 1,
                None => work.unclassified_item_visits += 1,
            }
        }
    });
}
pub(crate) fn block() {
    update(|work| work.block_visits += 1);
}
pub(crate) fn wire_item() {
    update(|work| work.wire_item_visits += 1);
}
pub(crate) fn wire_field() {
    update(|work| work.wire_field_visits += 1);
}
fn update(update: impl FnOnce(&mut Work)) {
    FRAME.with(|slot| {
        if let Some(frame) = &*slot.borrow() {
            update(&mut frame.work.borrow_mut());
        }
    });
}

/// The actual HTTP serializer receives precisely the same owned JSON value.
pub(crate) struct Payload {
    value: serde_json::Value,
    context: Option<Context>,
}
impl From<serde_json::Value> for Payload {
    fn from(value: serde_json::Value) -> Self {
        Self {
            value,
            context: CONTEXT.with(|slot| slot.borrow().clone()),
        }
    }
}
impl Serialize for Payload {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let _phase = phase_in(Phase::HttpEncode, self.context.clone());
        self.value.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_serializer_receives_byte_identical_json() {
        let _request = RequestScope::new(ModelProtocol::OpenAiChatCompletions, "dispatch");
        let value = serde_json::json!({
            "messages": [{"role":"user", "content":"é\"\\\n"}],
            "n": 9_007_199_254_740_993_u64,
            "opaque": {"array":[null,true,1.25]},
        });
        let expected = serde_json::to_vec(&value).unwrap();
        let payload = Payload::from(value);
        assert_eq!(serde_json::to_vec(&payload).unwrap(), expected);
    }

    #[test]
    fn bounded_records_and_exact_nested_visit_classification() {
        // Install an explicit test context; never mutate process environment.
        let context = Context {
            operation: 1,
            protocol: ModelProtocol::OpenAiResponses,
            origin: "preflight",
            emit: false,
            native: Some(NativeContext {
                request_id: bounded_label(&"\u{0001}é".repeat(200)),
                input: InputLayout {
                    selected_history_count: 3,
                    request_only_at: Some(1),
                },
            }),
        };
        let _request = RequestScope(CONTEXT.with(|slot| slot.replace(Some(context))));
        let outer = phase(Phase::Validate);
        borrowed_visit();
        item(0);
        {
            let inner = phase(Phase::TranslateMessages);
            for index in 0..5 {
                borrowed_visit();
                item(index);
            }
            // Real nested helper/prefix read with no original rank evidence.
            borrowed_visit();
            block();
            wire_item();
            wire_field();
            let active = inner.0.as_ref().unwrap();
            let work = active.work.borrow();
            assert_eq!(work.selected_value_borrows, 6);
            assert_eq!(work.selected_history_item_visits, 3);
            assert_eq!(work.request_delta_item_visits, 2);
            assert_eq!(work.item_visits, 5);
            let value = record(&active.context, active.phase, 1, &work);
            assert_eq!(value["unclassified_selected_borrows"], 1);
            assert!(serde_json::to_vec(&value).unwrap().len() < 2048);
        }
        borrowed_visit();
        item(4);
        let work = outer.0.as_ref().unwrap().work.borrow();
        assert_eq!(work.selected_value_borrows, 2);
        assert_eq!(work.selected_history_item_visits, 1);
        assert_eq!(work.request_delta_item_visits, 1);
    }
}
