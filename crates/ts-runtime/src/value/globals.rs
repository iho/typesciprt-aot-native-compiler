//! Module-level global variables and process built-ins.

use std::sync::{OnceLock, RwLock};
use std::collections::HashMap;
use super::{TsVal, UNDEFINED, ts_retain_val, ts_release_val};
use super::string_val::ts_string_new;
use super::object::{ts_obj_new, ts_obj_set_val_key};
use super::array::{ts_arr_new, ts_arr_push};

static MODULE_GLOBALS: OnceLock<RwLock<HashMap<String, TsVal>>> = OnceLock::new();

fn globals_map() -> &'static RwLock<HashMap<String, TsVal>> {
    MODULE_GLOBALS.get_or_init(|| RwLock::new(HashMap::new()))
}

#[no_mangle]
pub unsafe extern "C" fn ts_set_module_global(name_ptr: *const i8, val: TsVal) {
    let name = std::ffi::CStr::from_ptr(name_ptr).to_string_lossy().into_owned();
    if val.is_ptr() {
        let ptr = val.as_ptr();
        let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
        let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
        let rc = (*header).ref_count.load(std::sync::atomic::Ordering::SeqCst);
        if rc == 0 || rc == 0xDEAD_BEEF_DEAD_BEEFu64 || rc > 0x100_0000 {
            eprintln!("ts_set_module_global: storing freed/corrupted value for key '{}' ptr={:p} rc={:#x}", name, ptr, rc);
            std::process::abort();
        }
    }
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

/// process.exit(code) — terminate the process with the given exit code.
#[no_mangle]
pub unsafe extern "C" fn ts_process_exit(code: i32) {
    std::process::exit(code);
}

/// process.argv — returns a TsArray of command-line argument strings.
#[no_mangle]
pub unsafe extern "C" fn ts_process_argv() -> TsVal {
    let arr = ts_arr_new(0);
    for arg in std::env::args() {
        let mut bytes = arg.into_bytes();
        bytes.push(0u8);
        let s = ts_string_new(bytes.as_ptr() as *const i8);
        ts_arr_push(arr, s);
        ts_release_val(s);
    }
    arr
}

/// process.pid — returns the current process ID as a NaN-boxed integer.
#[no_mangle]
pub unsafe extern "C" fn ts_process_pid() -> TsVal {
    TsVal::from_i32(std::process::id() as i32)
}

/// process.env — returns a TsObject mapping env var names to their string values.
#[no_mangle]
pub unsafe extern "C" fn ts_process_env() -> TsVal {
    let obj = ts_obj_new();
    for (key, val) in std::env::vars() {
        let mut key_bytes = key.into_bytes();
        key_bytes.push(0u8);
        let key_str = ts_string_new(key_bytes.as_ptr() as *const i8);
        let mut val_bytes = val.into_bytes();
        val_bytes.push(0u8);
        let val_str = ts_string_new(val_bytes.as_ptr() as *const i8);
        ts_obj_set_val_key(obj, key_str, val_str);
        ts_release_val(key_str);
        ts_release_val(val_str);
    }
    obj
}

