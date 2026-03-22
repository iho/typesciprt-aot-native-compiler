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

/// `buf.toString(encoding, start, end)` — decode a byte range without allocating an intermediate slice.
#[no_mangle]
pub unsafe extern "C" fn ts_buffer_to_string_range(buf: TsVal, encoding: TsVal, start: TsVal, end: TsVal) -> TsVal {
    if !buf.is_ptr() || heap_tag(buf) != HEAP_TAG_BUFFER { return new_string(""); }
    let b = &*(buf.as_ptr() as *const TsBuffer);
    let len = b.data.len();
    let s = (val_to_i32(start).max(0) as usize).min(len);
    let e = if end.is_undefined() { len } else { (val_to_i32(end).max(0) as usize).min(len) };
    let slice = if s <= e { &b.data[s..e] } else { &b.data[0..0] };
    let enc = val_to_string(encoding).unwrap_or_else(|| "utf8".into()).to_lowercase();
    let result = match enc.as_str() {
        "hex"    => hex::encode(slice),
        "base64" => base64_encode(slice),
        _        => String::from_utf8_lossy(slice).into_owned(),
    };
    new_string(&result)
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

/// Helper: get raw bytes slice from a Buffer TsVal (returns empty slice if not a buffer).
unsafe fn buf_bytes(buf: TsVal) -> Option<&'static [u8]> {
    if !buf.is_ptr() || heap_tag(buf) != HEAP_TAG_BUFFER { return None; }
    let b = &*(buf.as_ptr() as *const TsBuffer);
    Some(std::slice::from_raw_parts(b.data.as_ptr(), b.data.len()))
}

unsafe fn buf_bytes_mut(buf: TsVal) -> Option<&'static mut [u8]> {
    if !buf.is_ptr() || heap_tag(buf) != HEAP_TAG_BUFFER { return None; }
    let b = &mut *(buf.as_ptr() as *mut TsBuffer);
    Some(std::slice::from_raw_parts_mut(b.data.as_mut_ptr(), b.data.len()))
}

// ── Binary read methods ───────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_u8(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_i32(0); };
    if off >= bytes.len() { return TsVal::from_i32(0); }
    TsVal::from_i32(bytes[off] as i32)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_i8(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_i32(0); };
    if off >= bytes.len() { return TsVal::from_i32(0); }
    TsVal::from_i32(bytes[off] as i8 as i32)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_u16_be(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_i32(0); };
    if off + 2 > bytes.len() { return TsVal::from_i32(0); }
    let v = u16::from_be_bytes([bytes[off], bytes[off+1]]);
    TsVal::from_i32(v as i32)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_u16_le(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_i32(0); };
    if off + 2 > bytes.len() { return TsVal::from_i32(0); }
    let v = u16::from_le_bytes([bytes[off], bytes[off+1]]);
    TsVal::from_i32(v as i32)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_i16_be(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_i32(0); };
    if off + 2 > bytes.len() { return TsVal::from_i32(0); }
    let v = i16::from_be_bytes([bytes[off], bytes[off+1]]);
    TsVal::from_i32(v as i32)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_u32_be(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_i32(0); };
    if off + 4 > bytes.len() { return TsVal::from_i32(0); }
    let v = u32::from_be_bytes([bytes[off], bytes[off+1], bytes[off+2], bytes[off+3]]);
    // Represent as f64 if > i32::MAX to preserve the unsigned range.
    if v > i32::MAX as u32 { TsVal::from_f64(v as f64) } else { TsVal::from_i32(v as i32) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_u32_le(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_i32(0); };
    if off + 4 > bytes.len() { return TsVal::from_i32(0); }
    let v = u32::from_le_bytes([bytes[off], bytes[off+1], bytes[off+2], bytes[off+3]]);
    if v > i32::MAX as u32 { TsVal::from_f64(v as f64) } else { TsVal::from_i32(v as i32) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_i32_be(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_i32(0); };
    if off + 4 > bytes.len() { return TsVal::from_i32(0); }
    let v = i32::from_be_bytes([bytes[off], bytes[off+1], bytes[off+2], bytes[off+3]]);
    TsVal::from_i32(v)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_i32_le(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_i32(0); };
    if off + 4 > bytes.len() { return TsVal::from_i32(0); }
    let v = i32::from_le_bytes([bytes[off], bytes[off+1], bytes[off+2], bytes[off+3]]);
    TsVal::from_i32(v)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_double_be(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_f64(0.0); };
    if off + 8 > bytes.len() { return TsVal::from_f64(0.0); }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&bytes[off..off+8]);
    TsVal::from_f64(f64::from_be_bytes(arr))
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_read_double_le(buf: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes(buf) else { return TsVal::from_f64(0.0); };
    if off + 8 > bytes.len() { return TsVal::from_f64(0.0); }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&bytes[off..off+8]);
    TsVal::from_f64(f64::from_le_bytes(arr))
}

// ── Binary write methods ──────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_set_byte(buf: TsVal, index: TsVal, value: TsVal) -> TsVal {
    let i = super::val_to_i32(index).max(0) as usize;
    let Some(bytes) = buf_bytes_mut(buf) else { return TsVal::from_i32(0); };
    if i < bytes.len() { bytes[i] = (super::val_to_i32(value) & 0xFF) as u8; }
    TsVal::from_i32(i as i32 + 1)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_write_u8(buf: TsVal, value: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let Some(bytes) = buf_bytes_mut(buf) else { return TsVal::from_i32(off as i32 + 1); };
    if off < bytes.len() { bytes[off] = (super::val_to_i32(value) & 0xFF) as u8; }
    TsVal::from_i32(off as i32 + 1)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_write_u16_be(buf: TsVal, value: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let v = (super::val_to_i32(value) & 0xFFFF) as u16;
    let Some(bytes) = buf_bytes_mut(buf) else { return TsVal::from_i32(off as i32 + 2); };
    if off + 2 <= bytes.len() { bytes[off..off+2].copy_from_slice(&v.to_be_bytes()); }
    TsVal::from_i32(off as i32 + 2)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_write_u16_le(buf: TsVal, value: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let v = (super::val_to_i32(value) & 0xFFFF) as u16;
    let Some(bytes) = buf_bytes_mut(buf) else { return TsVal::from_i32(off as i32 + 2); };
    if off + 2 <= bytes.len() { bytes[off..off+2].copy_from_slice(&v.to_le_bytes()); }
    TsVal::from_i32(off as i32 + 2)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_write_u32_be(buf: TsVal, value: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let v_f = if value.is_number() { value.as_f64() } else { super::val_to_i32(value) as f64 };
    let v = v_f as u32;
    let Some(bytes) = buf_bytes_mut(buf) else { return TsVal::from_i32(off as i32 + 4); };
    if off + 4 <= bytes.len() { bytes[off..off+4].copy_from_slice(&v.to_be_bytes()); }
    TsVal::from_i32(off as i32 + 4)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_write_u32_le(buf: TsVal, value: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let v_f = if value.is_number() { value.as_f64() } else { super::val_to_i32(value) as f64 };
    let v = v_f as u32;
    let Some(bytes) = buf_bytes_mut(buf) else { return TsVal::from_i32(off as i32 + 4); };
    if off + 4 <= bytes.len() { bytes[off..off+4].copy_from_slice(&v.to_le_bytes()); }
    TsVal::from_i32(off as i32 + 4)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_write_i32_be(buf: TsVal, value: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let v = super::val_to_i32(value);
    let Some(bytes) = buf_bytes_mut(buf) else { return TsVal::from_i32(off as i32 + 4); };
    if off + 4 <= bytes.len() { bytes[off..off+4].copy_from_slice(&v.to_be_bytes()); }
    TsVal::from_i32(off as i32 + 4)
}

#[no_mangle]
pub unsafe extern "C" fn ts_buffer_write_i32_le(buf: TsVal, value: TsVal, offset: TsVal) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let v = super::val_to_i32(value);
    let Some(bytes) = buf_bytes_mut(buf) else { return TsVal::from_i32(off as i32 + 4); };
    if off + 4 <= bytes.len() { bytes[off..off+4].copy_from_slice(&v.to_le_bytes()); }
    TsVal::from_i32(off as i32 + 4)
}

/// Write a string into the buffer at `offset`.  Returns number of bytes written.
#[no_mangle]
pub unsafe extern "C" fn ts_buffer_write_string(
    buf: TsVal, str_val: TsVal, offset: TsVal, _length: TsVal, encoding: TsVal,
) -> TsVal {
    let off = super::val_to_i32(offset).max(0) as usize;
    let s = super::val_to_string(str_val).unwrap_or_default();
    let enc = super::val_to_string(encoding).unwrap_or_else(|| "utf8".into()).to_lowercase();
    let src: Vec<u8> = match enc.as_str() {
        "hex"    => hex::decode(&s).unwrap_or_default(),
        "base64" => base64_decode(&s),
        _        => s.into_bytes(),
    };
    let Some(bytes) = buf_bytes_mut(buf) else { return TsVal::from_i32(0); };
    let n = src.len().min(bytes.len().saturating_sub(off));
    bytes[off..off+n].copy_from_slice(&src[..n]);
    TsVal::from_i32(n as i32)
}

/// Copy bytes from `src` buffer into `target` buffer.
/// Returns number of bytes copied.
#[no_mangle]
pub unsafe extern "C" fn ts_buffer_copy(
    src: TsVal, target: TsVal, target_start: TsVal, source_start: TsVal, source_end: TsVal,
) -> TsVal {
    let ts = super::val_to_i32(target_start).max(0) as usize;
    let ss = super::val_to_i32(source_start).max(0) as usize;

    let Some(src_bytes) = buf_bytes(src) else { return TsVal::from_i32(0); };
    let se = if source_end.is_undefined() { src_bytes.len() }
             else { (super::val_to_i32(source_end).max(0) as usize).min(src_bytes.len()) };
    if ss >= se { return TsVal::from_i32(0); }
    let chunk: Vec<u8> = src_bytes[ss..se].to_vec();

    let Some(tgt_bytes) = buf_bytes_mut(target) else { return TsVal::from_i32(0); };
    let n = chunk.len().min(tgt_bytes.len().saturating_sub(ts));
    tgt_bytes[ts..ts+n].copy_from_slice(&chunk[..n]);
    TsVal::from_i32(n as i32)
}

/// `Buffer.byteLength(str, encoding)` — byte length of string in the given encoding.
#[no_mangle]
pub unsafe extern "C" fn ts_buffer_byte_length(str_val: TsVal, encoding: TsVal) -> TsVal {
    // If it's already a Buffer, return its length.
    if str_val.is_ptr() && heap_tag(str_val) == HEAP_TAG_BUFFER {
        return ts_buffer_length(str_val);
    }
    let s = super::val_to_string(str_val).unwrap_or_default();
    let enc = super::val_to_string(encoding).unwrap_or_else(|| "utf8".into()).to_lowercase();
    let n = match enc.as_str() {
        "hex"    => s.len() / 2,
        "base64" => { let p = s.trim_end_matches('=').len(); (p * 3) / 4 }
        _        => s.len(), // utf8: byte count = char count for ASCII; close enough for sizing
    };
    TsVal::from_i32(n as i32)
}

/// Create a Buffer from raw bytes (called from net/tls runtime when socket data arrives).
pub fn alloc_buffer_pub(data: Vec<u8>) -> TsVal { alloc_buffer(data) }

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

/// Returns a TsFunction that creates a Buffer from various input types.
/// Used to provide `global.Buffer` to CJS modules.
#[no_mangle]
pub unsafe extern "C" fn ts_get_buffer_constructor() -> TsVal {
    // Return a TsFunction wrapping ts_buffer_from_val (arity=1, no closure env).
    crate::value::func::ts_closure_new(
        ts_buffer_from_val as *const u8,
        1,
        crate::value::UNDEFINED,
    )
}

/// Create a Buffer from any value: Buffer instance → copy, string → utf8 encode, array → byte array.
#[no_mangle]
pub unsafe extern "C" fn ts_buffer_from_val(val: TsVal) -> TsVal {
    use crate::value::{heap_tag, TsString, TsArray};
    if val.is_ptr() {
        let tag = heap_tag(val);
        if tag == HEAP_TAG_BUFFER {
            let b = &*(val.as_ptr() as *const TsBuffer);
            return alloc_buffer(b.data.clone());
        }
        if tag == 2 {
            let s = &*(val.as_ptr() as *const TsString);
            return alloc_buffer(s.inner.as_bytes().to_vec());
        }
        if tag == 1 {
            let arr = &*(val.as_ptr() as *const TsArray);
            let bytes: Vec<u8> = arr.elements.iter().map(|v: &TsVal| {
                if v.is_int32() { v.as_i32() as u8 } else { 0u8 }
            }).collect();
            return alloc_buffer(bytes);
        }
    }
    alloc_buffer(vec![])
}
