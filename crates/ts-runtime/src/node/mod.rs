//! Node.js built-in module implementations.

pub mod path;
pub mod os;
pub mod fs;
pub mod crypto;
pub mod events;
pub mod buffer;
pub mod process_ext;
pub mod http;
pub mod net;
pub mod url;
pub mod child_process;
pub mod zlib;
pub mod readline;
pub mod perf_hooks;
pub mod dns;

pub use events::HEAP_TAG_EVENT_EMITTER;
pub use buffer::HEAP_TAG_BUFFER;

/// Helper: allocate a TsVal string from a Rust &str.
pub(super) unsafe fn new_string(s: &str) -> crate::value::TsVal {
    use crate::value::string_val::ts_string_new;
    let cs = std::ffi::CString::new(s.as_bytes().to_vec()).unwrap_or_else(|_| {
        std::ffi::CString::new("").unwrap()
    });
    ts_string_new(cs.as_ptr())
}

/// Helper: extract Rust String from a TsVal string (tag 2).
pub(super) unsafe fn val_to_string(v: crate::value::TsVal) -> Option<String> {
    if !v.is_ptr() { return None; }
    if crate::value::heap_tag(v) != 2 { return None; }
    let s = &*(v.as_ptr() as *const crate::value::TsString);
    Some(s.inner.clone())
}

/// Helper: i32 from TsVal (int or float).
pub(super) fn val_to_i32(v: crate::value::TsVal) -> i32 {
    if v.is_int32() { v.as_i32() }
    else if v.is_number() { v.as_f64() as i32 }
    else { 0 }
}
