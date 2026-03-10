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




