//! URL and URLSearchParams.

use super::{TsVal, TsString, TsMap, TsObject, TsUrl, UNDEFINED, NULL, heap_tag, ts_release_val};
use super::array::{ts_arr_new, ts_arr_push};
use super::object::{ts_obj_new, ts_obj_set};
use super::map::ts_map_set;
use super::uri::{rust_str_to_val, percent_encode, percent_decode};
use super::string_val::{ts_string_new, ts_val_to_string};
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

/// Destructor for TsUrl (tag=20): release all 12 TsVal fields.
pub unsafe extern "C" fn ts_url_destructor(ptr: *mut u8) {
    let u = &mut *(ptr as *mut TsUrl);
    ts_release_val(u.href);
    ts_release_val(u.protocol);
    ts_release_val(u.host);
    ts_release_val(u.hostname);
    ts_release_val(u.port);
    ts_release_val(u.pathname);
    ts_release_val(u.search);
    ts_release_val(u.hash);
    ts_release_val(u.origin);
    ts_release_val(u.username);
    ts_release_val(u.password);
    ts_release_val(u.search_params);
    std::ptr::drop_in_place(u as *mut TsUrl);
}

/// Fast-path URL parsing for `http://` and `https://` URLs (no base, no auth, no fragment).
/// Returns None if the URL doesn't match the fast path — caller falls back to `url::Url::parse`.
fn try_parse_http_url_fast(s: &str) -> Option<(&str, &str, &str, &str, &str, &str, &str)> {
    // scheme = "http" or "https" (only)
    let (scheme, rest) = if let Some(r) = s.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = s.strip_prefix("http://") {
        ("http", r)
    } else {
        return None;
    };
    // No userinfo allowed in fast path (no '@')
    if rest.contains('@') { return None; }
    // Find start of path (or end of authority)
    let path_start = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..path_start];
    let path_query_hash = &rest[path_start..];
    // Split host:port
    let (hostname, port) = if let Some(colon_pos) = authority.rfind(':') {
        (&authority[..colon_pos], &authority[colon_pos + 1..])
    } else {
        (authority, "")
    };
    // Split path, query, hash
    let (path_query, hash) = if let Some(h) = path_query_hash.find('#') {
        (&path_query_hash[..h], &path_query_hash[h..])
    } else {
        (path_query_hash, "")
    };
    let (pathname, search) = if let Some(q) = path_query.find('?') {
        (&path_query[..q], &path_query[q..])
    } else {
        (path_query, "")
    };
    Some((scheme, hostname, port, pathname, search, hash, s))
}

/// `new URL(href, base?)` — parse a URL and return a TsUrl (tag=20) with all URL properties.
/// Uses a fixed-field struct instead of a TsObject HashMap, eliminating 10 HashMap inserts.
/// Uses a fast-path parser for plain http:// and https:// URLs.
#[no_mangle]
pub unsafe extern "C" fn ts_url_new(href: TsVal, base: TsVal) -> TsVal {
    let href_str = ts_val_to_rust_string(href);

    // Only treat base as a URL string if it's actually a TsString (tag=2).
    let base_str = if base.is_ptr() && heap_tag(base) == 2 {
        let ts_str = &*(base.as_ptr() as *const TsString);
        ts_str.inner.clone()
    } else {
        String::new()
    };

    // Fast path for plain http:// / https:// with no base URL.
    if base_str.is_empty() {
        if let Some((scheme, hostname, port, pathname, search, hash, href_out)) =
            try_parse_http_url_fast(&href_str)
        {
            // Use static C-string literals to avoid heap alloc for static strings.
            // Interned strings have IMMORTAL_RC so retain/release are no-ops —
            // safe to store multiple references to the same immortal TsVal.
            let protocol_val = if scheme == "https" {
                ts_string_new(b"https:\0".as_ptr() as *const i8)
            } else {
                ts_string_new(b"http:\0".as_ptr() as *const i8)
            };
            let empty_val = ts_string_new(b"\0".as_ptr() as *const i8);

            let host = if port.is_empty() {
                hostname.to_string()
            } else {
                format!("{}:{}", hostname, port)
            };
            let origin = format!("{}://{}", scheme, host);
            let query = search.strip_prefix('?').unwrap_or("");

            let size = std::mem::size_of::<TsUrl>();
            let ptr = crate::alloc::ts_alloc_rc(size, 20) as *mut TsUrl;
            if ptr.is_null() { return NULL; }

            std::ptr::write(ptr, TsUrl {
                href:          rust_str_to_val(href_out.to_string()),
                protocol:      protocol_val,
                host:          rust_str_to_val(host),
                hostname:      rust_str_to_val(hostname.to_string()),
                port:          rust_str_to_val(port.to_string()),
                pathname:      rust_str_to_val(if pathname.is_empty() { "/".to_string() } else { pathname.to_string() }),
                search:        if search.is_empty() { empty_val } else { rust_str_to_val(search.to_string()) },
                hash:          if hash.is_empty() { empty_val } else { rust_str_to_val(hash.to_string()) },
                origin:        rust_str_to_val(origin),
                username:      empty_val,
                password:      empty_val,
                search_params: parse_query_string(query),
            });
            return TsVal::from_ptr(ptr as *mut u8);
        }
    }

    // Full parse via the url crate for everything else.
    use url::Url;
    let parsed = if base_str.is_empty() {
        Url::parse(&href_str)
    } else {
        Url::parse(&base_str).and_then(|b| b.join(&href_str))
    };

    let parsed = match parsed {
        Ok(u) => u,
        Err(_) => {
            // Return an empty TsUrl on parse error
            let empty = rust_str_to_val(String::new());
            let href_val = rust_str_to_val(href_str);
            let sp = ts_urlsearchparams_new(UNDEFINED);
            let size = std::mem::size_of::<TsUrl>();
            let ptr = crate::alloc::ts_alloc_rc(size, 20) as *mut TsUrl;
            if ptr.is_null() { ts_release_val(href_val); ts_release_val(empty); ts_release_val(sp); return NULL; }
            super::ts_retain_val(empty); super::ts_retain_val(empty); super::ts_retain_val(empty);
            super::ts_retain_val(empty); super::ts_retain_val(empty); super::ts_retain_val(empty);
            super::ts_retain_val(empty); super::ts_retain_val(empty); super::ts_retain_val(empty);
            std::ptr::write(ptr, TsUrl {
                href: href_val, protocol: empty, host: empty,
                hostname: empty, port: empty, pathname: empty,
                search: empty, hash: empty, origin: empty,
                username: empty, password: empty, search_params: sp,
            });
            ts_release_val(empty);
            return TsVal::from_ptr(ptr as *mut u8);
        }
    };

    let host_str = parsed.host_str().unwrap_or("");
    let port_str = parsed.port().map(|p| p.to_string()).unwrap_or_default();
    let host = if port_str.is_empty() {
        host_str.to_string()
    } else {
        format!("{}:{}", host_str, port_str)
    };
    let origin = format!("{}://{}", parsed.scheme(), host);
    let query = parsed.query().unwrap_or("");

    let size = std::mem::size_of::<TsUrl>();
    let ptr = crate::alloc::ts_alloc_rc(size, 20) as *mut TsUrl;
    if ptr.is_null() { return NULL; }

    std::ptr::write(ptr, TsUrl {
        href:          rust_str_to_val(parsed.as_str().to_string()),
        protocol:      rust_str_to_val(format!("{}:", parsed.scheme())),
        host:          rust_str_to_val(host),
        hostname:      rust_str_to_val(host_str.to_string()),
        port:          rust_str_to_val(port_str),
        pathname:      rust_str_to_val(parsed.path().to_string()),
        search:        rust_str_to_val(if query.is_empty() { String::new() } else { format!("?{}", query) }),
        hash:          rust_str_to_val(parsed.fragment().map(|f| format!("#{}", f)).unwrap_or_default()),
        origin:        rust_str_to_val(origin),
        username:      rust_str_to_val(parsed.username().to_string()),
        password:      rust_str_to_val(parsed.password().unwrap_or("").to_string()),
        search_params: parse_query_string(query),
    });
    TsVal::from_ptr(ptr as *mut u8)
}
