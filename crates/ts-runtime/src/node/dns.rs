//! Node.js `dns` module — hostname resolution.

use crate::value::{TsVal, TsPromise, UNDEFINED};
use crate::value::array::{ts_arr_new, ts_arr_push};
use crate::value::promise::{get_runtime, make_promise_pair, alloc_promise, resolve_arc};
use super::new_string;
use super::val_to_string;

/// Synchronous DNS lookup: resolves hostname to first IPv4/IPv6 address.
/// ts_dns_lookup(hostname) -> string (IP address) or empty string on failure.
#[no_mangle]
pub unsafe extern "C" fn ts_dns_lookup(hostname_val: TsVal) -> TsVal {
    let hostname = val_to_string(hostname_val).unwrap_or_default();
    match std::net::ToSocketAddrs::to_socket_addrs(&format!("{}:0", hostname)) {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.next() {
                return new_string(&addr.ip().to_string());
            }
            new_string("")
        }
        Err(_) => new_string(""),
    }
}

/// Synchronous DNS resolve: returns array of addresses for the given hostname.
/// ts_dns_resolve(hostname, rrtype) -> TsArray of strings.
#[no_mangle]
pub unsafe extern "C" fn ts_dns_resolve(hostname_val: TsVal, _rrtype: TsVal) -> TsVal {
    let hostname = val_to_string(hostname_val).unwrap_or_default();
    let arr = ts_arr_new(0);
    match std::net::ToSocketAddrs::to_socket_addrs(&format!("{}:0", hostname)) {
        Ok(addrs) => {
            let mut seen = std::collections::HashSet::new();
            for addr in addrs {
                let ip = addr.ip().to_string();
                if seen.insert(ip.clone()) {
                    ts_arr_push(arr, new_string(&ip));
                }
            }
        }
        Err(_) => {}
    }
    arr
}

/// Async DNS lookup. Returns Promise<string>.
#[no_mangle]
pub unsafe extern "C" fn ts_dns_lookup_async(hostname_val: TsVal) -> TsVal {
    let hostname = val_to_string(hostname_val).unwrap_or_default();
    let (resolved, notify, blocking_notify) = make_promise_pair();
    let r2 = resolved.clone();
    let n2 = notify.clone();
    let bn2 = blocking_notify.clone();
    get_runtime().spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            match std::net::ToSocketAddrs::to_socket_addrs(&format!("{}:0", hostname)) {
                Ok(mut addrs) => addrs.next().map(|a| a.ip().to_string()).unwrap_or_default(),
                Err(_) => String::new(),
            }
        }).await.unwrap_or_default();
        resolve_arc(&r2, &n2, &bn2, unsafe { new_string(&result) });
    });
    alloc_promise(TsPromise { resolved, notify, blocking_notify })
}

/// Async DNS resolve. Returns Promise<string[]>.
#[no_mangle]
pub unsafe extern "C" fn ts_dns_resolve_async(hostname_val: TsVal, _rrtype: TsVal) -> TsVal {
    let hostname = val_to_string(hostname_val).unwrap_or_default();
    let (resolved, notify, blocking_notify) = make_promise_pair();
    let r2 = resolved.clone();
    let n2 = notify.clone();
    let bn2 = blocking_notify.clone();
    get_runtime().spawn(async move {
        let ips = tokio::task::spawn_blocking(move || {
            let mut result = Vec::new();
            let mut seen = std::collections::HashSet::new();
            if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&format!("{}:0", hostname)) {
                for addr in addrs {
                    let ip = addr.ip().to_string();
                    if seen.insert(ip.clone()) { result.push(ip); }
                }
            }
            result
        }).await.unwrap_or_default();
        let arr = unsafe {
            let a = ts_arr_new(0);
            for ip in &ips { ts_arr_push(a, new_string(ip)); }
            a
        };
        resolve_arc(&r2, &n2, &bn2, arr);
    });
    alloc_promise(TsPromise { resolved, notify, blocking_notify })
}
