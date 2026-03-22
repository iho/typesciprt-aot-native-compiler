//! Node.js `net` module — TCP server and streaming client sockets.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, AtomicI32, Ordering}};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::value::{
    TsVal, UNDEFINED, NULL, ts_retain_val, ts_release_val,
    ts_obj_new, ts_obj_set,
    ts_func_call1, ts_func_call2,
};
use crate::value::promise::{get_runtime, acquire_js_lock, release_js_lock, make_promise_pair, alloc_promise, resolve_arc};
use crate::value::TsPromise;
use super::{new_string, val_to_i32, val_to_string};

// ── Socket registry ───────────────────────────────────────────────────────────

enum WriteMsg { Data(Vec<u8>), End, Destroy }

/// Lock-free data queue for pull-mode sockets.
/// Data arriving from the network is pushed here without acquiring the JS lock.
/// `ts_socket_read_chunk` blocks on this queue and resolves a Promise when data arrives.
struct DataQueue {
    chunks: Mutex<VecDeque<Vec<u8>>>,
    cvar: std::sync::Condvar,
    eof: AtomicBool,
}

impl DataQueue {
    fn new() -> Arc<Self> {
        Arc::new(DataQueue {
            chunks: Mutex::new(VecDeque::new()),
            cvar: std::sync::Condvar::new(),
            eof: AtomicBool::new(false),
        })
    }

    fn push_data(&self, data: Vec<u8>) {
        self.chunks.lock().unwrap().push_back(data);
        self.cvar.notify_one();
    }

    fn push_eof(&self) {
        self.eof.store(true, Ordering::Release);
        self.cvar.notify_all();
    }

    /// Block until a chunk is available; returns None on EOF/error.
    fn blocking_pop(&self) -> Option<Vec<u8>> {
        if self.eof.load(Ordering::Acquire) { return None; }
        let mut guard = self.chunks.lock().unwrap();
        loop {
            if let Some(chunk) = guard.pop_front() { return Some(chunk); }
            if self.eof.load(Ordering::Acquire) { return None; }
            guard = self.cvar.wait(guard).unwrap();
        }
    }
}

struct SocketEntry {
    write_tx: UnboundedSender<WriteMsg>,
    data_queue: Arc<DataQueue>,
    pull_mode: Arc<AtomicBool>,
}

static SOCKET_REGISTRY: OnceLock<Mutex<HashMap<i32, SocketEntry>>> = OnceLock::new();
static NEXT_SOCKET_ID: AtomicI32 = AtomicI32::new(1);

fn socket_registry() -> &'static Mutex<HashMap<i32, SocketEntry>> {
    SOCKET_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

// ── Streaming TCP client socket ───────────────────────────────────────────────

/// Connect to `host:port`, fire events via `emit_fn(event, arg?)`.
///
/// Events emitted (push mode only):
///   emit_fn("connect")         — TCP handshake complete
///   emit_fn("data",   Buffer)  — chunk received (push mode)
///   emit_fn("end")             — server closed write side (FIN)
///   emit_fn("error",  String)  — connection / I/O error
///   emit_fn("close",  Bool)    — socket fully closed (hadError=true/false)
///
/// In pull mode (after ts_socket_set_pull_mode), data events are suppressed and
/// data is delivered via ts_socket_read_chunk instead.
///
/// Returns a socket handle ID (i32 TsVal).
#[no_mangle]
pub unsafe extern "C" fn ts_net_socket_connect(
    host: TsVal, port: TsVal, emit_fn: TsVal,
) -> TsVal {
    let host_str = val_to_string(host).unwrap_or_else(|| "localhost".to_string());
    let port_num = val_to_i32(port).max(0) as u16;

    ts_retain_val(emit_fn);
    let emit_raw = emit_fn.0;

    let id = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);

    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<WriteMsg>();
    let data_queue = DataQueue::new();
    let pull_mode = Arc::new(AtomicBool::new(false));

    socket_registry().lock().unwrap().insert(id, SocketEntry {
        write_tx,
        data_queue: data_queue.clone(),
        pull_mode: pull_mode.clone(),
    });

    // Channel from async tasks → blocking event processor (control events only in pull mode).
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SocketEvent>();
    let event_tx_err = event_tx.clone();

    get_runtime().spawn(async move {
        let addr = format!("{}:{}", host_str, port_num);
        let stream = match tokio::net::TcpStream::connect(&addr).await {
            Ok(s) => s,
            Err(e) => {
                data_queue.push_eof();
                let _ = event_tx_err.send(SocketEvent::Error(e.to_string()));
                let _ = event_tx_err.send(SocketEvent::Close(true));
                return;
            }
        };
        let _ = stream.set_nodelay(true);
        let _ = event_tx.send(SocketEvent::Connect);

        let (mut reader, mut writer) = tokio::io::split(stream);

        let event_tx2 = event_tx.clone();

        // Write task: forwards WriteMsg → TCP.
        tokio::spawn(async move {
            loop {
                match write_rx.recv().await {
                    Some(WriteMsg::Data(bytes)) => {
                        if writer.write_all(&bytes).await.is_err() { break; }
                    }
                    Some(WriteMsg::End) => {
                        let _ = writer.shutdown().await;
                        break;
                    }
                    Some(WriteMsg::Destroy) | None => break,
                }
            }
        });

        // Read task: TCP → data_queue (pull mode) or event channel (push mode).
        let mut buf = vec![0u8; 65536];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => {
                    data_queue.push_eof();
                    let _ = event_tx2.send(SocketEvent::End);
                    let _ = event_tx2.send(SocketEvent::Close(false));
                    break;
                }
                Ok(n) => {
                    let bytes = buf[..n].to_vec();
                    if pull_mode.load(Ordering::Acquire) {
                        // Pull mode: push directly to queue, no JS lock involved.
                        data_queue.push_data(bytes);
                    } else {
                        // Push mode: deliver via event processor (acquires JS lock).
                        let _ = event_tx2.send(SocketEvent::Data(bytes));
                    }
                }
                Err(e) => {
                    data_queue.push_eof();
                    let _ = event_tx2.send(SocketEvent::Error(e.to_string()));
                    let _ = event_tx2.send(SocketEvent::Close(true));
                    break;
                }
            }
        }
    });

    // Event processor: runs in a blocking thread, processes control events one at a time.
    // Acquires the JS execution lock around each TS callback call so we don't
    // race with concurrent HTTP handler threads.
    get_runtime().spawn_blocking(move || {
        loop {
            // Wait for the next socket event WITHOUT holding the JS lock.
            let evt = match event_rx.blocking_recv() {
                Some(e) => e,
                None => break,
            };
            // Acquire the JS lock before calling any TS code.
            acquire_js_lock();
            let emit = TsVal(emit_raw);
            let done = unsafe {
                match evt {
                    SocketEvent::Connect => {
                        let ev = new_string("connect");
                        let r = ts_func_call1(emit, ev);
                        ts_release_val(r); ts_release_val(ev);
                        false
                    }
                    SocketEvent::Data(bytes) => {
                        // Push mode: deliver data event to TS callbacks.
                        let ev = new_string("data");
                        let buf = super::buffer::alloc_buffer_pub(bytes);
                        let r = ts_func_call2(emit, ev, buf);
                        ts_release_val(r); ts_release_val(buf); ts_release_val(ev);
                        false
                    }
                    SocketEvent::End => {
                        let ev = new_string("end");
                        let r = ts_func_call1(emit, ev);
                        ts_release_val(r); ts_release_val(ev);
                        false
                    }
                    SocketEvent::Error(msg) => {
                        let ev = new_string("error");
                        let err_str = new_string(&msg);
                        let r = ts_func_call2(emit, ev, err_str);
                        ts_release_val(r); ts_release_val(err_str); ts_release_val(ev);
                        false
                    }
                    SocketEvent::Close(had_error) => {
                        let ev = new_string("close");
                        let r = ts_func_call2(emit, ev, TsVal::from_bool(had_error));
                        ts_release_val(r); ts_release_val(ev);
                        ts_release_val(TsVal(emit_raw));
                        socket_registry().lock().unwrap().remove(&id);
                        true
                    }
                }
            };
            release_js_lock();
            if done { break; }
        }
    });

    TsVal::from_i32(id)
}

enum SocketEvent {
    Connect,
    Data(Vec<u8>),
    End,
    Error(String),
    Close(bool),
}

/// Enable pull mode for a socket: subsequent data is pushed to the internal
/// DataQueue instead of being delivered via the JS emit callback.
/// Call this before sending any data that would cause the remote to respond.
#[no_mangle]
pub unsafe extern "C" fn ts_socket_set_pull_mode(handle_id: TsVal) -> TsVal {
    let id = val_to_i32(handle_id);
    if let Ok(reg) = socket_registry().lock() {
        if let Some(entry) = reg.get(&id) {
            entry.pull_mode.store(true, Ordering::Release);
        }
    }
    UNDEFINED
}

/// Read the next available data chunk from the socket's DataQueue.
/// Returns a Promise<Buffer> that resolves when data arrives, or Promise<null> on EOF.
/// Releases the JS lock while waiting so other concurrent handlers can run.
#[no_mangle]
pub unsafe extern "C" fn ts_socket_read_chunk(handle_id: TsVal) -> TsVal {
    let id = val_to_i32(handle_id);
    let data_queue = {
        let reg = socket_registry().lock().unwrap();
        reg.get(&id).map(|e| e.data_queue.clone())
    };
    let data_queue = match data_queue {
        Some(q) => q,
        None => {
            // Socket gone — return a resolved Promise<null>.
            let (resolved, notify, blocking_notify) = make_promise_pair();
            let _ = resolved.set(NULL);
            return alloc_promise(TsPromise { resolved, notify, blocking_notify });
        }
    };

    let (resolved, notify, blocking_notify) = make_promise_pair();
    let r2 = resolved.clone();
    let n2 = notify.clone();
    let bn2 = blocking_notify.clone();

    get_runtime().spawn_blocking(move || {
        // Block on the data queue WITHOUT holding the JS lock.
        // Data is pushed here directly from the async read task — no lock transitions needed.
        let chunk = data_queue.blocking_pop();
        let val = match chunk {
            Some(bytes) => super::buffer::alloc_buffer_pub(bytes),
            None => NULL,
        };
        resolve_arc(&r2, &n2, &bn2, val);
    });

    alloc_promise(TsPromise { resolved, notify, blocking_notify })
}

/// Write data (Buffer TsVal or String) to the socket identified by `handle_id`.
#[no_mangle]
pub unsafe extern "C" fn ts_net_socket_write(handle_id: TsVal, data: TsVal) -> TsVal {
    let id = val_to_i32(handle_id);
    let bytes = tsval_to_bytes(data);
    if let Some(bytes) = bytes {
        if let Ok(reg) = socket_registry().lock() {
            if let Some(entry) = reg.get(&id) {
                let _ = entry.write_tx.send(WriteMsg::Data(bytes));
            }
        }
    }
    TsVal::from_bool(true)
}

/// Gracefully close the write side of the socket (sends FIN).
#[no_mangle]
pub unsafe extern "C" fn ts_net_socket_end(handle_id: TsVal) -> TsVal {
    let id = val_to_i32(handle_id);
    if let Ok(reg) = socket_registry().lock() {
        if let Some(entry) = reg.get(&id) {
            let _ = entry.write_tx.send(WriteMsg::End);
        }
    }
    UNDEFINED
}

/// Destroy the socket immediately.
#[no_mangle]
pub unsafe extern "C" fn ts_net_socket_destroy(handle_id: TsVal) -> TsVal {
    let id = val_to_i32(handle_id);
    if let Ok(mut reg) = socket_registry().lock() {
        if let Some(entry) = reg.remove(&id) {
            let _ = entry.write_tx.send(WriteMsg::Destroy);
        }
    }
    UNDEFINED
}

/// Set TCP_NODELAY on the socket (best-effort, no-op if handle is gone).
#[no_mangle]
pub unsafe extern "C" fn ts_net_socket_set_nodelay(_handle_id: TsVal, _enable: TsVal) -> TsVal {
    // TCP_NODELAY is always set at connect time; this call is a no-op.
    UNDEFINED
}

/// Set TCP keep-alive (best-effort stub — the option is not exposed after connect).
#[no_mangle]
pub unsafe extern "C" fn ts_net_socket_set_keepalive(
    _handle_id: TsVal, _enable: TsVal, _initial_delay: TsVal,
) -> TsVal {
    UNDEFINED
}

/// Return 4 for IPv4, 6 for IPv6, 0 otherwise.
#[no_mangle]
pub unsafe extern "C" fn ts_net_is_ip(str_val: TsVal) -> TsVal {
    let s = val_to_string(str_val).unwrap_or_default();
    if s.parse::<std::net::Ipv4Addr>().is_ok() { return TsVal::from_i32(4); }
    if s.parse::<std::net::Ipv6Addr>().is_ok() { return TsVal::from_i32(6); }
    TsVal::from_i32(0)
}

// ── TCP server (unchanged) ────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn ts_net_server_listen(port: TsVal, connection_handler: TsVal) -> TsVal {
    let port_num = val_to_i32(port).max(0) as u16;
    ts_retain_val(connection_handler);
    let handler_raw = connection_handler.0;

    release_js_lock();

    get_runtime().block_on(async move {
        let addr = format!("0.0.0.0:{}", port_num);
        let listener = TcpListener::bind(&addr)
            .await
            .expect("ts_net_server_listen: failed to bind port");
        eprintln!("TCP server listening on {}", addr);
        loop {
            let (stream, peer_addr) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => { eprintln!("net accept error: {e}"); continue; }
            };
            let remote_addr_str = peer_addr.ip().to_string();
            let remote_port_num = peer_addr.port() as i32;
            let local_addr_str = stream.local_addr().map(|a| a.to_string()).unwrap_or_default();
            let local_port_num = stream.local_addr().map(|a| a.port() as i32).unwrap_or(port_num as i32);
            let mut buf = Vec::new();
            let mut s = stream;
            let _ = s.read_to_end(&mut buf).await;
            let data = String::from_utf8_lossy(&buf).into_owned();
            get_runtime().spawn_blocking(move || unsafe {
                acquire_js_lock();
                let handler = TsVal(handler_raw);
                let socket_obj = ts_obj_new();
                set_str_prop(socket_obj, b"remoteAddress\0", &remote_addr_str);
                set_str_prop(socket_obj, b"localAddress\0", &local_addr_str);
                ts_obj_set(socket_obj, b"remotePort\0".as_ptr() as *const i8, TsVal::from_i32(remote_port_num));
                ts_obj_set(socket_obj, b"localPort\0".as_ptr() as *const i8, TsVal::from_i32(local_port_num));
                set_str_prop(socket_obj, b"data\0", &data);
                let result = ts_func_call1(handler, socket_obj);
                ts_release_val(result); ts_release_val(socket_obj);
                release_js_lock();
            }).await.ok();
        }
    });
    UNDEFINED
}

/// Legacy connect (non-streaming).  Kept for backward compat with old shim.
#[no_mangle]
pub unsafe extern "C" fn ts_net_connect(port: TsVal, host: TsVal, connect_cb: TsVal) -> TsVal {
    let port_num = val_to_i32(port).max(0) as u16;
    let host_str = val_to_string(host).unwrap_or_else(|| "localhost".to_string());
    ts_retain_val(connect_cb);
    let cb_raw = connect_cb.0;
    get_runtime().spawn(async move {
        let addr = format!("{}:{}", host_str, port_num);
        match tokio::net::TcpStream::connect(&addr).await {
            Ok(_) => {
                get_runtime().spawn_blocking(move || unsafe {
                    acquire_js_lock();
                    let cb = TsVal(cb_raw);
                    let r = ts_func_call1(cb, UNDEFINED);
                    ts_release_val(r); ts_release_val(cb);
                    release_js_lock();
                });
            }
            Err(e) => {
                eprintln!("ts_net_connect: {e}");
                get_runtime().spawn_blocking(move || unsafe {
                    acquire_js_lock();
                    let cb = TsVal(cb_raw);
                    ts_release_val(cb);
                    release_js_lock();
                });
            }
        }
    });
    UNDEFINED
}

// ── Helpers ───────────────────────────────────────────────────────────────────

unsafe fn set_str_prop(obj: TsVal, key: &[u8], val: &str) {
    let v = new_string(val);
    ts_obj_set(obj, key.as_ptr() as *const i8, v);
    ts_release_val(v);
}

/// Convert a TsVal (Buffer class instance, raw TsBuffer, or String) to bytes.
unsafe fn tsval_to_bytes(val: TsVal) -> Option<Vec<u8>> {
    use crate::value::heap_tag;
    if !val.is_ptr() { return None; }
    let tag = heap_tag(val);
    if tag == 17 {
        // Raw TsBuffer
        let b = &*(val.as_ptr() as *const super::buffer::TsBuffer);
        return Some(b.data.clone());
    }
    if tag == 2 {
        // TsString
        use crate::value::TsString;
        let s = &*(val.as_ptr() as *const TsString);
        return Some(s.inner.as_bytes().to_vec());
    }
    if tag == 0 {
        // TsObject — might be a Buffer class instance with `_buf` property
        use crate::value::ts_obj_get;
        let inner = ts_obj_get(val, b"_buf\0".as_ptr() as *const i8);
        if inner.is_ptr() && heap_tag(inner) == 17 {
            let b = &*(inner.as_ptr() as *const super::buffer::TsBuffer);
            let bytes = b.data.clone();
            crate::value::ts_release_val(inner);
            return Some(bytes);
        }
        if inner != crate::value::UNDEFINED { crate::value::ts_release_val(inner); }
    }
    None
}
