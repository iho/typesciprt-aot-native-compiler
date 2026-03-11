//! Runtime implementations of `console.*` built-ins.
use crate::value::TsVal;

/// `console.log(n: number)` for integer values.
///
/// Called by compiled TypeScript code via the C ABI.
#[no_mangle]
pub extern "C" fn __ts_console_log_i32(n: i32) {
    println!("{n}");
}

/// Format a TsVal as a string for display (shared by log variants).
unsafe fn fmt_val(val: TsVal) -> String {
    if val.is_number() { return val.as_f64().to_string(); }
    if val.is_int32()  { return val.as_i32().to_string(); }
    if val.is_undefined() { return "undefined".to_string(); }
    if val.is_null()      { return "null".to_string(); }
    if val.is_bool()      { return val.as_bool().to_string(); }
    if val.is_ptr() {
        let ptr = val.as_ptr();
        let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
        let header = ptr.sub(header_size) as *const crate::alloc::ArcHeader;
        match (*header).tag {
            0 => return "[object Object]".to_string(),
            1 => {
                let arr = &*(ptr as *const crate::value::TsArray);
                let elems: Vec<String> = arr.elements.iter().map(|&v| fmt_val(v)).collect();
                return format!("[ {} ]", elems.join(", "));
            }
            2 => {
                let s = &*(ptr as *const crate::value::TsString);
                return s.inner.clone();
            }
            _ => return "[ptr]".to_string(),
        }
    }
    "[unknown]".to_string()
}

/// `console.log(v: any)` — prints one value followed by a newline.
#[no_mangle]
pub unsafe extern "C" fn __ts_console_log_val(val: TsVal) {
    println!("{}", fmt_val(val));
}

/// Print a value without a trailing newline (for multi-arg console.log).
#[no_mangle]
pub unsafe extern "C" fn __ts_console_log_val_inline(val: TsVal) {
    print!("{}", fmt_val(val));
}

/// Print a single space (separator between console.log arguments).
#[no_mangle]
pub extern "C" fn __ts_console_log_space() {
    print!(" ");
}

/// Print a newline (end of console.log call).
#[no_mangle]
pub extern "C" fn __ts_console_log_newline() {
    println!();
}
