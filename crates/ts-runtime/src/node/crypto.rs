//! Node.js `crypto` module — cryptographic utilities.

use crate::value::TsVal;
use super::{new_string, val_to_string, val_to_i32};
use sha2::{Sha256, Sha512, Digest};
use sha1::Sha1;
use md5::Md5;
use hmac::{Hmac, Mac};
use rand::Rng;
use uuid::Uuid;

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
