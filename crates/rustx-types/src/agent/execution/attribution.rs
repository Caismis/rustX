//! Scoped scalar timings for explicitly invoked native request measurements.
//! These are elapsed timings (including any I/O waits). All scopes are inclusive: start contains commit and its original receipt-evidence check; prepare
//! may contain context projection. These durations must not be summed together.

use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy)]
pub enum Phase {
    Prepare,
    Start,
    Commit,
    CommitEvidence,
    ContextProjection,
    Source,
    SourceCapture,
    SourceLookup,
    SourceLeaf,
    SourceScalar,
    SourceGeometry,
    ContextObserve,
    ContextMeasure,
    InputAssembly,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PhaseProfile {
    pub calls: u64,
    pub total_us: u128,
}

/// Inclusive actual Source I/O. Lookup and leaf work are subsets of request work.
#[derive(Clone, Copy, Debug, Default)]
pub struct SourceWork {
    pub samples: u64,
    pub native_header_queries: u64,
    pub native_node_queries: u64,
    pub index_pages: u64,
    pub index_bytes: u64,
    pub metadata_blobs: u64,
    pub metadata_bytes: u64,
    pub new_leaves: u64,
    pub new_nodes: u64,
    pub new_fragment_bytes: u64,
    pub header_decodes: u64,
}
impl SourceWork {
    fn add(&mut self, value: Self) {
        self.samples += value.samples;
        self.native_header_queries += value.native_header_queries;
        self.native_node_queries += value.native_node_queries;
        self.index_pages += value.index_pages;
        self.index_bytes += value.index_bytes;
        self.metadata_blobs += value.metadata_blobs;
        self.metadata_bytes += value.metadata_bytes;
        self.new_leaves += value.new_leaves;
        self.new_nodes += value.new_nodes;
        self.new_fragment_bytes += value.new_fragment_bytes;
        self.header_decodes += value.header_decodes;
    }
}
#[derive(Clone, Copy)]
pub enum SourceScope {
    Request,
    Lookup,
    Leaf,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct ContextWork {
    pub anchor_roots: u64,
    pub token_bytes: u64,
    pub estimator_calls: u64,
    pub materialized_items: u64,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeWork {
    pub preparation_items: u64,
    pub old_items: u64,
    pub new_items: u64,
    pub index_queries: u64,
    pub hash_bytes: u64,
}
impl SourceWork {
    #[must_use]
    pub fn since(&self, previous: &Self) -> Self {
        Self {
            samples: self.samples - previous.samples,
            native_header_queries: self.native_header_queries - previous.native_header_queries,
            native_node_queries: self.native_node_queries - previous.native_node_queries,
            index_pages: self.index_pages - previous.index_pages,
            index_bytes: self.index_bytes - previous.index_bytes,
            metadata_blobs: self.metadata_blobs - previous.metadata_blobs,
            metadata_bytes: self.metadata_bytes - previous.metadata_bytes,
            new_leaves: self.new_leaves - previous.new_leaves,
            new_nodes: self.new_nodes - previous.new_nodes,
            new_fragment_bytes: self.new_fragment_bytes - previous.new_fragment_bytes,
            header_decodes: self.header_decodes - previous.header_decodes,
        }
    }
}
impl ContextWork {
    #[must_use]
    pub fn since(&self, previous: &Self) -> Self {
        Self {
            anchor_roots: self.anchor_roots - previous.anchor_roots,
            token_bytes: self.token_bytes - previous.token_bytes,
            estimator_calls: self.estimator_calls - previous.estimator_calls,
            materialized_items: self.materialized_items - previous.materialized_items,
        }
    }
}
impl NativeWork {
    #[must_use]
    pub fn since(&self, previous: &Self) -> Self {
        Self {
            preparation_items: self.preparation_items - previous.preparation_items,
            old_items: self.old_items - previous.old_items,
            new_items: self.new_items - previous.new_items,
            index_queries: self.index_queries - previous.index_queries,
            hash_bytes: self.hash_bytes - previous.hash_bytes,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct AgentRequestProfile {
    pub prepare: PhaseProfile,
    pub start: PhaseProfile,
    pub commit: PhaseProfile,
    pub commit_evidence: PhaseProfile,
    pub context_projection: PhaseProfile,
    pub source: PhaseProfile,
    pub source_capture: PhaseProfile,
    pub source_lookup: PhaseProfile,
    pub source_leaf: PhaseProfile,
    pub source_scalar: PhaseProfile,
    pub source_geometry: PhaseProfile,
    pub context_observe: PhaseProfile,
    pub context_measure: PhaseProfile,
    pub input_assembly: PhaseProfile,
    pub source_work: SourceWork,
    pub source_lookup_work: SourceWork,
    pub source_leaf_work: SourceWork,
    pub context_work: ContextWork,
    pub native_work: NativeWork,
}

static ACTIVE: Mutex<Option<Arc<Mutex<AgentRequestProfile>>>> = Mutex::new(None);

pub fn active() -> bool {
    ACTIVE.lock().unwrap().is_some()
}
pub fn record_source(scope: SourceScope, work: SourceWork) {
    let active = ACTIVE.lock().unwrap().clone();
    let Some(active) = active else {
        return;
    };
    let mut profile = active.lock().unwrap();
    match scope {
        SourceScope::Request => &mut profile.source_work,
        SourceScope::Lookup => &mut profile.source_lookup_work,
        SourceScope::Leaf => &mut profile.source_leaf_work,
    }
    .add(work);
}
pub fn record_context(work: ContextWork) {
    let active = ACTIVE.lock().unwrap().clone();
    let Some(active) = active else {
        return;
    };
    let mut profile = active.lock().unwrap();
    profile.context_work.anchor_roots += work.anchor_roots;
    profile.context_work.token_bytes += work.token_bytes;
    profile.context_work.estimator_calls += work.estimator_calls;
    profile.context_work.materialized_items += work.materialized_items;
}
pub fn record_native(work: NativeWork) {
    let active = ACTIVE.lock().unwrap().clone();
    let Some(active) = active else {
        return;
    };
    let mut profile = active.lock().unwrap();
    profile.native_work.preparation_items += work.preparation_items;
    profile.native_work.old_items += work.old_items;
    profile.native_work.new_items += work.new_items;
    profile.native_work.index_queries += work.index_queries;
    profile.native_work.hash_bytes += work.hash_bytes;
}
pub struct Guard {
    profile: Arc<Mutex<AgentRequestProfile>>,
}

pub fn start() -> Guard {
    let mut active = ACTIVE.lock().unwrap();
    assert!(active.is_none(), "one native agent request measurement");
    let profile = Arc::new(Mutex::new(AgentRequestProfile::default()));
    *active = Some(Arc::clone(&profile));
    Guard { profile }
}

impl Guard {
    #[must_use]
    pub fn snapshot(&self) -> AgentRequestProfile {
        *self.profile.lock().unwrap()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        *ACTIVE.lock().unwrap() = None;
    }
}

pub struct Timer {
    phase: Phase,
    started: Option<(Arc<Mutex<AgentRequestProfile>>, Instant)>,
}

impl Timer {
    pub fn new(phase: Phase) -> Self {
        let started = ACTIVE
            .lock()
            .unwrap()
            .as_ref()
            .map(|profile| (Arc::clone(profile), Instant::now()));
        Self { phase, started }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let Some((profile, began)) = &self.started else {
            return;
        };
        let elapsed = began.elapsed().as_micros();
        let mut profile = profile.lock().unwrap();
        let phase = match self.phase {
            Phase::Prepare => &mut profile.prepare,
            Phase::Start => &mut profile.start,
            Phase::Commit => &mut profile.commit,
            Phase::CommitEvidence => &mut profile.commit_evidence,
            Phase::ContextProjection => &mut profile.context_projection,
            Phase::Source => &mut profile.source,
            Phase::SourceCapture => &mut profile.source_capture,
            Phase::SourceLookup => &mut profile.source_lookup,
            Phase::SourceLeaf => &mut profile.source_leaf,
            Phase::SourceScalar => &mut profile.source_scalar,
            Phase::SourceGeometry => &mut profile.source_geometry,
            Phase::ContextObserve => &mut profile.context_observe,
            Phase::ContextMeasure => &mut profile.context_measure,
            Phase::InputAssembly => &mut profile.input_assembly,
        };
        phase.calls += 1;
        phase.total_us += elapsed;
    }
}
