//! Scoped scalar timings for explicitly invoked native request measurements.
//! All scopes are inclusive: start contains commit and reconstruction; prepare
//! may contain context projection. These durations must not be summed together.

use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Prepare,
    Start,
    Commit,
    Reconstruct,
    ContextProjection,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PhaseProfile {
    pub(crate) calls: u64,
    pub(crate) total_us: u128,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AgentRequestProfile {
    pub(crate) prepare: PhaseProfile,
    pub(crate) start: PhaseProfile,
    pub(crate) commit: PhaseProfile,
    pub(crate) reconstruct: PhaseProfile,
    pub(crate) context_projection: PhaseProfile,
}

static ACTIVE: Mutex<Option<Arc<Mutex<AgentRequestProfile>>>> = Mutex::new(None);

pub(crate) struct Guard {
    profile: Arc<Mutex<AgentRequestProfile>>,
}

pub(crate) fn start() -> Guard {
    let mut active = ACTIVE.lock().unwrap();
    assert!(active.is_none(), "one native agent request measurement");
    let profile = Arc::new(Mutex::new(AgentRequestProfile::default()));
    *active = Some(Arc::clone(&profile));
    Guard { profile }
}

impl Guard {
    pub(crate) fn snapshot(&self) -> AgentRequestProfile {
        *self.profile.lock().unwrap()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        *ACTIVE.lock().unwrap() = None;
    }
}

pub(crate) struct Timer {
    phase: Phase,
    started: Option<(Arc<Mutex<AgentRequestProfile>>, Instant)>,
}

impl Timer {
    pub(crate) fn new(phase: Phase) -> Self {
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
            Phase::Reconstruct => &mut profile.reconstruct,
            Phase::ContextProjection => &mut profile.context_projection,
        };
        phase.calls += 1;
        phase.total_us += elapsed;
    }
}
