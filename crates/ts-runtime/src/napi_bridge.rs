//! Node-API (N-API) bridge — exposes compiled TypeScript exports as a Node.js `.node` addon.
//!
//! Compiled with `--features napi` only.
//!
//! Flow:
//!   1. Node.js dlopen()s the `.node` file and calls `napi_register_module_v1`.
//!   2. `napi_register_module_v1` calls the compiled `__napi_init()` function.
//!   3. `__napi_init()` calls `ts_napi_register_export(name, fn_ptr, arity)` for each
//!      top-level `export function` in the TypeScript source.
//!   4. Back in `napi_register_module_v1`, each export is wrapped in a generic napi
//!      callback and set as a property on the `exports` object.

use std::ffi::{CStr, CString};
use std::sync::{Mutex, OnceLock};

use crate::ts_runtime;
use crate::value::{TsVal, UNDEFINED, NULL, TRUE, FALSE, TsString, TsArray, TsObject, heap_tag};
use crate::value::string_val::ts_string_new;
use crate::value::array::{ts_arr_new, ts_arr_push, ts_arr_len, ts_arr_get};
use crate::value::object::{ts_obj_new, ts_obj_set_val_key, ts_obj_keys};
use crate::value::promise::ts_promise_await;
use crate::value::{ts_retain_val, ts_release_val};

// ── Raw Node-API opaque types ─────────────────────────────────────────────────

#[repr(C)]
struct napi_env__([u8; 0]);
#[repr(C)]
struct napi_value__([u8; 0]);
#[repr(C)]
struct napi_callback_info__([u8; 0]);

#[allow(non_camel_case_types)]
type napi_env = *mut napi_env__;
#[allow(non_camel_case_types)]
type napi_value = *mut napi_value__;
#[allow(non_camel_case_types)]
type napi_callback_info = *mut napi_callback_info__;
#[allow(non_camel_case_types)]
type napi_status = i32;
#[allow(non_camel_case_types)]
type napi_callback = unsafe extern "C" fn(napi_env, napi_callback_info) -> napi_value;

const NAPI_OK: napi_status = 0;
const NAPI_AUTO_LENGTH: usize = usize::MAX;

// napi_valuetype ordinal constants
const NAPI_UNDEFINED: i32 = 0;
const NAPI_NULL: i32 = 1;
const NAPI_BOOLEAN: i32 = 2;
const NAPI_NUMBER: i32 = 3;
const NAPI_STRING: i32 = 4;
#[allow(dead_code)]
const NAPI_SYMBOL: i32 = 5;
const NAPI_OBJECT: i32 = 6;
#[allow(dead_code)]
const NAPI_FUNCTION: i32 = 7;
#[allow(dead_code)]
const NAPI_BIGINT: i32 = 9;

// Node-API symbols are resolved at load time from the host Node.js process.
// On macOS: linked with -undefined dynamic_lookup.
// On Linux: symbols come from the node executable in the process image.
extern "C" {
    fn napi_get_cb_info(
        env: napi_env,
        info: napi_callback_info,
        argc: *mut usize,
        argv: *mut napi_value,
        this_arg: *mut napi_value,
        data: *mut *mut (),
    ) -> napi_status;

    fn napi_create_function(
        env: napi_env,
        utf8name: *const u8,
        length: usize,
        cb: napi_callback,
        data: *mut (),
        result: *mut napi_value,
    ) -> napi_status;

    fn napi_set_named_property(
        env: napi_env,
        object: napi_value,
        utf8name: *const u8,
        value: napi_value,
    ) -> napi_status;

    fn napi_get_undefined(env: napi_env, result: *mut napi_value) -> napi_status;
    fn napi_get_null(env: napi_env, result: *mut napi_value) -> napi_status;
    fn napi_get_boolean(env: napi_env, value: bool, result: *mut napi_value) -> napi_status;

    fn napi_create_double(env: napi_env, value: f64, result: *mut napi_value) -> napi_status;
    fn napi_create_int32(env: napi_env, value: i32, result: *mut napi_value) -> napi_status;

    fn napi_create_string_utf8(
        env: napi_env,
        str_ptr: *const u8,
        length: usize,
        result: *mut napi_value,
    ) -> napi_status;

    fn napi_typeof(env: napi_env, value: napi_value, result: *mut i32) -> napi_status;
    fn napi_get_value_double(env: napi_env, value: napi_value, result: *mut f64) -> napi_status;
    fn napi_get_value_int32(env: napi_env, value: napi_value, result: *mut i32) -> napi_status;
    fn napi_get_value_bool(env: napi_env, value: napi_value, result: *mut bool) -> napi_status;

    fn napi_get_value_string_utf8(
        env: napi_env,
        value: napi_value,
        buf: *mut u8,
        bufsize: usize,
        result: *mut usize,
    ) -> napi_status;

    fn napi_create_object(env: napi_env, result: *mut napi_value) -> napi_status;

    fn napi_create_array_with_length(
        env: napi_env,
        length: usize,
        result: *mut napi_value,
    ) -> napi_status;

    fn napi_set_element(
        env: napi_env,
        array: napi_value,
        index: u32,
        value: napi_value,
    ) -> napi_status;

    fn napi_get_array_length(
        env: napi_env,
        value: napi_value,
        result: *mut u32,
    ) -> napi_status;

    fn napi_get_element(
        env: napi_env,
        array: napi_value,
        index: u32,
        result: *mut napi_value,
    ) -> napi_status;

    fn napi_is_array(env: napi_env, value: napi_value, result: *mut bool) -> napi_status;

    fn napi_get_property_names(
        env: napi_env,
        object: napi_value,
        result: *mut napi_value,
    ) -> napi_status;

    fn napi_get_named_property(
        env: napi_env,
        object: napi_value,
        utf8name: *const u8,
        result: *mut napi_value,
    ) -> napi_status;
}

// ── Export registry ───────────────────────────────────────────────────────────

/// Metadata for a single exported TypeScript function.
struct NapiExportEntry {
    /// Null-terminated function name (used as both the JS property name and napi function name).
    name: CString,
    /// Raw pointer to the compiled native function.
    fn_ptr: *const u8,
    /// Number of parameters the TS function expects.
    arity: i32,
}

// SAFETY: fn_ptr is a code pointer (read-only); safe to send between threads.
unsafe impl Send for NapiExportEntry {}
unsafe impl Sync for NapiExportEntry {}

static NAPI_EXPORTS: OnceLock<Mutex<Vec<NapiExportEntry>>> = OnceLock::new();

fn napi_exports() -> &'static Mutex<Vec<NapiExportEntry>> {
    NAPI_EXPORTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Called from the compiled `__napi_init()` MLIR function for each exported function.
/// Stores the export so `napi_register_module_v1` can register it on the exports object.
#[no_mangle]
pub unsafe extern "C" fn ts_napi_register_export(
    name_ptr: *const i8,
    fn_ptr: *const u8,
    arity: i32,
) {
    let name = CStr::from_ptr(name_ptr).to_owned();
    let entry = NapiExportEntry {
        name: CString::from(name),
        fn_ptr,
        arity,
    };
    napi_exports().lock().unwrap().push(entry);
}

// ── TsVal ↔ napi_value conversion ────────────────────────────────────────────

/// Convert a TsVal to an napi_value. Returns a null napi_value on failure.
/// The caller is responsible for releasing `val` if it is a heap value.
unsafe fn tsval_to_napi(env: napi_env, val: TsVal) -> napi_value {
    let mut result: napi_value = std::ptr::null_mut();

    if val.is_undefined() {
        napi_get_undefined(env, &mut result);
        return result;
    }
    if val.is_null() {
        napi_get_null(env, &mut result);
        return result;
    }
    if val.is_bool() {
        napi_get_boolean(env, val.as_bool(), &mut result);
        return result;
    }
    if val.is_int32() {
        napi_create_int32(env, val.as_i32(), &mut result);
        return result;
    }
    if val.is_number() {
        // f64 number (not NaN-boxed)
        napi_create_double(env, val.as_f64(), &mut result);
        return result;
    }
    if val.is_ptr() {
        let tag = heap_tag(val);
        match tag {
            2 => {
                // TsString
                let s = &*(val.as_ptr() as *const TsString);
                let bytes = s.inner.as_bytes();
                napi_create_string_utf8(env, bytes.as_ptr(), bytes.len(), &mut result);
                return result;
            }
            1 => {
                // TsArray — convert element by element
                let len = ts_arr_len(val);
                let n = if len.is_int32() { len.as_i32() as usize } else { 0 };
                napi_create_array_with_length(env, n, &mut result);
                for i in 0..n {
                    let elem = ts_arr_get(val, i as i32);
                    let napi_elem = tsval_to_napi(env, elem);
                    ts_release_val(elem);
                    napi_set_element(env, result, i as u32, napi_elem);
                }
                return result;
            }
            0 => {
                // TsObject — convert own properties
                napi_create_object(env, &mut result);
                let obj = &*(val.as_ptr() as *const TsObject);
                for (key, &prop_val) in &obj.properties {
                    let napi_val = tsval_to_napi(env, prop_val);
                    let key_cstr = format!("{}\0", key);
                    napi_set_named_property(env, result, key_cstr.as_ptr(), napi_val);
                }
                return result;
            }
            3 => {
                // TsPromise — await it synchronously and convert the resolved value
                ts_retain_val(val);
                let resolved = ts_promise_await(val);
                let r = tsval_to_napi(env, resolved);
                ts_release_val(resolved);
                return r;
            }
            _ => {
                // Unsupported type: return undefined
                napi_get_undefined(env, &mut result);
                return result;
            }
        }
    }

    napi_get_undefined(env, &mut result);
    result
}

/// Convert an napi_value to a TsVal. Returns an owned reference for heap values.
unsafe fn napi_to_tsval(env: napi_env, value: napi_value) -> TsVal {
    if value.is_null() {
        return UNDEFINED;
    }

    let mut vtype: i32 = NAPI_UNDEFINED;
    napi_typeof(env, value, &mut vtype);

    match vtype {
        NAPI_UNDEFINED => UNDEFINED,
        NAPI_NULL => NULL,
        NAPI_BOOLEAN => {
            let mut b = false;
            napi_get_value_bool(env, value, &mut b);
            TsVal::from_bool(b)
        }
        NAPI_NUMBER => {
            let mut d: f64 = 0.0;
            napi_get_value_double(env, value, &mut d);
            // Represent as int if it is an exact integer within i32 range
            if d.fract() == 0.0 && d >= i32::MIN as f64 && d <= i32::MAX as f64 {
                TsVal::from_i32(d as i32)
            } else {
                TsVal::from_f64(d)
            }
        }
        NAPI_STRING => {
            // First pass: get required length
            let mut len: usize = 0;
            napi_get_value_string_utf8(env, value, std::ptr::null_mut(), 0, &mut len);
            // Second pass: read the string (len does not include null terminator)
            let mut buf = vec![0u8; len + 1];
            napi_get_value_string_utf8(env, value, buf.as_mut_ptr(), buf.len(), &mut len);
            ts_string_new(buf.as_ptr() as *const i8)
        }
        NAPI_OBJECT => {
            // Check if it's an array
            let mut is_arr = false;
            napi_is_array(env, value, &mut is_arr);
            if is_arr {
                let mut arr_len: u32 = 0;
                napi_get_array_length(env, value, &mut arr_len);
                let ts_arr = ts_arr_new(0);
                for i in 0..arr_len {
                    let mut elem: napi_value = std::ptr::null_mut();
                    napi_get_element(env, value, i, &mut elem);
                    let ts_elem = napi_to_tsval(env, elem);
                    ts_arr_push(ts_arr, ts_elem);
                    ts_release_val(ts_elem);
                }
                ts_arr
            } else {
                // Plain object: copy own enumerable string properties
                let ts_obj = ts_obj_new();
                let mut keys_napi: napi_value = std::ptr::null_mut();
                napi_get_property_names(env, value, &mut keys_napi);
                let mut keys_len: u32 = 0;
                napi_get_array_length(env, keys_napi, &mut keys_len);
                for i in 0..keys_len {
                    let mut key_napi: napi_value = std::ptr::null_mut();
                    napi_get_element(env, keys_napi, i, &mut key_napi);
                    // Convert key to TsString
                    let ts_key = napi_to_tsval(env, key_napi);
                    // Get property value using key string as C string
                    if ts_key.is_ptr() && heap_tag(ts_key) == 2 {
                        let key_str = &*(ts_key.as_ptr() as *const TsString);
                        let key_cstr = format!("{}\0", key_str.inner);
                        let mut prop_napi: napi_value = std::ptr::null_mut();
                        napi_get_named_property(env, value, key_cstr.as_ptr(), &mut prop_napi);
                        let ts_val = napi_to_tsval(env, prop_napi);
                        ts_obj_set_val_key(ts_obj, ts_key, ts_val);
                        ts_release_val(ts_val);
                    }
                    ts_release_val(ts_key);
                }
                ts_obj
            }
        }
        _ => UNDEFINED,
    }
}

// ── Generic napi callback ─────────────────────────────────────────────────────

/// Maximum number of arguments supported per call.
const MAX_NAPI_ARGS: usize = 16;

/// Generic napi callback used for all exported TypeScript functions.
/// `data` is a raw pointer to a heap-allocated `NapiExportEntry`.
unsafe extern "C" fn ts_napi_generic_callback(
    env: napi_env,
    info: napi_callback_info,
) -> napi_value {
    let mut argc: usize = MAX_NAPI_ARGS;
    let mut argv: [napi_value; MAX_NAPI_ARGS] = [std::ptr::null_mut(); MAX_NAPI_ARGS];
    let mut data: *mut () = std::ptr::null_mut();

    napi_get_cb_info(
        env,
        info,
        &mut argc,
        argv.as_mut_ptr(),
        std::ptr::null_mut(),
        &mut data,
    );

    let entry = &*(data as *const NapiExportEntry);
    let arity = entry.arity as usize;

    // Convert napi_value arguments to TsVal
    let mut ts_args: [TsVal; MAX_NAPI_ARGS] = [UNDEFINED; MAX_NAPI_ARGS];
    let nargs = argc.min(arity);
    for i in 0..nargs {
        ts_args[i] = napi_to_tsval(env, argv[i]);
    }

    // Call the compiled TypeScript function with the appropriate arity
    let result_ts = call_ts_fn(entry.fn_ptr, arity, &ts_args);

    // Release converted args
    for i in 0..nargs {
        ts_release_val(ts_args[i]);
    }

    // Convert result back to napi_value
    let result_napi = tsval_to_napi(env, result_ts);
    ts_release_val(result_ts);

    result_napi
}

/// Dispatch a call to a compiled TypeScript function with the given arity.
/// TypeScript functions have C calling convention: (i64, i64, ...) -> i64.
unsafe fn call_ts_fn(fn_ptr: *const u8, arity: usize, args: &[TsVal]) -> TsVal {
    let a = |i: usize| -> i64 {
        if i < args.len() { args[i].0 as i64 } else { UNDEFINED.0 as i64 }
    };

    let raw = fn_ptr as i64;

    let result: i64 = match arity {
        0 => {
            let f: unsafe extern "C" fn() -> i64 = std::mem::transmute(raw);
            f()
        }
        1 => {
            let f: unsafe extern "C" fn(i64) -> i64 = std::mem::transmute(raw);
            f(a(0))
        }
        2 => {
            let f: unsafe extern "C" fn(i64, i64) -> i64 = std::mem::transmute(raw);
            f(a(0), a(1))
        }
        3 => {
            let f: unsafe extern "C" fn(i64, i64, i64) -> i64 = std::mem::transmute(raw);
            f(a(0), a(1), a(2))
        }
        4 => {
            let f: unsafe extern "C" fn(i64, i64, i64, i64) -> i64 = std::mem::transmute(raw);
            f(a(0), a(1), a(2), a(3))
        }
        5 => {
            let f: unsafe extern "C" fn(i64, i64, i64, i64, i64) -> i64 =
                std::mem::transmute(raw);
            f(a(0), a(1), a(2), a(3), a(4))
        }
        6 => {
            let f: unsafe extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64 =
                std::mem::transmute(raw);
            f(a(0), a(1), a(2), a(3), a(4), a(5))
        }
        7 => {
            let f: unsafe extern "C" fn(i64, i64, i64, i64, i64, i64, i64) -> i64 =
                std::mem::transmute(raw);
            f(a(0), a(1), a(2), a(3), a(4), a(5), a(6))
        }
        _ => {
            // 8+ args: pass first 8
            let f: unsafe extern "C" fn(i64, i64, i64, i64, i64, i64, i64, i64) -> i64 =
                std::mem::transmute(raw);
            f(a(0), a(1), a(2), a(3), a(4), a(5), a(6), a(7))
        }
    };

    TsVal(result as u64)
}

// ── Node-API module entry point ───────────────────────────────────────────────

// Declare the compiled __napi_init() symbol — emitted by codegen in addon mode.
extern "C" {
    fn __napi_init();
}

/// Called by Node.js when the `.node` addon is loaded via `require()` or `import`.
/// Initialises the TypeScript module and registers all exported functions on `exports`.
#[no_mangle]
pub unsafe extern "C" fn napi_register_module_v1(
    env: napi_env,
    exports: napi_value,
) -> napi_value {
    // 1. Initialise the TypeScript module (runs module-level code, registers exports).
    __napi_init();

    // 2. Wrap each registered export in a napi function and attach it to `exports`.
    let registry = napi_exports().lock().unwrap();
    for entry in registry.iter() {
        // Box the entry so its address is stable for the lifetime of the module.
        let boxed: Box<NapiExportEntry> = Box::new(NapiExportEntry {
            name: entry.name.clone(),
            fn_ptr: entry.fn_ptr,
            arity: entry.arity,
        });
        let data_ptr = Box::into_raw(boxed) as *mut ();

        let mut fn_val: napi_value = std::ptr::null_mut();
        let name_bytes = entry.name.as_bytes_with_nul();
        napi_create_function(
            env,
            name_bytes.as_ptr(),
            NAPI_AUTO_LENGTH,
            ts_napi_generic_callback,
            data_ptr,
            &mut fn_val,
        );

        if !fn_val.is_null() {
            napi_set_named_property(env, exports, name_bytes.as_ptr(), fn_val);
        }
    }

    exports
}
