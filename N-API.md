# Node-API (N-API) Implementation Plan

## What This Enables

Supporting N-API lets the compiler load native npm packages (`.node` shared
libraries) such as `bcrypt`, `better-sqlite3`, `sharp`, `canvas`, and any
`node-gyp`-built addon — without Node.js.

## Direction

`napi_bridge.rs` (existing) goes **out**: it exposes compiled TypeScript *as*
a Node.js `.node` addon so Node.js can `require()` it.

This file covers the **in** direction: implementing the `napi_*` C functions
ourselves so that a `.node` addon can call them and get TsVal-backed results.

## Key Concepts

| N-API concept | Mapping in this runtime |
|---|---|
| `napi_env` | `*mut NapiEnv` — heap-allocated per loaded module |
| `napi_value` | `*mut TsVal` — pointer to a TsVal slot owned by a handle scope |
| `napi_handle_scope` | `*mut HandleScope` — arena of `Box<TsVal>` slots; released on close |
| `napi_ref` | `Box<NapiRef>` as raw pointer — persistent ref outside any scope |
| `napi_callback_info` | Struct holding `this`, arg slice, data pointer |
| `TsNapiFunction` (tag 18) | Heap object storing `(napi_callback, data, env)` |

### `napi_value` lifetime rules
- Every `napi_*` function that creates a value allocates a `Box<TsVal>`, retains
  the inner TsVal, and registers the box with the current handle scope.
- When the scope closes all boxes are released (inner TsVal released, box dropped).
- `napi_ref` values live outside scopes and must be explicitly deleted.

## Loading flow

```
require('native-pkg')
  → resolve_local_import finds pkg/build/Release/native.node
  → ts_napi_load("…/native.node")
      dlopen(path)
      dlsym → napi_register_module_v1
      allocate NapiEnv + root HandleScope
      create empty TsObject as exports napi_value
      call napi_register_module_v1(env, exports_napi)
        → addon calls napi_create_function, napi_set_named_property, …
      extract TsObject from exports napi_value
      push scope close → release addon napi_values
      register TsObject with ts_cjs_register_ns(pkg_name)
```

## Implementation: 30 most-used functions

Located in `crates/ts-runtime/src/napi/mod.rs` (always compiled, no feature
flag — the functions must be present in the binary so dlopen'd addons can find
them via dynamic linker symbol resolution).

### Group 1 — primitive value creation/reading (12 functions)
| Function | What it does |
|---|---|
| `napi_get_undefined` | alloc slot = UNDEFINED |
| `napi_get_null` | alloc slot = NULL |
| `napi_get_boolean` | alloc slot = TRUE / FALSE |
| `napi_create_int32` | alloc slot = TsVal::from_i32 |
| `napi_create_uint32` | alloc slot = TsVal::from_i32(v as i32) |
| `napi_create_double` | alloc slot = TsVal::from_f64 |
| `napi_create_string_utf8` | alloc slot = ts_string_new |
| `napi_typeof` | inspect TsVal tag → napi_valuetype |
| `napi_get_value_int32` | as_i32 |
| `napi_get_value_uint32` | as_i32 cast to u32 |
| `napi_get_value_double` | as_f64 |
| `napi_get_value_string_utf8` | copy TsString bytes into caller buffer |
| `napi_get_value_bool` | as_bool |
| `napi_coerce_to_string` | ts_coerce_string → alloc slot |
| `napi_coerce_to_number` | ts_coerce_number → alloc slot |

### Group 2 — object / array (8 functions)
| Function | What it does |
|---|---|
| `napi_create_object` | ts_obj_new → alloc slot |
| `napi_create_array` | ts_arr_new(0) → alloc slot |
| `napi_create_array_with_length` | ts_arr_new(n) → alloc slot |
| `napi_get_named_property` | ts_obj_get → alloc slot |
| `napi_set_named_property` | ts_obj_set |
| `napi_get_array_length` | ts_arr_len |
| `napi_get_element` | ts_arr_get → alloc slot |
| `napi_set_element` | ts_arr_set |
| `napi_is_array` | heap_tag == 1 |
| `napi_get_property_names` | ts_obj_keys → alloc slot |
| `napi_has_own_property` | ts_obj_has |

### Group 3 — functions and calls (3 functions)
| Function | What it does |
|---|---|
| `napi_create_function` | alloc TsNapiFunction (tag 18), wrap in slot |
| `napi_call_function` | dispatch_callback with converted args |
| `napi_new_instance` | allocate object, call constructor |

### Group 4 — handle scopes and refs (5 functions)
| Function | What it does |
|---|---|
| `napi_open_handle_scope` | push new scope on NapiEnv stack |
| `napi_close_handle_scope` | pop scope, release all TsVal slots |
| `napi_create_reference` | Box the TsVal independently of scopes |
| `napi_delete_reference` | release and drop the Box |
| `napi_get_reference_value` | alloc slot = clone of ref's TsVal |

### Group 5 — errors (2 functions)
| Function | What it does |
|---|---|
| `napi_throw_error` | set NapiEnv.pending_exception |
| `napi_is_exception_pending` | check NapiEnv.pending_exception |
| `napi_get_and_clear_last_exception` | take pending_exception → alloc slot |

## New heap tag

| Tag | Type |
|---|---|
| 18 | `TsNapiFunction` — stores `(napi_callback, data: *mut c_void, env: *mut NapiEnv)` |

`dispatch_callback` in `func.rs` checks for tag 18 and calls the N-API callback
instead of the normal transmuted function pointer path.

## Files changed

| File | Change |
|---|---|
| `crates/ts-runtime/src/napi/mod.rs` | **new** — all 30+ napi_* implementations |
| `crates/ts-runtime/src/lib.rs` | `pub mod napi;` |
| `crates/ts-runtime/src/value/mod.rs` | tag 18 destructor in ts_release_val |
| `crates/ts-runtime/src/value/func.rs` | tag-18 branch in dispatch_callback |
| `crates/ts-runtime/src/value/globals.rs` | `ts_napi_load(path)` |
| `crates/ts-codegen/src/lowering/mod.rs` | resolve `.node` files → ts_napi_load |

## Remaining work (after top-30)

- `napi_wrap` / `napi_unwrap` — C++ class binding pattern (`__napi_tag` hidden field)
- `napi_define_properties` — property descriptor support (getters/setters)
- `napi_create_async_work` / `napi_queue_async_work` — Tokio integration
- `napi_create_threadsafe_function` — cross-thread TsVal ownership
- `napi_create_buffer` / buffer typed arrays — extend TsBuffer (tag 17)
- `napi_create_external` — opaque data pointer in a TsVal
- Error propagation — N-API pending-exception model across callback boundaries
- `napi_adjust_external_memory` — no-op or approximate ARC hint
- Symbol support — `napi_create_symbol` → TsSymbol (tag 10)
- BigInt — `napi_create_bigint_*` (not currently supported)
