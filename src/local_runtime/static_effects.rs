//! Test-only observation of entry into effect-owning boundaries.
use std::cell::RefCell;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Effect {
    Process,
    Connect,
    Provider,
    Session,
    State,
    Trust,
    Preparation,
    Credentials,
}

thread_local! { static COUNTS: RefCell<Option<[usize; 8]>> = const { RefCell::new(None) }; }

pub(crate) fn observe(effect: Effect) {
    COUNTS.with(|counts| {
        if let Some(counts) = counts.borrow_mut().as_mut() {
            counts[effect as usize] += 1;
        }
    });
}

pub(crate) fn measure<T>(operation: impl FnOnce() -> T) -> (T, [usize; 8]) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            COUNTS.with(|counts| *counts.borrow_mut() = None);
        }
    }
    COUNTS.with(|counts| {
        assert!(counts.borrow().is_none());
        *counts.borrow_mut() = Some([0; 8]);
    });
    let _reset = Reset;
    let value = operation();
    let counts = COUNTS.with(|counts| counts.borrow().expect("measurement active"));
    (value, counts)
}
