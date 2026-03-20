//! Heap allocator stub.
//!
//! Today this is just a thin wrapper around the system malloc.  Later it will
//! be replaced by a proper GC (mark-and-sweep initially, then a moving GC).

use std::alloc::{alloc, dealloc, Layout};
use std::sync::atomic::{AtomicU32, Ordering};

/// Header for heap-allocated objects with ARC.
///
/// Reduced from 24 bytes to 16 bytes:
///   - `ref_count`: AtomicU32 (4 bytes) — supports up to 4B references
///   - `size`: u32 (4 bytes) — supports objects up to 4 GB
///   - `tag`: u8 (1 byte)
///   - `_pad`: 7 bytes to reach 16, ensuring user data (at offset 16) is 8-byte aligned
///     when the overall allocation starts at an 8-byte aligned address.
#[repr(C)]
pub struct ArcHeader {
    pub ref_count: AtomicU32,
    pub size: u32,
    pub tag: u8,
    _pad: [u8; 7],
}

/// Allocate `size` bytes on the TS heap with an ARC header and a type tag.
/// Returns a pointer to the storage *after* the header.
#[no_mangle]
pub unsafe extern "C" fn ts_alloc_rc(size: usize, tag: u8) -> *mut u8 {
    let header_size = std::mem::size_of::<ArcHeader>();
    let total_size = header_size + size;
    // Align to 8 so that user data (at offset `header_size` = 16) is also 8-byte aligned.
    let layout = Layout::from_size_align(total_size, 8).expect("invalid layout");

    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        return std::ptr::null_mut();
    }

    let header = ptr as *mut ArcHeader;
    unsafe {
        (*header).ref_count.store(1, Ordering::Relaxed);
        (*header).size = size as u32;
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
    let cur_rc = unsafe { (*header_ptr).ref_count.load(Ordering::Relaxed) };
    if cur_rc == 0 || cur_rc == 0xDEAD_BEEFu32 || cur_rc > 0x100_0000 {
        eprintln!("ts_retain: retaining freed/corrupted object at ptr={:p} rc={:#x}", ptr, cur_rc);
        let bt = std::backtrace::Backtrace::capture();
        eprintln!("{}", bt);
        std::process::abort();
    }
    unsafe {
        (*header_ptr).ref_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Decrement the reference count. If it reaches zero, call `destructor` and free.
#[no_mangle]
pub unsafe extern "C" fn ts_release(ptr: *mut u8, destructor: Option<unsafe extern "C" fn(*mut u8)>) {
    if ptr.is_null() { return; }
    let header_size = std::mem::size_of::<ArcHeader>();
    let header_ptr = unsafe { ptr.sub(header_size) } as *mut ArcHeader;

    // Release ordering: ensures all prior writes to the object are visible to
    // the thread that finally drops it (which uses Acquire when checking rc == 1).
    let old_rc = unsafe {
        (*header_ptr).ref_count.fetch_sub(1, Ordering::Release)
    };

    if old_rc == 0 || old_rc == 0xDEAD_BEEFu32 || old_rc > 0x100_0000 {
        // Double-free detected: refcount was already 0 or poisoned (use-after-free).
        eprintln!("ts_release: double-free/use-after-free detected at ptr={:p} old_rc={:#x}", ptr, old_rc);
        // Dump memory around the ptr for context
        let header_start = ptr.sub(header_size);
        eprintln!("  Memory dump (header to data+48):");
        for i in 0..8usize {
            let p = header_start.add(i * 8) as *const u64;
            eprintln!("  [{:+4}] {:p} = {:#018x}", (i as isize * 8) - (header_size as isize), p, unsafe { *p });
        }
        let bt = std::backtrace::Backtrace::capture();
        eprintln!("{}", bt);
        std::process::abort();
    }

    if old_rc == 1 {
        // Acquire fence: synchronize with all Release decrements from other threads
        // before we run the destructor and dealloc.
        std::sync::atomic::fence(Ordering::Acquire);
        if let Some(dtor) = destructor {
            unsafe { dtor(ptr) };
        }
        let size = unsafe { (*header_ptr).size } as usize;
        let total_size = header_size + size;
        let layout = Layout::from_size_align(total_size, 8).expect("invalid layout");
        // Poison the refcount to catch use-after-free.
        unsafe { (*header_ptr).ref_count.store(0xDEAD_BEEFu32, Ordering::Relaxed); }
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
