//! URL and URLSearchParams.

use super::{TsVal, TsString, TsMap, TsObject, UNDEFINED, NULL, heap_tag, ts_release_val};
use super::array::{ts_arr_new, ts_arr_push};
use super::object::{ts_obj_new, ts_obj_set};
use super::map::ts_map_set;
use super::uri::{rust_str_to_val, percent_encode, percent_decode};
use super::string_val::ts_val_to_string;
// ── URLSearchParams helpers ───────────────────────────────────────────────────

/// Decode a query-string component: `+` → space, then percent-decode.
pub(super) fn query_decode(s: &str) -> String {
    percent_decode(&s.replace('+', " "))
}

/// Parse a query string into a URLSearchParams (tag=9, TsMap layout).
pub(super) unsafe fn parse_query_string(query: &str) -> TsVal {
    let size = std::mem::size_of::<TsMap>();
    let ptr = crate::alloc::ts_alloc_rc(size, 9) as *mut TsMap; // tag 9 = URLSearchParams
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsMap { entries: Vec::new() });
    let sp = TsVal::from_ptr(ptr as *mut u8);

    let q = query.trim_start_matches('?');
    if q.is_empty() { return sp; }

    for pair in q.split('&') {
        if pair.is_empty() { continue; }
        let (k, v) = if let Some(idx) = pair.find('=') {
            (&pair[..idx], &pair[idx + 1..])
        } else {
            (pair, "")
        };
        let key_str = query_decode(k);
        let val_str = query_decode(v);
        let k_val = rust_str_to_val(key_str);
        let v_val = rust_str_to_val(val_str);
        // URLSearchParams allows duplicate keys (unlike Map), so we push directly
        let map = &mut *ptr;
        super::ts_retain_val(k_val);
        super::ts_retain_val(v_val);
        map.entries.push((k_val, v_val));
        ts_release_val(k_val);
        ts_release_val(v_val);
    }
    sp
}

/// `new URLSearchParams(init?)` — create a URLSearchParams object.
#[no_mangle]
pub unsafe extern "C" fn ts_urlsearchparams_new(init: TsVal) -> TsVal {
    if init.is_undefined() || init.is_null() {
        let size = std::mem::size_of::<TsMap>();
        let ptr = crate::alloc::ts_alloc_rc(size, 9) as *mut TsMap;
        if ptr.is_null() { return NULL; }
        std::ptr::write(ptr, TsMap { entries: Vec::new() });
        return TsVal::from_ptr(ptr as *mut u8);
    }
    if init.is_ptr() {
        let tag = heap_tag(init);
        if tag == 2 {
            // String: parse as query string
            let ts_str = &*(init.as_ptr() as *const TsString);
            return parse_query_string(&ts_str.inner);
        }
        if tag == 5 || tag == 9 {
            // Copy from another Map/URLSearchParams
            let size = std::mem::size_of::<TsMap>();
            let ptr = crate::alloc::ts_alloc_rc(size, 9) as *mut TsMap;
            if ptr.is_null() { return NULL; }
            std::ptr::write(ptr, TsMap { entries: Vec::new() });
            let sp = TsVal::from_ptr(ptr as *mut u8);
            let src = &*(init.as_ptr() as *const TsMap);
            let dst = &mut *ptr;
            for (k, v) in &src.entries {
                super::ts_retain_val(*k);
                super::ts_retain_val(*v);
                dst.entries.push((*k, *v));
            }
            return sp;
        }
        if tag == 0 {
            // Object: copy string properties
            let size = std::mem::size_of::<TsMap>();
            let ptr = crate::alloc::ts_alloc_rc(size, 9) as *mut TsMap;
            if ptr.is_null() { return NULL; }
            std::ptr::write(ptr, TsMap { entries: Vec::new() });
            let sp = TsVal::from_ptr(ptr as *mut u8);
            let obj = &*(init.as_ptr() as *const TsObject);
            for (k, v) in &obj.properties {
                if k.starts_with("__") { continue; }
                let k_val = rust_str_to_val(k.clone());
                ts_map_set(sp, k_val, *v);
                ts_release_val(k_val);
            }
            return sp;
        }
    }
    // String coerce
    let s_val = ts_val_to_string(init);
    let result = if s_val.is_ptr() && heap_tag(s_val) == 2 {
        let ts_str = &*(s_val.as_ptr() as *const TsString);
        parse_query_string(&ts_str.inner)
    } else {
        let size = std::mem::size_of::<TsMap>();
        let ptr = crate::alloc::ts_alloc_rc(size, 9) as *mut TsMap;
        if ptr.is_null() { return NULL; }
        std::ptr::write(ptr, TsMap { entries: Vec::new() });
        TsVal::from_ptr(ptr as *mut u8)
    };
    ts_release_val(s_val);
    result
}

/// `searchParams.toString()` — serialize to "k=v&k2=v2" query string (no leading '?').
#[no_mangle]
pub unsafe extern "C" fn ts_urlsearchparams_to_string(sp: TsVal) -> TsVal {
    if !sp.is_ptr() || (heap_tag(sp) != 9 && heap_tag(sp) != 5) {
        return rust_str_to_val(String::new());
    }
    let map = &*(sp.as_ptr() as *const TsMap);
    let mut parts: Vec<String> = Vec::new();
    for (k, v) in &map.entries {
        let k_str = ts_val_to_rust_string(*k);
        let v_str = ts_val_to_rust_string(*v);
        parts.push(format!("{}={}", percent_encode(&k_str, false), percent_encode(&v_str, false)));
    }
    rust_str_to_val(parts.join("&"))
}

/// `searchParams.append(name, value)` — append without replacing existing keys.
#[no_mangle]
pub unsafe extern "C" fn ts_urlsearchparams_append(sp: TsVal, name: TsVal, value: TsVal) -> TsVal {
    if !sp.is_ptr() || (heap_tag(sp) != 9 && heap_tag(sp) != 5) {
        return UNDEFINED;
    }
    let map = &mut *(sp.as_ptr() as *mut TsMap);
    super::ts_retain_val(name);
    super::ts_retain_val(value);
    map.entries.push((name, value));
    UNDEFINED
}

/// `searchParams.getAll(name)` — return TsArray of all values for the given key.
#[no_mangle]
pub unsafe extern "C" fn ts_urlsearchparams_get_all(sp: TsVal, name: TsVal) -> TsVal {
    let result = ts_arr_new(0);
    if !sp.is_ptr() || (heap_tag(sp) != 9 && heap_tag(sp) != 5) {
        return result;
    }
    let map = &*(sp.as_ptr() as *const TsMap);
    for (k, v) in &map.entries {
        if super::map::map_key_eq(*k, name) {
            ts_arr_push(result, *v);
        }
    }
    result
}

/// Helper: extract a Rust String from a TsVal without retaining.
unsafe fn ts_val_to_rust_string(val: TsVal) -> String {
    if val.is_ptr() && heap_tag(val) == 2 {
        let ts_str = &*(val.as_ptr() as *const TsString);
        ts_str.inner.clone()
    } else {
        let s = ts_val_to_string(val);
        let result = if s.is_ptr() && heap_tag(s) == 2 {
            let ts_str = &*(s.as_ptr() as *const TsString);
            ts_str.inner.clone()
        } else {
            String::new()
        };
        ts_release_val(s);
        result
    }
}

/// `new URL(href, base?)` — parse a URL and return a TsObject with all URL properties.
#[no_mangle]
pub unsafe extern "C" fn ts_url_new(href: TsVal, _base: TsVal) -> TsVal {
    use url::Url;
    let href_str = ts_val_to_rust_string(href);

    let parsed = match Url::parse(&href_str) {
        Ok(u) => u,
        Err(_) => {
            // Return an object with just href set and empty others
            let obj = ts_obj_new();
            let href_val = rust_str_to_val(href_str);
            ts_obj_set(obj, b"href\0".as_ptr() as *const i8, href_val);
            ts_release_val(href_val);
            let empty = rust_str_to_val(String::new());
            for key in &[b"protocol\0".as_ptr(), b"host\0".as_ptr(), b"hostname\0".as_ptr(),
                         b"port\0".as_ptr(), b"pathname\0".as_ptr(), b"search\0".as_ptr(),
                         b"hash\0".as_ptr(), b"origin\0".as_ptr()] {
                ts_obj_set(obj, *key as *const i8, empty);
            }
            ts_release_val(empty);
            let sp = ts_urlsearchparams_new(UNDEFINED);
            ts_obj_set(obj, b"searchParams\0".as_ptr() as *const i8, sp);
            ts_release_val(sp);
            return obj;
        }
    };

    let obj = ts_obj_new();

    let set_str_prop = |obj: TsVal, key: &[u8], val: String| {
        let v = rust_str_to_val(val);
        ts_obj_set(obj, key.as_ptr() as *const i8, v);
        ts_release_val(v);
    };

    set_str_prop(obj, b"href\0",     parsed.as_str().to_string());
    set_str_prop(obj, b"protocol\0", format!("{}:", parsed.scheme()));
    set_str_prop(obj, b"host\0",     parsed.host_str().unwrap_or("").to_string()
        .chars().chain(parsed.port().map(|p| format!(":{}", p)).unwrap_or_default().chars()).collect());
    set_str_prop(obj, b"hostname\0", parsed.host_str().unwrap_or("").to_string());
    set_str_prop(obj, b"port\0",     parsed.port().map(|p| p.to_string()).unwrap_or_default());
    set_str_prop(obj, b"pathname\0", parsed.path().to_string());
    set_str_prop(obj, b"search\0",   parsed.query().map(|q| format!("?{}", q)).unwrap_or_default());
    set_str_prop(obj, b"hash\0",     parsed.fragment().map(|f| format!("#{}", f)).unwrap_or_default());

    // origin = scheme + "://" + host
    let origin = format!("{}://{}{}",
        parsed.scheme(),
        parsed.host_str().unwrap_or(""),
        parsed.port().map(|p| format!(":{}", p)).unwrap_or_default(),
    );
    set_str_prop(obj, b"origin\0", origin);

    // searchParams
    let query = parsed.query().unwrap_or("");
    let sp = parse_query_string(query);
    ts_obj_set(obj, b"searchParams\0".as_ptr() as *const i8, sp);
    ts_release_val(sp);

    obj
}
