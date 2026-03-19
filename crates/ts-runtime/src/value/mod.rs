//! The universal TypeScript value representation.
//!
//! TypeScript values are NaN-boxed 64-bit words (similar to JavaScriptCore /
//! V8 Smi encoding).  This module defines the tag bits and boxing/unboxing
//! helpers that the compiler will emit calls to.
//!
//! Layout (NaN-boxing):
//!   - If the top 13 bits are all 1 (quiet NaN range) and bit 50 is 1 →
//!     tagged pointer or special value.
//!   - Otherwise → IEEE-754 double (JS `number`).
//!
//! Tags (bits 49..48 of the quiet-NaN word):
//!   00 → undefined / null (bit 47: 0=undefined, 1=null)
//!   01 → boolean         (bit 0: value)
//!   10 → pointer to heap object
//!   11 → small integer (int32 in lower 32 bits)

#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::missing_safety_doc)]

/// Opaque 64-bit value type.  The compiler treats every TS value as a `TsVal`.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TsVal(pub u64);

// Quiet NaN mask: bits 63..51 all set, plus the quiet bit (bit 50).
pub(crate) const NAN_MASK:       u64 = 0x7FF8_0000_0000_0000;
pub(crate) const TAG_MASK:       u64 = 0x0006_0000_0000_0000;
pub(crate) const TAG_UNDEFINED:  u64 = 0x0000_0000_0000_0000;
pub(crate) const TAG_NULL:       u64 = 0x0001_0000_0000_0000;
pub(crate) const TAG_BOOL:       u64 = 0x0002_0000_0000_0000;
pub(crate) const TAG_PTR:        u64 = 0x0004_0000_0000_0000;
pub(crate) const TAG_INT:        u64 = 0x0006_0000_0000_0000;

pub const UNDEFINED: TsVal = TsVal(NAN_MASK | TAG_UNDEFINED);
pub const NULL:      TsVal = TsVal(NAN_MASK | TAG_NULL);
pub const TRUE:      TsVal = TsVal(NAN_MASK | TAG_BOOL | 1);
pub const FALSE:     TsVal = TsVal(NAN_MASK | TAG_BOOL | 0);

// TsVal is a plain u64 with ARC-managed heap pointers — safe to transfer across threads.
unsafe impl Send for TsVal {}
unsafe impl Sync for TsVal {}

impl TsVal {
    #[inline]
    pub fn from_f64(n: f64) -> Self {
        Self(n.to_bits())
    }

    #[inline]
    pub fn from_i32(n: i32) -> Self {
        Self(NAN_MASK | TAG_INT | (n as u32 as u64))
    }

    #[inline]
    pub fn from_bool(b: bool) -> Self {
        if b { TRUE } else { FALSE }
    }

    #[inline]
    pub fn from_ptr(p: *mut u8) -> Self {
        Self(NAN_MASK | TAG_PTR | (p as u64 & 0x0000_FFFF_FFFF_FFFF))
    }

    #[inline]
    fn is_nan_boxed(self) -> bool {
        (self.0 & NAN_MASK) == NAN_MASK
    }

    #[inline]
    pub fn is_number(self) -> bool {
        !self.is_nan_boxed()
    }

    #[inline]
    pub fn is_undefined(self) -> bool {
        self.is_nan_boxed() && (self.0 & (TAG_MASK | 1)) == TAG_UNDEFINED
    }

    #[inline]
    pub fn is_null(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_NULL
    }

    #[inline]
    pub fn is_bool(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_BOOL
    }

    #[inline]
    pub fn is_ptr(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_PTR
    }

    #[inline]
    pub fn is_int32(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_INT
    }

    #[inline]
    pub fn as_f64(self) -> f64 {
        f64::from_bits(self.0)
    }

    #[inline]
    pub fn as_i32(self) -> i32 {
        (self.0 & 0x0000_0000_FFFF_FFFF) as i32
    }

    #[inline]
    pub fn as_bool(self) -> bool {
        (self.0 & 1) != 0
    }

    #[inline]
    pub fn as_ptr(self) -> *mut u8 {
        (self.0 & 0x0000_FFFF_FFFF_FFFF) as *mut u8
    }
}

// ── Heap Objects ──────────────────────────────────────────────────────────────

use std::collections::HashMap;

/// A heap-allocated TypeScript object.
pub struct TsObject {
    pub properties: HashMap<String, TsVal>,
}

/// A heap-allocated TypeScript array.
pub struct TsArray {
    pub elements: Vec<TsVal>,
}

/// A heap-allocated TypeScript string.
pub struct TsString {
    pub inner: String,
}

/// A heap-allocated TypeScript Map (tag = 5).
/// Maintains insertion order via a Vec of (key, value) pairs.
pub struct TsMap {
    pub entries: Vec<(TsVal, TsVal)>,
}

/// A heap-allocated RegExp value.
/// tag = 6
pub struct TsRegExp {
    pub source: String,
    pub flags: String,
}

/// A heap-allocated TypeScript function value (arrow function or function expression).
/// tag = 4
/// If `env` is not UNDEFINED, this is a closure and env is a TsArray of captured values.
/// The underlying MLIR function then takes `(env: TsVal, param0, param1, ...)`.
pub struct TsFunction {
    /// Pointer to the compiled native function.
    pub fn_ptr: *const u8,
    /// Number of declared parameters (NOT counting env or this).
    pub arity: u8,
    /// 1 if the function expects `this` as its first MLIR parameter.
    pub has_this: u8,
    /// 1 if the last MLIR parameter is a rest array (excess args get bundled into it).
    pub has_rest: u8,
    /// Captured environment (TsArray) or UNDEFINED if not a closure.
    pub env: TsVal,
}

/// Heap-allocated TypeScript Promise.
/// The inner value lives in an `OnceLock`; a `Notify` wakes any blockers.
pub struct TsPromise {
    pub resolved: std::sync::Arc<std::sync::OnceLock<TsVal>>,
    pub notify:   std::sync::Arc<tokio::sync::Notify>,
}

/// TsResponse heap type (tag=8).
pub struct TsResponse {
    pub status: u16,
    pub body: TsVal,    // TsString or NULL
    pub headers: TsVal, // TsHeaders (tag=7)
}

/// A heap-allocated JavaScript Symbol (tag=10).
/// Unique identity value; the `id` is a globally-monotonic counter.
/// `description` is the optional string passed to Symbol() — owned by this struct.
pub struct TsSymbol {
    pub id: u64,
    pub description: TsVal, // TsString or UNDEFINED
}

/// A heap-allocated JavaScript Set (tag=11).
/// Stores unique values in insertion order using same-value-zero equality.
pub struct TsSet {
    pub entries: Vec<TsVal>,
}

/// A heap-allocated JavaScript WeakMap (tag=12).
/// Keys are stored as raw pointers (not retained); values are strongly retained.
pub struct TsWeakMap {
    pub entries: Vec<(*mut u8, TsVal)>,
}

// TsWeakMap contains raw pointers but is accessed only from a single thread per object.
unsafe impl Send for TsWeakMap {}

/// A heap-allocated JavaScript WeakSet (tag=13).
/// Members are stored as raw pointers (not retained).
pub struct TsWeakSet {
    pub entries: Vec<*mut u8>,
}

/// TsDate heap type (tag=14). Stores Unix timestamp in milliseconds.
pub struct TsDate {
    pub millis: f64,
}

/// TsWeakRef heap type (tag=15).
/// Holds a strong reference to the target (ARC doesn't support true weak refs).
/// Functionally correct: deref() returns the target; GC-safety is not needed since we use ARC.
pub struct TsWeakRef {
    pub target: TsVal,
}

unsafe impl Send for TsWeakSet {}

// ── Submodule declarations ────────────────────────────────────────────────────

pub mod func;
pub mod object;
pub mod array;
pub mod string_val;
pub mod map;
pub mod regexp;
pub mod promise;
pub mod operators;
pub mod json;
pub mod uri;
pub mod globals;
pub mod http;
pub mod url;
pub mod symbol;
pub mod set;
pub mod weak;
pub mod container;
pub mod reflect;
pub mod date;
pub mod weakref;

// ── Re-exports from submodules ────────────────────────────────────────────────

pub use func::{
    ts_func_new, ts_func_new_this, ts_closure_new, ts_closure_new_rest, ts_closure_get_env,
    ts_func_bind,
    ts_func_call0, ts_func_call1, ts_func_call2, ts_func_call3, ts_func_call4,
    ts_method_call0, ts_method_call1, ts_method_call2, ts_method_call3, ts_method_call4,
};
pub use object::{
    ts_obj_new, ts_error_new, ts_obj_get, ts_obj_set, ts_obj_delete, ts_obj_delete_key,
    ts_val_has_key, ts_obj_set_val_key, ts_val_get_key,
    ts_obj_rest, ts_obj_keys, ts_obj_values, ts_obj_entries, ts_obj_merge,
    ts_obj_assign, ts_obj_create, ts_obj_from_entries,
    ts_obj_get_own_property_names, ts_obj_get_prototype_of, ts_obj_define_property,
    ts_obj_define_getter, ts_obj_define_setter,
    ts_structured_clone,
};
pub use array::{
    ts_arr_new, ts_arr_get, ts_arr_set, ts_arr_len, ts_iterable_len, ts_iterable_get, ts_normalize_iterable,
    ts_arr_reverse, ts_arr_fill, ts_arr_splice, ts_arr_slice_range,
    ts_arr_includes, ts_arr_last_index_of, ts_arr_copy_within,
    ts_arr_push, ts_arr_pop, ts_arr_unshift, ts_arr_shift, ts_arr_push_all, ts_arr_join,
    ts_arr_index_of, ts_arr_rest,
    ts_arr_map, ts_arr_filter, ts_arr_for_each, ts_arr_reduce, ts_arr_reduce_right,
    ts_arr_find, ts_arr_find_index, ts_arr_find_last, ts_arr_find_last_index, ts_arr_some, ts_arr_every,
    ts_arr_sort, ts_arr_flat_map, ts_arr_flat, ts_arr_concat,
    ts_arr_to_sorted, ts_arr_to_reversed, ts_arr_with,
    ts_arr_keys, ts_arr_values, ts_arr_entries,
};
pub use string_val::{
    ts_string_new, ts_string_concat, ts_str_trim_start, ts_str_trim_end,
    ts_val_to_string, ts_val_length,
    ts_str_index_of, ts_str_index_of_from, ts_str_last_index_of, ts_str_includes, ts_val_index_of, ts_val_includes,
    ts_str_slice, ts_str_substring, ts_str_to_upper, ts_str_to_lower, ts_str_trim,
    ts_str_split, ts_str_replace, ts_str_replace_all,
    ts_str_starts_with, ts_str_ends_with,
    ts_str_pad_start, ts_str_pad_end,
    ts_str_char_at, ts_str_char_code_at, ts_str_repeat, ts_str_from_char_code,
    ts_str_at, ts_val_at,
    ts_str_locale_compare,
};
pub use map::{
    ts_map_new, ts_map_set, ts_map_get, ts_map_has, ts_map_delete,
    ts_map_clear, ts_map_size, ts_map_keys, ts_map_values,
    ts_map_for_each, ts_map_entries,
    ts_map_from_arr,
};
pub use regexp::{
    ts_regexp_new, ts_regexp_from_val, ts_regexp_test, ts_regexp_exec,
    ts_regexp_source, ts_str_match, ts_str_replace_regex, ts_str_match_all, ts_str_search,
};
pub use promise::{
    ts_promise_resolve, ts_promise_await, ts_promise_destructor,
    ts_promise_race, ts_promise_race_all, ts_promise_all, ts_promise_all_settled, ts_promise_any, ts_promise_reject,
    ts_sleep,
    ts_set_timeout, ts_set_interval, ts_clear_timeout, ts_clear_interval, ts_queue_microtask,
    ts_async_spawn0, ts_async_spawn1, ts_async_spawn2, ts_async_spawn3, ts_async_spawn4,
};
pub use operators::{
    ts_number_is_integer, ts_number_is_finite, ts_number_is_nan, ts_number_is_safe_integer,
    ts_add, ts_sub, ts_mul, ts_div, ts_mod,
    ts_lt, ts_le, ts_gt, ts_ge,
    ts_math_abs, ts_math_floor, ts_math_ceil, ts_math_round, ts_math_sqrt,
    ts_math_trunc, ts_math_log, ts_math_log2, ts_math_log10,
    ts_math_sin, ts_math_cos, ts_math_tan, ts_math_sign,
    ts_math_asin, ts_math_acos, ts_math_atan, ts_math_sinh, ts_math_cosh, ts_math_tanh,
    ts_math_exp, ts_math_expm1, ts_math_log1p, ts_math_cbrt,
    ts_math_clz32, ts_math_fround, ts_math_imul, ts_math_random,
    ts_math_min, ts_math_max, ts_math_pow, ts_math_atan2, ts_math_hypot,
    ts_parse_int, ts_parse_float,
    ts_is_nan_val, ts_is_finite_val,
    ts_typeof, ts_val_strict_eq, ts_is_nullish, ts_is_truthy, ts_val_not, ts_is_undefined,
    ts_is_array, ts_func_spread_call, ts_method_spread_call,
    ts_coerce_number, ts_coerce_string, ts_coerce_bool,
    ts_num_to_fixed, ts_num_to_precision, ts_num_to_exponential,
};
pub use json::{ts_json_stringify, ts_json_parse};
pub use uri::{
    ts_encode_uri_component, ts_decode_uri_component, ts_encode_uri, ts_decode_uri,
};
pub use globals::{ts_set_module_global, ts_get_module_global, ts_process_exit, ts_process_argv, ts_process_env};
pub use http::{
    ts_headers_new, ts_headers_append, ts_headers_get_set_cookie,
    ts_headers_get, ts_headers_has, ts_headers_set, ts_headers_delete,
    ts_response_new, ts_response_clone,
    ts_response_status, ts_response_ok, ts_response_headers,
    ts_request_new, ts_fetch, ts_serve, ts_serve_worker,
    ts_add_event_listener, ts_remove_event_listener,
    ts_val_text, ts_val_json,
};
pub use url::{ts_url_new, ts_urlsearchparams_new, ts_urlsearchparams_to_string, ts_urlsearchparams_append, ts_urlsearchparams_get_all};
pub use symbol::{ts_symbol_new, ts_symbol_description, ts_symbol_iterator};
pub use set::{
    ts_set_new, ts_set_new_from_iter,
    ts_set_add, ts_set_has, ts_set_delete, ts_set_clear,
    ts_set_size, ts_set_keys, ts_set_values, ts_set_entries, ts_set_for_each,
};
pub use weak::{
    ts_weakmap_new, ts_weakmap_set, ts_weakmap_get, ts_weakmap_has, ts_weakmap_delete,
    ts_weakset_new, ts_weakset_add, ts_weakset_has, ts_weakset_delete,
};
pub use reflect::{
    ts_reflect_define_metadata, ts_reflect_get_metadata, ts_reflect_get_own_metadata,
    ts_reflect_has_metadata, ts_reflect_has_own_metadata,
    ts_reflect_get_metadata_keys, ts_reflect_get_own_metadata_keys,
    ts_reflect_delete_metadata,
};
pub use container::{
    ts_container_get, ts_container_set, ts_container_add,
    ts_container_has, ts_container_delete, ts_container_clear,
    ts_container_size, ts_container_keys, ts_container_values,
    ts_container_entries, ts_container_for_each,
};
pub use weakref::{ts_weakref_new, ts_weakref_deref};
pub use date::{
    ts_date_new, ts_date_from_val, ts_date_now,
    ts_date_get_time, ts_date_get_full_year, ts_date_get_month, ts_date_get_date,
    ts_date_get_day, ts_date_get_hours, ts_date_get_minutes, ts_date_get_seconds,
    ts_date_get_milliseconds, ts_date_to_iso_string, ts_date_to_locale_date_string,
    ts_date_to_locale_time_string, ts_date_to_string,
};

// ── ARC: retain / release ─────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn ts_retain_val(val: TsVal) {
    if val.is_ptr() {
        crate::alloc::ts_retain(val.as_ptr());
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_release_val(val: TsVal) {
    if val.is_ptr() {
        let ptr = val.as_ptr();
        let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
        let header = ptr.sub(header_size) as *mut crate::alloc::ArcHeader;
        let tag = (*header).tag;

        let destructor = match tag {
            0 => Some(object::ts_obj_destructor as unsafe extern "C" fn(*mut u8)),
            1 => Some(array::ts_arr_destructor as unsafe extern "C" fn(*mut u8)),
            2 => Some(string_val::ts_string_destructor as unsafe extern "C" fn(*mut u8)),
            3 => Some(promise::ts_promise_destructor as unsafe extern "C" fn(*mut u8)),
            4 => Some(func::ts_func_destructor as unsafe extern "C" fn(*mut u8)),
            5 => Some(map::ts_map_destructor as unsafe extern "C" fn(*mut u8)),
            6 => Some(regexp::ts_regexp_destructor as unsafe extern "C" fn(*mut u8)),
            7 => Some(http::ts_headers_destructor as unsafe extern "C" fn(*mut u8)),
            8 => Some(http::ts_response_destructor as unsafe extern "C" fn(*mut u8)),
            9  => Some(map::ts_map_destructor as unsafe extern "C" fn(*mut u8)), // URLSearchParams
            10 => Some(symbol::ts_symbol_destructor as unsafe extern "C" fn(*mut u8)),
            11 => Some(set::ts_set_destructor as unsafe extern "C" fn(*mut u8)),
            12 => Some(weak::ts_weakmap_destructor as unsafe extern "C" fn(*mut u8)),
            13 => Some(weak::ts_weakset_destructor as unsafe extern "C" fn(*mut u8)),
            14 => Some(date::ts_date_destructor as unsafe extern "C" fn(*mut u8)),
            15 => Some(weakref::ts_weakref_destructor as unsafe extern "C" fn(*mut u8)),
            _ => None,
        };

        crate::alloc::ts_release(ptr, destructor);
    }
}

// ── heap_tag ──────────────────────────────────────────────────────────────────

/// Read the heap-allocation tag (0=Object,1=Array,2=String,3=Promise) for a
/// pointer TsVal **without** modifying the reference count.
/// Returns 255 if `val` is not a heap pointer.
pub(crate) unsafe fn heap_tag(val: TsVal) -> u8 {
    if !val.is_ptr() { return 255; }
    let ptr = val.as_ptr();
    let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
    let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
    (*header).tag
}
