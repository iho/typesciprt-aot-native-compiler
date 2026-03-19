use super::{TsVal, TsWeakRef, UNDEFINED, ts_retain_val, ts_release_val, heap_tag};

pub unsafe extern "C" fn ts_weakref_destructor(ptr: *mut u8) {
    let wr = &mut *(ptr as *mut TsWeakRef);
    ts_release_val(wr.target);
    std::ptr::drop_in_place(ptr as *mut TsWeakRef);
}

/// `new WeakRef(target)` — holds a strong reference (our ARC system has no true weak refs).
#[no_mangle]
pub unsafe extern "C" fn ts_weakref_new(target: TsVal) -> TsVal {
    let size = std::mem::size_of::<TsWeakRef>();
    let ptr = crate::alloc::ts_alloc_rc(size, 15) as *mut TsWeakRef;
    if ptr.is_null() { return UNDEFINED; }
    ts_retain_val(target);
    std::ptr::write(ptr, TsWeakRef { target });
    TsVal::from_ptr(ptr as *mut u8)
}

/// `wr.deref()` — returns the target value (or undefined if not a WeakRef).
#[no_mangle]
pub unsafe extern "C" fn ts_weakref_deref(wr_val: TsVal) -> TsVal {
    if !wr_val.is_ptr() || heap_tag(wr_val) != 15 {
        return UNDEFINED;
    }
    let wr = &*(wr_val.as_ptr() as *const TsWeakRef);
    ts_retain_val(wr.target);
    wr.target
}
