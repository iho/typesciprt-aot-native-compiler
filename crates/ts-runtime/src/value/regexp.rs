//! TsRegExp: heap-allocated regular expressions.

use super::{TsVal, TsRegExp, NULL, FALSE, TRUE, heap_tag};
use super::uri::{str_val_to_rust, rust_str_to_val};
use super::array::{ts_arr_new, ts_arr_set};
use super::string_val::ts_str_replace;

use regex::Regex;

pub unsafe extern "C" fn ts_regexp_destructor(ptr: *mut u8) {
    std::ptr::drop_in_place(ptr as *mut TsRegExp);
}

/// Convert JS regex flags to Rust regex crate flags prefix.
fn js_flags_to_regex_prefix(source: &str, flags: &str) -> String {
    let mut prefix = String::new();
    if flags.contains('i') { prefix.push_str("(?i)"); }
    if flags.contains('s') { prefix.push_str("(?s)"); }
    if flags.contains('m') { prefix.push_str("(?m)"); }
    // Convert JS regex syntax → Rust regex syntax (best-effort)
    format!("{}{}", prefix, source)
}

/// Create a new RegExp from source (C string) and flags (C string).
#[no_mangle]
pub unsafe extern "C" fn ts_regexp_new(source_ptr: *const i8, flags_ptr: *const i8) -> TsVal {
    let source = std::ffi::CStr::from_ptr(source_ptr).to_string_lossy().into_owned();
    let flags = if flags_ptr.is_null() { String::new() }
                else { std::ffi::CStr::from_ptr(flags_ptr).to_string_lossy().into_owned() };
    let size = std::mem::size_of::<TsRegExp>();
    let ptr = crate::alloc::ts_alloc_rc(size, 6) as *mut TsRegExp;
    if ptr.is_null() { return super::UNDEFINED; }
    std::ptr::write(ptr, TsRegExp { source, flags });
    TsVal::from_ptr(ptr as *mut u8)
}

/// Create a RegExp from a TsString source and optional TsString flags.
#[no_mangle]
pub unsafe extern "C" fn ts_regexp_from_val(source_val: TsVal, flags_val: TsVal) -> TsVal {
    use super::TsString;
    let source = if source_val.is_ptr() && heap_tag(source_val) == 2 {
        let ts_str = &*(source_val.as_ptr() as *const TsString);
        ts_str.inner.clone()
    } else {
        return super::UNDEFINED;
    };
    let flags = if flags_val.is_ptr() && heap_tag(flags_val) == 2 {
        let ts_str = &*(flags_val.as_ptr() as *const TsString);
        ts_str.inner.clone()
    } else {
        String::new()
    };
    let size = std::mem::size_of::<TsRegExp>();
    let ptr = crate::alloc::ts_alloc_rc(size, 6) as *mut TsRegExp;
    if ptr.is_null() { return super::UNDEFINED; }
    std::ptr::write(ptr, TsRegExp { source, flags });
    TsVal::from_ptr(ptr as *mut u8)
}

/// re.test(str) → boolean TsVal
#[no_mangle]
pub unsafe extern "C" fn ts_regexp_test(re_val: TsVal, str_val: TsVal) -> TsVal {
    if !re_val.is_ptr() || heap_tag(re_val) != 6 { return FALSE; }
    let re_obj = &*(re_val.as_ptr() as *const TsRegExp);
    let s = if let Some(s) = str_val_to_rust(str_val) { s } else { return FALSE; };
    let pattern = js_flags_to_regex_prefix(&re_obj.source, &re_obj.flags);
    match Regex::new(&pattern) {
        Ok(re) => if re.is_match(&s) { TRUE } else { FALSE },
        Err(_) => FALSE,
    }
}

/// re.exec(str) → TsArray of matches or null
#[no_mangle]
pub unsafe extern "C" fn ts_regexp_exec(re_val: TsVal, str_val: TsVal) -> TsVal {
    if !re_val.is_ptr() || heap_tag(re_val) != 6 { return NULL; }
    let re_obj = &*(re_val.as_ptr() as *const TsRegExp);
    let s = if let Some(s) = str_val_to_rust(str_val) { s } else { return NULL; };
    let pattern = js_flags_to_regex_prefix(&re_obj.source, &re_obj.flags);
    match Regex::new(&pattern) {
        Ok(re) => {
            match re.captures(&s) {
                Some(caps) => {
                    let n = caps.len() as i32;
                    let arr = ts_arr_new(n);
                    for i in 0..n as usize {
                        let val = if let Some(m) = caps.get(i) {
                            rust_str_to_val(m.as_str().to_string())
                        } else { NULL };
                        ts_arr_set(arr, i as i32, val);
                    }
                    arr
                }
                None => NULL,
            }
        }
        Err(_) => NULL,
    }
}

/// str.match(re) → TsArray or null (non-global: same as exec; global: all matches)
#[no_mangle]
pub unsafe extern "C" fn ts_str_match(str_val: TsVal, re_val: TsVal) -> TsVal {
    if !re_val.is_ptr() || heap_tag(re_val) != 6 { return NULL; }
    let re_obj = &*(re_val.as_ptr() as *const TsRegExp);
    let s = if let Some(s) = str_val_to_rust(str_val) { s } else { return NULL; };
    let pattern = js_flags_to_regex_prefix(&re_obj.source, &re_obj.flags);
    match Regex::new(&pattern) {
        Ok(re) => {
            if re_obj.flags.contains('g') {
                // Global: return all full matches as array
                let matches: Vec<&str> = re.find_iter(&s).map(|m| m.as_str()).collect();
                let n = matches.len() as i32;
                let arr = ts_arr_new(n);
                for (i, m) in matches.iter().enumerate() {
                    let val = rust_str_to_val(m.to_string());
                    ts_arr_set(arr, i as i32, val);
                }
                arr
            } else {
                ts_regexp_exec(re_val, str_val)
            }
        }
        Err(_) => NULL,
    }
}

/// str.replace(re_or_str, replacement_str_or_fn) — handles regex replacement with string or callback.
#[no_mangle]
pub unsafe extern "C" fn ts_str_replace_regex(str_val: TsVal, re_val: TsVal, rep_val: TsVal) -> TsVal {
    if !re_val.is_ptr() || heap_tag(re_val) != 6 {
        return ts_str_replace(str_val, re_val, rep_val);
    }
    let re_obj = &*(re_val.as_ptr() as *const TsRegExp);
    let s = if let Some(s) = str_val_to_rust(str_val) { s } else { return str_val; };
    let pattern = js_flags_to_regex_prefix(&re_obj.source, &re_obj.flags);
    let re = match Regex::new(&pattern) {
        Ok(r) => r,
        Err(_) => return str_val,
    };

    // Callback replacer: rep_val is a TsFunction
    if rep_val.is_ptr() && heap_tag(rep_val) == 4 {
        use super::func::dispatch_callback;
        let global = re_obj.flags.contains('g');
        let mut result = String::new();
        let mut last_end = 0;

        for caps in re.captures_iter(&s) {
            let full_match = caps.get(0).unwrap();
            let match_start = full_match.start();
            let match_end = full_match.end();

            // Build args: [fullMatch, group1, group2, ...]
            let mut args: Vec<TsVal> = Vec::with_capacity(caps.len());
            for i in 0..caps.len() {
                args.push(if let Some(m) = caps.get(i) {
                    rust_str_to_val(m.as_str().to_string())
                } else {
                    super::UNDEFINED
                });
            }

            result.push_str(&s[last_end..match_start]);

            let ret_val = dispatch_callback(rep_val, &args);
            result.push_str(&str_val_to_rust(ret_val).unwrap_or_default());

            for &arg in &args { super::ts_release_val(arg); }
            super::ts_release_val(ret_val);

            last_end = match_end;
            if !global { break; }
        }

        result.push_str(&s[last_end..]);
        return rust_str_to_val(result);
    }

    // String replacement
    let rep = if let Some(r) = str_val_to_rust(rep_val) { r } else { String::new() };
    let result = if re_obj.flags.contains('g') {
        re.replace_all(&s, rep.as_str()).into_owned()
    } else {
        re.replace(&s, rep.as_str()).into_owned()
    };
    rust_str_to_val(result)
}

/// Get .source property of RegExp.
#[no_mangle]
pub unsafe extern "C" fn ts_regexp_source(re_val: TsVal) -> TsVal {
    if !re_val.is_ptr() || heap_tag(re_val) != 6 { return super::UNDEFINED; }
    let re_obj = &*(re_val.as_ptr() as *const TsRegExp);
    rust_str_to_val(re_obj.source.clone())
}

/// `str.matchAll(re)` — return array of all capture-group arrays (like global exec).
/// Each element is an array: [fullMatch, group1, group2, ...].
#[no_mangle]
pub unsafe extern "C" fn ts_str_match_all(str_val: TsVal, re_val: TsVal) -> TsVal {
    use super::array::{ts_arr_push, ts_arr_set};
    let s = if let Some(s) = str_val_to_rust(str_val) { s } else { return ts_arr_new(0); };
    // Accept regexp or string
    let (source, flags) = if re_val.is_ptr() && heap_tag(re_val) == 6 {
        let re_obj = &*(re_val.as_ptr() as *const TsRegExp);
        (re_obj.source.clone(), re_obj.flags.clone())
    } else if let Some(s) = str_val_to_rust(re_val) {
        (s, String::new())
    } else {
        return ts_arr_new(0);
    };
    let pattern = js_flags_to_regex_prefix(&source, &flags);
    let re = match Regex::new(&pattern) {
        Ok(r) => r,
        Err(_) => return ts_arr_new(0),
    };
    let result = ts_arr_new(0);
    for caps in re.captures_iter(&s) {
        let match_arr = ts_arr_new(caps.len() as i32);
        for (i, cap) in caps.iter().enumerate() {
            let val = match cap {
                Some(m) => rust_str_to_val(m.as_str().to_string()),
                None => super::UNDEFINED,
            };
            ts_arr_set(match_arr, i as i32, val);
            if !val.is_undefined() { super::ts_release_val(val); }
        }
        ts_arr_push(result, match_arr);
        super::ts_release_val(match_arr);
    }
    result
}

/// `str.search(re)` — returns index of first match or -1.
#[no_mangle]
pub unsafe extern "C" fn ts_str_search(str_val: TsVal, re_val: TsVal) -> TsVal {
    let s = if let Some(s) = str_val_to_rust(str_val) { s } else { return TsVal::from_i32(-1); };
    let (source, flags) = if re_val.is_ptr() && heap_tag(re_val) == 6 {
        let re_obj = &*(re_val.as_ptr() as *const TsRegExp);
        (re_obj.source.clone(), re_obj.flags.clone())
    } else if let Some(s) = str_val_to_rust(re_val) {
        (s, String::new())
    } else {
        return TsVal::from_i32(-1);
    };
    let pattern = js_flags_to_regex_prefix(&source, &flags);
    match Regex::new(&pattern) {
        Ok(re) => match re.find(&s) {
            Some(m) => TsVal::from_i32(m.start() as i32),
            None => TsVal::from_i32(-1),
        },
        Err(_) => TsVal::from_i32(-1),
    }
}
