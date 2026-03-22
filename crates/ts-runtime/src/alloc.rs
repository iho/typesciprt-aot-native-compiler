//! Heap allocator stub.
//!
//! Today this is just a thin wrapper around the system malloc.  Later it will
//! be replaced by a proper GC (mark-and-sweep initially, then a moving GC).

use std::alloc::{alloc, dealloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};

/// Sentinel refcount for immortal (interned) objects.
/// When `ref_count == IMMORTAL_RC`, both `ts_retain` and `ts_release` are no-ops —
/// the object lives forever and is never freed. Used by the string intern table.
pub const IMMORTAL_RC: u32 = u32::MAX;

/// Sentinel refcount for arena-allocated objects.
/// When `ref_count == ARENA_RC`, both `ts_retain` and `ts_release` are no-ops —
/// the object's lifetime is managed by the fiber arena, not by ARC.
/// Destructors are called by `arena_exit` when the arena is freed.
pub const ARENA_RC: u32 = u32::MAX - 1;

// ── Per-fiber bump-pointer arena ─────────────────────────────────────────────

const ARENA_CAPACITY: usize = 64 * 1024; // 64 KB per fiber (pooled and reused)

/// Bump-pointer arena used inside a single JsFiber's lifetime.
///
/// All allocations go in order: [ArcHeader | user-data | ArcHeader | user-data | …].
/// On `arena_exit`, we walk the buffer and call tag-specific destructors so
/// heap-allocated contents (HashMap buckets, Vec elements, …) are properly released
/// before the backing buffer is freed.
pub struct BumpArena {
    base:     *mut u8,
    capacity: usize,
    pub offset:   usize,
}

// Safety: BumpArenas are only accessed on the single LocalSet thread.
unsafe impl Send for BumpArena {}

impl BumpArena {
    pub unsafe fn new() -> Self {
        let layout = Layout::from_size_align(ARENA_CAPACITY, 8).expect("arena layout");
        let base = alloc(layout);
        assert!(!base.is_null(), "BumpArena: backing allocation failed");
        BumpArena { base, capacity: ARENA_CAPACITY, offset: 0 }
    }

    /// Attempt a bump-pointer allocation of `total_size` bytes (header + data).
    /// Returns null if the arena is full; caller must fall back to the heap.
    pub unsafe fn try_alloc(&mut self, total_size: usize) -> *mut u8 {
        let aligned = (self.offset + 7) & !7;
        if aligned + total_size > self.capacity {
            return std::ptr::null_mut();
        }
        let ptr = self.base.add(aligned);
        self.offset = aligned + total_size;
        ptr
    }

    /// Walk every allocation in the arena and call its tag-specific destructor.
    /// This releases all references to heap-allocated data (HashMaps, Vecs, Strings…)
    /// before the backing buffer is freed.
    pub unsafe fn destroy_all(&mut self) {
        let header_size = std::mem::size_of::<ArcHeader>();
        let mut pos: usize = 0;
        while pos + header_size <= self.offset {
            let aligned = (pos + 7) & !7;
            if aligned + header_size > self.offset { break; }
            let hdr = self.base.add(aligned) as *const ArcHeader;
            let tag  = (*hdr).tag;
            let size = (*hdr).size as usize;
            if size == 0 { break; } // safety sentinel
            let data = self.base.add(aligned + header_size);
            call_ts_destructor(tag, data);
            pos = aligned + header_size + size;
        }
    }
}

impl Drop for BumpArena {
    fn drop(&mut self) {
        if !self.base.is_null() {
            let layout = Layout::from_size_align(self.capacity, 8).expect("arena layout");
            unsafe { dealloc(self.base, layout); }
            self.base = std::ptr::null_mut();
        }
    }
}

/// Call the appropriate destructor for an arena-allocated object by heap tag.
/// Mirrors the destructor table in `value/mod.rs ts_release_val`.
unsafe fn call_ts_destructor(tag: u8, ptr: *mut u8) {
    match tag {
        0  => crate::value::object::ts_obj_destructor(ptr),
        1  => crate::value::array::ts_arr_destructor(ptr),
        2  => crate::value::string_val::ts_string_destructor(ptr),
        3  => crate::value::promise::ts_promise_destructor(ptr),
        4  => crate::value::func::ts_func_destructor(ptr),
        5  => crate::value::map::ts_map_destructor(ptr),
        6  => crate::value::regexp::ts_regexp_destructor(ptr),
        7  => crate::value::http::ts_headers_destructor(ptr),
        8  => crate::value::http::ts_response_destructor(ptr),
        9  => crate::value::map::ts_map_destructor(ptr), // URLSearchParams
        10 => crate::value::symbol::ts_symbol_destructor(ptr),
        11 => crate::value::set::ts_set_destructor(ptr),
        12 => crate::value::weak::ts_weakmap_destructor(ptr),
        13 => crate::value::weak::ts_weakset_destructor(ptr),
        14 => crate::value::date::ts_date_destructor(ptr),
        15 => crate::value::weakref::ts_weakref_destructor(ptr),
        16 => crate::node::events::ts_event_emitter_destructor(ptr),
        17 => crate::node::buffer::ts_buffer_destructor(ptr),
        18 => crate::napi::ts_napi_function_destructor(ptr),
        19 => crate::value::http::ts_node_request_destructor(ptr),
        20 => crate::value::url::ts_url_destructor(ptr),
        _  => {}
    }
}

// ── Thread-local active arena ─────────────────────────────────────────────────

thread_local! {
    /// Points to the BumpArena of the currently executing JsFiber.
    /// Null when not inside a fiber (or when the fiber has no arena active).
    /// Saved and restored by `fiber_yield` so interleaved fibers each see their own arena.
    pub static ACTIVE_ARENA: Cell<*mut BumpArena> = Cell::new(std::ptr::null_mut());

    /// One-slot per-thread arena pool. When a JsFiber exits it deposits its arena here
    /// instead of freeing it. The next `arena_enter` on the same thread reclaims it,
    /// avoiding a malloc/free round-trip for every HTTP request.
    static POOLED_ARENA: Cell<*mut BumpArena> = Cell::new(std::ptr::null_mut());
}

/// Called at the start of each JsFiber to set up a fresh arena.
/// Must be balanced by exactly one call to `arena_exit`.
pub unsafe fn arena_enter() {
    // Try to reuse a previously-pooled arena from this thread.
    let arena_ptr = POOLED_ARENA.with(|cell| {
        let ptr = cell.get();
        if !ptr.is_null() {
            cell.set(std::ptr::null_mut());
            ptr
        } else {
            std::ptr::null_mut()
        }
    });
    let arena_ptr = if arena_ptr.is_null() {
        Box::into_raw(Box::new(BumpArena::new()))
    } else {
        // Reset offset: memory is ready to reuse (destroy_all was already called on exit).
        (*arena_ptr).offset = 0;
        arena_ptr
    };
    ACTIVE_ARENA.with(|cell| cell.set(arena_ptr));
}

/// Called at the end of each JsFiber to destroy and free the arena.
/// Runs all pending destructors first so heap references are properly released.
pub unsafe fn arena_exit() {
    ACTIVE_ARENA.with(|cell| {
        let ptr = cell.get();
        if ptr.is_null() { return; }
        cell.set(std::ptr::null_mut());
        (*ptr).destroy_all();
        // Reset offset so the arena is ready for reuse.
        (*ptr).offset = 0;
        // Deposit into the per-thread pool instead of freeing.
        POOLED_ARENA.with(|pool| {
            let old = pool.get();
            if old.is_null() {
                pool.set(ptr);
            } else {
                // Pool already occupied — free this one.
                drop(Box::from_raw(ptr));
            }
        });
    });
}

/// Allocate `size` bytes from the current fiber's arena, falling back to the heap.
/// Arena allocations get `ARENA_RC` so `ts_retain`/`ts_release` are no-ops on them.
#[no_mangle]
pub unsafe extern "C" fn ts_alloc_arena(size: usize, tag: u8) -> *mut u8 {
    let header_size = std::mem::size_of::<ArcHeader>();
    let total = (header_size + size + 7) & !7;

    let arena_ptr = ACTIVE_ARENA.with(|cell| cell.get());
    if !arena_ptr.is_null() {
        let slot = (*arena_ptr).try_alloc(total);
        if !slot.is_null() {
            let hdr = slot as *mut ArcHeader;
            (*hdr).ref_count.store(ARENA_RC, Ordering::Relaxed);
            (*hdr).size = size as u32;
            (*hdr).tag  = tag;
            return slot.add(header_size);
        }
        // Arena full — fall through to heap.
    }
    ts_alloc_rc(size, tag)
}

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
    let header_ptr = ptr.sub(header_size) as *mut ArcHeader;
    let cur_rc = (*header_ptr).ref_count.load(Ordering::Relaxed);
    if cur_rc == IMMORTAL_RC || cur_rc == ARENA_RC { return; }
    #[cfg(debug_assertions)]
    if cur_rc == 0 || cur_rc == 0xDEAD_BEEFu32 || cur_rc > 0x100_0000 {
        eprintln!("ts_retain: retaining freed/corrupted object at ptr={:p} rc={:#x}", ptr, cur_rc);
        let bt = std::backtrace::Backtrace::capture();
        eprintln!("{}", bt);
        std::process::abort();
    }
    (*header_ptr).ref_count.fetch_add(1, Ordering::Relaxed);
}

/// Decrement the reference count. If it reaches zero, call `destructor` and free.
#[no_mangle]
pub unsafe extern "C" fn ts_release(ptr: *mut u8, destructor: Option<unsafe extern "C" fn(*mut u8)>) {
    if ptr.is_null() { return; }
    let header_size = std::mem::size_of::<ArcHeader>();
    let header_ptr = unsafe { ptr.sub(header_size) } as *mut ArcHeader;

    // Check for immortal/arena BEFORE fetch_sub to avoid corrupting the sentinels.
    let pre_rc = (*header_ptr).ref_count.load(Ordering::Relaxed);
    if pre_rc == IMMORTAL_RC || pre_rc == ARENA_RC { return; }

    // Release ordering: ensures all prior writes to the object are visible to
    // the thread that finally drops it (which uses Acquire when checking rc == 1).
    let old_rc = (*header_ptr).ref_count.fetch_sub(1, Ordering::Release);

    #[cfg(debug_assertions)]
    if old_rc == 0 || old_rc == 0xDEAD_BEEFu32 || old_rc > 0x100_0000 {
        eprintln!("ts_release: double-free/use-after-free detected at ptr={:p} old_rc={:#x}", ptr, old_rc);
        let header_start = ptr.sub(header_size);
        eprintln!("  Memory dump (header to data+48):");
        for i in 0..8usize {
            let p = header_start.add(i * 8) as *const u64;
            eprintln!("  [{:+4}] {:p} = {:#018x}", (i as isize * 8) - (header_size as isize), p, *p);
        }
        let bt = std::backtrace::Backtrace::capture();
        eprintln!("{}", bt);
        std::process::abort();
    }

    if old_rc == 1 {
        // Acquire fence: synchronize with all Release decrements from other threads
        // before we run the destructor and dealloc.
        std::sync::atomic::fence(Ordering::Acquire);
        let size = (*header_ptr).size as usize;
        if let Some(dtor) = destructor {
            dtor(ptr);
        }
        let total_size = header_size + size;
        let layout = Layout::from_size_align(total_size, 8).expect("invalid layout");
        #[cfg(debug_assertions)]
        (*header_ptr).ref_count.store(0xDEAD_BEEFu32, Ordering::Relaxed);
        dealloc(header_ptr as *mut u8, layout);
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
