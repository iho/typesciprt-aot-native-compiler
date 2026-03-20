//! Extensions to the `process` global: cwd, platform, version, etc.

use crate::value::{TsVal, UNDEFINED};
use crate::value::object::{ts_obj_new, ts_obj_set};
use crate::value::ts_release_val;
use super::new_string;

#[no_mangle]
pub unsafe extern "C" fn ts_process_cwd() -> TsVal {
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    new_string(&cwd)
}

#[no_mangle]
pub unsafe extern "C" fn ts_process_platform() -> TsVal {
    new_string(if cfg!(target_os="macos") { "darwin" }
               else if cfg!(target_os="windows") { "win32" }
               else { "linux" })
}

#[no_mangle]
pub unsafe extern "C" fn ts_process_version() -> TsVal {
    new_string("v22.0.0")
}

#[no_mangle]
pub unsafe extern "C" fn ts_process_versions() -> TsVal {
    let obj = ts_obj_new();
    let node_ver = new_string("22.0.0");
    ts_obj_set(obj, "node\0".as_ptr() as *const i8, node_ver);
    ts_release_val(node_ver);
    obj
}

#[no_mangle]
pub unsafe extern "C" fn ts_process_hrtime() -> TsVal {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64).unwrap_or(0);
    // Return nanoseconds as i32 (wraps around after ~2s but sufficient for relative timing)
    TsVal::from_i32((ns % i32::MAX as i64) as i32)
}

#[no_mangle]
pub unsafe extern "C" fn ts_process_uptime() -> TsVal {
    // Approximate: use monotonic time if available
    TsVal::from_i32(0)
}
