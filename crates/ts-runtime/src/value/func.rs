//! TsFunction: heap-allocated function values, closures, dispatch.

use super::{TsVal, TsFunction, UNDEFINED, NULL, heap_tag, ts_retain_val, ts_release_val};

#[no_mangle]
pub unsafe extern "C" fn ts_func_new(fn_ptr: *const u8, arity: i32) -> TsVal {
    let size = std::mem::size_of::<TsFunction>();
    let ptr = crate::alloc::ts_alloc_rc(size, 4) as *mut TsFunction; // tag 4 = Function
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsFunction { fn_ptr, arity: arity as u8, has_this: 0, has_rest: 0, env: UNDEFINED });
    TsVal::from_ptr(ptr as *mut u8)
}

/// Create a non-closure function that expects `this` as its first MLIR parameter.
#[no_mangle]
pub unsafe extern "C" fn ts_func_new_this(fn_ptr: *const u8, arity: i32) -> TsVal {
    let size = std::mem::size_of::<TsFunction>();
    let ptr = crate::alloc::ts_alloc_rc(size, 4) as *mut TsFunction;
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsFunction { fn_ptr, arity: arity as u8, has_this: 1, has_rest: 0, env: UNDEFINED });
    TsVal::from_ptr(ptr as *mut u8)
}

/// Create a closure: a function + captured environment array.
#[no_mangle]
pub unsafe extern "C" fn ts_closure_new(fn_ptr: *const u8, arity: i32, env: TsVal) -> TsVal {
    let size = std::mem::size_of::<TsFunction>();
    let ptr = crate::alloc::ts_alloc_rc(size, 4) as *mut TsFunction;
    if ptr.is_null() { return NULL; }
    ts_retain_val(env);
    std::ptr::write(ptr, TsFunction { fn_ptr, arity: arity as u8, has_this: 0, has_rest: 0, env });
    TsVal::from_ptr(ptr as *mut u8)
}

/// Create a closure with a rest parameter: last MLIR param is a TsArray for excess args.
/// When called dynamically, args beyond `arity-1` are bundled into a TsArray.
#[no_mangle]
pub unsafe extern "C" fn ts_closure_new_rest(fn_ptr: *const u8, arity: i32, env: TsVal) -> TsVal {
    let size = std::mem::size_of::<TsFunction>();
    let ptr = crate::alloc::ts_alloc_rc(size, 4) as *mut TsFunction;
    if ptr.is_null() { return NULL; }
    ts_retain_val(env);
    std::ptr::write(ptr, TsFunction { fn_ptr, arity: arity as u8, has_this: 0, has_rest: 1, env });
    TsVal::from_ptr(ptr as *mut u8)
}

pub unsafe extern "C" fn ts_func_destructor(ptr: *mut u8) {
    let func_ptr = &mut *(ptr as *mut TsFunction);
    ts_release_val(func_ptr.env);
    std::ptr::drop_in_place(func_ptr as *mut TsFunction);
}

/// Get the captured environment array of a closure.
/// Returns UNDEFINED if `fn_val` is not a closure.
/// The caller receives a retained reference to the env array.
#[no_mangle]
pub unsafe extern "C" fn ts_closure_get_env(fn_val: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let func = &*(fn_val.as_ptr() as *const TsFunction);
    ts_retain_val(func.env);
    func.env
}

/// Dispatch helper for when has_rest=1: args before rest_start are regular, rest_arr is the bundled rest.
unsafe fn dispatch_callback_with_rest(
    func: &TsFunction,
    is_closure: bool,
    env: TsVal,
    args: &[TsVal],
    rest_start: usize,
    rest_arr: TsVal,
) -> TsVal {
    let a0 = args.first().copied().unwrap_or(UNDEFINED);
    let a1 = args.get(1).copied().unwrap_or(UNDEFINED);
    // arity = rest_start + 1. The last param is rest_arr.
    if is_closure {
        match rest_start {
            0 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, rest_arr)
            }
            1 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0, rest_arr)
            }
            _ => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0, a1, rest_arr)
            }
        }
    } else {
        match rest_start {
            0 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal) -> TsVal>(func.fn_ptr);
                f(rest_arr)
            }
            1 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, rest_arr)
            }
            _ => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, a1, rest_arr)
            }
        }
    }
}

/// Bundle args from index `start` onwards into a TsArray for rest parameter dispatch.
unsafe fn make_rest_array(args: &[TsVal], start: usize) -> TsVal {
    use super::array::{ts_arr_new, ts_arr_set};
    let count = if args.len() > start { args.len() - start } else { 0 };
    let arr = ts_arr_new(count as i32);
    for (i, &v) in args[start..].iter().enumerate() {
        ts_arr_set(arr, i as i32, v);
    }
    arr
}

/// Internal: call a TsFunction value with up to 4 TsVal arguments.
/// If the function is a closure (env ≠ UNDEFINED), passes env as the first arg.
/// If has_rest=1, args from index arity-1 are bundled into a TsArray for the last param.
/// Public alias used by the napi module for napi_call_function.
pub(crate) unsafe fn dispatch_callback_pub(fn_val: TsVal, args: &[TsVal]) -> TsVal {
    dispatch_callback(fn_val, args)
}

pub(super) unsafe fn dispatch_callback(fn_val: TsVal, args: &[TsVal]) -> TsVal {
    if !fn_val.is_ptr() { return UNDEFINED; }
    let tag = heap_tag(fn_val);
    // Tag 18 = TsNapiFunction: dispatch through N-API callback protocol.
    if tag == 18 {
        return crate::napi::dispatch_napi_function(fn_val, args);
    }
    if tag != 4 { return UNDEFINED; }
    let func = &*(fn_val.as_ptr() as *const TsFunction);
    let is_closure = !func.env.is_undefined();
    let env = func.env;

    // When has_rest=1, bundle args from index (arity-1) into a TsArray for the rest param.
    if func.has_rest != 0 {
        let rest_start = (func.arity as usize).saturating_sub(1);
        let rest_arr = make_rest_array(args, rest_start);
        let result = dispatch_callback_with_rest(func, is_closure, env, args, rest_start, rest_arr);
        ts_release_val(rest_arr);
        return result;
    }

    let a0 = args.first().copied().unwrap_or(UNDEFINED);
    let a1 = args.get(1).copied().unwrap_or(UNDEFINED);
    let a2 = args.get(2).copied().unwrap_or(UNDEFINED);
    let a3 = args.get(3).copied().unwrap_or(UNDEFINED);
    let a4 = args.get(4).copied().unwrap_or(UNDEFINED);
    let a5 = args.get(5).copied().unwrap_or(UNDEFINED);
    let a6 = args.get(6).copied().unwrap_or(UNDEFINED);
    if is_closure {
        match func.arity {
            0 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal) -> TsVal>(func.fn_ptr); f(env) }
            1 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr); f(env, a0) }
            2 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(env, a0, a1) }
            3 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(env, a0, a1, a2) }
            4 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(env, a0, a1, a2, a3) }
            5 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(env, a0, a1, a2, a3, a4) }
            6 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(env, a0, a1, a2, a3, a4, a5) }
            _ => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(env, a0, a1, a2, a3, a4, a5, a6) }
        }
    } else {
        match func.arity {
            0 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn() -> TsVal>(func.fn_ptr); f() }
            1 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal) -> TsVal>(func.fn_ptr); f(a0) }
            2 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr); f(a0, a1) }
            3 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(a0, a1, a2) }
            4 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(a0, a1, a2, a3) }
            5 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(a0, a1, a2, a3, a4) }
            6 => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(a0, a1, a2, a3, a4, a5) }
            _ => { let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr); f(a0, a1, a2, a3, a4, a5, a6) }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call0(fn_val: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[])
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call1(fn_val: TsVal, a: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[a])
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call2(fn_val: TsVal, a: TsVal, b: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[a, b])
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call3(fn_val: TsVal, a: TsVal, b: TsVal, c: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[a, b, c])
}

#[no_mangle]
pub unsafe extern "C" fn ts_func_call4(fn_val: TsVal, a: TsVal, b: TsVal, c: TsVal, d: TsVal) -> TsVal {
    dispatch_callback(fn_val, &[a, b, c, d])
}

/// Call a function that has `has_this=1` with an explicit `this` value and args.
pub(super) unsafe fn dispatch_method_callback(fn_val: TsVal, this_val: TsVal, args: &[TsVal]) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let func = &*(fn_val.as_ptr() as *const TsFunction);

    // When has_rest=1, bundle args from index (arity-1) into a TsArray (arity includes this).
    if func.has_rest != 0 {
        let rest_start = (func.arity as usize).saturating_sub(1);
        let rest_arr = make_rest_array(args, rest_start);
        let a0 = this_val;
        let a1 = args.first().copied().unwrap_or(UNDEFINED);
        let a2 = args.get(1).copied().unwrap_or(UNDEFINED);
        let result = match rest_start {
            0 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, rest_arr)
            }
            1 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, a1, rest_arr)
            }
            _ => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, a1, a2, rest_arr)
            }
        };
        ts_release_val(rest_arr);
        return result;
    }

    let a0 = this_val;
    let a1 = args.first().copied().unwrap_or(UNDEFINED);
    let a2 = args.get(1).copied().unwrap_or(UNDEFINED);
    let a3 = args.get(2).copied().unwrap_or(UNDEFINED);
    let a4 = args.get(3).copied().unwrap_or(UNDEFINED);
    let a5 = args.get(4).copied().unwrap_or(UNDEFINED);
    let a6 = args.get(5).copied().unwrap_or(UNDEFINED);
    let a7 = args.get(6).copied().unwrap_or(UNDEFINED);
    // non-closure function with this as first param
    match func.arity {
        0 => {
            let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal) -> TsVal>(func.fn_ptr);
            f(a0)
        }
        1 => {
            let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr);
            f(a0, a1)
        }
        2 => {
            let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
            f(a0, a1, a2)
        }
        3 => {
            let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
            f(a0, a1, a2, a3)
        }
        4 => {
            let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
            f(a0, a1, a2, a3, a4)
        }
        5 => {
            let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
            f(a0, a1, a2, a3, a4, a5)
        }
        6 => {
            let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
            f(a0, a1, a2, a3, a4, a5, a6)
        }
        _ => {
            let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
            f(a0, a1, a2, a3, a4, a5, a6, a7)
        }
    }
}

/// Call a member function with the receiver as `this`. Checks `has_this` to decide dispatch path.
#[no_mangle]
pub unsafe extern "C" fn ts_method_call0(fn_val: TsVal, obj: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let has_this = (*(fn_val.as_ptr() as *const TsFunction)).has_this;
    if has_this != 0 {
        dispatch_method_callback(fn_val, obj, &[])
    } else {
        dispatch_callback(fn_val, &[])
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_method_call1(fn_val: TsVal, obj: TsVal, a: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let has_this = (*(fn_val.as_ptr() as *const TsFunction)).has_this;
    if has_this != 0 {
        dispatch_method_callback(fn_val, obj, &[a])
    } else {
        dispatch_callback(fn_val, &[a])
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_method_call2(fn_val: TsVal, obj: TsVal, a: TsVal, b: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let has_this = (*(fn_val.as_ptr() as *const TsFunction)).has_this;
    if has_this != 0 {
        dispatch_method_callback(fn_val, obj, &[a, b])
    } else {
        dispatch_callback(fn_val, &[a, b])
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_method_call3(fn_val: TsVal, obj: TsVal, a: TsVal, b: TsVal, c: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let has_this = (*(fn_val.as_ptr() as *const TsFunction)).has_this;
    if has_this != 0 {
        dispatch_method_callback(fn_val, obj, &[a, b, c])
    } else {
        dispatch_callback(fn_val, &[a, b, c])
    }
}

#[no_mangle]
pub unsafe extern "C" fn ts_method_call4(fn_val: TsVal, obj: TsVal, a: TsVal, b: TsVal, c: TsVal, d: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let has_this = (*(fn_val.as_ptr() as *const TsFunction)).has_this;
    if has_this != 0 { dispatch_method_callback(fn_val, obj, &[a, b, c, d]) } else { dispatch_callback(fn_val, &[a, b, c, d]) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_method_call5(fn_val: TsVal, obj: TsVal, a: TsVal, b: TsVal, c: TsVal, d: TsVal, e: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let has_this = (*(fn_val.as_ptr() as *const TsFunction)).has_this;
    if has_this != 0 { dispatch_method_callback(fn_val, obj, &[a, b, c, d, e]) } else { dispatch_callback(fn_val, &[a, b, c, d, e]) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_method_call6(fn_val: TsVal, obj: TsVal, a: TsVal, b: TsVal, c: TsVal, d: TsVal, e: TsVal, f2: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let has_this = (*(fn_val.as_ptr() as *const TsFunction)).has_this;
    if has_this != 0 { dispatch_method_callback(fn_val, obj, &[a, b, c, d, e, f2]) } else { dispatch_callback(fn_val, &[a, b, c, d, e, f2]) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_method_call7(fn_val: TsVal, obj: TsVal, a: TsVal, b: TsVal, c: TsVal, d: TsVal, e: TsVal, f2: TsVal, g: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let has_this = (*(fn_val.as_ptr() as *const TsFunction)).has_this;
    if has_this != 0 { dispatch_method_callback(fn_val, obj, &[a, b, c, d, e, f2, g]) } else { dispatch_callback(fn_val, &[a, b, c, d, e, f2, g]) }
}

#[no_mangle]
pub unsafe extern "C" fn ts_method_call8(fn_val: TsVal, obj: TsVal, a: TsVal, b: TsVal, c: TsVal, d: TsVal, e: TsVal, f2: TsVal, g: TsVal, h: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let has_this = (*(fn_val.as_ptr() as *const TsFunction)).has_this;
    if has_this != 0 { dispatch_method_callback(fn_val, obj, &[a, b, c, d, e, f2, g, h]) } else { dispatch_callback(fn_val, &[a, b, c, d, e, f2, g, h]) }
}

/// Trampoline for bound functions: env=[orig_fn, this_val], rest=args array.
/// Called by ts_func_bind-created closures when dispatched via dispatch_callback.
unsafe extern "C" fn __ts_bound_trampoline(env: TsVal, rest: TsVal) -> TsVal {
    use super::array::ts_arr_get;
    let orig_fn = ts_arr_get(env, 0);
    let this_val = ts_arr_get(env, 1);
    let result = super::operators::ts_method_spread_call(orig_fn, this_val, rest);
    ts_release_val(orig_fn);
    ts_release_val(this_val);
    result
}

/// `fn.bind(thisArg)` — binds `thisArg` as the `this` for `fn`.
/// If `fn` is a plain function/closure (has_this=0), it doesn't use `this` anyway,
/// so we return `fn` unchanged (with a retain). For method functions (has_this=1),
/// we create a trampoline closure that forwards all args with `thisArg` as receiver.
#[no_mangle]
pub unsafe extern "C" fn ts_func_bind(fn_val: TsVal, this_val: TsVal) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 {
        return UNDEFINED;
    }
    let func = &*(fn_val.as_ptr() as *const TsFunction);
    if func.has_this == 0 {
        // Plain function or closure: bind is a no-op, return fn unchanged.
        ts_retain_val(fn_val);
        return fn_val;
    }
    // Method function: create trampoline closure [orig_fn, this_val].
    use super::array::{ts_arr_new, ts_arr_set};
    let env = ts_arr_new(2);
    ts_arr_set(env, 0, fn_val);   // retains fn_val
    ts_arr_set(env, 1, this_val); // retains this_val
    // Closure with has_rest=1, arity=1: all args are bundled into a TsArray (rest).
    let bound = ts_closure_new_rest(__ts_bound_trampoline as *const u8, 1, env);
    ts_release_val(env); // closure retains env
    bound
}
