//! TsArray: heap-allocated TypeScript arrays and their operations.

use super::{TsVal, TsArray, TsString, UNDEFINED, NULL, TRUE, FALSE, heap_tag, ts_retain_val, ts_release_val};
use super::string_val::{ts_string_new, ts_val_to_string};
use super::func::dispatch_callback;

pub unsafe extern "C" fn ts_arr_destructor(ptr: *mut u8) {
    let arr_ptr = ptr as *mut TsArray;
    for val in (*arr_ptr).elements.drain(..) {
        ts_release_val(val);
    }
    std::ptr::drop_in_place(arr_ptr);
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_new(capacity: i32) -> TsVal {
    let size = std::mem::size_of::<TsArray>();
    let ptr = crate::alloc::ts_alloc_rc(size, 1) as *mut TsArray; // tag 1 = Array
    if ptr.is_null() {
        return NULL;
    }
    std::ptr::write(ptr, TsArray {
        elements: Vec::with_capacity(capacity as usize),
    });
    // Initialize with undefined.
    for _ in 0..capacity {
        (&mut *ptr).elements.push(UNDEFINED);
    }
    TsVal::from_ptr(ptr as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_get(arr_val: TsVal, idx: i32) -> TsVal {
    let ptr = arr_val.as_ptr();
    if !ptr.is_null() && idx >= 0 {
        let arr = ptr as *mut TsArray;
        let idx = idx as usize;
        if idx < (&*arr).elements.len() {
            let val = (&*arr).elements[idx];
            ts_retain_val(val);
            return val;
        }
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_set(arr_val: TsVal, idx: i32, val: TsVal) {
    let ptr = arr_val.as_ptr();
    if !ptr.is_null() && idx >= 0 {
        let arr = ptr as *mut TsArray;
        let idx = idx as usize;

        // ARC: Retain new value.
        ts_retain_val(val);

        if idx < (&*arr).elements.len() {
            let old_val = std::mem::replace(&mut (&mut *arr).elements[idx], val);
            ts_release_val(old_val);
        } else {
            // A real JS array would resize/pad.
            if idx == (&*arr).elements.len() {
                (&mut *arr).elements.push(val);
            } else {
                // Pad with undefined.
                while (&*arr).elements.len() < idx {
                    (&mut *arr).elements.push(UNDEFINED);
                }
                (&mut *arr).elements.push(val);
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_len(arr_val: TsVal) -> TsVal {
    let ptr = arr_val.as_ptr();
    if !ptr.is_null() {
        let arr = ptr as *mut TsArray;
        return TsVal::from_i32((&*arr).elements.len() as i32);
    }
    TsVal::from_i32(0)
}

/// Append `val` to the end of `arr`. Returns the new length.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_push(arr_val: TsVal, val: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
        ts_retain_val(val);
        arr.elements.push(val);
        return TsVal::from_i32(arr.elements.len() as i32);
    }
    TsVal::from_i32(0)
}

/// Remove and return the last element (or `undefined`).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_pop(arr_val: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
        if let Some(val) = arr.elements.pop() {
            return val; // Transfer ownership — ref count already belongs to caller.
        }
    }
    UNDEFINED
}

/// Append all elements of `src` to `dst` (implements `[...src, ...]`).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_push_all(dst: TsVal, src: TsVal) {
    if !dst.is_ptr() || heap_tag(dst) != 1 { return; }
    if !src.is_ptr() || heap_tag(src) != 1 { return; }
    let src_arr = &*(src.as_ptr() as *const TsArray);
    let dst_arr = &mut *(dst.as_ptr() as *mut TsArray);
    for &val in &src_arr.elements {
        ts_retain_val(val);
        dst_arr.elements.push(val);
    }
}

/// Join array elements with `sep` between each (returns a string TsVal).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_join(arr_val: TsVal, sep_val: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &*(arr_val.as_ptr() as *const TsArray);
        let sep = if sep_val.is_ptr() && heap_tag(sep_val) == 2 {
            let s = &*(sep_val.as_ptr() as *const TsString);
            s.inner.clone()
        } else {
            ",".to_string()
        };
        let parts: Vec<String> = arr.elements.iter().map(|&v| {
            if v.is_int32() { v.as_i32().to_string() }
            else if v.is_bool() { v.as_bool().to_string() }
            else if v.is_null() || v.is_undefined() { String::new() }
            else if v.is_ptr() && heap_tag(v) == 2 {
                let s = &*(v.as_ptr() as *const TsString);
                s.inner.clone()
            } else { String::new() }
        }).collect();
        let result = parts.join(&sep);
        let mut bytes = result.into_bytes();
        bytes.push(0u8);
        return ts_string_new(bytes.as_ptr() as *const i8);
    }
    ts_string_new(b"\0".as_ptr() as *const i8)
}

/// Returns the index of `search` in `arr` (or -1).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_index_of(arr_val: TsVal, search: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &*(arr_val.as_ptr() as *const TsArray);
        for (i, &val) in arr.elements.iter().enumerate() {
            if super::operators::ts_val_strict_eq(val, search) != 0 {
                return TsVal::from_i32(i as i32);
            }
        }
    }
    TsVal::from_i32(-1)
}

/// Returns a new TsArray containing elements from index `start` to the end.
/// Used for array destructuring rest: `const [a, b, ...rest] = arr`.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_rest(arr: TsVal, start: i32) -> TsVal {
    if !arr.is_ptr() || heap_tag(arr) != 1 {
        return ts_arr_new(0);
    }
    let src = &*(arr.as_ptr() as *const TsArray);
    let len = src.elements.len() as i32;
    let from = start.max(0) as usize;
    let new_len = ((len - start).max(0)) as i32;
    let result = ts_arr_new(new_len);
    for i in 0..(new_len as usize) {
        let val = src.elements[from + i];
        ts_retain_val(val);
        ts_arr_set(result, i as i32, val);
    }
    result
}

// ── Array higher-order methods ────────────────────────────────────────────────

/// Internal: check if a TsVal is truthy (non-zero, non-false, non-null, non-undefined).
pub(super) unsafe fn ts_val_is_truthy(val: TsVal) -> bool {
    if val.is_int32() { return val.as_i32() != 0; }
    if val.is_number() { let f = val.as_f64(); return f != 0.0 && !f.is_nan(); }
    if val.is_bool()   { return val.as_bool(); }
    if val.is_null() || val.is_undefined() { return false; }
    if val.is_ptr()    {
        if heap_tag(val) == 2 {
            // Empty string is falsy
            let s_ptr = val.as_ptr() as *const TsString;
            return !(&*s_ptr).inner.is_empty();
        }
        return true; // objects/arrays/functions are truthy
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_map(arr: TsVal, callback: TsVal) -> TsVal {
    if !arr.is_ptr() || heap_tag(arr) != 1 { return ts_arr_new(0); }
    let arr_ptr = arr.as_ptr() as *const TsArray;
    let len = (*arr_ptr).elements.len();
    let result = ts_arr_new(len as i32);
    for i in 0..len {
        let elem = { let r = &*arr_ptr; r.elements[i] };
        ts_retain_val(elem);
        let index = TsVal::from_i32(i as i32);
        ts_retain_val(arr);
        let mapped = dispatch_callback(callback, &[elem, index, arr]);
        ts_release_val(elem);
        ts_release_val(arr);
        ts_arr_set(result, i as i32, mapped);
        ts_release_val(mapped);
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_filter(arr: TsVal, callback: TsVal) -> TsVal {
    if !arr.is_ptr() || heap_tag(arr) != 1 { return ts_arr_new(0); }
    let arr_ptr = arr.as_ptr() as *const TsArray;
    let len = (*arr_ptr).elements.len();
    let result = ts_arr_new(0);
    for i in 0..len {
        let elem = { let r = &*arr_ptr; r.elements[i] };
        ts_retain_val(elem);
        let index = TsVal::from_i32(i as i32);
        ts_retain_val(arr);
        let keep = dispatch_callback(callback, &[elem, index, arr]);
        ts_release_val(arr);
        let truthy = ts_val_is_truthy(keep);
        ts_release_val(keep);
        if truthy {
            ts_arr_push(result, elem);
        }
        ts_release_val(elem);
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_for_each(arr: TsVal, callback: TsVal) -> TsVal {
    eprintln!("[DEBUG] ts_arr_for_each: arr={:016x}", arr.0);
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        eprintln!("[DEBUG] ts_arr_for_each: len={}", len);
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            eprintln!("[DEBUG] ts_arr_for_each: calling dispatch_callback for elem[{}]={:016x}", i, elem.0);
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let result = dispatch_callback(callback, &[elem, index, arr]);
            eprintln!("[DEBUG] ts_arr_for_each: dispatch_callback returned");
            ts_release_val(arr);
            ts_release_val(elem);
            ts_release_val(result);
        }
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_reduce(arr: TsVal, callback: TsVal, init: TsVal) -> TsVal {
    ts_retain_val(init);
    let mut acc = init;
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let new_acc = dispatch_callback(callback, &[acc, elem, index, arr]);
            ts_release_val(arr);
            ts_release_val(elem);
            ts_release_val(acc);
            acc = new_acc;
        }
    }
    acc
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_find(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let found = dispatch_callback(callback, &[elem, index, arr]);
            ts_release_val(arr);
            let truthy = ts_val_is_truthy(found);
            ts_release_val(found);
            if truthy {
                return elem; // already retained
            }
            ts_release_val(elem);
        }
    }
    UNDEFINED
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_find_index(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let found = dispatch_callback(callback, &[elem, index, arr]);
            ts_release_val(arr);
            ts_release_val(elem);
            let truthy = ts_val_is_truthy(found);
            ts_release_val(found);
            if truthy {
                return TsVal::from_i32(i as i32);
            }
        }
    }
    TsVal::from_i32(-1)
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_some(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let result = dispatch_callback(callback, &[elem, index, arr]);
            ts_release_val(arr);
            ts_release_val(elem);
            let truthy = ts_val_is_truthy(result);
            ts_release_val(result);
            if truthy { return TRUE; }
        }
    }
    FALSE
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_every(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            ts_retain_val(arr);
            let result = dispatch_callback(callback, &[elem, index, arr]);
            ts_release_val(arr);
            ts_release_val(elem);
            let truthy = ts_val_is_truthy(result);
            ts_release_val(result);
            if !truthy { return FALSE; }
        }
    }
    TRUE
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_sort(arr: TsVal, comparator: TsVal) -> TsVal {
    if !arr.is_ptr() || heap_tag(arr) != 1 { return arr; }
    let arr_ptr = arr.as_ptr() as *mut TsArray;
    let len = (*arr_ptr).elements.len();
    // Simple insertion sort to avoid Rust's sort closures requiring &mut issues
    for i in 1..len {
        let mut j = i;
        while j > 0 {
            let a = { let r = &*arr_ptr; r.elements[j - 1] };
            let b = { let r = &*arr_ptr; r.elements[j] };
            ts_retain_val(a);
            ts_retain_val(b);
            let cmp_result = if comparator.is_ptr() && heap_tag(comparator) == 4 {
                dispatch_callback(comparator, &[a, b])
            } else {
                // Default: lexicographic string comparison
                let sa = ts_val_to_string(a);
                let sb = ts_val_to_string(b);
                let cmp = (*(sa.as_ptr() as *const TsString)).inner
                    .cmp(&(*(sb.as_ptr() as *const TsString)).inner) as i32;
                ts_release_val(sa);
                ts_release_val(sb);
                TsVal::from_i32(cmp)
            };
            ts_release_val(a);
            ts_release_val(b);
            let should_swap = if cmp_result.is_int32() {
                cmp_result.as_i32() > 0
            } else {
                super::operators::ts_val_to_f64_raw(cmp_result) > 0.0
            };
            ts_release_val(cmp_result);
            if should_swap {
                (*arr_ptr).elements.swap(j - 1, j);
                j -= 1;
            } else {
                break;
            }
        }
    }
    ts_retain_val(arr);
    arr
}

#[no_mangle]
pub unsafe extern "C" fn ts_arr_flat_map(arr: TsVal, callback: TsVal) -> TsVal {
    if !arr.is_ptr() || heap_tag(arr) != 1 { return ts_arr_new(0); }
    let result = ts_arr_new(0);
    let arr_ptr = arr.as_ptr() as *const TsArray;
    let len = (*arr_ptr).elements.len();
    for i in 0..len {
        let elem = { let r = &*arr_ptr; r.elements[i] };
        ts_retain_val(elem);
        let index = TsVal::from_i32(i as i32);
        ts_retain_val(arr);
        let mapped = dispatch_callback(callback, &[elem, index, arr]);
        ts_release_val(arr);
        ts_release_val(elem);
        if mapped.is_ptr() && heap_tag(mapped) == 1 {
            // flatten one level
            let inner_ptr = mapped.as_ptr() as *const TsArray;
            let inner_len = (&*inner_ptr).elements.len();
            for k in 0..inner_len {
                let v = { let r = &*inner_ptr; r.elements[k] };
                ts_arr_push(result, v);
            }
        } else {
            ts_arr_push(result, mapped);
        }
        ts_release_val(mapped);
    }
    result
}

/// `arr.flat([depth])` — flatten nested arrays up to `depth` levels (default 1).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_flat(arr: TsVal, depth: i32) -> TsVal {
    let result = ts_arr_new(0);
    if !arr.is_ptr() || heap_tag(arr) != 1 {
        return result;
    }
    unsafe fn flatten(src: TsVal, result: TsVal, depth: i32) {
        if !src.is_ptr() || heap_tag(src) != 1 { return; }
        let src_ptr = src.as_ptr() as *const TsArray;
        let src_ref = &*src_ptr;
        let len = src_ref.elements.len();
        for i in 0..len {
            let elem = src_ref.elements[i];
            if depth > 0 && elem.is_ptr() && heap_tag(elem) == 1 {
                flatten(elem, result, depth - 1);
            } else {
                ts_arr_push(result, elem);
            }
        }
    }
    flatten(arr, result, depth);
    result
}
