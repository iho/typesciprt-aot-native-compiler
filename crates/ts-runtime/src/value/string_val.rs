//! TsString: heap-allocated TypeScript strings and string methods.

use super::{TsVal, TsString, TsArray, NULL, heap_tag, ts_retain_val, ts_release_val};

pub unsafe extern "C" fn ts_string_destructor(ptr: *mut u8) {
    let s_ptr = ptr as *mut TsString;
    std::ptr::drop_in_place(s_ptr);
}

#[no_mangle]
pub unsafe extern "C" fn ts_string_new(c_str: *const i8) -> TsVal {
    let s = std::ffi::CStr::from_ptr(c_str).to_string_lossy().into_owned();
    let size = std::mem::size_of::<TsString>();
    let ptr = crate::alloc::ts_alloc_rc(size, 2) as *mut TsString; // tag 2 = String
    if ptr.is_null() {
        return NULL;
    }
    std::ptr::write(ptr, TsString { inner: s });
    TsVal::from_ptr(ptr as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn ts_string_concat(v1: TsVal, v2: TsVal) -> TsVal {
    let p1 = v1.as_ptr() as *mut TsString;
    let p2 = v2.as_ptr() as *mut TsString;

    let s1 = &(*p1).inner;
    let s2 = &(*p2).inner;

    let new_s = format!("{}{}", s1, s2);
    let size = std::mem::size_of::<TsString>();
    let ptr = crate::alloc::ts_alloc_rc(size, 2) as *mut TsString;
    if ptr.is_null() {
        return NULL;
    }
    std::ptr::write(ptr, TsString { inner: new_s });
    TsVal::from_ptr(ptr as *mut u8)
}

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
            6 => {
                // RegExp: format as /source/flags
                let re = &*(val.as_ptr() as *const crate::value::TsRegExp);
                let s = format!("/{}/{}\0", re.source, re.flags);
                return ts_string_new(s.as_ptr() as *const i8);
            }
            4 => return ts_string_new(b"function() { [native code] }\0".as_ptr() as *const i8),
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

/// `str.indexOf(search, fromIndex)` — returns first char-index >= fromIndex of `search` in `s`.
#[no_mangle]
pub unsafe extern "C" fn ts_str_index_of_from(s_val: TsVal, search_val: TsVal, from_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 && search_val.is_ptr() && heap_tag(search_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let search = &*(search_val.as_ptr() as *const TsString);
        let chars: Vec<char> = s.inner.chars().collect();
        let from = if from_val.is_int32() {
            (from_val.as_i32() as usize).min(chars.len())
        } else {
            0
        };
        // Convert char-index to byte-index for the slice
        let byte_start: usize = chars[..from].iter().map(|c: &char| c.len_utf8()).sum();
        if let Some(byte_pos) = s.inner[byte_start..].find(search.inner.as_str()) {
            let char_pos = s.inner[..byte_start + byte_pos].chars().count();
            return TsVal::from_i32(char_pos as i32);
        }
    }
    TsVal::from_i32(-1)
}

/// `str.lastIndexOf(search)` — returns last char-index of `search` in `s` (or -1).
#[no_mangle]
pub unsafe extern "C" fn ts_str_last_index_of(s_val: TsVal, search_val: TsVal) -> TsVal {
    if s_val.is_ptr() && heap_tag(s_val) == 2 && search_val.is_ptr() && heap_tag(search_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let search = &*(search_val.as_ptr() as *const TsString);
        if let Some(pos) = s.inner.rfind(search.inner.as_str()) {
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
            1 => super::array::ts_arr_index_of(obj, search),
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
            1 => TsVal::from_bool(super::array::ts_arr_index_of(obj, search).as_i32() >= 0),
            2 => ts_str_includes(obj, search),
            _ => TsVal::from_bool(false),
        }
    } else {
        TsVal::from_bool(false)
    }
}

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
            return super::array::ts_arr_new(0);
        };
        let parts: Vec<String> = s.inner.split(sep.as_str()).map(|p| p.to_string()).collect();
        let arr = super::array::ts_arr_new(0);
        for part in parts {
            let mut bytes = part.into_bytes();
            bytes.push(0u8);
            let part_val = ts_string_new(bytes.as_ptr() as *const i8);
            super::array::ts_arr_push(arr, part_val);
            ts_release_val(part_val);
        }
        return arr;
    }
    super::array::ts_arr_new(0)
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

/// `s.at(index)` — return the character at index (supports negative indices) as a string.
/// Returns `undefined` if index is out of bounds.
#[no_mangle]
pub unsafe extern "C" fn ts_str_at(s_val: TsVal, idx_val: TsVal) -> TsVal {
    use super::UNDEFINED;
    if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let s = &*(s_val.as_ptr() as *const TsString);
        let chars: Vec<char> = s.inner.chars().collect();
        let len = chars.len() as i32;
        let idx = if idx_val.is_int32() { idx_val.as_i32() } else { 0 };
        let actual = if idx < 0 { len + idx } else { idx };
        if actual >= 0 && actual < len {
            let mut bytes = chars[actual as usize].to_string().into_bytes();
            bytes.push(0u8);
            return ts_string_new(bytes.as_ptr() as *const i8);
        }
    }
    UNDEFINED
}
