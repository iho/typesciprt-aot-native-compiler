//! Reflect metadata API — polyfill for reflect-metadata.
//!
//! Metadata is stored in a global map keyed by (target_identity, Option<property_key>).
//! Target identity is the raw heap pointer for pointer TsVals, otherwise the raw u64.

use super::{TsVal, UNDEFINED, FALSE, TRUE, ts_retain_val, ts_release_val};
use super::string_val::ts_string_new;

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
