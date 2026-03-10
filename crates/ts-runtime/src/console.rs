//! Runtime implementations of `console.*` built-ins.

/// `console.log(n: number)` for integer values.
///
/// Called by compiled TypeScript code via the C ABI.
#[no_mangle]
pub extern "C" fn __ts_console_log_i32(n: i32) {
    println!("{n}");
}

/// `console.log(s: string)` for string values.
///
/// `ptr` must point to a valid null-terminated UTF-8 string.
/// Called by compiled TypeScript code via the C ABI.
#[no_mangle]
pub unsafe extern "C" fn __ts_console_log_str(ptr: *const u8) {
    let s = unsafe { std::ffi::CStr::from_ptr(ptr as *const std::ffi::c_char) };
    println!("{}", s.to_string_lossy());
}
