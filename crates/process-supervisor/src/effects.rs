//! Test-only observation of entry into the physical process-effect
//! boundary.
//!
//! The host runtime installs one observer so its static-effects
//! measurement can count boundary entries inside this package; without an
//! installed observer the call is a no-op. No production code path depends
//! on the observer.

#[cfg(any(test, feature = "test-support"))]
static PROCESS_EFFECT_OBSERVER: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Installs the host's process-effect observer. The first install wins;
/// the observation contract is process-global and single-owner.
#[cfg(any(test, feature = "test-support"))]
pub fn set_process_effect_observer(observer: fn()) {
    let _ = PROCESS_EFFECT_OBSERVER.set(observer);
}

/// Records one entry into the physical process boundary when the host
/// installed an observer.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn observe_process() {
    if let Some(observer) = PROCESS_EFFECT_OBSERVER.get() {
        observer();
    }
}
