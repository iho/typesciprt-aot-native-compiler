//! Module-level global variables.

use std::sync::{OnceLock, RwLock};
use std::collections::HashMap;
use super::{TsVal, UNDEFINED, ts_retain_val, ts_release_val};

static MODULE_GLOBALS: OnceLock<RwLock<HashMap<String, TsVal>>> = OnceLock::new();

fn globals_map() -> &'static RwLock<HashMap<String, TsVal>> {
    MODULE_GLOBALS.get_or_init(|| RwLock::new(HashMap::new()))
}

#[no_mangle]
pub unsafe extern "C" fn ts_set_module_global(name_ptr: *const i8, val: TsVal) {
    let name = std::ffi::CStr::from_ptr(name_ptr).to_string_lossy().into_owned();
    ts_retain_val(val);
    let mut map = globals_map().write().unwrap();
    if let Some(old) = map.insert(name, val) {
        ts_release_val(old);
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_get_module_global(name_ptr: *const i8) -> TsVal {
    let name = std::ffi::CStr::from_ptr(name_ptr).to_string_lossy().into_owned();
    let map = globals_map().read().unwrap();
    if let Some(&val) = map.get(&name) {
        ts_retain_val(val);
        val
    } else {
        UNDEFINED
    }
}
