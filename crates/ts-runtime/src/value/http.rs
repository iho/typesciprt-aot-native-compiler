//! HTTP: Headers, Response, Request, serve, addEventListener.

use super::{TsVal, TsMap, TsObject, TsString, TsResponse, UNDEFINED, NULL, heap_tag, ts_retain_val, ts_release_val};
use super::array::{ts_arr_new, ts_arr_push};
use super::object::{ts_obj_new, ts_obj_get, ts_obj_set};
use super::map::{ts_map_set, map_key_eq};
use super::uri::{rust_str_to_val, str_val_to_rust};
use super::promise::{ts_promise_resolve, ts_promise_await, get_runtime, make_promise_pair, alloc_promise, resolve_arc};
use super::TsPromise;
use super::func::dispatch_callback;
use super::string_val::{ts_string_new, ts_val_to_string};
use super::json::ts_json_parse;

use std::sync::atomic::{AtomicU64, Ordering};

// ── Web API: Headers (tag=7) ──────────────────────────────────────────────────

/// TsHeaders has the same memory layout as TsMap so all ts_map_* functions work for it.
/// tag=7 allocated via ts_alloc_rc(size, 7).

pub unsafe extern "C" fn ts_headers_destructor(ptr: *mut u8) {
    // Same layout as TsMap
    let map_ptr = ptr as *mut TsMap;
    for (k, v) in (*map_ptr).entries.drain(..) {
        ts_release_val(k);
        ts_release_val(v);
    }
    std::ptr::drop_in_place(map_ptr);
}

/// `new Headers(init?)` — create a new Headers object.
#[no_mangle]
pub unsafe extern "C" fn ts_headers_new(init: TsVal) -> TsVal {
    let size = std::mem::size_of::<TsMap>();
    let ptr = crate::alloc::ts_alloc_rc(size, 7) as *mut TsMap;
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsMap { entries: Vec::new() });
    let headers_val = TsVal::from_ptr(ptr as *mut u8);

    if init.is_ptr() {
        let tag = heap_tag(init);
        if tag == 0 {
            // TsObject: copy string properties as header key/val pairs
            let obj = &*(init.as_ptr() as *const TsObject);
            for (k, v) in &obj.properties {
                if k.starts_with("__") { continue; }
                let k_val = rust_str_to_val(k.clone());
                ts_map_set(headers_val, k_val, *v);
                ts_release_val(k_val);
            }
        } else if tag == 7 {
            // Clone from another TsHeaders
            let src = &*(init.as_ptr() as *const TsMap);
            let dst = &mut *ptr;
            for (k, v) in &src.entries {
                ts_retain_val(*k);
                ts_retain_val(*v);
                dst.entries.push((*k, *v));
            }
        }
    }
    headers_val
}

/// `headers.append(name, value)` — add entry without removing existing ones.
/// Also handles URLSearchParams (tag=9) which uses the same TsMap layout.
#[no_mangle]
pub unsafe extern "C" fn ts_headers_append(headers_val: TsVal, name: TsVal, value: TsVal) -> TsVal {
    let tag = if headers_val.is_ptr() { heap_tag(headers_val) } else { 255 };
    if tag != 7 && tag != 9 {
        ts_retain_val(headers_val);
        return headers_val;
    }
    let map = &mut *(headers_val.as_ptr() as *mut TsMap);
    ts_retain_val(name);
    ts_retain_val(value);
    map.entries.push((name, value));
    ts_retain_val(headers_val);
    headers_val
}

/// `headers.get(name)` — return first matching header value or null.
#[no_mangle]
pub unsafe extern "C" fn ts_headers_get(headers_val: TsVal, name: TsVal) -> TsVal {
    if !headers_val.is_ptr() || heap_tag(headers_val) != 7 { return NULL; }
    let map = &*(headers_val.as_ptr() as *const TsMap);
    for (k, v) in &map.entries {
        if map_key_eq(*k, name) {
            ts_retain_val(*v);
            return *v;
        }
    }
    NULL
}

/// `headers.has(name)` — return true if header exists.
#[no_mangle]
pub unsafe extern "C" fn ts_headers_has(headers_val: TsVal, name: TsVal) -> TsVal {
    if !headers_val.is_ptr() || heap_tag(headers_val) != 7 {
        return TsVal::from_bool(false);
    }
    let map = &*(headers_val.as_ptr() as *const TsMap);
    for (k, _) in &map.entries {
        if map_key_eq(*k, name) {
            return TsVal::from_bool(true);
        }
    }
    TsVal::from_bool(false)
}

/// `headers.set(name, value)` — set or replace a header value.
#[no_mangle]
pub unsafe extern "C" fn ts_headers_set(headers_val: TsVal, name: TsVal, value: TsVal) -> TsVal {
    if !headers_val.is_ptr() || heap_tag(headers_val) != 7 {
        ts_retain_val(headers_val);
        return headers_val;
    }
    let map = &mut *(headers_val.as_ptr() as *mut TsMap);
    // Replace existing entry if found.
    for (k, v) in map.entries.iter_mut() {
        if map_key_eq(*k, name) {
            ts_release_val(*v);
            ts_retain_val(value);
            *v = value;
            ts_retain_val(headers_val);
            return headers_val;
        }
    }
    // Otherwise append.
    ts_retain_val(name);
    ts_retain_val(value);
    map.entries.push((name, value));
    ts_retain_val(headers_val);
    headers_val
}

/// `headers.delete(name)` — remove a header.
#[no_mangle]
pub unsafe extern "C" fn ts_headers_delete(headers_val: TsVal, name: TsVal) -> TsVal {
    if !headers_val.is_ptr() || heap_tag(headers_val) != 7 {
        return TsVal::from_bool(false);
    }
    let map = &mut *(headers_val.as_ptr() as *mut TsMap);
    let before = map.entries.len();
    map.entries.retain(|(k, v)| {
        if map_key_eq(*k, name) {
            ts_release_val(*k);
            ts_release_val(*v);
            false
        } else {
            true
        }
    });
    TsVal::from_bool(map.entries.len() < before)
}

/// `headers.getSetCookie()` — return TsArray of all "set-cookie" header values.
#[no_mangle]
pub unsafe extern "C" fn ts_headers_get_set_cookie(headers_val: TsVal) -> TsVal {
    let result = ts_arr_new(0);
    if !headers_val.is_ptr() || heap_tag(headers_val) != 7 { return result; }
    let map = &*(headers_val.as_ptr() as *const TsMap);
    let target = rust_str_to_val("set-cookie".to_string());
    for (k, v) in &map.entries {
        if map_key_eq(*k, target) {
            ts_retain_val(*v);
            ts_arr_push(result, *v);
        }
    }
    ts_release_val(target);
    result
}

// ── Web API: Response (tag=8) ─────────────────────────────────────────────────

pub unsafe extern "C" fn ts_response_destructor(ptr: *mut u8) {
    let resp = &mut *(ptr as *mut TsResponse);
    ts_release_val(resp.body);
    ts_release_val(resp.headers);
    std::ptr::drop_in_place(resp as *mut TsResponse);
}

/// `new Response(body?, init?)` — create a new Response object.
#[no_mangle]
pub unsafe extern "C" fn ts_response_new(body: TsVal, init: TsVal) -> TsVal {
    let size = std::mem::size_of::<TsResponse>();
    let ptr = crate::alloc::ts_alloc_rc(size, 8) as *mut TsResponse;
    if ptr.is_null() { return NULL; }

    let mut status: u16 = 200;
    let headers_val = ts_headers_new(UNDEFINED);

    // Parse init
    if init.is_ptr() {
        let tag = heap_tag(init);
        if tag == 0 {
            // { status?: number, headers?: HeadersInit }
            let obj = &*(init.as_ptr() as *const TsObject);
            if let Some(&s) = obj.properties.get("status") {
                if s.is_int32() { status = s.as_i32() as u16; }
                else if !s.is_nan_boxed() { status = s.as_f64() as u16; }
            }
            if let Some(&h) = obj.properties.get("headers") {
                // Copy headers from init object headers field
                if h.is_ptr() {
                    let h_tag = heap_tag(h);
                    if h_tag == 7 {
                        // Copy all entries
                        let src = &*(h.as_ptr() as *const TsMap);
                        let dst = &mut *(headers_val.as_ptr() as *mut TsMap);
                        for (k, v) in &src.entries {
                            ts_retain_val(*k);
                            ts_retain_val(*v);
                            dst.entries.push((*k, *v));
                        }
                    } else if h_tag == 0 {
                        // TsObject: copy string properties
                        let h_obj = &*(h.as_ptr() as *const TsObject);
                        for (k, v) in &h_obj.properties {
                            if k.starts_with("__") { continue; }
                            let k_val = rust_str_to_val(k.clone());
                            ts_map_set(headers_val, k_val, *v);
                            ts_release_val(k_val);
                        }
                    }
                }
            }
        } else if tag == 8 {
            // Clone from another Response: copy status and headers
            let src_resp = &*(init.as_ptr() as *const TsResponse);
            status = src_resp.status;
            // Copy headers
            if src_resp.headers.is_ptr() && heap_tag(src_resp.headers) == 7 {
                let src_map = &*(src_resp.headers.as_ptr() as *const TsMap);
                let dst_map = &mut *(headers_val.as_ptr() as *mut TsMap);
                for (k, v) in &src_map.entries {
                    ts_retain_val(*k);
                    ts_retain_val(*v);
                    dst_map.entries.push((*k, *v));
                }
            }
        }
    }

    // Retain body
    ts_retain_val(body);

    std::ptr::write(ptr, TsResponse { status, body, headers: headers_val });
    TsVal::from_ptr(ptr as *mut u8)
}

/// `response.status` — return HTTP status code as integer.
/// For TsObject (e.g. Request-like) falls back to the "status" property.
#[no_mangle]
pub unsafe extern "C" fn ts_response_status(resp_val: TsVal) -> TsVal {
    if !resp_val.is_ptr() { return TsVal::from_i32(0); }
    match heap_tag(resp_val) {
        8 => {
            let resp = &*(resp_val.as_ptr() as *const TsResponse);
            TsVal::from_i32(resp.status as i32)
        }
        0 => ts_obj_get(resp_val, b"status\0".as_ptr() as *const i8),
        _ => TsVal::from_i32(0),
    }
}

/// `response.ok` — return true if status is 200-299.
/// For TsObject falls back to reading the "ok" property.
#[no_mangle]
pub unsafe extern "C" fn ts_response_ok(resp_val: TsVal) -> TsVal {
    if !resp_val.is_ptr() { return TsVal::from_bool(false); }
    match heap_tag(resp_val) {
        8 => {
            let resp = &*(resp_val.as_ptr() as *const TsResponse);
            TsVal::from_bool(resp.status >= 200 && resp.status < 300)
        }
        0 => ts_obj_get(resp_val, b"ok\0".as_ptr() as *const i8),
        _ => TsVal::from_bool(false),
    }
}

/// `response.headers` — return the Headers object (retained).
/// For TsObject (e.g. Request) falls back to the "headers" property.
#[no_mangle]
pub unsafe extern "C" fn ts_response_headers(resp_val: TsVal) -> TsVal {
    if !resp_val.is_ptr() { return NULL; }
    match heap_tag(resp_val) {
        8 => {
            let resp = &*(resp_val.as_ptr() as *const TsResponse);
            ts_retain_val(resp.headers);
            resp.headers
        }
        0 => ts_obj_get(resp_val, b"headers\0".as_ptr() as *const i8),
        _ => NULL,
    }
}

/// `response.clone()` — clone a Response.
#[no_mangle]
pub unsafe extern "C" fn ts_response_clone(resp_val: TsVal) -> TsVal {
    if !resp_val.is_ptr() || heap_tag(resp_val) != 8 { return resp_val; }
    let resp = &*(resp_val.as_ptr() as *const TsResponse);
    ts_retain_val(resp.body);
    ts_response_new(resp.body, resp_val)
}

// ── Web API: Request ──────────────────────────────────────────────────────────

/// Build a TsVal Request object from raw parts (used by the hyper server).
pub(super) unsafe fn build_ts_request_from_parts(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body_bytes: bytes::Bytes,
) -> TsVal {
    let url_val = rust_str_to_val(uri.to_string());

    // Build TsHeaders (tag=7).
    let ts_hdrs = ts_headers_new(UNDEFINED);
    for (k, v) in headers {
        let kv = rust_str_to_val(k.clone());
        let vv = rust_str_to_val(v.clone());
        ts_map_set(ts_hdrs, kv, vv);
        ts_release_val(kv);
        ts_release_val(vv);
    }

    // Build init object: { method, headers, body }.
    let init = ts_obj_new();
    let method_val = rust_str_to_val(method.to_string());
    ts_obj_set(init, b"method\0".as_ptr() as *const i8, method_val);
    ts_release_val(method_val);
    ts_obj_set(init, b"headers\0".as_ptr() as *const i8, ts_hdrs);
    ts_release_val(ts_hdrs);
    if body_bytes.is_empty() {
        ts_obj_set(init, b"body\0".as_ptr() as *const i8, NULL);
    } else {
        let body_val = rust_str_to_val(String::from_utf8_lossy(&body_bytes).into_owned());
        ts_obj_set(init, b"body\0".as_ptr() as *const i8, body_val);
        ts_release_val(body_val);
    }

    let req = ts_request_new(url_val, init);
    ts_release_val(url_val);
    ts_release_val(init);
    req
}

/// Convert a TsVal Response (tag=8) into a hyper Response.
pub(super) unsafe fn ts_response_to_hyper(
    resp_val: TsVal,
) -> hyper::Response<http_body_util::Full<bytes::Bytes>> {
    use http_body_util::Full;

    if !resp_val.is_ptr() || heap_tag(resp_val) != 8 {
        ts_release_val(resp_val);
        return hyper::Response::builder()
            .status(500)
            .body(Full::new(bytes::Bytes::from("Internal Server Error")))
            .unwrap();
    }

    let resp = &*(resp_val.as_ptr() as *const TsResponse);
    let status = resp.status;

    // Body string.
    let body_bytes = if resp.body.is_ptr() && heap_tag(resp.body) == 2 {
        let ts_str = &*(resp.body.as_ptr() as *const TsString);
        bytes::Bytes::from(ts_str.inner.clone())
    } else {
        bytes::Bytes::new()
    };

    // Headers.
    let mut builder = hyper::Response::builder().status(status);
    if resp.headers.is_ptr() && heap_tag(resp.headers) == 7 {
        let map = &*(resp.headers.as_ptr() as *const TsMap);
        for (k, v) in &map.entries {
            if let (Some(ks), Some(vs)) = (str_val_to_rust(*k), str_val_to_rust(*v)) {
                if let (Ok(hname), Ok(hval)) = (
                    hyper::header::HeaderName::from_bytes(ks.as_bytes()),
                    hyper::header::HeaderValue::from_str(&vs),
                ) {
                    builder = builder.header(hname, hval);
                }
            }
        }
    }

    ts_release_val(resp_val);
    builder.body(Full::new(body_bytes)).unwrap_or_else(|_| {
        hyper::Response::builder()
            .status(500)
            .body(Full::new(bytes::Bytes::new()))
            .unwrap()
    })
}

/// `serve(port, fetchFn)` — start a hyper HTTP/1.1 server.
#[no_mangle]
pub unsafe extern "C" fn ts_serve(port: i32, fetch_fn: TsVal) -> TsVal {
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use http_body_util::BodyExt;
    use tokio::net::TcpListener;

    // Retain fetch_fn so it lives as long as the server.
    ts_retain_val(fetch_fn);
    let fetch_raw = fetch_fn.0; // u64, Copy + Send

    get_runtime().block_on(async move {
        let addr = format!("0.0.0.0:{}", port);
        let listener = TcpListener::bind(&addr)
            .await
            .expect("ts_serve: failed to bind port");

        eprintln!("Listening on http://{}", addr);

        loop {
            let (stream, _peer) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => { eprintln!("ts_serve: accept error: {e}"); continue; }
            };
            let io = TokioIo::new(stream);

            tokio::task::spawn(async move {
                let svc = service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let fetch_raw = fetch_raw;
                    async move {
                        // Collect request parts.
                        let method  = req.method().as_str().to_string();
                        let path    = req.uri().to_string();
                        let headers: Vec<(String, String)> = req.headers().iter()
                            .map(|(k, v)| (
                                k.as_str().to_string(),
                                v.to_str().unwrap_or("").to_string(),
                            ))
                            .collect();
                        // Build a full URL so `new URL(req.url)` works in user code.
                        let host = headers.iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("host"))
                            .map(|(_, v)| v.as_str())
                            .unwrap_or("localhost");
                        let uri = if path.starts_with('/') {
                            format!("http://{}{}", host, path)
                        } else {
                            path
                        };
                        let body_bytes = req.into_body()
                            .collect().await
                            .map(|c| c.to_bytes())
                            .unwrap_or_default();

                        // Call TS handler on a blocking thread (not async-safe).
                        let hyper_resp = get_runtime()
                            .spawn_blocking(move || unsafe {
                                let fetch_fn = TsVal(fetch_raw);
                                let ts_req = build_ts_request_from_parts(
                                    &method, &uri, &headers, body_bytes,
                                );
                                // Call fetch(request).
                                let result = dispatch_callback(fetch_fn, &[ts_req]);
                                ts_release_val(ts_req);
                                // Await if Promise (consumes result, returns owned resolved value).
                                let resolved = ts_promise_await(result);
                                ts_response_to_hyper(resolved)
                            })
                            .await
                            .unwrap_or_else(|_| {
                                hyper::Response::builder()
                                    .status(500)
                                    .body(http_body_util::Full::new(bytes::Bytes::new()))
                                    .unwrap()
                            });

                        Ok::<_, std::convert::Infallible>(hyper_resp)
                    }
                });

                if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                    eprintln!("ts_serve: connection error: {e}");
                }
            });
        }
    });

    UNDEFINED
}

// ── addEventListener / serve(port) ───────────────────────────────────────────

static FETCH_LISTENER: AtomicU64 = AtomicU64::new(0x7FF8_0000_0000_0000); // UNDEFINED bits

/// `addEventListener('fetch', handler)` — register a global fetch handler.
#[no_mangle]
pub unsafe extern "C" fn ts_add_event_listener(event: TsVal, handler: TsVal) -> TsVal {
    let event_str = ts_val_to_rust_string(event);
    if event_str == "fetch" {
        let old = FETCH_LISTENER.swap(handler.0, Ordering::SeqCst);
        let old_val = TsVal(old);
        ts_retain_val(handler);
        ts_release_val(old_val);
    }
    UNDEFINED
}

/// `removeEventListener('fetch', handler)` — no-op (we keep the handler until replaced).
#[no_mangle]
pub unsafe extern "C" fn ts_remove_event_listener(_event: TsVal, _handler: TsVal) -> TsVal {
    UNDEFINED
}

/// `serve(port)` — start HTTP server using the registered fetch handler.
#[no_mangle]
pub unsafe extern "C" fn ts_serve_worker(port: i32) -> TsVal {
    let raw = FETCH_LISTENER.load(Ordering::SeqCst);
    let fetch_fn = TsVal(raw);
    if !fetch_fn.is_ptr() || heap_tag(fetch_fn) != 4 {
        eprintln!("ts_serve_worker: no fetch listener registered");
        return UNDEFINED;
    }
    ts_retain_val(fetch_fn);
    ts_serve(port, fetch_fn)
}

/// `new Request(url, init?)` — create a Request-like TsObject with url/method/headers/body.
#[no_mangle]
pub unsafe extern "C" fn ts_request_new(url: TsVal, init: TsVal) -> TsVal {
    let obj = ts_obj_new();
    let url_key = b"url\0";
    let method_key = b"method\0";
    let headers_key = b"headers\0";
    let body_key = b"body\0";
    ts_obj_set(obj, url_key.as_ptr() as *const i8, url);
    // Extract method/headers/body from init if it's an object
    if init.is_ptr() && heap_tag(init) == 0 {
        let method = ts_obj_get(init, method_key.as_ptr() as *const i8);
        let headers = ts_obj_get(init, headers_key.as_ptr() as *const i8);
        let body = ts_obj_get(init, body_key.as_ptr() as *const i8);
        ts_obj_set(obj, method_key.as_ptr() as *const i8, method);
        ts_obj_set(obj, headers_key.as_ptr() as *const i8, headers);
        ts_obj_set(obj, body_key.as_ptr() as *const i8, body);
        ts_release_val(method);
        ts_release_val(headers);
        ts_release_val(body);
    } else {
        let get_str = ts_string_new(b"GET\0".as_ptr() as *const i8);
        ts_obj_set(obj, method_key.as_ptr() as *const i8, get_str);
        ts_release_val(get_str);
    }
    obj
}

// ── Request / Response body methods ─────────────────────────────────────────

/// `request.text()` or `response.text()` — return body as Promise<string>.
#[no_mangle]
pub unsafe extern "C" fn ts_val_text(val: TsVal) -> TsVal {
    let body = if val.is_ptr() {
        match heap_tag(val) {
            0 => ts_obj_get(val, b"body\0".as_ptr() as *const i8),
            8 => {
                let resp = &*(val.as_ptr() as *const TsResponse);
                ts_retain_val(resp.body);
                resp.body
            }
            _ => UNDEFINED,
        }
    } else {
        UNDEFINED
    };
    // Coerce to string if not already
    let body_str = if body.is_ptr() && heap_tag(body) == 2 {
        ts_retain_val(body);
        body
    } else if body.is_null() || body.is_undefined() {
        rust_str_to_val(String::new())
    } else {
        ts_val_to_string(body)
    };
    ts_release_val(body);
    let p = ts_promise_resolve(body_str);
    ts_release_val(body_str);
    p
}

/// `request.json()` or `response.json()` — return body parsed as Promise<any>.
#[no_mangle]
pub unsafe extern "C" fn ts_val_json(val: TsVal) -> TsVal {
    let body = if val.is_ptr() {
        match heap_tag(val) {
            0 => ts_obj_get(val, b"body\0".as_ptr() as *const i8),
            8 => {
                let resp = &*(val.as_ptr() as *const TsResponse);
                ts_retain_val(resp.body);
                resp.body
            }
            _ => UNDEFINED,
        }
    } else {
        UNDEFINED
    };
    let parsed = ts_json_parse(body);
    ts_release_val(body);
    let p = ts_promise_resolve(parsed);
    ts_release_val(parsed);
    p
}

// ── Global fetch() ───────────────────────────────────────────────────────────

/// `fetch(url, init?)` — perform an HTTP request and return Promise<Response>.
/// `url` may be a string or a Request object.
/// `init` may be an object with `method`, `headers`, `body` fields.
#[no_mangle]
pub unsafe extern "C" fn ts_fetch(url: TsVal, init: TsVal) -> TsVal {
    // Extract URL string.
    let url_str = if url.is_ptr() {
        match heap_tag(url) {
            2 => {
                let ts_str = &*(url.as_ptr() as *const TsString);
                ts_str.inner.clone()
            }
            0 => {
                // Request object: read .url property
                let url_prop = ts_obj_get(url, b"url\0".as_ptr() as *const i8);
                let s = ts_val_to_rust_string(url_prop);
                ts_release_val(url_prop);
                s
            }
            _ => String::new(),
        }
    } else {
        String::new()
    };

    if url_str.is_empty() {
        let err = rust_str_to_val("fetch: invalid URL".to_string());
        let p = ts_promise_resolve(err);
        ts_release_val(err);
        return p;
    }

    // Extract method, headers, body from init or from the Request object.
    let mut method = String::from("GET");
    let mut req_headers: Vec<(String, String)> = Vec::new();
    let mut body_str: Option<String> = None;

    // If url is a Request object (tag=0), extract its fields first.
    if url.is_ptr() && heap_tag(url) == 0 {
        let method_val = ts_obj_get(url, b"method\0".as_ptr() as *const i8);
        if !method_val.is_undefined() { method = ts_val_to_rust_string(method_val); }
        ts_release_val(method_val);
        let headers_val = ts_obj_get(url, b"headers\0".as_ptr() as *const i8);
        if headers_val.is_ptr() && heap_tag(headers_val) == 7 {
            let map = &*(headers_val.as_ptr() as *const TsMap);
            for (k, v) in &map.entries {
                if let (Some(ks), Some(vs)) = (str_val_to_rust(*k), str_val_to_rust(*v)) {
                    req_headers.push((ks, vs));
                }
            }
        }
        ts_release_val(headers_val);
        let body_val = ts_obj_get(url, b"body\0".as_ptr() as *const i8);
        if !body_val.is_undefined() && !body_val.is_null() {
            body_str = Some(ts_val_to_rust_string(body_val));
        }
        ts_release_val(body_val);
    }

    // Apply init overrides.
    if init.is_ptr() && heap_tag(init) == 0 {
        let method_val = ts_obj_get(init, b"method\0".as_ptr() as *const i8);
        if !method_val.is_undefined() { method = ts_val_to_rust_string(method_val); }
        ts_release_val(method_val);
        let headers_val = ts_obj_get(init, b"headers\0".as_ptr() as *const i8);
        if headers_val.is_ptr() {
            match heap_tag(headers_val) {
                7 => {
                    let map = &*(headers_val.as_ptr() as *const TsMap);
                    for (k, v) in &map.entries {
                        if let (Some(ks), Some(vs)) = (str_val_to_rust(*k), str_val_to_rust(*v)) {
                            req_headers.push((ks, vs));
                        }
                    }
                }
                0 => {
                    let obj = &*(headers_val.as_ptr() as *const TsObject);
                    for (k, v) in &obj.properties {
                        if k.starts_with("__") { continue; }
                        if let Some(vs) = str_val_to_rust(*v) {
                            req_headers.push((k.clone(), vs));
                        }
                    }
                }
                _ => {}
            }
        }
        ts_release_val(headers_val);
        let body_val = ts_obj_get(init, b"body\0".as_ptr() as *const i8);
        if !body_val.is_undefined() && !body_val.is_null() {
            body_str = Some(ts_val_to_rust_string(body_val));
        }
        ts_release_val(body_val);
    }

    // Perform the request asynchronously using reqwest via the Tokio runtime.
    let (resolved, notify) = make_promise_pair();
    let r2 = resolved.clone();
    let n2 = notify.clone();

    get_runtime().spawn(async move {
        let client = reqwest::Client::new();
        let req_method = reqwest::Method::from_bytes(method.as_bytes())
            .unwrap_or(reqwest::Method::GET);
        let mut builder = client.request(req_method, &url_str);
        for (k, v) in &req_headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        if let Some(body) = body_str {
            builder = builder.body(body);
        }
        let result = builder.send().await;
        unsafe {
            let ts_resp = match result {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let headers: Vec<(String, String)> = resp.headers().iter()
                        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect();
                    let body_bytes = resp.bytes().await.unwrap_or_default();
                    let body_str = String::from_utf8_lossy(&body_bytes).into_owned();
                    let body_val = rust_str_to_val(body_str);

                    // Build headers object.
                    let ts_hdrs = ts_headers_new(UNDEFINED);
                    for (k, v) in &headers {
                        let kv = rust_str_to_val(k.clone());
                        let vv = rust_str_to_val(v.clone());
                        ts_map_set(ts_hdrs, kv, vv);
                        ts_release_val(kv);
                        ts_release_val(vv);
                    }

                    // Build init object for ts_response_new.
                    let init_obj = ts_obj_new();
                    let status_val = TsVal::from_i32(status as i32);
                    ts_obj_set(init_obj, b"status\0".as_ptr() as *const i8, status_val);
                    ts_obj_set(init_obj, b"headers\0".as_ptr() as *const i8, ts_hdrs);
                    ts_release_val(ts_hdrs);

                    let resp_val = ts_response_new(body_val, init_obj);
                    ts_release_val(body_val);
                    ts_release_val(init_obj);
                    resp_val
                }
                Err(e) => {
                    let err_str = format!("fetch error: {}", e);
                    rust_str_to_val(err_str)
                }
            };
            ts_retain_val(ts_resp);
            resolve_arc(&r2, &n2, ts_resp);
            ts_release_val(ts_resp);
        }
    });

    alloc_promise(TsPromise { resolved, notify })
}

// ── Helper ────────────────────────────────────────────────────────────────────

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
