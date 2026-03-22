//! Node.js `events` module — EventEmitter (heap tag 16).

use crate::alloc::ts_alloc_rc;
use crate::value::{TsVal, UNDEFINED, heap_tag, ts_retain_val, ts_release_val, TsArray};
use crate::value::array::{ts_arr_new, ts_arr_push};
use crate::value::func::{ts_func_call1, ts_func_call2};
use super::val_to_string;
use rustc_hash::FxHashMap;

pub const HEAP_TAG_EVENT_EMITTER: u8 = 16;

pub struct TsEventEmitter {
    pub listeners: FxHashMap<String, Vec<TsVal>>,
    pub once_listeners: FxHashMap<String, Vec<TsVal>>,
}

#[no_mangle]
pub unsafe extern "C" fn ts_event_emitter_destructor(ptr: *mut u8) {
    let ee = &mut *(ptr as *mut TsEventEmitter);
    for (_, ls) in ee.listeners.drain() { for l in ls { ts_release_val(l); } }
    for (_, ls) in ee.once_listeners.drain() { for l in ls { ts_release_val(l); } }
    std::ptr::drop_in_place(ee);
}

#[no_mangle]
pub unsafe extern "C" fn ts_event_emitter_new() -> TsVal {
    let size = std::mem::size_of::<TsEventEmitter>();
    let ptr = ts_alloc_rc(size, HEAP_TAG_EVENT_EMITTER);
    std::ptr::write(ptr as *mut TsEventEmitter, TsEventEmitter {
        listeners: FxHashMap::default(),
        once_listeners: FxHashMap::default(),
    });
    TsVal::from_ptr(ptr)
}

#[no_mangle]
pub unsafe extern "C" fn ts_event_emitter_on(emitter: TsVal, event: TsVal, listener: TsVal) -> TsVal {
    if !emitter.is_ptr() || heap_tag(emitter) != HEAP_TAG_EVENT_EMITTER { return emitter; }
    let ee = &mut *(emitter.as_ptr() as *mut TsEventEmitter);
    let ev = val_to_string(event).unwrap_or_default();
    ts_retain_val(listener);
    ee.listeners.entry(ev).or_default().push(listener);
    ts_retain_val(emitter);
    emitter
}

#[no_mangle]
pub unsafe extern "C" fn ts_event_emitter_once(emitter: TsVal, event: TsVal, listener: TsVal) -> TsVal {
    if !emitter.is_ptr() || heap_tag(emitter) != HEAP_TAG_EVENT_EMITTER { return emitter; }
    let ee = &mut *(emitter.as_ptr() as *mut TsEventEmitter);
    let ev = val_to_string(event).unwrap_or_default();
    ts_retain_val(listener);
    ee.once_listeners.entry(ev).or_default().push(listener);
    ts_retain_val(emitter);
    emitter
}

#[no_mangle]
pub unsafe extern "C" fn ts_event_emitter_off(emitter: TsVal, event: TsVal, listener: TsVal) -> TsVal {
    if !emitter.is_ptr() || heap_tag(emitter) != HEAP_TAG_EVENT_EMITTER { return emitter; }
    let ee = &mut *(emitter.as_ptr() as *mut TsEventEmitter);
    let ev = val_to_string(event).unwrap_or_default();
    if let Some(ls) = ee.listeners.get_mut(&ev) {
        if let Some(pos) = ls.iter().position(|&l| l == listener) {
            let removed = ls.remove(pos);
            ts_release_val(removed);
        }
    }
    ts_retain_val(emitter);
    emitter
}

/// emit(event, arg) — takes a single extra arg for simplicity (most events pass one value)
#[no_mangle]
pub unsafe extern "C" fn ts_event_emitter_emit(emitter: TsVal, event: TsVal, arg: TsVal) -> TsVal {
    if !emitter.is_ptr() || heap_tag(emitter) != HEAP_TAG_EVENT_EMITTER { return TsVal::from_bool(false); }
    let ee = &mut *(emitter.as_ptr() as *mut TsEventEmitter);
    let ev = val_to_string(event).unwrap_or_default();
    let mut called = false;

    if let Some(ls) = ee.listeners.get(&ev).cloned() {
        for listener in ls {
            if arg.is_undefined() { ts_func_call1(listener, UNDEFINED); }
            else { ts_func_call1(listener, arg); }
            called = true;
        }
    }
    if let Some(ls) = ee.once_listeners.remove(&ev) {
        for listener in ls {
            if arg.is_undefined() { ts_func_call1(listener, UNDEFINED); }
            else { ts_func_call1(listener, arg); }
            ts_release_val(listener);
            called = true;
        }
    }
    TsVal::from_bool(called)
}

#[no_mangle]
pub unsafe extern "C" fn ts_event_emitter_remove_all_listeners(emitter: TsVal, event: TsVal) -> TsVal {
    if !emitter.is_ptr() || heap_tag(emitter) != HEAP_TAG_EVENT_EMITTER { return emitter; }
    let ee = &mut *(emitter.as_ptr() as *mut TsEventEmitter);
    if event.is_undefined() || !event.is_ptr() {
        for (_, ls) in ee.listeners.drain() { for l in ls { ts_release_val(l); } }
        for (_, ls) in ee.once_listeners.drain() { for l in ls { ts_release_val(l); } }
    } else {
        let ev = val_to_string(event).unwrap_or_default();
        if let Some(ls) = ee.listeners.remove(&ev) { for l in ls { ts_release_val(l); } }
        if let Some(ls) = ee.once_listeners.remove(&ev) { for l in ls { ts_release_val(l); } }
    }
    ts_retain_val(emitter);
    emitter
}

#[no_mangle]
pub unsafe extern "C" fn ts_event_emitter_listeners(emitter: TsVal, event: TsVal) -> TsVal {
    let arr = ts_arr_new(0);
    if !emitter.is_ptr() || heap_tag(emitter) != HEAP_TAG_EVENT_EMITTER { return arr; }
    let ee = &*(emitter.as_ptr() as *const TsEventEmitter);
    let ev = val_to_string(event).unwrap_or_default();
    if let Some(ls) = ee.listeners.get(&ev) {
        for &l in ls {
            ts_retain_val(l);
            ts_arr_push(arr, l);
            ts_release_val(l);
        }
    }
    arr
}
