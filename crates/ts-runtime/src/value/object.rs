//! TsObject: heap-allocated TypeScript objects and their operations.

use super::{TsVal, TsObject, TsArray, TsString, TsMap, TsResponse, UNDEFINED, NULL,
            heap_tag, ts_retain_val, ts_release_val};
use super::string_val::ts_string_new;
use super::array::{ts_arr_new, ts_arr_set, ts_arr_push};
use super::map::ts_map_get;
use super::uri::rust_str_to_val;
use super::symbol::symbol_to_key_string;

pub unsafe extern "C" fn ts_obj_destructor(ptr: *mut u8) {
    let obj_ptr = ptr as *mut TsObject;
    for (_, val) in (*obj_ptr).properties.drain() {
        ts_release_val(val);
    }
    std::ptr::drop_in_place(obj_ptr);
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_new() -> TsVal {
    let size = std::mem::size_of::<TsObject>();
    let ptr = crate::alloc::ts_alloc_rc(size, 0) as *mut TsObject; // tag 0 = Object
    if ptr.is_null() {
        return NULL;
    }
    std::ptr::write(ptr, TsObject {
        properties: std::collections::HashMap::new(),
    });
    TsVal::from_ptr(ptr as *mut u8)
}

#[no_mangle]
/// Create an Error object with a `message` property. Represented as a plain TsObject.
pub unsafe extern "C" fn ts_error_new(message: TsVal) -> TsVal {
    let err = ts_obj_new();
    let msg_key = b"message\0";
    ts_obj_set(err, msg_key.as_ptr() as *const i8, message);
    let name_c = b"Error\0";
    let name_str = ts_string_new(name_c.as_ptr() as *const i8);
    let name_key = b"name\0";
    ts_obj_set(err, name_key.as_ptr() as *const i8, name_str);
    ts_release_val(name_str);
    err
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_get(obj_val: TsVal, key_ptr: *const i8) -> TsVal {
    let ptr = obj_val.as_ptr();
    if ptr.is_null() || key_ptr.is_null() { return UNDEFINED; }
    let tag = heap_tag(obj_val);
    let key = std::ffi::CStr::from_ptr(key_ptr).to_string_lossy().into_owned();
    if tag == 7 {
        // TsHeaders: __class__ property or nothing else via obj_get
        if key == "__class__" {
            return rust_str_to_val("Headers".to_string());
        }
        return UNDEFINED;
    }
    if tag == 8 {
        // TsResponse: expose body, status, headers, __class__
        let resp = &*(ptr as *const TsResponse);
        match key.as_str() {
            "__class__" => return rust_str_to_val("Response".to_string()),
            "body" => { ts_retain_val(resp.body); return resp.body; }
            "status" => return TsVal::from_i32(resp.status as i32),
            "headers" => { ts_retain_val(resp.headers); return resp.headers; }
            "ok" => return TsVal::from_bool(resp.status >= 200 && resp.status < 300),
            _ => return UNDEFINED,
        }
    }
    let obj = ptr as *mut TsObject;
    if let Some(&val) = (&*obj).properties.get(&key) {
        ts_retain_val(val);
        return val;
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_set(obj_val: TsVal, key_ptr: *const i8, val: TsVal) {
    let ptr = obj_val.as_ptr();
    if !ptr.is_null() && !key_ptr.is_null() {
        let obj = ptr as *mut TsObject;
        let key = std::ffi::CStr::from_ptr(key_ptr).to_string_lossy().into_owned();

        // ARC: retain new value
        ts_retain_val(val);

        let old_val = (&mut *obj).properties.insert(key, val);
        if let Some(v) = old_val {
            ts_release_val(v);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_delete(obj_val: TsVal, key_ptr: *const i8) -> TsVal {
    let ptr = obj_val.as_ptr();
    if !ptr.is_null() && !key_ptr.is_null() {
        let obj = ptr as *mut TsObject;
        let key = std::ffi::CStr::from_ptr(key_ptr).to_string_lossy().into_owned();
        if let Some(old) = (&mut *obj).properties.remove(&key) {
            ts_release_val(old);
        }
    }
    TsVal::from_bool(true)
}

#[no_mangle]
/// Convert a TsVal key to a Rust String for HashMap lookup, or None if not representable.
pub(super) unsafe fn tsval_to_key_string(key: TsVal) -> Option<String> {
    if key.is_int32() {
        return Some(key.as_i32().to_string());
    }
    if key.is_ptr() {
        match heap_tag(key) {
            2  => {
                let ts_str = &*(key.as_ptr() as *const TsString);
                return Some(ts_str.inner.clone());
            }
            10 => return symbol_to_key_string(key), // Symbol → "\x01sym{id}"
            _  => {}
        }
    }
    if !key.is_nan_boxed() {
        return Some(key.as_f64().to_string());
    }
    None
}

/// `key in obj` — returns boolean TsVal
#[no_mangle]
pub unsafe extern "C" fn ts_val_has_key(obj: TsVal, key: TsVal) -> TsVal {
    if !obj.is_ptr() { return TsVal::from_bool(false); }
    let tag = heap_tag(obj);
    let ptr = obj.as_ptr();
    if tag == 0 {
        // TsObject
        let key_str = tsval_to_key_string(key);
        let has = if let Some(k) = key_str {
            (*(ptr as *const TsObject)).properties.contains_key(&k)
        } else { false };
        return TsVal::from_bool(has);
    }
    if tag == 1 {
        // TsArray: check index
        if key.is_int32() {
            let idx = key.as_i32() as usize;
            let arr = &*(ptr as *const TsArray);
            return TsVal::from_bool(idx < arr.elements.len());
        }
        return TsVal::from_bool(false);
    }
    if tag == 5 {
        // TsMap
        use super::operators::ts_val_strict_eq;
        let map = &*(ptr as *const TsMap);
        let kv = key;
        let has = map.entries.iter().any(|(k, _)| ts_val_strict_eq(*k, kv) != 0);
        return TsVal::from_bool(has);
    }
    if tag == 11 {
        // TsSet — `key in set` checks membership
        return super::set::ts_set_has(obj, key);
    }
    TsVal::from_bool(false)
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_delete_key(obj_val: TsVal, key: TsVal) -> TsVal {
    let ptr = obj_val.as_ptr();
    if !ptr.is_null() {
        let obj = ptr as *mut TsObject;
        if let Some(key_str) = tsval_to_key_string(key) {
            if let Some(old) = (&mut *obj).properties.remove(&key_str) {
                ts_release_val(old);
            }
        }
    }
    TsVal::from_bool(true)
}

/// Set an object property using a `TsVal` key (for computed property names `{ [expr]: val }`
/// and dynamic assignment `obj[key] = val`).
/// For arrays with integer keys, delegates to `ts_arr_set`. Otherwise treats as object property.
#[no_mangle]
pub unsafe extern "C" fn ts_obj_set_val_key(obj: TsVal, key: TsVal, val: TsVal) {
    // Array with integer key → use ts_arr_set for proper element assignment.
    if obj.is_ptr() && heap_tag(obj) == 1 && key.is_int32() {
        ts_arr_set(obj, key.as_i32(), val);
        return;
    }

    if let Some(key_string) = tsval_to_key_string(key) {
        let mut bytes = key_string.into_bytes();
        bytes.push(0u8);
        ts_obj_set(obj, bytes.as_ptr() as *const i8, val);
    }
    // skip null, undefined, bool, unrecognised object keys
}

/// Generic getter for `obj[key]` — works for arrays (integer index), objects (string key),
/// Map (TsVal key), and strings (character-at index).
#[no_mangle]
pub unsafe extern "C" fn ts_val_get_key(obj: TsVal, key: TsVal) -> TsVal {
    if !obj.is_ptr() {
        return UNDEFINED;
    }
    let tag = heap_tag(obj);
    // Array with integer index
    if tag == 1 && key.is_int32() {
        return super::array::ts_arr_get(obj, key.as_i32());
    }
    // Map or Headers with any key
    if tag == 5 || tag == 7 {
        return ts_map_get(obj, key);
    }
    // Response: property access via string key
    if tag == 8 {
        let key_str = if key.is_ptr() && heap_tag(key) == 2 {
            let ts_str = &*(key.as_ptr() as *const TsString);
            ts_str.inner.clone()
        } else if key.is_int32() {
            key.as_i32().to_string()
        } else {
            return UNDEFINED;
        };
        let mut bytes = key_str.into_bytes();
        bytes.push(0u8);
        return ts_obj_get(obj, bytes.as_ptr() as *const i8);
    }
    // String: character at integer index
    if tag == 2 && key.is_int32() {
        let idx = key.as_i32() as usize;
        let ts_str = &*(obj.as_ptr() as *const TsString);
        let chars: Vec<char> = ts_str.inner.chars().collect();
        if idx < chars.len() {
            let ch = chars[idx].to_string();
            let mut bytes = ch.into_bytes();
            bytes.push(0u8);
            return ts_string_new(bytes.as_ptr() as *const i8);
        }
        return UNDEFINED;
    }
    // Object (or array) with string or symbol key
    if let Some(key_string) = tsval_to_key_string(key) {
        let mut bytes = key_string.into_bytes();
        bytes.push(0u8);
        ts_obj_get(obj, bytes.as_ptr() as *const i8)
    } else {
        UNDEFINED
    }
}

/// Returns a new TsObject with all enumerable own properties EXCEPT those in `keys_arr`.
/// `keys_arr` is a TsArray of TsString values (the already-destructured keys).
/// Used for `const { a, b, ...rest } = obj`.
#[no_mangle]
pub unsafe extern "C" fn ts_obj_rest(obj: TsVal, keys_arr: TsVal) -> TsVal {
    if !obj.is_ptr() || heap_tag(obj) != 0 {
        return ts_obj_new();
    }
    // Collect excluded key strings
    let mut excluded: std::collections::HashSet<String> = std::collections::HashSet::new();
    if keys_arr.is_ptr() && heap_tag(keys_arr) == 1 {
        let arr = &*(keys_arr.as_ptr() as *const TsArray);
        for &k in &arr.elements {
            if k.is_ptr() && heap_tag(k) == 2 {
                let ts_str = &*(k.as_ptr() as *const TsString);
                excluded.insert(ts_str.inner.clone());
            } else if k.is_int32() {
                excluded.insert(k.as_i32().to_string());
            }
        }
    }
    let result = ts_obj_new();
    let src = &*(obj.as_ptr() as *const TsObject);
    for (key, val) in &src.properties {
        if excluded.contains(key) { continue; }
        if key.starts_with("__") { continue; }
        let mut bytes = key.as_bytes().to_vec();
        bytes.push(0u8);
        ts_retain_val(*val);
        ts_obj_set(result, bytes.as_ptr() as *const i8, *val);
    }
    result
}

/// Returns a `TsArray` containing the own enumerable string keys of `obj`.
/// Internal keys (prefixed with `__`) are excluded.
#[no_mangle]
pub unsafe extern "C" fn ts_obj_keys(obj: TsVal) -> TsVal {
    if !obj.is_ptr() || heap_tag(obj) != 0 {
        return ts_arr_new(0);
    }
    let ts_obj = &*(obj.as_ptr() as *const TsObject);
    let keys: Vec<String> = ts_obj.properties.keys()
        .filter(|k| !k.starts_with("__"))
        .cloned()
        .collect();
    let n = keys.len() as i32;
    let arr = ts_arr_new(n);
    for (i, key) in keys.iter().enumerate() {
        let mut bytes = key.as_bytes().to_vec();
        bytes.push(0u8);
        let key_val = ts_string_new(bytes.as_ptr() as *const i8);
        ts_arr_set(arr, i as i32, key_val);
        ts_release_val(key_val);
    }
    arr
}

/// Returns a TsArray of `obj`'s own enumerable values.
#[no_mangle]
pub unsafe extern "C" fn ts_obj_values(obj: TsVal) -> TsVal {
    if !obj.is_ptr() || heap_tag(obj) != 0 { return ts_arr_new(0); }
    let ts_obj = &*(obj.as_ptr() as *const TsObject);
    let vals: Vec<TsVal> = ts_obj.properties.iter()
        .filter(|(k, _)| !k.starts_with("__"))
        .map(|(_, &v)| v)
        .collect();
    let arr = ts_arr_new(0);
    for val in vals {
        ts_arr_push(arr, val);
    }
    arr
}

/// Returns a TsArray of `[key, value]` TsArray pairs for each own property.
#[no_mangle]
pub unsafe extern "C" fn ts_obj_entries(obj: TsVal) -> TsVal {
    if !obj.is_ptr() || heap_tag(obj) != 0 { return ts_arr_new(0); }
    let ts_obj = &*(obj.as_ptr() as *const TsObject);
    let entries: Vec<(String, TsVal)> = ts_obj.properties.iter()
        .filter(|(k, _)| !k.starts_with("__"))
        .map(|(k, &v)| (k.clone(), v))
        .collect();
    let result = ts_arr_new(0);
    for (key, val) in entries {
        let pair = ts_arr_new(2);
        let mut bytes = key.into_bytes();
        bytes.push(0u8);
        let key_val = ts_string_new(bytes.as_ptr() as *const i8);
        ts_arr_set(pair, 0, key_val);
        ts_release_val(key_val);
        ts_arr_set(pair, 1, val);
        ts_arr_push(result, pair);
        ts_release_val(pair);
    }
    result
}

/// Copy all own enumerable properties from `src` to `dst` (object spread).
#[no_mangle]
pub unsafe extern "C" fn ts_obj_merge(dst: TsVal, src: TsVal) {
    if !dst.is_ptr() || heap_tag(dst) != 0 { return; }
    if !src.is_ptr() || heap_tag(src) != 0 { return; }
    let src_obj = &*(src.as_ptr() as *const TsObject);
    let entries: Vec<(String, TsVal)> = src_obj.properties.iter()
        .filter(|(k, _)| !k.starts_with("__"))
        .map(|(k, &v)| (k.clone(), v))
        .collect();
    for (key, val) in entries {
        let mut bytes = key.into_bytes();
        bytes.push(0u8);
        ts_obj_set(dst, bytes.as_ptr() as *const i8, val);
    }
}

/// `Object.assign(target, source)` — copy own enumerable properties from source to target.
/// Returns the target object.
#[no_mangle]
pub unsafe extern "C" fn ts_obj_assign(target: TsVal, source: TsVal) -> TsVal {
    ts_obj_merge(target, source);
    ts_retain_val(target);
    target
}

/// `Object.create(proto)` — create a new object; proto is ignored for now (treated as null).
#[no_mangle]
pub unsafe extern "C" fn ts_obj_create(_proto: TsVal) -> TsVal {
    ts_obj_new()
}

/// `Object.fromEntries(iterable)` — build object from `[[key, val], ...]` array.
#[no_mangle]
pub unsafe extern "C" fn ts_obj_from_entries(arr: TsVal) -> TsVal {
    let obj = ts_obj_new();
    if !arr.is_ptr() || heap_tag(arr) != 1 { return obj; }
    let arr_ptr = arr.as_ptr() as *const TsArray;
    let len = { let r = &*arr_ptr; r.elements.len() };
    for i in 0..len {
        let pair = { let r = &*arr_ptr; r.elements[i] };
        if !pair.is_ptr() || heap_tag(pair) != 1 { continue; }
        let pair_ptr = pair.as_ptr() as *const TsArray;
        let pair_len = { let r = &*pair_ptr; r.elements.len() };
        if pair_len < 2 { continue; }
        let key = { let r = &*pair_ptr; r.elements[0] };
        let val = { let r = &*pair_ptr; r.elements[1] };
        ts_obj_set_val_key(obj, key, val);
    }
    obj
}


