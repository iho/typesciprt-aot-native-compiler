//! Node.js `crypto` module — cryptographic utilities.

use crate::value::{TsVal, heap_tag, ts_retain_val};
use crate::alloc::ts_alloc_rc;
use super::{new_string, val_to_string, val_to_i32};
use super::buffer::{TsBuffer, HEAP_TAG_BUFFER};
use sha2::{Sha256, Sha512, Digest};
use sha1::Sha1;
use md5::Md5;
use hmac::{Hmac, Mac};
use rand::Rng;
use uuid::Uuid;
use pbkdf2::pbkdf2_hmac;
use scrypt::{scrypt, Params as ScryptParams};

// STRING heap tag constant (tag 2)
const HEAP_TAG_STRING: u8 = 2;

type HmacSha256 = Hmac<Sha256>;
type HmacSha512 = Hmac<Sha512>;
type HmacSha1   = Hmac<Sha1>;

#[no_mangle]
pub unsafe extern "C" fn ts_crypto_random_uuid() -> TsVal {
    new_string(&Uuid::new_v4().to_string())
}

#[no_mangle]
pub unsafe extern "C" fn ts_crypto_random_bytes_hex(size: TsVal) -> TsVal {
    let n = val_to_i32(size).max(0) as usize;
    let mut bytes = vec![0u8; n];
    rand::thread_rng().fill(&mut bytes[..]);
    new_string(&hex::encode(&bytes))
}

/// Returns a TsBuffer filled with `size` random bytes.
#[no_mangle]
pub unsafe extern "C" fn ts_crypto_random_bytes(size: TsVal) -> TsVal {
    let n = val_to_i32(size).max(0) as usize;
    let mut bytes = vec![0u8; n];
    rand::thread_rng().fill(&mut bytes[..]);
    alloc_buffer_val(bytes)
}

/// Single-shot hash: ts_crypto_hash_sync(algorithm, data, encoding) → string
#[no_mangle]
pub unsafe extern "C" fn ts_crypto_hash_sync(algorithm: TsVal, data: TsVal, encoding: TsVal) -> TsVal {
    let algo = val_to_string(algorithm).unwrap_or_default().to_lowercase();
    let input = val_to_string(data).unwrap_or_default();
    let enc   = val_to_string(encoding).unwrap_or_else(|| "hex".into()).to_lowercase();

    let hash_bytes: Vec<u8> = match algo.as_str() {
        "sha256" => { let mut h = Sha256::new(); h.update(input.as_bytes()); h.finalize().to_vec() }
        "sha512" => { let mut h = Sha512::new(); h.update(input.as_bytes()); h.finalize().to_vec() }
        "sha1"   => { let mut h = Sha1::new();   h.update(input.as_bytes()); h.finalize().to_vec() }
        "md5"    => { let mut h = Md5::new();     h.update(input.as_bytes()); h.finalize().to_vec() }
        _        => { let mut h = Sha256::new(); h.update(input.as_bytes()); h.finalize().to_vec() }
    };

    new_string(&encode_hash(&hash_bytes, &enc))
}

/// Single-shot HMAC: ts_crypto_hmac_sync(algorithm, key, data, encoding) → string
#[no_mangle]
pub unsafe extern "C" fn ts_crypto_hmac_sync(algorithm: TsVal, key: TsVal, data: TsVal, encoding: TsVal) -> TsVal {
    let algo  = val_to_string(algorithm).unwrap_or_default().to_lowercase();
    let key_s = val_to_string(key).unwrap_or_default();
    let input = val_to_string(data).unwrap_or_default();
    let enc   = val_to_string(encoding).unwrap_or_else(|| "hex".into()).to_lowercase();

    let hash_bytes: Vec<u8> = match algo.as_str() {
        "sha512" => {
            let mut mac = HmacSha512::new_from_slice(key_s.as_bytes()).unwrap();
            mac.update(input.as_bytes()); mac.finalize().into_bytes().to_vec()
        }
        "sha1" => {
            let mut mac = HmacSha1::new_from_slice(key_s.as_bytes()).unwrap();
            mac.update(input.as_bytes()); mac.finalize().into_bytes().to_vec()
        }
        _ => { // default sha256
            let mut mac = HmacSha256::new_from_slice(key_s.as_bytes()).unwrap();
            mac.update(input.as_bytes()); mac.finalize().into_bytes().to_vec()
        }
    };

    new_string(&encode_hash(&hash_bytes, &enc))
}

fn encode_hash(bytes: &[u8], enc: &str) -> String {
    match enc {
        "base64" => base64_encode(bytes),
        _        => hex::encode(bytes),
    }
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

/// Helper: extract raw bytes from a TsVal that is either a TsString (tag 2) or a TsBuffer (tag 17).
unsafe fn val_to_bytes(v: TsVal) -> Vec<u8> {
    if !v.is_ptr() { return vec![]; }
    let tag = heap_tag(v);
    if tag == HEAP_TAG_STRING {
        let s = &*(v.as_ptr() as *const crate::value::TsString);
        s.inner.as_bytes().to_vec()
    } else if tag == HEAP_TAG_BUFFER {
        let b = &*(v.as_ptr() as *const TsBuffer);
        b.data.clone()
    } else {
        vec![]
    }
}

/// Allocate a new Buffer TsVal from a Vec<u8>.
unsafe fn alloc_buffer_val(data: Vec<u8>) -> TsVal {
    let size = std::mem::size_of::<TsBuffer>();
    let ptr = ts_alloc_rc(size, HEAP_TAG_BUFFER);
    std::ptr::write(ptr as *mut TsBuffer, TsBuffer { data });
    TsVal::from_ptr(ptr)
}

/// PBKDF2 key derivation.
/// ts_crypto_pbkdf2_sync(password, salt, iterations, keylen, digest) -> Buffer
#[no_mangle]
pub unsafe extern "C" fn ts_crypto_pbkdf2_sync(
    password: TsVal,
    salt: TsVal,
    iterations: TsVal,
    keylen: TsVal,
    digest: TsVal,
) -> TsVal {
    let pass_bytes = val_to_bytes(password);
    let salt_bytes = val_to_bytes(salt);
    let iters = val_to_i32(iterations).max(1) as u32;
    let klen  = val_to_i32(keylen).max(1) as usize;
    let algo  = val_to_string(digest).unwrap_or_else(|| "sha256".into()).to_lowercase();
    let mut dk = vec![0u8; klen];
    match algo.as_str() {
        "sha512" => pbkdf2_hmac::<Sha512>(&pass_bytes, &salt_bytes, iters, &mut dk),
        "sha1"   => pbkdf2_hmac::<Sha1>(&pass_bytes, &salt_bytes, iters, &mut dk),
        _        => pbkdf2_hmac::<Sha256>(&pass_bytes, &salt_bytes, iters, &mut dk),
    }
    alloc_buffer_val(dk)
}

/// scrypt key derivation.
/// ts_crypto_scrypt_sync(password, salt, keylen, options) -> Buffer
/// options is a TsObject with optional fields: N (default 16384), r (default 8), p (default 1).
#[no_mangle]
pub unsafe extern "C" fn ts_crypto_scrypt_sync(
    password: TsVal,
    salt: TsVal,
    keylen: TsVal,
    options: TsVal,
) -> TsVal {
    let pass_bytes = val_to_bytes(password);
    let salt_bytes = val_to_bytes(salt);
    let klen = val_to_i32(keylen).max(1) as usize;

    // Extract N, r, p from options TsObject (heap tag 0).
    let (n_log2, r, p) = if options.is_ptr() && heap_tag(options) == 0 {
        let obj = &*(options.as_ptr() as *const crate::value::TsObject);
        let get_u32 = |key: &str, default: u32| -> u32 {
            obj.properties.get(key).map(|&v| val_to_i32(v).max(1) as u32).unwrap_or(default)
        };
        let n_val = get_u32("N", 16384);
        // log2 of N
        let mut log2 = 0u8;
        let mut tmp = n_val;
        while tmp > 1 { tmp >>= 1; log2 += 1; }
        (log2, get_u32("r", 8), get_u32("p", 1))
    } else {
        (14u8, 8u32, 1u32) // defaults: N=2^14=16384, r=8, p=1
    };

    let params = ScryptParams::new(n_log2, r, p, klen).unwrap_or_else(|_| {
        ScryptParams::new(14, 8, 1, klen).unwrap()
    });
    let mut dk = vec![0u8; klen];
    let _ = scrypt(&pass_bytes, &salt_bytes, &params, &mut dk);
    alloc_buffer_val(dk)
}

/// Constant-time comparison of two Buffers.
/// Returns false if lengths differ; uses XOR accumulator for constant-time compare.
#[no_mangle]
pub unsafe extern "C" fn ts_crypto_timing_safe_equal(a: TsVal, b: TsVal) -> TsVal {
    if !a.is_ptr() || heap_tag(a) != HEAP_TAG_BUFFER
        || !b.is_ptr() || heap_tag(b) != HEAP_TAG_BUFFER
    {
        return TsVal::from_bool(false);
    }
    let ba = &*(a.as_ptr() as *const TsBuffer);
    let bb = &*(b.as_ptr() as *const TsBuffer);
    if ba.data.len() != bb.data.len() {
        return TsVal::from_bool(false);
    }
    let mut acc: u8 = 0;
    for (&x, &y) in ba.data.iter().zip(bb.data.iter()) {
        acc |= x ^ y;
    }
    TsVal::from_bool(acc == 0)
}

/// Fill a Buffer in-place with cryptographically random bytes.
/// Returns the same buffer (retaining ownership).
#[no_mangle]
pub unsafe extern "C" fn ts_crypto_random_fill_sync(buffer: TsVal) -> TsVal {
    if buffer.is_ptr() && heap_tag(buffer) == HEAP_TAG_BUFFER {
        let b = &mut *(buffer.as_ptr() as *mut TsBuffer);
        rand::thread_rng().fill(&mut b.data[..]);
        ts_retain_val(buffer);
    }
    buffer
}
