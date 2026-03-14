//! TsSymbol: heap-allocated JavaScript Symbol (tag = 10).
//!
//! Symbols are unique opaque identity values.  Each `ts_symbol_new()` call returns
//! a distinct heap-allocated object with a monotonically-increasing `id`.
//! When used as an object property key the id is encoded as `"\x01sym{id}"` in the
//! HashMap, which prevents collisions with any user-visible string key.

use std::sync::atomic::{AtomicU64, Ordering};
use super::{TsVal, TsSymbol, NULL, UNDEFINED, ts_retain_val, ts_release_val, heap_tag};

static SYMBOL_COUNTER: AtomicU64 = AtomicU64::new(1);

pub unsafe extern "C" fn ts_symbol_destructor(ptr: *mut u8) {
    let sym = &mut *(ptr as *mut TsSymbol);
    ts_release_val(sym.description);
    std::ptr::drop_in_place(ptr as *mut TsSymbol);
}

/// `Symbol(description?)` — allocate a fresh, unique Symbol heap object.
#[no_mangle]
pub unsafe extern "C" fn ts_symbol_new(description: TsVal) -> TsVal {
    let id = SYMBOL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let size = std::mem::size_of::<TsSymbol>();
    let ptr = crate::alloc::ts_alloc_rc(size, 10) as *mut TsSymbol; // tag 10 = Symbol
    if ptr.is_null() { return NULL; }
    ts_retain_val(description);
    std::ptr::write(ptr, TsSymbol { id, description });
    TsVal::from_ptr(ptr as *mut u8)
}

/// `sym.description` — return the description string (or undefined).
#[no_mangle]
pub unsafe extern "C" fn ts_symbol_description(sym_val: TsVal) -> TsVal {
    if !sym_val.is_ptr() || heap_tag(sym_val) != 10 {
        return UNDEFINED;
    }
    let sym = &*(sym_val.as_ptr() as *const TsSymbol);
    ts_retain_val(sym.description);
    sym.description
}

/// Convert a Symbol TsVal to the internal property-key string `"\x01sym{id}"`.
/// Returns `None` if `val` is not a Symbol.
pub(super) unsafe fn symbol_to_key_string(val: TsVal) -> Option<String> {
    if val.is_ptr() && heap_tag(val) == 10 {
        let sym = &*(val.as_ptr() as *const TsSymbol);
        Some(format!("\x01sym{}", sym.id))
    } else {
        None
    }
}
