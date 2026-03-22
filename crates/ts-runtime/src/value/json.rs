//! JSON.stringify and JSON.parse.

use super::{TsVal, TsObject, TsArray, TsString, UNDEFINED, NULL, TRUE, FALSE, heap_tag};
use super::string_val::{ts_string_new};
use super::array::{ts_arr_new, ts_arr_set};
use super::object::{ts_obj_new, ts_obj_set};

thread_local! {
    /// Reused JSON serialization buffer: avoids per-call allocation and resizing.
    /// Cleared at the start of each stringify call; capacity is kept across calls.
    static JSON_BUF: std::cell::UnsafeCell<String> = std::cell::UnsafeCell::new(String::with_capacity(2048));
}

/// JSON.stringify: convert TsVal to a JSON string TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_json_stringify(val: TsVal) -> TsVal {
    let result = JSON_BUF.with(|cell| {
        let buf = &mut *cell.get();
        buf.clear();
        ts_val_write_json(buf, val, 0);
        // Build the null-terminated C string for ts_string_new.
        let mut bytes = buf.as_bytes().to_vec();
        bytes.push(0u8);
        unsafe { ts_string_new(bytes.as_ptr() as *const i8) }
    });
    result
}

/// Write the JSON representation of `val` into `buf`.
/// Uses a write-into-buffer strategy — no intermediate heap allocations per value.
unsafe fn ts_val_write_json(buf: &mut String, val: TsVal, depth: usize) {
    use std::fmt::Write as _;
    if depth > 50 { buf.push_str("null"); return; }
    if val == UNDEFINED || val == NULL { buf.push_str("null"); return; }
    if val == TRUE  { buf.push_str("true");  return; }
    if val == FALSE { buf.push_str("false"); return; }
    if val.is_int32() {
        let _ = write!(buf, "{}", val.as_i32());
        return;
    }
    if val.is_number() {
        let f = val.as_f64();
        if f.is_infinite() || f.is_nan() { buf.push_str("null"); return; }
        if f == f.floor() && f.abs() < 1e15 {
            let _ = write!(buf, "{}", f as i64);
        } else {
            let _ = write!(buf, "{}", f);
        }
        return;
    }
    if val.is_ptr() {
        let tag = heap_tag(val);
        if tag == 2 {
            let ts_str = &*(val.as_ptr() as *const TsString);
            buf.push('"');
            for ch in ts_str.inner.chars() {
                match ch {
                    '\\' => buf.push_str("\\\\"),
                    '"'  => buf.push_str("\\\""),
                    '\n' => buf.push_str("\\n"),
                    '\r' => buf.push_str("\\r"),
                    '\t' => buf.push_str("\\t"),
                    c    => buf.push(c),
                }
            }
            buf.push('"');
            return;
        }
        if tag == 1 {
            let arr = &*(val.as_ptr() as *const TsArray);
            buf.push('[');
            for (i, &v) in arr.elements.iter().enumerate() {
                if i > 0 { buf.push(','); }
                if v == UNDEFINED { buf.push_str("null"); }
                else { ts_val_write_json(buf, v, depth + 1); }
            }
            buf.push(']');
            return;
        }
        if tag == 0 {
            let obj = &*(val.as_ptr() as *const TsObject);
            buf.push('{');
            let mut first = true;
            for (k, &v) in &obj.properties {
                if k.starts_with("__") || v == UNDEFINED { continue; }
                if !first { buf.push(','); }
                first = false;
                buf.push('"');
                if k.contains('"') {
                    for ch in k.chars() {
                        if ch == '"' { buf.push_str("\\\""); } else { buf.push(ch); }
                    }
                } else {
                    buf.push_str(k);
                }
                buf.push_str("\":");
                ts_val_write_json(buf, v, depth + 1);
            }
            buf.push('}');
            return;
        }
    }
    buf.push_str("null");
}

/// JSON.parse: parse JSON string TsVal to a TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_json_parse(str_val: TsVal) -> TsVal {
    if !str_val.is_ptr() || heap_tag(str_val) != 2 {
        return UNDEFINED;
    }
    let ts_str = &*(str_val.as_ptr() as *const TsString);
    let s = ts_str.inner.trim().to_string();
    ts_parse_json_value(&s)
}

unsafe fn ts_parse_json_value(s: &str) -> TsVal {
    let s = s.trim();
    if s == "null" { return NULL; }
    if s == "true" { return TRUE; }
    if s == "false" { return FALSE; }
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len()-1];
        let unescaped = inner
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
            .replace("\\n", "\n")
            .replace("\\r", "\r")
            .replace("\\t", "\t");
        let mut bytes = unescaped.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    if s.starts_with('[') && s.ends_with(']') {
        let inner = s[1..s.len()-1].trim();
        let arr = ts_arr_new(0);
        if inner.is_empty() { return arr; }
        let mut i = 0i32;
        for item_str in json_split_values(inner) {
            let val = ts_parse_json_value(item_str.trim());
            ts_arr_set(arr, i, val);
            super::ts_release_val(val);
            i += 1;
        }
        return arr;
    }
    if s.starts_with('{') && s.ends_with('}') {
        let inner = s[1..s.len()-1].trim();
        let obj = ts_obj_new();
        if inner.is_empty() { return obj; }
        for pair_str in json_split_values(inner) {
            let pair = pair_str.trim();
            if let Some(colon_pos) = json_find_colon(pair) {
                let key_str = pair[..colon_pos].trim();
                let val_str = pair[colon_pos+1..].trim();
                if key_str.starts_with('"') && key_str.ends_with('"') && key_str.len() >= 2 {
                    let key = &key_str[1..key_str.len()-1];
                    let val = ts_parse_json_value(val_str);
                    let mut key_bytes = key.as_bytes().to_vec();
                    key_bytes.push(0u8);
                    ts_obj_set(obj, key_bytes.as_ptr() as *const i8, val);
                    super::ts_release_val(val);
                }
            }
        }
        return obj;
    }
    // Number
    if let Ok(i) = s.parse::<i32>() {
        return TsVal::from_i32(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return TsVal::from_f64(f);
    }
    NULL
}

fn json_split_values(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    let mut start = 0usize;
    for (i, ch) in s.char_indices() {
        if escape { escape = false; continue; }
        if ch == '\\' && in_str { escape = true; continue; }
        if ch == '"' { in_str = !in_str; continue; }
        if in_str { continue; }
        match ch {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    result.push(&s[start..]);
    result
}

fn json_find_colon(s: &str) -> Option<usize> {
    let mut in_str = false;
    let mut escape = false;
    for (i, ch) in s.char_indices() {
        if escape { escape = false; continue; }
        if ch == '\\' && in_str { escape = true; continue; }
        if ch == '"' { in_str = !in_str; continue; }
        if !in_str && ch == ':' { return Some(i); }
    }
    None
}
