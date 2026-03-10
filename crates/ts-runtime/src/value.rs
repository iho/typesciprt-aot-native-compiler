//! The universal TypeScript value representation.
//!
//! TypeScript values are NaN-boxed 64-bit words (similar to JavaScriptCore /
//! V8 Smi encoding).  This module defines the tag bits and boxing/unboxing
//! helpers that the compiler will emit calls to.
//!
//! Layout (NaN-boxing):
//!   - If the top 13 bits are all 1 (quiet NaN range) and bit 50 is 1 →
//!     tagged pointer or special value.
//!   - Otherwise → IEEE-754 double (JS `number`).
//!
//! Tags (bits 49..48 of the quiet-NaN word):
//!   00 → undefined / null (bit 47: 0=undefined, 1=null)
//!   01 → boolean         (bit 0: value)
//!   10 → pointer to heap object
//!   11 → small integer (int32 in lower 32 bits)

/// Opaque 64-bit value type.  The compiler treats every TS value as a `TsVal`.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TsVal(pub u64);

// Quiet NaN mask: bits 63..51 all set, plus the quiet bit (bit 50).
const NAN_MASK:       u64 = 0x7FF8_0000_0000_0000;
const TAG_MASK:       u64 = 0x0006_0000_0000_0000;
const TAG_UNDEFINED:  u64 = 0x0000_0000_0000_0000;
const TAG_NULL:       u64 = 0x0001_0000_0000_0000;
const TAG_BOOL:       u64 = 0x0002_0000_0000_0000;
const TAG_PTR:        u64 = 0x0004_0000_0000_0000;
const TAG_INT:        u64 = 0x0006_0000_0000_0000;

pub const UNDEFINED: TsVal = TsVal(NAN_MASK | TAG_UNDEFINED);
pub const NULL:      TsVal = TsVal(NAN_MASK | TAG_NULL);
pub const TRUE:      TsVal = TsVal(NAN_MASK | TAG_BOOL | 1);
pub const FALSE:     TsVal = TsVal(NAN_MASK | TAG_BOOL | 0);

// TsVal is a plain u64 with ARC-managed heap pointers — safe to transfer across threads.
unsafe impl Send for TsVal {}
unsafe impl Sync for TsVal {}

impl TsVal {
    #[inline]
    pub fn from_f64(n: f64) -> Self {
        Self(n.to_bits())
    }

    #[inline]
    pub fn from_i32(n: i32) -> Self {
        Self(NAN_MASK | TAG_INT | (n as u32 as u64))
    }

    #[inline]
    pub fn from_bool(b: bool) -> Self {
        if b { TRUE } else { FALSE }
    }

    #[inline]
    pub fn from_ptr(p: *mut u8) -> Self {
        Self(NAN_MASK | TAG_PTR | (p as u64 & 0x0000_FFFF_FFFF_FFFF))
    }

    #[inline]
    fn is_nan_boxed(self) -> bool {
        (self.0 & NAN_MASK) == NAN_MASK
    }

    #[inline]
    pub fn is_number(self) -> bool {
        !self.is_nan_boxed()
    }

    #[inline]
    pub fn is_undefined(self) -> bool {
        self.is_nan_boxed() && (self.0 & (TAG_MASK | 1)) == TAG_UNDEFINED
    }

    #[inline]
    pub fn is_null(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_NULL
    }

    #[inline]
    pub fn is_bool(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_BOOL
    }

    #[inline]
    pub fn is_ptr(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_PTR
    }

    #[inline]
    pub fn is_int32(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_INT
    }

    #[inline]
    pub fn as_f64(self) -> f64 {
        f64::from_bits(self.0)
    }

    #[inline]
    pub fn as_i32(self) -> i32 {
        (self.0 & 0x0000_0000_FFFF_FFFF) as i32
    }

    #[inline]
    pub fn as_bool(self) -> bool {
        (self.0 & 1) != 0
    }

    #[inline]
    pub fn as_ptr(self) -> *mut u8 {
        (self.0 & 0x0000_FFFF_FFFF_FFFF) as *mut u8
    }
}

// ── Heap Objects ──────────────────────────────────────────────────────────────

use std::collections::HashMap;
use crate::alloc::ts_alloc_rc;

/// A heap-allocated TypeScript object.
pub struct TsObject {
    pub properties: HashMap<String, TsVal>,
}

/// A heap-allocated TypeScript array.
pub struct TsArray {
    pub elements: Vec<TsVal>,
}

/// A heap-allocated TypeScript string.
pub struct TsString {
    pub inner: String,
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_new() -> TsVal {
    let size = std::mem::size_of::<TsObject>();
    let ptr = ts_alloc_rc(size, 0) as *mut TsObject; // tag 0 = Object
    if ptr.is_null() {
        return NULL;
    }
    unsafe {
        std::ptr::write(ptr, TsObject {
            properties: HashMap::new(),
        });
    }
    TsVal::from_ptr(ptr as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_get(obj_val: TsVal, key_ptr: *const i8) -> TsVal {
    let ptr = obj_val.as_ptr();
    if !ptr.is_null() && !key_ptr.is_null() {
        let obj = ptr as *mut TsObject;
        let key = unsafe { std::ffi::CStr::from_ptr(key_ptr) }.to_string_lossy().into_owned();
        if let Some(&val) = (&*obj).properties.get(&key) {
            ts_retain_val(val);
            return val;
        }
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_set(obj_val: TsVal, key_ptr: *const i8, val: TsVal) {
    let ptr = obj_val.as_ptr();
    if !ptr.is_null() && !key_ptr.is_null() {
        let obj = ptr as *mut TsObject;
        let key = unsafe { std::ffi::CStr::from_ptr(key_ptr) }.to_string_lossy().into_owned();
        
        // ARC: retain new value
        ts_retain_val(val);
        
        let old_val = (&mut *obj).properties.insert(key, val);
        if let Some(v) = old_val {
            ts_release_val(v);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_new(capacity: i32) -> TsVal {
    let size = std::mem::size_of::<TsArray>();
    let ptr = ts_alloc_rc(size, 1) as *mut TsArray; // tag 1 = Array
    if ptr.is_null() {
        return NULL;
    }
    unsafe {
        std::ptr::write(ptr, TsArray {
            elements: Vec::with_capacity(capacity as usize),
        });
        // Initialize with undefined.
        for _ in 0..capacity {
            (&mut *ptr).elements.push(UNDEFINED);
        }
    }
    TsVal::from_ptr(ptr as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn ts_string_new(c_str: *const i8) -> TsVal {
    let s = unsafe { std::ffi::CStr::from_ptr(c_str) }.to_string_lossy().into_owned();
    let size = std::mem::size_of::<TsString>();
    let ptr = ts_alloc_rc(size, 2) as *mut TsString; // tag 2 = String
    if ptr.is_null() {
        return NULL;
    }
    unsafe {
        std::ptr::write(ptr, TsString { inner: s });
    }
    TsVal::from_ptr(ptr as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn ts_string_concat(v1: TsVal, v2: TsVal) -> TsVal {
    let p1 = v1.as_ptr() as *mut TsString;
    let p2 = v2.as_ptr() as *mut TsString;
    
    let s1 = unsafe { &(*p1).inner };
    let s2 = unsafe { &(*p2).inner };
    
    let new_s = format!("{}{}", s1, s2);
    let size = std::mem::size_of::<TsString>();
    let ptr = ts_alloc_rc(size, 2) as *mut TsString;
    if ptr.is_null() {
        return NULL;
    }
    unsafe {
        std::ptr::write(ptr, TsString { inner: new_s });
    }
    TsVal::from_ptr(ptr as *mut u8)
}

/// Polymorphic `+`: integer add if both TAG_INT, otherwise string concat.
#[no_mangle]
pub unsafe extern "C" fn ts_add(a: TsVal, b: TsVal) -> TsVal {
    if a.is_int32() && b.is_int32() {
        TsVal::from_i32(a.as_i32().wrapping_add(b.as_i32()))
    } else {
        ts_string_concat(a, b)
    }
}

// ── Global Tokio runtime ──────────────────────────────────────────────────────

static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

fn get_runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create Tokio runtime")
    })
}

// ── Promise ───────────────────────────────────────────────────────────────────

const TAG_PROMISE_ALLOC: u8 = 3;

/// Heap-allocated TypeScript Promise.
/// The inner value lives in an `OnceLock`; a `Notify` wakes any blockers.
pub struct TsPromise {
    resolved: std::sync::Arc<std::sync::OnceLock<TsVal>>,
    notify:   std::sync::Arc<tokio::sync::Notify>,
}

fn make_promise_pair() -> (
    std::sync::Arc<std::sync::OnceLock<TsVal>>,
    std::sync::Arc<tokio::sync::Notify>,
) {
    (
        std::sync::Arc::new(std::sync::OnceLock::new()),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    )
}

unsafe fn alloc_promise(p: TsPromise) -> TsVal {
    let size = std::mem::size_of::<TsPromise>();
    let ptr = crate::alloc::ts_alloc_rc(size, TAG_PROMISE_ALLOC) as *mut TsPromise;
    if ptr.is_null() { return NULL; }
    ptr.write(p);
    TsVal::from_ptr(ptr as *mut u8)
}

fn resolve_arc(
    resolved: &std::sync::Arc<std::sync::OnceLock<TsVal>>,
    notify:   &std::sync::Arc<tokio::sync::Notify>,
    val: TsVal,
) {
    let _ = resolved.set(val);
    notify.notify_waiters();
}

/// Wait until the promise's OnceLock is set, then return the value.
/// Works both from the main thread and from inside `spawn_blocking` tasks.
fn block_until_resolved(
    resolved: std::sync::Arc<std::sync::OnceLock<TsVal>>,
    notify:   std::sync::Arc<tokio::sync::Notify>,
) -> TsVal {
    if let Some(&v) = resolved.get() { return v; }
    let fut = async move {
        loop {
            if let Some(&v) = resolved.get() { return v; }
            notify.notified().await;
        }
    };
    // Inside spawn_blocking we have a Handle but are NOT in an async context,
    // so Handle::block_on is the right tool. From the main thread, use the
    // global runtime directly.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(fut),
        Err(_)     => get_runtime().block_on(fut),
    }
}

/// Wrap `val` in a resolved Promise (heap object, alloc tag 3).
#[no_mangle]
pub unsafe extern "C" fn ts_promise_resolve(val: TsVal) -> TsVal {
    let (resolved, notify) = make_promise_pair();
    ts_retain_val(val); // promise owns a reference
    let _ = resolved.set(val);
    alloc_promise(TsPromise { resolved, notify })
}

/// Await a Promise, blocking until resolved. Non-Promise values pass through.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_await(val: TsVal) -> TsVal {
    if !val.is_ptr() {
        ts_retain_val(val);
        return val;
    }
    let ptr = val.as_ptr();
    let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
    let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
    if (*header).tag != TAG_PROMISE_ALLOC {
        ts_retain_val(val);
        return val;
    }
    let promise = &*(ptr as *const TsPromise);
    let result = block_until_resolved(promise.resolved.clone(), promise.notify.clone());
    ts_retain_val(result);
    ts_release_val(val); // may run ts_promise_destructor
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_promise_destructor(ptr: *mut u8) {
    let p = ptr as *mut TsPromise;
    if let Some(&val) = (*p).resolved.get() {
        ts_release_val(val);
    }
    std::ptr::drop_in_place(p);
}

// ── sleep ─────────────────────────────────────────────────────────────────────

/// Returns a Promise<undefined> that resolves after `ms` milliseconds.
#[no_mangle]
pub unsafe extern "C" fn ts_sleep(ms: i32) -> TsVal {
    let (resolved, notify) = make_promise_pair();
    let r2 = resolved.clone();
    let n2 = notify.clone();
    get_runtime().spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ms.max(0) as u64)).await;
        resolve_arc(&r2, &n2, UNDEFINED);
    });
    alloc_promise(TsPromise { resolved, notify })
}

// ── Promise.race ──────────────────────────────────────────────────────────────

async fn wait_for_promise(
    resolved: std::sync::Arc<std::sync::OnceLock<TsVal>>,
    notify:   std::sync::Arc<tokio::sync::Notify>,
) -> TsVal {
    loop {
        if let Some(&v) = resolved.get() { return v; }
        notify.notified().await;
    }
}

/// Returns a Promise that resolves to whichever of `p1` / `p2` resolves first.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_race(p1: TsVal, p2: TsVal) -> TsVal {
    unsafe fn promise_arcs(v: TsVal) -> Option<(
        std::sync::Arc<std::sync::OnceLock<TsVal>>,
        std::sync::Arc<tokio::sync::Notify>,
    )> {
        if !v.is_ptr() { return None; }
        let ptr = v.as_ptr();
        let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
        let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
        if (*header).tag != TAG_PROMISE_ALLOC { return None; }
        let p = &*(ptr as *const TsPromise);
        Some((p.resolved.clone(), p.notify.clone()))
    }

    let (res1, not1) = match promise_arcs(p1) {
        Some(pair) => pair,
        None => { ts_release_val(p2); ts_retain_val(p1); return ts_promise_resolve(p1); }
    };
    let (res2, not2) = match promise_arcs(p2) {
        Some(pair) => pair,
        None => { ts_release_val(p1); ts_retain_val(p2); return ts_promise_resolve(p2); }
    };

    // Fast path: already resolved.
    if let Some(&v) = res1.get() { ts_release_val(p2); return ts_promise_resolve(v); }
    if let Some(&v) = res2.get() { ts_release_val(p1); return ts_promise_resolve(v); }

    let (rr, rn) = make_promise_pair();
    let rr2 = rr.clone();
    let rn2 = rn.clone();
    let p1_raw = p1.0;
    let p2_raw = p2.0;
    // Keep both promises alive while the race task runs.
    ts_retain_val(p1);
    ts_retain_val(p2);

    get_runtime().spawn(async move {
        let result = tokio::select! {
            v = wait_for_promise(res1, not1) => v,
            v = wait_for_promise(res2, not2) => v,
        };
        // Retain for the result promise's ownership.
        unsafe { ts_retain_val(result); }
        resolve_arc(&rr2, &rn2, result);
        unsafe {
            ts_release_val(TsVal(p1_raw));
            ts_release_val(TsVal(p2_raw));
        }
    });

    alloc_promise(TsPromise { resolved: rr, notify: rn })
}

// ── Async spawn (spawn_blocking + function pointer) ───────────────────────────

type AsyncFn0 = unsafe extern "C" fn() -> u64;
type AsyncFn1 = unsafe extern "C" fn(i32) -> u64;
type AsyncFn2 = unsafe extern "C" fn(i32, i32) -> u64;
type AsyncFn3 = unsafe extern "C" fn(i32, i32, i32) -> u64;
type AsyncFn4 = unsafe extern "C" fn(i32, i32, i32, i32) -> u64;

fn do_spawn<F: FnOnce() -> u64 + Send + 'static>(f: F) -> TsVal {
    let (resolved, notify) = make_promise_pair();
    let r2 = resolved.clone();
    let n2 = notify.clone();
    get_runtime().spawn_blocking(move || {
        let raw = f();
        resolve_arc(&r2, &n2, TsVal(raw));
    });
    unsafe { alloc_promise(TsPromise { resolved, notify }) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn0(fn_ptr: *const u8) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn0 = std::mem::transmute(addr); f() })
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn1(fn_ptr: *const u8, a0: i32) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn1 = std::mem::transmute(addr); f(a0) })
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn2(fn_ptr: *const u8, a0: i32, a1: i32) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn2 = std::mem::transmute(addr); f(a0, a1) })
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn3(fn_ptr: *const u8, a0: i32, a1: i32, a2: i32) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn3 = std::mem::transmute(addr); f(a0, a1, a2) })
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn4(fn_ptr: *const u8, a0: i32, a1: i32, a2: i32, a3: i32) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn4 = std::mem::transmute(addr); f(a0, a1, a2, a3) })
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_get(arr_val: TsVal, idx: i32) -> TsVal {
    let ptr = arr_val.as_ptr();
    if !ptr.is_null() && idx >= 0 {
        let arr = ptr as *mut TsArray;
        let idx = idx as usize;
        if idx < (&*arr).elements.len() {
            let val = (&*arr).elements[idx];
            ts_retain_val(val);
            return val;
        }
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_set(arr_val: TsVal, idx: i32, val: TsVal) {
    let ptr = arr_val.as_ptr();
    if !ptr.is_null() && idx >= 0 {
        let arr = ptr as *mut TsArray;
        let idx = idx as usize;

        // ARC: Retain new value.
        ts_retain_val(val);

        if idx < (&*arr).elements.len() {
            let old_val = std::mem::replace(&mut (&mut *arr).elements[idx], val);
            ts_release_val(old_val);
        } else {
            // A real JS array would resize/pad.
            if idx == (&*arr).elements.len() {
                (&mut *arr).elements.push(val);
            } else {
                // Pad with undefined.
                while (&*arr).elements.len() < idx {
                    (&mut *arr).elements.push(UNDEFINED);
                }
                (&mut *arr).elements.push(val);
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_len(arr_val: TsVal) -> TsVal {
    let ptr = arr_val.as_ptr();
    if !ptr.is_null() {
        let arr = ptr as *mut TsArray;
        return TsVal::from_i32((&*arr).elements.len() as i32);
    }
    TsVal::from_i32(0)
}

#[no_mangle]
pub unsafe extern "C" fn ts_retain_val(val: TsVal) {
    if val.is_ptr() {
        crate::alloc::ts_retain(val.as_ptr());
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_release_val(val: TsVal) {
    if val.is_ptr() {
        let ptr = val.as_ptr();
        let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
        let header = unsafe { ptr.sub(header_size) as *mut crate::alloc::ArcHeader };
        let tag = unsafe { (*header).tag };
        
        let destructor = match tag {
            0 => Some(ts_obj_destructor as unsafe extern "C" fn(*mut u8)),
            1 => Some(ts_arr_destructor as unsafe extern "C" fn(*mut u8)),
            2 => Some(ts_string_destructor as unsafe extern "C" fn(*mut u8)),
            3 => Some(ts_promise_destructor as unsafe extern "C" fn(*mut u8)),
            _ => None,
        };
        
        crate::alloc::ts_release(ptr, destructor);
    }
}

/// Destructor for TsObject.
#[no_mangle]
pub unsafe extern "C" fn ts_obj_destructor(ptr: *mut u8) {
    let obj_ptr = ptr as *mut TsObject;
    unsafe {
        for (_, val) in (*obj_ptr).properties.drain() {
            ts_release_val(val);
        }
        std::ptr::drop_in_place(obj_ptr);
    }
}

/// Destructor for TsArray.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_destructor(ptr: *mut u8) {
    let arr_ptr = ptr as *mut TsArray;
    unsafe {
        for val in (*arr_ptr).elements.drain(..) {
            ts_release_val(val);
        }
        std::ptr::drop_in_place(arr_ptr);
    }
}

/// Destructor for TsString.
#[no_mangle]
pub unsafe extern "C" fn ts_string_destructor(ptr: *mut u8) {
    let s_ptr = ptr as *mut TsString;
    unsafe {
        std::ptr::drop_in_place(s_ptr);
    }
}

// ── Type introspection ────────────────────────────────────────────────────────

/// Read the heap-allocation tag (0=Object,1=Array,2=String,3=Promise) for a
/// pointer TsVal **without** modifying the reference count.
/// Returns 255 if `val` is not a heap pointer.
unsafe fn heap_tag(val: TsVal) -> u8 {
    if !val.is_ptr() { return 255; }
    let ptr = val.as_ptr();
    let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
    let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
    (*header).tag
}

/// Returns a new string TsVal describing the JavaScript `typeof` the value.
/// Caller receives an owned reference (refcount already bumped inside
/// `ts_string_new`).
#[no_mangle]
pub unsafe extern "C" fn ts_typeof(val: TsVal) -> TsVal {
    let type_bytes: &'static [u8] = if !val.is_nan_boxed() {
        b"number\0"
    } else {
        match val.0 & TAG_MASK {
            TAG_UNDEFINED => b"undefined\0",
            TAG_NULL      => b"object\0",    // historical JS semantics
            TAG_BOOL      => b"boolean\0",
            TAG_INT       => b"number\0",
            TAG_PTR       => match heap_tag(val) {
                2 => b"string\0",
                _ => b"object\0",
            },
            _ => b"undefined\0",
        }
    };
    ts_string_new(type_bytes.as_ptr() as *const i8)
}

/// Strict equality (`===`).  Returns 1 if equal, 0 otherwise.
/// For strings the content is compared; for all other types the bit patterns
/// must be identical (covers numbers, booleans, undefined, null, and object
/// identity).
#[no_mangle]
pub unsafe extern "C" fn ts_val_strict_eq(a: TsVal, b: TsVal) -> i32 {
    if a.0 == b.0 { return 1; }
    if a.is_ptr() && b.is_ptr() && heap_tag(a) == 2 && heap_tag(b) == 2 {
        let sa = &*(a.as_ptr() as *const TsString);
        let sb = &*(b.as_ptr() as *const TsString);
        return if sa.inner == sb.inner { 1 } else { 0 };
    }
    0
}




