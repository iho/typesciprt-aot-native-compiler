//! URI encoding/decoding and string helpers.

use super::{TsVal, TsString, UNDEFINED, heap_tag};
use super::string_val::ts_string_new;

pub(super) fn percent_encode(s: &str, reserved_ok: bool) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        let c = *b as char;
        let unreserved = c.is_ascii_alphanumeric() || matches!(c, '-'|'_'|'.'|'!'|'~'|'*'|'\''|'('|')');
        let reserved = matches!(c, ';'|','|'/'|'?'|':'|'@'|'&'|'='|'+'|'$'|'#');
        if unreserved || (reserved_ok && reserved) {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

pub(super) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Ok(hi), Ok(lo)) = (
                std::str::from_utf8(&bytes[i+1..i+2]).map(|s| u8::from_str_radix(s, 16)),
                std::str::from_utf8(&bytes[i+2..i+3]).map(|s| u8::from_str_radix(s, 16)),
            ) {
                if let (Ok(h), Ok(l)) = (hi, lo) {
                    out.push(h << 4 | l);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(super) fn str_val_to_rust(val: TsVal) -> Option<String> {
    if val.is_ptr() && unsafe { heap_tag(val) } == 2 {
        let ts_str = unsafe { &*(val.as_ptr() as *const TsString) };
        Some(ts_str.inner.clone())
    } else {
        None
    }
}

pub(super) fn rust_str_to_val(s: String) -> TsVal {
    let mut bytes = s.into_bytes();
    bytes.push(0u8);
    unsafe { ts_string_new(bytes.as_ptr() as *const i8) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_encode_uri_component(val: TsVal) -> TsVal {
    let s = if let Some(s) = str_val_to_rust(val) { s } else { return UNDEFINED; };
    rust_str_to_val(percent_encode(&s, false))
}

#[no_mangle]
pub unsafe extern "C" fn ts_decode_uri_component(val: TsVal) -> TsVal {
    let s = if let Some(s) = str_val_to_rust(val) { s } else { return UNDEFINED; };
    rust_str_to_val(percent_decode(&s))
}

#[no_mangle]
pub unsafe extern "C" fn ts_encode_uri(val: TsVal) -> TsVal {
    let s = if let Some(s) = str_val_to_rust(val) { s } else { return UNDEFINED; };
    rust_str_to_val(percent_encode(&s, true))
}

#[no_mangle]
pub unsafe extern "C" fn ts_decode_uri(val: TsVal) -> TsVal {
    let s = if let Some(s) = str_val_to_rust(val) { s } else { return UNDEFINED; };
    rust_str_to_val(percent_decode(&s))
}
