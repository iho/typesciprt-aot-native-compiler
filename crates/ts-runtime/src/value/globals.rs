//! Module-level global variables, process built-ins, and CJS module registry.

use std::sync::{OnceLock, RwLock, Mutex};
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
        let rc = (*header).ref_count.load(std::sync::atomic::Ordering::Relaxed);
        if rc == 0 || rc == 0xDEAD_BEEFu32 || rc > 0x100_0000 {
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

// ── CJS module namespace registry ────────────────────────────────────────────

/// Global registry mapping CJS module specifier → namespace TsObject.
/// Used by `require()` to return the exported namespace of a loaded CJS module.
static CJS_REGISTRY: OnceLock<Mutex<HashMap<String, TsVal>>> = OnceLock::new();

fn cjs_registry() -> &'static Mutex<HashMap<String, TsVal>> {
    CJS_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Extract a Rust String from a TsString value (heap tag 2).
unsafe fn tsval_to_rust_string(val: TsVal) -> Option<String> {
    if val.is_ptr() && super::heap_tag(val) == 2 {
        let s = &*(val.as_ptr() as *const super::TsString);
        Some(s.inner.clone())
    } else {
        None
    }
}

/// Register a CJS namespace object under a module specifier.
/// Called by generated code when a CJS module is initialized.
/// Takes ownership of `name` (releases it after extracting the string) and `ns`
/// (retains ns internally so the registry holds a reference).
#[no_mangle]
pub unsafe extern "C" fn ts_cjs_register_ns(name: TsVal, ns: TsVal) {
    let key = tsval_to_rust_string(name).unwrap_or_default();
    ts_release_val(name);
    ts_retain_val(ns);
    let mut reg = cjs_registry().lock().unwrap();
    if let Some(old) = reg.insert(key, ns) {
        ts_release_val(old);
    }
}

/// Look up a CJS namespace object by module specifier.
/// Called by `require('specifier')` in generated code.
/// Takes ownership of `name` (releases it after lookup).
/// Returns a retained reference to the namespace, or UNDEFINED if not registered.
#[no_mangle]
pub unsafe extern "C" fn ts_cjs_require_ns(name: TsVal) -> TsVal {
    let key = tsval_to_rust_string(name).unwrap_or_default();
    ts_release_val(name);
    let reg = cjs_registry().lock().unwrap();
    if let Some(&val) = reg.get(&key) {
        ts_retain_val(val);
        val
    } else {
        UNDEFINED
    }
}

/// import.meta — returns an object with `url`, `dirname`, `filename`, and `env` properties.
#[no_mangle]
pub unsafe extern "C" fn ts_import_meta_new() -> TsVal {
    let meta = ts_obj_new();
    // Determine the executable path.
    let exe_path = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("unknown"));
    let exe_str = exe_path.to_string_lossy();
    // url = "file://<exe_path>"
    let url_s = format!("file://{}\0", exe_str);
    let url_val = ts_string_new(url_s.as_ptr() as *const i8);
    let url_key = b"url\0";
    let url_key_str = ts_string_new(url_key.as_ptr() as *const i8);
    ts_obj_set_val_key(meta, url_key_str, url_val);
    ts_release_val(url_key_str);
    ts_release_val(url_val);
    // dirname = parent directory
    let dir_s = exe_path.parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    let dir_s = format!("{}\0", dir_s);
    let dir_val = ts_string_new(dir_s.as_ptr() as *const i8);
    let dir_key = b"dirname\0";
    let dir_key_str = ts_string_new(dir_key.as_ptr() as *const i8);
    ts_obj_set_val_key(meta, dir_key_str, dir_val);
    ts_release_val(dir_key_str);
    ts_release_val(dir_val);
    // filename = full path
    let file_s = format!("{}\0", exe_str);
    let file_val = ts_string_new(file_s.as_ptr() as *const i8);
    let file_key = b"filename\0";
    let file_key_str = ts_string_new(file_key.as_ptr() as *const i8);
    ts_obj_set_val_key(meta, file_key_str, file_val);
    ts_release_val(file_key_str);
    ts_release_val(file_val);
    // env = process.env equivalent
    let env_val = ts_process_env();
    let env_key = b"env\0";
    let env_key_str = ts_string_new(env_key.as_ptr() as *const i8);
    ts_obj_set_val_key(meta, env_key_str, env_val);
    ts_release_val(env_key_str);
    ts_release_val(env_val);
    meta
}

