//! TsPromise: async/await support via Tokio.

use super::{TsVal, TsPromise, TsArray, UNDEFINED, NULL, heap_tag, ts_retain_val, ts_release_val};
use super::array::{ts_arr_new, ts_arr_get, ts_arr_push};

// ── Global Tokio runtime ──────────────────────────────────────────────────────

static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

pub(super) fn get_runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create Tokio runtime")
    })
}

// ── Promise internals ─────────────────────────────────────────────────────────

const TAG_PROMISE_ALLOC: u8 = 3;

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
    ts_release_val(arr);
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
