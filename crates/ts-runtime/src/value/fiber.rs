//! Stackful coroutine (fiber) support for the JS event loop.
//!
//! Each HTTP/TCP handler runs in a JsFiber — a lightweight stackful coroutine.
//! When TS code calls `ts_promise_await` on a pending promise, the fiber yields
//! its stack back to the Tokio task driving it (`JsFiber::run()`), which then
//! awaits the promise asynchronously without blocking any OS thread.
//!
//! # Before (spawn_blocking + global JS lock)
//! ```
//! 50 requests = 50 OS threads competing for 1 Mutex
//! each await: release lock → Condvar::wait (~100 µs) → acquire lock
//! ```
//!
//! # After (fiber + LocalSet)
//! ```
//! 50 requests = 50 fibers on 1 LocalSet thread
//! each await: stack-switch (~50 ns) → tokio awaits promise → resume
//! ```
//!
//! Only one fiber runs at a time (cooperative, like a JS event loop), so all
//! JS values remain single-threaded with no synchronisation needed.

use std::cell::{Cell, RefCell};
use corosensei::{Coroutine, CoroutineResult, Yielder};
use corosensei::stack::DefaultStack;

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
                let result = f();
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
pub unsafe fn fiber_yield(promise_raw: u64) -> u64 {
    let ptr = YIELDER_PTR.with(|p| p.get());
    debug_assert!(!ptr.is_null(), "fiber_yield called outside fiber context");
    // suspend() saves the current stack frame and returns to resume().
    // The next resume() call passes back the resolved value as the return.
    (*ptr).suspend(promise_raw)
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
