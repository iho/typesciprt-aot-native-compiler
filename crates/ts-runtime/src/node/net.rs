//! Node.js `net` module — raw TCP server and client connections.
//!
//! `ts_net_server_listen(port, handler)` starts a TCP server.
//! The handler is called as `handler(socketObj)` for each incoming connection where:
//!
//!   socketObj — TsObject { remoteAddress, remotePort, localAddress, localPort }
//!
//! `ts_net_connect(port, host, cb)` connects to a TCP server and calls `cb(socketObj)`.

use crate::value::{
    TsVal, UNDEFINED, ts_retain_val, ts_release_val,
    ts_obj_new, ts_obj_set,
    ts_func_call1,
};
use crate::value::promise::get_runtime;
use super::{new_string, val_to_i32, val_to_string};

use tokio::net::TcpListener;

/// Start a TCP server on `port`, calling `connection_handler(socketObj)` for each connection.
///
/// Blocks the calling thread running the server loop.
#[no_mangle]
pub unsafe extern "C" fn ts_net_server_listen(port: TsVal, connection_handler: TsVal) -> TsVal {
    let port_num = val_to_i32(port).max(0) as u16;
    ts_retain_val(connection_handler);
    let handler_raw = connection_handler.0;

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

            let local_addr = stream.local_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let remote_addr_str = peer_addr.ip().to_string();
            let remote_port_num = peer_addr.port() as i32;
            let local_addr_str = local_addr.clone();
            let local_port_num = stream.local_addr()
                .map(|a| a.port() as i32)
                .unwrap_or(port_num as i32);

            // Read all data from the socket (simple blocking read).
            let data = {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                let mut s = stream;
                let _ = s.read_to_end(&mut buf).await;
                String::from_utf8_lossy(&buf).into_owned()
            };

            get_runtime()
                .spawn_blocking(move || unsafe {
                    let handler = TsVal(handler_raw);

                    // Build socket TsObject with address properties.
                    let socket_obj = ts_obj_new();
                    set_str_prop(socket_obj, b"remoteAddress\0", &remote_addr_str);
                    set_str_prop(socket_obj, b"localAddress\0", &local_addr_str);
                    ts_obj_set(socket_obj, b"remotePort\0".as_ptr() as *const i8,
                               TsVal::from_i32(remote_port_num));
                    ts_obj_set(socket_obj, b"localPort\0".as_ptr() as *const i8,
                               TsVal::from_i32(local_port_num));
                    set_str_prop(socket_obj, b"data\0", &data);

                    // Call connection_handler(socketObj).
                    let result = ts_func_call1(handler, socket_obj);
                    ts_release_val(result);
                    ts_release_val(socket_obj);
                })
                .await
                .ok();
        }
    });

    UNDEFINED
}

/// Connect to a TCP server at `host:port` and call `cb(socketObj)`.
///
/// Reads any initial data from the server and stores it in socketObj.data.
/// Returns UNDEFINED (the socket interaction happens via the callback).
#[no_mangle]
pub unsafe extern "C" fn ts_net_connect(port: TsVal, host: TsVal, connect_cb: TsVal) -> TsVal {
    let port_num = val_to_i32(port).max(0) as u16;
    let host_str = unsafe { val_to_string(host).unwrap_or_else(|| "localhost".to_string()) };
    ts_retain_val(connect_cb);
    let cb_raw = connect_cb.0;

    get_runtime().block_on(async move {
        use tokio::io::AsyncReadExt;

        let addr = format!("{}:{}", host_str, port_num);
        let mut stream = match tokio::net::TcpStream::connect(&addr).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("ts_net_connect: failed to connect to {}: {}", addr, e);
                // Still call cb with an empty socket object on error.
                get_runtime()
                    .spawn_blocking(move || unsafe {
                        let cb = TsVal(cb_raw);
                        let socket_obj = ts_obj_new();
                        set_str_prop(socket_obj, b"remoteAddress\0", &format!("{}", host_str));
                        ts_obj_set(socket_obj, b"remotePort\0".as_ptr() as *const i8,
                                   TsVal::from_i32(port_num as i32));
                        set_str_prop(socket_obj, b"data\0", "");
                        let result = ts_func_call1(cb, socket_obj);
                        ts_release_val(result);
                        ts_release_val(socket_obj);
                    })
                    .await
                    .ok();
                return;
            }
        };

        let local_addr = stream.local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        let local_port_num = stream.local_addr()
            .map(|a| a.port() as i32)
            .unwrap_or(0);

        // Read available data (with a short timeout to avoid blocking forever).
        let mut buf = Vec::new();
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            stream.read_to_end(&mut buf),
        ).await;
        let data = String::from_utf8_lossy(&buf).into_owned();

        let host_clone = host_str.clone();
        get_runtime()
            .spawn_blocking(move || unsafe {
                let cb = TsVal(cb_raw);

                let socket_obj = ts_obj_new();
                set_str_prop(socket_obj, b"remoteAddress\0", &host_clone);
                set_str_prop(socket_obj, b"localAddress\0", &local_addr);
                ts_obj_set(socket_obj, b"remotePort\0".as_ptr() as *const i8,
                           TsVal::from_i32(port_num as i32));
                ts_obj_set(socket_obj, b"localPort\0".as_ptr() as *const i8,
                           TsVal::from_i32(local_port_num));
                set_str_prop(socket_obj, b"data\0", &data);

                let result = ts_func_call1(cb, socket_obj);
                ts_release_val(result);
                ts_release_val(socket_obj);
            })
            .await
            .ok();
    });

    UNDEFINED
}

/// Helper: set a string property on a TsObject using a NUL-terminated byte-string key.
unsafe fn set_str_prop(obj: TsVal, key: &[u8], val: &str) {
    let v = new_string(val);
    ts_obj_set(obj, key.as_ptr() as *const i8, v);
    ts_release_val(v);
}
