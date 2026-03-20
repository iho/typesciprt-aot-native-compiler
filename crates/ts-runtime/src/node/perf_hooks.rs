//! Node.js `perf_hooks` module — performance.now(), mark(), measure().

use crate::value::{TsVal, UNDEFINED};
use crate::value::array::{ts_arr_new, ts_arr_push};
use crate::value::object::{ts_obj_new, ts_obj_set};
use super::{new_string, val_to_string};
use std::sync::Mutex;
use std::collections::HashMap;
use std::time::Instant;

static PROCESS_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn start_time() -> &'static Instant {
    PROCESS_START.get_or_init(Instant::now)
}

// Named marks: name -> timestamp_ms
static MARKS: std::sync::OnceLock<Mutex<HashMap<String, f64>>> = std::sync::OnceLock::new();

fn marks() -> &'static Mutex<HashMap<String, f64>> {
    MARKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns high-resolution time in milliseconds since process start (like Date.now() but more precise).
#[no_mangle]
pub unsafe extern "C" fn ts_performance_now() -> TsVal {
    let elapsed = start_time().elapsed();
    let ms = elapsed.as_secs_f64() * 1000.0;
    TsVal::from_f64(ms)
}

/// Record a named performance mark.
#[no_mangle]
pub unsafe extern "C" fn ts_performance_mark(name_val: TsVal) -> TsVal {
    let name = val_to_string(name_val).unwrap_or_default();
    if !name.is_empty() {
        let elapsed = start_time().elapsed();
        let ms = elapsed.as_secs_f64() * 1000.0;
        if let Ok(mut m) = marks().lock() {
            m.insert(name, ms);
        }
    }
    UNDEFINED
}

/// Measure time between two marks (or from process start).
/// Returns a PerformanceEntry object.
#[no_mangle]
pub unsafe extern "C" fn ts_performance_measure(name_val: TsVal, start_mark_val: TsVal) -> TsVal {
    let name = val_to_string(name_val).unwrap_or_default();
    let start_mark = val_to_string(start_mark_val).unwrap_or_default();
    let now_ms = start_time().elapsed().as_secs_f64() * 1000.0;
    let start_ms = if start_mark.is_empty() {
        0.0
    } else {
        marks().lock().ok().and_then(|m| m.get(&start_mark).copied()).unwrap_or(0.0)
    };
    let duration = now_ms - start_ms;
    let entry = ts_obj_new();
    let k_name = std::ffi::CString::new("name").unwrap();
    let k_duration = std::ffi::CString::new("duration").unwrap();
    let k_start = std::ffi::CString::new("startTime").unwrap();
    let k_type = std::ffi::CString::new("entryType").unwrap();
    ts_obj_set(entry, k_name.as_ptr(), new_string(&name));
    ts_obj_set(entry, k_duration.as_ptr(), TsVal::from_f64(duration));
    ts_obj_set(entry, k_start.as_ptr(), TsVal::from_f64(start_ms));
    ts_obj_set(entry, k_type.as_ptr(), new_string("measure"));
    entry
}

/// Returns all entries with the given name.
#[no_mangle]
pub unsafe extern "C" fn ts_performance_get_entries_by_name(name_val: TsVal) -> TsVal {
    let name = val_to_string(name_val).unwrap_or_default();
    let arr = ts_arr_new(0);
    if let Ok(m) = marks().lock() {
        if let Some(&ms) = m.get(&name) {
            let entry = ts_obj_new();
            let k_name = std::ffi::CString::new("name").unwrap();
            let k_start = std::ffi::CString::new("startTime").unwrap();
            let k_type = std::ffi::CString::new("entryType").unwrap();
            ts_obj_set(entry, k_name.as_ptr(), new_string(&name));
            ts_obj_set(entry, k_start.as_ptr(), TsVal::from_f64(ms));
            ts_obj_set(entry, k_type.as_ptr(), new_string("mark"));
            ts_arr_push(arr, entry);
        }
    }
    arr
}
