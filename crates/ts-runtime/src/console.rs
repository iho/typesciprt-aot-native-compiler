//! Runtime implementations of `console.*` built-ins.
use crate::value::TsVal;

/// `console.log(n: number)` for integer values.
///
/// Called by compiled TypeScript code via the C ABI.
#[no_mangle]
pub extern "C" fn __ts_console_log_i32(n: i32) {
    println!("{n}");
}

/// `console.log(v: any)` for any TsVal.
#[no_mangle]
pub unsafe extern "C" fn __ts_console_log_val(val: TsVal) {
    if val.is_number() {
        println!("{}", val.as_f64());
    } else if val.is_int32() {
        println!("{}", val.as_i32());
    } else if val.is_undefined() {
        println!("undefined");
    } else if val.is_null() {
        println!("null");
    } else if val.is_bool() {
        println!("{}", val.as_bool());
    } else if val.is_ptr() {
        let ptr = val.as_ptr();
        // Check tag
        let header_size = std::mem::size_of::<crate::alloc::ArcHeader>();
        let header = ptr.sub(header_size) as *mut crate::alloc::ArcHeader;
        let tag = (*header).tag;
        
        match tag {
            0 => println!("[object Object]"),
            1 => {
                let arr = ptr as *mut crate::value::TsArray;
                print!("[ ");
                for (i, v) in (&*arr).elements.iter().enumerate() {
                    if i > 0 { print!(", "); }
                    if v.is_number() { print!("{}", v.as_f64()); }
                    else if v.is_int32() { print!("{}", v.as_i32()); }
                    else if v.is_bool() { print!("{}", v.as_bool()); }
                    else if v.is_undefined() { print!("undefined"); }
                    else if v.is_null() { print!("null"); }
                    else if v.is_ptr() {
                        let sub_ptr = v.as_ptr();
                        let sub_header = sub_ptr.sub(header_size) as *mut crate::alloc::ArcHeader;
                        let sub_tag = (*sub_header).tag;
                        match sub_tag {
                            0 => print!("[object Object]"),
                            1 => print!("[Array]"),
                            2 => {
                                let s = sub_ptr as *mut crate::value::TsString;
                                print!("'{}'", (&*s).inner);
                            }
                            _ => print!("[ptr]"),
                        }
                    }
                }
                println!(" ]");
            }
            2 => {
                let s = ptr as *mut crate::value::TsString;
                println!("{}", (&*s).inner);
            }
            _ => println!("[unknown pointer]"),
        }
    } else {
        println!("[unknown value]");
    }
}
