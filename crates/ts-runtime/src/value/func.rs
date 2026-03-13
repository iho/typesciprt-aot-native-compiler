//! TsFunction: heap-allocated function values, closures, dispatch.

use super::{TsVal, TsFunction, UNDEFINED, NULL, heap_tag, ts_retain_val, ts_release_val};

#[no_mangle]
pub unsafe extern "C" fn ts_func_new(fn_ptr: *const u8, arity: i32) -> TsVal {
    let size = std::mem::size_of::<TsFunction>();
    let ptr = crate::alloc::ts_alloc_rc(size, 4) as *mut TsFunction; // tag 4 = Function
    if ptr.is_null() { return NULL; }
    std::ptr::write(ptr, TsFunction { fn_ptr, arity: arity as u8, env: UNDEFINED });
    TsVal::from_ptr(ptr as *mut u8)
}

/// Create a closure: a function + captured environment array.
#[no_mangle]
pub unsafe extern "C" fn ts_closure_new(fn_ptr: *const u8, arity: i32, env: TsVal) -> TsVal {
    let size = std::mem::size_of::<TsFunction>();
    let ptr = crate::alloc::ts_alloc_rc(size, 4) as *mut TsFunction;
    if ptr.is_null() { return NULL; }
    ts_retain_val(env);
    std::ptr::write(ptr, TsFunction { fn_ptr, arity: arity as u8, env });
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

/// Internal: call a TsFunction value with up to 4 TsVal arguments.
/// If the function is a closure (env ≠ UNDEFINED), passes env as the first arg.
pub(super) unsafe fn dispatch_callback(fn_val: TsVal, args: &[TsVal]) -> TsVal {
    if !fn_val.is_ptr() || heap_tag(fn_val) != 4 { return UNDEFINED; }
    let func = &*(fn_val.as_ptr() as *const TsFunction);
    let is_closure = !func.env.is_undefined();
    let env = func.env;
    let a0 = args.first().copied().unwrap_or(UNDEFINED);
    let a1 = args.get(1).copied().unwrap_or(UNDEFINED);
    let a2 = args.get(2).copied().unwrap_or(UNDEFINED);
    let a3 = args.get(3).copied().unwrap_or(UNDEFINED);
    if is_closure {
        // fn(env, arg0, arg1, ...) — env is always first
        match func.arity {
            0 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal) -> TsVal>(func.fn_ptr);
                f(env)
            }
            1 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0)
            }
            2 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0, a1)
            }
            3 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0, a1, a2)
            }
            _ => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(env, a0, a1, a2, a3)
            }
        }
    } else {
        match func.arity {
            0 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn() -> TsVal>(func.fn_ptr);
                f()
            }
            1 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal) -> TsVal>(func.fn_ptr);
                f(a0)
            }
            2 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, a1)
            }
            3 => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, a1, a2)
            }
            _ => {
                let f = std::mem::transmute::<*const u8, unsafe extern "C" fn(TsVal, TsVal, TsVal, TsVal) -> TsVal>(func.fn_ptr);
                f(a0, a1, a2, a3)
            }
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
