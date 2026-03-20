//! Node.js `Buffer` class (heap tag 17).

use crate::alloc::ts_alloc_rc;
use crate::value::{TsVal, UNDEFINED, heap_tag, ts_retain_val, ts_release_val, TsArray};
use crate::value::array::{ts_arr_new, ts_arr_push};
use super::{new_string, val_to_string, val_to_i32};

pub const HEAP_TAG_BUFFER: u8 = 17;

pub struct TsBuffer {
    pub data: Vec<u8>,
}

fn alloc_buffer(data: Vec<u8>) -> TsVal {
    let size = std::mem::size_of::<TsBuffer>();
    unsafe {
        let ptr = ts_alloc_rc(size, HEAP_TAG_BUFFER);
        std::ptr::write(ptr as *mut TsBuffer, TsBuffer { data });
        TsVal::from_ptr(ptr)
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_destructor(ptr: *mut u8) {
    std::ptr::drop_in_place(ptr as *mut TsBuffer);
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_from_string(str_val: TsVal, encoding: TsVal) -> TsVal {
    let s = val_to_string(str_val).unwrap_or_default();
    let enc = val_to_string(encoding).unwrap_or_else(|| "utf8".into()).to_lowercase();
    let data = match enc.as_str() {
        "hex"    => hex::decode(&s).unwrap_or_default(),
        "base64" => base64_decode(&s),
        _        => s.into_bytes(),
    };
    alloc_buffer(data)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_from_array(arr: TsVal) -> TsVal {
    let mut data = vec![];
    if arr.is_ptr() {
        let a = &*(arr.as_ptr() as *const TsArray);
        for &v in &a.elements { data.push((val_to_i32(v) & 0xFF) as u8); }
    }
    alloc_buffer(data)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_alloc(size: TsVal, fill: TsVal) -> TsVal {
    let n = val_to_i32(size).max(0) as usize;
    let fill_byte = if fill.is_int32() { (fill.as_i32() & 0xFF) as u8 } else { 0 };
    alloc_buffer(vec![fill_byte; n])
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_alloc_unsafe(size: TsVal) -> TsVal {
    let n = val_to_i32(size).max(0) as usize;
    alloc_buffer(vec![0u8; n])
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_concat(list: TsVal, _total_length: TsVal) -> TsVal {
    let mut data = vec![];
    if list.is_ptr() {
        let arr = &*(list.as_ptr() as *const TsArray);
        for &v in &arr.elements {
            if v.is_ptr() && heap_tag(v) == HEAP_TAG_BUFFER {
                let buf = &*(v.as_ptr() as *const TsBuffer);
                data.extend_from_slice(&buf.data);
            }
        }
    }
    alloc_buffer(data)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_to_string(buf: TsVal, encoding: TsVal) -> TsVal {
    if !buf.is_ptr() || heap_tag(buf) != HEAP_TAG_BUFFER { return new_string(""); }
    let b = &*(buf.as_ptr() as *const TsBuffer);
    let enc = val_to_string(encoding).unwrap_or_else(|| "utf8".into()).to_lowercase();
    let s = match enc.as_str() {
        "hex"    => hex::encode(&b.data),
        "base64" => base64_encode(&b.data),
        _        => String::from_utf8_lossy(&b.data).into_owned(),
    };
    new_string(&s)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_length(buf: TsVal) -> TsVal {
    if !buf.is_ptr() || heap_tag(buf) != HEAP_TAG_BUFFER { return TsVal::from_i32(0); }
    let b = &*(buf.as_ptr() as *const TsBuffer);
    TsVal::from_i32(b.data.len() as i32)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_slice(buf: TsVal, start: TsVal, end: TsVal) -> TsVal {
    if !buf.is_ptr() || heap_tag(buf) != HEAP_TAG_BUFFER { return alloc_buffer(vec![]); }
    let b = &*(buf.as_ptr() as *const TsBuffer);
    let len = b.data.len();
    let s = (val_to_i32(start).max(0) as usize).min(len);
    let e = if end.is_undefined() { len } else { (val_to_i32(end).max(0) as usize).min(len) };
    alloc_buffer(if s <= e { b.data[s..e].to_vec() } else { vec![] })
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_get_byte(buf: TsVal, index: TsVal) -> TsVal {
    if !buf.is_ptr() || heap_tag(buf) != HEAP_TAG_BUFFER { return UNDEFINED; }
    let b = &*(buf.as_ptr() as *const TsBuffer);
    let i = val_to_i32(index) as usize;
    if i < b.data.len() { TsVal::from_i32(b.data[i] as i32) } else { UNDEFINED }
}

fn base64_encode(data: &[u8]) -> String {
    const C: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0=chunk[0]; let b1=if chunk.len()>1{chunk[1]}else{0}; let b2=if chunk.len()>2{chunk[2]}else{0};
        out.push(C[(b0>>2)as usize]as char); out.push(C[((b0&3)<<4|b1>>4)as usize]as char);
        out.push(if chunk.len()>1{C[((b1&0xf)<<2|b2>>6)as usize]as char}else{'='});
        out.push(if chunk.len()>2{C[(b2&0x3f)as usize]as char}else{'='});
    }
    out
}

fn base64_decode(s: &str) -> Vec<u8> {
    const DECODE: [u8; 256] = {
        let mut t = [255u8; 256];
        let enc = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0usize;
        while i < 64 { t[enc[i] as usize] = i as u8; i += 1; }
        t
    };
    let bytes: Vec<u8> = s.bytes().filter(|&b| b != b'=').collect();
    let mut out = vec![];
    for chunk in bytes.chunks(4) {
        let b: Vec<u8> = chunk.iter().map(|&c| DECODE[c as usize]).collect();
        if b.len() >= 2 { out.push((b[0]<<2)|(b[1]>>4)); }
        if b.len() >= 3 { out.push((b[1]<<4)|(b[2]>>2)); }
        if b.len() >= 4 { out.push((b[2]<<6)|b[3]); }
    }
    out
}
