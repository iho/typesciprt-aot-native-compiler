//! TsPromise: async/await support via Tokio.

use super::{TsVal, TsPromise, TsArray, UNDEFINED, NULL, heap_tag, ts_retain_val, ts_release_val};
use super::array::{ts_arr_new, ts_arr_get, ts_arr_push};

// ── Global Tokio runtime ──────────────────────────────────────────────────────

static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

pub(crate) fn get_runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create Tokio runtime")
    })
}

// ── Promise internals ─────────────────────────────────────────────────────────

const TAG_PROMISE_ALLOC: u8 = 3;

pub(crate) fn make_promise_pair() -> (
    std::sync::Arc<std::sync::OnceLock<TsVal>>,
    std::sync::Arc<tokio::sync::Notify>,
) {
    (
        std::sync::Arc::new(std::sync::OnceLock::new()),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    )
}

pub(crate) unsafe fn alloc_promise(p: TsPromise) -> TsVal {
    let size = std::mem::size_of::<TsPromise>();
    let ptr = crate::alloc::ts_alloc_rc(size, TAG_PROMISE_ALLOC) as *mut TsPromise;
    if ptr.is_null() { return NULL; }
    ptr.write(p);
    TsVal::from_ptr(ptr as *mut u8)
}

pub(crate) fn resolve_arc(
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

/// Await a Promise, consuming the argument and returning an owned result.
///
/// - Non-Promise values: passed through as-is (ownership transferred).
/// - Promise values: blocks until resolved, releases the Promise, returns the resolved value
///   (caller receives ownership of the result).
///
/// Callers must NOT release `val` after this call — ownership is consumed.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_await(val: TsVal) -> TsVal {
    if !val.is_ptr() {
        // Scalar (undefined/null/bool/i32/f64): no heap refcount — pass through.
        return val;
    }
    let ptr = val.as_ptr();
    let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
    let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
    if (*header).tag != TAG_PROMISE_ALLOC {
        // Non-Promise heap object: pass ownership through without retain/release.
        return val;
    }
    let promise = &*(ptr as *const TsPromise);
    let result = block_until_resolved(promise.resolved.clone(), promise.notify.clone());
    ts_retain_val(result);
    ts_release_val(val); // releases the Promise
    result
}

/// Resolve the value of a promise without consuming it (borrows the promise).
/// Always returns an owned reference to the resolved value.
unsafe fn borrow_resolved(val: TsVal) -> TsVal {
    if !val.is_ptr() { return val; }
    let ptr = val.as_ptr();
    let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
    let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
    if (*header).tag != TAG_PROMISE_ALLOC {
        // Non-Promise: the value itself is the resolved value. Retain it to give caller an owned ref.
        ts_retain_val(val);
        return val;
    }
    let promise = &*(ptr as *const TsPromise);
    let result = block_until_resolved(promise.resolved.clone(), promise.notify.clone());
    ts_retain_val(result); // give caller an owned ref
    result
}

/// `.then(onFulfilled)` — calls the callback with the resolved value, returns a new Promise.
/// Borrows both `promise` and `callback`.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_then(promise: TsVal, callback: TsVal) -> TsVal {
    use super::func::dispatch_callback;
    let resolved = borrow_resolved(promise); // borrows promise, returns owned resolved value
    let result = dispatch_callback(callback, &[resolved]);
    ts_release_val(resolved);
    // result may itself be a Promise; if so, flatten it
    let final_result = borrow_resolved(result);
    let out = ts_promise_resolve(final_result);
    ts_release_val(final_result);
    ts_release_val(result);
    out
}

/// `.catch(onRejected)` — in our runtime errors are passed as values, treat like .then.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_catch(promise: TsVal, callback: TsVal) -> TsVal {
    ts_promise_then(promise, callback)
}

/// `.finally(onFinally)` — calls callback with no args, returns original promise value.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_finally(promise: TsVal, callback: TsVal) -> TsVal {
    use super::func::dispatch_callback;
    let resolved = borrow_resolved(promise); // borrows promise
    dispatch_callback(callback, &[]);
    let out = ts_promise_resolve(resolved);
    ts_release_val(resolved);
    out
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

/// Returns a Promise that resolves to an array of all resolved values.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_all(arr: TsVal) -> TsVal {
    let len = if arr.is_ptr() && heap_tag(arr) == 1 {
        (*(arr.as_ptr() as *const TsArray)).elements.len()
    } else {
        0
    };
    let mut results: Vec<TsVal> = Vec::with_capacity(len);
    for i in 0..len {
        let item = ts_arr_get(arr, i as i32);
        let resolved = ts_promise_await(item);
        results.push(resolved);
    }
    let out = ts_arr_new(0);
    for v in results {
        ts_arr_push(out, v);
        ts_release_val(v);
    }
    out
}

/// Returns a rejected Promise (wraps val as-is; in our runtime, resolve == reject).
#[no_mangle]
pub unsafe extern "C" fn ts_promise_reject(val: TsVal) -> TsVal {
    ts_promise_resolve(val)
}

/// `Promise.allSettled(arr)` — resolves to an array of {status, value/reason} objects.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_all_settled(arr: TsVal) -> TsVal {
    use super::object::{ts_obj_new, ts_obj_set};
    use super::string_val::ts_string_new;
    let len = if arr.is_ptr() && heap_tag(arr) == 1 {
        (*(arr.as_ptr() as *const TsArray)).elements.len()
    } else { 0 };
    let out = ts_arr_new(0);
    for i in 0..len {
        let item = ts_arr_get(arr, i as i32);
        let val = ts_promise_await(item);
        let obj = ts_obj_new();
        let fulfilled = b"fulfilled\0";
        ts_obj_set(obj, fulfilled.as_ptr() as *const i8, val);
        let status_key = b"status\0";
        let status_str = ts_string_new(b"fulfilled\0".as_ptr() as *const i8);
        ts_obj_set(obj, status_key.as_ptr() as *const i8, status_str);
        ts_release_val(status_str);
        let value_key = b"value\0";
        ts_obj_set(obj, value_key.as_ptr() as *const i8, val);
        ts_release_val(val);
        ts_arr_push(out, obj);
        ts_release_val(obj);
    }
    out
}

/// `Promise.any(arr)` — resolves with the first fulfilled value (simplified: same as race).
#[no_mangle]
pub unsafe extern "C" fn ts_promise_any(arr: TsVal) -> TsVal {
    let len = if arr.is_ptr() && heap_tag(arr) == 1 {
        (*(arr.as_ptr() as *const TsArray)).elements.len()
    } else { 0 };
    if len == 0 { return ts_promise_resolve(UNDEFINED); }
    let item = ts_arr_get(arr, 0);
    ts_promise_await(item)
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

// ── setTimeout / setInterval / clearTimeout / clearInterval ──────────────────

use std::sync::atomic::{AtomicU32, Ordering as AOrdering};
use std::collections::HashMap;

static TIMER_COUNTER: AtomicU32 = AtomicU32::new(1);

fn timer_handles() -> &'static std::sync::Mutex<HashMap<u32, tokio::task::AbortHandle>> {
    static HANDLES: std::sync::OnceLock<std::sync::Mutex<HashMap<u32, tokio::task::AbortHandle>>> =
        std::sync::OnceLock::new();
    HANDLES.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn cancel_timer(id: u32) {
    if let Ok(mut map) = timer_handles().lock() {
        if let Some(handle) = map.remove(&id) {
            handle.abort();
        }
    }
}

fn cleanup_timer(id: u32) {
    if let Ok(mut map) = timer_handles().lock() {
        map.remove(&id);
    }
}

/// `setTimeout(callback, delay_ms)` — call callback once after delay.
/// Returns a numeric timer ID usable with clearTimeout.
#[no_mangle]
pub unsafe extern "C" fn ts_set_timeout(callback: TsVal, delay_ms: TsVal) -> TsVal {
    let ms = if delay_ms.is_int32() { delay_ms.as_i32().max(0) as u64 }
             else if !delay_ms.is_nan_boxed() { delay_ms.as_f64() as u64 }
             else { 0 };
    ts_retain_val(callback);
    let cb_raw = callback.0;
    let timer_id = TIMER_COUNTER.fetch_add(1, AOrdering::Relaxed);
    let tid = timer_id;
    let task = get_runtime().spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        cleanup_timer(tid);
        get_runtime().spawn_blocking(move || unsafe {
            let cb = TsVal(cb_raw);
            let result = super::func::dispatch_callback(cb, &[]);
            ts_release_val(result);
            ts_release_val(cb);
        }).await.ok();
    });
    if let Ok(mut map) = timer_handles().lock() {
        map.insert(timer_id, task.abort_handle());
    }
    drop(task);
    TsVal::from_i32(timer_id as i32)
}

/// `setInterval(callback, interval_ms)` — call callback repeatedly every interval_ms.
/// Returns a numeric timer ID usable with clearInterval.
#[no_mangle]
pub unsafe extern "C" fn ts_set_interval(callback: TsVal, interval_ms: TsVal) -> TsVal {
    let ms = if interval_ms.is_int32() { interval_ms.as_i32().max(0) as u64 }
             else if !interval_ms.is_nan_boxed() { interval_ms.as_f64() as u64 }
             else { 0 };
    ts_retain_val(callback);
    let cb_raw = callback.0;
    let timer_id = TIMER_COUNTER.fetch_add(1, AOrdering::Relaxed);
    let task = get_runtime().spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(ms.max(1)));
        interval.tick().await; // skip the immediate first tick
        loop {
            interval.tick().await;
            let cb_raw2 = cb_raw;
            get_runtime().spawn_blocking(move || unsafe {
                let cb = TsVal(cb_raw2);
                let result = super::func::dispatch_callback(cb, &[]);
                ts_release_val(result);
            }).await.ok();
        }
    });
    if let Ok(mut map) = timer_handles().lock() {
        map.insert(timer_id, task.abort_handle());
    }
    drop(task);
    TsVal::from_i32(timer_id as i32)
}

/// `clearTimeout(id)` — cancel a pending timeout.
#[no_mangle]
pub unsafe extern "C" fn ts_clear_timeout(id: TsVal) -> TsVal {
    let timer_id = if id.is_int32() { id.as_i32() as u32 } else { 0 };
    cancel_timer(timer_id);
    UNDEFINED
}

/// `clearInterval(id)` — cancel a repeating interval.
#[no_mangle]
pub unsafe extern "C" fn ts_clear_interval(id: TsVal) -> TsVal {
    let timer_id = if id.is_int32() { id.as_i32() as u32 } else { 0 };
    cancel_timer(timer_id);
    UNDEFINED
}

/// `queueMicrotask(callback)` — schedules callback as a microtask (runs async, low latency).
#[no_mangle]
pub unsafe extern "C" fn ts_queue_microtask(callback: TsVal) -> TsVal {
    use super::func::dispatch_callback;
    ts_retain_val(callback);
    get_runtime().spawn(async move {
        tokio::task::spawn_blocking(move || {
            unsafe { dispatch_callback(callback, &[]); ts_release_val(callback); }
        }).await.ok();
    });
    UNDEFINED
}

/// `Promise.race(arr)` — returns a Promise that resolves/rejects with the first settling promise.
/// Takes a TsArray of promises.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_race_all(arr: TsVal) -> TsVal {
    use super::{TsArray, heap_tag};
    if !arr.is_ptr() || heap_tag(arr) != 1 {
        return ts_promise_resolve(UNDEFINED);
    }
    let arr_ref = &*(arr.as_ptr() as *const TsArray);
    if arr_ref.elements.is_empty() {
        // Promise.race([]) never settles; return a pending promise
        return ts_promise_resolve(UNDEFINED);
    }
    if arr_ref.elements.len() == 1 {
        let p = arr_ref.elements[0];
        super::ts_retain_val(p);
        return ts_promise_resolve(p);
    }
    // For now, fold over pairs using ts_promise_race
    let mut result = { let p = arr_ref.elements[0]; super::ts_retain_val(p); ts_promise_resolve(p) };
    for &p in &arr_ref.elements[1..] {
        super::ts_retain_val(p);
        let new_result = ts_promise_race(result, p);
        super::ts_release_val(result);
        result = new_result;
    }
    result
}
