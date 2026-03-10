//! Heap-allocated immutable string type used by the TS runtime.

use std::alloc::{alloc, dealloc, Layout};
use std::ptr;
use std::slice;
use std::str;

/// Reference-counted, immutable UTF-8 string.
///
/// Layout: `[u32 refcount | u32 len | u8 bytes…]`
#[repr(C)]
pub struct TsString {
    pub refcount: u32,
    pub len:      u32,
    // Followed immediately in memory by `len` UTF-8 bytes.
}

impl TsString {
    /// Allocate a new `TsString` from a Rust `&str`.
    ///
    /// # Safety
    /// Caller must eventually call `ts_string_release`.
    pub unsafe fn new(s: &str) -> *mut TsString {
        let bytes = s.as_bytes();
        let header_size = std::mem::size_of::<TsString>();
        let total = header_size + bytes.len();
        let layout = Layout::from_size_align(total, 4).unwrap();
        let ptr = unsafe { alloc(layout) } as *mut TsString;
        unsafe {
            (*ptr).refcount = 1;
            (*ptr).len      = bytes.len() as u32;
            let data_ptr = (ptr as *mut u8).add(header_size);
            ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
        }
        ptr
    }

    /// Get the UTF-8 byte slice of the string.
    ///
    /// # Safety
    /// `ptr` must point to a valid, live `TsString`.
    pub unsafe fn as_str<'a>(ptr: *const TsString) -> &'a str {
        let header_size = std::mem::size_of::<TsString>();
        let len = unsafe { (*ptr).len as usize };
        let data = unsafe { (ptr as *const u8).add(header_size) };
        let bytes = unsafe { slice::from_raw_parts(data, len) };
        str::from_utf8_unchecked(bytes)
    }
}

/// Increment the reference count of a `TsString`.
///
/// # Safety
/// `ptr` must point to a valid `TsString`.
#[no_mangle]
pub unsafe extern "C" fn ts_string_retain(ptr: *mut TsString) {
    if !ptr.is_null() {
        unsafe { (*ptr).refcount += 1 };
    }
}

/// Decrement the reference count; free when it reaches zero.
///
/// # Safety
/// `ptr` must point to a valid `TsString`.
#[no_mangle]
pub unsafe extern "C" fn ts_string_release(ptr: *mut TsString) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        (*ptr).refcount -= 1;
        if (*ptr).refcount == 0 {
            let len = (*ptr).len as usize;
            let header_size = std::mem::size_of::<TsString>();
            let total  = header_size + len;
            let layout = Layout::from_size_align(total, 4).unwrap();
            dealloc(ptr as *mut u8, layout);
        }
    }
}
