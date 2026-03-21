//! N-API (Node-API) host implementation.
//!
//! Provides the `napi_*` C functions that native `.node` addons link against.
//! These are exposed as `#[no_mangle]` symbols so the dynamic linker resolves
//! them when a `.node` shared library is `dlopen`'d.
//!
//! Entry point: `ts_napi_load(path)` — opens a `.node` file, calls its
//! `napi_register_module_v1`, and returns the exports as a `TsVal` TsObject.

use std::ffi::{c_void, CStr, CString};
use std::sync::Mutex;

use crate::value::{
    TsVal, TsNapiFunction, UNDEFINED, NULL, TRUE, FALSE,
    ts_retain_val, ts_release_val, heap_tag,
};
use crate::value::string_val::ts_string_new;
use crate::value::array::{ts_arr_new, ts_arr_len, ts_arr_get, ts_arr_set};
use crate::value::object::{ts_obj_new, ts_obj_get, ts_obj_set, ts_obj_keys};

/// Convenience: get a named property using a Rust &str.
unsafe fn ts_obj_get_str(obj: TsVal, key: &str) -> TsVal {
    let ckey = CString::new(key).unwrap_or_default();
    ts_obj_get(obj, ckey.as_ptr())
}

/// Convenience: set a named property using a Rust &str.
unsafe fn ts_obj_set_str(obj: TsVal, key: &str, val: TsVal) {
    let ckey = CString::new(key).unwrap_or_default();
    ts_obj_set(obj, ckey.as_ptr(), val);
}

// ── Public types (used by value/mod.rs and value/func.rs) ────────────────────

/// Per-module N-API environment.  One is allocated by `ts_napi_load` and lives
/// for the entire lifetime of the loaded addon.
pub struct NapiEnv {
    /// Stack of handle scopes.  Each scope owns the `Box<TsVal>` slots created
    /// while it is active.  Closing a scope releases all of them.
    scopes: Vec<Vec<*mut TsVal>>,
    /// Persistent references (outside any scope).
    refs: Vec<*mut NapiRef>,
    /// Pending exception value, if any.
    pending_exception: Option<TsVal>,
}

/// A persistent N-API reference that keeps a TsVal alive across scope closes.
pub struct NapiRef {
    value: TsVal,
    ref_count: u32,
}

/// The callback-info struct passed to every N-API callback invocation.
pub struct NapiCallbackInfo {
    pub this_arg: TsVal,
    pub args: Vec<TsVal>,
    pub data: *mut c_void,
}

// Opaque handle types required by the C ABI.
#[repr(C)] pub struct napi_env__([u8; 0]);
#[repr(C)] pub struct napi_value__([u8; 0]);
#[repr(C)] pub struct napi_handle_scope__([u8; 0]);
#[repr(C)] pub struct napi_callback_scope__([u8; 0]);
#[repr(C)] pub struct napi_callback_info__([u8; 0]);
#[repr(C)] pub struct napi_ref__([u8; 0]);

#[allow(non_camel_case_types)] pub type napi_env            = *mut napi_env__;
#[allow(non_camel_case_types)] pub type napi_value          = *mut napi_value__;
#[allow(non_camel_case_types)] pub type napi_handle_scope   = *mut napi_handle_scope__;
#[allow(non_camel_case_types)] pub type napi_callback_scope = *mut napi_callback_scope__;
#[allow(non_camel_case_types)] pub type napi_callback_info  = *mut napi_callback_info__;
#[allow(non_camel_case_types)] pub type napi_ref            = *mut napi_ref__;
#[allow(non_camel_case_types)] pub type napi_status         = i32;

/// Function pointer type for N-API callbacks.
#[allow(non_camel_case_types)]
pub type napi_callback = unsafe extern "C" fn(napi_env, napi_callback_info) -> napi_value;

// napi_status constants
pub const NAPI_OK: napi_status = 0;
pub const NAPI_INVALID_ARG: napi_status = 1;
pub const NAPI_GENERIC_FAILURE: napi_status = 9;
pub const NAPI_PENDING_EXCEPTION: napi_status = 10;

// napi_valuetype ordinals
const NAPI_UNDEFINED:  i32 = 0;
const NAPI_NULL:       i32 = 1;
const NAPI_BOOLEAN:    i32 = 2;
const NAPI_NUMBER:     i32 = 3;
const NAPI_STRING:     i32 = 4;
const NAPI_OBJECT:     i32 = 6;
const NAPI_FUNCTION:   i32 = 7;

const NAPI_AUTO_LENGTH: usize = usize::MAX;

// ── NapiEnv helpers ──────────────────────────────────────────────────────────

impl NapiEnv {
    fn new() -> Box<Self> {
        let mut env = Box::new(NapiEnv {
            scopes: Vec::new(),
            refs: Vec::new(),
            pending_exception: None,
        });
        // Push a root scope so napi_values created before the first
        // explicit napi_open_handle_scope are still tracked.
        env.scopes.push(Vec::new());
        env
    }

    /// Allocate a slot for `val` in the current scope, retaining it.
    /// Returns the slot pointer cast to `napi_value`.
    unsafe fn alloc_slot(&mut self, val: TsVal) -> napi_value {
        ts_retain_val(val);
        let slot = Box::into_raw(Box::new(val));
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(slot);
        }
        slot as *mut _ as napi_value
    }

    /// Read the TsVal stored in a slot without changing refcount.
    unsafe fn read_slot(nv: napi_value) -> TsVal {
        if nv.is_null() { return UNDEFINED; }
        *(nv as *mut TsVal)
    }
}

/// Destructor called by `ts_release_val` for heap tag 18 (TsNapiFunction).
pub unsafe extern "C" fn ts_napi_function_destructor(ptr: *mut u8) {
    let _ = Box::from_raw(ptr as *mut TsNapiFunction);
    // data lifetime is managed by the addon; we do not free it.
}

/// Called by `dispatch_callback` in func.rs when it encounters tag 18.
pub unsafe fn dispatch_napi_function(fn_val: TsVal, args: &[TsVal]) -> TsVal {
    let napi_fn = &*(fn_val.as_ptr() as *const TsNapiFunction);

    let info = Box::new(NapiCallbackInfo {
        this_arg: UNDEFINED,
        args: args.to_vec(),
        data: napi_fn.data,
    });
    let info_raw = Box::into_raw(info);

    let cb: napi_callback = std::mem::transmute(napi_fn.callback);
    let result_nv = cb(
        napi_fn.env as napi_env,
        info_raw as napi_callback_info,
    );

    let _ = Box::from_raw(info_raw); // drop the info box

    // result_nv is a slot pointer owned by some scope; read its value.
    if result_nv.is_null() {
        return UNDEFINED;
    }
    let result = NapiEnv::read_slot(result_nv);
    ts_retain_val(result);
    result
}

// ── Group 1: primitive value creation / reading ───────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn napi_get_undefined(env: napi_env, result: *mut napi_value) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(UNDEFINED);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_null(env: napi_env, result: *mut napi_value) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(NULL);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_boolean(env: napi_env, value: bool, result: *mut napi_value) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(if value { TRUE } else { FALSE });
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_create_int32(env: napi_env, value: i32, result: *mut napi_value) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(TsVal::from_i32(value));
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_create_uint32(env: napi_env, value: u32, result: *mut napi_value) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(TsVal::from_i32(value as i32));
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_create_int64(env: napi_env, value: i64, result: *mut napi_value) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    // Represent as i32 if in range, else f64.
    let ts = if value >= i32::MIN as i64 && value <= i32::MAX as i64 {
        TsVal::from_i32(value as i32)
    } else {
        TsVal::from_f64(value as f64)
    };
    *result = e.alloc_slot(ts);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_create_double(env: napi_env, value: f64, result: *mut napi_value) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    let ts = if value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64 {
        TsVal::from_i32(value as i32)
    } else {
        TsVal::from_f64(value)
    };
    *result = e.alloc_slot(ts);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_create_string_utf8(
    env: napi_env,
    str_ptr: *const u8,
    length: usize,
    result: *mut napi_value,
) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    let s = if length == NAPI_AUTO_LENGTH {
        // null-terminated
        let cstr = CStr::from_ptr(str_ptr as *const i8);
        ts_string_new(cstr.as_ptr())
    } else {
        let bytes = std::slice::from_raw_parts(str_ptr, length);
        let owned = String::from_utf8_lossy(bytes).into_owned();
        let cstring = CString::new(owned).unwrap_or_default();
        ts_string_new(cstring.as_ptr())
    };
    *result = e.alloc_slot(s);
    ts_release_val(s); // alloc_slot retained; release our own ref
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_typeof(
    _env: napi_env,
    value: napi_value,
    result: *mut i32,
) -> napi_status {
    let val = NapiEnv::read_slot(value);
    *result = if val.is_undefined() {
        NAPI_UNDEFINED
    } else if val.is_null() {
        NAPI_NULL
    } else if val.is_bool() {
        NAPI_BOOLEAN
    } else if val.is_number() || val.is_int32() {
        NAPI_NUMBER
    } else if val.is_ptr() {
        match heap_tag(val) {
            2 => NAPI_STRING,
            4 | 18 => NAPI_FUNCTION,
            _ => NAPI_OBJECT,
        }
    } else {
        NAPI_UNDEFINED
    };
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_value_double(
    _env: napi_env,
    value: napi_value,
    result: *mut f64,
) -> napi_status {
    let val = NapiEnv::read_slot(value);
    if val.is_int32() { *result = val.as_i32() as f64; }
    else if val.is_number() { *result = val.as_f64(); }
    else { *result = 0.0; return NAPI_INVALID_ARG; }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_value_int32(
    _env: napi_env,
    value: napi_value,
    result: *mut i32,
) -> napi_status {
    let val = NapiEnv::read_slot(value);
    if val.is_int32() { *result = val.as_i32(); }
    else if val.is_number() { *result = val.as_f64() as i32; }
    else { *result = 0; return NAPI_INVALID_ARG; }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_value_uint32(
    _env: napi_env,
    value: napi_value,
    result: *mut u32,
) -> napi_status {
    let val = NapiEnv::read_slot(value);
    if val.is_int32() { *result = val.as_i32() as u32; }
    else if val.is_number() { *result = val.as_f64() as u32; }
    else { *result = 0; return NAPI_INVALID_ARG; }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_value_int64(
    _env: napi_env,
    value: napi_value,
    result: *mut i64,
) -> napi_status {
    let val = NapiEnv::read_slot(value);
    if val.is_int32() { *result = val.as_i32() as i64; }
    else if val.is_number() { *result = val.as_f64() as i64; }
    else { *result = 0; return NAPI_INVALID_ARG; }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_value_bool(
    _env: napi_env,
    value: napi_value,
    result: *mut bool,
) -> napi_status {
    let val = NapiEnv::read_slot(value);
    if !val.is_bool() { *result = false; return NAPI_INVALID_ARG; }
    *result = val.as_bool();
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_value_string_utf8(
    _env: napi_env,
    value: napi_value,
    buf: *mut u8,
    bufsize: usize,
    result: *mut usize,
) -> napi_status {
    use crate::value::TsString;
    let val = NapiEnv::read_slot(value);
    if !val.is_ptr() || heap_tag(val) != 2 {
        if !result.is_null() { *result = 0; }
        return NAPI_INVALID_ARG;
    }
    let s = &*(val.as_ptr() as *const TsString);
    let bytes = s.inner.as_bytes();
    if !result.is_null() { *result = bytes.len(); }
    if !buf.is_null() && bufsize > 0 {
        let copy_len = bytes.len().min(bufsize - 1);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, copy_len);
        *buf.add(copy_len) = 0; // null terminate
        if !result.is_null() { *result = copy_len; }
    }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_coerce_to_string(
    env: napi_env,
    value: napi_value,
    result: *mut napi_value,
) -> napi_status {
    use crate::value::operators::ts_coerce_string;
    let val = NapiEnv::read_slot(value);
    ts_retain_val(val);
    let s = ts_coerce_string(val);
    ts_release_val(val);
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(s);
    ts_release_val(s);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_coerce_to_number(
    env: napi_env,
    value: napi_value,
    result: *mut napi_value,
) -> napi_status {
    use crate::value::operators::ts_coerce_number;
    let val = NapiEnv::read_slot(value);
    ts_retain_val(val);
    let n = ts_coerce_number(val);
    ts_release_val(val);
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(n);
    ts_release_val(n);
    NAPI_OK
}

// ── Group 2: object / array ───────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn napi_create_object(env: napi_env, result: *mut napi_value) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    let obj = ts_obj_new();
    *result = e.alloc_slot(obj);
    ts_release_val(obj);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_create_array(env: napi_env, result: *mut napi_value) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    let arr = ts_arr_new(0);
    *result = e.alloc_slot(arr);
    ts_release_val(arr);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_create_array_with_length(
    env: napi_env,
    length: usize,
    result: *mut napi_value,
) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    let arr = ts_arr_new(length as i32);
    *result = e.alloc_slot(arr);
    ts_release_val(arr);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_is_array(
    _env: napi_env,
    value: napi_value,
    result: *mut bool,
) -> napi_status {
    let val = NapiEnv::read_slot(value);
    *result = val.is_ptr() && heap_tag(val) == 1;
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_array_length(
    _env: napi_env,
    value: napi_value,
    result: *mut u32,
) -> napi_status {
    let val = NapiEnv::read_slot(value);
    if !val.is_ptr() || heap_tag(val) != 1 { *result = 0; return NAPI_INVALID_ARG; }
    let len = ts_arr_len(val);
    *result = if len.is_int32() { len.as_i32() as u32 } else { 0 };
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_element(
    env: napi_env,
    array: napi_value,
    index: u32,
    result: *mut napi_value,
) -> napi_status {
    let arr = NapiEnv::read_slot(array);
    if !arr.is_ptr() || heap_tag(arr) != 1 { return NAPI_INVALID_ARG; }
    let elem = ts_arr_get(arr, index as i32);
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(elem);
    ts_release_val(elem); // alloc_slot retained
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_set_element(
    _env: napi_env,
    array: napi_value,
    index: u32,
    value: napi_value,
) -> napi_status {
    let arr = NapiEnv::read_slot(array);
    if !arr.is_ptr() || heap_tag(arr) != 1 { return NAPI_INVALID_ARG; }
    let val = NapiEnv::read_slot(value);
    ts_arr_set(arr, index as i32, val);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_named_property(
    env: napi_env,
    object: napi_value,
    utf8name: *const u8,
    result: *mut napi_value,
) -> napi_status {
    let obj = NapiEnv::read_slot(object);
    let key = CStr::from_ptr(utf8name as *const i8).to_str().unwrap_or("");
    let val = ts_obj_get_str(obj, key);
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(val);
    ts_release_val(val);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_set_named_property(
    _env: napi_env,
    object: napi_value,
    utf8name: *const u8,
    value: napi_value,
) -> napi_status {
    let obj = NapiEnv::read_slot(object);
    if !obj.is_ptr() { return NAPI_INVALID_ARG; }
    let key = CStr::from_ptr(utf8name as *const i8).to_str().unwrap_or("").to_string();
    let val = NapiEnv::read_slot(value);
    ts_obj_set_str(obj, &key, val);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_property(
    env: napi_env,
    object: napi_value,
    key: napi_value,
    result: *mut napi_value,
) -> napi_status {
    let key_val = NapiEnv::read_slot(key);
    // Convert key to string for the property lookup.
    if key_val.is_ptr() && heap_tag(key_val) == 2 {
        use crate::value::TsString;
        let ks = &*(key_val.as_ptr() as *const TsString);
        let key_str = ks.inner.clone();
        let obj = NapiEnv::read_slot(object);
        let val = ts_obj_get_str(obj, &key_str);
        let e = &mut *(env as *mut NapiEnv);
        *result = e.alloc_slot(val);
        ts_release_val(val);
    } else {
        napi_get_undefined(env, result);
    }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_set_property(
    env: napi_env,
    object: napi_value,
    key: napi_value,
    value: napi_value,
) -> napi_status {
    let key_val = NapiEnv::read_slot(key);
    if key_val.is_ptr() && heap_tag(key_val) == 2 {
        use crate::value::TsString;
        let ks = &*(key_val.as_ptr() as *const TsString);
        let key_str = ks.inner.clone();
        let obj = NapiEnv::read_slot(object);
        let val = NapiEnv::read_slot(value);
        ts_obj_set_str(obj, &key_str, val);
    }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_has_own_property(
    _env: napi_env,
    object: napi_value,
    key: napi_value,
    result: *mut bool,
) -> napi_status {
    let obj = NapiEnv::read_slot(object);
    let key_val = NapiEnv::read_slot(key);
    if obj.is_ptr() && heap_tag(obj) == 0 && key_val.is_ptr() && heap_tag(key_val) == 2 {
        use crate::value::{TsString, TsObject};
        let ks = &*(key_val.as_ptr() as *const TsString);
        let o = &*(obj.as_ptr() as *const TsObject);
        *result = o.properties.contains_key(&ks.inner);
    } else {
        *result = false;
    }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_property_names(
    env: napi_env,
    object: napi_value,
    result: *mut napi_value,
) -> napi_status {
    let obj = NapiEnv::read_slot(object);
    let keys = ts_obj_keys(obj);
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(keys);
    ts_release_val(keys);
    NAPI_OK
}

// ── Group 3: functions and calls ──────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn napi_create_function(
    env: napi_env,
    _utf8name: *const u8,
    _length: usize,
    cb: napi_callback,
    data: *mut c_void,
    result: *mut napi_value,
) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    // Allocate a TsNapiFunction with tag 18.
    let size = std::mem::size_of::<TsNapiFunction>();
    let ptr = crate::alloc::ts_alloc_rc(size, 18) as *mut TsNapiFunction;
    if ptr.is_null() { return NAPI_GENERIC_FAILURE; }
    std::ptr::write(ptr, TsNapiFunction {
        callback: cb as *const u8,
        data,
        env: env as *mut NapiEnv,
    });
    let fn_val = TsVal::from_ptr(ptr as *mut u8);
    *result = e.alloc_slot(fn_val);
    ts_release_val(fn_val); // alloc_slot retained
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_call_function(
    env: napi_env,
    _recv: napi_value,
    func: napi_value,
    argc: usize,
    argv: *const napi_value,
    result: *mut napi_value,
) -> napi_status {
    let fn_val = NapiEnv::read_slot(func);
    // Build args slice.
    let args: Vec<TsVal> = (0..argc)
        .map(|i| NapiEnv::read_slot(*argv.add(i)))
        .collect();
    // Retain all args (dispatch_callback expects owned refs).
    for &a in &args { ts_retain_val(a); }

    let ret = crate::value::func::dispatch_callback_pub(fn_val, &args);

    for &a in &args { ts_release_val(a); }

    if !result.is_null() {
        let e = &mut *(env as *mut NapiEnv);
        *result = e.alloc_slot(ret);
        ts_release_val(ret);
    } else {
        ts_release_val(ret);
    }
    NAPI_OK
}

// ── Group 4: handle scopes and refs ──────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn napi_open_handle_scope(
    env: napi_env,
    result: *mut napi_handle_scope,
) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    e.scopes.push(Vec::new());
    // Return the index of the new scope as the handle (not a real pointer,
    // but sufficient for a matching close call).
    *result = e.scopes.len() as *mut napi_handle_scope__;
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_close_handle_scope(
    env: napi_env,
    _scope: napi_handle_scope,
) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    if let Some(scope) = e.scopes.pop() {
        for slot in scope {
            let val = *slot;
            ts_release_val(val);
            drop(Box::from_raw(slot));
        }
    }
    // Always keep at least the root scope.
    if e.scopes.is_empty() { e.scopes.push(Vec::new()); }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_create_reference(
    _env: napi_env,
    value: napi_value,
    initial_refcount: u32,
    result: *mut napi_ref,
) -> napi_status {
    let val = NapiEnv::read_slot(value);
    ts_retain_val(val);
    let r = Box::new(NapiRef { value: val, ref_count: initial_refcount });
    *result = Box::into_raw(r) as *mut _ as napi_ref;
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_delete_reference(
    _env: napi_env,
    ref_: napi_ref,
) -> napi_status {
    if ref_.is_null() { return NAPI_INVALID_ARG; }
    let r = Box::from_raw(ref_ as *mut NapiRef);
    ts_release_val(r.value);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_reference_ref(
    _env: napi_env,
    ref_: napi_ref,
    result: *mut u32,
) -> napi_status {
    let r = &mut *(ref_ as *mut NapiRef);
    r.ref_count += 1;
    if !result.is_null() { *result = r.ref_count; }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_reference_unref(
    _env: napi_env,
    ref_: napi_ref,
    result: *mut u32,
) -> napi_status {
    let r = &mut *(ref_ as *mut NapiRef);
    r.ref_count = r.ref_count.saturating_sub(1);
    if !result.is_null() { *result = r.ref_count; }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_reference_value(
    env: napi_env,
    ref_: napi_ref,
    result: *mut napi_value,
) -> napi_status {
    let r = &*(ref_ as *const NapiRef);
    let e = &mut *(env as *mut NapiEnv);
    *result = e.alloc_slot(r.value);
    ts_release_val(r.value); // alloc_slot retained; undo extra retain
    NAPI_OK
}

// ── Group 5: errors ───────────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn napi_throw_error(
    env: napi_env,
    _code: *const u8,
    msg: *const u8,
) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    let message = if msg.is_null() {
        "unknown napi error".to_string()
    } else {
        CStr::from_ptr(msg as *const i8).to_string_lossy().into_owned()
    };
    let cstr = CString::new(message).unwrap_or_default();
    let ts_str = ts_string_new(cstr.as_ptr());
    if let Some(prev) = e.pending_exception.take() { ts_release_val(prev); }
    e.pending_exception = Some(ts_str);
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_throw_type_error(
    env: napi_env,
    code: *const u8,
    msg: *const u8,
) -> napi_status {
    napi_throw_error(env, code, msg)
}

#[no_mangle]
pub unsafe extern "C" fn napi_throw_range_error(
    env: napi_env,
    code: *const u8,
    msg: *const u8,
) -> napi_status {
    napi_throw_error(env, code, msg)
}

#[no_mangle]
pub unsafe extern "C" fn napi_is_exception_pending(
    env: napi_env,
    result: *mut bool,
) -> napi_status {
    let e = &*(env as *const NapiEnv);
    *result = e.pending_exception.is_some();
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_and_clear_last_exception(
    env: napi_env,
    result: *mut napi_value,
) -> napi_status {
    let e = &mut *(env as *mut NapiEnv);
    let val = e.pending_exception.take().unwrap_or(UNDEFINED);
    *result = e.alloc_slot(val);
    ts_release_val(val); // alloc_slot retained
    NAPI_OK
}

// ── No-op stubs for less critical functions ───────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn napi_get_last_error_info(
    _env: napi_env,
    result: *mut *const u8,
) -> napi_status {
    if !result.is_null() { *result = std::ptr::null(); }
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_adjust_external_memory(
    _env: napi_env,
    _change_in_bytes: i64,
    _result: *mut i64,
) -> napi_status {
    NAPI_OK
}

#[no_mangle]
pub unsafe extern "C" fn napi_get_node_version(
    env: napi_env,
    result: *mut napi_value,
) -> napi_status {
    // Return an object {major:20, minor:0, patch:0, release:"node"}
    let e = &mut *(env as *mut NapiEnv);
    let obj = ts_obj_new();
    ts_obj_set_str(obj, "major", TsVal::from_i32(20));
    ts_obj_set_str(obj, "minor", TsVal::from_i32(0));
    ts_obj_set_str(obj, "patch", TsVal::from_i32(0));
    let rel = ts_string_new(b"node\0".as_ptr() as *const i8);
    ts_obj_set_str(obj, "release", rel);
    ts_release_val(rel);
    *result = e.alloc_slot(obj);
    ts_release_val(obj);
    NAPI_OK
}

// ── Module loading: ts_napi_load ─────────────────────────────────────────────

/// Loaded module registry: path → TsObject of exports.
static LOADED_MODULES: std::sync::OnceLock<Mutex<std::collections::HashMap<String, TsVal>>> =
    std::sync::OnceLock::new();

fn module_registry() -> &'static Mutex<std::collections::HashMap<String, TsVal>> {
    LOADED_MODULES.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Open a `.node` native addon, call its `napi_register_module_v1`, and return
/// the resulting exports as a TsObject (owned reference).
///
/// On failure returns `UNDEFINED`.
#[no_mangle]
pub unsafe extern "C" fn ts_napi_load(path_ptr: *const i8) -> TsVal {
    let path = CStr::from_ptr(path_ptr).to_string_lossy().into_owned();

    // Return cached exports if already loaded.
    {
        let reg = module_registry().lock().unwrap();
        if let Some(&cached) = reg.get(&path) {
            ts_retain_val(cached);
            return cached;
        }
    }

    #[cfg(unix)]
    {
        use std::ffi::CString;

        // RTLD_NOW | RTLD_GLOBAL so the addon's symbols are globally visible.
        let path_c = CString::new(path.clone()).unwrap();
        let handle = libc::dlopen(path_c.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
        if handle.is_null() {
            let err = CStr::from_ptr(libc::dlerror()).to_string_lossy();
            eprintln!("ts_napi_load: dlopen failed for {path}: {err}");
            return UNDEFINED;
        }

        let sym_name = CString::new("napi_register_module_v1").unwrap();
        let sym = libc::dlsym(handle, sym_name.as_ptr());
        if sym.is_null() {
            eprintln!("ts_napi_load: no napi_register_module_v1 in {path}");
            libc::dlclose(handle);
            return UNDEFINED;
        }

        // Allocate and initialise the NapiEnv.
        let mut env_box = NapiEnv::new();
        let env_ptr = &mut *env_box as *mut NapiEnv;
        // Leak the env — it lives for the module's lifetime.
        std::mem::forget(env_box);

        let env = env_ptr as napi_env;

        // Create the exports object.
        let exports_val = ts_obj_new();
        let exports_nv = (*(env as *mut NapiEnv)).alloc_slot(exports_val);
        ts_release_val(exports_val); // alloc_slot retained

        // Call napi_register_module_v1(env, exports).
        type RegisterFn = unsafe extern "C" fn(napi_env, napi_value) -> napi_value;
        let register_fn: RegisterFn = std::mem::transmute(sym);
        let result_nv = register_fn(env, exports_nv);

        // Extract the returned exports TsVal (may differ from our exports object).
        let result_val = if !result_nv.is_null() {
            let v = NapiEnv::read_slot(result_nv);
            ts_retain_val(v);
            v
        } else {
            NapiEnv::read_slot(exports_nv)
        };

        // Cache and return.
        {
            let mut reg = module_registry().lock().unwrap();
            ts_retain_val(result_val); // keep one ref in the cache
            reg.insert(path, result_val);
        }

        result_val
    }

    #[cfg(not(unix))]
    {
        eprintln!("ts_napi_load: not supported on this platform");
        UNDEFINED
    }
}
