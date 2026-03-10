//! Heap allocator stub.
//!
//! Today this is just a thin wrapper around the system malloc.  Later it will
//! be replaced by a proper GC (mark-and-sweep initially, then a moving GC).

use std::alloc::{alloc, dealloc, Layout};
use std::sync::atomic::{AtomicU64, Ordering};

/// Header for heap-allocated objects with ARC.
#[repr(C)]
pub struct ArcHeader {
    pub ref_count: AtomicU64,
    pub size: usize,
    pub tag: u8, // 0 = Object, 1 = Array, 2 = String
}

/// Allocate `size` bytes on the TS heap with an ARC header and a type tag.
/// Returns a pointer to the storage *after* the header.
#[no_mangle]
pub unsafe extern "C" fn ts_alloc_rc(size: usize, tag: u8) -> *mut u8 {
    let header_size = std::mem::size_of::<ArcHeader>();
    let total_size = header_size + size;
    let layout = Layout::from_size_align(total_size, 8).expect("invalid layout");
    
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    
    let header = ptr as *mut ArcHeader;
    unsafe {
        (*header).ref_count.store(1, Ordering::SeqCst);
        (*header).size = size;
        (*header).tag = tag;
    }
    
    unsafe { ptr.add(header_size) }
}


/// Increment the reference count of the object.
#[no_mangle]
pub unsafe extern "C" fn ts_retain(ptr: *mut u8) {
    if ptr.is_null() { return; }
    let header_size = std::mem::size_of::<ArcHeader>();
    let header_ptr = unsafe { ptr.sub(header_size) } as *mut ArcHeader;
    unsafe {
        (*header_ptr).ref_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Decrement the reference count. If it reaches zero, call `destructor` and free.
#[no_mangle]
pub unsafe extern "C" fn ts_release(ptr: *mut u8, destructor: Option<unsafe extern "C" fn(*mut u8)>) {
    if ptr.is_null() { return; }
    let header_size = std::mem::size_of::<ArcHeader>();
    let header_ptr = unsafe { ptr.sub(header_size) } as *mut ArcHeader;
    
    let old_rc = unsafe {
        (*header_ptr).ref_count.fetch_sub(1, Ordering::SeqCst)
    };
    
    if old_rc == 1 {
        // Refcount reached zero.
        if let Some(dtor) = destructor {
            unsafe { dtor(ptr) };
        }
        let size = unsafe { (*header_ptr).size };
        let total_size = header_size + size;
        let layout = Layout::from_size_align(total_size, 8).expect("invalid layout");
        unsafe { dealloc(header_ptr as *mut u8, layout) };
    }
}


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
