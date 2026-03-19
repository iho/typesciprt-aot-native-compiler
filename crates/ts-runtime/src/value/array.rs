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

/// Generic iterable length: works for arrays (tag=1) and strings (tag=2).
/// Returns a NaN-boxed i32 (same as ts_arr_len).
#[no_mangle]
pub unsafe extern "C" fn ts_iterable_len(val: TsVal) -> TsVal {
    if !val.is_ptr() { return TsVal::from_i32(0); }
    match heap_tag(val) {
        1 => {
            let arr = &*(val.as_ptr() as *const TsArray);
            TsVal::from_i32(arr.elements.len() as i32)
        }
        2 => {
            let s = &*(val.as_ptr() as *const TsString);
            TsVal::from_i32(s.inner.chars().count() as i32)
        }
        _ => TsVal::from_i32(0),
    }
}

/// Generic iterable element fetch: arrays return element, strings return single-char string.
#[no_mangle]
pub unsafe extern "C" fn ts_iterable_get(val: TsVal, idx: i32) -> TsVal {
    if !val.is_ptr() || idx < 0 { return UNDEFINED; }
    match heap_tag(val) {
        1 => ts_arr_get(val, idx),
        2 => {
            let s = &*(val.as_ptr() as *const TsString);
            if let Some(c) = s.inner.chars().nth(idx as usize) {
                let mut bytes = c.to_string().into_bytes();
                bytes.push(0u8);
                super::string_val::ts_string_new(bytes.as_ptr() as *const i8)
            } else {
                UNDEFINED
            }
        }
        _ => UNDEFINED,
    }
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

/// Prepend `val` to the beginning of `arr`. Returns the new length.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_unshift(arr_val: TsVal, val: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
        ts_retain_val(val);
        arr.elements.insert(0, val);
        return TsVal::from_i32(arr.elements.len() as i32);
    }
    TsVal::from_i32(0)
}

/// Remove and return the first element (or `undefined`).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_shift(arr_val: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
        if arr.elements.is_empty() { return UNDEFINED; }
        arr.elements.remove(0) // Transfer ownership to caller.
    } else {
        UNDEFINED
    }
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
pub unsafe extern "C" fn ts_arr_reduce_right(arr: TsVal, callback: TsVal, init: TsVal) -> TsVal {
    ts_retain_val(init);
    let mut acc = init;
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in (0..len).rev() {
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
pub unsafe extern "C" fn ts_arr_find_last(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in (0..len).rev() {
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
pub unsafe extern "C" fn ts_arr_find_last_index(arr: TsVal, callback: TsVal) -> TsVal {
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in (0..len).rev() {
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

/// `Array.from(iterable, mapFn?)` — creates a new array from an array-like or iterable.
/// Handles TsArray (clone), TsString (array of char strings), and array-like objects with `.length`.
/// `map_fn` should be UNDEFINED when not provided.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_from(iterable: TsVal, map_fn: TsVal) -> TsVal {
    use super::string_val::ts_str_char_at;
    use super::{ts_val_get_key, heap_tag};
    let has_map = !map_fn.is_undefined();

    // Case 1: TsArray — iterate elements.
    if iterable.is_ptr() && heap_tag(iterable) == 1 {
        let arr_ptr = iterable.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        let result = ts_arr_new(len as i32);
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_retain_val(elem);
            let index = TsVal::from_i32(i as i32);
            let out = if has_map {
                ts_retain_val(iterable);
                let v = dispatch_callback(map_fn, &[elem, index, iterable]);
                ts_release_val(iterable);
                ts_release_val(elem);
                v
            } else {
                elem
            };
            ts_arr_set(result, i as i32, out);
            ts_release_val(out);
        }
        return result;
    }

    // Case 2: TsString — iterate Unicode chars.
    if iterable.is_ptr() && heap_tag(iterable) == 2 {
        let s_ptr = iterable.as_ptr() as *const TsString;
        let len = (*s_ptr).inner.chars().count() as i32;
        let result = ts_arr_new(len);
        for i in 0..len {
            let idx_val = TsVal::from_i32(i);
            let ch = ts_str_char_at(iterable, idx_val);
            ts_release_val(idx_val);
            let index = TsVal::from_i32(i);
            let out = if has_map {
                ts_retain_val(iterable);
                let v = dispatch_callback(map_fn, &[ch, index, iterable]);
                ts_release_val(iterable);
                ts_release_val(ch);
                v
            } else {
                ch
            };
            ts_arr_set(result, i, out);
            ts_release_val(out);
        }
        return result;
    }

    // Case 3: Array-like object with .length property.
    if iterable.is_ptr() && heap_tag(iterable) == 0 {
        let length_key = b"length\0".as_ptr() as *const i8;
        let len_val = super::object::ts_obj_get(iterable, length_key);
        let len = if len_val.is_int32() { len_val.as_i32() } else { 0 };
        ts_release_val(len_val);
        let result = ts_arr_new(len);
        for i in 0..len {
            let key = TsVal::from_i32(i);
            let elem = ts_val_get_key(iterable, key);
            ts_release_val(key);
            let index = TsVal::from_i32(i);
            let out = if has_map {
                ts_retain_val(iterable);
                let v = dispatch_callback(map_fn, &[elem, index, iterable]);
                ts_release_val(iterable);
                ts_release_val(elem);
                v
            } else {
                elem
            };
            ts_arr_set(result, i, out);
            ts_release_val(out);
        }
        return result;
    }

    // Fallback: return empty array.
    ts_arr_new(0)
}

/// `arr.concat(other)` — returns a new array with elements of `arr` followed by elements of `other`.
/// `other` can be an array (spreads its elements) or any other value (appended as-is).
/// Also handles String.prototype.concat: if `arr` is a TsString, delegates to ts_string_concat.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_concat(arr: TsVal, other: TsVal) -> TsVal {
    // String.prototype.concat: concatenate string representations
    if arr.is_ptr() && heap_tag(arr) == 2 {
        return super::string_val::ts_string_concat(arr, other);
    }
    let result = ts_arr_new(0);
    // Copy elements from arr
    if arr.is_ptr() && heap_tag(arr) == 1 {
        let arr_ptr = arr.as_ptr() as *const TsArray;
        let len = (*arr_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*arr_ptr; r.elements[i] };
            ts_arr_push(result, elem);
        }
    }
    // Append elements from other (spread if array, push otherwise)
    if other.is_ptr() && heap_tag(other) == 1 {
        let other_ptr = other.as_ptr() as *const TsArray;
        let len = (*other_ptr).elements.len();
        for i in 0..len {
            let elem = { let r = &*other_ptr; r.elements[i] };
            ts_arr_push(result, elem);
        }
    } else {
        ts_arr_push(result, other);
    }
    result
}

/// `arr.reverse()` — reverse in place, returns the array.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_reverse(arr_val: TsVal) -> TsVal {
    if arr_val.is_ptr() && heap_tag(arr_val) == 1 {
        let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
        arr.elements.reverse();
    }
    ts_retain_val(arr_val);
    arr_val
}

/// `arr.fill(value, start?, end?)` — fill arr[start..end] with value, returns arr.
/// start and end are i64 TsVals; UNDEFINED means 0 / arr.length.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_fill(arr_val: TsVal, value: TsVal, start_val: TsVal, end_val: TsVal) -> TsVal {
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 {
        ts_retain_val(arr_val);
        return arr_val;
    }
    let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
    let len = arr.elements.len() as i64;
    let start = if start_val.is_int32() { start_val.as_i32() as i64 } else if start_val.is_undefined() { 0 } else { 0 };
    let end   = if end_val.is_int32()   { end_val.as_i32()   as i64 } else if end_val.is_undefined()   { len } else { len };
    let start = start.clamp(0, len) as usize;
    let end   = end.clamp(0, len) as usize;
    ts_retain_val(value);
    for i in start..end {
        let old = arr.elements[i];
        arr.elements[i] = value;
        ts_retain_val(value);
        ts_release_val(old);
    }
    ts_release_val(value); // one extra retain from above
    ts_retain_val(arr_val);
    arr_val
}

/// `arr.splice(start, deleteCount?)` — remove deleteCount elements starting at start.
/// Returns a TsArray of removed elements. Modifies arr in place.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_splice(arr_val: TsVal, start_val: TsVal, delete_count_val: TsVal) -> TsVal {
    let removed = ts_arr_new(0);
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 {
        return removed;
    }
    let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
    let len = arr.elements.len() as i64;
    let start = if start_val.is_int32() { start_val.as_i32() as i64 } else { 0 };
    let start = start.clamp(0, len) as usize;
    let delete_count = if delete_count_val.is_int32() {
        (delete_count_val.as_i32() as i64).clamp(0, len - start as i64) as usize
    } else if delete_count_val.is_undefined() {
        len as usize - start
    } else {
        0
    };
    let drained: Vec<TsVal> = arr.elements.drain(start..start + delete_count).collect();
    for val in drained {
        ts_arr_push(removed, val);
        ts_release_val(val); // ts_arr_push retains; release our local ownership
    }
    removed
}

/// `arr.slice(start?, end?)` — returns a shallow copy of arr[start..end].
#[no_mangle]
pub unsafe extern "C" fn ts_arr_slice_range(arr_val: TsVal, start_val: TsVal, end_val: TsVal) -> TsVal {
    let result = ts_arr_new(0);
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 {
        return result;
    }
    let arr = &*(arr_val.as_ptr() as *const TsArray);
    let len = arr.elements.len() as i64;
    let start = if start_val.is_int32() { start_val.as_i32() as i64 } else if start_val.is_undefined() { 0 } else { 0 };
    let end   = if end_val.is_int32()   { end_val.as_i32()   as i64 } else if end_val.is_undefined()   { len } else { len };
    let start = if start < 0 { (len + start).max(0) } else { start.min(len) } as usize;
    let end   = if end < 0   { (len + end).max(0)   } else { end.min(len)   } as usize;
    for i in start..end {
        ts_arr_push(result, arr.elements[i]);
    }
    result
}

/// `arr.includes(val)` — returns true if val is in the array (strict equality).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_includes(arr_val: TsVal, val: TsVal) -> TsVal {
    use super::operators::ts_val_strict_eq;
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 {
        return TsVal::from_bool(false);
    }
    let arr = &*(arr_val.as_ptr() as *const TsArray);
    TsVal::from_bool(arr.elements.iter().any(|&e| ts_val_strict_eq(e, val) != 0))
}

/// `arr.lastIndexOf(val)` — returns the last index of val (-1 if not found).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_last_index_of(arr_val: TsVal, val: TsVal) -> TsVal {
    use super::operators::ts_val_strict_eq;
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 {
        return TsVal::from_i32(-1);
    }
    let arr = &*(arr_val.as_ptr() as *const TsArray);
    for i in (0..arr.elements.len()).rev() {
        if ts_val_strict_eq(arr.elements[i], val) != 0 {
            return TsVal::from_i32(i as i32);
        }
    }
    TsVal::from_i32(-1)
}

/// `arr.copyWithin(target, start?, end?)` — copies elements within the array.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_copy_within(arr_val: TsVal, target_val: TsVal, start_val: TsVal, end_val: TsVal) -> TsVal {
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 {
        ts_retain_val(arr_val);
        return arr_val;
    }
    let arr = &mut *(arr_val.as_ptr() as *mut TsArray);
    let len = arr.elements.len() as i64;
    let mut target = if target_val.is_int32() { target_val.as_i32() as i64 } else { 0 };
    let mut start  = if start_val.is_int32()  { start_val.as_i32()  as i64 } else { 0 };
    let mut end    = if end_val.is_int32()    { end_val.as_i32()    as i64 } else { len };
    if target < 0 { target = (len + target).max(0); }
    if start < 0  { start  = (len + start).max(0);  }
    if end < 0    { end    = (len + end).max(0);    }
    let target = target.min(len) as usize;
    let start = start.min(len) as usize;
    let end = end.min(len) as usize;
    let count = (end - start).min(len as usize - target);
    // Copy
    let snapshot: Vec<TsVal> = arr.elements[start..start + count].to_vec();
    for (i, &val) in snapshot.iter().enumerate() {
        let old = arr.elements[target + i];
        arr.elements[target + i] = val;
        ts_retain_val(val);
        ts_release_val(old);
    }
    ts_retain_val(arr_val);
    arr_val
}

/// `arr.toSorted(comparator?)` — returns a sorted copy without mutating original.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_to_sorted(arr_val: TsVal, comparator: TsVal) -> TsVal {
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 { return ts_arr_new(0); }
    let src = &*(arr_val.as_ptr() as *const TsArray);
    let copy = ts_arr_new(src.elements.len() as i32);
    for (i, &val) in src.elements.iter().enumerate() {
        ts_arr_set(copy, i as i32, val);
    }
    ts_arr_sort(copy, comparator)
}

/// `arr.toReversed()` — returns a reversed copy without mutating original.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_to_reversed(arr_val: TsVal) -> TsVal {
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 { return ts_arr_new(0); }
    let src = &*(arr_val.as_ptr() as *const TsArray);
    let copy = ts_arr_new(src.elements.len() as i32);
    let len = src.elements.len();
    for (i, &val) in src.elements.iter().enumerate() {
        ts_arr_set(copy, (len - 1 - i) as i32, val);
    }
    copy
}

/// `arr.with(index, value)` — returns a copy with arr[index] replaced by value.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_with(arr_val: TsVal, idx_val: TsVal, value: TsVal) -> TsVal {
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 { return ts_arr_new(0); }
    let src = &*(arr_val.as_ptr() as *const TsArray);
    let len = src.elements.len() as i32;
    let idx = if idx_val.is_int32() {
        let i = idx_val.as_i32();
        if i < 0 { (len + i).max(0) as usize } else { i.min(len - 1).max(0) as usize }
    } else { return ts_arr_new(0); };
    let copy = ts_arr_new(len);
    for (i, &val) in src.elements.iter().enumerate() {
        if i == idx {
            ts_arr_set(copy, i as i32, value);
        } else {
            ts_arr_set(copy, i as i32, val);
        }
    }
    copy
}

/// `arr.keys()` — returns array of integer indices [0, 1, 2, ...].
#[no_mangle]
pub unsafe extern "C" fn ts_arr_keys(arr_val: TsVal) -> TsVal {
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 { return ts_arr_new(0); }
    let src = &*(arr_val.as_ptr() as *const TsArray);
    let len = src.elements.len();
    let result = ts_arr_new(len as i32);
    for i in 0..len {
        ts_arr_set(result, i as i32, TsVal::from_i32(i as i32));
    }
    result
}

/// `arr.values()` — returns a shallow copy of the array (array of its values).
#[no_mangle]
pub unsafe extern "C" fn ts_arr_values(arr_val: TsVal) -> TsVal {
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 { return ts_arr_new(0); }
    let src = &*(arr_val.as_ptr() as *const TsArray);
    let len = src.elements.len();
    let result = ts_arr_new(len as i32);
    for (i, &val) in src.elements.iter().enumerate() {
        ts_arr_set(result, i as i32, val);
    }
    result
}

/// `arr.entries()` — returns array of [index, value] pairs.
#[no_mangle]
pub unsafe extern "C" fn ts_arr_entries(arr_val: TsVal) -> TsVal {
    if !arr_val.is_ptr() || heap_tag(arr_val) != 1 { return ts_arr_new(0); }
    let src = &*(arr_val.as_ptr() as *const TsArray);
    let len = src.elements.len();
    let result = ts_arr_new(len as i32);
    for (i, &val) in src.elements.iter().enumerate() {
        let pair = ts_arr_new(2);
        ts_arr_set(pair, 0, TsVal::from_i32(i as i32));
        ts_arr_set(pair, 1, val);
        ts_arr_set(result, i as i32, pair);
        ts_release_val(pair);
    }
    result
}
