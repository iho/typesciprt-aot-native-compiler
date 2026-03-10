//! Thread-local exception state for TypeScript try/catch/throw.
//!
//! JavaScript exceptions are modelled as a thread-local flag + value.
//! `ts_throw` sets the flag; `ts_check_exception` tests it; `ts_catch_exception`
//! clears it and returns the thrown value (caller takes ownership).

use crate::value::{TsVal, UNDEFINED};
use crate::value::{ts_retain_val, ts_release_val};

use std::cell::Cell;

thread_local! {
    /// Raw bits of the currently pending exception (or UNDEFINED if none).
    static EXC_VAL:    Cell<u64>  = Cell::new(UNDEFINED.0);
    /// True when an exception is pending.
    static EXC_ACTIVE: Cell<bool> = Cell::new(false);
}

/// Raise a JavaScript exception.  Sets the thread-local exception state.
/// The previous exception (if any) is released before storing the new one.
#[no_mangle]
pub unsafe extern "C" fn ts_throw(val: TsVal) {
    // Release any existing uncaught exception.
    if EXC_ACTIVE.with(|c| c.get()) {
        let old = TsVal(EXC_VAL.with(|c| c.get()));
        ts_release_val(old);
    }
    ts_retain_val(val);
    EXC_VAL.with(|c| c.set(val.0));
    EXC_ACTIVE.with(|c| c.set(true));
}

/// Return 1 if an exception is pending, 0 otherwise.
#[no_mangle]
pub extern "C" fn ts_check_exception() -> i32 {
    if EXC_ACTIVE.with(|c| c.get()) { 1 } else { 0 }
}

/// Clear the pending exception and return its value.
/// The caller receives ownership of the returned TsVal (it was retained
/// by `ts_throw` and must eventually be released by the caller).
#[no_mangle]
pub extern "C" fn ts_catch_exception() -> TsVal {
    let val = TsVal(EXC_VAL.with(|c| c.get()));
    EXC_ACTIVE.with(|c| c.set(false));
    EXC_VAL.with(|c| c.set(UNDEFINED.0));
    val
}
