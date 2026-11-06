//! Single-threaded, resource-bounded native Template worker.
//!
//! The allocator cap is local to this executable. Allocation failure aborts
//! this worker, whose existing supervisor owns termination and reaping; it can
//! never abort the application server rendering an authored template.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

const HEAP_LIMIT: usize = 128 * 1024 * 1024;

struct BoundedAllocator;
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

fn reserve(bytes: usize) -> bool {
    LIVE_BYTES
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
            live.checked_add(bytes).filter(|next| *next <= HEAP_LIMIT)
        })
        .is_ok()
}

#[global_allocator]
static ALLOCATOR: BoundedAllocator = BoundedAllocator;

// This is the standard System allocator contract with one atomic requested-
// bytes reservation. No allocator path allocates or unwinds itself.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for BoundedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: the caller supplies the valid System allocation layout.
        let pointer = unsafe { System.alloc(layout) };
        if pointer.is_null() {
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: the caller supplies the valid System allocation layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if pointer.is_null() {
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller guarantees this pointer belongs to this layout.
        unsafe {
            System.dealloc(pointer, layout);
        }
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let increase = new_size.saturating_sub(layout.size());
        if !reserve(increase) {
            return std::ptr::null_mut();
        }
        // SAFETY: the caller guarantees pointer/layout and the nonzero new
        // size satisfy System's realloc contract. On failure the old block
        // remains live, so only its speculative increase is released.
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if resized.is_null() {
            LIVE_BYTES.fetch_sub(increase, Ordering::Relaxed);
        } else {
            LIVE_BYTES.fetch_sub(layout.size().saturating_sub(new_size), Ordering::Relaxed);
        }
        resized
    }
}

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(
        u8::try_from(rustx::runtime::workflow::run_template_renderer()).unwrap_or(1),
    )
}
