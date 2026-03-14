//! TsWeakMap (tag=12) and TsWeakSet (tag=13).
//!
//! WeakMap and WeakSet hold WEAK references to their keys/members: the ARC
//! reference count of a key is NOT incremented when it is inserted.  This means
//! an object can be freed even while it is still a key in a WeakMap/WeakSet.
//! After the key is freed, lookups by that pointer address will simply find
//! no matching entry (the raw pointer is no longer valid, but we only compare
//! addresses — we never dereference a potentially-stale key pointer).
//!
//! WeakMap values ARE strongly retained (WeakMap owns the values, not the keys).
//! WeakSet entries are entirely unretained (membership-only, no value stored).

use super::{TsVal, TsWeakMap, TsWeakSet, NULL, UNDEFINED, ts_retain_val, ts_release_val, heap_tag};

// ── WeakMap ───────────────────────────────────────────────────────────────────

pub unsafe extern "C" fn ts_weakmap_destructor(ptr: *mut u8) {
    let map = &mut *(ptr as *mut TsWeakMap);
    // Release values (NOT keys — they are held weakly)
    for (_, val) in map.entries.drain(..) {
        ts_release_val(val);
    }
    std::ptr::drop_in_place(ptr as *mut TsWeakMap);
}

/// `new WeakMap()` — create an empty WeakMap.
#[no_mangle]
pub unsafe extern "C" fn ts_weakmap_new() -> TsVal {
    let size = std::mem::size_of::<TsWeakMap>();
    let ptr = crate::alloc::ts_alloc_rc(size, 12) as *mut TsWeakMap; // tag 12 = WeakMap
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsWeakMap { entries: Vec::new() });
    TsVal::from_ptr(ptr as *mut u8)
}

/// `weakmap.set(key, val)` — insert or update.  Key must be a heap object.
/// Returns the WeakMap (owned ref).
#[no_mangle]
pub unsafe extern "C" fn ts_weakmap_set(map_val: TsVal, key: TsVal, val: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 12 {
        ts_retain_val(map_val);
        return map_val;
    }
    if !key.is_ptr() {
        // WeakMap keys must be objects; silently ignore primitive keys
        ts_retain_val(map_val);
        return map_val;
    }
    let key_ptr = key.as_ptr();
    let map = &mut *(map_val.as_ptr() as *mut TsWeakMap);
    for entry in &mut map.entries {
        if entry.0 == key_ptr {
            ts_retain_val(val);
            let old = entry.1;
            entry.1 = val;
            ts_release_val(old);
            ts_retain_val(map_val);
            return map_val;
        }
    }
    ts_retain_val(val); // strongly retain value; do NOT retain key
    map.entries.push((key_ptr, val));
    ts_retain_val(map_val);
    map_val
}

/// `weakmap.get(key)` — return value or undefined.
#[no_mangle]
pub unsafe extern "C" fn ts_weakmap_get(map_val: TsVal, key: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 12 || !key.is_ptr() {
        return UNDEFINED;
    }
    let key_ptr = key.as_ptr();
    let map = &*(map_val.as_ptr() as *const TsWeakMap);
    for &(k, v) in &map.entries {
        if k == key_ptr {
            ts_retain_val(v);
            return v;
        }
    }
    UNDEFINED
}

/// `weakmap.has(key)` — boolean TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_weakmap_has(map_val: TsVal, key: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 12 || !key.is_ptr() {
        return TsVal::from_bool(false);
    }
    let key_ptr = key.as_ptr();
    let map = &*(map_val.as_ptr() as *const TsWeakMap);
    TsVal::from_bool(map.entries.iter().any(|&(k, _)| k == key_ptr))
}

/// `weakmap.delete(key)` — returns true if key was present.
#[no_mangle]
pub unsafe extern "C" fn ts_weakmap_delete(map_val: TsVal, key: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 12 || !key.is_ptr() {
        return TsVal::from_bool(false);
    }
    let key_ptr = key.as_ptr();
    let map = &mut *(map_val.as_ptr() as *mut TsWeakMap);
    if let Some(pos) = map.entries.iter().position(|&(k, _)| k == key_ptr) {
        let (_, val) = map.entries.remove(pos);
        ts_release_val(val);
        TsVal::from_bool(true)
    } else {
        TsVal::from_bool(false)
    }
}

// ── WeakSet ───────────────────────────────────────────────────────────────────

pub unsafe extern "C" fn ts_weakset_destructor(ptr: *mut u8) {
    // Entries are raw pointers held weakly — no releases needed
    std::ptr::drop_in_place(ptr as *mut TsWeakSet);
}

/// `new WeakSet()` — create an empty WeakSet.
#[no_mangle]
pub unsafe extern "C" fn ts_weakset_new() -> TsVal {
    let size = std::mem::size_of::<TsWeakSet>();
    let ptr = crate::alloc::ts_alloc_rc(size, 13) as *mut TsWeakSet; // tag 13 = WeakSet
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsWeakSet { entries: Vec::new() });
    TsVal::from_ptr(ptr as *mut u8)
}

/// `weakset.add(val)` — add object to the set; returns the WeakSet (owned ref).
#[no_mangle]
pub unsafe extern "C" fn ts_weakset_add(set_val: TsVal, val: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 13 {
        ts_retain_val(set_val);
        return set_val;
    }
    if !val.is_ptr() {
        // WeakSet members must be objects
        ts_retain_val(set_val);
        return set_val;
    }
    let val_ptr = val.as_ptr();
    let set = &mut *(set_val.as_ptr() as *mut TsWeakSet);
    if !set.entries.contains(&val_ptr) {
        set.entries.push(val_ptr); // no retain — weak reference
    }
    ts_retain_val(set_val);
    set_val
}

/// `weakset.has(val)` — boolean TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_weakset_has(set_val: TsVal, val: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 13 || !val.is_ptr() {
        return TsVal::from_bool(false);
    }
    let val_ptr = val.as_ptr();
    let set = &*(set_val.as_ptr() as *const TsWeakSet);
    TsVal::from_bool(set.entries.contains(&val_ptr))
}

/// `weakset.delete(val)` — returns true if val was present.
#[no_mangle]
pub unsafe extern "C" fn ts_weakset_delete(set_val: TsVal, val: TsVal) -> TsVal {
    if !set_val.is_ptr() || heap_tag(set_val) != 13 || !val.is_ptr() {
        return TsVal::from_bool(false);
    }
    let val_ptr = val.as_ptr();
    let set = &mut *(set_val.as_ptr() as *mut TsWeakSet);
    if let Some(pos) = set.entries.iter().position(|&p| p == val_ptr) {
        set.entries.remove(pos);
        TsVal::from_bool(true)
    } else {
        TsVal::from_bool(false)
    }
}
