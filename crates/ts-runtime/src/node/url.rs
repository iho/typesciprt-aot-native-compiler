//! Node.js `url` module — URL class backed by the `url` crate.

use crate::value::{TsVal, UNDEFINED};
use crate::value::object::{ts_obj_new, ts_obj_set};
use super::{new_string, val_to_string};
use url::Url;

/// Set a string property on a TsObject using a Rust string key.
unsafe fn obj_set_str(obj: TsVal, key: &str, val: &str) {
    let k = std::ffi::CString::new(key).unwrap();
    ts_obj_set(obj, k.as_ptr(), new_string(val));
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

    obj_set_str(obj, "href",     url.as_str());
    obj_set_str(obj, "protocol", &format!("{}:", url.scheme()));
    obj_set_str(obj, "username", url.username());
    obj_set_str(obj, "password", url.password().unwrap_or(""));
    obj_set_str(obj, "hostname", url.host_str().unwrap_or(""));
    obj_set_str(obj, "port",     &url.port().map(|p| p.to_string()).unwrap_or_default());

    let host = if let Some(port) = url.port() {
        format!("{}:{}", url.host_str().unwrap_or(""), port)
    } else {
        url.host_str().unwrap_or("").to_string()
    };
    obj_set_str(obj, "host", &host);
    obj_set_str(obj, "pathname", url.path());
    obj_set_str(obj, "search",   &if let Some(q) = url.query() { format!("?{}", q) } else { String::new() });
    obj_set_str(obj, "hash",     &if let Some(f) = url.fragment() { format!("#{}", f) } else { String::new() });
    obj_set_str(obj, "origin",   &format!("{}://{}", url.scheme(), host));

    obj
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
    let key = std::ffi::CString::new("href").unwrap();
    let href = ts_obj_get(obj_val, key.as_ptr());
    if href.is_ptr() { href } else { new_string("") }
}
