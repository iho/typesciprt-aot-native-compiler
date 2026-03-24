//! Node.js `http` module — HTTP server and outbound requests.
//!
//! `ts_http_server_listen(port, handler)` starts a Hyper HTTP/1.1 server.
//!
//! Set `HTTP_WORKERS=N` to distribute connections across N OS threads, each
//! with its own SO_REUSEPORT listener and tokio LocalSet.  The kernel
//! load-balances incoming connections across workers.  Defaults to the number
//! of available CPU cores.
//!
//! **Thread-safety note**: with N>1 workers, module-level globals are shared
//! across workers.  `MODULE_GLOBALS` is protected by an `RwLock` so concurrent
//! reads are safe.  Request handlers must not mutate shared TsObjects/TsArrays
//! (e.g. in-memory caches); DB-backed stateless handlers are safe.

use crate::value::{
    TsVal, TsObject, UNDEFINED, ts_retain_val, ts_release_val, heap_tag,
    ts_obj_new, ts_obj_new_arena, ts_obj_get, ts_obj_set, ts_obj_set_val_key,
    ts_func_call2,
};
use crate::value::http::{ts_node_request_new, bind_reuseport};
use crate::value::promise::{get_runtime, ts_promise_await, release_js_lock};
use crate::value::fiber::{JsFiber, register_local_set_tx};
use super::{new_string, val_to_string, val_to_i32};

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use http_body_util::BodyExt;
use tokio::net::TcpListener;

/// Core accept loop for node-style HTTP.  Drives request fibers and also
/// drains the fiber channel so promise callbacks (`.then`, timers) run
/// cooperatively on this LocalSet without acquiring the global JS lock.
async fn node_accept_loop(
    listener: TcpListener,
    handler_raw: u64,
    fiber_rx: Option<tokio::sync::mpsc::UnboundedReceiver<JsFiber>>,
) {
    // Spawn a local task that forwards fiber channel entries onto this LocalSet.
    if let Some(mut rx) = fiber_rx {
        tokio::task::spawn_local(async move {
            while let Some(fiber) = rx.recv().await {
                tokio::task::spawn_local(fiber.run());
            }
        });
    }

    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => { eprintln!("http accept error: {e}"); continue; }
        };
        let io = TokioIo::new(stream);

        // Each connection is a local task (single-threaded per worker, no lock needed).
        tokio::task::spawn_local(async move {
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

                    // Run the JS handler in a fiber — non-blocking awaits.
                    let fiber = JsFiber::new(move || unsafe {
                        let handler = TsVal(handler_raw);

                        // Build req as TsRequest (tag=19) — fixed-field struct, no HashMap.
                        // Use arena allocation: retain/release become no-ops, freed by arena_exit.
                        let hdrs_obj = ts_obj_new_arena();
                        for (k, v) in &headers {
                            let kv = new_string(k);
                            let vv = new_string(v);
                            ts_obj_set_val_key(hdrs_obj, kv, vv);
                            ts_release_val(kv);
                            ts_release_val(vv);
                        }
                        // Transfer ownership of all 4 values into TsRequest
                        let url_val    = new_string(&uri);
                        let method_val = new_string(&method);
                        let body_val   = new_string(&body_str);
                        let req_obj = ts_node_request_new(url_val, method_val, hdrs_obj, body_val);

                        // Build res TsObject — arena allocated, freed by arena_exit.
                        let res_obj = ts_obj_new_arena();
                        ts_obj_set(res_obj, b"__status\0".as_ptr() as *const i8, TsVal::from_i32(200));
                        let res_hdrs = ts_obj_new_arena();
                        ts_obj_set(res_obj, b"__headers\0".as_ptr() as *const i8, res_hdrs);
                        set_str_prop(res_obj, b"__body\0", "");

                        // Call handler(req, res) — fiber yields at each await
                        let result = ts_func_call2(handler, req_obj, res_obj);
                        let resolved = ts_promise_await(result);
                        ts_release_val(resolved);

                        // Pack response into a Box and leak it as a raw pointer.
                        // The async block below will reclaim it.
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

                        let boxed = Box::new((status, body, resp_headers));
                        Box::into_raw(boxed) as u64
                    });

                    // Drive the fiber to completion (yields at each ts_promise_await).
                    let raw_ptr = fiber.run().await;
                    let (status, body, resp_headers) = unsafe {
                        *Box::from_raw(raw_ptr as *mut (u16, String, Vec<(String, String)>))
                    };

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
                let msg = e.to_string();
                if !msg.contains("connection closed before message") {
                    eprintln!("http connection error: {e}");
                }
            }
        });
    }
}

/// Start a Node.js-style HTTP server on `port`, calling `handler(req, res)` for each request.
///
/// Each request handler runs in a JsFiber on a tokio::task::LocalSet — cooperative
/// concurrency with no global JS lock.  Set `HTTP_WORKERS=N` to use N worker threads.
#[no_mangle]
pub unsafe extern "C" fn ts_http_server_listen(port: TsVal, handler: TsVal) -> TsVal {
    let port_num = val_to_i32(port).max(0) as u16;
    let nworkers: usize = std::env::var("HTTP_WORKERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))
        .max(1);

    // Retain handler once per worker — each worker holds a logical reference.
    ts_retain_val(handler);
    for _ in 1..nworkers { ts_retain_val(handler); }
    let handler_raw = handler.0;

    // Release the JS execution lock before entering the fiber-based event loop.
    release_js_lock();

    // Spawn extra workers (2..N).  Each gets its own SO_REUSEPORT listener,
    // its own LocalSet, and its own fiber channel for promise callbacks.
    for _ in 1..nworkers {
        std::thread::spawn(move || {
            let std_listener = unsafe { bind_reuseport(port_num) }
                .expect("http worker: failed to bind port");
            let (fiber_tx, fiber_rx) =
                tokio::sync::mpsc::unbounded_channel::<JsFiber>();
            register_local_set_tx(fiber_tx);
            let local = tokio::task::LocalSet::new();
            get_runtime().block_on(local.run_until(async move {
                let listener = TcpListener::from_std(std_listener)
                    .expect("http worker: from_std failed");
                node_accept_loop(listener, handler_raw, Some(fiber_rx)).await;
            }));
        });
    }

    // Main worker: bind its own SO_REUSEPORT listener and carry the fiber channel.
    let std_listener = bind_reuseport(port_num)
        .expect("ts_http_server_listen: failed to bind port");

    // Register the main thread's fiber channel.
    let (fiber_tx, fiber_rx) =
        tokio::sync::mpsc::unbounded_channel::<JsFiber>();
    register_local_set_tx(fiber_tx);

    eprintln!("Listening on http://0.0.0.0:{} ({} worker{})",
        port_num, nworkers, if nworkers == 1 { "" } else { "s" });

    let local = tokio::task::LocalSet::new();
    get_runtime().block_on(local.run_until(async move {
        let listener = TcpListener::from_std(std_listener)
            .expect("ts_http_server_listen: from_std failed");
        node_accept_loop(listener, handler_raw, Some(fiber_rx)).await;
    }));

    UNDEFINED
}

unsafe fn set_str_prop(obj: TsVal, key: &[u8], val: &str) {
    let v = new_string(val);
    ts_obj_set(obj, key.as_ptr() as *const i8, v);
    ts_release_val(v);
}
