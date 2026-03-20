//! Node.js `os` module — operating system utilities.

use crate::value::TsVal;
use super::new_string;

#[no_mangle]
pub unsafe extern "C" fn ts_os_platform() -> TsVal {
    new_string(if cfg!(target_os="macos") { "darwin" }
               else if cfg!(target_os="windows") { "win32" }
               else { "linux" })
}

#[no_mangle]
pub unsafe extern "C" fn ts_os_homedir() -> TsVal {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/".to_string());
    new_string(&home)
}

#[no_mangle]
pub unsafe extern "C" fn ts_os_tmpdir() -> TsVal {
    let tmp = std::env::var("TMPDIR")
        .or_else(|_| std::env::var("TEMP"))
        .unwrap_or_else(|_| "/tmp".to_string());
    new_string(tmp.trim_end_matches('/'))
}

#[no_mangle]
pub unsafe extern "C" fn ts_os_hostname() -> TsVal {
    let name = std::env::var("HOSTNAME").unwrap_or_else(|_| {
        std::fs::read_to_string("/etc/hostname")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "localhost".to_string())
    });
    new_string(&name)
}

#[no_mangle]
pub unsafe extern "C" fn ts_os_eol() -> TsVal {
    new_string(if cfg!(target_os="windows") { "\r\n" } else { "\n" })
}

#[no_mangle]
pub unsafe extern "C" fn ts_os_arch() -> TsVal {
    new_string(if cfg!(target_arch="x86_64") { "x64" }
               else if cfg!(target_arch="aarch64") { "arm64" }
               else { "unknown" })
}

#[no_mangle]
pub unsafe extern "C" fn ts_os_cpus() -> TsVal {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    TsVal::from_i32(n as i32)
}
