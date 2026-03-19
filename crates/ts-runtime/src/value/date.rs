//! TsDate: heap-allocated JavaScript Date object (tag = 14).
//!
//! Stores the date as a Unix timestamp in milliseconds (f64), matching the
//! JavaScript Date internal representation.

use super::{TsVal, TsDate, UNDEFINED};
use super::uri::rust_str_to_val;

pub unsafe extern "C" fn ts_date_destructor(ptr: *mut u8) {
    std::ptr::drop_in_place(ptr as *mut TsDate);
}

/// `new Date()` — current time.
#[no_mangle]
pub unsafe extern "C" fn ts_date_new() -> TsVal {
    let millis = current_millis_f64();
    ts_date_new_from_millis(millis)
}

/// `new Date(value)` — construct from a TsVal (number = millis, string = parsed).
#[no_mangle]
pub unsafe extern "C" fn ts_date_from_val(val: TsVal) -> TsVal {
    let millis = if val.is_number() {
        val.as_f64()
    } else if val.is_int32() {
        val.as_i32() as f64
    } else if val.is_undefined() {
        current_millis_f64()
    } else {
        // Try to parse as string; fall back to NaN
        f64::NAN
    };
    ts_date_new_from_millis(millis)
}

/// Internal: allocate a TsDate with the given milliseconds timestamp.
pub unsafe fn ts_date_new_from_millis(millis: f64) -> TsVal {
    let size = std::mem::size_of::<TsDate>();
    let ptr = crate::alloc::ts_alloc_rc(size, 14) as *mut TsDate;
    if ptr.is_null() { return UNDEFINED; }
    std::ptr::write(ptr, TsDate { millis });
    TsVal::from_ptr(ptr as *mut u8)
}

/// `Date.now()` — returns milliseconds since Unix epoch as a TsVal number.
#[no_mangle]
pub unsafe extern "C" fn ts_date_now() -> TsVal {
    TsVal::from_f64(current_millis_f64())
}

/// `date.getTime()` — milliseconds since epoch.
#[no_mangle]
pub unsafe extern "C" fn ts_date_get_time(date: TsVal) -> TsVal {
    TsVal::from_f64(read_millis(date))
}

/// `date.getFullYear()` — local year (UTC year for simplicity).
#[no_mangle]
pub unsafe extern "C" fn ts_date_get_full_year(date: TsVal) -> TsVal {
    let dt = millis_to_components(read_millis(date));
    TsVal::from_i32(dt.year)
}

/// `date.getMonth()` — 0-based month (0=January).
#[no_mangle]
pub unsafe extern "C" fn ts_date_get_month(date: TsVal) -> TsVal {
    let dt = millis_to_components(read_millis(date));
    TsVal::from_i32(dt.month - 1)
}

/// `date.getDate()` — day of the month (1-31).
#[no_mangle]
pub unsafe extern "C" fn ts_date_get_date(date: TsVal) -> TsVal {
    let dt = millis_to_components(read_millis(date));
    TsVal::from_i32(dt.day)
}

/// `date.getDay()` — day of the week (0=Sunday).
#[no_mangle]
pub unsafe extern "C" fn ts_date_get_day(date: TsVal) -> TsVal {
    // Unix epoch (1970-01-01) was a Thursday = 4
    let total_days = (read_millis(date) / 86400000.0).floor() as i64;
    TsVal::from_i32((((total_days % 7) + 4) % 7) as i32)
}

/// `date.getHours()` — hours (0-23).
#[no_mangle]
pub unsafe extern "C" fn ts_date_get_hours(date: TsVal) -> TsVal {
    let dt = millis_to_components(read_millis(date));
    TsVal::from_i32(dt.hours)
}

/// `date.getMinutes()` — minutes (0-59).
#[no_mangle]
pub unsafe extern "C" fn ts_date_get_minutes(date: TsVal) -> TsVal {
    let dt = millis_to_components(read_millis(date));
    TsVal::from_i32(dt.minutes)
}

/// `date.getSeconds()` — seconds (0-59).
#[no_mangle]
pub unsafe extern "C" fn ts_date_get_seconds(date: TsVal) -> TsVal {
    let dt = millis_to_components(read_millis(date));
    TsVal::from_i32(dt.seconds)
}

/// `date.getMilliseconds()` — milliseconds (0-999).
#[no_mangle]
pub unsafe extern "C" fn ts_date_get_milliseconds(date: TsVal) -> TsVal {
    let ms = read_millis(date);
    TsVal::from_i32((ms % 1000.0) as i32)
}

/// `date.toISOString()` — e.g. "2024-01-15T12:34:56.789Z".
#[no_mangle]
pub unsafe extern "C" fn ts_date_to_iso_string(date: TsVal) -> TsVal {
    let ms = read_millis(date);
    let dt = millis_to_components(ms);
    let frac = (ms % 1000.0) as u32;
    let s = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year, dt.month, dt.day, dt.hours, dt.minutes, dt.seconds, frac
    );
    rust_str_to_val(s)
}

/// `date.toLocaleDateString()` — simplified locale date, e.g. "1/15/2024".
#[no_mangle]
pub unsafe extern "C" fn ts_date_to_locale_date_string(date: TsVal) -> TsVal {
    let dt = millis_to_components(read_millis(date));
    let s = format!("{}/{}/{}", dt.month, dt.day, dt.year);
    rust_str_to_val(s)
}

/// `date.toLocaleTimeString()` — simplified, e.g. "12:34:56 PM".
#[no_mangle]
pub unsafe extern "C" fn ts_date_to_locale_time_string(date: TsVal) -> TsVal {
    let dt = millis_to_components(read_millis(date));
    let period = if dt.hours < 12 { "AM" } else { "PM" };
    let h12 = if dt.hours % 12 == 0 { 12 } else { dt.hours % 12 };
    let s = format!("{}:{:02}:{:02} {}", h12, dt.minutes, dt.seconds, period);
    rust_str_to_val(s)
}

/// `date.toString()` / `date.toLocaleString()` — ISO-ish string.
#[no_mangle]
pub unsafe extern "C" fn ts_date_to_string(date: TsVal) -> TsVal {
    ts_date_to_iso_string(date)
}

// ── helpers ────────────────────────────────────────────────────────────────────

fn current_millis_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

unsafe fn read_millis(date: TsVal) -> f64 {
    use super::heap_tag;
    if date.is_ptr() && heap_tag(date) == 14 {
        (*(date.as_ptr() as *const TsDate)).millis
    } else {
        0.0
    }
}

struct DateComponents {
    year: i32,
    month: i32, // 1-based
    day: i32,
    hours: i32,
    minutes: i32,
    seconds: i32,
}

/// Convert milliseconds since Unix epoch to UTC calendar components.
/// Uses a simple Proleptic Gregorian calendar algorithm.
fn millis_to_components(ms: f64) -> DateComponents {
    let secs_total = (ms / 1000.0).floor() as i64;
    let seconds = ((secs_total % 86400 + 86400) % 86400) as i32;
    let hours   = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs    = seconds % 60;

    // Days since epoch (1970-01-01)
    let mut days = (ms / 86400000.0).floor() as i64;

    // Shift epoch to 1 March 0000 for easier leap-year maths
    days += 719468;
    let era    = (if days >= 0 { days } else { days - 146096 }) / 146097;
    let doe    = days - era * 146097; // day of era [0, 146096]
    let yoe    = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era [0, 399]
    let y      = yoe + era * 400;
    let doy    = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp     = (5 * doy + 2) / 153; // month of year [0, 11] (March=0)
    let d      = doy - (153 * mp + 2) / 5 + 1; // day [1, 31]
    let m      = if mp < 10 { mp + 3 } else { mp - 9 }; // month [1, 12]
    let yr     = if m <= 2 { y + 1 } else { y };

    DateComponents {
        year:    yr as i32,
        month:   m as i32,
        day:     d as i32,
        hours,
        minutes,
        seconds: secs,
    }
}
