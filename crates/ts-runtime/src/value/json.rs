//! JSON.stringify and JSON.parse.

use super::{TsVal, TsObject, TsArray, TsString, UNDEFINED, NULL, TRUE, FALSE, heap_tag};
use super::string_val::{ts_string_new};
use super::array::{ts_arr_new, ts_arr_set};
use super::object::{ts_obj_new, ts_obj_set};

/// JSON.stringify: convert TsVal to a JSON string TsVal.
#[no_mangle]
pub unsafe extern "C" fn ts_json_stringify(val: TsVal) -> TsVal {
    let s = ts_val_to_json_string(val, 0);
    let mut bytes = s.into_bytes();
    bytes.push(0u8);
    ts_string_new(bytes.as_ptr() as *const i8)
}

unsafe fn ts_val_to_json_string(val: TsVal, depth: usize) -> String {
    if depth > 50 { return "null".to_string(); }
    if val == UNDEFINED { return "null".to_string(); }
    if val == NULL { return "null".to_string(); }
    if val == TRUE { return "true".to_string(); }
    if val == FALSE { return "false".to_string(); }
    if val.is_int32() {
        return val.as_i32().to_string();
    }
    if val.is_number() {
        let f = val.as_f64();
        if f.is_infinite() || f.is_nan() { return "null".to_string(); }
        if f == f.floor() && f.abs() < 1e15 {
            return format!("{}", f as i64);
        }
        return format!("{}", f);
    }
    if val.is_ptr() {
        let tag = heap_tag(val);
        if tag == 2 {
            // String: JSON-encode it
            let ts_str = &*(val.as_ptr() as *const TsString);
            let mut escaped = String::with_capacity(ts_str.inner.len() + 2);
            for ch in ts_str.inner.chars() {
                match ch {
                    '\\' => escaped.push_str("\\\\"),
                    '"'  => escaped.push_str("\\\""),
                    '\n' => escaped.push_str("\\n"),
                    '\r' => escaped.push_str("\\r"),
                    '\t' => escaped.push_str("\\t"),
                    c    => escaped.push(c),
                }
            }
            return format!("\"{}\"", escaped);
        }
        if tag == 1 {
            // Array
            let arr = &*(val.as_ptr() as *const TsArray);
            let items: Vec<String> = arr.elements.iter().map(|&v| {
                if v == UNDEFINED { "null".to_string() }
                else { ts_val_to_json_string(v, depth + 1) }
            }).collect();
            return format!("[{}]", items.join(","));
        }
        if tag == 0 {
            // Object
            let obj = &*(val.as_ptr() as *const TsObject);
            let props: Vec<String> = obj.properties.iter()
                .filter(|(k, _)| !k.starts_with("__"))
                .filter_map(|(k, &v)| {
                    if v == UNDEFINED { return None; }
                    let escaped_key = k.replace('"', "\\\"");
                    Some(format!("\"{}\":{}", escaped_key, ts_val_to_json_string(v, depth + 1)))
                })
                .collect();
            return format!("{{{}}}", props.join(","));
        }
    }
    "null".to_string()
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
