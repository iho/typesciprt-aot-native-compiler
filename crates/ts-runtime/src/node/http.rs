//! Node.js `http` module — HTTP server and outbound requests.
//!
//! `ts_http_server_listen(port, handler)` starts a Hyper HTTP/1.1 server.
//! The handler is called as `handler(req, res)` for each incoming request where:
//!
//!   req — TsObject { method, url, headers: TsObject, body: string }
//!   res — TsObject { __status: 200, __headers: TsObject, __body: "" }
//!
//! After the handler returns (or its Promise resolves), the res fields are
//! read back and converted to a hyper::Response.

use crate::value::{
    TsVal, TsObject, UNDEFINED, ts_retain_val, ts_release_val, heap_tag,
    ts_obj_new, ts_obj_get, ts_obj_set, ts_obj_set_val_key,
    ts_func_call2,
};
use crate::value::promise::{get_runtime, ts_promise_await, acquire_js_lock, release_js_lock};
use super::{new_string, val_to_string, val_to_i32};

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use http_body_util::BodyExt;
use tokio::net::TcpListener;

/// Start a Node.js-style HTTP server on `port`, calling `handler(req, res)` for each request.
///
/// Blocks the calling thread running the server loop (like `ts_serve`).
#[no_mangle]
pub unsafe extern "C" fn ts_http_server_listen(port: TsVal, handler: TsVal) -> TsVal {
    let port_num = val_to_i32(port).max(0) as u16;
    ts_retain_val(handler);
    let handler_raw = handler.0; // Copy as u64 for Send across threads.

    // Release the JS execution lock before entering the blocking server loop.
    release_js_lock();

    get_runtime().block_on(async move {
        let addr = format!("0.0.0.0:{}", port_num);
        let listener = TcpListener::bind(&addr)
            .await
            .expect("ts_http_server_listen: failed to bind port");

        eprintln!("Listening on http://{}", addr);

        loop {
            let (stream, _peer) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => { eprintln!("http accept error: {e}"); continue; }
            };
            let io = TokioIo::new(stream);

            tokio::task::spawn(async move {
                let svc = service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    async move {
                        let method  = req.method().as_str().to_string();
                        let uri     = req.uri().to_string();
                        let headers: Vec<(String, String)> = req.headers().iter()
                            .map(|(k, v)| (
                                k.as_str().to_string(),
                                v.to_str().unwrap_or("").to_string(),
                            ))
                            .collect();
                        let body_bytes = req.into_body().collect().await
                            .map(|c| c.to_bytes())
                            .unwrap_or_default();
                        let body_str = String::from_utf8_lossy(&body_bytes).into_owned();

                        let resp_data = get_runtime()
                            .spawn_blocking(move || unsafe {
                                acquire_js_lock();
                                let handler = TsVal(handler_raw);

                                // --- Build req TsObject ---
                                let req_obj = ts_obj_new();
                                set_str_prop(req_obj, b"method\0", &method);
                                set_str_prop(req_obj, b"url\0", &uri);
                                set_str_prop(req_obj, b"body\0", &body_str);

                                // req.headers = TsObject { "header-name": "value" }
                                let hdrs_obj = ts_obj_new();
                                for (k, v) in &headers {
                                    let kv = new_string(k);
                                    let vv = new_string(v);
                                    ts_obj_set_val_key(hdrs_obj, kv, vv);
                                    ts_release_val(kv);
                                    ts_release_val(vv);
                                }
                                ts_obj_set(req_obj, b"headers\0".as_ptr() as *const i8, hdrs_obj);
                                ts_release_val(hdrs_obj);

                                // --- Build res TsObject ---
                                let res_obj = ts_obj_new();
                                ts_obj_set(res_obj, b"__status\0".as_ptr() as *const i8, TsVal::from_i32(200));
                                let res_hdrs = ts_obj_new();
                                ts_obj_set(res_obj, b"__headers\0".as_ptr() as *const i8, res_hdrs);
                                ts_release_val(res_hdrs);
                                set_str_prop(res_obj, b"__body\0", "");

                                // --- Call handler(req, res) ---
                                let result = ts_func_call2(handler, req_obj, res_obj);
                                // Await if the handler returned a Promise.
                                let resolved = ts_promise_await(result);
                                ts_release_val(resolved);

                                // --- Read back response state ---
                                let status = {
                                    let s = ts_obj_get(res_obj, b"__status\0".as_ptr() as *const i8);
                                    let v = (val_to_i32(s) as u16).max(100);
                                    ts_release_val(s);
                                    v
                                };
                                let body = {
                                    let b = ts_obj_get(res_obj, b"__body\0".as_ptr() as *const i8);
                                    let s = val_to_string(b).unwrap_or_default();
                                    ts_release_val(b);
                                    s
                                };
                                let resp_headers: Vec<(String, String)> = {
                                    let h = ts_obj_get(res_obj, b"__headers\0".as_ptr() as *const i8);
                                    let pairs = if h.is_ptr() && heap_tag(h) == 0 {
                                        let obj = &*(h.as_ptr() as *const TsObject);
                                        obj.properties.iter()
                                            .filter(|(k, _)| !k.starts_with("__"))
                                            .filter_map(|(k, &v)| val_to_string(v).map(|vs| (k.clone(), vs)))
                                            .collect()
                                    } else {
                                        vec![]
                                    };
                                    ts_release_val(h);
                                    pairs
                                };

                                ts_release_val(req_obj);
                                ts_release_val(res_obj);

                                let result = (status, body, resp_headers);
                                release_js_lock();
                                result
                            })
                            .await
                            .unwrap_or((500, String::new(), vec![]));

                        let (status, body, resp_headers) = resp_data;

                        let mut builder = hyper::Response::builder().status(status);
                        for (k, v) in &resp_headers {
                            if let (Ok(hn), Ok(hv)) = (
                                hyper::header::HeaderName::from_bytes(k.as_bytes()),
                                hyper::header::HeaderValue::from_str(v),
                            ) {
                                builder = builder.header(hn, hv);
                            }
                        }

                        Ok::<_, std::convert::Infallible>(
                            builder
                                .body(http_body_util::Full::new(bytes::Bytes::from(body)))
                                .unwrap_or_else(|_| {
                                    hyper::Response::builder()
                                        .status(500)
                                        .body(http_body_util::Full::new(bytes::Bytes::new()))
                                        .unwrap()
                                })
                        )
                    }
                });

                if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                    eprintln!("http connection error: {e}");
                }
            });
        }
    });

    UNDEFINED
}

/// Helper: set a string property on a TsObject using a NUL-terminated byte-string key.
unsafe fn set_str_prop(obj: TsVal, key: &[u8], val: &str) {
    let v = new_string(val);
    ts_obj_set(obj, key.as_ptr() as *const i8, v);
    ts_release_val(v);
}
