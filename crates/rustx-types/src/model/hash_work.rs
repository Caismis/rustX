//! Test-only instrumentation counting semantic hashing work shared with
//! selection-preparation tests in the monolith crate.
thread_local! {
    static HASH_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// Record `original` canonical bytes and `semantic` projected bytes hashed.
pub fn record(original: usize, semantic: usize) {
    HASH_WORK.with(|n| {
        let (o, s) = n.get();
        n.set((o + original, s + semantic));
    });
}

/// Take and reset the accumulated counts.
pub fn take() -> (usize, usize) {
    HASH_WORK.with(|n| n.replace((0, 0)))
}
