//! Reflect metadata API — polyfill for reflect-metadata.
//!
//! Metadata is stored in a global map keyed by (target_identity, Option<property_key>).
//! Target identity is the raw heap pointer for pointer TsVals, otherwise the raw u64.

use super::{TsVal, UNDEFINED, FALSE, TRUE, ts_retain_val, ts_release_val, heap_tag};
use super::func::{ts_closure_new, ts_func_call2};
use super::string_val::ts_string_new;
use super::array::{ts_arr_new, ts_arr_push, ts_arr_get, ts_arr_len};

use std::sync::Mutex;
use std::collections::HashMap;

type MetaKey = (u64, Option<String>);
// lazily initialised; avoids issues with global destructor ordering
static METADATA: Mutex<Option<HashMap<MetaKey, HashMap<String, TsVal>>>> =
    Mutex::new(None);

/// Extract the Rust String from a TsString TsVal, or None for anything else.
unsafe fn val_to_string(val: TsVal) -> Option<String> {
    if val.is_ptr() && super::heap_tag(val) == 2 {
        let s = &*(val.as_ptr() as *const super::TsString);
        return Some(s.inner.clone());
    }
    None
}

/// Identity key of a target TsVal.
fn target_id(target: TsVal) -> u64 {
    if target.is_ptr() { target.as_ptr() as u64 } else { target.0 }
}

/// `Reflect.defineMetadata(metadataKey, metadataValue, target[, propertyKey])`
/// Pass UNDEFINED for `prop_key` when there is no property key.
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_define_metadata(
    meta_key: TsVal,
    meta_val: TsVal,
    target:   TsVal,
    prop_key: TsVal,
) {
    let mk = match val_to_string(meta_key) { Some(s) => s, None => return };
    let tid = target_id(target);
    let pk  = val_to_string(prop_key);

    let mut guard = METADATA.lock().unwrap();
    let store = guard.get_or_insert_with(HashMap::new);
    let entry = store.entry((tid, pk)).or_default();
    if let Some(old) = entry.get(&mk) { ts_release_val(*old); }
    ts_retain_val(meta_val);
    entry.insert(mk, meta_val);
}

/// `Reflect.getMetadata(metadataKey, target[, propertyKey])` → TsVal
/// Returns UNDEFINED if no metadata is found.
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_get_metadata(
    meta_key: TsVal,
    target:   TsVal,
    prop_key: TsVal,
) -> TsVal {
    let mk = match val_to_string(meta_key) { Some(s) => s, None => return UNDEFINED };
    let tid = target_id(target);
    let pk  = val_to_string(prop_key);

    let guard = METADATA.lock().unwrap();
    let result = guard.as_ref()
        .and_then(|s| s.get(&(tid, pk)))
        .and_then(|m| m.get(&mk))
        .copied()
        .unwrap_or(UNDEFINED);
    if !result.is_undefined() { ts_retain_val(result); }
    result
}

/// `Reflect.getOwnMetadata` — identical to getMetadata in our flat (no-chain) model.
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_get_own_metadata(
    meta_key: TsVal,
    target:   TsVal,
    prop_key: TsVal,
) -> TsVal {
    ts_reflect_get_metadata(meta_key, target, prop_key)
}

/// `Reflect.hasMetadata(metadataKey, target[, propertyKey])` → boolean TsVal
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_has_metadata(
    meta_key: TsVal,
    target:   TsVal,
    prop_key: TsVal,
) -> TsVal {
    let mk = match val_to_string(meta_key) { Some(s) => s, None => return FALSE };
    let tid = target_id(target);
    let pk  = val_to_string(prop_key);

    let guard = METADATA.lock().unwrap();
    let has = guard.as_ref()
        .and_then(|s| s.get(&(tid, pk)))
        .map(|m| m.contains_key(&mk))
        .unwrap_or(false);
    TsVal::from_bool(has)
}

/// `Reflect.hasOwnMetadata` — identical to hasMetadata in our flat model.
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_has_own_metadata(
    meta_key: TsVal,
    target:   TsVal,
    prop_key: TsVal,
) -> TsVal {
    ts_reflect_has_metadata(meta_key, target, prop_key)
}

/// `Reflect.getMetadataKeys(target[, propertyKey])` → TsArray of TsString keys
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_get_metadata_keys(
    target:   TsVal,
    prop_key: TsVal,
) -> TsVal {
    let tid = target_id(target);
    let pk  = val_to_string(prop_key);

    let arr = super::array::ts_arr_new(4);
    let guard = METADATA.lock().unwrap();
    if let Some(store) = guard.as_ref() {
        if let Some(map) = store.get(&(tid, pk)) {
            for key in map.keys() {
                let mut bytes = key.as_bytes().to_vec();
                bytes.push(0);
                let key_val = ts_string_new(bytes.as_ptr() as *const i8);
                super::array::ts_arr_push(arr, key_val);
                ts_release_val(key_val);
            }
        }
    }
    arr
}

/// `Reflect.getOwnMetadataKeys` — identical to getMetadataKeys in our flat model.
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_get_own_metadata_keys(
    target:   TsVal,
    prop_key: TsVal,
) -> TsVal {
    ts_reflect_get_metadata_keys(target, prop_key)
}

/// `Reflect.getPrototypeOf(obj)` — returns UNDEFINED in our flat model.
/// All methods are stored directly on instances; there is no separate prototype object.
/// Any `while (proto = Reflect.getPrototypeOf(proto))` loop terminates immediately.
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_get_prototype_of(_obj: TsVal) -> TsVal {
    UNDEFINED
}

/// `Reflect.getOwnPropertyDescriptor(obj, key)` → descriptor object or UNDEFINED.
/// Descriptor shape: `{value: V, writable: true, enumerable: true, configurable: true}`.
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_get_own_property_descriptor(obj: TsVal, key: TsVal) -> TsVal {
    let val = super::object::ts_val_get_key(obj, key);
    if val.is_undefined() {
        return UNDEFINED;
    }
    let desc = super::object::ts_obj_new();
    let value_key = ts_string_new(b"value\0".as_ptr() as *const i8);
    super::object::ts_obj_set_val_key(desc, value_key, val);
    ts_release_val(value_key);
    ts_release_val(val);
    let writable_key = ts_string_new(b"writable\0".as_ptr() as *const i8);
    super::object::ts_obj_set_val_key(desc, writable_key, super::TRUE);
    ts_release_val(writable_key);
    let enum_key = ts_string_new(b"enumerable\0".as_ptr() as *const i8);
    super::object::ts_obj_set_val_key(desc, enum_key, super::TRUE);
    ts_release_val(enum_key);
    let conf_key = ts_string_new(b"configurable\0".as_ptr() as *const i8);
    super::object::ts_obj_set_val_key(desc, conf_key, super::TRUE);
    ts_release_val(conf_key);
    desc
}

/// `Reflect.metadata(key, value)` → decorator function (TsFunction closure).
/// The returned decorator, when called as `decorator(target)` or `decorator(target, propKey)`,
/// calls `Reflect.defineMetadata(key, value, target, propKey)`.
/// This is what TypeScript's `__metadata(key, value)` helper emits.
///
/// We create a small closure that captures [key, value] and, when invoked, calls
/// ts_reflect_define_metadata.
///
/// Closure env layout: [meta_key, meta_val]  (both retained)
/// Closure body: __reflect_meta_apply(env, target, prop_key)
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_metadata_decorator(meta_key: TsVal, meta_val: TsVal) -> TsVal {
    // Build env array [meta_key, meta_val]
    let env = ts_arr_new(0);
    ts_arr_push(env, meta_key);
    ts_arr_push(env, meta_val);
    // Create closure with arity 2 (target, propKey), pointing to __reflect_meta_apply_fn
    extern "C" fn __reflect_meta_apply_fn(env: TsVal, target: TsVal, prop_key: TsVal) -> TsVal {
        unsafe {
            let mk  = ts_arr_get(env, 0);
            let mv  = ts_arr_get(env, 1);
            ts_reflect_define_metadata(mk, mv, target, prop_key);
            // Release only what we explicitly retained: mk and mv from ts_arr_get.
            // target, prop_key, and env are NOT owned by this closure body:
            //   - target/prop_key: owned+released by the caller (ts_func_call2 caller emits
            //     ts_release_val for each arg after the call)
            //   - env: passed through from TsFunction.env without an extra retain by
            //     dispatch_callback; the TsFunction destructor owns it
            ts_release_val(mk);
            ts_release_val(mv);
            UNDEFINED
        }
    }
    let closure = ts_closure_new(__reflect_meta_apply_fn as *const u8, 2, env);
    ts_release_val(env);
    closure
}

/// `__decorate(decorators, target, key, desc)` → applies each decorator in the array.
/// This is TypeScript's `__decorate` helper function used in compiled output.
///
/// For class decorators (key is UNDEFINED/null): each decorator is called as `decorator(target)`.
/// For property/method decorators: each decorator is called as `decorator(target, key, desc)`.
/// Returns the (potentially modified) target for class decorators, or desc for method decorators.
#[no_mangle]
pub unsafe extern "C" fn ts_apply_decorators(
    decorators: TsVal,
    target:     TsVal,
    key:        TsVal,
    desc:       TsVal,
) -> TsVal {
    if !decorators.is_ptr() || heap_tag(decorators) != 1 {
        // Not an array — return target unchanged
        ts_retain_val(target);
        return target;
    }
    let n = {
        let arr = &*(decorators.as_ptr() as *const super::TsArray);
        arr.elements.len()
    };
    let is_class_decorator = key.is_undefined() || key.is_null();
    let mut result = if is_class_decorator { target } else { desc };
    ts_retain_val(result);

    // Apply decorators in reverse order (last decorator wraps outermost)
    for i in (0..n).rev() {
        let dec = ts_arr_get(decorators, i as i32);
        if dec.is_undefined() {
            continue;
        }
        let new_result = if is_class_decorator {
            ts_func_call2(dec, result, UNDEFINED)
        } else {
            // property/method decorator: (target, key, desc) → may return new descriptor
            ts_func_call2(dec, target, key)
        };
        ts_release_val(dec);
        if !new_result.is_undefined() {
            ts_release_val(result);
            result = new_result;
        } else {
            ts_release_val(new_result);
        }
    }
    result
}

/// `Reflect.deleteMetadata(metadataKey, target[, propertyKey])` → boolean TsVal
#[no_mangle]
pub unsafe extern "C" fn ts_reflect_delete_metadata(
    meta_key: TsVal,
    target:   TsVal,
    prop_key: TsVal,
) -> TsVal {
    let mk = match val_to_string(meta_key) { Some(s) => s, None => return FALSE };
    let tid = target_id(target);
    let pk  = val_to_string(prop_key);

    let mut guard = METADATA.lock().unwrap();
    if let Some(store) = guard.as_mut() {
        if let Some(map) = store.get_mut(&(tid, pk)) {
            if let Some(old) = map.remove(&mk) {
                ts_release_val(old);
                return TRUE;
            }
        }
    }
    FALSE
}
