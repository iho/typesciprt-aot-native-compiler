//! Node.js `url` module — URL class backed by the `url` crate.

use crate::value::{TsVal, UNDEFINED};
use crate::value::object::{ts_obj_new, ts_obj_set};
use super::{new_string, val_to_string};
use url::Url;

/// Macro to produce a static nul-terminated C string pointer from a string literal.
macro_rules! cstr {
    ($s:literal) => { concat!($s, "\0").as_ptr() as *const i8 }
}

/// Set a string property on a TsObject using a static C string key (zero allocation).
#[inline]
unsafe fn set_str(obj: TsVal, key_cstr: *const i8, val: &str) {
    ts_obj_set(obj, key_cstr, new_string(val));
}

/// Parse a URL string and return a TsObject with all URL properties.
/// ts_url_parse(href, base) -> TsObject | UNDEFINED on error
#[no_mangle]
pub unsafe extern "C" fn ts_url_parse(href_val: TsVal, base_val: TsVal) -> TsVal {
    let href_str = val_to_string(href_val).unwrap_or_default();

    let parsed = if let Some(base_str) = val_to_string(base_val) {
        if base_str.is_empty() {
            Url::parse(&href_str)
        } else {
            Url::parse(&base_str).and_then(|base| base.join(&href_str))
        }
    } else {
        Url::parse(&href_str)
    };

    let url = match parsed {
        Ok(u) => u,
        Err(_) => return UNDEFINED,
    };

    let obj = ts_obj_new();

    let host = if let Some(port) = url.port() {
        format!("{}:{}", url.host_str().unwrap_or(""), port)
    } else {
        url.host_str().unwrap_or("").to_string()
    };

    set_str(obj, cstr!("href"),     url.as_str());
    set_str(obj, cstr!("protocol"), &format!("{}:", url.scheme()));
    set_str(obj, cstr!("username"), url.username());
    set_str(obj, cstr!("password"), url.password().unwrap_or(""));
    set_str(obj, cstr!("hostname"), url.host_str().unwrap_or(""));
    set_str(obj, cstr!("port"),     &url.port().map(|p| p.to_string()).unwrap_or_default());
    set_str(obj, cstr!("host"),     &host);
    set_str(obj, cstr!("pathname"), url.path());
    set_str(obj, cstr!("search"),   &if let Some(q) = url.query() { format!("?{}", q) } else { String::new() });
    set_str(obj, cstr!("hash"),     &if let Some(f) = url.fragment() { format!("#{}", f) } else { String::new() });
    set_str(obj, cstr!("origin"),   &format!("{}://{}", url.scheme(), host));

    // Build searchParams as a URLSearchParams (TsMap, tag=9) if query exists
    if let Some(query) = url.query() {
        let sp = crate::value::map::ts_map_new();
        for pair in query.split('&') {
            let (k, v) = if let Some(pos) = pair.find('=') {
                (&pair[..pos], &pair[pos+1..])
            } else {
                (pair, "")
            };
            let k_decoded = percent_decode(k);
            let v_decoded = percent_decode(v);
            let k_val = new_string(&k_decoded);
            let v_val = new_string(&v_decoded);
            crate::value::ts_release_val(crate::value::map::ts_map_set(sp, k_val, v_val));
            crate::value::ts_release_val(k_val);
            crate::value::ts_release_val(v_val);
        }
        ts_obj_set(obj, cstr!("searchParams"), sp);
        crate::value::ts_release_val(sp);
    } else {
        let sp = crate::value::map::ts_map_new();
        ts_obj_set(obj, cstr!("searchParams"), sp);
        crate::value::ts_release_val(sp);
    }

    obj
}

/// Simple percent-decode: replace %XX and + sequences.
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i+1]), from_hex(bytes[i+2])) {
                result.push(char::from(h << 4 | l));
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            result.push(' ');
            i += 1;
            continue;
        }
        result.push(char::from(bytes[i]));
        i += 1;
    }
    result
}

#[inline]
fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Resolve a relative URL against a base URL.
/// ts_url_resolve(base, relative) -> string
#[no_mangle]
pub unsafe extern "C" fn ts_url_resolve(base_val: TsVal, relative_val: TsVal) -> TsVal {
    let base_str = val_to_string(base_val).unwrap_or_default();
    let rel_str  = val_to_string(relative_val).unwrap_or_default();
    match Url::parse(&base_str).and_then(|b| b.join(&rel_str)) {
        Ok(u)  => new_string(u.as_str()),
        Err(_) => new_string(&rel_str),
    }
}

/// Format a URL object back to a string (reads href property).
/// ts_url_format(obj) -> string
#[no_mangle]
pub unsafe extern "C" fn ts_url_format(obj_val: TsVal) -> TsVal {
    use crate::value::object::ts_obj_get;
    let href = ts_obj_get(obj_val, cstr!("href"));
    if href.is_ptr() { href } else { new_string("") }
}
