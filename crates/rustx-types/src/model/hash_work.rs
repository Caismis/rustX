//! Test-only instrumentation counting semantic hashing work shared with
//! selection-preparation tests in the monolith crate.
thread_local! {
    static HASH_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// Record `original` canonical bytes and `semantic` projected bytes hashed.
///
/// Restores the pre-move accounting: the TLS counter observed by
/// selection-preparation tests and the native-work attribution read through
/// `agent::execution::attribution` profiles.
pub fn record(original: usize, semantic: usize) {
    HASH_WORK.with(|n| {
        let (o, s) = n.get();
        n.set((o + original, s + semantic));
    });
    crate::agent::execution::attribution::record_native(
        crate::agent::execution::attribution::NativeWork {
            hash_bytes: (original + semantic) as u64,
            ..Default::default()
        },
    );
}

/// Take and reset the accumulated counts.
#[must_use]
pub fn take() -> (usize, usize) {
    HASH_WORK.with(|n| n.replace((0, 0)))
}
