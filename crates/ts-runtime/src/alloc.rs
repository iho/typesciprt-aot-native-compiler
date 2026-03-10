//! Heap allocator stub.
//!
//! Today this is just a thin wrapper around the system malloc.  Later it will
//! be replaced by a proper GC (mark-and-sweep initially, then a moving GC).

use std::alloc::{alloc, dealloc, Layout};

/// Allocate `size` bytes on the TS heap.  Returns a null pointer on failure.
///
/// # Safety
/// The returned pointer must be freed with `ts_free`.
#[no_mangle]
pub unsafe extern "C" fn ts_alloc(size: usize) -> *mut u8 {
    let layout = Layout::from_size_align(size, 8).expect("invalid layout");
    unsafe { alloc(layout) }
}

/// Free a pointer previously obtained from `ts_alloc`.
///
/// # Safety
/// `ptr` must have been returned by `ts_alloc(size)` with the same `size`.
#[no_mangle]
pub unsafe extern "C" fn ts_free(ptr: *mut u8, size: usize) {
    if ptr.is_null() {
        return;
    }
    let layout = Layout::from_size_align(size, 8).expect("invalid layout");
    unsafe { dealloc(ptr, layout) }
}
