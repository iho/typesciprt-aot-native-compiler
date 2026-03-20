//! Node.js `fs` module — file system operations.

use crate::value::{TsVal, UNDEFINED, TsArray};
use crate::value::array::{ts_arr_new, ts_arr_push};
use crate::value::object::{ts_obj_new, ts_obj_set};
use crate::value::ts_release_val;
use crate::value::promise::ts_promise_resolve;
use super::{new_string, val_to_string};
use std::path::Path;

#[no_mangle]
pub unsafe extern "C" fn ts_fs_read_file_sync(path: TsVal, encoding: TsVal) -> TsVal {
    let p = match val_to_string(path) { Some(s) => s, None => return UNDEFINED };
    match std::fs::read(&p) {
        Ok(bytes) => {
            let enc = val_to_string(encoding).unwrap_or_default().to_lowercase();
            let content = match enc.as_str() {
                "hex"    => hex::encode(&bytes),
                "base64" => base64_encode(&bytes),
                _        => String::from_utf8_lossy(&bytes).into_owned(),
            };
            new_string(&content)
        }
        Err(_) => UNDEFINED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_write_file_sync(path: TsVal, data: TsVal) -> TsVal {
    let p = match val_to_string(path) { Some(s) => s, None => return UNDEFINED };
    let content = val_to_string(data).unwrap_or_default();
    let _ = std::fs::write(&p, content.as_bytes());
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_exists_sync(path: TsVal) -> TsVal {
    let p = match val_to_string(path) { Some(s) => s, None => return TsVal::from_bool(false) };
    TsVal::from_bool(Path::new(&p).exists())
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_mkdir_sync(path: TsVal, _options: TsVal) -> TsVal {
    let p = match val_to_string(path) { Some(s) => s, None => return UNDEFINED };
    let _ = std::fs::create_dir_all(&p);
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_readdir_sync(path: TsVal) -> TsVal {
    let p = match val_to_string(path) { Some(s) => s, None => return ts_arr_new(0) };
    let arr = ts_arr_new(0);
    if let Ok(entries) = std::fs::read_dir(&p) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let name_val = new_string(&name);
            ts_arr_push(arr, name_val);
            ts_release_val(name_val);
        }
    }
    arr
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_stat_sync(path: TsVal) -> TsVal {
    let p = match val_to_string(path) { Some(s) => s, None => return UNDEFINED };
    let Ok(meta) = std::fs::metadata(&p) else { return UNDEFINED };
    let obj = ts_obj_new();
    ts_obj_set(obj, "size\0".as_ptr() as *const i8, TsVal::from_i32(meta.len() as i32));
    ts_obj_set(obj, "isFile\0".as_ptr() as *const i8, TsVal::from_bool(meta.is_file()));
    ts_obj_set(obj, "isDirectory\0".as_ptr() as *const i8, TsVal::from_bool(meta.is_dir()));
    obj
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_unlink_sync(path: TsVal) -> TsVal {
    let p = match val_to_string(path) { Some(s) => s, None => return UNDEFINED };
    let _ = std::fs::remove_file(&p);
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_rename_sync(old: TsVal, new: TsVal) -> TsVal {
    let o = match val_to_string(old) { Some(s) => s, None => return UNDEFINED };
    let n = match val_to_string(new) { Some(s) => s, None => return UNDEFINED };
    let _ = std::fs::rename(&o, &n);
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_copy_file_sync(src: TsVal, dst: TsVal) -> TsVal {
    let s = match val_to_string(src) { Some(s) => s, None => return UNDEFINED };
    let d = match val_to_string(dst) { Some(s) => s, None => return UNDEFINED };
    let _ = std::fs::copy(&s, &d);
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_rm_sync(path: TsVal, _options: TsVal) -> TsVal {
    let p = match val_to_string(path) { Some(s) => s, None => return UNDEFINED };
    if Path::new(&p).is_dir() { let _ = std::fs::remove_dir_all(&p); }
    else { let _ = std::fs::remove_file(&p); }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_read_file_async(path: TsVal, encoding: TsVal) -> TsVal {
    let result = ts_fs_read_file_sync(path, encoding);
    crate::value::ts_retain_val(result);
    let promise = ts_promise_resolve(result);
    crate::value::ts_release_val(result);
    promise
}

#[no_mangle]
pub unsafe extern "C" fn ts_fs_write_file_async(path: TsVal, data: TsVal) -> TsVal {
    ts_fs_write_file_sync(path, data);
    ts_promise_resolve(UNDEFINED)
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
