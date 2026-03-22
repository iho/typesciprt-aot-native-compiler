//! Stackful coroutine (fiber) support for the JS event loop.
//!
//! Each HTTP/TCP handler runs in a JsFiber — a lightweight stackful coroutine.
//! When TS code calls `ts_promise_await` on a pending promise, the fiber yields
//! its stack back to the Tokio task driving it (`JsFiber::run()`), which then
//! awaits the promise asynchronously without blocking any OS thread.
//!
//! # Execution model
//! ```
//! 50 requests = 50 fibers on 1 LocalSet thread
//! each await: stack-switch (~50 ns) → tokio awaits promise → resume
//! ```
//!
//! Only one fiber runs at a time (cooperative, like a JS event loop), so all
//! JS values remain single-threaded with no synchronisation needed.
//!
//! # Global LocalSet fiber channel
//!
//! `register_local_set_tx` / `schedule_fiber` let any thread post a JsFiber
//! onto the active LocalSet.  The LocalSet drains the channel by spawning each
//! fiber as a new `spawn_local` task.  Used by promise callbacks (`.then()`,
//! `.finally()`) and timer callbacks so they run cooperatively instead of
//! holding the global JS lock on a `spawn_blocking` thread.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock as StdOnceLock;
use corosensei::{Coroutine, CoroutineResult, Yielder};
use corosensei::stack::DefaultStack;

// ── Global LocalSet fiber channel ────────────────────────────────────────────
//
// When `ts_serve` (or any LocalSet-based server) starts, it registers an
// UnboundedSender here.  Any thread can then call `schedule_fiber()` to post
// a JsFiber onto the LocalSet without needing the global JS lock.

static LOCAL_FIBER_TX: StdOnceLock<tokio::sync::mpsc::UnboundedSender<JsFiber>> =
    StdOnceLock::new();

/// Register the LocalSet's fiber intake channel.  Called once from `ts_serve`
/// before entering the event loop.
pub fn register_local_set_tx(tx: tokio::sync::mpsc::UnboundedSender<JsFiber>) {
    // If already set (e.g. server restarted in tests), ignore — the existing
    // sender may or may not still be valid, but we can't overwrite an OnceLock.
    let _ = LOCAL_FIBER_TX.set(tx);
}

/// Returns a clone of the LocalSet sender, or `None` if no LocalSet is active.
pub fn get_local_set_tx() -> Option<tokio::sync::mpsc::UnboundedSender<JsFiber>> {
    LOCAL_FIBER_TX.get().cloned()
}

/// Schedule `fiber` to run on the active LocalSet.  Returns `true` if
/// successfully queued, `false` if no LocalSet is registered (caller should
/// fall back to `spawn_blocking` + JS lock).
pub fn schedule_fiber(fiber: JsFiber) -> bool {
    if let Some(tx) = LOCAL_FIBER_TX.get() {
        tx.send(fiber).is_ok()
    } else {
        false
    }
}

// ── Thread-local suspend handle ───────────────────────────────────────────────

// Raw pointer to the Yielder stored on the fiber's own stack.
// Valid only while the fiber is executing (between resume() and suspend()).
// Null when not currently running inside a fiber.
thread_local! {
    static YIELDER_PTR: Cell<*const Yielder<u64, u64>> =
        Cell::new(std::ptr::null());
}

// ── Fiber stack pool ──────────────────────────────────────────────────────────

// Stack size for fiber stacks: 256 KB is enough for typical TS handler call
// chains while keeping memory usage ~4× lower than the 1 MB default.
const FIBER_STACK_SIZE: usize = 256 * 1024;

// Maximum stacks cached per thread.  Beyond this, returned stacks are dropped
// (munmap'd) immediately so idle threads don't hold too much memory.
const STACK_POOL_MAX: usize = 32;

thread_local! {
    static STACK_POOL: RefCell<Vec<DefaultStack>> =
        RefCell::new(Vec::with_capacity(STACK_POOL_MAX));
}

fn acquire_stack() -> DefaultStack {
    STACK_POOL.with(|pool| {
        pool.borrow_mut().pop()
    }).unwrap_or_else(|| {
        DefaultStack::new(FIBER_STACK_SIZE)
            .expect("failed to allocate fiber stack")
    })
}

fn release_stack(stack: DefaultStack) {
    STACK_POOL.with(|pool| {
        let mut p = pool.borrow_mut();
        if p.len() < STACK_POOL_MAX {
            p.push(stack);
        }
        // If pool is full, `stack` is dropped here (munmap'd).
    });
}

// ── JsFiber ──────────────────────────────────────────────────────────────────

/// A stackful coroutine that runs a JS handler function cooperatively.
pub struct JsFiber {
    inner: Coroutine<u64, u64, u64, DefaultStack>,
}

// Safety: JsFibers are only ever driven inside a tokio::task::LocalSet, which
// ensures they run on a single thread.  The raw-pointer-encoded TsVals they
// capture are safe because the LocalSet pins execution to one thread.
unsafe impl Send for JsFiber {}

impl JsFiber {
    /// Create a fiber that runs `f()` and returns the result as a raw `u64`
    /// (a TsVal bit-pattern).  `f` executes on the fiber's own stack.
    ///
    /// Each fiber automatically gets a 128 KB bump-pointer arena.
    /// Arena allocations skip ARC entirely; the arena is bulk-freed (with proper
    /// destructor calls) when the fiber returns.
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce() -> u64 + Send + 'static,
    {
        let stack = acquire_stack();
        let inner = Coroutine::<u64, u64, u64, DefaultStack>::with_stack(
            stack,
            move |yielder: &Yielder<u64, u64>, _initial: u64| -> u64 {
                // Store the yielder pointer so ts_promise_await can reach it.
                YIELDER_PTR.with(|p| p.set(yielder as *const _));
                // Set up a bump-pointer arena for this fiber.
                unsafe { crate::alloc::arena_enter(); }
                let result = f();
                // Destroy and free the arena (runs all pending destructors).
                unsafe { crate::alloc::arena_exit(); }
                YIELDER_PTR.with(|p| p.set(std::ptr::null()));
                result
            },
        );
        JsFiber { inner }
    }

    /// Drive the fiber to completion, yielding to the Tokio event loop at
    /// every `await` point.  Returns the fiber's final return value.
    pub async fn run(mut self) -> u64 {
        let mut resume_input: u64 = super::UNDEFINED.0;

        loop {
            match self.inner.resume(resume_input) {
                CoroutineResult::Yield(promise_raw) => {
                    // The fiber yielded because ts_promise_await was called.
                    // Wait for the promise asynchronously (no OS thread blocked).
                    resume_input = wait_for_promise(promise_raw).await;
                }
                CoroutineResult::Return(result) => {
                    // Reclaim the stack into the thread-local pool so the next
                    // fiber creation avoids a costly mmap/munmap pair.
                    let stack = self.inner.into_stack();
                    release_stack(stack);
                    return result;
                }
            }
        }
    }
}

// ── Called from ts_promise_await when inside a fiber ─────────────────────────

/// Yield the current fiber, passing `promise_raw` to the driving Tokio task.
/// Returns the resolved value when the fiber is resumed.
///
/// # Safety
/// Must only be called while a fiber is executing (YIELDER_PTR is non-null).
///
/// Saves and restores both `YIELDER_PTR` and `ACTIVE_ARENA` around the yield so
/// that other fibers which run while this one is suspended get a clean TLS state,
/// and this fiber sees its own arena/yielder again on resume.
pub unsafe fn fiber_yield(promise_raw: u64) -> u64 {
    // These locals live on the fiber's own stack, so they are saved and
    // restored automatically as part of the coroutine context switch.
    let yielder_ptr = YIELDER_PTR.with(|p| p.get());
    let arena_ptr   = crate::alloc::ACTIVE_ARENA.with(|a| {
        let p = a.get();
        a.set(std::ptr::null_mut()); // clear so next fiber starts clean
        p
    });
    YIELDER_PTR.with(|p| p.set(std::ptr::null()));

    debug_assert!(!yielder_ptr.is_null(), "fiber_yield called outside fiber context");
    // suspend() saves the current stack frame and returns to JsFiber::run().
    // The next resume() call passes back the resolved value.
    let result = (*yielder_ptr).suspend(promise_raw);

    // Restore this fiber's TLS state after being resumed.
    YIELDER_PTR.with(|p| p.set(yielder_ptr));
    crate::alloc::ACTIVE_ARENA.with(|a| a.set(arena_ptr));

    result
}

/// Returns `true` if the calling code is currently running inside a JsFiber.
#[inline]
pub fn in_fiber() -> bool {
    YIELDER_PTR.with(|p| !p.get().is_null())
}

// ── Async promise wait (used by JsFiber::run) ─────────────────────────────────

/// Asynchronously wait for a TsPromise (encoded as a `u64` bit-pattern) to
/// resolve.  Returns the resolved value's bit-pattern.
pub async fn wait_for_promise(promise_raw: u64) -> u64 {
    use crate::value::{TsVal, TsPromise};
    use crate::alloc::ArcHeader;

    let val = TsVal(promise_raw);

    if !val.is_ptr() {
        return promise_raw; // scalar — already a value
    }

    unsafe {
        let ptr = val.as_ptr();
        let hdr_size = std::mem::size_of::<ArcHeader>();
        let hdr = ptr.sub(hdr_size) as *const ArcHeader;

        if (*hdr).tag != 3 {
            // Not a TsPromise — treat the heap object itself as the value
            return promise_raw;
        }

        let p = &*(ptr as *const TsPromise);

        // Fast path: already resolved
        if let Some(&v) = p.resolved.get() {
            return v.0;
        }

        // Slow path: wait for the tokio::sync::Notify signal
        loop {
            if let Some(&v) = p.resolved.get() {
                return v.0;
            }
            p.notify.notified().await;
        }
    }
}
