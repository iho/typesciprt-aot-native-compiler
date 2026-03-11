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

/// Opaque 64-bit value type.  The compiler treats every TS value as a `TsVal`.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TsVal(pub u64);

// Quiet NaN mask: bits 63..51 all set, plus the quiet bit (bit 50).
const NAN_MASK:       u64 = 0x7FF8_0000_0000_0000;
const TAG_MASK:       u64 = 0x0006_0000_0000_0000;
const TAG_UNDEFINED:  u64 = 0x0000_0000_0000_0000;
const TAG_NULL:       u64 = 0x0001_0000_0000_0000;
const TAG_BOOL:       u64 = 0x0002_0000_0000_0000;
const TAG_PTR:        u64 = 0x0004_0000_0000_0000;
const TAG_INT:        u64 = 0x0006_0000_0000_0000;

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
use crate::alloc::ts_alloc_rc;

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

/// A heap-allocated TypeScript function value (arrow function or function expression).
/// tag = 4
/// If `env` is not UNDEFINED, this is a closure and env is a TsArray of captured values.
/// The underlying MLIR function then takes `(env: TsVal, param0, param1, ...)`.
pub struct TsFunction {
    /// Pointer to the compiled native function.
    pub fn_ptr: *const u8,
    /// Number of declared parameters (NOT counting env).
    pub arity: u8,
    /// Captured environment (TsArray) or UNDEFINED if not a closure.
    pub env: TsVal,
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_new(fn_ptr: *const u8, arity: i32) -> TsVal {
    let size = std::mem::size_of::<TsFunction>();
    let ptr = crate::alloc::ts_alloc_rc(size, 4) as *mut TsFunction; // tag 4 = Function
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsFunction { fn_ptr, arity: arity as u8, env: UNDEFINED });
    TsVal::from_ptr(ptr as *mut u8)
}

/// Create a closure: a function + captured environment array.
#[no_mangle]
pub unsafe extern "C" fn ts_closure_new(fn_ptr: *const u8, arity: i32, env: TsVal) -> TsVal {
    let size = std::mem::size_of::<TsFunction>();
    let ptr = crate::alloc::ts_alloc_rc(size, 4) as *mut TsFunction;
    if ptr.is_null() { return NULL; }
    ts_retain_val(env);
    std::ptr::write(ptr, TsFunction { fn_ptr, arity: arity as u8, env });
    TsVal::from_ptr(ptr as *mut u8)
}

pub unsafe extern "C" fn ts_func_destructor(ptr: *mut u8) {
    let func_ptr = &mut *(ptr as *mut TsFunction);
    ts_release_val(func_ptr.env);
    std::ptr::drop_in_place(func_ptr as *mut TsFunction);
}

/// Internal: call a TsFunction value with up to 4 TsVal arguments.
/// If the function is a closure (env ≠ UNDEFINED), passes env as the first arg.
unsafe fn dispatch_callback(fn_val: TsVal, args: &[TsVal]) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let func = &*(fn_val.as_ptr() as *const TsFunction);
    let is_closure = !func.env.is_undefined();
    let env = func.env;
    let a0 = args.first().copied().unwrap_or(UNDEFINED);
    let a1 = args.get(1).copied().unwrap_or(UNDEFINED);
    let a2 = args.get(2).copied().unwrap_or(UNDEFINED);
    let a3 = args.get(3).copied().unwrap_or(UNDEFINED);
    if is_closure {
        // fn(env, arg0, arg1, ...) — env is always first
        match func.arity {
            0 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal) -> TsVal>(func.fn_ptr);
                f(env)
            }
            1 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0)
            }
            2 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0, a1)
            }
            3 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0, a1, a2)
            }
            _ => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0, a1, a2, a3)
            }
        }
    } else {
        match func.arity {
            0 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn() -> TsVal>(func.fn_ptr);
                f()
            }
            1 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal) -> TsVal>(func.fn_ptr);
                f(a0)
            }
            2 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, a1)
            }
            3 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, a1, a2)
            }
            _ => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, a1, a2, a3)
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call0(fn_val: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[])
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call1(fn_val: TsVal, a: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[a])
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call2(fn_val: TsVal, a: TsVal, b: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[a, b])
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call3(fn_val: TsVal, a: TsVal, b: TsVal, c: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[a, b, c])
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call4(fn_val: TsVal, a: TsVal, b: TsVal, c: TsVal, d: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[a, b, c, d])
}

// ── Array higher-order methods ────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn ts_arr_map(arr: TsVal, callback: TsVal) -> TsVal {
    if !arr.is_ptr() || heap_tag(arr) != 1 { return ts_arr_new(0); }
    let arr_ptr = arr.as_ptr() as *const TsArray;
    let len = (*arr_ptr).elements.len();
    let result = ts_arr_new(len as i32);
    for i in 0..len {
        let elem = { let r = &*arr_ptr; r.elements[i] };
        ts_retain_val(elem);
        let index = TsVal::from_i32(i as i32);
        ts_retain_val(arr);
        let mapped = dispatch_callback(callback, &[elem, index, arr]);
        ts_release_val(elem);
        ts_release_val(arr);
        ts_arr_set(result, i as i32, mapped);
        ts_release_val(mapped);
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_filter(arr: TsVal, callback: TsVal) -> TsVal {
    if !arr.is_ptr() || heap_tag(arr) != 1 { return ts_arr_new(0); }
    let arr_ptr = arr.as_ptr() as *const TsArray;
    let len = (*arr_ptr).elements.len();
    let result = ts_arr_new(0);
    for i in 0..len {
        let elem = { let r = &*arr_ptr; r.elements[i] };
        ts_retain_val(elem);
        let index = TsVal::from_i32(i as i32);
        ts_retain_val(arr);
        let keep = dispatch_callback(callback, &[elem, index, arr]);
        ts_release_val(arr);
        let truthy = ts_val_is_truthy(keep);
        ts_release_val(keep);
        if truthy {
            ts_arr_push(result, elem);
        }
        ts_release_val(elem);
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_for_each(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let result = dispatch_callback(callback, &[elem, index, arr]);
            ts_release_val(arr);
            ts_release_val(elem);
            ts_release_val(result);
        }
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_reduce(arr: TsVal, callback: TsVal, init: TsVal) -> TsVal {
    ts_retain_val(init);
    let mut acc = init;
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let new_acc = dispatch_callback(callback, &[acc, elem, index, arr]);
            ts_release_val(arr);
            ts_release_val(elem);
            ts_release_val(acc);
            acc = new_acc;
        }
    }
    acc
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_find(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let found = dispatch_callback(callback, &[elem, index, arr]);
            ts_release_val(arr);
            let truthy = ts_val_is_truthy(found);
            ts_release_val(found);
            if truthy {
                return elem; // already retained
            }
            ts_release_val(elem);
        }
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_find_index(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let found = dispatch_callback(callback, &[elem, index, arr]);
            ts_release_val(arr);
            ts_release_val(elem);
            let truthy = ts_val_is_truthy(found);
            ts_release_val(found);
            if truthy {
                return TsVal::from_i32(i as i32);
            }
        }
    }
    TsVal::from_i32(-1)
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_some(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let result = dispatch_callback(callback, &[elem, index, arr]);
            ts_release_val(arr);
            ts_release_val(elem);
            let truthy = ts_val_is_truthy(result);
            ts_release_val(result);
            if truthy { return TRUE; }
        }
    }
    FALSE
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_every(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let result = dispatch_callback(callback, &[elem, index, arr]);
            ts_release_val(arr);
            ts_release_val(elem);
            let truthy = ts_val_is_truthy(result);
            ts_release_val(result);
            if !truthy { return FALSE; }
        }
    }
    TRUE
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_sort(arr: TsVal, comparator: TsVal) -> TsVal {
    if !arr.is_ptr() || heap_tag(arr) != 1 { return arr; }
    let arr_ptr = arr.as_ptr() as *mut TsArray;
    let len = (*arr_ptr).elements.len();
    // Simple insertion sort to avoid Rust's sort closures requiring &mut issues
    for i in 1..len {
        let mut j = i;
        while j > 0 {
            let a = { let r = &*arr_ptr; r.elements[j - 1] };
            let b = { let r = &*arr_ptr; r.elements[j] };
            ts_retain_val(a);
            ts_retain_val(b);
            let cmp_result = if comparator.is_ptr() && heap_tag(comparator) == 4 {
                dispatch_callback(comparator, &[a, b])
            } else {
                // Default: lexicographic string comparison
                let sa = ts_val_to_string(a);
                let sb = ts_val_to_string(b);
                let sa_ptr = (*(sa.as_ptr() as *const TsString)).inner.clone();
                let sb_ptr = (*(sb.as_ptr() as *const TsString)).inner.clone();
                ts_release_val(sa);
                ts_release_val(sb);
                TsVal::from_i32(sa_ptr.cmp(&sb_ptr) as i32)
            };
            ts_release_val(a);
            ts_release_val(b);
            let should_swap = if cmp_result.is_int32() {
                cmp_result.as_i32() > 0
            } else {
                ts_val_to_f64_raw(cmp_result) > 0.0
            };
            ts_release_val(cmp_result);
            if should_swap {
                (*arr_ptr).elements.swap(j - 1, j);
                j -= 1;
            } else {
                break;
            }
        }
    }
    ts_retain_val(arr);
    arr
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_flat_map(arr: TsVal, callback: TsVal) -> TsVal {
    if !arr.is_ptr() || heap_tag(arr) != 1 { return ts_arr_new(0); }
    let result = ts_arr_new(0);
    let arr_ptr = arr.as_ptr() as *const TsArray;
    let len = (*arr_ptr).elements.len();
    for i in 0..len {
        let elem = { let r = &*arr_ptr; r.elements[i] };
        ts_retain_val(elem);
        let index = TsVal::from_i32(i as i32);
        ts_retain_val(arr);
        let mapped = dispatch_callback(callback, &[elem, index, arr]);
        ts_release_val(arr);
        ts_release_val(elem);
        if mapped.is_ptr() && heap_tag(mapped) == 1 {
            // flatten one level
            let inner_ptr = mapped.as_ptr() as *const TsArray;
            let inner_len = (&*inner_ptr).elements.len();
            for k in 0..inner_len {
                let v = { let r = &*inner_ptr; r.elements[k] };
                ts_arr_push(result, v);
            }
        } else {
            ts_arr_push(result, mapped);
        }
        ts_release_val(mapped);
    }
    result
}

/// Internal: check if a TsVal is truthy (non-zero, non-false, non-null, non-undefined).
unsafe fn ts_val_is_truthy(val: TsVal) -> bool {
    if val.is_int32() { return val.as_i32() != 0; }
    if val.is_number() { let f = val.as_f64(); return f != 0.0 && !f.is_nan(); }
    if val.is_bool()   { return val.as_bool(); }
    if val.is_null() || val.is_undefined() { return false; }
    if val.is_ptr()    {
        if heap_tag(val) == 2 {
            // Empty string is falsy
            let s_ptr = val.as_ptr() as *const TsString;
            return !(&*s_ptr).inner.is_empty();
        }
        return true; // objects/arrays/functions are truthy
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_new() -> TsVal {
    let size = std::mem::size_of::<TsObject>();
    let ptr = ts_alloc_rc(size, 0) as *mut TsObject; // tag 0 = Object
    if ptr.is_null() {
        return NULL;
    }
    unsafe {
        std::ptr::write(ptr, TsObject {
            properties: HashMap::new(),
        });
    }
    TsVal::from_ptr(ptr as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_get(obj_val: TsVal, key_ptr: *const i8) -> TsVal {
    let ptr = obj_val.as_ptr();
    if !ptr.is_null() && !key_ptr.is_null() {
        let obj = ptr as *mut TsObject;
        let key = unsafe { std::ffi::CStr::from_ptr(key_ptr) }.to_string_lossy().into_owned();
        if let Some(&val) = (&*obj).properties.get(&key) {
            ts_retain_val(val);
            return val;
        }
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_obj_set(obj_val: TsVal, key_ptr: *const i8, val: TsVal) {
    let ptr = obj_val.as_ptr();
    if !ptr.is_null() && !key_ptr.is_null() {
        let obj = ptr as *mut TsObject;
        let key = unsafe { std::ffi::CStr::from_ptr(key_ptr) }.to_string_lossy().into_owned();
        
        // ARC: retain new value
        ts_retain_val(val);
        
        let old_val = (&mut *obj).properties.insert(key, val);
        if let Some(v) = old_val {
            ts_release_val(v);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_new(capacity: i32) -> TsVal {
    let size = std::mem::size_of::<TsArray>();
    let ptr = ts_alloc_rc(size, 1) as *mut TsArray; // tag 1 = Array
    if ptr.is_null() {
        return NULL;
    }
    unsafe {
        std::ptr::write(ptr, TsArray {
            elements: Vec::with_capacity(capacity as usize),
        });
        // Initialize with undefined.
        for _ in 0..capacity {
            (&mut *ptr).elements.push(UNDEFINED);
        }
    }
    TsVal::from_ptr(ptr as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn ts_string_new(c_str: *const i8) -> TsVal {
    let s = unsafe { std::ffi::CStr::from_ptr(c_str) }.to_string_lossy().into_owned();
    let size = std::mem::size_of::<TsString>();
    let ptr = ts_alloc_rc(size, 2) as *mut TsString; // tag 2 = String
    if ptr.is_null() {
        return NULL;
    }
    unsafe {
        std::ptr::write(ptr, TsString { inner: s });
    }
    TsVal::from_ptr(ptr as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn ts_string_concat(v1: TsVal, v2: TsVal) -> TsVal {
    let p1 = v1.as_ptr() as *mut TsString;
    let p2 = v2.as_ptr() as *mut TsString;
    
    let s1 = unsafe { &(*p1).inner };
    let s2 = unsafe { &(*p2).inner };
    
    let new_s = format!("{}{}", s1, s2);
    let size = std::mem::size_of::<TsString>();
    let ptr = ts_alloc_rc(size, 2) as *mut TsString;
    if ptr.is_null() {
        return NULL;
    }
    unsafe {
        std::ptr::write(ptr, TsString { inner: new_s });
    }
    TsVal::from_ptr(ptr as *mut u8)
}

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

// ── Global Tokio runtime ──────────────────────────────────────────────────────

static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

fn get_runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create Tokio runtime")
    })
}

// ── Promise ───────────────────────────────────────────────────────────────────

const TAG_PROMISE_ALLOC: u8 = 3;

/// Heap-allocated TypeScript Promise.
/// The inner value lives in an `OnceLock`; a `Notify` wakes any blockers.
pub struct TsPromise {
    resolved: std::sync::Arc<std::sync::OnceLock<TsVal>>,
    notify:   std::sync::Arc<tokio::sync::Notify>,
}

fn make_promise_pair() -> (
    std::sync::Arc<std::sync::OnceLock<TsVal>>,
    std::sync::Arc<tokio::sync::Notify>,
) {
    (
        std::sync::Arc::new(std::sync::OnceLock::new()),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    )
}

unsafe fn alloc_promise(p: TsPromise) -> TsVal {
    let size = std::mem::size_of::<TsPromise>();
    let ptr = crate::alloc::ts_alloc_rc(size, TAG_PROMISE_ALLOC) as *mut TsPromise;
    if ptr.is_null() { return NULL; }
    ptr.write(p);
    TsVal::from_ptr(ptr as *mut u8)
}

fn resolve_arc(
    resolved: &std::sync::Arc<std::sync::OnceLock<TsVal>>,
    notify:   &std::sync::Arc<tokio::sync::Notify>,
    val: TsVal,
) {
    let _ = resolved.set(val);
    notify.notify_waiters();
}

/// Wait until the promise's OnceLock is set, then return the value.
/// Works both from the main thread and from inside `spawn_blocking` tasks.
fn block_until_resolved(
    resolved: std::sync::Arc<std::sync::OnceLock<TsVal>>,
    notify:   std::sync::Arc<tokio::sync::Notify>,
) -> TsVal {
    if let Some(&v) = resolved.get() { return v; }
    let fut = async move {
        loop {
            if let Some(&v) = resolved.get() { return v; }
            notify.notified().await;
        }
    };
    // Inside spawn_blocking we have a Handle but are NOT in an async context,
    // so Handle::block_on is the right tool. From the main thread, use the
    // global runtime directly.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(fut),
        Err(_)     => get_runtime().block_on(fut),
    }
}

/// Wrap `val` in a resolved Promise (heap object, alloc tag 3).
#[no_mangle]
pub unsafe extern "C" fn ts_promise_resolve(val: TsVal) -> TsVal {
    let (resolved, notify) = make_promise_pair();
    ts_retain_val(val); // promise owns a reference
    let _ = resolved.set(val);
    alloc_promise(TsPromise { resolved, notify })
}

/// Await a Promise, blocking until resolved. Non-Promise values pass through.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_await(val: TsVal) -> TsVal {
    if !val.is_ptr() {
        ts_retain_val(val);
        return val;
    }
    let ptr = val.as_ptr();
    let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
    let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
    if (*header).tag != TAG_PROMISE_ALLOC {
        ts_retain_val(val);
        return val;
    }
    let promise = &*(ptr as *const TsPromise);
    let result = block_until_resolved(promise.resolved.clone(), promise.notify.clone());
    ts_retain_val(result);
    ts_release_val(val); // may run ts_promise_destructor
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_promise_destructor(ptr: *mut u8) {
    let p = ptr as *mut TsPromise;
    if let Some(&val) = (*p).resolved.get() {
        ts_release_val(val);
    }
    std::ptr::drop_in_place(p);
}

// ── sleep ─────────────────────────────────────────────────────────────────────

/// Returns a Promise<undefined> that resolves after `ms` milliseconds.
#[no_mangle]
pub unsafe extern "C" fn ts_sleep(ms: i32) -> TsVal {
    let (resolved, notify) = make_promise_pair();
    let r2 = resolved.clone();
    let n2 = notify.clone();
    get_runtime().spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ms.max(0) as u64)).await;
        resolve_arc(&r2, &n2, UNDEFINED);
    });
    alloc_promise(TsPromise { resolved, notify })
}

// ── Promise.race ──────────────────────────────────────────────────────────────

async fn wait_for_promise(
    resolved: std::sync::Arc<std::sync::OnceLock<TsVal>>,
    notify:   std::sync::Arc<tokio::sync::Notify>,
) -> TsVal {
    loop {
        if let Some(&v) = resolved.get() { return v; }
        notify.notified().await;
    }
}

/// Returns a Promise that resolves to whichever of `p1` / `p2` resolves first.
#[no_mangle]
pub unsafe extern "C" fn ts_promise_race(p1: TsVal, p2: TsVal) -> TsVal {
    unsafe fn promise_arcs(v: TsVal) -> Option<(
        std::sync::Arc<std::sync::OnceLock<TsVal>>,
        std::sync::Arc<tokio::sync::Notify>,
    )> {
        if !v.is_ptr() { return None; }
        let ptr = v.as_ptr();
        let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
        let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
        if (*header).tag != TAG_PROMISE_ALLOC { return None; }
        let p = &*(ptr as *const TsPromise);
        Some((p.resolved.clone(), p.notify.clone()))
    }

    let (res1, not1) = match promise_arcs(p1) {
        Some(pair) => pair,
        None => { ts_release_val(p2); ts_retain_val(p1); return ts_promise_resolve(p1); }
    };
    let (res2, not2) = match promise_arcs(p2) {
        Some(pair) => pair,
        None => { ts_release_val(p1); ts_retain_val(p2); return ts_promise_resolve(p2); }
    };

    // Fast path: already resolved.
    if let Some(&v) = res1.get() { ts_release_val(p2); return ts_promise_resolve(v); }
    if let Some(&v) = res2.get() { ts_release_val(p1); return ts_promise_resolve(v); }

    let (rr, rn) = make_promise_pair();
    let rr2 = rr.clone();
    let rn2 = rn.clone();
    let p1_raw = p1.0;
    let p2_raw = p2.0;
    // Keep both promises alive while the race task runs.
    ts_retain_val(p1);
    ts_retain_val(p2);

    get_runtime().spawn(async move {
        let result = tokio::select! {
            v = wait_for_promise(res1, not1) => v,
            v = wait_for_promise(res2, not2) => v,
        };
        // Retain for the result promise's ownership.
        unsafe { ts_retain_val(result); }
        resolve_arc(&rr2, &rn2, result);
        unsafe {
            ts_release_val(TsVal(p1_raw));
            ts_release_val(TsVal(p2_raw));
        }
    });

    alloc_promise(TsPromise { resolved: rr, notify: rn })
}

// ── Async spawn (spawn_blocking + function pointer) ───────────────────────────

type AsyncFn0 = unsafe extern "C" fn() -> u64;
type AsyncFn1 = unsafe extern "C" fn(i32) -> u64;
type AsyncFn2 = unsafe extern "C" fn(i32, i32) -> u64;
type AsyncFn3 = unsafe extern "C" fn(i32, i32, i32) -> u64;
type AsyncFn4 = unsafe extern "C" fn(i32, i32, i32, i32) -> u64;

fn do_spawn<F: FnOnce() -> u64 + Send + 'static>(f: F) -> TsVal {
    let (resolved, notify) = make_promise_pair();
    let r2 = resolved.clone();
    let n2 = notify.clone();
    get_runtime().spawn_blocking(move || {
        let raw = f();
        resolve_arc(&r2, &n2, TsVal(raw));
    });
    unsafe { alloc_promise(TsPromise { resolved, notify }) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn0(fn_ptr: *const u8) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn0 = std::mem::transmute(addr); f() })
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn1(fn_ptr: *const u8, a0: i32) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn1 = std::mem::transmute(addr); f(a0) })
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn2(fn_ptr: *const u8, a0: i32, a1: i32) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn2 = std::mem::transmute(addr); f(a0, a1) })
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn3(fn_ptr: *const u8, a0: i32, a1: i32, a2: i32) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn3 = std::mem::transmute(addr); f(a0, a1, a2) })
}

#[no_mangle]
pub unsafe extern "C" fn ts_async_spawn4(fn_ptr: *const u8, a0: i32, a1: i32, a2: i32, a3: i32) -> TsVal {
    let addr = fn_ptr as usize;
    do_spawn(move || unsafe { let f: AsyncFn4 = std::mem::transmute(addr); f(a0, a1, a2, a3) })
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_get(arr_val: TsVal, idx: i32) -> TsVal {
    let ptr = arr_val.as_ptr();
    if !ptr.is_null() && idx >= 0 {
        let arr = ptr as *mut TsArray;
        let idx = idx as usize;
        if idx < (&*arr).elements.len() {
            let val = (&*arr).elements[idx];
            ts_retain_val(val);
            return val;
        }
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_set(arr_val: TsVal, idx: i32, val: TsVal) {
    let ptr = arr_val.as_ptr();
    if !ptr.is_null() && idx >= 0 {
        let arr = ptr as *mut TsArray;
        let idx = idx as usize;

        // ARC: Retain new value.
        ts_retain_val(val);

        if idx < (&*arr).elements.len() {
            let old_val = std::mem::replace(&mut (&mut *arr).elements[idx], val);
            ts_release_val(old_val);
        } else {
            // A real JS array would resize/pad.
            if idx == (&*arr).elements.len() {
                (&mut *arr).elements.push(val);
            } else {
                // Pad with undefined.
                while (&*arr).elements.len() < idx {
                    (&mut *arr).elements.push(UNDEFINED);
                }
                (&mut *arr).elements.push(val);
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_len(arr_val: TsVal) -> TsVal {
    let ptr = arr_val.as_ptr();
    if !ptr.is_null() {
        let arr = ptr as *mut TsArray;
        return TsVal::from_i32((&*arr).elements.len() as i32);
    }
    TsVal::from_i32(0)
}

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
        let header = unsafe { ptr.sub(header_size) as *mut crate::alloc::ArcHeader };
        let tag = unsafe { (*header).tag };
        
        let destructor = match tag {
            0 => Some(ts_obj_destructor as unsafe extern "C" fn(*mut u8)),
            1 => Some(ts_arr_destructor as unsafe extern "C" fn(*mut u8)),
            2 => Some(ts_string_destructor as unsafe extern "C" fn(*mut u8)),
            3 => Some(ts_promise_destructor as unsafe extern "C" fn(*mut u8)),
            4 => Some(ts_func_destructor as unsafe extern "C" fn(*mut u8)),
            5 => Some(ts_map_destructor as unsafe extern "C" fn(*mut u8)),
            _ => None,
        };
        
        crate::alloc::ts_release(ptr, destructor);
    }
}

/// Destructor for TsObject.
#[no_mangle]
pub unsafe extern "C" fn ts_obj_destructor(ptr: *mut u8) {
    let obj_ptr = ptr as *mut TsObject;
    unsafe {
        for (_, val) in (*obj_ptr).properties.drain() {
            ts_release_val(val);
        }
        std::ptr::drop_in_place(obj_ptr);
    }
}

/// Destructor for TsArray.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_destructor(ptr: *mut u8) {
    let arr_ptr = ptr as *mut TsArray;
    unsafe {
        for val in (*arr_ptr).elements.drain(..) {
            ts_release_val(val);
        }
        std::ptr::drop_in_place(arr_ptr);
    }
}

/// Destructor for TsString.
#[no_mangle]
pub unsafe extern "C" fn ts_string_destructor(ptr: *mut u8) {
    let s_ptr = ptr as *mut TsString;
    unsafe {
        std::ptr::drop_in_place(s_ptr);
    }
}

// ── Type introspection ────────────────────────────────────────────────────────

/// Read the heap-allocation tag (0=Object,1=Array,2=String,3=Promise) for a
/// pointer TsVal **without** modifying the reference count.
/// Returns 255 if `val` is not a heap pointer.
unsafe fn heap_tag(val: TsVal) -> u8 {
    if !val.is_ptr() { return 255; }
    let ptr = val.as_ptr();
    let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
    let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
    (*header).tag
}

/// Returns a new string TsVal describing the JavaScript `typeof` the value.
/// Caller receives an owned reference (refcount already bumped inside
/// `ts_string_new`).
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
                2 => b"string\0",
                4 => b"function\0",
                _ => b"object\0",
            },
            _ => b"undefined\0",
        }
    };
    ts_string_new(type_bytes.as_ptr() as *const i8)
}

/// Strict equality (`===`).  Returns 1 if equal, 0 otherwise.
/// For strings the content is compared; for all other types the bit patterns
/// must be identical (covers numbers, booleans, undefined, null, and object
/// identity).
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
/// Used to implement the nullish coalescing operator (`??`) and optional chaining (`?.`).
#[no_mangle]
pub unsafe extern "C" fn ts_is_nullish(val: TsVal) -> i32 {
    if val.is_null() || val.is_undefined() { 1 } else { 0 }
}

/// Returns 1 if `val` is truthy (JS semantics), 0 otherwise.
/// Used to implement `||=` and `&&=` logical assignment operators.
#[no_mangle]
pub unsafe extern "C" fn ts_is_truthy(val: TsVal) -> i32 {
    if ts_val_is_truthy(val) { 1 } else { 0 }
}

/// Returns 1 if `val` is `undefined`, 0 otherwise.
/// Used to implement default parameter values.
#[no_mangle]
pub unsafe extern "C" fn ts_is_undefined(val: TsVal) -> i32 {
    if val.is_undefined() { 1 } else { 0 }
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

    let key_string = if key.is_int32() {
        key.as_i32().to_string()
    } else if key.is_ptr() && heap_tag(key) == 2 {
        let ts_str = &*(key.as_ptr() as *const TsString);
        ts_str.inner.clone()
    } else if !key.is_nan_boxed() {
        key.as_f64().to_string()
    } else {
        return; // skip null, undefined, bool, object keys
    };
    let mut bytes = key_string.into_bytes();
    bytes.push(0u8);
    ts_obj_set(obj, bytes.as_ptr() as *const i8, val);
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

// ── String / value coercion ───────────────────────────────────────────────────

/// Convert any TsVal to a string TsVal. Returns an owned reference.
/// Used for template literal interpolation: `` `${expr}` ``.
#[no_mangle]
pub unsafe extern "C" fn ts_val_to_string(val: TsVal) -> TsVal {
    if val.is_int32() {
        let s = val.as_i32().to_string();
        let mut bytes = s.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    if val.is_bool() {
        let s: &[u8] = if val.as_bool() { b"true\0" } else { b"false\0" };
        return ts_string_new(s.as_ptr() as *const i8);
    }
    if val.is_null() {
        return ts_string_new(b"null\0".as_ptr() as *const i8);
    }
    if val.is_undefined() {
        return ts_string_new(b"undefined\0".as_ptr() as *const i8);
    }
    if !val.is_nan_boxed() {
        let s = val.as_f64().to_string();
        let mut bytes = s.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    if val.is_ptr() {
        match heap_tag(val) {
            2 => { ts_retain_val(val); return val; }
            0 => return ts_string_new(b"[object Object]\0".as_ptr() as *const i8),
            1 => return ts_string_new(b"[object Array]\0".as_ptr() as *const i8),
            _ => {}
        }
    }
    ts_string_new(b"undefined\0".as_ptr() as *const i8)
}

/// Returns `.length`: array element count or string char count.
#[no_mangle]
pub unsafe extern "C" fn ts_val_length(val: TsVal) -> TsVal {
    if val.is_ptr() {
        match heap_tag(val) {
            1 => {
                let arr = &*(val.as_ptr() as *const TsArray);
                return TsVal::from_i32(arr.elements.len() as i32);
            }
            2 => {
                let s = &*(val.as_ptr() as *const TsString);
                return TsVal::from_i32(s.inner.chars().count() as i32);
            }
            _ => {}
        }
    }
    TsVal::from_i32(0)
}

// ── Array mutation methods ─────────────────────────────────────────────────────

/// Append `val` to the end of `arr`. Returns the new length.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_push(arr_val: TsVal, val: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
        ts_retain_val(val);
        arr.elements.push(val);
        return TsVal::from_i32(arr.elements.len() as i32);
    }
    TsVal::from_i32(0)
}

/// Remove and return the last element (or `undefined`).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_pop(arr_val: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
        if let Some(val) = arr.elements.pop() {
            return val; // Transfer ownership — ref count already belongs to caller.
        }
    }
    UNDEFINED
}

/// Append all elements of `src` to `dst` (implements `[...src, ...]`).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_push_all(dst: TsVal, src: TsVal) {
    if !dst.is_ptr() || heap_tag(dst) != 1 { return; }
    if !src.is_ptr() || heap_tag(src) != 1 { return; }
    let src_arr = &*(src.as_ptr() as *const TsArray);
    let dst_arr = &mut *(dst.as_ptr() as *mut TsArray);
    for &val in &src_arr.elements {
        ts_retain_val(val);
        dst_arr.elements.push(val);
    }
}

/// Join array elements with `sep` between each (returns a string TsVal).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_join(arr_val: TsVal, sep_val: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &*(arr_val.as_ptr() as *const TsArray);
        let sep = if sep_val.is_ptr() && heap_tag(sep_val) == 2 {
            let s = &*(sep_val.as_ptr() as *const TsString);
            s.inner.clone()
        } else {
            ",".to_string()
        };
        let parts: Vec<String> = arr.elements.iter().map(|&v| {
            if v.is_int32() { v.as_i32().to_string() }
            else if v.is_bool() { v.as_bool().to_string() }
            else if v.is_null() || v.is_undefined() { String::new() }
            else if v.is_ptr() && heap_tag(v) == 2 {
                let s = &*(v.as_ptr() as *const TsString);
                s.inner.clone()
            } else { String::new() }
        }).collect();
        let result = parts.join(&sep);
        let mut bytes = result.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

// ── Array / string search ──────────────────────────────────────────────────────

/// Returns the index of `search` in `arr` (or -1).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_index_of(arr_val: TsVal, search: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &*(arr_val.as_ptr() as *const TsArray);
        for (i, &val) in arr.elements.iter().enumerate() {
            if ts_val_strict_eq(val, search) != 0 {
                return TsVal::from_i32(i as i32);
            }
        }
    }
    TsVal::from_i32(-1)
}

/// Returns the first char-index of `search` in string `s` (or -1).
#[no_mangle]
pub unsafe extern "C" fn ts_str_index_of(s_val: TsVal, search_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 && search_val.is_ptr() && heap_tag(search_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let search = &*(search_val.as_ptr() as *const TsString);
        if let Some(pos) = s.inner.find(search.inner.as_str()) {
            return TsVal::from_i32(s.inner[..pos].chars().count() as i32);
        }
    }
    TsVal::from_i32(-1)
}

/// Polymorphic `indexOf`: dispatches to array or string variant at runtime.
#[no_mangle]
pub unsafe extern "C" fn ts_val_index_of(obj: TsVal, search: TsVal) -> TsVal {
    if obj.is_ptr() {
        match heap_tag(obj) {
            1 => ts_arr_index_of(obj, search),
            2 => ts_str_index_of(obj, search),
            _ => TsVal::from_i32(-1),
        }
    } else {
        TsVal::from_i32(-1)
    }
}

/// Returns `true` if `s` contains `search` (string `includes`).
#[no_mangle]
pub unsafe extern "C" fn ts_str_includes(s_val: TsVal, search_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 && search_val.is_ptr() && heap_tag(search_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let search = &*(search_val.as_ptr() as *const TsString);
        return TsVal::from_bool(s.inner.contains(search.inner.as_str()));
    }
    TsVal::from_bool(false)
}

/// Polymorphic `includes`: dispatches to array or string variant at runtime.
#[no_mangle]
pub unsafe extern "C" fn ts_val_includes(obj: TsVal, search: TsVal) -> TsVal {
    if obj.is_ptr() {
        match heap_tag(obj) {
            1 => TsVal::from_bool(ts_arr_index_of(obj, search).as_i32() >= 0),
            2 => ts_str_includes(obj, search),
            _ => TsVal::from_bool(false),
        }
    } else {
        TsVal::from_bool(false)
    }
}

// ── String transformation methods ─────────────────────────────────────────────

/// Returns a substring from `start` to `end` (char indices; negative = from end).
/// Pass `undefined` as `end` to slice to end of string.
#[no_mangle]
pub unsafe extern "C" fn ts_str_slice(s_val: TsVal, start_val: TsVal, end_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let chars: Vec<char> = s.inner.chars().collect();
        let len = chars.len() as i32;
        let norm = |idx: i32| -> usize {
            if idx < 0 { (len + idx).max(0) as usize } else { idx.min(len) as usize }
        };
        let start = if start_val.is_int32() { norm(start_val.as_i32()) } else { 0 };
        let end   = if end_val.is_int32()   { norm(end_val.as_i32())   } else { chars.len() };
        if start >= end {
            return ts_string_new(b"\0".as_ptr() as *const i8);
        }
        let sub: String = chars[start..end.min(chars.len())].iter().collect();
        let mut bytes = sub.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// Returns `s` converted to uppercase.
#[no_mangle]
pub unsafe extern "C" fn ts_str_to_upper(s_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let upper = s.inner.to_uppercase();
        let mut bytes = upper.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// Returns `s` converted to lowercase.
#[no_mangle]
pub unsafe extern "C" fn ts_str_to_lower(s_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let lower = s.inner.to_lowercase();
        let mut bytes = lower.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// Returns `s` with leading and trailing whitespace removed.
#[no_mangle]
pub unsafe extern "C" fn ts_str_trim(s_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let trimmed = s.inner.trim().to_string();
        let mut bytes = trimmed.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// Split `s` by `sep` and return a `TsArray` of string parts.
#[no_mangle]
pub unsafe extern "C" fn ts_str_split(s_val: TsVal, sep_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let sep = if sep_val.is_ptr() && heap_tag(sep_val) == 2 {
            let sep_s = &*(sep_val.as_ptr() as *const TsString);
            sep_s.inner.clone()
        } else {
            return ts_arr_new(0);
        };
        let parts: Vec<String> = s.inner.split(sep.as_str()).map(|p| p.to_string()).collect();
        let arr = ts_arr_new(0);
        for part in parts {
            let mut bytes = part.into_bytes();
            bytes.push(0u8);
            let part_val = ts_string_new(bytes.as_ptr() as *const i8);
            ts_arr_push(arr, part_val);
            ts_release_val(part_val);
        }
        return arr;
    }
    ts_arr_new(0)
}

/// `s.replace(search, replacement)` — replace first occurrence of `search` with `replacement`.
#[no_mangle]
pub unsafe extern "C" fn ts_str_replace(s_val: TsVal, search_val: TsVal, repl_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let search = if search_val.is_ptr() && heap_tag(search_val) == 2 {
            (&*(search_val.as_ptr() as *const TsString)).inner.clone()
        } else { return ts_val_to_string(s_val); };
        let repl = if repl_val.is_ptr() && heap_tag(repl_val) == 2 {
            (&*(repl_val.as_ptr() as *const TsString)).inner.clone()
        } else { String::new() };
        let result = s.inner.replacen(search.as_str(), repl.as_str(), 1);
        let mut bytes = result.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// `s.replaceAll(search, replacement)` — replace all occurrences of `search` with `replacement`.
#[no_mangle]
pub unsafe extern "C" fn ts_str_replace_all(s_val: TsVal, search_val: TsVal, repl_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let search = if search_val.is_ptr() && heap_tag(search_val) == 2 {
            (&*(search_val.as_ptr() as *const TsString)).inner.clone()
        } else { return ts_val_to_string(s_val); };
        let repl = if repl_val.is_ptr() && heap_tag(repl_val) == 2 {
            (&*(repl_val.as_ptr() as *const TsString)).inner.clone()
        } else { String::new() };
        let result = s.inner.replace(search.as_str(), repl.as_str());
        let mut bytes = result.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// `s.startsWith(prefix)` — true if string starts with prefix.
#[no_mangle]
pub unsafe extern "C" fn ts_str_starts_with(s_val: TsVal, prefix_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 && prefix_val.is_ptr() && heap_tag(prefix_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let p = &*(prefix_val.as_ptr() as *const TsString);
        return TsVal::from_bool(s.inner.starts_with(p.inner.as_str()));
    }
    TsVal::from_bool(false)
}

/// `s.endsWith(suffix)` — true if string ends with suffix.
#[no_mangle]
pub unsafe extern "C" fn ts_str_ends_with(s_val: TsVal, suffix_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 && suffix_val.is_ptr() && heap_tag(suffix_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let p = &*(suffix_val.as_ptr() as *const TsString);
        return TsVal::from_bool(s.inner.ends_with(p.inner.as_str()));
    }
    TsVal::from_bool(false)
}

/// `s.padStart(len, fillChar)` — pad string at the start to reach total length `len`.
#[no_mangle]
pub unsafe extern "C" fn ts_str_pad_start(s_val: TsVal, len_val: TsVal, fill_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let target_len = if len_val.is_int32() { len_val.as_i32() as usize } else { 0 };
        let fill = if fill_val.is_ptr() && heap_tag(fill_val) == 2 {
            (&*(fill_val.as_ptr() as *const TsString)).inner.clone()
        } else { " ".to_string() };
        let fill_char = if fill.is_empty() { ' ' } else { fill.chars().next().unwrap() };
        let current = s.inner.len();
        let result = if current >= target_len {
            s.inner.clone()
        } else {
            let pad: String = std::iter::repeat(fill_char).take(target_len - current).collect();
            format!("{}{}", pad, s.inner)
        };
        let mut bytes = result.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// `s.padEnd(len, fillChar)` — pad string at the end to reach total length `len`.
#[no_mangle]
pub unsafe extern "C" fn ts_str_pad_end(s_val: TsVal, len_val: TsVal, fill_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let target_len = if len_val.is_int32() { len_val.as_i32() as usize } else { 0 };
        let fill = if fill_val.is_ptr() && heap_tag(fill_val) == 2 {
            (&*(fill_val.as_ptr() as *const TsString)).inner.clone()
        } else { " ".to_string() };
        let fill_char = if fill.is_empty() { ' ' } else { fill.chars().next().unwrap() };
        let current = s.inner.len();
        let result = if current >= target_len {
            s.inner.clone()
        } else {
            let pad: String = std::iter::repeat(fill_char).take(target_len - current).collect();
            format!("{}{}", s.inner, pad)
        };
        let mut bytes = result.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// `s.charAt(index)` — return the character at index as a string.
#[no_mangle]
pub unsafe extern "C" fn ts_str_char_at(s_val: TsVal, idx_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let idx = if idx_val.is_int32() { idx_val.as_i32() } else { 0 };
        if idx >= 0 {
            if let Some(c) = s.inner.chars().nth(idx as usize) {
                let mut bytes = c.to_string().into_bytes();
                bytes.push(0u8);
                return ts_string_new(bytes.as_ptr() as *const i8);
            }
        }
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// `s.charCodeAt(index)` — return char code at index as integer.
#[no_mangle]
pub unsafe extern "C" fn ts_str_char_code_at(s_val: TsVal, idx_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let idx = if idx_val.is_int32() { idx_val.as_i32() } else { 0 };
        if idx >= 0 {
            if let Some(c) = s.inner.chars().nth(idx as usize) {
                return TsVal::from_i32(c as i32);
            }
        }
    }
    // NaN for out-of-bounds
    TsVal::from_f64(f64::NAN)
}

/// `s.repeat(count)` — repeat string `count` times.
#[no_mangle]
pub unsafe extern "C" fn ts_str_repeat(s_val: TsVal, count_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let count = if count_val.is_int32() { count_val.as_i32().max(0) as usize } else { 0 };
        let result = s.inner.repeat(count);
        let mut bytes = result.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// `String.fromCharCode(code)` — create a string from a char code.
#[no_mangle]
pub unsafe extern "C" fn ts_str_from_char_code(code_val: TsVal) -> TsVal {
    let code = if code_val.is_int32() { code_val.as_i32() } else { 0 };
    if let Some(c) = char::from_u32(code as u32) {
        let mut bytes = c.to_string().into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

// ── Math built-ins ────────────────────────────────────────────────────────────

/// Convert a TsVal to f64 for numeric operations (JS ToNumber semantics).
fn ts_val_to_f64_raw(val: TsVal) -> f64 {
    if val.is_int32()  { return val.as_i32() as f64; }
    if val.is_number() { return val.as_f64(); }
    if val.is_bool()   { return if val.as_bool() { 1.0 } else { 0.0 }; }
    if val.is_null()   { return 0.0; }
    // undefined → NaN
    f64::NAN
}

/// Convert f64 back to TsVal: prefer i32 if the value is an exact integer.
fn f64_to_ts_num(n: f64) -> TsVal {
    if n.is_finite() && n == n.trunc() && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
        TsVal::from_i32(n as i32)
    } else {
        TsVal::from_f64(n)
    }
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

/// Polymorphic comparisons returning i32 (0 or 1).
#[no_mangle] pub unsafe extern "C" fn ts_lt(a: TsVal, b: TsVal) -> i32  { if (ts_val_to_f64_raw(a) < ts_val_to_f64_raw(b)) { 1 } else { 0 } }
#[no_mangle] pub unsafe extern "C" fn ts_le(a: TsVal, b: TsVal) -> i32  { if (ts_val_to_f64_raw(a) <= ts_val_to_f64_raw(b)) { 1 } else { 0 } }
#[no_mangle] pub unsafe extern "C" fn ts_gt(a: TsVal, b: TsVal) -> i32  { if (ts_val_to_f64_raw(a) > ts_val_to_f64_raw(b)) { 1 } else { 0 } }
#[no_mangle] pub unsafe extern "C" fn ts_ge(a: TsVal, b: TsVal) -> i32  { if (ts_val_to_f64_raw(a) >= ts_val_to_f64_raw(b)) { 1 } else { 0 } }

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

// ── Object static methods ─────────────────────────────────────────────────────

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

/// `arr.flat([depth])` — flatten nested arrays up to `depth` levels (default 1).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_flat(arr: TsVal, depth: i32) -> TsVal {
    let result = ts_arr_new(0);
    if !arr.is_ptr() || heap_tag(arr) != 1 {
        return result;
    }
    unsafe fn flatten(src: TsVal, result: TsVal, depth: i32) {
        if !src.is_ptr() || heap_tag(src) != 1 { return; }
        let src_ptr = src.as_ptr() as *const TsArray;
        let src_ref = &*src_ptr;
        let len = src_ref.elements.len();
        for i in 0..len {
            let elem = src_ref.elements[i];
            if depth > 0 && elem.is_ptr() && heap_tag(elem) == 1 {
                flatten(elem, result, depth - 1);
            } else {
                ts_arr_push(result, elem);
            }
        }
    }
    flatten(arr, result, depth);
    result
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

// ── Map built-in ──────────────────────────────────────────────────────────────

/// Compare two TsVals for Map key equality (strict same-value semantics).
unsafe fn map_key_eq(a: TsVal, b: TsVal) -> bool {
    if a.0 == b.0 { return true; }
    // String content equality
    if a.is_ptr() && heap_tag(a) == 2 && b.is_ptr() && heap_tag(b) == 2 {
        let sa = &*(a.as_ptr() as *const TsString);
        let sb = &*(b.as_ptr() as *const TsString);
        return sa.inner == sb.inner;
    }
    false
}

/// `new Map()` — create an empty Map.
#[no_mangle]
pub unsafe extern "C" fn ts_map_new() -> TsVal {
    let size = std::mem::size_of::<TsMap>();
    let ptr = ts_alloc_rc(size, 5) as *mut TsMap; // tag 5 = Map
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsMap { entries: Vec::new() });
    TsVal::from_ptr(ptr as *mut u8)
}

/// `map.set(key, val)` — insert or update a key; returns the map (owned ref).
#[no_mangle]
pub unsafe extern "C" fn ts_map_set(map_val: TsVal, key: TsVal, val: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 5 {
        ts_retain_val(map_val);
        return map_val;
    }
    let map = &mut *(map_val.as_ptr() as *mut TsMap);
    for entry in &mut map.entries {
        if map_key_eq(entry.0, key) {
            ts_retain_val(val);
            let old = entry.1;
            entry.1 = val;
            ts_release_val(old);
            ts_retain_val(map_val);
            return map_val;
        }
    }
    ts_retain_val(key);
    ts_retain_val(val);
    map.entries.push((key, val));
    ts_retain_val(map_val);
    map_val
}

/// `map.get(key)` — retrieve value or undefined.
#[no_mangle]
pub unsafe extern "C" fn ts_map_get(map_val: TsVal, key: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 5 { return UNDEFINED; }
    let map = &*(map_val.as_ptr() as *const TsMap);
    for (k, v) in &map.entries {
        if map_key_eq(*k, key) {
            ts_retain_val(*v);
            return *v;
        }
    }
    UNDEFINED
}

/// `map.has(key)` — true if key is present.
#[no_mangle]
pub unsafe extern "C" fn ts_map_has(map_val: TsVal, key: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 5 { return TsVal::from_bool(false); }
    let map = &*(map_val.as_ptr() as *const TsMap);
    for (k, _) in &map.entries {
        if map_key_eq(*k, key) { return TsVal::from_bool(true); }
    }
    TsVal::from_bool(false)
}

/// `map.delete(key)` — remove a key; returns true if it was present.
#[no_mangle]
pub unsafe extern "C" fn ts_map_delete(map_val: TsVal, key: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 5 { return TsVal::from_bool(false); }
    let map = &mut *(map_val.as_ptr() as *mut TsMap);
    let pos = map.entries.iter().position(|(k, _)| map_key_eq(*k, key));
    if let Some(i) = pos {
        let (k, v) = map.entries.remove(i);
        ts_release_val(k);
        ts_release_val(v);
        return TsVal::from_bool(true);
    }
    TsVal::from_bool(false)
}

/// `map.clear()` — remove all entries.
#[no_mangle]
pub unsafe extern "C" fn ts_map_clear(map_val: TsVal) {
    if !map_val.is_ptr() || heap_tag(map_val) != 5 { return; }
    let map = &mut *(map_val.as_ptr() as *mut TsMap);
    for (k, v) in map.entries.drain(..) {
        ts_release_val(k);
        ts_release_val(v);
    }
}

/// `map.size` — number of entries as integer TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_map_size(map_val: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 5 { return TsVal::from_i32(0); }
    let map = &*(map_val.as_ptr() as *const TsMap);
    TsVal::from_i32(map.entries.len() as i32)
}

/// `map.keys()` — returns a TsArray of all keys (owned refs).
#[no_mangle]
pub unsafe extern "C" fn ts_map_keys(map_val: TsVal) -> TsVal {
    let result = ts_arr_new(0);
    if !map_val.is_ptr() || heap_tag(map_val) != 5 { return result; }
    let map = &*(map_val.as_ptr() as *const TsMap);
    for (k, _) in &map.entries {
        ts_arr_push(result, *k);
    }
    result
}

/// `map.values()` — returns a TsArray of all values (owned refs).
#[no_mangle]
pub unsafe extern "C" fn ts_map_values(map_val: TsVal) -> TsVal {
    let result = ts_arr_new(0);
    if !map_val.is_ptr() || heap_tag(map_val) != 5 { return result; }
    let map = &*(map_val.as_ptr() as *const TsMap);
    for (_, v) in &map.entries {
        ts_arr_push(result, *v);
    }
    result
}

/// `map.forEach(cb)` — call cb(value, key, map) for each entry.
#[no_mangle]
pub unsafe extern "C" fn ts_map_for_each(map_val: TsVal, callback: TsVal) -> TsVal {
    if !map_val.is_ptr() || heap_tag(map_val) != 5 { return UNDEFINED; }
    let map = &*(map_val.as_ptr() as *const TsMap);
    let len = map.entries.len();
    for i in 0..len {
        let (k, v) = { let m = &*(map_val.as_ptr() as *const TsMap); m.entries[i] };
        dispatch_callback(callback, &[v, k, map_val]);
    }
    UNDEFINED
}

/// Destructor for TsMap — releases all keys and values.
pub unsafe extern "C" fn ts_map_destructor(ptr: *mut u8) {
    let map = &mut *(ptr as *mut TsMap);
    for (k, v) in map.entries.drain(..) {
        ts_release_val(k);
        ts_release_val(v);
    }
    std::ptr::drop_in_place(ptr as *mut TsMap);
}

// ── Type predicates ───────────────────────────────────────────────────────────

/// Returns 1 if `val` is a TsArray, 0 otherwise (for `Array.isArray`).
#[no_mangle]
pub unsafe extern "C" fn ts_is_array(val: TsVal) -> i32 {
    if val.is_ptr() && heap_tag(val) == 1 { 1 } else { 0 }
}

// ── Global parsing functions ──────────────────────────────────────────────────

/// Parse the string `s` as an integer in the given `radix` (default 10).
/// Returns NaN (as TsVal f64) if parsing fails.
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

