//! Polymorphic container dispatch.
//!
//! The codegen emits a single function call for each container method
//! (has, delete, get, set/add, clear, size, keys, values, entries, forEach)
//! regardless of the runtime type of the receiver.  The functions here
//! inspect the heap tag and delegate to the concrete implementation.
//!
//! Heap tags handled:
//!   1  = TsArray  (forEach only)
//!   5  = TsMap
//!   7  = TsHeaders (get, set, has, delete)
//!   9  = URLSearchParams (get, set, has, delete)
//!  11  = TsSet
//!  12  = TsWeakMap
//!  13  = TsWeakSet

use super::{TsVal, UNDEFINED, ts_retain_val, ts_release_val, heap_tag};
use super::map::{
    ts_map_get, ts_map_set, ts_map_has, ts_map_delete, ts_map_clear,
    ts_map_size, ts_map_keys, ts_map_values, ts_map_entries, ts_map_for_each,
};
use super::object::ts_obj_get;
use super::func::{ts_method_call0, ts_method_call1, ts_method_call2, ts_method_call3};
use super::set::{
    ts_set_add, ts_set_has, ts_set_delete, ts_set_clear,
    ts_set_size, ts_set_keys, ts_set_values, ts_set_entries, ts_set_for_each,
};
use super::weak::{
    ts_weakmap_get, ts_weakmap_set, ts_weakmap_has, ts_weakmap_delete,
    ts_weakset_add, ts_weakset_has, ts_weakset_delete,
};
use super::array::{ts_arr_for_each, ts_arr_keys, ts_arr_values, ts_arr_entries};

/// Dynamic method lookup helper for user class instances (heap tag 0).
/// Looks up `method_name` on the object and calls it with up to 3 args.
unsafe fn obj_dynamic_call(obj: TsVal, method_name: &[u8], a0: TsVal, a1: TsVal, a2: TsVal, n_args: usize) -> TsVal {
    let fn_val = ts_obj_get(obj, method_name.as_ptr() as *const i8);
    let result = match n_args {
        0 => ts_method_call0(fn_val, obj),
        1 => ts_method_call1(fn_val, obj, a0),
        2 => ts_method_call2(fn_val, obj, a0, a1),
        _ => ts_method_call3(fn_val, obj, a0, a1, a2),
    };
    ts_release_val(fn_val);
    result
}

/// `container.get(key)` — Map/WeakMap: look up by key.  Anything else: undefined.
#[no_mangle]
pub unsafe extern "C" fn ts_container_get(container: TsVal, key: TsVal) -> TsVal {
    if !container.is_ptr() { return UNDEFINED; }
    match heap_tag(container) {
        0         => obj_dynamic_call(container, b"get\0", key, UNDEFINED, UNDEFINED, 1),
        5 | 7 | 9 => ts_map_get(container, key),
        12        => ts_weakmap_get(container, key),
        _         => UNDEFINED,
    }
}

/// `container.set(key, val)` — Map/WeakMap: insert/update.  Returns container (owned ref).
#[no_mangle]
pub unsafe extern "C" fn ts_container_set(container: TsVal, key: TsVal, val: TsVal) -> TsVal {
    if !container.is_ptr() {
        ts_retain_val(container);
        return container;
    }
    match heap_tag(container) {
        0         => obj_dynamic_call(container, b"set\0", key, val, UNDEFINED, 2),
        5 | 7 | 9 => ts_map_set(container, key, val),
        12        => ts_weakmap_set(container, key, val),
        _ => { ts_retain_val(container); container }
    }
}

/// `container.add(val)` — Set/WeakSet: insert value.  Returns container (owned ref).
#[no_mangle]
pub unsafe extern "C" fn ts_container_add(container: TsVal, val: TsVal) -> TsVal {
    if !container.is_ptr() {
        ts_retain_val(container);
        return container;
    }
    match heap_tag(container) {
        0  => obj_dynamic_call(container, b"add\0", val, UNDEFINED, UNDEFINED, 1),
        11 => ts_set_add(container, val),
        13 => ts_weakset_add(container, val),
        _ => { ts_retain_val(container); container }
    }
}

/// `container.has(val)` — Map/Set/WeakMap/WeakSet: membership test.  Returns boolean TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_container_has(container: TsVal, val: TsVal) -> TsVal {
    if !container.is_ptr() { return TsVal::from_bool(false); }
    match heap_tag(container) {
        0         => obj_dynamic_call(container, b"has\0", val, UNDEFINED, UNDEFINED, 1),
        5 | 7 | 9 => ts_map_has(container, val),
        11        => ts_set_has(container, val),
        12        => ts_weakmap_has(container, val),
        13        => ts_weakset_has(container, val),
        _         => TsVal::from_bool(false),
    }
}

/// `container.delete(val)` — Map/Set/WeakMap/WeakSet: remove.  Returns boolean TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_container_delete(container: TsVal, val: TsVal) -> TsVal {
    if !container.is_ptr() { return TsVal::from_bool(false); }
    match heap_tag(container) {
        0         => obj_dynamic_call(container, b"delete\0", val, UNDEFINED, UNDEFINED, 1),
        5 | 7 | 9 => ts_map_delete(container, val),
        11        => ts_set_delete(container, val),
        12        => ts_weakmap_delete(container, val),
        13        => ts_weakset_delete(container, val),
        _         => TsVal::from_bool(false),
    }
}

/// `container.clear()` — Map/Set: remove all entries.
#[no_mangle]
pub unsafe extern "C" fn ts_container_clear(container: TsVal) {
    if !container.is_ptr() { return; }
    match heap_tag(container) {
        0         => { obj_dynamic_call(container, b"clear\0", UNDEFINED, UNDEFINED, UNDEFINED, 0); }
        5 | 7 | 9 => ts_map_clear(container),
        11        => ts_set_clear(container),
        _         => {}
    }
}

/// `container.size` — number of entries as integer TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_container_size(container: TsVal) -> TsVal {
    if !container.is_ptr() { return TsVal::from_i32(0); }
    match heap_tag(container) {
        5 | 7 | 9 => ts_map_size(container),
        11        => ts_set_size(container),
        _         => TsVal::from_i32(0),
    }
}

/// `container.keys()` — Array: index array; Map: key array; Set: value array.
#[no_mangle]
pub unsafe extern "C" fn ts_container_keys(container: TsVal) -> TsVal {
    if !container.is_ptr() { return super::array::ts_arr_new(0); }
    match heap_tag(container) {
        0         => obj_dynamic_call(container, b"keys\0", UNDEFINED, UNDEFINED, UNDEFINED, 0),
        1         => ts_arr_keys(container),
        5 | 7 | 9 => ts_map_keys(container),
        11        => ts_set_keys(container),
        _         => super::array::ts_arr_new(0),
    }
}

/// `container.values()` — Array: copy of elements; Map: value array; Set: value array.
#[no_mangle]
pub unsafe extern "C" fn ts_container_values(container: TsVal) -> TsVal {
    if !container.is_ptr() { return super::array::ts_arr_new(0); }
    match heap_tag(container) {
        0         => obj_dynamic_call(container, b"values\0", UNDEFINED, UNDEFINED, UNDEFINED, 0),
        1         => ts_arr_values(container),
        5 | 7 | 9 => ts_map_values(container),
        11        => ts_set_values(container),
        _         => super::array::ts_arr_new(0),
    }
}

/// `container.entries()` — Array: [[i,v]…]; Map: [[k,v]…]; Set: [[v,v]…].
#[no_mangle]
pub unsafe extern "C" fn ts_container_entries(container: TsVal) -> TsVal {
    if !container.is_ptr() { return super::array::ts_arr_new(0); }
    match heap_tag(container) {
        0         => obj_dynamic_call(container, b"entries\0", UNDEFINED, UNDEFINED, UNDEFINED, 0),
        1         => ts_arr_entries(container),
        5 | 7 | 9 => ts_map_entries(container),
        11        => ts_set_entries(container),
        _         => super::array::ts_arr_new(0),
    }
}

/// `container.forEach(cb)` — Array: cb(el,i,arr); Map: cb(v,k,map); Set: cb(v,v,set).
#[no_mangle]
pub unsafe extern "C" fn ts_container_for_each(container: TsVal, callback: TsVal) -> TsVal {
if !container.is_ptr() { return UNDEFINED; }
    match heap_tag(container) {
        0         => obj_dynamic_call(container, b"forEach\0", callback, UNDEFINED, UNDEFINED, 1),
        1         => ts_arr_for_each(container, callback),
        5 | 7 | 9 => ts_map_for_each(container, callback),
        11        => ts_set_for_each(container, callback),
        _         => UNDEFINED,
    }
}
