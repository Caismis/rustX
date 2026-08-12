//! Test-only synchronization hooks of the Runtime Client projection.
//!
//! [`ProjectionProbe`] can park the projection at its two externally
//! meaningful linearization boundaries:
//!
//! - `snapshot`: while the projection lock is held, before the snapshot
//!   and its cursor are read;
//! - `publish`: while the projection lock is held, after the observation
//!   fold and before the cursor allocation/publication.
//!
//! A test **arms** a gate before the operation it wants to park; an armed
//! gate parks exactly one operation and then releases/disarms. Unarmed
//! gates never interfere, so a probe can be installed on a projection
//! without stalling its other operations. A test can therefore construct
//! exact interleavings (snapshot before a transition, transition before a
//! snapshot) without timing assumptions. All synchronization is `std`
//! (mutex + condvar) because the projection boundary is a `std` mutex
//! critical section; the parking blocks the OS thread, so the race tests
//! run on a multi-threaded runtime. This module exists only under
//! `#[cfg(test)]`.
#![cfg(test)]

use std::sync::{Arc, Condvar, Mutex};

/// One two-phase gate of a projection boundary.
#[derive(Debug, Default)]
struct Gate {
    state: Mutex<GateState>,
    condvar: Condvar,
}

#[derive(Debug, Default)]
struct GateState {
    armed: bool,
    entered: bool,
    proceed: bool,
}

impl Gate {
    /// Signals that the boundary was entered (the projection lock is
    /// held); when armed, parks until [`Gate::release`]. An unarmed gate
    /// never blocks.
    fn enter(&self) {
        let mut state = self.state.lock().expect("projection probe lock poisoned");
        if !state.armed {
            return;
        }
        state.entered = true;
        self.condvar.notify_all();
        while !state.proceed {
            state = self
                .condvar
                .wait(state)
                .expect("projection probe wait poisoned");
        }
        state.armed = false;
    }

    /// Arms the gate: the next [`Gate::enter`] parks.
    fn arm(&self) {
        let mut state = self.state.lock().expect("projection probe lock poisoned");
        state.armed = true;
        state.entered = false;
        state.proceed = false;
    }

    /// Blocks until the boundary was entered.
    fn wait_entered(&self) {
        let mut state = self.state.lock().expect("projection probe lock poisoned");
        while !state.entered {
            state = self
                .condvar
                .wait(state)
                .expect("projection probe wait poisoned");
        }
    }

    /// Releases a parked boundary.
    fn release(&self) {
        let mut state = self.state.lock().expect("projection probe lock poisoned");
        state.proceed = true;
        self.condvar.notify_all();
    }
}

/// The test-only linearization hooks of one projection instance.
///
/// Unarmed gates never interfere: the probe only affects operations that
/// occur after the corresponding gate was armed.
#[derive(Debug, Default, Clone)]
pub(crate) struct ProjectionProbe {
    snapshot_gate: Arc<Gate>,
    publish_gate: Arc<Gate>,
}

impl ProjectionProbe {
    /// Parks a snapshot at its boundary when armed.
    pub(crate) fn snapshot_enter(&self) {
        self.snapshot_gate.enter();
    }

    /// Parks a publication at its boundary when armed.
    pub(crate) fn publish_enter(&self) {
        self.publish_gate.enter();
    }

    /// Arms the snapshot gate: the next snapshot parks.
    pub(crate) fn arm_snapshot(&self) {
        self.snapshot_gate.arm();
    }

    /// Arms the publish gate: the next publication parks.
    pub(crate) fn arm_publish(&self) {
        self.publish_gate.arm();
    }

    /// Blocks until a snapshot is parked at its boundary.
    pub(crate) fn wait_snapshot_entered(&self) {
        self.snapshot_gate.wait_entered();
    }

    /// Blocks until a publication is parked at its boundary.
    pub(crate) fn wait_publish_entered(&self) {
        self.publish_gate.wait_entered();
    }

    /// Releases a parked snapshot.
    pub(crate) fn release_snapshot(&self) {
        self.snapshot_gate.release();
    }

    /// Releases a parked publication.
    pub(crate) fn release_publish(&self) {
        self.publish_gate.release();
    }
}
