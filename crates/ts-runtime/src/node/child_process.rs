//! Node.js `child_process` module.

use crate::value::{TsVal, TsArray, TsPromise};
use crate::value::object::{ts_obj_new, ts_obj_set};
use crate::value::promise::{get_runtime, make_promise_pair, alloc_promise, resolve_arc};
use super::{new_string, val_to_string, val_to_i32};

/// Set a string property on a TsObject.
unsafe fn obj_set_str(obj: TsVal, key: &str, val: &str) {
    let k = std::ffi::CString::new(key).unwrap();
    ts_obj_set(obj, k.as_ptr(), new_string(val));
}

/// Set an integer property on a TsObject.
unsafe fn obj_set_i32(obj: TsVal, key: &str, val: i32) {
    let k = std::ffi::CString::new(key).unwrap();
    ts_obj_set(obj, k.as_ptr(), TsVal::from_i32(val));
}

fn build_result_obj(stdout: &str, stderr: &str, status: i32, error: Option<&str>) -> TsVal {
    unsafe {
        let obj = ts_obj_new();
        obj_set_str(obj, "stdout", stdout);
        obj_set_str(obj, "stderr", stderr);
        obj_set_i32(obj, "status", status);
        if let Some(e) = error {
            obj_set_str(obj, "error", e);
        }
        obj
    }
}

/// Run a shell command synchronously.
/// Returns TsObject { stdout: string, stderr: string, status: number, error?: string }.
#[no_mangle]
pub unsafe extern "C" fn ts_exec_sync(cmd_val: TsVal) -> TsVal {
    let cmd = val_to_string(cmd_val).unwrap_or_default();
    match std::process::Command::new("sh").arg("-c").arg(&cmd).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let status = output.status.code().unwrap_or(-1);
            build_result_obj(&stdout, &stderr, status, None)
        }
        Err(e) => build_result_obj("", "", -1, Some(&e.to_string())),
    }
}

/// Run a command + args synchronously.
/// ts_spawn_sync(cmd, args_array, options) -> { stdout, stderr, status }
#[no_mangle]
pub unsafe extern "C" fn ts_spawn_sync(cmd_val: TsVal, args_val: TsVal, _options_val: TsVal) -> TsVal {
    let cmd = val_to_string(cmd_val).unwrap_or_default();
    let mut command = std::process::Command::new(&cmd);
    if args_val.is_ptr() {
        let arr = &*(args_val.as_ptr() as *const TsArray);
        for &a in &arr.elements {
            if let Some(s) = val_to_string(a) {
                command.arg(&s);
            }
        }
    }
    match command.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let status = output.status.code().unwrap_or(-1);
            build_result_obj(&stdout, &stderr, status, None)
        }
        Err(e) => build_result_obj("", "", -1, Some(&e.to_string())),
    }
}

/// Run a shell command asynchronously. Returns a Promise<{stdout, stderr, status}>.
#[no_mangle]
pub unsafe extern "C" fn ts_exec_async(cmd_val: TsVal) -> TsVal {
    let cmd = val_to_string(cmd_val).unwrap_or_default();
    let (resolved, notify) = make_promise_pair();
    let r2 = resolved.clone();
    let n2 = notify.clone();
    get_runtime().spawn(async move {
        let result = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .await;
        let obj = match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                let status = output.status.code().unwrap_or(-1);
                build_result_obj(&stdout, &stderr, status, None)
            }
            Err(e) => build_result_obj("", "", -1, Some(&e.to_string())),
        };
        resolve_arc(&r2, &n2, obj);
    });
    alloc_promise(TsPromise { resolved, notify })
}
