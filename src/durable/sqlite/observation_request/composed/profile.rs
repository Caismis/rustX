//! Local opt-in measurements, never authentication evidence or a result cache.
//! The fixed diagnostic set records potential reuse; every SQL check still runs.
use super::{Connection, Root, index, manifest};
use serde::Serialize;
use std::{cell::RefCell, collections::HashMap, time::Instant};

const IDENTITY_LIMIT: usize = 512;
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Identity {
    native_id: u64,
    native_digest: [u8; 32],
    native_count: u64,
    native_height: u8,
    source_hash: index::Hash,
    origin_sequence: u64,
    origin_hash: Option<index::Hash>,
}
#[derive(Default, Serialize)]
struct Summary {
    kind: &'static str,
    sequence: u64,
    selected_items: u64,
    layout_completed: bool,
    source_us: u128,
    capture_us: u128,
    scalar_us: u128,
    authenticate_us: u128,
    paths_us: u128,
    length_us: u128,
    auth_calls: u64,
    origin_root_loads: u64,
    path_hops: u64,
    max_path_hops: u8,
    source_node_loads: u64,
    authenticated_targets: u64,
    repeated_identities: u64,
    same_write_token_repeats: u64,
    write_invalidated_repeats: u64,
    tracked_identities: usize,
    identity_limit: usize,
    overflow_visits: u64,
}
struct Active {
    summary: Summary,
    started: Instant,
    // Capacity is reserved once. Entries contain only fixed identity scalars
    // and the connection's total_changes token; no metadata/body is retained.
    identities: HashMap<Identity, u64>,
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
        if std::env::var_os("RUSTX_SOURCE_AUTH_PROFILE").as_deref()
            != Some(std::ffi::OsStr::new("1"))
        {
            return Self(false);
        }
        Self(ACTIVE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return false;
            }
            *slot = Some(Active {
                summary: Summary {
                    kind: "source_auth_profile",
                    sequence,
                    selected_items,
                    identity_limit: IDENTITY_LIMIT,
                    ..Summary::default()
                },
                started: Instant::now(),
                identities: HashMap::with_capacity(IDENTITY_LIMIT),
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
        active.summary.tracked_identities = active.identities.len();
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
    Authenticate,
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
                Phase::Authenticate => &mut active.summary.authenticate_us,
                Phase::Paths => &mut active.summary.paths_us,
                Phase::Length => &mut active.summary.length_us,
            };
            *value += elapsed;
        });
    }
}
pub(super) fn auth_call() {
    record(|active| active.summary.auth_calls += 1);
}
pub(super) fn origin_root_load() {
    record(|active| active.summary.origin_root_loads += 1);
}
pub(super) fn path_hop(depth: u8) {
    record(|active| {
        active.summary.path_hops += 1;
        active.summary.max_path_hops = active.summary.max_path_hops.max(depth);
    });
}
pub(super) fn source_node_load() {
    record(|active| active.summary.source_node_loads += 1);
}
pub(super) fn proof_visit(
    connection: &Connection,
    native: manifest::NodeRef,
    expected: index::Hash,
    origin: &Root,
    target: bool,
) {
    record(|active| {
        active.summary.authenticated_targets += u64::from(target);
        let identity = Identity {
            native_id: native.id,
            native_digest: native.digest,
            native_count: native.count,
            native_height: native.height,
            source_hash: expected,
            origin_sequence: origin.sequence,
            origin_hash: origin.source_hash,
        };
        active.visit(identity, connection.total_changes());
    });
}
impl Active {
    fn visit(&mut self, identity: Identity, write_token: u64) {
        if let Some(previous) = self.identities.get_mut(&identity) {
            self.summary.repeated_identities += 1;
            if *previous == write_token {
                self.summary.same_write_token_repeats += 1;
            } else {
                self.summary.write_invalidated_repeats += 1;
            }
            *previous = write_token;
        } else if self.identities.len() < IDENTITY_LIMIT {
            self.identities.insert(identity, write_token);
        } else {
            // This counts untracked visits, not distinct overflow identities.
            // Repetition counters become lower bounds when this is nonzero.
            self.summary.overflow_visits += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Active, IDENTITY_LIMIT, Identity, Summary};
    use std::{collections::HashMap, time::Instant};

    fn identity(id: u64) -> Identity {
        Identity {
            native_id: id,
            native_digest: [1; 32],
            native_count: 1,
            native_height: 1,
            source_hash: [2; 32],
            origin_sequence: 7,
            origin_hash: Some([3; 32]),
        }
    }

    #[test]
    fn diagnostic_repeats_distinguish_writes_and_stop_admitting_at_the_cap() {
        let mut active = Active {
            summary: Summary::default(),
            started: Instant::now(),
            identities: HashMap::with_capacity(IDENTITY_LIMIT),
        };
        active.visit(identity(0), 10);
        active.visit(identity(0), 10);
        active.visit(identity(0), 11);
        assert_eq!(active.summary.repeated_identities, 2);
        assert_eq!(active.summary.same_write_token_repeats, 1);
        assert_eq!(active.summary.write_invalidated_repeats, 1);
        for id in 1..=u64::try_from(IDENTITY_LIMIT).unwrap() {
            active.visit(identity(id), 11);
        }
        let capacity = active.identities.capacity();
        assert_eq!(active.identities.len(), IDENTITY_LIMIT);
        assert_eq!(active.summary.overflow_visits, 1);
        active.visit(identity(u64::MAX), 11);
        active.visit(identity(u64::MAX), 11);
        assert_eq!(active.identities.capacity(), capacity);
        assert_eq!(active.summary.overflow_visits, 3);
        assert_eq!(active.summary.repeated_identities, 2);
    }

    #[test]
    fn scalar_record_is_bounded_even_at_every_numeric_limit() {
        let summary = Summary {
            kind: "source_auth_profile",
            sequence: u64::MAX,
            selected_items: u64::MAX,
            layout_completed: true,
            source_us: u128::MAX,
            capture_us: u128::MAX,
            scalar_us: u128::MAX,
            authenticate_us: u128::MAX,
            paths_us: u128::MAX,
            length_us: u128::MAX,
            auth_calls: u64::MAX,
            origin_root_loads: u64::MAX,
            path_hops: u64::MAX,
            max_path_hops: u8::MAX,
            source_node_loads: u64::MAX,
            authenticated_targets: u64::MAX,
            repeated_identities: u64::MAX,
            same_write_token_repeats: u64::MAX,
            write_invalidated_repeats: u64::MAX,
            tracked_identities: usize::MAX,
            identity_limit: IDENTITY_LIMIT,
            overflow_visits: u64::MAX,
        };
        assert!(summary.line().unwrap().len() <= 2048);
    }
}
