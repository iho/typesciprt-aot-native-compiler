//! TsSet: heap-allocated JavaScript Set (tag = 11).
//!
//! Stores unique values using same-value-zero equality (pointer identity for
//! heap objects, bitwise equality for all others).  Insertion order is preserved.

use super::{TsVal, TsSet, TsArray, NULL, UNDEFINED, ts_retain_val, ts_release_val, heap_tag};
use super::array::{ts_arr_new, ts_arr_push, ts_arr_set};
use super::func::dispatch_callback;
use super::operators::ts_val_strict_eq;

/// Same-value-zero equality used by Set membership tests.
unsafe fn svz_eq(a: TsVal, b: TsVal) -> bool {
    ts_val_strict_eq(a, b) != 0
}

pub unsafe extern "C" fn ts_set_destructor(ptr: *mut u8) {
    let set = &mut *(ptr as *mut TsSet);
    for val in set.entries.drain(..) {
        ts_release_val(val);
    }
    std::ptr::drop_in_place(ptr as *mut TsSet);
}

/// `new Set()` — create an empty Set.
#[no_mangle]
pub unsafe extern "C" fn ts_set_new() -> TsVal {
    let size = std::mem::size_of::<TsSet>();
    let ptr = crate::alloc::ts_alloc_rc(size, 11) as *mut TsSet; // tag 11 = Set
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsSet { entries: Vec::new() });
    TsVal::from_ptr(ptr as *mut u8)
}

/// `new Set(iterable)` — create a Set pre-populated from a TsArray or TsString iterable.
#[no_mangle]
pub unsafe extern "C" fn ts_set_new_from_iter(iter: TsVal) -> TsVal {
    let set_val = ts_set_new();
    if iter.is_ptr() {
        match heap_tag(iter) {
            1 => {
                // TsArray: add each element
                let arr = &*(iter.as_ptr() as *const TsArray);
                let elems: Vec<TsVal> = arr.elements.clone();
                for el in elems {
                    let result = ts_set_add(set_val, el);
                    ts_release_val(result);
                }
            }
            2 => {
                // TsString: add each Unicode character
                use super::TsString;
                use super::uri::rust_str_to_val;
                let ts_str = &*(iter.as_ptr() as *const TsString);
                let chars: Vec<String> = ts_str.inner.chars().map(|c| c.to_string()).collect();
                for ch in chars {
                    let ch_val = rust_str_to_val(ch);
                    let result = ts_set_add(set_val, ch_val);
                    ts_release_val(result);
                    ts_release_val(ch_val);
                }
            }
            _ => {}
        }
    }
    set_val
}

/// `set.add(val)` — add a value if not already present; returns the Set (owned ref).
#[no_mangle]
pub unsafe extern "C" fn ts_set_add(set_val: TsVal, val: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 11 {
        ts_retain_val(set_val);
        return set_val;
    }
    let set = &mut *(set_val.as_ptr() as *mut TsSet);
    if !set.entries.iter().any(|&e| svz_eq(e, val)) {
        ts_retain_val(val);
        set.entries.push(val);
    }
    ts_retain_val(set_val);
    set_val
}

/// `set.has(val)` — returns boolean TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_set_has(set_val: TsVal, val: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 11 {
        return TsVal::from_bool(false);
    }
    let set = &*(set_val.as_ptr() as *const TsSet);
    TsVal::from_bool(set.entries.iter().any(|&e| svz_eq(e, val)))
}

/// `set.delete(val)` — remove a value; returns true if it was present.
#[no_mangle]
pub unsafe extern "C" fn ts_set_delete(set_val: TsVal, val: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 11 {
        return TsVal::from_bool(false);
    }
    let set = &mut *(set_val.as_ptr() as *mut TsSet);
    if let Some(pos) = set.entries.iter().position(|&e| svz_eq(e, val)) {
        let removed = set.entries.remove(pos);
        ts_release_val(removed);
        TsVal::from_bool(true)
    } else {
        TsVal::from_bool(false)
    }
}

/// `set.clear()` — remove all values.
#[no_mangle]
pub unsafe extern "C" fn ts_set_clear(set_val: TsVal) {
    if !set_val.is_ptr() || heap_tag(set_val) != 11 { return; }
    let set = &mut *(set_val.as_ptr() as *mut TsSet);
    for val in set.entries.drain(..) {
        ts_release_val(val);
    }
}

/// `set.size` — number of elements as integer TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_set_size(set_val: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 11 {
        return TsVal::from_i32(0);
    }
    let set = &*(set_val.as_ptr() as *const TsSet);
    TsVal::from_i32(set.entries.len() as i32)
}

/// `set.values()` — returns a TsArray of all values (in insertion order).
#[no_mangle]
pub unsafe extern "C" fn ts_set_values(set_val: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 11 {
        return ts_arr_new(0);
    }
    let set = &*(set_val.as_ptr() as *const TsSet);
    let arr = ts_arr_new(set.entries.len() as i32);
    for &v in &set.entries {
        ts_arr_push(arr, v);
    }
    arr
}

/// `set.keys()` — same as values() (Set keys === values).
#[no_mangle]
pub unsafe extern "C" fn ts_set_keys(set_val: TsVal) -> TsVal {
    ts_set_values(set_val)
}

/// `set.entries()` — returns a TsArray of [value, value] pairs for destructuring.
#[no_mangle]
pub unsafe extern "C" fn ts_set_entries(set_val: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 11 {
        return ts_arr_new(0);
    }
    let set = &*(set_val.as_ptr() as *const TsSet);
    let outer = ts_arr_new(set.entries.len() as i32);
    for (i, &v) in set.entries.iter().enumerate() {
        let pair = ts_arr_new(2);
        ts_arr_set(pair, 0, v);
        ts_arr_set(pair, 1, v);
        ts_arr_set(outer, i as i32, pair);
        ts_release_val(pair);
    }
    outer
}

/// `set.forEach(cb)` — call `cb(value, value, set)` for each value in insertion order.
#[no_mangle]
pub unsafe extern "C" fn ts_set_for_each(set_val: TsVal, callback: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 11 {
        return UNDEFINED;
    }
    // Snapshot to allow mutation during iteration
    let entries: Vec<TsVal> = {
        let set = &*(set_val.as_ptr() as *const TsSet);
        let v = set.entries.clone();
        for &e in &v { ts_retain_val(e); }
        v
    };
    for val in entries {
        let result = dispatch_callback(callback, &[val, val, set_val]);
        ts_release_val(result);
        ts_release_val(val);
    }
    UNDEFINED
}
