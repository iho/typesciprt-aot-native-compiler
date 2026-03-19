//! Arithmetic, comparison, type-conversion, and other operators.

use super::{TsVal, TsString, UNDEFINED, NULL, TRUE, FALSE, TAG_MASK, TAG_UNDEFINED, TAG_NULL, TAG_BOOL, TAG_INT, TAG_PTR, heap_tag, ts_release_val};
use super::string_val::{ts_string_new, ts_val_to_string, ts_string_concat};
use super::array::ts_arr_get;
use super::func::{dispatch_callback, dispatch_method_callback};
use super::array::ts_val_is_truthy;

// ── Numeric helpers ───────────────────────────────────────────────────────────

/// Convert a TsVal to f64 for numeric operations (JS ToNumber semantics).
pub(super) fn ts_val_to_f64_raw(val: TsVal) -> f64 {
    if val.is_int32()  { return val.as_i32() as f64; }
    if val.is_number() { return val.as_f64(); }
    if val.is_bool()   { return if val.as_bool() { 1.0 } else { 0.0 }; }
    if val.is_null()   { return 0.0; }
    // undefined → NaN
    f64::NAN
}

/// Convert f64 back to TsVal: prefer i32 if the value is an exact integer.
pub(super) fn f64_to_ts_num(n: f64) -> TsVal {
    if n.is_finite() && n == n.trunc() && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
        TsVal::from_i32(n as i32)
    } else {
        TsVal::from_f64(n)
    }
}

// ── Arithmetic ────────────────────────────────────────────────────────────────

/// Polymorphic `+`: integer add, float add, or string concat (JS semantics).
#[no_mangle]
pub unsafe extern "C" fn ts_add(a: TsVal, b: TsVal) -> TsVal {
    // Integer fast path
    if a.is_int32() && b.is_int32() {
        return TsVal::from_i32(a.as_i32().wrapping_add(b.as_i32()));
    }
    // If either operand is a string, do string concatenation.
    let a_str = a.is_ptr() && heap_tag(a) == 2;
    let b_str = b.is_ptr() && heap_tag(b) == 2;
    if a_str || b_str {
        let sa = ts_val_to_string(a);
        let sb = ts_val_to_string(b);
        let result = ts_string_concat(sa, sb);
        ts_release_val(sa);
        ts_release_val(sb);
        return result;
    }
    // Numeric addition: handles int+float, bool+int, etc.
    let fa = ts_val_to_f64_raw(a);
    let fb = ts_val_to_f64_raw(b);
    f64_to_ts_num(fa + fb)
}

/// Polymorphic subtraction: i32-i32 → i32; otherwise f64.
#[no_mangle]
pub unsafe extern "C" fn ts_sub(a: TsVal, b: TsVal) -> TsVal {
    if a.is_int32() && b.is_int32() {
        TsVal::from_i32(a.as_i32().wrapping_sub(b.as_i32()))
    } else {
        f64_to_ts_num(ts_val_to_f64_raw(a) - ts_val_to_f64_raw(b))
    }
}

/// Polymorphic multiplication: i32*i32 → i32; otherwise f64.
#[no_mangle]
pub unsafe extern "C" fn ts_mul(a: TsVal, b: TsVal) -> TsVal {
    if a.is_int32() && b.is_int32() {
        TsVal::from_i32(a.as_i32().wrapping_mul(b.as_i32()))
    } else {
        f64_to_ts_num(ts_val_to_f64_raw(a) * ts_val_to_f64_raw(b))
    }
}

/// Polymorphic division: integer (exact) or f64.
#[no_mangle]
pub unsafe extern "C" fn ts_div(a: TsVal, b: TsVal) -> TsVal {
    if a.is_int32() && b.is_int32() {
        let av = a.as_i32();
        let bv = b.as_i32();
        if bv == 0 { return TsVal::from_f64(if av == 0 { f64::NAN } else if av > 0 { f64::INFINITY } else { f64::NEG_INFINITY }); }
        if av % bv == 0 { TsVal::from_i32(av / bv) } else { f64_to_ts_num(av as f64 / bv as f64) }
    } else {
        f64_to_ts_num(ts_val_to_f64_raw(a) / ts_val_to_f64_raw(b))
    }
}

/// Polymorphic remainder (%).
#[no_mangle]
pub unsafe extern "C" fn ts_mod(a: TsVal, b: TsVal) -> TsVal {
    if a.is_int32() && b.is_int32() {
        let bv = b.as_i32();
        if bv == 0 { TsVal::from_f64(f64::NAN) } else { TsVal::from_i32(a.as_i32() % bv) }
    } else {
        f64_to_ts_num(ts_val_to_f64_raw(a) % ts_val_to_f64_raw(b))
    }
}

// ── Comparisons ───────────────────────────────────────────────────────────────

/// JS abstract relational comparison: strings compared lexicographically, else numerically.
unsafe fn abstract_lt(a: TsVal, b: TsVal) -> Option<bool> {
    if a.is_ptr() && heap_tag(a) == 2 && b.is_ptr() && heap_tag(b) == 2 {
        let sa = &*(a.as_ptr() as *const TsString);
        let sb = &*(b.as_ptr() as *const TsString);
        return Some(sa.inner < sb.inner);
    }
    let fa = ts_val_to_f64_raw(a);
    let fb = ts_val_to_f64_raw(b);
    if fa.is_nan() || fb.is_nan() { None } else { Some(fa < fb) }
}

#[no_mangle] pub unsafe extern "C" fn ts_lt(a: TsVal, b: TsVal) -> i32 { abstract_lt(a, b).map_or(0, |v| v as i32) }
#[no_mangle] pub unsafe extern "C" fn ts_le(a: TsVal, b: TsVal) -> i32 { abstract_lt(b, a).map_or(0, |v| !v as i32) }
#[no_mangle] pub unsafe extern "C" fn ts_gt(a: TsVal, b: TsVal) -> i32 { abstract_lt(b, a).map_or(0, |v| v as i32) }
#[no_mangle] pub unsafe extern "C" fn ts_ge(a: TsVal, b: TsVal) -> i32 { abstract_lt(a, b).map_or(0, |v| !v as i32) }

// ── Math built-ins ────────────────────────────────────────────────────────────

#[no_mangle] pub unsafe extern "C" fn ts_math_abs(v: TsVal) -> TsVal   { f64_to_ts_num(ts_val_to_f64_raw(v).abs()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_floor(v: TsVal) -> TsVal { f64_to_ts_num(ts_val_to_f64_raw(v).floor()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_ceil(v: TsVal) -> TsVal  { f64_to_ts_num(ts_val_to_f64_raw(v).ceil()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_round(v: TsVal) -> TsVal { f64_to_ts_num(ts_val_to_f64_raw(v).round()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_sqrt(v: TsVal) -> TsVal  { f64_to_ts_num(ts_val_to_f64_raw(v).sqrt()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_trunc(v: TsVal) -> TsVal { f64_to_ts_num(ts_val_to_f64_raw(v).trunc()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_log(v: TsVal) -> TsVal   { f64_to_ts_num(ts_val_to_f64_raw(v).ln()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_log2(v: TsVal) -> TsVal  { f64_to_ts_num(ts_val_to_f64_raw(v).log2()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_log10(v: TsVal) -> TsVal { f64_to_ts_num(ts_val_to_f64_raw(v).log10()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_sin(v: TsVal) -> TsVal   { f64_to_ts_num(ts_val_to_f64_raw(v).sin()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_cos(v: TsVal) -> TsVal   { f64_to_ts_num(ts_val_to_f64_raw(v).cos()) }
#[no_mangle] pub unsafe extern "C" fn ts_math_tan(v: TsVal) -> TsVal   { f64_to_ts_num(ts_val_to_f64_raw(v).tan()) }

#[no_mangle]
pub unsafe extern "C" fn ts_math_min(a: TsVal, b: TsVal) -> TsVal {
    f64_to_ts_num(ts_val_to_f64_raw(a).min(ts_val_to_f64_raw(b)))
}
#[no_mangle]
pub unsafe extern "C" fn ts_math_max(a: TsVal, b: TsVal) -> TsVal {
    f64_to_ts_num(ts_val_to_f64_raw(a).max(ts_val_to_f64_raw(b)))
}
#[no_mangle]
pub unsafe extern "C" fn ts_math_pow(base: TsVal, exp: TsVal) -> TsVal {
    f64_to_ts_num(ts_val_to_f64_raw(base).powf(ts_val_to_f64_raw(exp)))
}
#[no_mangle]
pub unsafe extern "C" fn ts_math_atan2(y: TsVal, x: TsVal) -> TsVal {
    f64_to_ts_num(ts_val_to_f64_raw(y).atan2(ts_val_to_f64_raw(x)))
}
#[no_mangle]
pub unsafe extern "C" fn ts_math_hypot(a: TsVal, b: TsVal) -> TsVal {
    f64_to_ts_num(ts_val_to_f64_raw(a).hypot(ts_val_to_f64_raw(b)))
}

// ── Type introspection ────────────────────────────────────────────────────────

/// Returns a new string TsVal describing the JavaScript `typeof` the value.
#[no_mangle]
pub unsafe extern "C" fn ts_typeof(val: TsVal) -> TsVal {
    let type_bytes: &'static [u8] = if !val.is_nan_boxed() {
        b"number\0"
    } else {
        match val.0 & TAG_MASK {
            TAG_UNDEFINED => b"undefined\0",
            TAG_NULL      => b"object\0",    // historical JS semantics
            TAG_BOOL      => b"boolean\0",
            TAG_INT       => b"number\0",
            TAG_PTR       => match heap_tag(val) {
                2  => b"string\0",
                4  => b"function\0",
                10 => b"symbol\0",
                _  => b"object\0",
            },
            _ => b"undefined\0",
        }
    };
    ts_string_new(type_bytes.as_ptr() as *const i8)
}

/// Strict equality (`===`).  Returns 1 if equal, 0 otherwise.
#[no_mangle]
pub unsafe extern "C" fn ts_val_strict_eq(a: TsVal, b: TsVal) -> i32 {
    if a.0 == b.0 { return 1; }
    if a.is_ptr() && b.is_ptr() && heap_tag(a) == 2 && heap_tag(b) == 2 {
        let sa = &*(a.as_ptr() as *const TsString);
        let sb = &*(b.as_ptr() as *const TsString);
        return if sa.inner == sb.inner { 1 } else { 0 };
    }
    0
}

/// Returns 1 if `val` is `null` or `undefined`, 0 otherwise.
#[no_mangle]
pub unsafe extern "C" fn ts_is_nullish(val: TsVal) -> i32 {
    if val.is_null() || val.is_undefined() { 1 } else { 0 }
}

/// Returns 1 if `val` is truthy (JS semantics), 0 otherwise.
#[no_mangle]
pub unsafe extern "C" fn ts_is_truthy(val: TsVal) -> i32 {
    if ts_val_is_truthy(val) { 1 } else { 0 }
}

/// Logical NOT of a truthy i32 result (0 → 1, non-zero → 0).
#[no_mangle]
pub unsafe extern "C" fn ts_val_not(v: i32) -> i32 {
    if v == 0 { 1 } else { 0 }
}

/// Returns 1 if `val` is `undefined`, 0 otherwise.
#[no_mangle]
pub unsafe extern "C" fn ts_is_undefined(val: TsVal) -> i32 {
    if val.is_undefined() { 1 } else { 0 }
}

/// Returns 1 if `val` is a TsArray, 0 otherwise (for `Array.isArray`).
#[no_mangle]
pub unsafe extern "C" fn ts_is_array(val: TsVal) -> i32 {
    if val.is_ptr() && heap_tag(val) == 1 { 1 } else { 0 }
}

// ── Global parsing functions ──────────────────────────────────────────────────

/// Parse the string `s` as an integer in the given `radix` (default 10).
#[no_mangle]
pub unsafe extern "C" fn ts_parse_int(s_val: TsVal, radix_val: TsVal) -> TsVal {
    let s = if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let ts_str = &*(s_val.as_ptr() as *const TsString);
        ts_str.inner.trim().to_string()
    } else {
        return TsVal::from_f64(f64::NAN);
    };
    let radix = if radix_val.is_int32() { radix_val.as_i32() as u32 } else { 10u32 };
    let radix = if radix < 2 || radix > 36 { 10 } else { radix };
    let (sign, digits) = if s.starts_with('-') { (-1i64, &s[1..]) }
                         else if s.starts_with('+') { (1i64, &s[1..]) }
                         else { (1i64, s.as_str()) };
    let valid: String = digits.chars().take_while(|c| c.is_digit(radix)).collect();
    if valid.is_empty() { return TsVal::from_f64(f64::NAN); }
    match i64::from_str_radix(&valid, radix) {
        Ok(n) => f64_to_ts_num((sign * n) as f64),
        Err(_) => TsVal::from_f64(f64::NAN),
    }
}

/// Parse the string `s` as a floating-point number. Returns NaN if invalid.
#[no_mangle]
pub unsafe extern "C" fn ts_parse_float(s_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let ts_str = &*(s_val.as_ptr() as *const TsString);
        let s = ts_str.inner.trim();
        // JS-style: parse the longest valid numeric prefix
        let end = js_float_prefix_end(s);
        if end == 0 {
            return TsVal::from_f64(f64::NAN);
        }
        match s[..end].parse::<f64>() {
            Ok(n) => f64_to_ts_num(n),
            Err(_) => TsVal::from_f64(f64::NAN),
        }
    } else {
        TsVal::from_f64(f64::NAN)
    }
}

/// Returns the byte length of the longest JS-float prefix in `s`.
fn js_float_prefix_end(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    let n = b.len();
    if i < n && (b[i] == b'+' || b[i] == b'-') { i += 1; }
    // Infinity
    if s[i..].starts_with("Infinity") { return i + 8; }
    let mut has_digits = false;
    while i < n && b[i].is_ascii_digit() { has_digits = true; i += 1; }
    if i < n && b[i] == b'.' {
        i += 1;
        while i < n && b[i].is_ascii_digit() { has_digits = true; i += 1; }
    }
    if !has_digits { return 0; }
    // Optional exponent
    if i < n && (b[i] == b'e' || b[i] == b'E') {
        let j = i + 1;
        let mut k = j;
        if k < n && (b[k] == b'+' || b[k] == b'-') { k += 1; }
        let mut exp_digits = false;
        while k < n && b[k].is_ascii_digit() { exp_digits = true; k += 1; }
        if exp_digits { i = k; }
    }
    i
}

/// Returns 1 (true) if `val` is NaN.
#[no_mangle]
pub unsafe extern "C" fn ts_is_nan_val(val: TsVal) -> TsVal {
    if val.is_number() {
        TsVal::from_bool(val.as_f64().is_nan())
    } else {
        TsVal::from_bool(false) // ints, strings, etc. are not NaN
    }
}

/// Returns 1 (true) if `val` is a finite number.
#[no_mangle]
pub unsafe extern "C" fn ts_is_finite_val(val: TsVal) -> TsVal {
    if val.is_int32() {
        TsVal::from_bool(true)
    } else if val.is_number() {
        TsVal::from_bool(val.as_f64().is_finite())
    } else {
        TsVal::from_bool(false)
    }
}

// ── Number static methods (no coercion — strict type checks) ─────────────────

/// `Number.isInteger(val)` — true iff val is a finite integer (no coercion).
#[no_mangle]
pub unsafe extern "C" fn ts_number_is_integer(val: TsVal) -> TsVal {
    if val.is_int32() { return TsVal::from_bool(true); }
    if val.is_number() {
        let f = val.as_f64();
        return TsVal::from_bool(f.is_finite() && f.trunc() == f);
    }
    TsVal::from_bool(false)
}

/// `Number.isFinite(val)` — true iff val is a finite number (no coercion).
#[no_mangle]
pub unsafe extern "C" fn ts_number_is_finite(val: TsVal) -> TsVal {
    if val.is_int32() { return TsVal::from_bool(true); }
    if val.is_number() { return TsVal::from_bool(val.as_f64().is_finite()); }
    TsVal::from_bool(false)
}

/// `Number.isNaN(val)` — true iff val is NaN (no coercion).
#[no_mangle]
pub unsafe extern "C" fn ts_number_is_nan(val: TsVal) -> TsVal {
    if val.is_number() { return TsVal::from_bool(val.as_f64().is_nan()); }
    TsVal::from_bool(false)
}

/// `Number.isSafeInteger(val)` — true iff val is integer in [-2^53+1, 2^53-1].
#[no_mangle]
pub unsafe extern "C" fn ts_number_is_safe_integer(val: TsVal) -> TsVal {
    if val.is_int32() { return TsVal::from_bool(true); }
    if val.is_number() {
        let f = val.as_f64();
        let max = 9007199254740991.0_f64; // 2^53 - 1
        return TsVal::from_bool(f.is_finite() && f.trunc() == f && f.abs() <= max);
    }
    TsVal::from_bool(false)
}

// ── Coercion ──────────────────────────────────────────────────────────────────

/// Number(val) — converts any TsVal to a numeric TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_coerce_number(val: TsVal) -> TsVal {
    if val.is_int32() { return val; }
    if val.is_number() { return val; } // already float
    if val == TRUE { return TsVal::from_i32(1); }
    if val == FALSE || val == NULL { return TsVal::from_i32(0); }
    if val == UNDEFINED { return TsVal::from_f64(f64::NAN); }
    if val.is_ptr() && heap_tag(val) == 2 {
        let ts_str = &*(val.as_ptr() as *const TsString);
        let s = ts_str.inner.trim();
        if s.is_empty() { return TsVal::from_i32(0); }
        if let Ok(i) = s.parse::<i32>() { return TsVal::from_i32(i); }
        if let Ok(f) = s.parse::<f64>() { return TsVal::from_f64(f); }
    }
    TsVal::from_f64(f64::NAN)
}

/// String(val) — converts any TsVal to a string TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_coerce_string(val: TsVal) -> TsVal {
    ts_val_to_string(val)
}

#[no_mangle]
pub unsafe extern "C" fn ts_coerce_bool(val: TsVal) -> TsVal {
    TsVal::from_bool(ts_val_is_truthy(val))
}

// ── Spread function call ──────────────────────────────────────────────────────

/// Call a TsFunction with arguments spread from a TsArray.
#[no_mangle]
pub unsafe extern "C" fn ts_func_spread_call(fn_val: TsVal, args_arr: TsVal) -> TsVal {
    let len = if args_arr.is_ptr() && heap_tag(args_arr) == 1 {
        use super::TsArray;
        (*(args_arr.as_ptr() as *const TsArray)).elements.len()
    } else {
        0
    };
    match len {
        0 => dispatch_callback(fn_val, &[]),
        1 => {
            let a0 = ts_arr_get(args_arr, 0);
            let r = dispatch_callback(fn_val, &[a0]);
            ts_release_val(a0);
            r
        }
        2 => {
            let a0 = ts_arr_get(args_arr, 0);
            let a1 = ts_arr_get(args_arr, 1);
            let r = dispatch_callback(fn_val, &[a0, a1]);
            ts_release_val(a0);
            ts_release_val(a1);
            r
        }
        3 => {
            let a0 = ts_arr_get(args_arr, 0);
            let a1 = ts_arr_get(args_arr, 1);
            let a2 = ts_arr_get(args_arr, 2);
            let r = dispatch_callback(fn_val, &[a0, a1, a2]);
            ts_release_val(a0);
            ts_release_val(a1);
            ts_release_val(a2);
            r
        }
        _ => {
            let a0 = ts_arr_get(args_arr, 0);
            let a1 = ts_arr_get(args_arr, 1);
            let a2 = ts_arr_get(args_arr, 2);
            let a3 = ts_arr_get(args_arr, 3);
            let r = dispatch_callback(fn_val, &[a0, a1, a2, a3]);
            ts_release_val(a0);
            ts_release_val(a1);
            ts_release_val(a2);
            ts_release_val(a3);
            r
        }
    }
}

/// Call a method (with `this`) with arguments spread from a TsArray.
#[no_mangle]
pub unsafe extern "C" fn ts_method_spread_call(fn_val: TsVal, obj: TsVal, args_arr: TsVal) -> TsVal {
    let len = if args_arr.is_ptr() && heap_tag(args_arr) == 1 {
        use super::TsArray;
        (*(args_arr.as_ptr() as *const TsArray)).elements.len()
    } else {
        0
    };
    match len {
        0 => dispatch_method_callback(fn_val, obj, &[]),
        1 => {
            let a0 = ts_arr_get(args_arr, 0);
            let r = dispatch_method_callback(fn_val, obj, &[a0]);
            ts_release_val(a0);
            r
        }
        2 => {
            let a0 = ts_arr_get(args_arr, 0);
            let a1 = ts_arr_get(args_arr, 1);
            let r = dispatch_method_callback(fn_val, obj, &[a0, a1]);
            ts_release_val(a0);
            ts_release_val(a1);
            r
        }
        3 => {
            let a0 = ts_arr_get(args_arr, 0);
            let a1 = ts_arr_get(args_arr, 1);
            let a2 = ts_arr_get(args_arr, 2);
            let r = dispatch_method_callback(fn_val, obj, &[a0, a1, a2]);
            ts_release_val(a0);
            ts_release_val(a1);
            ts_release_val(a2);
            r
        }
        4 => {
            let a0 = ts_arr_get(args_arr, 0);
            let a1 = ts_arr_get(args_arr, 1);
            let a2 = ts_arr_get(args_arr, 2);
            let a3 = ts_arr_get(args_arr, 3);
            let r = dispatch_method_callback(fn_val, obj, &[a0, a1, a2, a3]);
            ts_release_val(a0);
            ts_release_val(a1);
            ts_release_val(a2);
            ts_release_val(a3);
            r
        }
        _ => {
            let mut args_vec = Vec::with_capacity(len);
            for i in 0..len {
                args_vec.push(ts_arr_get(args_arr, i as i32));
            }
            let r = dispatch_method_callback(fn_val, obj, &args_vec);
            for a in &args_vec {
                ts_release_val(*a);
            }
            r
        }
    }
}
