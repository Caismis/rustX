//! Local opt-in Source timings and operation counts; no capture authentication claim.
use serde::Serialize;
use std::{cell::RefCell, time::Instant};

#[derive(Default, Serialize)]
struct Summary {
    kind: &'static str,
    sequence: u64,
    selected_items: u64,
    layout_completed: bool,
    source_us: u128,
    capture_us: u128,
    scalar_us: u128,
    lookup_us: u128,
    paths_us: u128,
    length_us: u128,
    lookup_calls: u64,
    source_node_loads: u64,
}
struct Active {
    summary: Summary,
    started: Instant,
}
thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}
fn record(visit: impl FnOnce(&mut Active)) {
    ACTIVE.with(|active| {
        if let Some(active) = active.borrow_mut().as_mut() {
            visit(active);
        }
    });
}
pub(super) struct Request(bool);
impl Request {
    pub(super) fn new(sequence: u64, selected_items: u64) -> Self {
        if std::env::var_os("RUSTX_SOURCE_PROFILE").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return Self(false);
        }
        Self(ACTIVE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return false;
            }
            *slot = Some(Active {
                summary: Summary {
                    kind: "source_profile",
                    sequence,
                    selected_items,
                    ..Summary::default()
                },
                started: Instant::now(),
            });
            true
        }))
    }
    pub(super) fn layout_completed(&mut self) {
        if self.0 {
            record(|active| active.summary.layout_completed = true);
        }
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let Some(mut active) = ACTIVE.with(|slot| slot.borrow_mut().take()) else {
            return;
        };
        active.summary.source_us = active.started.elapsed().as_micros();
        // All serialized fields are fixed scalar types. No source identity text,
        // payload, error, path, or diagnostic-set contents enter the output.
        if let Some(line) = active.summary.line() {
            eprintln!("{line}");
        }
    }
}
impl Summary {
    fn line(&self) -> Option<String> {
        serde_json::to_string(self)
            .ok()
            .filter(|line| line.len() <= 2048)
    }
}
#[derive(Clone, Copy)]
pub(super) enum Phase {
    Capture,
    Scalar,
    Lookup,
    Paths,
    Length,
}
pub(super) struct Timer {
    phase: Phase,
    started: Option<Instant>,
}
impl Timer {
    pub(super) fn new(phase: Phase) -> Self {
        Self {
            phase,
            started: ACTIVE.with(|active| active.borrow().is_some().then(Instant::now)),
        }
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        let Some(started) = self.started else {
            return;
        };
        let elapsed = started.elapsed().as_micros();
        record(|active| {
            let value = match self.phase {
                Phase::Capture => &mut active.summary.capture_us,
                Phase::Scalar => &mut active.summary.scalar_us,
                Phase::Lookup => &mut active.summary.lookup_us,
                Phase::Paths => &mut active.summary.paths_us,
                Phase::Length => &mut active.summary.length_us,
            };
            *value += elapsed;
        });
    }
}
pub(super) fn lookup_call() {
    record(|active| active.summary.lookup_calls += 1);
}
pub(super) fn source_node_load() {
    record(|active| active.summary.source_node_loads += 1);
}
#[cfg(test)]
mod tests {
    use super::Summary;

    #[test]
    fn scalar_record_is_bounded_even_at_every_numeric_limit() {
        let summary = Summary {
            kind: "source_profile",
            sequence: u64::MAX,
            selected_items: u64::MAX,
            layout_completed: true,
            source_us: u128::MAX,
            capture_us: u128::MAX,
            scalar_us: u128::MAX,
            lookup_us: u128::MAX,
            paths_us: u128::MAX,
            length_us: u128::MAX,
            lookup_calls: u64::MAX,
            source_node_loads: u64::MAX,
        };
        assert!(summary.line().unwrap().len() <= 2048);
    }
}
