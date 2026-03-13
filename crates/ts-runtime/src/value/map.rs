//! TsMap: heap-allocated TypeScript Map and its operations.

use super::{TsVal, TsMap, TsString, UNDEFINED, NULL, heap_tag, ts_retain_val, ts_release_val};
use super::array::{ts_arr_new, ts_arr_set, ts_arr_push};
use super::func::dispatch_callback;

pub unsafe extern "C" fn ts_map_destructor(ptr: *mut u8) {
    let map = &mut *(ptr as *mut TsMap);
    for (k, v) in map.entries.drain(..) {
        ts_release_val(k);
        ts_release_val(v);
    }
    std::ptr::drop_in_place(ptr as *mut TsMap);
}

/// Compare two TsVals for Map key equality (strict same-value semantics).
pub(super) unsafe fn map_key_eq(a: TsVal, b: TsVal) -> bool {
    if a.0 == b.0 { return true; }
    // String content equality
    if a.is_ptr() && heap_tag(a) == 2 && b.is_ptr() && heap_tag(b) == 2 {
        let sa = &*(a.as_ptr() as *const TsString);
        let sb = &*(b.as_ptr() as *const TsString);
        return sa.inner == sb.inner;
    }
    false
}

/// `new Map()` — create an empty Map.
#[no_mangle]
pub unsafe extern "C" fn ts_map_new() -> TsVal {
    let size = std::mem::size_of::<TsMap>();
    let ptr = crate::alloc::ts_alloc_rc(size, 5) as *mut TsMap; // tag 5 = Map
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsMap { entries: Vec::new() });
    TsVal::from_ptr(ptr as *mut u8)
}

/// `map.set(key, val)` — insert or update a key; returns the map (owned ref).
/// Also handles TsHeaders (tag=7) and URLSearchParams (tag=9) which have the same Vec layout.
#[no_mangle]
pub unsafe extern "C" fn ts_map_set(map_val: TsVal, key: TsVal, val: TsVal) -> TsVal {
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 {
        ts_retain_val(map_val);
        return map_val;
    }
    let map = &mut *(map_val.as_ptr() as *mut TsMap);
    for entry in &mut map.entries {
        if map_key_eq(entry.0, key) {
            ts_retain_val(val);
            let old = entry.1;
            entry.1 = val;
            ts_release_val(old);
            ts_retain_val(map_val);
            return map_val;
        }
    }
    ts_retain_val(key);
    ts_retain_val(val);
    map.entries.push((key, val));
    ts_retain_val(map_val);
    map_val
}

/// `map.get(key)` — retrieve value or undefined. Also handles TsHeaders (tag=7) and URLSearchParams (tag=9).
#[no_mangle]
pub unsafe extern "C" fn ts_map_get(map_val: TsVal, key: TsVal) -> TsVal {
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 { return UNDEFINED; }
    let map = &*(map_val.as_ptr() as *const TsMap);
    for (k, v) in &map.entries {
        if map_key_eq(*k, key) {
            ts_retain_val(*v);
            return *v;
        }
    }
    UNDEFINED
}

/// `map.has(key)` — true if key is present. Also handles TsHeaders (tag=7) and URLSearchParams (tag=9).
#[no_mangle]
pub unsafe extern "C" fn ts_map_has(map_val: TsVal, key: TsVal) -> TsVal {
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 { return TsVal::from_bool(false); }
    let map = &*(map_val.as_ptr() as *const TsMap);
    for (k, _) in &map.entries {
        if map_key_eq(*k, key) { return TsVal::from_bool(true); }
    }
    TsVal::from_bool(false)
}

/// `map.delete(key)` — remove a key; returns true if it was present. Also handles TsHeaders (tag=7) and URLSearchParams (tag=9).
#[no_mangle]
pub unsafe extern "C" fn ts_map_delete(map_val: TsVal, key: TsVal) -> TsVal {
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 { return TsVal::from_bool(false); }
    let map = &mut *(map_val.as_ptr() as *mut TsMap);
    let pos = map.entries.iter().position(|(k, _)| map_key_eq(*k, key));
    if let Some(i) = pos {
        let (k, v) = map.entries.remove(i);
        ts_release_val(k);
        ts_release_val(v);
        return TsVal::from_bool(true);
    }
    TsVal::from_bool(false)
}

/// `map.clear()` — remove all entries. Also handles TsHeaders (tag=7) and URLSearchParams (tag=9).
#[no_mangle]
pub unsafe extern "C" fn ts_map_clear(map_val: TsVal) {
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 { return; }
    let map = &mut *(map_val.as_ptr() as *mut TsMap);
    for (k, v) in map.entries.drain(..) {
        ts_release_val(k);
        ts_release_val(v);
    }
}

/// `map.size` — number of entries as integer TsVal. Also handles TsHeaders (tag=7) and URLSearchParams (tag=9).
#[no_mangle]
pub unsafe extern "C" fn ts_map_size(map_val: TsVal) -> TsVal {
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 { return TsVal::from_i32(0); }
    let map = &*(map_val.as_ptr() as *const TsMap);
    TsVal::from_i32(map.entries.len() as i32)
}

/// `map.keys()` — returns a TsArray of all keys (owned refs). Also handles TsHeaders (tag=7) and URLSearchParams (tag=9).
#[no_mangle]
pub unsafe extern "C" fn ts_map_keys(map_val: TsVal) -> TsVal {
    let result = ts_arr_new(0);
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 { return result; }
    let map = &*(map_val.as_ptr() as *const TsMap);
    for (k, _) in &map.entries {
        ts_arr_push(result, *k);
    }
    result
}

/// `map.values()` — returns a TsArray of all values (owned refs). Also handles TsHeaders (tag=7) and URLSearchParams (tag=9).
#[no_mangle]
pub unsafe extern "C" fn ts_map_values(map_val: TsVal) -> TsVal {
    let result = ts_arr_new(0);
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 { return result; }
    let map = &*(map_val.as_ptr() as *const TsMap);
    for (_, v) in &map.entries {
        ts_arr_push(result, *v);
    }
    result
}

/// `map.forEach(cb)` — call cb(value, key, map) for each entry. Also handles TsHeaders (tag=7) and URLSearchParams (tag=9).
#[no_mangle]
pub unsafe extern "C" fn ts_map_for_each(map_val: TsVal, callback: TsVal) -> TsVal {
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 { return UNDEFINED; }
    let map = &*(map_val.as_ptr() as *const TsMap);
    let len = map.entries.len();
    for i in 0..len {
        let (k, v) = { let m = &*(map_val.as_ptr() as *const TsMap); m.entries[i] };
        dispatch_callback(callback, &[v, k, map_val]);
    }
    UNDEFINED
}

/// Returns a TsArray of 2-element TsArrays: [[k1,v1], [k2,v2], ...].
/// Used for `for (const [k, v] of m.entries())`.
#[no_mangle]
pub unsafe extern "C" fn ts_map_entries(map_val: TsVal) -> TsVal {
    let tag = if map_val.is_ptr() { heap_tag(map_val) } else { 255 };
    if tag != 5 && tag != 7 && tag != 9 {
        return ts_arr_new(0);
    }
    let map = &*(map_val.as_ptr() as *const TsMap);
    let n = map.entries.len() as i32;
    let result = ts_arr_new(n);
    for (i, (k, v)) in map.entries.iter().enumerate() {
        let pair = ts_arr_new(2);
        ts_retain_val(*k);
        ts_arr_set(pair, 0, *k);
        ts_retain_val(*v);
        ts_arr_set(pair, 1, *v);
        ts_arr_set(result, i as i32, pair);
        ts_release_val(pair);
    }
    result
}
