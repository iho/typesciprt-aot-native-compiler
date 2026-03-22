//! Node.js `zlib` module — compression/decompression using flate2.

use crate::value::{TsVal, TsPromise};
use crate::value::promise::{get_runtime, make_promise_pair, alloc_promise, resolve_arc};
use super::buffer::{TsBuffer, HEAP_TAG_BUFFER};
use super::val_to_string;
use crate::value::heap_tag;
use crate::alloc::ts_alloc_rc;
use flate2::{read::DeflateDecoder, read::GzDecoder, write::DeflateEncoder, write::GzEncoder, Compression};
use std::io::{Read, Write};

/// Extract raw bytes from a TsVal (Buffer or String).
unsafe fn val_to_bytes(v: TsVal) -> Vec<u8> {
    if !v.is_ptr() { return vec![]; }
    match crate::value::heap_tag(v) {
        17 => {
            let b = &*(v.as_ptr() as *const TsBuffer);
            b.data.clone()
        }
        2 => {
            let s = &*(v.as_ptr() as *const crate::value::TsString);
            s.inner.as_bytes().to_vec()
        }
        _ => vec![],
    }
}

/// Allocate a new TsBuffer from a Vec<u8>.
unsafe fn alloc_buffer(data: Vec<u8>) -> TsVal {
    let ptr = ts_alloc_rc(std::mem::size_of::<TsBuffer>(), HEAP_TAG_BUFFER);
    std::ptr::write(ptr as *mut TsBuffer, TsBuffer { data });
    TsVal::from_ptr(ptr)
}

/// deflate (zlib) compress synchronously. Returns TsBuffer.
#[no_mangle]
pub unsafe extern "C" fn ts_zlib_deflate_sync(data_val: TsVal) -> TsVal {
    let bytes = val_to_bytes(data_val);
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    let _ = encoder.write_all(&bytes);
    let compressed = encoder.finish().unwrap_or_default();
    alloc_buffer(compressed)
}

/// deflate (zlib) decompress synchronously. Returns TsBuffer.
#[no_mangle]
pub unsafe extern "C" fn ts_zlib_inflate_sync(data_val: TsVal) -> TsVal {
    let bytes = val_to_bytes(data_val);
    let mut decoder = DeflateDecoder::new(&bytes[..]);
    let mut out = Vec::new();
    let _ = decoder.read_to_end(&mut out);
    alloc_buffer(out)
}

/// gzip compress synchronously. Returns TsBuffer.
#[no_mangle]
pub unsafe extern "C" fn ts_zlib_gzip_sync(data_val: TsVal) -> TsVal {
    let bytes = val_to_bytes(data_val);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    let _ = encoder.write_all(&bytes);
    let compressed = encoder.finish().unwrap_or_default();
    alloc_buffer(compressed)
}

/// gzip decompress synchronously. Returns TsBuffer.
#[no_mangle]
pub unsafe extern "C" fn ts_zlib_gunzip_sync(data_val: TsVal) -> TsVal {
    let bytes = val_to_bytes(data_val);
    let mut decoder = GzDecoder::new(&bytes[..]);
    let mut out = Vec::new();
    let _ = decoder.read_to_end(&mut out);
    alloc_buffer(out)
}

/// Async gzip compress. Returns Promise<Buffer>.
#[no_mangle]
pub unsafe extern "C" fn ts_zlib_gzip_async(data_val: TsVal) -> TsVal {
    let bytes = val_to_bytes(data_val);
    let (resolved, notify, blocking_notify) = make_promise_pair();
    let r2 = resolved.clone(); let n2 = notify.clone(); let bn2 = blocking_notify.clone();
    get_runtime().spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            let _ = encoder.write_all(&bytes);
            encoder.finish().unwrap_or_default()
        }).await.unwrap_or_default();
        resolve_arc(&r2, &n2, &bn2, unsafe { alloc_buffer(result) });
    });
    alloc_promise(TsPromise { resolved, notify, blocking_notify })
}

/// Async gzip decompress. Returns Promise<Buffer>.
#[no_mangle]
pub unsafe extern "C" fn ts_zlib_gunzip_async(data_val: TsVal) -> TsVal {
    let bytes = val_to_bytes(data_val);
    let (resolved, notify, blocking_notify) = make_promise_pair();
    let r2 = resolved.clone(); let n2 = notify.clone(); let bn2 = blocking_notify.clone();
    get_runtime().spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let mut decoder = GzDecoder::new(&bytes[..]);
            let mut out = Vec::new();
            let _ = decoder.read_to_end(&mut out);
            out
        }).await.unwrap_or_default();
        resolve_arc(&r2, &n2, &bn2, unsafe { alloc_buffer(result) });
    });
    alloc_promise(TsPromise { resolved, notify, blocking_notify })
}

/// Async deflate compress. Returns Promise<Buffer>.
#[no_mangle]
pub unsafe extern "C" fn ts_zlib_deflate_async(data_val: TsVal) -> TsVal {
    let bytes = val_to_bytes(data_val);
    let (resolved, notify, blocking_notify) = make_promise_pair();
    let r2 = resolved.clone(); let n2 = notify.clone(); let bn2 = blocking_notify.clone();
    get_runtime().spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            let _ = encoder.write_all(&bytes);
            encoder.finish().unwrap_or_default()
        }).await.unwrap_or_default();
        resolve_arc(&r2, &n2, &bn2, unsafe { alloc_buffer(result) });
    });
    alloc_promise(TsPromise { resolved, notify, blocking_notify })
}

/// Async deflate decompress. Returns Promise<Buffer>.
#[no_mangle]
pub unsafe extern "C" fn ts_zlib_inflate_async(data_val: TsVal) -> TsVal {
    let bytes = val_to_bytes(data_val);
    let (resolved, notify, blocking_notify) = make_promise_pair();
    let r2 = resolved.clone(); let n2 = notify.clone(); let bn2 = blocking_notify.clone();
    get_runtime().spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let mut decoder = DeflateDecoder::new(&bytes[..]);
            let mut out = Vec::new();
            let _ = decoder.read_to_end(&mut out);
            out
        }).await.unwrap_or_default();
        resolve_arc(&r2, &n2, &bn2, unsafe { alloc_buffer(result) });
    });
    alloc_promise(TsPromise { resolved, notify, blocking_notify })
}
